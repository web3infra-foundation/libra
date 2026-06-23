//! Implements `rev-parse` to resolve revision names and print basic repository paths.

use std::{fs, io::Write, path::PathBuf};

use clap::Parser;
use git_internal::hash::ObjectHash;
use serde::Serialize;

use crate::{
    internal::{
        branch::{Branch, BranchStoreError},
        config::ConfigKv,
        head::Head,
    },
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
        text::SHORT_HASH_LEN,
        util::{self, CommitBaseError},
    },
};

/// `--help` examples shown in `libra rev-parse --help` output.
///
/// `rev-parse` is the canonical script bridge: resolve a revision spec
/// to a commit hash, a short hash, a branch name, or print the
/// repository top-level. The banner pins the four mutually-exclusive
/// modes plus a JSON variant for agents so users see all supported
/// forms without reading the design doc. Cross-cutting `--help`
/// EXAMPLES rollout per `docs/development/commands/_general.md` item B.
pub const REV_PARSE_EXAMPLES: &str = "\
EXAMPLES:
    libra rev-parse HEAD                Print the full 40-char hash for HEAD
    libra rev-parse main~3              Resolve any revision spec to a full hash
    libra rev-parse --short HEAD        Print a non-ambiguous short hash
    libra rev-parse --sq HEAD           Print the resolved object name, shell-quoted
    libra rev-parse --abbrev-ref HEAD   Print the branch name (or HEAD when detached)
    libra rev-parse --show-toplevel     Print the absolute path of the repository root
    libra rev-parse --verify HEAD       Assert HEAD resolves to one object (exit 128 if not)
    libra rev-parse --is-inside-work-tree  Print true/false for working-tree context
    libra rev-parse --is-inside-git-dir    Print true/false for .libra-directory context
    libra rev-parse --absolute-git-dir  Print the canonicalized absolute .libra path
    libra rev-parse --json HEAD         Structured JSON output for agents";

#[derive(Parser, Debug)]
#[command(after_help = REV_PARSE_EXAMPLES)]
pub struct RevParseArgs {
    /// Show a non-ambiguous short object name. Accepts an optional length
    /// (e.g. `--short=8`) to request a specific abbreviation.
    #[clap(long, num_args = 0..=1, require_equals = true, default_missing_value = "7", conflicts_with_all = ["abbrev_ref", "show_toplevel"])]
    pub short: Option<String>,

    /// Show the branch name instead of the commit hash.
    #[clap(long = "abbrev-ref", conflicts_with_all = ["show_toplevel", "short"])]
    pub abbrev_ref: bool,

    /// Show the absolute path of the top-level working tree.
    #[clap(long = "show-toplevel", conflicts_with_all = ["abbrev_ref", "short", "spec"])]
    pub show_toplevel: bool,

    /// Verify that the revision resolves to exactly one object; fail (exit 128) otherwise.
    /// With the global `-q`/`--quiet`, failure is silent with exit code 1.
    #[clap(long, conflicts_with_all = ["show_toplevel", "abbrev_ref", "is_inside_work_tree", "is_inside_git_dir", "is_bare_repository", "git_dir", "absolute_git_dir"])]
    pub verify: bool,

    /// Use this revision when no SPEC is given (Git's `--default <arg>`).
    #[clap(long, value_name = "ARG", conflicts_with_all = ["show_toplevel", "is_inside_work_tree", "is_inside_git_dir", "is_bare_repository", "git_dir", "absolute_git_dir"])]
    pub default: Option<String>,

    /// Shell-quote the resolved object name for safe shell consumption
    /// (Git's `--sq`). Only affects the resolved-revision output, not the
    /// repository-query modes (e.g. `--show-toplevel`).
    #[clap(long = "sq")]
    pub sq: bool,

    /// Print "true" when run inside a working tree, "false" otherwise.
    #[clap(long = "is-inside-work-tree", conflicts_with_all = ["short", "abbrev_ref", "show_toplevel", "spec", "is_bare_repository", "git_dir", "absolute_git_dir"])]
    pub is_inside_work_tree: bool,

    /// Print "true" when the current directory is inside the `.libra` directory, "false" otherwise.
    #[clap(long = "is-inside-git-dir", conflicts_with_all = ["short", "abbrev_ref", "show_toplevel", "spec", "is_inside_work_tree", "is_bare_repository", "git_dir", "absolute_git_dir"])]
    pub is_inside_git_dir: bool,

    /// Print "true" when the repository is bare, "false" otherwise.
    #[clap(long = "is-bare-repository", conflicts_with_all = ["short", "abbrev_ref", "show_toplevel", "spec", "git_dir", "absolute_git_dir"])]
    pub is_bare_repository: bool,

    /// Print the path to the `.libra` directory.
    #[clap(long = "git-dir", conflicts_with_all = ["short", "abbrev_ref", "show_toplevel", "spec"])]
    pub git_dir: bool,

    /// Print the canonicalized absolute path to the `.libra` directory (like
    /// `--git-dir`, but always absolute).
    #[clap(long = "absolute-git-dir", conflicts_with_all = ["short", "abbrev_ref", "show_toplevel", "spec", "git_dir"])]
    pub absolute_git_dir: bool,

    /// Print the path relative from the current directory to the repository root.
    #[clap(long = "show-cdup", conflicts_with_all = ["short", "abbrev_ref", "show_toplevel", "spec"])]
    pub show_cdup: bool,

    /// Print the path of the current directory relative to the repository root.
    #[clap(long = "show-prefix", conflicts_with_all = ["short", "abbrev_ref", "show_toplevel", "spec"])]
    pub show_prefix: bool,

    /// Revision to parse. Defaults to HEAD when omitted.
    #[clap(value_name = "SPEC")]
    pub spec: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct RevParseOutput {
    mode: &'static str,
    input: Option<String>,
    value: String,
}

pub async fn execute(args: RevParseArgs) -> Result<(), String> {
    execute_safe(args, &OutputConfig::default())
        .await
        .map_err(|err| err.render())
}

pub async fn execute_safe(args: RevParseArgs, output: &OutputConfig) -> CliResult<()> {
    if !args.show_toplevel {
        util::require_repo().map_err(|_| CliError::repo_not_found())?;
    }
    let result = match resolve_rev_parse(&args).await {
        Ok(result) => result,
        Err(error) => {
            // Git's `rev-parse --verify` fails with exit 128 when the argument does
            // not name exactly one object; with the global `-q`/`--quiet` it fails
            // silently with exit code 1 instead of printing a diagnostic.
            if args.verify {
                if output.quiet {
                    return Err(CliError::silent_exit(1));
                }
                let spec = args
                    .spec
                    .as_deref()
                    .or(args.default.as_deref())
                    .unwrap_or("HEAD");
                return Err(CliError::fatal(format!(
                    "Needed a single revision (could not resolve '{spec}')"
                ))
                .with_exit_code(128));
            }
            return Err(error);
        }
    };

    if output.is_json() {
        emit_json_data("rev-parse", &result, output)
    } else if output.quiet {
        Ok(())
    } else {
        let stdout = std::io::stdout();
        let mut writer = stdout.lock();
        // `--sq` shell-quotes the resolved object name (the `resolve`/`short`
        // modes), matching Git; the repository-query modes are left verbatim.
        let value = if args.sq && matches!(result.mode, "resolve" | "short") {
            sq_quote(&result.value)
        } else {
            result.value.clone()
        };
        write_rev_parse_output(&mut writer, &value)
    }
}

/// Single-quote a value for safe shell consumption (Git's `--sq`): wrap the
/// whole value in single quotes and escape any embedded single quote as
/// `'\''`. Applied unconditionally (Git quotes even values with no special
/// characters).
fn sq_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn write_rev_parse_output<W: Write>(writer: &mut W, value: &str) -> CliResult<()> {
    match writeln!(writer, "{value}") {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(
            CliError::fatal(format!("failed to write rev-parse output: {error}"))
                .with_stable_code(StableErrorCode::IoWriteFailed),
        ),
    }
}

async fn resolve_rev_parse(args: &RevParseArgs) -> CliResult<RevParseOutput> {
    if args.show_toplevel {
        let workdir = resolve_show_toplevel_path().await?;
        return Ok(RevParseOutput {
            mode: "show_toplevel",
            input: None,
            value: util::path_to_string(&workdir),
        });
    }

    if args.is_inside_work_tree {
        // A non-bare Libra repository always has a working tree we operate inside.
        let inside = !is_bare_repository().await?;
        return Ok(RevParseOutput {
            mode: "is_inside_work_tree",
            input: None,
            value: inside.to_string(),
        });
    }

    if args.is_inside_git_dir {
        // "true" when the current directory is inside `.libra` (Libra's
        // equivalent of Git's GIT_DIR), "false" anywhere else in the worktree.
        let storage = util::try_get_storage_path(None).map_err(map_repo_path_error)?;
        let cwd = util::cur_dir();
        return Ok(RevParseOutput {
            mode: "is_inside_git_dir",
            input: None,
            value: util::is_sub_path(&cwd, &storage).to_string(),
        });
    }

    if args.is_bare_repository {
        return Ok(RevParseOutput {
            mode: "is_bare_repository",
            input: None,
            value: is_bare_repository().await?.to_string(),
        });
    }

    if args.git_dir {
        let dir = util::try_get_storage_path(None).map_err(map_repo_path_error)?;
        return Ok(RevParseOutput {
            mode: "git_dir",
            input: None,
            value: util::path_to_string(&dir),
        });
    }

    if args.absolute_git_dir {
        let dir = util::try_get_storage_path(None).map_err(map_repo_path_error)?;
        // `--git-dir` already yields an absolute path in Libra; canonicalize to
        // guarantee Git's "canonicalized absolute path" contract, falling back
        // to the resolved path if canonicalization fails.
        let abs = std::fs::canonicalize(&dir).unwrap_or(dir);
        return Ok(RevParseOutput {
            mode: "absolute_git_dir",
            input: None,
            value: util::path_to_string(&abs),
        });
    }

    if args.show_prefix {
        let storage = util::try_get_storage_path(None).map_err(map_repo_path_error)?;
        let cwd = util::cur_dir();
        let prefix = cwd
            .strip_prefix(storage.parent().unwrap_or(&storage))
            .unwrap_or(&cwd);
        let value = if prefix.as_os_str().is_empty() {
            String::new()
        } else {
            format!("{}/", prefix.display())
        };
        return Ok(RevParseOutput {
            mode: "show_prefix",
            input: None,
            value,
        });
    }

    if args.show_cdup {
        let storage = util::try_get_storage_path(None).map_err(map_repo_path_error)?;
        let worktree_root = storage.parent().unwrap_or(&storage);
        let cwd = util::cur_dir();
        let value = if cwd == *worktree_root {
            String::new()
        } else {
            let rel = cwd.strip_prefix(worktree_root).unwrap_or(&cwd);
            let depth = rel.components().count();
            "../".repeat(depth)
        };
        return Ok(RevParseOutput {
            mode: "show_cdup",
            input: None,
            value,
        });
    }

    // `--default <arg>` supplies the revision when no positional SPEC is given.
    let spec = args
        .spec
        .as_deref()
        .or(args.default.as_deref())
        .unwrap_or("HEAD");

    if args.abbrev_ref {
        let value = resolve_abbrev_ref(spec).await?;
        return Ok(RevParseOutput {
            mode: "abbrev_ref",
            input: Some(spec.to_string()),
            value,
        });
    }

    let commit = util::get_commit_base_typed(spec)
        .await
        .map_err(|err| rev_parse_target_error(spec, err))?;
    let value = if let Some(short_len) = &args.short {
        let requested_len: usize = short_len.parse().map_err(|_| {
            CliError::command_usage(format!("invalid --short length: '{short_len}'"))
                .with_stable_code(StableErrorCode::CliInvalidArguments)
        })?;
        resolve_short_commit(&commit, Some(requested_len)).await?
    } else {
        commit.to_string()
    };

    Ok(RevParseOutput {
        mode: if args.short.is_some() {
            "short"
        } else {
            "resolve"
        },
        input: Some(spec.to_string()),
        value,
    })
}

async fn resolve_abbrev_ref(spec: &str) -> CliResult<String> {
    if spec == "HEAD" {
        return match Head::current_result().await {
            Ok(Head::Branch(name)) => Ok(name),
            Ok(Head::Detached(_)) => Ok("HEAD".to_string()),
            Err(error) => Err(map_head_resolution_error(error)),
        };
    }

    if let Some(branch_name) = spec.strip_prefix("refs/heads/")
        && let Some(branch) = Branch::find_branch_result(branch_name, None)
            .await
            .map_err(|error| map_symbolic_ref_resolution_error(spec, error))?
    {
        return Ok(branch.name);
    }

    if let Some(short_name) = spec.strip_prefix("refs/remotes/")
        && resolve_remote_tracking_ref(spec, short_name).await?
    {
        return Ok(short_name.to_string());
    }

    if let Some(branch) = Branch::find_branch_result(spec, None)
        .await
        .map_err(|error| map_symbolic_ref_resolution_error(spec, error))?
    {
        return Ok(branch.name);
    }

    if resolve_remote_tracking_ref(spec, spec).await? {
        return Ok(spec.to_string());
    }

    Err(CliError::failure(format!("not a symbolic ref: '{spec}'"))
        .with_stable_code(StableErrorCode::CliInvalidTarget)
        .with_hint("use 'libra rev-parse <rev>' to resolve it to a commit hash."))
}

async fn resolve_remote_tracking_ref(spec: &str, short_name: &str) -> CliResult<bool> {
    for (remote, branch_name) in util::remote_tracking_candidates(short_name) {
        let full_ref = format!("refs/remotes/{remote}/{branch_name}");

        if Branch::find_branch_result(&full_ref, Some(remote))
            .await
            .map_err(|error| map_symbolic_ref_resolution_error(spec, error))?
            .is_some()
        {
            return Ok(true);
        }

        if Branch::find_branch_result(branch_name, Some(remote))
            .await
            .map_err(|error| map_symbolic_ref_resolution_error(spec, error))?
            .is_some()
        {
            return Ok(true);
        }
    }

    Ok(false)
}

async fn resolve_short_commit(
    commit: &ObjectHash,
    requested_len: Option<usize>,
) -> CliResult<String> {
    let full = commit.to_string();
    let storage = util::objects_storage();

    let min_len = requested_len.unwrap_or(SHORT_HASH_LEN).max(1);

    for len in min_len..=full.len() {
        let prefix = &full[..len];
        let matches = storage.search_result(prefix).await.map_err(|error| {
            CliError::fatal(format!(
                "failed to search objects while abbreviating '{full}': {error}"
            ))
            .with_stable_code(StableErrorCode::IoReadFailed)
        })?;

        if matches.len() == 1 && matches[0] == *commit {
            return Ok(prefix.to_string());
        }
    }

    Ok(full)
}

async fn is_bare_repository() -> CliResult<bool> {
    fn parse_git_bool(value: &str) -> Option<bool> {
        match value.trim() {
            v if v.eq_ignore_ascii_case("true")
                || v.eq_ignore_ascii_case("yes")
                || v.eq_ignore_ascii_case("on")
                || v == "1" =>
            {
                Some(true)
            }
            v if v.eq_ignore_ascii_case("false")
                || v.eq_ignore_ascii_case("no")
                || v.eq_ignore_ascii_case("off")
                || v == "0" =>
            {
                Some(false)
            }
            _ => None,
        }
    }

    match ConfigKv::get("core.bare").await {
        Ok(Some(entry)) => parse_git_bool(&entry.value).ok_or_else(|| {
            CliError::fatal(format!(
                "Invalid core.bare value: '{}'. Expected true/false/yes/no/on/off/1/0",
                entry.value
            ))
            .with_stable_code(StableErrorCode::RepoCorrupt)
        }),
        Ok(None) => Ok(false),
        Err(error) => Err(
            CliError::fatal(format!("Failed to read core.bare config: {error}"))
                .with_stable_code(StableErrorCode::IoReadFailed),
        ),
    }
}

async fn resolve_show_toplevel_path() -> CliResult<PathBuf> {
    let workdir = util::try_working_dir().map_err(map_repo_path_error)?;
    let storage = util::try_get_storage_path(None).map_err(map_repo_path_error)?;
    if workdir == storage {
        if is_bare_repository().await? {
            return Err(CliError::fatal("this operation must be run in a work tree")
                .with_stable_code(StableErrorCode::RepoStateInvalid));
        }

        let storage = fs::canonicalize(&storage).map_err(|error| {
            CliError::io(format!(
                "failed to resolve repository storage path '{}': {error}",
                storage.display()
            ))
            .with_stable_code(StableErrorCode::IoReadFailed)
        })?;

        return storage
            .parent()
            .map(PathBuf::from)
            .ok_or_else(CliError::repo_not_found);
    }
    Ok(workdir)
}

fn map_repo_path_error(err: std::io::Error) -> CliError {
    match err.kind() {
        std::io::ErrorKind::NotFound => CliError::repo_not_found(),
        _ => CliError::io(format!("failed to determine repository root: {err}"))
            .with_stable_code(StableErrorCode::IoReadFailed),
    }
}

fn map_head_resolution_error(error: BranchStoreError) -> CliError {
    map_symbolic_ref_resolution_error("HEAD", error)
}

fn map_symbolic_ref_resolution_error(spec: &str, error: BranchStoreError) -> CliError {
    match error {
        BranchStoreError::Corrupt { detail, .. } => {
            CliError::fatal(format!("failed to resolve symbolic ref '{spec}': {detail}"))
                .with_stable_code(StableErrorCode::RepoCorrupt)
        }
        BranchStoreError::Query(detail)
        | BranchStoreError::NotFound(detail)
        | BranchStoreError::Delete { detail, .. } => {
            CliError::fatal(format!("failed to resolve symbolic ref '{spec}': {detail}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        }
    }
}

fn rev_parse_target_error(spec: &str, error: CommitBaseError) -> CliError {
    match error {
        CommitBaseError::HeadUnborn => CliError::failure(format!(
            "not a valid object name: '{spec}' (HEAD does not point to a commit)"
        ))
        .with_stable_code(StableErrorCode::CliInvalidTarget)
        .with_hint("create a commit before resolving HEAD."),
        CommitBaseError::InvalidReference(detail) => {
            CliError::failure(format!("not a valid object name: '{spec}' ({detail})"))
                .with_stable_code(StableErrorCode::CliInvalidTarget)
        }
        CommitBaseError::ReadFailure(detail) => {
            CliError::fatal(format!("failed to resolve '{spec}': {detail}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        }
        CommitBaseError::CorruptReference(detail) => {
            CliError::fatal(format!("failed to resolve '{spec}': {detail}"))
                .with_stable_code(StableErrorCode::RepoCorrupt)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};

    use clap::Parser;

    use super::{RevParseArgs, write_rev_parse_output};
    use crate::utils::error::StableErrorCode;

    struct FailingWriter {
        kind: io::ErrorKind,
    }

    impl Write for FailingWriter {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(self.kind, "test write failure"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn test_rev_parse_args_default() {
        let args = RevParseArgs::try_parse_from(["rev-parse"]).unwrap();
        assert!(args.short.is_none());
        assert!(!args.abbrev_ref);
        assert!(!args.show_toplevel);
        assert!(args.spec.is_none());
    }

    #[test]
    fn test_rev_parse_args_short_head() {
        let args = RevParseArgs::try_parse_from(["rev-parse", "--short", "HEAD"]).unwrap();
        // `--short` without `=<n>` takes its default_missing_value ("7"); "HEAD"
        // is consumed as the positional spec.
        assert_eq!(args.short.as_deref(), Some("7"));
        assert_eq!(args.spec.as_deref(), Some("HEAD"));
    }

    #[test]
    fn test_rev_parse_args_abbrev_ref() {
        let args = RevParseArgs::try_parse_from(["rev-parse", "--abbrev-ref", "HEAD"]).unwrap();
        assert!(args.abbrev_ref);
        assert_eq!(args.spec.as_deref(), Some("HEAD"));
    }

    #[test]
    fn test_rev_parse_args_show_toplevel() {
        let args = RevParseArgs::try_parse_from(["rev-parse", "--show-toplevel"]).unwrap();
        assert!(args.show_toplevel);
    }

    #[test]
    fn test_rev_parse_args_verify() {
        let args = RevParseArgs::try_parse_from(["rev-parse", "--verify", "HEAD"]).unwrap();
        assert!(args.verify);
        assert_eq!(args.spec.as_deref(), Some("HEAD"));
    }

    #[test]
    fn test_rev_parse_args_default_revision() {
        let args = RevParseArgs::try_parse_from(["rev-parse", "--default", "main"]).unwrap();
        assert_eq!(args.default.as_deref(), Some("main"));
        assert!(args.spec.is_none());
    }

    #[test]
    fn test_rev_parse_args_is_inside_work_tree() {
        let args = RevParseArgs::try_parse_from(["rev-parse", "--is-inside-work-tree"]).unwrap();
        assert!(args.is_inside_work_tree);
    }

    #[test]
    fn test_rev_parse_args_repo_query_modes() {
        let bare = RevParseArgs::try_parse_from(["rev-parse", "--is-bare-repository"]).unwrap();
        assert!(bare.is_bare_repository);
        let git_dir = RevParseArgs::try_parse_from(["rev-parse", "--git-dir"]).unwrap();
        assert!(git_dir.git_dir);
        let inside_git_dir =
            RevParseArgs::try_parse_from(["rev-parse", "--is-inside-git-dir"]).unwrap();
        assert!(inside_git_dir.is_inside_git_dir);
    }

    #[test]
    fn test_rev_parse_args_is_inside_git_dir_conflicts_with_spec() {
        let err = RevParseArgs::try_parse_from(["rev-parse", "--is-inside-git-dir", "HEAD"])
            .expect_err("--is-inside-git-dir should reject SPEC");
        let rendered = err.to_string();
        assert!(
            rendered.contains("cannot be used with") || rendered.contains("unexpected argument"),
            "unexpected clap error: {rendered}"
        );
    }

    #[test]
    fn test_rev_parse_args_is_inside_git_dir_conflicts_with_other_modes() {
        // Like the sibling query flags, `--is-inside-git-dir` must be rejected
        // (not silently ignored) when combined with --verify or --default.
        for combo in [
            vec!["rev-parse", "--verify", "--is-inside-git-dir"],
            vec!["rev-parse", "--default", "HEAD", "--is-inside-git-dir"],
            vec!["rev-parse", "--is-inside-git-dir", "--git-dir"],
        ] {
            let err = RevParseArgs::try_parse_from(combo.clone())
                .expect_err(&format!("{combo:?} should be rejected"));
            assert!(
                err.to_string().contains("cannot be used with"),
                "expected a conflict error for {combo:?}, got: {err}"
            );
        }
    }

    #[test]
    fn test_rev_parse_args_absolute_git_dir_conflicts_mirror_git_dir() {
        // `--absolute-git-dir` must share `--git-dir`'s conflict set so invalid
        // mode combinations are rejected rather than silently bypassed.
        for combo in [
            vec!["rev-parse", "--verify", "--absolute-git-dir"],
            vec!["rev-parse", "--default", "HEAD", "--absolute-git-dir"],
            vec!["rev-parse", "--is-inside-work-tree", "--absolute-git-dir"],
            vec!["rev-parse", "--is-inside-git-dir", "--absolute-git-dir"],
            vec!["rev-parse", "--is-bare-repository", "--absolute-git-dir"],
            vec!["rev-parse", "--git-dir", "--absolute-git-dir"],
            vec!["rev-parse", "--short", "--absolute-git-dir"],
            vec!["rev-parse", "--abbrev-ref", "--absolute-git-dir"],
            vec!["rev-parse", "--show-toplevel", "--absolute-git-dir"],
        ] {
            let err = RevParseArgs::try_parse_from(combo.clone())
                .expect_err(&format!("{combo:?} should be rejected"));
            assert!(
                err.to_string().contains("cannot be used with"),
                "expected a conflict error for {combo:?}, got: {err}"
            );
        }
    }

    #[test]
    fn test_rev_parse_args_is_inside_work_tree_conflicts_with_spec() {
        let err = RevParseArgs::try_parse_from(["rev-parse", "--is-inside-work-tree", "HEAD"])
            .expect_err("--is-inside-work-tree should reject SPEC");
        let rendered = err.to_string();
        assert!(
            rendered.contains("cannot be used with") || rendered.contains("unexpected argument"),
            "unexpected clap error: {rendered}"
        );
    }

    #[test]
    fn test_rev_parse_args_show_toplevel_conflicts_with_spec() {
        let err = RevParseArgs::try_parse_from(["rev-parse", "--show-toplevel", "HEAD"])
            .expect_err("--show-toplevel should reject SPEC");
        let rendered = err.to_string();
        assert!(
            rendered.contains("cannot be used with") || rendered.contains("unexpected argument"),
            "unexpected clap error: {rendered}"
        );
    }

    #[test]
    fn test_rev_parse_args_abbrev_ref_conflicts_with_short() {
        let err = RevParseArgs::try_parse_from(["rev-parse", "--abbrev-ref", "--short", "HEAD"])
            .expect_err("--abbrev-ref should reject --short");
        let rendered = err.to_string();
        assert!(
            rendered.contains("cannot be used with"),
            "unexpected clap error: {rendered}"
        );
    }

    #[test]
    fn test_rev_parse_args_show_toplevel_conflicts_with_short() {
        let err = RevParseArgs::try_parse_from(["rev-parse", "--show-toplevel", "--short"])
            .expect_err("--show-toplevel should reject --short");
        let rendered = err.to_string();
        assert!(
            rendered.contains("cannot be used with"),
            "unexpected clap error: {rendered}"
        );
    }

    #[test]
    fn test_write_rev_parse_output_maps_write_failure_to_write_code() {
        let mut writer = FailingWriter {
            kind: io::ErrorKind::PermissionDenied,
        };

        let error = write_rev_parse_output(&mut writer, "abc123").expect_err("write should fail");

        assert_eq!(error.stable_code(), StableErrorCode::IoWriteFailed);
    }

    #[test]
    fn test_write_rev_parse_output_ignores_broken_pipe() {
        let mut writer = FailingWriter {
            kind: io::ErrorKind::BrokenPipe,
        };

        write_rev_parse_output(&mut writer, "abc123").expect("broken pipe should be ignored");
    }
}
