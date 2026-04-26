//! Implements `rev-list` to enumerate commits reachable from a revision.

use std::io::Write;

use clap::Parser;
use serde::Serialize;

use crate::{
    command::log,
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
        util::{self, CommitBaseError},
    },
};

#[derive(Parser, Debug)]
pub struct RevListArgs {
    /// Revision to list from. Defaults to HEAD when omitted.
    #[clap(value_name = "SPEC")]
    pub spec: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct RevListOutput {
    input: String,
    commits: Vec<String>,
    total: usize,
}

pub async fn execute(args: RevListArgs) -> Result<(), String> {
    execute_safe(args, &OutputConfig::default())
        .await
        .map_err(|err| err.render())
}

pub async fn execute_safe(args: RevListArgs, output: &OutputConfig) -> CliResult<()> {
    util::require_repo().map_err(|_| CliError::repo_not_found())?;
    let result = resolve_rev_list(&args).await?;

    if output.is_json() {
        emit_json_data("rev-list", &result, output)
    } else if output.quiet {
        Ok(())
    } else {
        let stdout = std::io::stdout();
        let mut writer = stdout.lock();
        for commit in &result.commits {
            writeln!(writer, "{commit}")
                .map_err(|e| CliError::io(format!("failed to write rev-list output: {e}")))?;
        }
        Ok(())
    }
}

async fn resolve_rev_list(args: &RevListArgs) -> CliResult<RevListOutput> {
    let spec = args.spec.as_deref().unwrap_or("HEAD");
    let commit = util::get_commit_base_typed(spec)
        .await
        .map_err(|err| rev_list_target_error(spec, err))?;
    let mut commits = log::get_reachable_commits(commit.to_string(), None).await?;
    // sort by newest first as per rev-list contract
    commits.sort_by_key(|c| std::cmp::Reverse(c.committer.timestamp));

    let commits = commits
        .into_iter()
        .map(|commit| commit.id.to_string())
        .collect::<Vec<_>>();
    let total = commits.len();

    Ok(RevListOutput {
        input: spec.to_string(),
        commits,
        total,
    })
}

fn rev_list_target_error(spec: &str, error: CommitBaseError) -> CliError {
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
    use clap::Parser;

    use super::RevListArgs;

    #[test]
    fn test_rev_list_args_default() {
        let args = RevListArgs::try_parse_from(["rev-list"]).unwrap();
        assert!(args.spec.is_none());
    }

    #[test]
    fn test_rev_list_args_with_spec() {
        let args = RevListArgs::try_parse_from(["rev-list", "HEAD~1"]).unwrap();
        assert_eq!(args.spec.as_deref(), Some("HEAD~1"));
    }
}
