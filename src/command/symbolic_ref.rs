//! Implements `symbolic-ref` for reading and updating Libra's symbolic HEAD.

use std::io::Write;

use clap::Parser;
use git_internal::hash::{ObjectHash, get_hash_kind};
use sea_orm::DbErr;
use serde::Serialize;

use crate::{
    command::branch::is_valid_git_branch_name,
    internal::{
        branch::{Branch, BranchStoreError},
        db::get_db_conn_instance,
        head::Head,
        reflog::{ReflogAction, ReflogContext, ReflogError, with_reflog},
    },
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
        util,
    },
};

const HEAD_REF: &str = "HEAD";
const HEADS_PREFIX: &str = "refs/heads/";

/// `--help` examples shown in `libra symbolic-ref --help` output.
///
/// `symbolic-ref` reads or updates the symbolic target of `HEAD` (the
/// only symbolic ref Libra currently supports). The banner pins the
/// read, short-read, set, quiet, and JSON forms so users see all
/// supported forms without reading the design doc. Cross-cutting
/// `--help` EXAMPLES rollout per `docs/improvement/README.md` item B.
pub const SYMBOLIC_REF_EXAMPLES: &str = "\
EXAMPLES:
    libra symbolic-ref HEAD                       Print HEAD's symbolic target (refs/heads/<branch>)
    libra symbolic-ref --short HEAD               Print only the short branch name
    libra symbolic-ref HEAD refs/heads/main       Update HEAD to point at refs/heads/main
    libra symbolic-ref -m \"manual move\" HEAD refs/heads/main
                                                   Update HEAD and record a reflog reason
    libra symbolic-ref -q HEAD                    Suppress error output when HEAD is detached
    libra symbolic-ref --json HEAD                Structured JSON output for agents";

#[derive(Parser, Debug)]
#[command(after_help = SYMBOLIC_REF_EXAMPLES)]
pub struct SymbolicRefArgs {
    /// Suppress error output when the ref is not symbolic.
    #[clap(short = 'q', long)]
    pub quiet: bool,

    /// Print only the short branch name.
    #[clap(long)]
    pub short: bool,

    /// Delete the symbolic ref (rejected: Libra stores refs in SQLite and HEAD
    /// is its only symbolic ref).
    #[clap(short = 'd', long, conflicts_with_all = ["reason", "target", "short"], requires = "name")]
    pub delete: bool,

    /// Reflog reason to store when updating HEAD.
    #[clap(short = 'm', value_name = "REASON", requires = "target")]
    pub reason: Option<String>,

    /// Symbolic ref to read or update. Libra currently supports HEAD.
    #[clap(value_name = "NAME")]
    pub name: Option<String>,

    /// New symbolic target. Must be refs/heads/<branch>.
    #[clap(value_name = "REF")]
    pub target: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct SymbolicRefOutput {
    name: String,
    target: String,
    short: Option<String>,
    action: &'static str,
}

pub async fn execute(args: SymbolicRefArgs) -> Result<(), String> {
    execute_safe(args, &OutputConfig::default())
        .await
        .map_err(|err| err.render())
}

pub async fn execute_safe(args: SymbolicRefArgs, output: &OutputConfig) -> CliResult<()> {
    util::require_repo().map_err(|_| CliError::repo_not_found())?;
    let result = run_symbolic_ref(&args, !output.is_json()).await?;

    if output.is_json() {
        emit_json_data("symbolic-ref", &result, output)
    } else if output.quiet || result.action == "set" {
        Ok(())
    } else {
        let value = if args.short {
            result.short.as_deref().unwrap_or(result.target.as_str())
        } else {
            result.target.as_str()
        };
        write_symbolic_ref_output(value)
    }
}

async fn run_symbolic_ref(
    args: &SymbolicRefArgs,
    quiet_detached_head_is_silent: bool,
) -> CliResult<SymbolicRefOutput> {
    // `-d`/`--delete` is intentionally refused: Libra keeps refs in SQLite and
    // HEAD is the only symbolic ref, so there is no deletable symbolic ref.
    if args.delete {
        return Err(delete_symbolic_ref_error());
    }

    let name = args.name.as_deref().unwrap_or(HEAD_REF);
    validate_name(name)?;

    if let Some(target) = args.target.as_deref() {
        set_head_target(target, args.reason.as_deref()).await?;
        return Ok(SymbolicRefOutput {
            name: name.to_string(),
            target: target.to_string(),
            short: Some(branch_name_from_full_ref(target)?.to_string()),
            action: "set",
        });
    }

    let branch_name = match Head::current_result().await.map_err(map_head_error)? {
        Head::Branch(branch_name) => branch_name,
        Head::Detached(_) if args.quiet && quiet_detached_head_is_silent => {
            return Err(CliError::silent_exit(1));
        }
        Head::Detached(_) => {
            let error = CliError::failure("HEAD is not a symbolic ref")
                .with_stable_code(StableErrorCode::CliInvalidTarget);
            let error = if args.quiet {
                error.with_exit_code(1)
            } else {
                error.with_hint("switch to a branch before reading HEAD as a symbolic ref.")
            };
            return Err(error);
        }
    };

    Ok(SymbolicRefOutput {
        name: name.to_string(),
        target: format!("{HEADS_PREFIX}{branch_name}"),
        short: Some(branch_name),
        action: "read",
    })
}

fn delete_symbolic_ref_error() -> CliError {
    CliError::conflict(
        "delete symbolic ref is intentionally unsupported in Libra because SQLite storage requires a root reference",
    )
    .with_hint("use 'libra switch <branch>' or 'libra checkout <branch>' to change HEAD instead.")
}

fn validate_name(name: &str) -> CliResult<()> {
    if name == HEAD_REF {
        return Ok(());
    }

    Err(CliError::failure(format!(
        "unsupported symbolic ref '{name}'; Libra currently supports HEAD"
    ))
    .with_stable_code(StableErrorCode::CliInvalidTarget)
    .with_hint("use 'libra symbolic-ref HEAD' to inspect the current branch."))
}

async fn set_head_target(target: &str, reason: Option<&str>) -> CliResult<()> {
    let branch_name = branch_name_from_full_ref(target)?;
    let db = get_db_conn_instance().await;
    let target_branch = Branch::find_branch_result_with_conn(&db, branch_name, None)
        .await
        .map_err(map_head_write_error)?;

    let branch_name_owned = branch_name.to_string();
    let Some(target_branch) = target_branch else {
        Head::update_result_with_conn(&db, Head::Branch(branch_name_owned), None)
            .await
            .map_err(map_head_write_error)?;
        return Ok(());
    };

    let old_oid = Head::current_commit_result_with_conn(&db)
        .await
        .map_err(map_head_error)?
        .map(|oid| oid.to_string())
        .unwrap_or_else(|| ObjectHash::zero_str(get_hash_kind()).to_string());
    let from_ref_name = match Head::current_result_with_conn(&db)
        .await
        .map_err(map_head_error)?
    {
        Head::Branch(name) => name,
        Head::Detached(hash) => hash.to_string().chars().take(7).collect(),
    };
    let context = ReflogContext {
        old_oid,
        new_oid: target_branch.commit.to_string(),
        action: ReflogAction::Switch {
            from: from_ref_name,
            to: branch_name_owned.clone(),
        },
        message: reason.map(str::to_string),
    };

    let branch_for_update = branch_name_owned;
    with_reflog(
        context,
        move |txn: &sea_orm::DatabaseTransaction| {
            Box::pin(async move {
                Head::update_result_with_conn(txn, Head::Branch(branch_for_update), None)
                    .await
                    .map_err(|error| DbErr::Custom(error.to_string()))?;
                Ok(())
            })
        },
        false,
    )
    .await
    .map_err(map_reflog_write_error)?;
    Ok(())
}

fn branch_name_from_full_ref(target: &str) -> CliResult<&str> {
    let Some(branch_name) = target.strip_prefix(HEADS_PREFIX) else {
        return Err(CliError::failure(format!(
            "unsupported symbolic ref target '{target}'; expected refs/heads/<branch>"
        ))
        .with_stable_code(StableErrorCode::CliInvalidTarget)
        .with_hint("use a local branch ref such as 'refs/heads/main'."));
    };

    validate_branch_name(branch_name)?;
    Ok(branch_name)
}

fn validate_branch_name(branch_name: &str) -> CliResult<()> {
    if is_valid_git_branch_name(branch_name) {
        Ok(())
    } else {
        Err(invalid_branch_target(branch_name))
    }
}

fn invalid_branch_target(branch_name: &str) -> CliError {
    CliError::failure(format!(
        "invalid branch name in symbolic ref target: '{branch_name}'"
    ))
    .with_stable_code(StableErrorCode::CliInvalidTarget)
    .with_hint("use a valid local branch name under refs/heads/.")
}

fn write_symbolic_ref_output(value: &str) -> CliResult<()> {
    let stdout = std::io::stdout();
    let mut writer = stdout.lock();
    match writeln!(writer, "{value}") {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(
            CliError::fatal(format!("failed to write symbolic-ref output: {error}"))
                .with_stable_code(StableErrorCode::IoWriteFailed),
        ),
    }
}

fn map_head_error(error: BranchStoreError) -> CliError {
    match error {
        BranchStoreError::Query(detail) => {
            CliError::fatal(format!("failed to read HEAD symbolic ref: {detail}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        }
        other => CliError::fatal(format!("failed to read HEAD symbolic ref: {other}"))
            .with_stable_code(StableErrorCode::RepoCorrupt),
    }
}

fn map_head_write_error(error: BranchStoreError) -> CliError {
    match error {
        BranchStoreError::Query(detail) => {
            CliError::fatal(format!("failed to update HEAD symbolic ref: {detail}"))
                .with_stable_code(StableErrorCode::IoWriteFailed)
        }
        other => CliError::fatal(format!("failed to update HEAD symbolic ref: {other}"))
            .with_stable_code(StableErrorCode::RepoCorrupt),
    }
}

fn map_reflog_write_error(error: ReflogError) -> CliError {
    CliError::fatal(format!(
        "failed to update HEAD symbolic ref reflog: {error}"
    ))
    .with_stable_code(StableErrorCode::IoWriteFailed)
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::SymbolicRefArgs;

    #[test]
    fn parses_read_head_defaults() {
        let args = SymbolicRefArgs::try_parse_from(["symbolic-ref"]).unwrap();
        assert!(!args.quiet);
        assert!(!args.short);
        assert!(args.name.is_none());
        assert!(args.target.is_none());
    }

    #[test]
    fn parses_short_head() {
        let args = SymbolicRefArgs::try_parse_from(["symbolic-ref", "--short", "HEAD"]).unwrap();
        assert!(args.short);
        assert_eq!(args.name.as_deref(), Some("HEAD"));
    }

    #[test]
    fn parses_set_head_target() {
        let args = SymbolicRefArgs::try_parse_from(["symbolic-ref", "HEAD", "refs/heads/feature"])
            .unwrap();
        assert_eq!(args.name.as_deref(), Some("HEAD"));
        assert_eq!(args.target.as_deref(), Some("refs/heads/feature"));
    }

    #[test]
    fn parses_delete_flag() {
        let args = SymbolicRefArgs::try_parse_from(["symbolic-ref", "-d", "HEAD"]).unwrap();
        assert!(args.delete);
        assert_eq!(args.name.as_deref(), Some("HEAD"));
    }

    #[test]
    fn parses_reason_with_set_target() {
        let args = SymbolicRefArgs::try_parse_from([
            "symbolic-ref",
            "-m",
            "manual branch move",
            "HEAD",
            "refs/heads/feature",
        ])
        .unwrap();
        assert_eq!(args.reason.as_deref(), Some("manual branch move"));
        assert_eq!(args.name.as_deref(), Some("HEAD"));
        assert_eq!(args.target.as_deref(), Some("refs/heads/feature"));
    }

    #[test]
    fn rejects_delete_reason_target_short_and_missing_name_forms() {
        for argv in [
            vec!["symbolic-ref", "-d", "-m", "reason", "HEAD"],
            vec!["symbolic-ref", "-d", "HEAD", "refs/heads/main"],
            vec!["symbolic-ref", "-d", "--short", "HEAD"],
            vec!["symbolic-ref", "-m", "reason", "HEAD"],
            vec!["symbolic-ref", "-d"],
        ] {
            assert!(
                SymbolicRefArgs::try_parse_from(argv.clone()).is_err(),
                "argv should be rejected by clap: {argv:?}"
            );
        }
    }
}
