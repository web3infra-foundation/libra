//! Implementation of `describe` command, which finds the most recent tag reachable from a commit.
use std::collections::{HashMap, HashSet, VecDeque};

use clap::Parser;
use git_internal::{
    hash::ObjectHash,
    internal::object::{commit::Commit, types::ObjectType},
};

use crate::{
    command::{load_object, status},
    internal::tag::{self, TagObject},
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
        util,
    },
};

#[path = "describe_types.rs"]
mod describe_types;
use describe_types::{DescribeError, DescribeOutput};

const DESCRIBE_EXAMPLES: &str = "\
EXAMPLES:
    libra describe                  Describe HEAD using the nearest annotated tag
    libra describe --tags           Include lightweight tags (not just annotated ones) in the search
    libra describe --always         Fall back to abbreviated commit hash when no tag matches
    libra describe --exact-match    Only succeed when HEAD exactly matches a tag
    libra describe --dirty          Append -dirty when tracked content differs from HEAD
    libra describe HEAD~1           Describe a specific commit-ish (hash, ref, or HEAD~N)
    libra describe --abbrev 12      Use 12 hex digits instead of the default 7 in the hash portion
    libra describe --json           Structured JSON output for agents";

#[derive(Parser, Debug)]
#[command(after_help = DESCRIBE_EXAMPLES)]
pub struct DescribeArgs {
    /// Commit-ish (hash, ref, or tag) to describe. Defaults to HEAD
    pub commit: Option<String>,

    /// Consider any tag in refs/tags (not just annotated tags) when describing
    #[clap(long)]
    pub tags: bool,

    /// Use N hex digits for the abbreviated commit hash (default: 7)
    #[clap(long, value_name = "N")]
    pub abbrev: Option<usize>,

    /// Show an abbreviated commit hash when no tag can describe the target.
    #[clap(long)]
    pub always: bool,

    /// Only output exact tag matches.
    #[clap(long)]
    pub exact_match: bool,

    /// Append MARK when tracked content differs from HEAD.
    #[clap(long, value_name = "MARK", num_args = 0..=1, require_equals = true, default_missing_value = "-dirty")]
    pub dirty: Option<String>,
}

// Entry in tag lookup map
struct TagInfo {
    name: String,
    is_annotated: bool,
}

pub async fn execute(args: DescribeArgs) {
    if let Err(e) = execute_safe(args, &OutputConfig::default()).await {
        e.print_stderr();
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting.
pub async fn execute_safe(args: DescribeArgs, output: &OutputConfig) -> CliResult<()> {
    util::require_repo().map_err(|_| CliError::repo_not_found())?;
    let describe_output = run_describe(args).await.map_err(describe_cli_error)?;

    if output.is_json() {
        emit_json_data("describe", &describe_output, output)?;
    } else if !output.quiet {
        println!("{}", describe_output.result);
    }

    Ok(())
}

async fn run_describe(args: DescribeArgs) -> Result<DescribeOutput, DescribeError> {
    let input = args.commit.unwrap_or_else(|| "HEAD".to_string());
    let start_hash = util::get_commit_base_typed(&input)
        .await
        .map_err(DescribeError::from)?;
    let resolved_commit = start_hash.to_string();
    let abbrev = args.abbrev.unwrap_or(7);
    let include_lightweight = args.tags;
    let exact_match = args.exact_match;
    let always = args.always;
    let dirty_mark = args.dirty;

    // 2. Load all tags and build a mapping table: commit hash -> tag info (name, is_annotated)
    let all_tags = tag::list()
        .await
        .map_err(|e| DescribeError::CorruptReference(e.to_string()))?;
    let mut tag_map: HashMap<ObjectHash, TagInfo> = HashMap::new();

    for t in all_tags {
        let is_annotated = t.object.get_type() == ObjectType::Tag;

        // Only include light-weight tags if --tags is specified
        if is_annotated || include_lightweight {
            let tag_name = t.name;
            let target_commit_hash = match t.object {
                TagObject::Commit(c) => c.id,
                TagObject::Tag(tg) => tg.object_hash,
                _ => continue,
            };

            let should_replace = tag_map
                .get(&target_commit_hash)
                .is_none_or(|existing| prefer_tag(existing, &tag_name, is_annotated));
            if should_replace {
                tag_map.insert(
                    target_commit_hash,
                    TagInfo {
                        name: tag_name,
                        is_annotated,
                    },
                );
            }
        }
    }

    // 3. Search for  the closest tag using BFS (to find the shortest distance)
    let mut queue = VecDeque::new();
    let mut visited = HashSet::new();

    // Queue storage format: (current_commit_hash, distance_from_start)
    queue.push_back((start_hash, 0));
    visited.insert(start_hash);

    while let Some((curr_hash, dist)) = queue.pop_front() {
        // Check if current commit has a matching tag
        if let Some(tag_info) = tag_map.get(&curr_hash) {
            let output = describe_output(
                input.clone(),
                resolved_commit.clone(),
                &tag_info.name,
                dist,
                abbrev,
            );
            return apply_dirty_mark(output, dirty_mark).await;
        }

        if exact_match {
            break;
        }

        // Load commit to find parents
        let commit =
            load_object::<Commit>(&curr_hash).map_err(|error| DescribeError::LoadCommit {
                commit_id: curr_hash.to_string(),
                detail: error.to_string(),
            })?;

        for parent_id_str in commit.parent_commit_ids {
            if !visited.contains(&parent_id_str) {
                visited.insert(parent_id_str);
                queue.push_back((parent_id_str, dist + 1));
            }
        }
    }

    if exact_match {
        return Err(DescribeError::NoExactMatch {
            commit_id: resolved_commit,
        });
    }

    if always {
        let abbreviated = abbreviate_hash(&resolved_commit, abbrev);
        let output = DescribeOutput {
            input,
            resolved_commit,
            result: abbreviated.clone(),
            tag: None,
            distance: None,
            abbreviated_commit: Some(abbreviated),
            exact_match: false,
            used_always: true,
            dirty: false,
            dirty_mark: None,
        };
        return apply_dirty_mark(output, dirty_mark).await;
    }

    Err(DescribeError::NoNamesFound)
}

async fn apply_dirty_mark(
    mut output: DescribeOutput,
    dirty_mark: Option<String>,
) -> Result<DescribeOutput, DescribeError> {
    if let Some(mark) = dirty_mark
        && has_tracked_dirty_changes().await?
    {
        output.result.push_str(&mark);
        output.dirty = true;
        output.dirty_mark = Some(mark);
    }

    Ok(output)
}

async fn has_tracked_dirty_changes() -> Result<bool, DescribeError> {
    let staged = status::changes_to_be_committed_safe()
        .await
        .map_err(|error| DescribeError::ReadFailure(format!("{error}")))?;
    if !staged.is_empty() {
        return Ok(true);
    }

    let unstaged = status::changes_to_be_staged()
        .map_err(|error| DescribeError::ReadFailure(format!("{error}")))?;
    Ok(!unstaged.modified.is_empty()
        || !unstaged.deleted.is_empty()
        || !unstaged.renamed.is_empty())
}

// Formats the output string based on Git's describe rules.
fn format_describe_result(tag_name: &str, dist: usize, full_sha: &str, abbrev: usize) -> String {
    if dist == 0 || abbrev == 0 {
        // If the current commit is exactly at the tag, just return the tag name
        tag_name.to_string()
    } else {
        // Extract the abbreviated hash based on the specified length (default 7)
        let short_sha = abbreviate_hash(full_sha, abbrev);
        // format: <tag_name>-<distance>-g<abbreviated_sha>
        format!("{}-{}-g{}", tag_name, dist, short_sha)
    }
}

fn describe_output(
    input: String,
    resolved_commit: String,
    tag_name: &str,
    distance: usize,
    abbrev: usize,
) -> DescribeOutput {
    let abbreviated_commit =
        (distance > 0 && abbrev > 0).then(|| abbreviate_hash(&resolved_commit, abbrev));
    DescribeOutput {
        input,
        resolved_commit: resolved_commit.clone(),
        result: format_describe_result(tag_name, distance, &resolved_commit, abbrev),
        tag: Some(tag_name.to_string()),
        distance: Some(distance),
        abbreviated_commit,
        exact_match: distance == 0,
        used_always: false,
        dirty: false,
        dirty_mark: None,
    }
}

fn abbreviate_hash(full_sha: &str, abbrev: usize) -> String {
    if abbrev == 0 || abbrev >= full_sha.len() {
        full_sha.to_string()
    } else {
        full_sha[..abbrev].to_string()
    }
}

fn prefer_tag(existing: &TagInfo, candidate_name: &str, candidate_is_annotated: bool) -> bool {
    match (existing.is_annotated, candidate_is_annotated) {
        (false, true) => true,
        (true, false) => false,
        _ => candidate_name < existing.name.as_str(),
    }
}

fn describe_cli_error(error: DescribeError) -> CliError {
    match error {
        DescribeError::HeadUnborn => CliError::fatal(error.to_string())
            .with_stable_code(StableErrorCode::RepoStateInvalid)
            .with_hint("create a commit before running 'libra describe'."),
        DescribeError::InvalidReference(message) => CliError::command_usage(message)
            .with_stable_code(StableErrorCode::CliInvalidTarget)
            .with_hint("check the revision and try again."),
        DescribeError::ReadFailure(message) => {
            CliError::fatal(message).with_stable_code(StableErrorCode::IoReadFailed)
        }
        DescribeError::CorruptReference(message) => {
            CliError::fatal(message).with_stable_code(StableErrorCode::RepoCorrupt)
        }
        DescribeError::NoNamesFound => CliError::fatal("no names found, cannot describe anything")
            .with_stable_code(StableErrorCode::RepoStateInvalid)
            .with_hint(
                "create a tag, pass '--tags' to include lightweight tags, or use '--always'.",
            ),
        DescribeError::NoExactMatch { commit_id } => {
            CliError::fatal(format!("no tag exactly matches '{commit_id}'"))
                .with_stable_code(StableErrorCode::RepoStateInvalid)
                .with_hint("move to a tagged commit or omit '--exact-match'.")
        }
        DescribeError::LoadCommit { commit_id, detail } => {
            CliError::fatal(format!("failed to load commit '{commit_id}': {detail}"))
                .with_stable_code(StableErrorCode::RepoCorrupt)
        }
    }
}

#[cfg(test)]
#[path = "describe_tests.rs"]
mod tests;
