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
        write_rev_list_output(&mut writer, &result.commits)
    }
}

fn write_rev_list_output<W: Write>(writer: &mut W, commits: &[String]) -> CliResult<()> {
    for commit in commits {
        match writeln!(writer, "{commit}") {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => return Ok(()),
            Err(error) => {
                return Err(
                    CliError::fatal(format!("failed to write rev-list output: {error}"))
                        .with_stable_code(StableErrorCode::IoWriteFailed),
                );
            }
        }
    }
    Ok(())
}

async fn resolve_rev_list(args: &RevListArgs) -> CliResult<RevListOutput> {
    let spec = args.spec.as_deref().unwrap_or("HEAD");
    let commit = util::get_commit_base_typed(spec)
        .await
        .map_err(|err| rev_list_target_error(spec, err))?;
    let mut commits = log::get_reachable_commits(commit.to_string(), None).await?;
    sort_rev_list_commits(&mut commits);

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

fn sort_rev_list_commits(commits: &mut [git_internal::internal::object::commit::Commit]) {
    commits.sort_by(|a, b| {
        b.committer
            .timestamp
            .cmp(&a.committer.timestamp)
            .then_with(|| a.id.cmp(&b.id))
    });
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
    use std::io::{self, Write};

    use clap::Parser;
    use git_internal::{
        hash::{ObjectHash, get_hash_kind},
        internal::object::{
            commit::Commit,
            signature::{Signature, SignatureType},
        },
    };

    use super::{RevListArgs, sort_rev_list_commits, write_rev_list_output};
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

    fn test_signature(timestamp: usize) -> Signature {
        Signature {
            signature_type: SignatureType::Committer,
            name: "tester".to_string(),
            email: "tester@example.com".to_string(),
            timestamp,
            timezone: "+0000".to_string(),
        }
    }

    fn test_hash(byte: u8) -> ObjectHash {
        ObjectHash::from_bytes(&vec![byte; get_hash_kind().size()])
            .expect("test hash bytes should match active hash kind")
    }

    fn test_commit(id: ObjectHash, timestamp: usize) -> Commit {
        Commit {
            id,
            tree_id: id,
            parent_commit_ids: Vec::new(),
            author: test_signature(timestamp),
            committer: test_signature(timestamp),
            message: "test".to_string(),
        }
    }

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

    #[test]
    fn test_sort_rev_list_commits_uses_commit_id_tie_breaker() {
        let high = test_hash(0xff);
        let low = test_hash(0x01);
        let mut commits = vec![test_commit(high, 1), test_commit(low, 1)];

        sort_rev_list_commits(&mut commits);

        assert_eq!(commits[0].id, low);
        assert_eq!(commits[1].id, high);
    }

    #[test]
    fn test_sort_rev_list_commits_orders_newest_first() {
        let old = test_hash(0x01);
        let new = test_hash(0xff);
        let mut commits = vec![test_commit(old, 1), test_commit(new, 2)];

        sort_rev_list_commits(&mut commits);

        assert_eq!(commits[0].id, new);
        assert_eq!(commits[1].id, old);
    }

    #[test]
    fn test_write_rev_list_output_maps_write_failure_to_write_code() {
        let mut writer = FailingWriter {
            kind: io::ErrorKind::PermissionDenied,
        };

        let error = write_rev_list_output(&mut writer, &["abc123".to_string()])
            .expect_err("write should fail");

        assert_eq!(error.stable_code(), StableErrorCode::IoWriteFailed);
    }

    #[test]
    fn test_write_rev_list_output_ignores_broken_pipe() {
        let mut writer = FailingWriter {
            kind: io::ErrorKind::BrokenPipe,
        };

        write_rev_list_output(&mut writer, &["abc123".to_string()])
            .expect("broken pipe should be ignored");
    }
}
