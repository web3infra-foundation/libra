//! Implements `rev-list` to enumerate commits reachable from revisions.

use clap::Parser;

use crate::utils::{
    error::{CliError, CliResult},
    output::{OutputConfig, emit_json_data},
    util,
};

#[path = "rev_list_filter.rs"]
mod rev_list_filter;
#[path = "rev_list_output.rs"]
mod rev_list_output;
#[path = "rev_list_spec.rs"]
mod rev_list_spec;

#[cfg(test)]
use rev_list_filter::ParentCountFilter;
use rev_list_filter::{
    commit_matches_parent_count, commit_matches_time_window, parent_count_filter,
    rev_list_time_window, sort_rev_list_commits,
};
use rev_list_output::{REV_LIST_EXAMPLES, RevListEntry, RevListOutput, emit_human_rev_list};
#[cfg(test)]
use rev_list_output::{format_rev_list_entry, write_rev_list_count, write_rev_list_output};
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

    /// Print only the number of commits after filters
    #[clap(long)]
    pub count: bool,

    /// Print parent commit IDs after each commit
    #[clap(long)]
    pub parents: bool,

    /// Prefix each output line with the commit timestamp
    #[clap(long)]
    pub timestamp: bool,

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
    let selection = resolve_revision_selection(&args.specs).await?;
    let mut commits = selection.commits;
    sort_rev_list_commits(&mut commits);
    let time_window = rev_list_time_window(args)?;
    let parent_filter = parent_count_filter(args);

    let commits = commits
        .into_iter()
        .filter(|commit| commit_matches_time_window(commit, time_window))
        .filter(|commit| commit_matches_parent_count(commit, parent_filter))
        .skip(args.skip)
        .take(args.max_count.unwrap_or(usize::MAX))
        .collect::<Vec<_>>();
    let entries = if args.count || (!args.parents && !args.timestamp) {
        None
    } else {
        Some(
            commits
                .iter()
                .map(|commit| RevListEntry {
                    commit: commit.id.to_string(),
                    parents: if args.parents {
                        commit
                            .parent_commit_ids
                            .iter()
                            .map(ToString::to_string)
                            .collect()
                    } else {
                        Vec::new()
                    },
                    timestamp: args.timestamp.then_some(commit.committer.timestamp),
                })
                .collect(),
        )
    };
    let commits = commits
        .iter()
        .map(|commit| commit.id.to_string())
        .collect::<Vec<_>>();
    let total = commits.len();

    Ok(RevListOutput {
        input: selection.input,
        inputs: selection.inputs,
        commits,
        entries,
        total,
        count_only: args.count,
        parents: args.parents,
        timestamp: args.timestamp,
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

#[cfg(test)]
#[path = "rev_list_tests.rs"]
mod tests;
