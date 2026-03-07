//! Implementation of `describe` command, which finds the most recent tag reachable from a commit.
use std::{
    collections::{HashMap, HashSet, VecDeque},
    str::FromStr,
};

use clap::Parser;
use git_internal::{
    hash::ObjectHash,
    internal::object::{commit::Commit, types::ObjectType},
};

use crate::{
    command::load_object,
    internal::{
        head::Head,
        tag::{self, TagObject},
    },
    utils::{
        error::{CliError, CliResult},
        util,
    },
};

#[derive(Parser, Debug)]
pub struct DescribeArgs {
    // The commit object name, Defaults to HEAD.
    pub commit: Option<String>,

    // Instead of only using annotated tags, use any tag found in refs/tags namespace.
    #[clap(long)]
    pub tags: bool,

    // Instead of using the default 7 hexadecimal digits as the abbreviated object name, use <n> digits.
    #[clap(long)]
    pub abbrev: Option<usize>,
}

// Entry in tag lookup map
struct TagInfo {
    name: String,
    #[allow(dead_code)]
    is_annotated: bool,
}

pub async fn execute(args: DescribeArgs) {
    if let Err(e) = execute_safe(args).await {
        eprintln!("{}", e.render());
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting.
pub async fn execute_safe(args: DescribeArgs) -> CliResult<()> {
    util::require_repo().map_err(|_| CliError::repo_not_found())?;
    execute_inner(args)
        .await
        .map_err(CliError::from_legacy_string)
}

async fn execute_inner(args: DescribeArgs) -> Result<(), String> {
    // 1. Confirm the starting commit hash to start from (defaults to HEAD)
    let start_hash_str = if let Some(c) = args.commit {
        c
    } else {
        Head::current_commit()
            .await
            .ok_or("fatal: no commit at HEAD")?
            .to_string()
    };
    let start_hash = ObjectHash::from_str(&start_hash_str)
        .map_err(|_| format!("fatal: Not a valid object name {}", start_hash_str))?;

    // 2. Load all tags and build a mapping table: commit hash -> tag info (name, is_annotated)
    let all_tags = tag::list().await.map_err(|e| format!("fatal: {}", e))?;
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
            let output = format_describe_result(
                &tag_info.name,
                dist,
                &start_hash_str,
                args.abbrev.unwrap_or(7),
            );
            println!("{}", output);
            return Ok(());
        }

        // Load commit to find parents
        let commit = load_object::<Commit>(&curr_hash)
            .map_err(|_| format!("fatal: failed to load commit {}", curr_hash))?;

        for parent_id_str in commit.parent_commit_ids {
            // INVARIANT: parent IDs stored in commits are always valid hex hashes.
            let parent_hash = ObjectHash::from_str(&parent_id_str.to_string()).unwrap();
            if !visited.contains(&parent_hash) {
                visited.insert(parent_hash);
                queue.push_back((parent_hash, dist + 1));
            }
        }
    }

    // If the tag is not found after traversing the entire history record, return an error
    Err("fatal: No names found, cannot describe anything.".to_string())
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
