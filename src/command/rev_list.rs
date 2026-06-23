//! Implements `rev-list` to enumerate commits reachable from revisions.

use clap::Parser;

use crate::{
    internal::{branch::Branch, config::ConfigKv, head::Head, tag},
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
        util,
    },
};

#[path = "rev_list_cherry.rs"]
mod rev_list_cherry;
#[path = "rev_list_children.rs"]
mod rev_list_children;
#[path = "rev_list_filter.rs"]
mod rev_list_filter;
#[path = "rev_list_output.rs"]
mod rev_list_output;
#[path = "rev_list_spec.rs"]
mod rev_list_spec;

use rev_list_cherry::{RevListSelectedCommit, apply_cherry_filters, attach_cherry_metadata};
use rev_list_children::build_rev_list_children;
#[cfg(test)]
use rev_list_filter::ParentCountFilter;
use rev_list_filter::{
    commit_matches_author, commit_matches_committer, commit_matches_message,
    commit_matches_parent_count, commit_matches_time_window, filter_commits_by_pathspecs,
    parent_count_filter, rev_list_author_filter, rev_list_committer_filter,
    rev_list_message_filter, rev_list_time_window, sort_rev_list_commits,
};
use rev_list_output::{REV_LIST_EXAMPLES, RevListEntry, RevListOutput, emit_human_rev_list};
use rev_list_spec::resolve_revision_selection;

#[derive(Parser, Debug)]
#[command(after_help = REV_LIST_EXAMPLES)]
pub struct RevListArgs {
    /// Limit output to at most N commits
    #[clap(short = 'n', long = "max-count", value_name = "N")]
    pub max_count: Option<usize>,

    /// Skip the first N commits before output or counting
    #[clap(long, value_name = "N", default_value_t = 0)]
    pub skip: usize,

    /// Output the selected commits in reverse order. Commit limiting
    /// (`--max-count`/`--skip`) is applied first, then the result is reversed.
    #[clap(long)]
    pub reverse: bool,

    /// Pretend as if all refs (branches, remote-tracking branches, and
    /// tags) and the current HEAD are listed as `<SPEC>`, in addition to any
    /// explicit revisions.
    #[clap(long)]
    pub all: bool,

    /// Print only the number of commits after filters
    #[clap(long)]
    pub count: bool,

    /// Print parent commit IDs after each commit
    #[clap(long, conflicts_with = "children")]
    pub parents: bool,

    /// Print child commit IDs after each commit
    #[clap(long)]
    pub children: bool,

    /// Prefix each output line with the commit timestamp
    #[clap(long)]
    pub timestamp: bool,

    /// Follow only the first parent of merge commits
    #[clap(long = "first-parent")]
    pub first_parent: bool,

    /// Filter commits by author name or email
    #[clap(long, value_name = "PATTERN")]
    pub author: Option<String>,

    /// Filter commits by committer name or email
    #[clap(long, value_name = "PATTERN")]
    pub committer: Option<String>,

    /// Filter commits by message using a regular expression
    #[clap(long, value_name = "PATTERN")]
    pub grep: Vec<String>,

    /// Prefix symmetric-difference commits with '<' or '>'
    #[clap(long = "left-right")]
    pub left_right: bool,

    /// Show only the left side of a symmetric difference
    #[clap(long = "left-only", conflicts_with = "right_only")]
    pub left_only: bool,

    /// Show only the right side of a symmetric difference
    #[clap(long = "right-only", conflicts_with = "left_only")]
    pub right_only: bool,

    /// Omit patch-equivalent commits across symmetric-difference sides
    #[clap(long = "cherry-pick", conflicts_with = "cherry_mark")]
    pub cherry_pick: bool,

    /// Mark patch-equivalent commits with '=' and others with '+'
    #[clap(long = "cherry-mark", conflicts_with = "cherry_pick")]
    pub cherry_mark: bool,

    /// Show right-side commits and mark patch-equivalent commits
    #[clap(long = "cherry")]
    pub cherry: bool,

    /// Show commits more recent than DATE
    #[clap(long, visible_alias = "after", value_name = "DATE")]
    pub since: Option<String>,

    /// Show commits older than DATE
    #[clap(long, visible_alias = "before", value_name = "DATE")]
    pub until: Option<String>,

    /// Print only commits with at least two parents
    #[clap(long)]
    pub merges: bool,

    /// Omit commits with at least two parents
    #[clap(long = "no-merges")]
    pub no_merges: bool,

    /// Print only commits with at least N parents
    #[clap(long = "min-parents", value_name = "N")]
    pub min_parents: Option<usize>,

    /// Print only commits with at most N parents
    #[clap(long = "max-parents", value_name = "N")]
    pub max_parents: Option<usize>,

    /// Clear the lower parent-count bound
    #[clap(long = "no-min-parents")]
    pub no_min_parents: bool,

    /// Clear the upper parent-count bound
    #[clap(long = "no-max-parents")]
    pub no_max_parents: bool,

    /// Revisions to include or exclude. Defaults to HEAD when omitted.
    #[clap(value_name = "SPEC")]
    pub specs: Vec<String>,

    /// Paths to limit the commit list after an explicit `--` separator
    #[clap(last = true, value_name = "PATH")]
    pub pathspecs: Vec<String>,
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
    } else {
        emit_human_rev_list(output, &result)
    }
}

async fn resolve_rev_list(args: &RevListArgs) -> CliResult<RevListOutput> {
    // `--all` seeds the walk with every ref tip; explicit specs (incl `^`
    // exclusions) are appended so they still apply.
    let specs = if args.all {
        let mut specs = all_ref_specs().await?;
        specs.extend(args.specs.iter().cloned());
        specs
    } else {
        args.specs.clone()
    };
    // `--all` supplies the ref set as the input; don't fall back to HEAD when
    // that set (plus explicit specs) is empty (e.g. an unborn repository).
    let selection = resolve_revision_selection(&specs, args.first_parent, !args.all).await?;
    let mut commits = selection.commits;
    sort_rev_list_commits(&mut commits);
    let children = build_rev_list_children(&commits);
    let commits = filter_commits_by_pathspecs(commits, &args.pathspecs).await?;
    let commits = attach_cherry_metadata(commits, &selection.sides);
    let commits = apply_cherry_filters(commits, args)?;
    let time_window = rev_list_time_window(args)?;
    let author_filter = rev_list_author_filter(args);
    let committer_filter = rev_list_committer_filter(args);
    let message_filter = rev_list_message_filter(args)?;
    let parent_filter = parent_count_filter(args);

    let mut commits = commits
        .into_iter()
        .filter(|selected| commit_matches_author(&selected.commit, author_filter.as_deref()))
        .filter(|selected| commit_matches_committer(&selected.commit, committer_filter.as_deref()))
        .filter(|selected| commit_matches_message(&selected.commit, message_filter.as_ref()))
        .filter(|selected| commit_matches_time_window(&selected.commit, time_window))
        .filter(|selected| commit_matches_parent_count(&selected.commit, parent_filter))
        .skip(args.skip)
        .take(args.max_count.unwrap_or(usize::MAX))
        .collect::<Vec<_>>();
    // `--reverse` reverses the already-limited selection (Git applies commit
    // limiting first, then reverses for output). Order-independent `--count` is
    // unaffected.
    if args.reverse {
        commits.reverse();
    }
    let count_fields = if args.count {
        rev_list_count_fields(&commits, args)
    } else {
        Vec::new()
    };
    let entries = if args.count
        || (!args.parents
            && !args.children
            && !args.timestamp
            && !args.left_right
            && !args.cherry_mark
            && !args.cherry)
    {
        None
    } else {
        Some(
            commits
                .iter()
                .map(|selected| RevListEntry {
                    commit: selected.commit.id.to_string(),
                    side: selected.side,
                    cherry_equivalent: (args.cherry_mark || args.cherry)
                        .then_some(selected.cherry_equivalent),
                    parents: if args.parents {
                        selected
                            .commit
                            .parent_commit_ids
                            .iter()
                            .map(ToString::to_string)
                            .collect()
                    } else {
                        Vec::new()
                    },
                    children: if args.children {
                        children
                            .get(&selected.commit.id.to_string())
                            .cloned()
                            .unwrap_or_default()
                    } else {
                        Vec::new()
                    },
                    timestamp: args
                        .timestamp
                        .then_some(selected.commit.committer.timestamp),
                })
                .collect(),
        )
    };
    let commits = commits
        .iter()
        .map(|selected| selected.commit.id.to_string())
        .collect::<Vec<_>>();
    let total = commits.len();

    Ok(RevListOutput {
        input: selection.input,
        inputs: selection.inputs,
        commits,
        entries,
        total,
        count_fields,
        count_only: args.count,
        parents: args.parents,
        children: args.children,
        timestamp: args.timestamp,
        first_parent: args.first_parent,
        author: args.author.clone(),
        committer: args.committer.clone(),
        grep: args.grep.clone(),
        pathspecs: args.pathspecs.clone(),
        left_right: args.left_right,
        left_only: args.left_only,
        right_only: args.right_only,
        cherry_pick: args.cherry_pick,
        cherry_mark: args.cherry_mark,
        cherry: args.cherry,
        since: args.since.clone(),
        until: args.until.clone(),
        merges: args.merges,
        no_merges: args.no_merges,
        min_parents: args.min_parents,
        max_parents: args.max_parents,
        no_min_parents: args.no_min_parents,
        no_max_parents: args.no_max_parents,
        max_count: args.max_count,
        skip: args.skip,
    })
}

/// Collect a resolvable spec for every ref (local branches, remote-tracking
/// branches, and tags) for `--all`. Branches/remote-tracking refs contribute
/// their tip commit hash directly (unambiguous); tags contribute their name so
/// the normal spec resolver peels annotated tags. The resolver de-duplicates
/// the resulting commits.
async fn all_ref_specs() -> CliResult<Vec<String>> {
    let mut specs = Vec::new();

    // Git's `--all` seeds from every ref in refs/ AND the current HEAD, so a
    // detached-HEAD commit not pointed to by any branch/tag is still walked.
    // An unborn HEAD (None) contributes nothing; the resolver de-duplicates a
    // HEAD that coincides with a branch tip.
    if let Some(head_commit) = Head::current_commit_result().await.map_err(|source| {
        CliError::fatal(format!("failed to resolve HEAD: {source}"))
            .with_stable_code(StableErrorCode::IoReadFailed)
    })? {
        specs.push(head_commit.to_string());
    }

    let branches = Branch::list_branches_result(None).await.map_err(|source| {
        CliError::fatal(format!("failed to list branches: {source}"))
            .with_stable_code(StableErrorCode::IoReadFailed)
    })?;
    for branch in branches {
        specs.push(branch.commit.to_string());
    }

    let remotes = ConfigKv::all_remote_configs().await.map_err(|source| {
        CliError::fatal(format!("failed to list remotes: {source}"))
            .with_stable_code(StableErrorCode::IoReadFailed)
    })?;
    for remote in remotes {
        let remote_branches = Branch::list_branches_result(Some(&remote.name))
            .await
            .map_err(|source| {
                CliError::fatal(format!("failed to list remote branches: {source}"))
                    .with_stable_code(StableErrorCode::IoReadFailed)
            })?;
        for branch in remote_branches {
            specs.push(branch.commit.to_string());
        }
    }

    let tags = tag::list().await.map_err(|source| {
        CliError::fatal(format!("failed to list tags: {source}"))
            .with_stable_code(StableErrorCode::IoReadFailed)
    })?;
    for t in tags {
        // Seed by the ref target's object id (unambiguous) rather than the tag
        // name — a same-named branch would otherwise shadow the tag and drop
        // its tag-only commits. The resolver peels annotated-tag objects.
        let oid = match &t.object {
            tag::TagObject::Commit(commit) => commit.id,
            tag::TagObject::Tag(tag_obj) => tag_obj.id,
            tag::TagObject::Tree(tree) => tree.id,
            tag::TagObject::Blob(blob) => blob.id,
        };
        specs.push(oid.to_string());
    }

    Ok(specs)
}

fn rev_list_count_fields(commits: &[RevListSelectedCommit], args: &RevListArgs) -> Vec<usize> {
    if args.left_right && (args.cherry_mark || args.cherry) {
        return vec![
            side_count(commits, rev_list_spec::RevListSide::Left, false),
            side_count(commits, rev_list_spec::RevListSide::Right, false),
            commits
                .iter()
                .filter(|selected| selected.cherry_equivalent)
                .count(),
        ];
    }

    if args.left_right {
        return vec![
            side_total(commits, rev_list_spec::RevListSide::Left),
            side_total(commits, rev_list_spec::RevListSide::Right),
        ];
    }

    if args.cherry_mark || args.cherry {
        return vec![
            commits
                .iter()
                .filter(|selected| !selected.cherry_equivalent)
                .count(),
            commits
                .iter()
                .filter(|selected| selected.cherry_equivalent)
                .count(),
        ];
    }

    vec![commits.len()]
}

fn side_total(commits: &[RevListSelectedCommit], side: rev_list_spec::RevListSide) -> usize {
    commits
        .iter()
        .filter(|selected| selected.side == Some(side))
        .count()
}

fn side_count(
    commits: &[RevListSelectedCommit],
    side: rev_list_spec::RevListSide,
    cherry_equivalent: bool,
) -> usize {
    commits
        .iter()
        .filter(|selected| {
            selected.side == Some(side) && selected.cherry_equivalent == cherry_equivalent
        })
        .count()
}

#[cfg(test)]
#[path = "rev_list_output_tests.rs"]
mod output_tests;
#[cfg(test)]
#[path = "rev_list_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "rev_list_write_tests.rs"]
mod write_tests;
