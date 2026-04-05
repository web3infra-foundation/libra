//! Implementation of `describe` command, which finds the most recent tag reachable from a commit.
use std::collections::{HashMap, HashSet, VecDeque};

use clap::Parser;
use git_internal::{
    hash::ObjectHash,
    internal::object::{commit::Commit, types::ObjectType},
};
use serde::Serialize;

use crate::{
    command::load_object,
    internal::{
        head::Head,
        tag::{self, TagObject},
    },
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
        util::{self, CommitBaseError},
    },
};

const DESCRIBE_EXAMPLES: &str = "\
EXAMPLES:
  libra describe
  libra describe --tags
  libra describe --always
  libra describe HEAD~1
  libra describe --json
";

#[derive(Parser, Debug)]
#[command(after_help = DESCRIBE_EXAMPLES)]
pub struct DescribeArgs {
    // The commit object name, Defaults to HEAD.
    pub commit: Option<String>,

    // Instead of only using annotated tags, use any tag found in refs/tags namespace.
    #[clap(long)]
    pub tags: bool,

    // Instead of using the default 7 hexadecimal digits as the abbreviated object name, use <n> digits.
    #[clap(long)]
    pub abbrev: Option<usize>,

    /// Show an abbreviated commit hash when no tag can describe the target.
    #[clap(long)]
    pub always: bool,
}

// Entry in tag lookup map
struct TagInfo {
    name: String,
    #[allow(dead_code)]
    is_annotated: bool,
}

#[derive(Debug, Clone, Serialize)]
struct DescribeOutput {
    input: String,
    resolved_commit: String,
    result: String,
    tag: Option<String>,
    distance: Option<usize>,
    abbreviated_commit: Option<String>,
    exact_match: bool,
    used_always: bool,
}

#[derive(Debug, thiserror::Error)]
enum DescribeError {
    #[error("HEAD does not point to a commit")]
    HeadUnborn,
    #[error("{0}")]
    InvalidReference(String),
    #[error("{0}")]
    ReadFailure(String),
    #[error("{0}")]
    CorruptReference(String),
    #[error("failed to load commit '{commit_id}': {detail}")]
    LoadCommit { commit_id: String, detail: String },
    #[error("no names found, cannot describe anything")]
    NoNamesFound,
}

impl From<CommitBaseError> for DescribeError {
    fn from(error: CommitBaseError) -> Self {
        match error {
            CommitBaseError::HeadUnborn => Self::HeadUnborn,
            CommitBaseError::InvalidReference(message) => Self::InvalidReference(message),
            CommitBaseError::ReadFailure(message) => Self::ReadFailure(message),
            CommitBaseError::CorruptReference(message) => Self::CorruptReference(message),
        }
    }
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
    let start_hash = if input.eq_ignore_ascii_case("HEAD") {
        Head::current_commit()
            .await
            .ok_or(DescribeError::HeadUnborn)?
    } else {
        util::get_commit_base_typed(&input)
            .await
            .map_err(DescribeError::from)?
    };
    let resolved_commit = start_hash.to_string();
    let abbrev = args.abbrev.unwrap_or(7);

    // 2. Load all tags and build a mapping table: commit hash -> tag info (name, is_annotated)
    let all_tags = tag::list()
        .await
        .map_err(|e| DescribeError::CorruptReference(e.to_string()))?;
    let mut tag_map: HashMap<ObjectHash, TagInfo> = HashMap::new();

    for t in all_tags {
        let is_annotated = t.object.get_type() == ObjectType::Tag;

        // Only include light-weight tags if --tags is specified
        if is_annotated || args.tags {
            let target_commit_hash = match t.object {
                TagObject::Commit(c) => c.id,
                TagObject::Tag(tg) => tg.object_hash,
                _ => continue,
            };

            // If multiple tags point to the same commit, annotated tags take precedence
            // Here use the entry.or_insert logic to prioritize the preservation of the tag discovered first
            tag_map.entry(target_commit_hash).or_insert(TagInfo {
                name: t.name,
                is_annotated,
            });
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
            return Ok(describe_output(
                input.clone(),
                resolved_commit.clone(),
                &tag_info.name,
                dist,
                abbrev,
            ));
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

    if args.always {
        let abbreviated = abbreviate_hash(&resolved_commit, abbrev);
        return Ok(DescribeOutput {
            input,
            resolved_commit,
            result: abbreviated.clone(),
            tag: None,
            distance: None,
            abbreviated_commit: Some(abbreviated),
            exact_match: false,
            used_always: true,
        });
    }

    Err(DescribeError::NoNamesFound)
}

// Formats the output string based on Git's describe rules.
fn format_describe_result(tag_name: &str, dist: usize, full_sha: &str, abbrev: usize) -> String {
    if dist == 0 {
        // If the current commit is exactly at the tag, just return the tag name
        tag_name.to_string()
    } else {
        // Extract the abbreviated hash based on the specified length (default 7)
        let short_sha = if abbrev >= full_sha.len() {
            full_sha
        } else {
            &full_sha[..abbrev]
        };
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
    let abbreviated_commit = (distance > 0).then(|| abbreviate_hash(&resolved_commit, abbrev));
    DescribeOutput {
        input,
        resolved_commit: resolved_commit.clone(),
        result: format_describe_result(tag_name, distance, &resolved_commit, abbrev),
        tag: Some(tag_name.to_string()),
        distance: Some(distance),
        abbreviated_commit,
        exact_match: distance == 0,
        used_always: false,
    }
}

fn abbreviate_hash(full_sha: &str, abbrev: usize) -> String {
    if abbrev >= full_sha.len() {
        full_sha.to_string()
    } else {
        full_sha[..abbrev].to_string()
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
        DescribeError::LoadCommit { commit_id, detail } => {
            CliError::fatal(format!("failed to load commit '{commit_id}': {detail}"))
                .with_stable_code(StableErrorCode::RepoCorrupt)
        }
    }
}
