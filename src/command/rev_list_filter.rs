use git_internal::internal::object::commit::Commit;

use super::RevListArgs;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ParentCountFilter {
    pub(super) min: usize,
    pub(super) max: Option<usize>,
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

pub(super) fn commit_matches_parent_count(commit: &Commit, filter: ParentCountFilter) -> bool {
    let parent_count = commit.parent_commit_ids.len();
    parent_count >= filter.min && filter.max.is_none_or(|max| parent_count <= max)
}

pub(super) fn sort_rev_list_commits(commits: &mut [Commit]) {
    commits.sort_by_key(|commit| std::cmp::Reverse(commit.committer.timestamp));
}
