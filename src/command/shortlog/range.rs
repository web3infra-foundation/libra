use std::collections::HashSet;

use git_internal::internal::object::commit::Commit;

use crate::{
    command::log::get_reachable_commits,
    internal::log::date_parser::parse_date,
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        util::{self, CommitBaseError},
    },
};

pub(super) async fn get_commits_for_shortlog(
    revision: Option<&str>,
    since_ts: Option<i64>,
    until_ts: Option<i64>,
) -> CliResult<Vec<Commit>> {
    let revision = revision.unwrap_or("HEAD");
    let mut commits = match parse_revision_range(revision)? {
        RevisionSelection::Single(revision) => reachable_from(&revision).await?,
        RevisionSelection::DoubleDot { exclude, include } => {
            let excluded = reachable_from(&exclude).await?;
            let excluded_ids = excluded
                .iter()
                .map(|commit| commit.id.to_string())
                .collect::<HashSet<_>>();
            reachable_from(&include)
                .await?
                .into_iter()
                .filter(|commit| !excluded_ids.contains(&commit.id.to_string()))
                .collect()
        }
    };

    commits.retain(|commit| passes_filter(commit, since_ts, until_ts));
    commits.sort_by_key(|commit| std::cmp::Reverse(commit.committer.timestamp));

    Ok(commits)
}

#[derive(Debug)]
enum RevisionSelection {
    Single(String),
    DoubleDot { exclude: String, include: String },
}

fn parse_revision_range(revision: &str) -> CliResult<RevisionSelection> {
    if revision.starts_with('^') || revision.contains("...") {
        return Err(unsupported_revision_range(revision));
    }

    let Some(index) = revision.find("..") else {
        return Ok(RevisionSelection::Single(revision.to_string()));
    };

    let left = &revision[..index];
    let right = &revision[index + 2..];
    if right.contains("..") || left.starts_with('^') || right.starts_with('^') {
        return Err(unsupported_revision_range(revision));
    }

    Ok(RevisionSelection::DoubleDot {
        exclude: if left.is_empty() {
            "HEAD".to_string()
        } else {
            left.to_string()
        },
        include: if right.is_empty() {
            "HEAD".to_string()
        } else {
            right.to_string()
        },
    })
}

async fn reachable_from(revision: &str) -> CliResult<Vec<Commit>> {
    let commit_hash = util::get_commit_base_typed(revision)
        .await
        .map_err(|error| shortlog_commit_base_error(revision, error))?
        .to_string();
    get_reachable_commits(commit_hash, None).await
}

fn unsupported_revision_range(revision: &str) -> CliError {
    CliError::fatal(format!("unsupported shortlog revision range '{revision}'"))
        .with_stable_code(StableErrorCode::CliInvalidTarget)
        .with_hint("use a single revision or a double-dot range like A..B")
}

fn passes_filter(commit: &Commit, since_ts: Option<i64>, until_ts: Option<i64>) -> bool {
    let commit_ts = commit.committer.timestamp as i64;

    if let Some(since) = since_ts
        && commit_ts < since
    {
        return false;
    }

    if let Some(until) = until_ts
        && commit_ts > until
    {
        return false;
    }

    true
}

pub(super) fn parse_shortlog_date_arg(value: Option<&str>, flag: &str) -> CliResult<Option<i64>> {
    value.map(parse_date).transpose().map_err(|error| {
        CliError::fatal(format!("invalid {flag} date: {error}"))
            .with_stable_code(StableErrorCode::CliInvalidArguments)
            .with_hint(r#"supported formats: YYYY-MM-DD, "N days ago", unix timestamp"#)
    })
}

fn shortlog_commit_base_error(revision: &str, error: CommitBaseError) -> CliError {
    match error {
        CommitBaseError::HeadUnborn => CliError::fatal("HEAD does not point to a commit")
            .with_stable_code(StableErrorCode::RepoStateInvalid)
            .with_hint("create a commit before running 'libra shortlog'."),
        CommitBaseError::InvalidReference(message) => CliError::fatal(format!(
            "failed to resolve revision '{revision}': {message}"
        ))
        .with_stable_code(StableErrorCode::CliInvalidTarget),
        CommitBaseError::ReadFailure(message) => {
            CliError::fatal(message).with_stable_code(StableErrorCode::IoReadFailed)
        }
        CommitBaseError::CorruptReference(message) => {
            CliError::fatal(message).with_stable_code(StableErrorCode::RepoCorrupt)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_revision_range_accepts_single_revision() {
        match parse_revision_range("HEAD").unwrap() {
            RevisionSelection::Single(value) => assert_eq!(value, "HEAD"),
            RevisionSelection::DoubleDot { .. } => panic!("expected single revision"),
        }
    }

    #[test]
    fn parse_revision_range_accepts_double_dot_with_implicit_head() {
        match parse_revision_range("main..").unwrap() {
            RevisionSelection::DoubleDot { exclude, include } => {
                assert_eq!(exclude, "main");
                assert_eq!(include, "HEAD");
            }
            RevisionSelection::Single(_) => panic!("expected double-dot range"),
        }

        match parse_revision_range("..topic").unwrap() {
            RevisionSelection::DoubleDot { exclude, include } => {
                assert_eq!(exclude, "HEAD");
                assert_eq!(include, "topic");
            }
            RevisionSelection::Single(_) => panic!("expected double-dot range"),
        }
    }

    #[test]
    fn parse_revision_range_rejects_unsupported_range_syntax() {
        let error = parse_revision_range("main...topic").unwrap_err();
        assert_eq!(error.stable_code(), StableErrorCode::CliInvalidTarget);

        let error = parse_revision_range("^main").unwrap_err();
        assert_eq!(error.stable_code(), StableErrorCode::CliInvalidTarget);
    }
}
