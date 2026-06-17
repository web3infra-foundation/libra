use git_internal::internal::object::commit::Commit;

use super::RevListArgs;
use crate::{
    internal::log::date_parser::parse_date,
    utils::error::{CliError, CliResult, StableErrorCode},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ParentCountFilter {
    pub(super) min: usize,
    pub(super) max: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RevListTimeWindow {
    since: Option<i64>,
    until: Option<i64>,
}

pub(super) fn parent_count_filter(args: &RevListArgs) -> ParentCountFilter {
    let min = if args.no_min_parents {
        0
    } else {
        args.min_parents
            .unwrap_or(0)
            .max(usize::from(args.merges) * 2)
    };
    let max = if args.no_max_parents {
        None
    } else {
        match (args.max_parents, args.no_merges) {
            (Some(explicit), true) => Some(explicit.min(1)),
            (Some(explicit), false) => Some(explicit),
            (None, true) => Some(1),
            (None, false) => None,
        }
    };

    ParentCountFilter { min, max }
}

pub(super) fn rev_list_time_window(args: &RevListArgs) -> CliResult<RevListTimeWindow> {
    Ok(RevListTimeWindow {
        since: parse_rev_list_date_arg(args.since.as_deref(), "--since")?,
        until: parse_rev_list_date_arg(args.until.as_deref(), "--until")?,
    })
}

pub(super) fn rev_list_author_filter(args: &RevListArgs) -> Option<String> {
    args.author.as_ref().map(|pattern| pattern.to_lowercase())
}

pub(super) fn rev_list_committer_filter(args: &RevListArgs) -> Option<String> {
    args.committer
        .as_ref()
        .map(|pattern| pattern.to_lowercase())
}

pub(super) fn commit_matches_author(commit: &Commit, author_filter: Option<&str>) -> bool {
    signature_matches_filter(&commit.author.name, &commit.author.email, author_filter)
}

pub(super) fn commit_matches_committer(commit: &Commit, committer_filter: Option<&str>) -> bool {
    signature_matches_filter(
        &commit.committer.name,
        &commit.committer.email,
        committer_filter,
    )
}

fn signature_matches_filter(name: &str, email: &str, filter: Option<&str>) -> bool {
    let Some(filter) = filter else {
        return true;
    };
    if filter.is_empty() {
        return true;
    }

    let name = name.to_lowercase();
    let email = email.to_lowercase();
    name.contains(filter) || email.contains(filter) || format!("{name} <{email}>").contains(filter)
}

pub(super) fn commit_matches_parent_count(commit: &Commit, filter: ParentCountFilter) -> bool {
    let parent_count = commit.parent_commit_ids.len();
    parent_count >= filter.min && filter.max.is_none_or(|max| parent_count <= max)
}

pub(super) fn commit_matches_time_window(commit: &Commit, window: RevListTimeWindow) -> bool {
    let commit_ts = i64::try_from(commit.committer.timestamp).unwrap_or(i64::MAX);

    if let Some(since) = window.since
        && commit_ts < since
    {
        return false;
    }
    if let Some(until) = window.until
        && commit_ts > until
    {
        return false;
    }
    true
}

pub(super) fn sort_rev_list_commits(commits: &mut [Commit]) {
    commits.sort_by_key(|commit| std::cmp::Reverse(commit.committer.timestamp));
}

fn parse_rev_list_date_arg(value: Option<&str>, flag: &str) -> CliResult<Option<i64>> {
    value.map(parse_date).transpose().map_err(|error| {
        CliError::fatal(format!("invalid {flag} date: {error}"))
            .with_stable_code(StableErrorCode::CliInvalidArguments)
            .with_hint(r#"supported formats: YYYY-MM-DD, "N days ago", unix timestamp"#)
    })
}
