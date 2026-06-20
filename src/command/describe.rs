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

#[path = "describe_format.rs"]
mod describe_format;
#[path = "describe_types.rs"]
mod describe_types;
use describe_format::{abbreviate_hash, describe_output};
use describe_types::{DescribeError, DescribeOutput};

const DESCRIBE_EXAMPLES: &str = "\
EXAMPLES:
    libra describe                  Describe HEAD using the nearest annotated tag
    libra describe --tags           Include lightweight tags (not just annotated ones) in the search
    libra describe --always         Fall back to abbreviated commit hash when no tag matches
    libra describe --exact-match    Only succeed when HEAD exactly matches a tag
    libra describe --long           Force tag-0-gHASH form for exact tag matches
    libra describe --dirty          Append -dirty when tracked content differs from HEAD
    libra describe --first-parent   Follow only the first parent of merge commits when walking history
    libra describe --match 'v1.*'   Only consider tags whose name matches the glob
    libra describe --exclude '*rc*' Skip tags whose name matches the glob
    libra describe HEAD~1           Describe a specific commit-ish (hash, ref, or HEAD~N)
    libra describe --abbrev 12      Use 12 hex digits instead of the default 7 in the hash portion
    libra describe --json           Structured JSON output for agents";

/// Maximum byte length accepted for a `--match`/`--exclude` glob pattern, guarding
/// against pathological inputs. Longer patterns are rejected up front with
/// [`DescribeError::InvalidArgument`] (`CliInvalidArguments`, exit 129).
const MAX_GLOB_LEN: usize = 256;

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

    /// Always output the long format when a tag describes the target.
    #[clap(long)]
    pub long: bool,

    /// Append MARK when tracked content differs from HEAD.
    #[clap(long, value_name = "MARK", num_args = 0..=1, require_equals = true, default_missing_value = "-dirty")]
    pub dirty: Option<String>,

    /// Follow only the first parent of merge commits when walking history.
    #[clap(long = "first-parent")]
    pub first_parent: bool,

    /// Only consider tags whose name matches the glob (repeatable; OR semantics).
    #[clap(long = "match", value_name = "PATTERN")]
    pub match_patterns: Vec<String>,

    /// Exclude tags whose name matches the glob (repeatable; takes precedence over --match).
    #[clap(long, value_name = "PATTERN")]
    pub exclude: Vec<String>,
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
    let long_format = args.long;
    if long_format && abbrev == 0 {
        return Err(DescribeError::LongWithAbbrevZero);
    }
    let include_lightweight = args.tags;
    let exact_match = args.exact_match;
    let always = args.always;
    let dirty_mark = args.dirty;
    let first_parent = args.first_parent;

    // Compile the --match / --exclude name filters once. Overly long or malformed
    // patterns are rejected up front as usage errors (CliInvalidArguments, 129).
    let matchers = compile_globs(&args.match_patterns)?;
    let excluders = compile_globs(&args.exclude)?;

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
            // Apply --match / --exclude name filters (exclude wins over match).
            if !tag_passes_filters(&tag_name, &matchers, &excluders) {
                continue;
            }
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
                long_format,
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

        // With --first-parent only the first parent is followed, so merge commits
        // do not pull in their merged-in side history.
        let parents = commit.parent_commit_ids;
        let parents: &[ObjectHash] = if first_parent {
            &parents[..parents.len().min(1)]
        } else {
            &parents
        };
        for parent_id_str in parents {
            if !visited.contains(parent_id_str) {
                visited.insert(*parent_id_str);
                queue.push_back((*parent_id_str, dist + 1));
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
            long_format,
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

/// Compile `--match`/`--exclude` glob patterns, rejecting overly long or malformed
/// patterns with [`DescribeError::InvalidArgument`] (`CliInvalidArguments`, exit 129).
/// Returned globs borrow `patterns`, so the slice must outlive the filter loop.
fn compile_globs(patterns: &[String]) -> Result<Vec<wax::Glob<'_>>, DescribeError> {
    let mut globs = Vec::with_capacity(patterns.len());
    for pattern in patterns {
        if pattern.len() > MAX_GLOB_LEN {
            return Err(DescribeError::InvalidArgument(format!(
                "glob pattern too long ({} chars); the limit is {MAX_GLOB_LEN}",
                pattern.len()
            )));
        }
        let glob = wax::Glob::new(pattern.as_str()).map_err(|error| {
            DescribeError::InvalidArgument(format!("invalid glob pattern '{pattern}': {error}"))
        })?;
        globs.push(glob);
    }
    Ok(globs)
}

/// Whether a tag name survives the `--match`/`--exclude` filters. An exclude match
/// always rejects; with no `--match` patterns every non-excluded name passes,
/// otherwise the name must match at least one `--match` glob.
fn tag_passes_filters(name: &str, matchers: &[wax::Glob<'_>], excluders: &[wax::Glob<'_>]) -> bool {
    if excluders
        .iter()
        .any(|glob| wax::Program::is_match(glob, name))
    {
        return false;
    }
    if matchers.is_empty() {
        return true;
    }
    matchers
        .iter()
        .any(|glob| wax::Program::is_match(glob, name))
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
        DescribeError::LongWithAbbrevZero => CliError::command_usage(error.to_string())
            .with_stable_code(StableErrorCode::CliInvalidArguments)
            .with_hint("omit '--long' or choose a positive '--abbrev <N>'."),
        DescribeError::InvalidArgument(message) => CliError::command_usage(message)
            .with_stable_code(StableErrorCode::CliInvalidArguments)
            .with_hint("check the --match/--exclude glob syntax."),
        DescribeError::LoadCommit { commit_id, detail } => {
            CliError::fatal(format!("failed to load commit '{commit_id}': {detail}"))
                .with_stable_code(StableErrorCode::RepoCorrupt)
        }
    }
}

#[cfg(test)]
#[path = "describe_tests.rs"]
mod tests;
