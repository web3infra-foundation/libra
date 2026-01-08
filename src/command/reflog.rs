//! Reads and displays reflog entries for HEAD or branches with filtering and timestamp formatting options.

#[cfg(unix)]
use std::io::Write;
#[cfg(unix)]
use std::process::{Command, Stdio};
use std::{
    collections::HashMap,
    fmt::{Display, Formatter},
    str::FromStr,
};

use clap::{Parser, Subcommand};
use colored::Colorize;
use git_internal::{hash::ObjectHash, internal::object::commit::Commit};
use sea_orm::{ConnectionTrait, DbBackend, Statement, TransactionTrait, sqlx::types::chrono};

use crate::{
    command::load_object,
    internal::{
        config,
        db::get_db_conn_instance,
        model::reflog::Model,
        reflog::{HEAD, Reflog, ReflogError},
    },
};

#[derive(Parser, Debug)]
pub struct ReflogArgs {
    #[clap(subcommand)]
    command: Subcommands,
}

#[derive(Subcommand, Debug, Clone)]
enum Subcommands {
    /// show reflog records.
    Show {
        #[clap(default_value = "HEAD")]
        ref_name: String,
        #[arg(long = "pretty")]
        #[clap(default_value_t = FormatterKind::default())]
        pretty: FormatterKind,
        /// Show reflog entries newer than date
        #[arg(long)]
        since: Option<String>,
        /// Show reflog entries older than date
        #[arg(long)]
        until: Option<String>,
        /// Filter reflog entries by message pattern
        #[arg(long)]
        grep: Option<String>,
        /// Filter reflog entries by committer name or email
        #[arg(long)]
        author: Option<String>,
        /// Limit the number of output
        #[clap(short, long)]
        number: Option<usize>,
        /// Show diffs for each reflog entry
        #[clap(short = 'p', long = "patch")]
        patch: bool,
        /// Show diffstat for each reflog entry
        #[arg(long)]
        stat: bool,
    },
    /// clear the reflog record of the specified branch.
    Delete {
        #[clap(required = true, num_args = 1..)]
        selectors: Vec<String>,
    },
    /// check whether a reference has a reflog record, usually using by automatic scripts.
    Exists {
        #[clap(required = true)]
        ref_name: String,
    },
}

pub async fn execute(args: ReflogArgs) {
    match args.command {
        Subcommands::Show { ref_name, pretty, since, until, grep, author, number, patch, stat } => {
            handle_show(&ref_name, pretty, since, until, grep, author, number, patch, stat).await
        },
        Subcommands::Delete { selectors } => handle_delete(&selectors).await,
        Subcommands::Exists { ref_name } => handle_exists(&ref_name).await,
    }
}

async fn handle_show(
    ref_name: &str,
    pretty: FormatterKind,
    since: Option<String>,
    until: Option<String>,
    grep: Option<String>,
    author: Option<String>,
    number: Option<usize>,
    patch: bool,
    stat: bool,
) {
    let db = get_db_conn_instance().await;

    // Parse date filters
    let since_ts = match since.as_deref().map(crate::internal::log::date_parser::parse_date).transpose() {
        Ok(ts) => ts,
        Err(e) => {
            eprintln!("fatal: invalid --since date: {e}");
            return;
        }
    };

    let until_ts = match until.as_deref().map(crate::internal::log::date_parser::parse_date).transpose() {
        Ok(ts) => ts,
        Err(e) => {
            eprintln!("fatal: invalid --until date: {e}");
            return;
        }
    };

    let ref_name = parse_ref_name(ref_name).await;
    let logs = match Reflog::find_all(db, &ref_name).await {
        Ok(logs) => logs,
        Err(e) => {
            eprintln!("fatal: failed to get reflog entries: {e}");
            return;
        }
    };

    // Apply filters
    let filter = ReflogFilter::new(since_ts, until_ts, grep, author);
    let filtered_logs: Vec<_> = logs.into_iter()
        .filter(|log| filter.passes(log))
        .collect();

    // Apply number limit
    let max_output = number.unwrap_or(filtered_logs.len());
    let limited_logs = &filtered_logs[..filtered_logs.len().min(max_output)];

    let formatter = ReflogFormatter {
        logs: limited_logs,
        kind: pretty,
        patch,
        stat,
    };

    #[cfg(unix)]
    let mut less = Command::new("less") // create a pipe to less
        .arg("-R") // raw control characters
        .arg("-F")
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .spawn()
        .expect("failed to execute process");

    #[cfg(unix)]
    if let Some(ref mut stdin) = less.stdin {
        writeln!(stdin, "{}", formatter).expect("fatal: failed to write to stdin");
    } else {
        eprintln!("Failed to capture stdin");
    }

    #[cfg(unix)]
    let _ = less.wait().expect("failed to wait on child");

    #[cfg(not(unix))]
    println!("{formatter}")
}

// `partial_ref_name` is the branch name entered by the user.
async fn parse_ref_name(partial_ref_name: &str) -> String {
    if partial_ref_name == HEAD {
        return HEAD.to_string();
    }
    if !partial_ref_name.contains("/") {
        return format!("refs/heads/{partial_ref_name}");
    }
    let (ref_name, _) = partial_ref_name.split_once("/").unwrap();
    if config::Config::get("remote", Some(ref_name), "url")
        .await
        .is_some()
    {
        return format!("refs/remotes/{partial_ref_name}");
    }
    format!("refs/heads/{partial_ref_name}")
}

async fn handle_exists(ref_name: &str) {
    let db = get_db_conn_instance().await;
    let log = Reflog::find_one(db, ref_name)
        .await
        .expect("fatal: failed to get reflog entry");
    match log {
        Some(_) => {}
        None => {
            eprintln!("fatal: reflog entry for '{}' not found", ref_name);
        }
    }
}

async fn handle_delete(selectors: &[String]) {
    let mut groups = HashMap::new();
    for selector in selectors {
        if let Some(parsed) = parse_reflog_selector(selector) {
            groups
                .entry(parsed.0.to_string())
                .or_insert_with(Vec::new)
                .push(parsed);
            continue;
        }
        eprintln!("fatal: invalid reflog entry format: {selector}");
        return;
    }

    let groups = groups
        .into_values()
        .map(|mut group| {
            group.sort_by(|a, b| b.1.cmp(&a.1));
            group
        })
        .collect::<Vec<_>>();
    for group in groups {
        delete_single_group(&group).await;
    }
}

async fn delete_single_group(group: &[(&str, usize)]) {
    let db = get_db_conn_instance().await;
    // clone this to move it into async block to make compiler happy :(
    let group = group
        .iter()
        .map(|(s, i)| ((*s).to_string(), *i))
        .collect::<Vec<(String, usize)>>();

    db.transaction(|txn| {
        Box::pin(async move {
            let ref_name = &group[0].0;
            let logs = Reflog::find_all(txn, ref_name).await?;

            for (_, index) in &group {
                if let Some(entry) = logs.get(*index) {
                    let id = entry.id;
                    txn.execute(Statement::from_sql_and_values(
                        DbBackend::Sqlite,
                        "DELETE FROM reflog WHERE id = ?;",
                        [id.into()],
                    ))
                    .await?;
                    continue;
                }
                eprintln!("fatal: reflog entry `{ref_name}@{{{index}}}` not found")
            }

            Ok::<_, ReflogError>(())
        })
    })
    .await
    .expect("fatal: failed to delete reflog entries")
}

fn parse_reflog_selector(selector: &str) -> Option<(&str, usize)> {
    if let (Some(at_brace), Some(end_brace)) = (selector.find("@{"), selector.find('}'))
        && at_brace < end_brace
    {
        let ref_name = &selector[..at_brace];
        let index_str = &selector[at_brace + 2..end_brace];

        if let Ok(index) = index_str.parse::<usize>() {
            return Some((ref_name, index));
        }
    }
    None
}

/// Filter for reflog entries based on time and message patterns
struct ReflogFilter {
    since: Option<i64>,
    until: Option<i64>,
    grep: Option<String>,
    author: Option<String>,
}

impl ReflogFilter {
    /// Create a new filter from optional parameters
    fn new(
        since: Option<i64>,
        until: Option<i64>,
        grep: Option<String>,
        author: Option<String>,
    ) -> Self {
        Self {
            since,
            until,
            grep: grep.map(|s| s.to_lowercase()),
            author: author.map(|s| s.to_lowercase()),
        }
    }

    /// Check if a reflog entry passes all filters
    fn passes(&self, entry: &Model) -> bool {
        // Time filters
        let ts = entry.timestamp;

        if let Some(since) = self.since && ts < since {
            return false;
        }

        if let Some(until) = self.until && ts > until {
            return false;
        }

        // Message filter (matches both action and message fields)
        if let Some(grep_pattern) = &self.grep {
            let full_message = format!("{}: {}", entry.action, entry.message);
            if !full_message.to_lowercase().contains(grep_pattern) {
                return false;
            }
        }

        // Author filter (matches committer_name or committer_email)
        if let Some(author_filter) = &self.author {
            let committer = format!(
                "{} <{}>",
                entry.committer_name.to_lowercase(),
                entry.committer_email.to_lowercase()
            );
            if !committer.contains(author_filter) {
                return false;
            }
        }

        true
    }
}

#[derive(Debug, Copy, Clone, Default)]
enum FormatterKind {
    #[default]
    Oneline,
    Short,
    Medium,
    Full,
}

impl Display for FormatterKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Oneline => f.write_str("oneline"),
            Self::Short => f.write_str("short"),
            Self::Medium => f.write_str("medium"),
            Self::Full => f.write_str("full"),
        }
    }
}

impl From<String> for FormatterKind {
    fn from(value: String) -> Self {
        match value.as_str() {
            "oneline" => FormatterKind::Oneline,
            "short" => FormatterKind::Short,
            "medium" => FormatterKind::Medium,
            "full" => FormatterKind::Full,
            _ => FormatterKind::Oneline,
        }
    }
}

struct ReflogFormatter<'a> {
    logs: &'a [Model],
    kind: FormatterKind,
    patch: bool,
    stat: bool,
}

impl Display for ReflogFormatter<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let all = self.logs
            .iter()
            .enumerate()
            .map(|(idx, log)| {
                let head = format!("HEAD@{{{idx}}}");
                let new_oid = &log.new_oid[..7];

                let commit = find_commit(&log.new_oid);
                let full_msg = format!("{}: {}", log.action, log.message);

                let author = format!("{} <{}>", commit.author.name, commit.author.email);
                let committer = format!("{} <{}>", log.committer_name, log.committer_email);
                let commit_msg = &commit.message.trim();
                let datetime = format_datetime(log.timestamp);

                let mut output = match self.kind {
                    FormatterKind::Oneline => format!(
                        "{} {head}: {full_msg}",
                        new_oid.to_string().bright_magenta(),
                    ),
                    FormatterKind::Short => format!(
                        "{}\nReflog: {head} ({author})\nReflog message: {full_msg}\nAuthor: {author}\n\n  {commit_msg}\n",
                        format!("commit {new_oid}").bright_magenta(),
                    ),
                    FormatterKind::Medium => format!(
                        "{}\nReflog: {head} ({author})\nReflog message: {full_msg}\nAuthor: {author}\nDate:   {datetime}\n\n  {commit_msg}\n",
                        format!("commit {new_oid}").bright_magenta(),
                    ),
                    FormatterKind::Full => format!(
                        "{}\nReflog: {head} ({author})\nReflog message: {full_msg}\nAuthor: {author}\nCommit: {committer}\n\n  {commit_msg}\n",
                        format!("commit {new_oid}").bright_magenta(),
                    ),
                };

                // Append diff or stat output if requested
                if self.patch {
                    if let Ok(patch_output) = generate_diff_sync(&commit) {
                        if !patch_output.is_empty() {
                            if !output.ends_with('\n') {
                                output.push('\n');
                            }
                            output.push_str(&patch_output);
                        }
                    }
                } else if self.stat {
                    if let Ok(stat_output) = generate_stat_sync(&commit) {
                        if !stat_output.is_empty() {
                            if !output.ends_with('\n') {
                                output.push('\n');
                            }
                            output.push_str(&stat_output);
                        }
                    }
                }

                output
            })
            .collect::<Vec<_>>()
            .join("\n");
        writeln!(f, "{all}")
    }
}

fn find_commit(commit_hash: &str) -> Commit {
    let hash = ObjectHash::from_str(commit_hash).unwrap();
    load_object::<Commit>(&hash).unwrap()
}

fn format_datetime(timestamp: i64) -> String {
    let naive = chrono::DateTime::from_timestamp(timestamp, 0).unwrap();
    let local = naive.with_timezone(&chrono::Local);

    let git_format = "%a %b %d %H:%M:%S %Y %z";
    local.format(git_format).to_string()
}

/// Synchronous wrapper for generating diff output
fn generate_diff_sync(commit: &Commit) -> Result<String, Box<dyn std::error::Error>> {
    use git_internal::internal::object::{tree::Tree, blob::Blob};
    use git_internal::Diff;
    use crate::utils::object_ext::TreeExt;

    // new_blobs from commit tree
    let tree = load_object::<Tree>(&commit.tree_id)?;
    let new_blobs: Vec<(std::path::PathBuf, ObjectHash)> = tree.get_plain_items();

    // old_blobs from first parent if exists
    let old_blobs: Vec<(std::path::PathBuf, ObjectHash)> = if !commit.parent_commit_ids.is_empty() {
        let parent = &commit.parent_commit_ids[0];
        let parent_hash = ObjectHash::from_str(&parent.to_string())?;
        let parent_commit = load_object::<Commit>(&parent_hash)?;
        let parent_tree = load_object::<Tree>(&parent_commit.tree_id)?;
        parent_tree.get_plain_items()
    } else {
        Vec::new()
    };

    let read_content = |_file: &std::path::PathBuf, hash: &ObjectHash| -> Vec<u8> {
        load_object::<Blob>(hash)
            .map(|blob| blob.data)
            .unwrap_or_default()
    };

    let diffs = Diff::diff(
        old_blobs,
        new_blobs,
        Vec::new(), // No path filters for reflog
        read_content,
    );

    let mut diff_output = String::new();
    for diff in diffs {
        diff_output.push_str(&format!("--- a/{}\n", diff.path));
        diff_output.push_str(&format!("+++ b/{}\n", diff.path));
        diff_output.push_str(&diff.data);
        diff_output.push('\n');
    }

    Ok(diff_output)
}

/// Synchronous wrapper for generating stat output
fn generate_stat_sync(commit: &Commit) -> Result<String, Box<dyn std::error::Error>> {
    use git_internal::internal::object::{tree::Tree, blob::Blob};
    use git_internal::Diff;
    use crate::utils::object_ext::TreeExt;

    // new_blobs from commit tree
    let tree = load_object::<Tree>(&commit.tree_id)?;
    let new_blobs: Vec<(std::path::PathBuf, ObjectHash)> = tree.get_plain_items();

    // old_blobs from first parent if exists
    let old_blobs: Vec<(std::path::PathBuf, ObjectHash)> = if !commit.parent_commit_ids.is_empty() {
        let parent = &commit.parent_commit_ids[0];
        let parent_hash = ObjectHash::from_str(&parent.to_string())?;
        let parent_commit = load_object::<Commit>(&parent_hash)?;
        let parent_tree = load_object::<Tree>(&parent_commit.tree_id)?;
        parent_tree.get_plain_items()
    } else {
        Vec::new()
    };

    let read_content = |_file: &std::path::PathBuf, hash: &ObjectHash| -> Vec<u8> {
        load_object::<Blob>(hash)
            .map(|blob| blob.data)
            .unwrap_or_default()
    };

    let diffs = Diff::diff(
        old_blobs,
        new_blobs,
        Vec::new(), // No path filters for reflog
        read_content,
    );

    if diffs.is_empty() {
        return Ok(String::new());
    }

    let mut additions = 0;
    let mut deletions = 0;

    for diff in &diffs {
        for line in diff.data.lines() {
            if line.starts_with('+') && !line.starts_with("+++") {
                additions += 1;
            } else if line.starts_with('-') && !line.starts_with("---") {
                deletions += 1;
            }
        }
    }

    let stat_output = format!(
        " {} file{} changed, {} insertion{}(+), {} deletion{}(-)\n",
        diffs.len(),
        if diffs.len() != 1 { "s" } else { "" },
        additions,
        if additions != 1 { "s" } else { "" },
        deletions,
        if deletions != 1 { "s" } else { "" }
    );

    Ok(stat_output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn test_show_args_with_filters() {
        let args = ReflogArgs::parse_from([
            "reflog", "show",
            "--since", "2024-01-01",
            "--until", "2024-12-31",
            "--grep", "commit"
        ]);

        if let Subcommands::Show { ref_name, pretty: _, since, until, grep, author: _, number: _, patch: _, stat: _ } = args.command {
            assert_eq!(ref_name, "HEAD");
            assert_eq!(since.as_deref(), Some("2024-01-01"));
            assert_eq!(until.as_deref(), Some("2024-12-31"));
            assert_eq!(grep.as_deref(), Some("commit"));
        } else {
            panic!("Expected Show subcommand");
        }
    }

    #[test]
    fn test_reflog_filter_time() {
        let entry1 = Model {
            id: 1,
            ref_name: "HEAD".to_string(),
            old_oid: "abc".to_string(),
            new_oid: "def".to_string(),
            timestamp: 1_700_000_000,
            committer_name: "Test".to_string(),
            committer_email: "test@test.com".to_string(),
            action: "commit".to_string(),
            message: "Test message".to_string(),
        };

        let entry2 = Model {
            id: 2,
            ref_name: "HEAD".to_string(),
            old_oid: "def".to_string(),
            new_oid: "ghi".to_string(),
            timestamp: 1_750_000_000,
            committer_name: "Test".to_string(),
            committer_email: "test@test.com".to_string(),
            action: "commit".to_string(),
            message: "Another message".to_string(),
        };

        let filter = ReflogFilter::new(Some(1_720_000_000), None, None, None);
        assert!(!filter.passes(&entry1));
        assert!(filter.passes(&entry2));

        let filter = ReflogFilter::new(None, Some(1_730_000_000), None, None);
        assert!(filter.passes(&entry1));
        assert!(!filter.passes(&entry2));
    }

    #[test]
    fn test_reflog_filter_grep() {
        let entry1 = Model {
            id: 1,
            ref_name: "HEAD".to_string(),
            old_oid: "abc".to_string(),
            new_oid: "def".to_string(),
            timestamp: 1_700_000_000,
            committer_name: "Test".to_string(),
            committer_email: "test@test.com".to_string(),
            action: "commit".to_string(),
            message: "Add feature".to_string(),
        };

        let entry2 = Model {
            id: 2,
            ref_name: "HEAD".to_string(),
            old_oid: "def".to_string(),
            new_oid: "ghi".to_string(),
            timestamp: 1_750_000_000,
            committer_name: "Test".to_string(),
            committer_email: "test@test.com".to_string(),
            action: "merge".to_string(),
            message: "Merge branch".to_string(),
        };

        let filter = ReflogFilter::new(None, None, Some("COMMIT".to_string()), None);
        assert!(filter.passes(&entry1));
        assert!(!filter.passes(&entry2));

        let filter = ReflogFilter::new(None, None, Some("merge".to_string()), None);
        assert!(!filter.passes(&entry1));
        assert!(filter.passes(&entry2));
    }

    #[test]
    fn test_reflog_filter_combined() {
        let entry = Model {
            id: 1,
            ref_name: "HEAD".to_string(),
            old_oid: "abc".to_string(),
            new_oid: "def".to_string(),
            timestamp: 1_725_000_000,
            committer_name: "Test".to_string(),
            committer_email: "test@test.com".to_string(),
            action: "commit".to_string(),
            message: "Add feature".to_string(),
        };

        let filter = ReflogFilter::new(
            Some(1_700_000_000),
            Some(1_750_000_000),
            Some("feature".to_string()),
            None
        );
        assert!(filter.passes(&entry));

        let filter = ReflogFilter::new(
            Some(1_730_000_000),
            Some(1_750_000_000),
            Some("feature".to_string()),
            None
        );
        assert!(!filter.passes(&entry));
    }

    #[test]
    fn test_reflog_filter_author() {
        let entry1 = Model {
            id: 1,
            ref_name: "HEAD".to_string(),
            old_oid: "abc".to_string(),
            new_oid: "def".to_string(),
            timestamp: 1_700_000_000,
            committer_name: "Alice".to_string(),
            committer_email: "alice@example.com".to_string(),
            action: "commit".to_string(),
            message: "Test message".to_string(),
        };

        let entry2 = Model {
            id: 2,
            ref_name: "HEAD".to_string(),
            old_oid: "def".to_string(),
            new_oid: "ghi".to_string(),
            timestamp: 1_750_000_000,
            committer_name: "Bob".to_string(),
            committer_email: "bob@example.com".to_string(),
            action: "commit".to_string(),
            message: "Another message".to_string(),
        };

        // Test author filtering by name
        let filter = ReflogFilter::new(None, None, None, Some("alice".to_string()));
        assert!(filter.passes(&entry1));
        assert!(!filter.passes(&entry2));

        // Test author filtering by email
        let filter = ReflogFilter::new(None, None, None, Some("bob@example".to_string()));
        assert!(!filter.passes(&entry1));
        assert!(filter.passes(&entry2));

        // Test case-insensitive matching
        let filter = ReflogFilter::new(None, None, None, Some("ALICE".to_string()));
        assert!(filter.passes(&entry1));
    }
}
