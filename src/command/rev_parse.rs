//! Implements `rev-parse` to resolve revision names and print basic repository paths.

use std::io::Write;

use clap::Parser;
use serde::Serialize;

use crate::{
    internal::{branch::Branch, head::Head},
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
        util,
    },
};

#[derive(Parser, Debug)]
pub struct RevParseArgs {
    /// Show a non-ambiguous short object name.
    #[clap(long)]
    pub short: bool,

    /// Show the branch name instead of the commit hash.
    #[clap(long = "abbrev-ref", conflicts_with = "show_toplevel")]
    pub abbrev_ref: bool,

    /// Show the absolute path of the top-level working tree.
    #[clap(long = "show-toplevel", conflicts_with = "abbrev_ref")]
    pub show_toplevel: bool,

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
    let result = resolve_rev_parse(&args).await?;

    if output.is_json() {
        emit_json_data("rev-parse", &result, output)
    } else if output.quiet {
        Ok(())
    } else {
        let stdout = std::io::stdout();
        let mut writer = stdout.lock();
        writeln!(writer, "{}", result.value)
            .map_err(|e| CliError::io(format!("failed to write rev-parse output: {e}")))
    }
}

async fn resolve_rev_parse(args: &RevParseArgs) -> CliResult<RevParseOutput> {
    if args.show_toplevel {
        let workdir = util::try_working_dir().map_err(map_repo_path_error)?;
        return Ok(RevParseOutput {
            mode: "show_toplevel",
            input: None,
            value: util::path_to_string(&workdir),
        });
    }

    let spec = args.spec.as_deref().unwrap_or("HEAD");

    if args.abbrev_ref {
        let value = resolve_abbrev_ref(spec).await?;
        return Ok(RevParseOutput {
            mode: "abbrev_ref",
            input: Some(spec.to_string()),
            value,
        });
    }

    let commit = util::get_commit_base(spec)
        .await
        .map_err(|e| rev_parse_invalid_target(spec, e))?;
    let value = if args.short {
        commit.to_string().chars().take(7).collect()
    } else {
        commit.to_string()
    };

    Ok(RevParseOutput {
        mode: if args.short { "short" } else { "resolve" },
        input: Some(spec.to_string()),
        value,
    })
}

async fn resolve_abbrev_ref(spec: &str) -> CliResult<String> {
    if spec.eq_ignore_ascii_case("HEAD") {
        return Ok(match Head::current().await {
            Head::Branch(name) => name,
            Head::Detached(_) => "HEAD".to_string(),
        });
    }

    if let Some(branch) = Branch::find_branch(spec, None).await {
        return Ok(branch.name);
    }

    if let Some((remote, branch_name)) = spec.split_once('/')
        && !remote.is_empty()
        && !branch_name.is_empty()
        && Branch::find_branch(branch_name, Some(remote))
            .await
            .is_some()
    {
        return Ok(spec.to_string());
    }

    Err(CliError::failure(format!("not a symbolic ref: '{spec}'"))
        .with_stable_code(StableErrorCode::CliInvalidTarget)
        .with_hint("use 'libra rev-parse <rev>' to resolve it to a commit hash."))
}

fn map_repo_path_error(err: std::io::Error) -> CliError {
    match err.kind() {
        std::io::ErrorKind::NotFound => CliError::repo_not_found(),
        _ => CliError::io(format!("failed to determine repository root: {err}"))
            .with_stable_code(StableErrorCode::IoReadFailed),
    }
}

fn rev_parse_invalid_target(spec: &str, message: String) -> CliError {
    let detail = message
        .strip_prefix("fatal: ")
        .unwrap_or(message.as_str())
        .to_string();
    CliError::failure(format!("not a valid object name: '{spec}' ({detail})"))
        .with_stable_code(StableErrorCode::CliInvalidTarget)
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::RevParseArgs;

    #[test]
    fn test_rev_parse_args_default() {
        let args = RevParseArgs::try_parse_from(["rev-parse"]).unwrap();
        assert!(!args.short);
        assert!(!args.abbrev_ref);
        assert!(!args.show_toplevel);
        assert!(args.spec.is_none());
    }

    #[test]
    fn test_rev_parse_args_short_head() {
        let args = RevParseArgs::try_parse_from(["rev-parse", "--short", "HEAD"]).unwrap();
        assert!(args.short);
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
}
