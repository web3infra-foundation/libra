//! Range-diff command for comparing two commit ranges.
//!
//! This module implements `git range-diff` functionality. It compares two
//! commit ranges (e.g., `main..old-feature` and `main..rebased-feature`) and
//! shows how each commit evolved between the two ranges.
//!
//! # Algorithm
//! 1. Resolve both ranges to commit lists
//! 2. Compute patch-ids for each commit (SHA1 of normalized diff vs parent)
//! 3. Match commits between old and new ranges by patch-id
//! 4. For matched pairs with changes, compute diff-between-diffs
//! 5. Render output with color-coded status markers

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use clap::Parser;
use colored::*;
use git_internal::{
    Diff,
    hash::ObjectHash,
    internal::object::{blob::Blob, commit::Commit},
};
use serde::Serialize;
use sha1::{Digest, Sha1};

use crate::{
    command,
    utils::{
        error::{CliError, CliResult},
        output::{OutputConfig, emit_json_data},
        pager::Pager,
        util::require_repo,
    },
};

const RANGE_DIFF_EXAMPLES: &str = "\
EXAMPLES:
    libra range-diff main..old-feature main..new-feature
    libra range-diff main..feature rebased-feature
    libra range-diff --patch HEAD~3..HEAD main..rebased
    libra range-diff --json main..v1 main..v2";

#[derive(Parser, Debug)]
#[command(after_help = RANGE_DIFF_EXAMPLES)]
pub struct RangeDiffArgs {
    /// First commit range (e.g., `base..head` or a single ref defaults to HEAD..ref)
    #[arg(value_name = "OLD-RANGE")]
    pub old_range: String,

    /// Second commit range (e.g., `base..head` or a single ref defaults to HEAD..ref)
    #[arg(value_name = "NEW-RANGE")]
    pub new_range: String,

    /// Show the full diff-between-diffs for changed commits
    #[arg(long)]
    pub patch: bool,

    /// Minimum ratio of matching lines to consider a commit paired
    #[arg(
        long,
        default_value = "0.6",
        value_name = "RATIO",
        allow_hyphen_values = true
    )]
    pub creation_factor: f64,
}

// ── Entry points ───────────────────────────────────────────────────────────

/// User-facing entry point used by the CLI dispatcher.
pub async fn execute(args: RangeDiffArgs) {
    if let Err(e) = execute_safe(args, &OutputConfig::default()).await {
        e.print_stderr();
    }
}

/// Core implementation parameterized over `OutputConfig` for testability.
///
/// # Side Effects
/// Writes range-diff output to stdout (or JSON to stdout).
///
/// # Errors
/// Returns `CliError::fatal` for invalid refs, ranges, or repo state.
pub async fn execute_safe(args: RangeDiffArgs, output: &OutputConfig) -> CliResult<()> {
    require_repo().map_err(|e| CliError::fatal(e.to_string()))?;

    // 1. Parse ranges
    let (old_base_hash, old_head_hash) = parse_range(&args.old_range).await?;
    let (new_base_hash, new_head_hash) = parse_range(&args.new_range).await?;

    // 2. Collect commits in each range
    let old_commits = collect_range_commits(old_base_hash, old_head_hash).await?;
    let new_commits = collect_range_commits(new_base_hash, new_head_hash).await?;

    if old_commits.is_empty() && new_commits.is_empty() {
        if output.is_json() {
            let empty = RangeDiffOutput::default();
            return emit_json_data("range-diff", &empty, output);
        }
        let mut pager = Pager::with_config(output)?;
        pager.write_line("no commits in either range")?;
        return pager.finish();
    }

    // 3. Compute patch-ids with cached diff texts
    let old_entries = compute_patch_ids(&old_commits).await?;
    let new_entries = compute_patch_ids(&new_commits).await?;

    // 4. Match commits
    let range_entries = match_commits(&old_entries, &new_entries);

    // 5. Compute diff-between-diffs for changed pairs
    let range_entries = compute_diff_between_diffs(range_entries, &args).await?;

    // 6. Render output
    render_range_diff(&range_entries, &args, output)
}

// ── Range parsing ───────────────────────────────────────────────────────────

/// Parse a range string like `base..head` or a single ref into two `ObjectHash` values.
///
/// For a single ref (no `..`), defaults the base to HEAD.
async fn parse_range(raw: &str) -> Result<(ObjectHash, ObjectHash), CliError> {
    if let Some(dot_pos) = raw.find("..") {
        // Ensure it's not "..." (symmetric diff)
        if raw.as_bytes().get(dot_pos + 2) == Some(&b'.') {
            return Err(CliError::fatal(format!(
                "invalid range '{}': range-diff expects '..' not '...'",
                raw
            )));
        }
        let base = &raw[..dot_pos];
        let head = &raw[dot_pos + 2..];
        let base = if base.is_empty() { "HEAD" } else { base };
        let head = if head.is_empty() { "HEAD" } else { head };

        let base_hash = command::get_target_commit(base)
            .await
            .map_err(|e| CliError::fatal(format!("invalid old-range base '{}': {}", base, e)))?;
        let head_hash = command::get_target_commit(head)
            .await
            .map_err(|e| CliError::fatal(format!("invalid old-range head '{}': {}", head, e)))?;
        Ok((base_hash, head_hash))
    } else {
        // Single ref: base defaults to HEAD
        let head_hash = command::get_target_commit(raw)
            .await
            .map_err(|e| CliError::fatal(format!("invalid range '{}': {}", raw, e)))?;
        let base_hash = command::get_target_commit("HEAD")
            .await
            .map_err(|e| CliError::fatal(format!("cannot resolve HEAD: {}", e)))?;
        Ok((base_hash, head_hash))
    }
}

// ── Commit range collection ─────────────────────────────────────────────────

/// Collect all commits reachable from `head` but not from `base`.
///
/// Returns commits in oldest-first order to match `git range-diff` behavior.
async fn collect_range_commits(
    base: ObjectHash,
    head: ObjectHash,
) -> Result<Vec<Commit>, CliError> {
    if base == head {
        return Ok(Vec::new());
    }

    let head_commits = crate::command::log::get_reachable_commits(head.to_string(), None).await?;
    let base_commits = crate::command::log::get_reachable_commits(base.to_string(), None).await?;

    let base_set: HashSet<ObjectHash> = base_commits.into_iter().map(|c| c.id).collect();

    let mut range: Vec<Commit> = head_commits
        .into_iter()
        .filter(|c| !base_set.contains(&c.id))
        .collect();

    // BFS traversal returns newest-first; reverse to oldest-first
    range.reverse();
    Ok(range)
}

// ── Patch-ID computation ────────────────────────────────────────────────────

/// A commit paired with its patch-id and raw diff text.
struct PatchIdEntry {
    commit: Commit,
    patch_id: String,
    diff_text: String,
}

/// Compute patch-ids for all commits in a list.
async fn compute_patch_ids(commits: &[Commit]) -> Result<Vec<PatchIdEntry>, CliError> {
    let mut entries = Vec::with_capacity(commits.len());
    for commit in commits {
        let (patch_id, diff_text) = compute_patch_id(commit).await?;
        entries.push(PatchIdEntry {
            commit: commit.clone(),
            patch_id,
            diff_text,
        });
    }
    Ok(entries)
}

/// Compute the patch-id for a single commit.
///
/// The patch-id is the SHA1 hash of the normalized diff between the commit
/// and its first parent (or an empty tree for root commits).
async fn compute_patch_id(commit: &Commit) -> Result<(String, String), CliError> {
    let diff_text = resolve_diff_text(commit).await?;
    let normalized = normalize_diff_for_patch_id(&diff_text);
    let mut hasher = Sha1::new();
    hasher.update(normalized.as_bytes());
    let hash = hasher.finalize();
    Ok((hex::encode(hash), diff_text))
}

/// Get the diff text for a commit against its first parent.
async fn resolve_diff_text(commit: &Commit) -> Result<String, CliError> {
    let parent_blobs: Vec<(PathBuf, ObjectHash)> =
        if let Some(parent_id) = commit.parent_commit_ids.first() {
            get_commit_blobs(parent_id).await?
        } else {
            Vec::new()
        };

    let commit_blobs = get_commit_blobs(&commit.id).await?;

    let content_reader = |_path: &PathBuf, hash: &ObjectHash| -> Vec<u8> {
        command::load_object::<Blob>(hash)
            .map(|blob| blob.data)
            .unwrap_or_default()
    };

    let diff_items = Diff::diff(parent_blobs, commit_blobs, Vec::new(), content_reader);

    let diff_text: String = diff_items.iter().map(|item| item.data.clone()).collect();
    Ok(diff_text)
}

/// Get file blobs from a commit's tree.
async fn get_commit_blobs(
    commit_hash: &ObjectHash,
) -> Result<Vec<(PathBuf, ObjectHash)>, CliError> {
    use crate::utils::object_ext::TreeExt;
    use git_internal::internal::object::tree::Tree;

    let commit: Commit = command::load_object(commit_hash)
        .map_err(|e| CliError::fatal(format!("failed to load commit '{}': {}", commit_hash, e)))?;
    let tree: Tree = command::load_object(&commit.tree_id)
        .map_err(|e| CliError::fatal(format!("failed to load tree '{}': {}", commit.tree_id, e)))?;
    Ok(tree.get_plain_items())
}

/// Normalize a diff for patch-id computation, following Git's algorithm.
///
/// - Strip line numbers from hunk headers: `@@ -a,b +c,d @@` → `@@ -0,0 +0,0 @@`
/// - Strip metadata lines (`diff`, `index`, `---`, `+++`)
/// - Strip trailing whitespace from context lines
fn normalize_diff_for_patch_id(diff_text: &str) -> String {
    // Match hunk headers like "@@ -1,5 +2,6 @@"
    let hunk_header_prefix = "@@ -";
    let mut output = String::new();
    let mut in_hunk = false;

    for line in diff_text.lines() {
        if line.starts_with(hunk_header_prefix) {
            output.push_str("@@ -0,0 +0,0 @@\n");
            in_hunk = true;
        } else if in_hunk && line.starts_with(' ') {
            output.push_str(line.trim_end());
            output.push('\n');
        } else if in_hunk && (line.starts_with('+') || line.starts_with('-')) {
            output.push_str(line);
            output.push('\n');
        } else if line.starts_with("diff ")
            || line.starts_with("index ")
            || line.starts_with("--- ")
            || line.starts_with("+++ ")
        {
            // Skip metadata lines
            continue;
        } else {
            in_hunk = false;
        }
    }
    output
}

// ── Commit matching ─────────────────────────────────────────────────────────

/// Entry in the range-diff output.
#[derive(Debug, Clone)]
enum RangeDiffEntry {
    /// Commit removed from the new range.
    Removed { old_idx: usize, old_commit: Commit },
    /// Commit added in the new range.
    Added { new_idx: usize, new_commit: Commit },
    /// Commit unchanged between ranges.
    #[allow(dead_code)]
    Unchanged {
        old_idx: usize,
        old_commit: Commit,
        new_idx: usize,
        new_commit: Commit,
    },
    /// Commit modified between ranges (patch-id matched but content differs).
    Changed {
        old_idx: usize,
        old_commit: Commit,
        new_idx: usize,
        new_commit: Commit,
        diff_of_diffs: Option<String>,
    },
}

/// Match commits between old and new ranges by patch-id.
fn match_commits(
    old_entries: &[PatchIdEntry],
    new_entries: &[PatchIdEntry],
) -> Vec<RangeDiffEntry> {
    let mut new_by_patch_id: HashMap<&str, Vec<usize>> = HashMap::new();
    for (idx, entry) in new_entries.iter().enumerate() {
        new_by_patch_id
            .entry(&entry.patch_id)
            .or_default()
            .push(idx);
    }

    let mut matched_new: HashSet<usize> = HashSet::new();
    let mut entries = Vec::new();

    for (old_idx, old_entry) in old_entries.iter().enumerate() {
        if let Some(new_indices) = new_by_patch_id.get(old_entry.patch_id.as_str()) {
            // Take the first unmatched new commit with this patch-id
            if let Some(&new_idx) = new_indices.iter().find(|&&i| !matched_new.contains(&i)) {
                matched_new.insert(new_idx);
                let new_entry = &new_entries[new_idx];

                // Determine if unchanged or changed
                if old_entry.diff_text == new_entry.diff_text
                    && old_entry.commit.message.trim() == new_entry.commit.message.trim()
                {
                    entries.push(RangeDiffEntry::Unchanged {
                        old_idx: old_idx + 1,
                        old_commit: old_entry.commit.clone(),
                        new_idx: new_idx + 1,
                        new_commit: new_entry.commit.clone(),
                    });
                } else {
                    entries.push(RangeDiffEntry::Changed {
                        old_idx: old_idx + 1,
                        old_commit: old_entry.commit.clone(),
                        new_idx: new_idx + 1,
                        new_commit: new_entry.commit.clone(),
                        diff_of_diffs: None, // computed later
                    });
                }
                continue;
            }
        }
        // No match found
        entries.push(RangeDiffEntry::Removed {
            old_idx: old_idx + 1,
            old_commit: old_entry.commit.clone(),
        });
    }

    // Remaining unmatched new commits are "added"
    for (new_idx, new_entry) in new_entries.iter().enumerate() {
        if !matched_new.contains(&new_idx) {
            entries.push(RangeDiffEntry::Added {
                new_idx: new_idx + 1,
                new_commit: new_entry.commit.clone(),
            });
        }
    }

    entries
}

// ── Diff-between-diffs ──────────────────────────────────────────────────────

/// For all `Changed` entries, compute the diff-between-diffs.
async fn compute_diff_between_diffs(
    entries: Vec<RangeDiffEntry>,
    args: &RangeDiffArgs,
) -> Result<Vec<RangeDiffEntry>, CliError> {
    let mut result = Vec::new();
    for entry in entries {
        match entry {
            RangeDiffEntry::Changed {
                old_idx,
                old_commit,
                new_idx,
                new_commit,
                ..
            } => {
                let diff_of_diffs = if args.patch {
                    Some(compute_diff_of_diffs(&old_commit, &new_commit).await?)
                } else {
                    None
                };
                result.push(RangeDiffEntry::Changed {
                    old_idx,
                    old_commit,
                    new_idx,
                    new_commit,
                    diff_of_diffs,
                });
            }
            other => result.push(other),
        }
    }
    Ok(result)
}

/// Compute a unified diff between the patches of two commits.
async fn compute_diff_of_diffs(
    old_commit: &Commit,
    new_commit: &Commit,
) -> Result<String, CliError> {
    use git_internal::diff::compute_diff;

    let old_diff = resolve_diff_text(old_commit).await?;
    let new_diff = resolve_diff_text(new_commit).await?;

    let old_lines: Vec<String> = old_diff.lines().map(|s| s.to_string()).collect();
    let new_lines: Vec<String> = new_diff.lines().map(|s| s.to_string()).collect();

    let ops = compute_diff(&old_lines, &new_lines);

    let mut output = String::new();
    for op in &ops {
        match op {
            git_internal::diff::DiffOperation::Equal { old_line, .. } => {
                let idx = old_line
                    .saturating_sub(1)
                    .min(old_lines.len().saturating_sub(1));
                if let Some(line) = old_lines.get(idx) {
                    output.push_str(&format!(" {}\n", line));
                }
            }
            git_internal::diff::DiffOperation::Delete { line, .. } => {
                let idx = line
                    .saturating_sub(1)
                    .min(old_lines.len().saturating_sub(1));
                if let Some(line) = old_lines.get(idx) {
                    output.push_str(&format!("-{}\n", line));
                }
            }
            git_internal::diff::DiffOperation::Insert { line, .. } => {
                let idx = line
                    .saturating_sub(1)
                    .min(new_lines.len().saturating_sub(1));
                if let Some(line) = new_lines.get(idx) {
                    output.push_str(&format!("+{}\n", line));
                }
            }
        }
    }
    Ok(output)
}

// ── Output rendering ────────────────────────────────────────────────────────

/// Render the range-diff output to terminal or JSON.
fn render_range_diff(
    entries: &[RangeDiffEntry],
    args: &RangeDiffArgs,
    output: &OutputConfig,
) -> CliResult<()> {
    if output.is_json() {
        render_json(entries, args, output)
    } else {
        render_terminal(entries, args, output)
    }
}

fn render_terminal(
    entries: &[RangeDiffEntry],
    _args: &RangeDiffArgs,
    output: &OutputConfig,
) -> CliResult<()> {
    let mut pager = Pager::with_config(output)?;

    for entry in entries {
        match entry {
            RangeDiffEntry::Added {
                new_idx,
                new_commit,
            } => {
                let hash_short = short_hash(&new_commit.id);
                let subject = first_line(&new_commit.message);
                pager.write_line(&format!(
                    "-:  {} > {}:  {} {}",
                    "-------".yellow(),
                    new_idx.to_string().bright_green(),
                    hash_short.bright_green(),
                    subject.bright_green()
                ))?;
            }
            RangeDiffEntry::Removed {
                old_idx,
                old_commit,
            } => {
                let hash_short = short_hash(&old_commit.id);
                let subject = first_line(&old_commit.message);
                pager.write_line(&format!(
                    "{}:  {} < -:  {} {}",
                    old_idx.to_string().bright_red(),
                    hash_short.bright_red(),
                    "-------".yellow(),
                    subject.bright_red()
                ))?;
            }
            RangeDiffEntry::Unchanged {
                old_idx,
                old_commit,
                new_idx,
                ..
            } => {
                let hash_short = short_hash(&old_commit.id);
                let subject = first_line(&old_commit.message);
                pager.write_line(&format!(
                    "{}:  {} {} {}:  {} {}",
                    old_idx.to_string().dimmed(),
                    hash_short.dimmed(),
                    "=".green(),
                    new_idx.to_string().dimmed(),
                    hash_short.dimmed(),
                    subject.dimmed()
                ))?;
            }
            RangeDiffEntry::Changed {
                old_idx,
                old_commit,
                new_idx,
                new_commit,
                diff_of_diffs,
            } => {
                let old_hash_short = short_hash(&old_commit.id);
                let new_hash_short = short_hash(&new_commit.id);
                let subject = first_line(&new_commit.message);
                pager.write_line(&format!(
                    "{}:  {} {} {}:  {} {}",
                    old_idx.to_string().yellow(),
                    old_hash_short.yellow(),
                    "!".yellow(),
                    new_idx.to_string().yellow(),
                    new_hash_short.yellow(),
                    subject.yellow()
                ))?;

                // If commit message changed, show subject diff
                if old_commit.message.trim() != new_commit.message.trim() {
                    pager.write_line(&format!("    {}:", "Subject".cyan().bold()))?;
                    pager.write_line(&format!("    -{}", first_line(&old_commit.message).red()))?;
                    pager
                        .write_line(&format!("    +{}", first_line(&new_commit.message).green()))?;
                }

                // Show diff-of-diffs when --patch
                if let Some(diff) = diff_of_diffs {
                    for line in diff.lines() {
                        let colored_line = match line.chars().next() {
                            Some('+') => line.green(),
                            Some('-') => line.red(),
                            Some('@') => line.cyan(),
                            _ => line.normal(),
                        };
                        pager.write_line(&format!("    {}", colored_line))?;
                    }
                }
            }
        }
        pager.write_line("")?;
    }

    pager.finish()?;
    Ok(())
}

fn render_json(
    entries: &[RangeDiffEntry],
    _args: &RangeDiffArgs,
    output: &OutputConfig,
) -> CliResult<()> {
    let json_entries: Vec<RangeDiffEntryJson> = entries
        .iter()
        .map(|e| match e {
            RangeDiffEntry::Added {
                new_idx,
                new_commit,
            } => RangeDiffEntryJson::Added {
                new_index: *new_idx,
                new_hash: new_commit.id.to_string(),
                new_subject: first_line(&new_commit.message),
            },
            RangeDiffEntry::Removed {
                old_idx,
                old_commit,
            } => RangeDiffEntryJson::Removed {
                old_index: *old_idx,
                old_hash: old_commit.id.to_string(),
                old_subject: first_line(&old_commit.message),
            },
            RangeDiffEntry::Unchanged {
                old_idx,
                old_commit,
                new_idx,
                ..
            } => RangeDiffEntryJson::Unchanged {
                old_index: *old_idx,
                old_hash: old_commit.id.to_string(),
                new_index: *new_idx,
                subject: first_line(&old_commit.message),
            },
            RangeDiffEntry::Changed {
                old_idx,
                old_commit,
                new_idx,
                new_commit,
                diff_of_diffs,
            } => RangeDiffEntryJson::Changed {
                old_index: *old_idx,
                old_hash: old_commit.id.to_string(),
                old_subject: first_line(&old_commit.message),
                new_index: *new_idx,
                new_hash: new_commit.id.to_string(),
                new_subject: first_line(&new_commit.message),
                diff_text: diff_of_diffs.clone(),
            },
        })
        .collect();

    let output_struct = RangeDiffOutput {
        entries: json_entries,
    };
    emit_json_data("range-diff", &output_struct, output)
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Short 7-char hex hash.
fn short_hash(hash: &ObjectHash) -> String {
    let full = hash.to_string();
    if full.len() > 7 {
        full[..7].to_string()
    } else {
        full
    }
}

/// First line (subject) of a commit message.
fn first_line(msg: &str) -> String {
    msg.lines().next().unwrap_or("").to_string()
}

// ── JSON output types ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Default)]
struct RangeDiffOutput {
    entries: Vec<RangeDiffEntryJson>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status")]
enum RangeDiffEntryJson {
    #[serde(rename = "added")]
    Added {
        new_index: usize,
        new_hash: String,
        new_subject: String,
    },
    #[serde(rename = "removed")]
    Removed {
        old_index: usize,
        old_hash: String,
        old_subject: String,
    },
    #[serde(rename = "unchanged")]
    Unchanged {
        old_index: usize,
        old_hash: String,
        new_index: usize,
        subject: String,
    },
    #[serde(rename = "changed")]
    Changed {
        old_index: usize,
        old_hash: String,
        old_subject: String,
        new_index: usize,
        new_hash: String,
        new_subject: String,
        diff_text: Option<String>,
    },
}
