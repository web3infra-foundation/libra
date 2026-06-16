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

/// `--help` examples shown in `libra rev-list --help` output.
///
/// `rev-list` walks the commit graph from the given spec (default
/// `HEAD`) and prints each reachable commit hash on its own line. The
/// banner pins the default `HEAD` walk, an explicit branch walk, a
/// quiet form, and a JSON variant for agents so users see all
/// supported forms without reading the design doc. Cross-cutting
/// `--help` EXAMPLES rollout per `docs/development/commands/_general.md` item B.
pub const REV_LIST_EXAMPLES: &str = "\
EXAMPLES:
    libra rev-list                  Walk ancestry from HEAD (one hash per line)
    libra rev-list --count HEAD     Count reachable commits after filters
    libra rev-list -n 5 HEAD        Limit output to the first five commits
    libra rev-list --parents HEAD   Include parent commit IDs on each line
    libra rev-list --timestamp HEAD Prefix each line with the committer timestamp
    libra rev-list main             Walk ancestry from refs/heads/main
    libra rev-list HEAD~5           Walk ancestry from a relative ref
    libra rev-list --json HEAD      Structured JSON output (input + commits[] + total)
    libra rev-list --quiet HEAD     Suppress stdout (use exit code as truthy probe)";

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

    /// Revision to list from. Defaults to HEAD when omitted.
    #[clap(value_name = "SPEC")]
    pub spec: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct RevListEntry {
    commit: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    parents: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timestamp: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
struct RevListOutput {
    input: String,
    commits: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    entries: Option<Vec<RevListEntry>>,
    total: usize,
    count_only: bool,
    parents: bool,
    timestamp: bool,
    max_count: Option<usize>,
    skip: usize,
}

impl RevListOutput {
    fn human_lines(&self) -> Vec<String> {
        if let Some(entries) = &self.entries {
            return entries
                .iter()
                .map(|entry| format_rev_list_entry(entry, self.parents, self.timestamp))
                .collect();
        }

        self.commits.clone()
    }
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
    } else if result.count_only {
        let stdout = std::io::stdout();
        let mut writer = stdout.lock();
        write_rev_list_count(&mut writer, result.total)
    } else {
        let stdout = std::io::stdout();
        let mut writer = stdout.lock();
        write_rev_list_output(&mut writer, &result.human_lines())
    }
}

fn write_rev_list_count<W: Write>(writer: &mut W, total: usize) -> CliResult<()> {
    match writeln!(writer, "{total}") {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(
            CliError::fatal(format!("failed to write rev-list output: {error}"))
                .with_stable_code(StableErrorCode::IoWriteFailed),
        ),
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
        input: spec.to_string(),
        commits,
        entries,
        total,
        count_only: args.count,
        parents: args.parents,
        timestamp: args.timestamp,
        max_count: args.max_count,
        skip: args.skip,
    })
}

fn format_rev_list_entry(entry: &RevListEntry, show_parents: bool, show_timestamp: bool) -> String {
    let mut fields = Vec::new();
    if show_timestamp && let Some(timestamp) = entry.timestamp {
        fields.push(timestamp.to_string());
    }
    fields.push(entry.commit.clone());
    if show_parents {
        fields.extend(entry.parents.iter().cloned());
    }
    fields.join(" ")
}

fn sort_rev_list_commits(commits: &mut [git_internal::internal::object::commit::Commit]) {
    // `sort_by_key` is stable, so equal timestamps keep the traversal order
    // returned by `get_reachable_commits` (HEAD before parent in linear history).
    commits.sort_by_key(|commit| std::cmp::Reverse(commit.committer.timestamp));
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
#[path = "rev_list_tests.rs"]
mod tests;
