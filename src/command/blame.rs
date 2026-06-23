//! Per-line authorship attribution (`libra blame`).
//!
//! Implements the `blame` subcommand. Loads the file at the requested
//! revision, walks the commit graph backwards from that revision, and uses
//! `compute_diff` against each parent to migrate line ownership to the
//! oldest ancestor whose content still matches.
//!
//! Non-obvious responsibilities:
//! - Maps domain failures into stable [`CliError`] codes via the
//!   `From<BlameError>` impl so JSON consumers and shell scripts can match
//!   on machine-readable categories.
//! - Supports JSON, quiet, and paged-text output: human output is fed
//!   through [`Pager`] so very long blames behave well in a terminal.
//! - Tracks two parallel structures: the in-flight `LineBlame` vector
//!   (mutated as the BFS progresses) and the queued
//!   `(commit, parent_lines)` work items.

use chrono::DateTime;
use clap::Parser;
use git_internal::{
    diff::compute_diff,
    hash::ObjectHash,
    internal::object::{blob::Blob, commit::Commit, tree::Tree},
};
use serde::Serialize;

use crate::{
    command::{get_target_commit, load_object},
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        object_ext::TreeExt,
        output::{OutputConfig, emit_json_data},
        pager::Pager,
        util,
    },
};

const BLAME_EXAMPLES: &str = "\
EXAMPLES:
    libra blame src/main.rs                Blame a file at HEAD
    libra blame src/main.rs abc1234        Blame a file at a specific commit
    libra blame -L 10,20 src/main.rs       Blame lines 10-20
    libra blame -L 10,+5 src/main.rs       Blame 5 lines starting at line 10
    libra blame -l src/main.rs             Show full commit hashes
    libra blame -s src/main.rs             Suppress the author and date columns
    libra --json blame src/main.rs         Structured JSON output for agents";

#[derive(Parser, Debug)]
#[command(after_help = BLAME_EXAMPLES)]
pub struct BlameArgs {
    /// The file to blame
    #[clap(value_name = "FILE")]
    pub file: String,

    /// The commit to use for blame
    #[clap(value_name = "COMMIT", default_value = "HEAD")]
    pub commit: String,

    /// The line range to blame
    #[clap(short = 'L', value_name = "RANGE")]
    pub line_range: Option<String>,

    /// Emit the machine-readable porcelain format (commit metadata once per commit).
    #[clap(short = 'p', long)]
    pub porcelain: bool,

    /// Like --porcelain, but repeat the commit metadata header for every line.
    #[clap(long = "line-porcelain")]
    pub line_porcelain: bool,

    /// Show the author email instead of the author name in the default output.
    #[clap(short = 'e', long = "show-email")]
    pub show_email: bool,

    /// Show the long (full) commit hash instead of the abbreviated one.
    #[clap(short = 'l')]
    pub long: bool,

    /// Suppress the author name and timestamp columns from the default output.
    #[clap(short = 's')]
    pub suppress: bool,

    /// Show the raw author timestamp (epoch seconds) instead of a formatted date.
    #[clap(short = 't')]
    pub raw_timestamp: bool,

    /// Use N hex digits for the abbreviated commit hash (ignored when `-l` is set).
    #[clap(long, value_name = "N")]
    pub abbrev: Option<usize>,
}

/// Single attributed line of a blame report. Serialised verbatim to JSON.
#[derive(Debug, Clone, Serialize)]
pub struct BlameLine {
    pub line_number: usize,
    pub short_hash: String,
    pub hash: String,
    pub author: String,
    pub author_email: String,
    pub date: String,
    /// Raw author timestamp (epoch seconds); surfaced for `-t` and JSON callers.
    pub timestamp: i64,
    pub content: String,
}

/// Whole-file result of a `libra blame` invocation.
#[derive(Debug, Clone, Serialize)]
pub struct BlameOutput {
    pub file: String,
    pub revision: String,
    pub lines: Vec<BlameLine>,
}

/// Internal mutable state for one source line during the back-walk.
/// `commit_id` is updated whenever an older ancestor still contains the same
/// text — the final value is the line's introducing commit.
struct LineBlame {
    line_number: usize,
    commit_id: ObjectHash,
    author: String,
    author_email: String,
    timestamp: i64,
    content: String,
}

/// Domain error for `libra blame`. Mapped to stable [`CliError`] codes by
/// the `From` impl below.
#[derive(Debug, thiserror::Error)]
enum BlameError {
    /// CWD is not inside a `.libra` repository.
    #[error("not a libra repository")]
    NotInRepo,

    /// User-supplied revision could not be resolved by `get_target_commit`.
    #[error("invalid revision: '{0}'")]
    InvalidRevision(String),

    /// A repository object (commit/tree/blob) failed to load — typically
    /// indicates corruption or partial fetch.
    #[error("failed to load {kind} '{object_id}': {detail}")]
    ObjectLoad {
        kind: &'static str,
        object_id: String,
        detail: String,
    },

    /// The requested path is not present in the tree of the target revision.
    #[error("file '{path}' not found in revision '{revision}'")]
    FileNotFound { path: String, revision: String },

    /// `-L` argument did not match `LINE`, `START,END`, or `START,+COUNT`,
    /// or the numbers were out of range. Mapped to a usage error.
    #[error("invalid line range: {0}")]
    InvalidLineRange(String),
}

impl From<BlameError> for CliError {
    fn from(error: BlameError) -> Self {
        let message = error.to_string();
        match error {
            BlameError::NotInRepo => CliError::repo_not_found(),
            BlameError::InvalidRevision(_) => CliError::fatal(message)
                .with_stable_code(StableErrorCode::CliInvalidTarget)
                .with_hint("check the revision name and try again"),
            BlameError::ObjectLoad { .. } => CliError::fatal(message)
                .with_stable_code(StableErrorCode::RepoCorrupt)
                .with_hint("the object store may be corrupted"),
            BlameError::FileNotFound { .. } => CliError::fatal(message)
                .with_stable_code(StableErrorCode::CliInvalidTarget)
                .with_hint("check the file path; use 'libra show <rev>:' to list available files"),
            BlameError::InvalidLineRange(_) => CliError::command_usage(message)
                .with_stable_code(StableErrorCode::CliInvalidArguments)
                .with_hint(r#"supported formats: "10", "10,20", "10,+5""#),
        }
    }
}

/// Fire-and-forget CLI dispatcher for `libra blame`.
///
/// Functional scope:
/// - Calls [`execute_safe`] with a default [`OutputConfig`] and prints any
///   error to stderr without propagating it.
pub async fn execute(args: BlameArgs) {
    if let Err(e) = execute_safe(args, &OutputConfig::default()).await {
        e.print_stderr();
    }
}

/// Structured entry point used by `cli::parse` and integration tests.
///
/// Functional scope:
/// - Runs [`run_blame`] to produce a [`BlameOutput`], then renders to JSON,
///   stays silent in `--quiet` mode, prints "File is empty" for an empty
///   blob, or formats human-friendly lines and pipes them through [`Pager`].
///
/// Boundary conditions:
/// - Errors from [`run_blame`] are mapped to [`CliError`] via the
///   `From<BlameError>` impl, preserving stable codes and hints.
///
/// See: tests::blame_error_mapping_reports_repo_corrupt_for_storage_failures
/// in src/command/blame.rs:367;
/// tests::test_blame_json_output_includes_lines in
/// tests/command/blame_test.rs:50.
pub async fn execute_safe(args: BlameArgs, out_config: &OutputConfig) -> CliResult<()> {
    let result = run_blame(&args).await.map_err(CliError::from)?;

    if out_config.is_json() {
        return emit_json_data("blame", &result, out_config);
    }

    if out_config.quiet {
        return Ok(());
    }

    if args.porcelain || args.line_porcelain {
        return render_blame_porcelain(&result, &args.file, args.line_porcelain);
    }

    if result.lines.is_empty() {
        println!("File is empty");
        return Ok(());
    }

    let mut output = String::new();
    for blame in &result.lines {
        // Hash column: `-l` shows the full hash; `--abbrev=<n>` shows n digits;
        // otherwise the default short hash.
        let hash_col = if args.long {
            blame.hash.clone()
        } else if let Some(n) = args.abbrev {
            blame.hash.chars().take(n).collect::<String>()
        } else {
            blame.short_hash.clone()
        };

        // `-s` suppresses the author/timestamp columns entirely.
        if args.suppress {
            output.push_str(&format!(
                "{} {}) {}\n",
                hash_col, blame.line_number, blame.content
            ));
            continue;
        }

        // `-e`/`--show-email` shows `<email>` (Git's form) in the author slot.
        let display_author = if args.show_email {
            format!("<{}>", blame.author_email)
        } else {
            blame.author.clone()
        };
        let author_short = if display_author.chars().count() > 15 {
            let truncated: String = display_author.chars().take(12).collect();
            format!("{truncated}...")
        } else {
            format!("{display_author:15}")
        };
        // `-t` shows the raw epoch timestamp; otherwise the localized date.
        let date_col = if args.raw_timestamp {
            blame.timestamp.to_string()
        } else {
            blame
                .date
                .parse::<DateTime<chrono::FixedOffset>>()
                .map(|dt| {
                    dt.with_timezone(&chrono::Local)
                        .format("%Y-%m-%d %H:%M:%S %z")
                        .to_string()
                })
                .unwrap_or_else(|_| blame.date.clone())
        };

        output.push_str(&format!(
            "{} ({:19} {} {}) {}\n",
            hash_col, author_short, date_col, blame.line_number, blame.content
        ));
    }

    let mut pager = Pager::with_config(out_config)?;
    pager.write_str(&output)?;
    pager.finish()?;
    Ok(())
}

/// Compute the per-line attribution.
///
/// Functional scope:
/// - Resolves the start commit and reads the file's lines at that revision.
/// - Initialises one [`LineBlame`] per line, blaming everything to the start
///   commit, then BFS-walks parents. For each `Equal` chunk in the diff to a
///   parent, lines whose content still matches inherit the parent's commit
///   id, author, and timestamp.
/// - Applies the optional `-L` filter as a final pass.
///
/// Boundary conditions:
/// - Empty target file -> returns an empty [`BlameOutput`] without walking
///   history.
/// - Failed parent loads (e.g. shallow clone boundary) are silently skipped
///   so blame still produces a partial answer.
/// - Bad `-L` ranges produce [`BlameError::InvalidLineRange`].
async fn run_blame(args: &BlameArgs) -> Result<BlameOutput, BlameError> {
    util::require_repo().map_err(|_| BlameError::NotInRepo)?;

    let commit_id = get_target_commit(&args.commit)
        .await
        .map_err(|_| BlameError::InvalidRevision(args.commit.clone()))?;

    let commit_obj = load_object::<Commit>(&commit_id).map_err(|e| BlameError::ObjectLoad {
        kind: "commit",
        object_id: commit_id.to_string(),
        detail: e.to_string(),
    })?;

    let target_lines = get_file_lines(&commit_obj, &args.file, &args.commit)?;

    if target_lines.is_empty() {
        return Ok(BlameOutput {
            file: args.file.clone(),
            revision: commit_id.to_string(),
            lines: Vec::new(),
        });
    }

    let mut blame_lines: Vec<LineBlame> = target_lines
        .iter()
        .enumerate()
        .map(|(idx, content)| LineBlame {
            line_number: idx + 1,
            commit_id,
            author: commit_obj.author.name.clone(),
            author_email: commit_obj.author.email.clone(),
            timestamp: commit_obj.author.timestamp as i64,
            content: content.clone(),
        })
        .collect();

    use std::collections::VecDeque;
    let mut queue: VecDeque<(ObjectHash, Commit, Vec<String>)> = VecDeque::new();
    queue.push_back((commit_id, commit_obj, target_lines));

    while let Some((current_id, current_commit, current_lines)) = queue.pop_front() {
        if !blame_lines.iter().any(|b| b.commit_id == current_id) {
            continue;
        }

        for parent_id in &current_commit.parent_commit_ids {
            let parent_commit = match load_object::<Commit>(parent_id) {
                Ok(obj) => obj,
                Err(_) => continue,
            };

            let parent_revision = parent_id.to_string();
            let parent_lines = match get_file_lines(&parent_commit, &args.file, &parent_revision) {
                Ok(lines) if !lines.is_empty() => lines,
                _ => continue,
            };

            let operations = compute_diff(&parent_lines, &current_lines);
            for op in operations {
                use git_internal::diff::DiffOperation;
                match op {
                    DiffOperation::Insert { .. } | DiffOperation::Delete { .. } => {}
                    DiffOperation::Equal { old_line, new_line } => {
                        let final_idx = new_line - 1;
                        if let Some(blame) = blame_lines.get_mut(final_idx)
                            && blame.commit_id == current_id
                        {
                            let parent_content = parent_lines.get(old_line - 1);
                            if Some(&blame.content) == parent_content {
                                blame.commit_id = *parent_id;
                                blame.author = parent_commit.author.name.clone();
                                blame.author_email = parent_commit.author.email.clone();
                                blame.timestamp = parent_commit.author.timestamp as i64;
                            }
                        }
                    }
                }
            }
            queue.push_back((*parent_id, parent_commit, parent_lines));
        }
    }

    let filtered_lines = if let Some(ref range) = args.line_range {
        let (start, end) =
            parse_line_range(range, blame_lines.len()).map_err(BlameError::InvalidLineRange)?;
        blame_lines
            .into_iter()
            .filter(|b| b.line_number >= start && b.line_number <= end)
            .collect::<Vec<_>>()
    } else {
        blame_lines
    };

    Ok(BlameOutput {
        file: args.file.clone(),
        revision: commit_id.to_string(),
        lines: filtered_lines
            .into_iter()
            .map(|line| {
                let hash = line.commit_id.to_string();
                BlameLine {
                    line_number: line.line_number,
                    short_hash: hash.chars().take(8).collect(),
                    hash,
                    author: line.author,
                    author_email: line.author_email,
                    date: format_blame_timestamp(line.timestamp),
                    timestamp: line.timestamp,
                    content: line.content,
                }
            })
            .collect(),
    })
}
/// Read `file_path` at `commit` and return its lines (without trailing
/// newlines).
///
/// Boundary conditions:
/// - Returns [`BlameError::FileNotFound`] if the path is absent in the tree.
/// - Non-UTF-8 blobs are decoded with `from_utf8_lossy`, replacing invalid
///   sequences with U+FFFD.
fn get_file_lines(
    commit: &Commit,
    file_path: &str,
    revision: &str,
) -> Result<Vec<String>, BlameError> {
    let tree = load_object::<Tree>(&commit.tree_id).map_err(|e| BlameError::ObjectLoad {
        kind: "tree",
        object_id: commit.tree_id.to_string(),
        detail: e.to_string(),
    })?;

    let plain_items = tree.get_plain_items();
    let target_path = util::to_workdir_path(file_path);

    let blob_hash = plain_items
        .iter()
        .find(|(path, _)| path == &target_path)
        .map(|(_, hash)| hash)
        .ok_or_else(|| BlameError::FileNotFound {
            path: file_path.to_string(),
            revision: revision.to_string(),
        })?;

    let blob = load_object::<Blob>(blob_hash).map_err(|e| BlameError::ObjectLoad {
        kind: "blob",
        object_id: blob_hash.to_string(),
        detail: e.to_string(),
    })?;

    let content = String::from_utf8_lossy(&blob.data);
    Ok(content.lines().map(|s| s.to_string()).collect())
}

/// Format an epoch second as RFC 3339 (UTC). Falls back to the raw integer
/// when the timestamp is outside chrono's representable range.
/// Render blame output in Git's porcelain format. With `line_porcelain`, the
/// commit metadata header is repeated for every line; otherwise it is printed
/// once per commit (subsequent lines from that commit print only the SHA header
/// and the content). The commit metadata is read by reloading each attributing
/// commit. NOTE: the original line number is approximated by the final line
/// number — the blame walk does not track per-commit origin line numbers.
fn render_blame_porcelain(result: &BlameOutput, file: &str, line_porcelain: bool) -> CliResult<()> {
    use std::{collections::HashSet, io::Write, str::FromStr};

    let lines = &result.lines;
    // For each line, record the group size when it starts a new consecutive run
    // of lines from the same commit (Git prints this count on the group's first
    // line only).
    let mut group_start_size: Vec<Option<usize>> = vec![None; lines.len()];
    let mut i = 0;
    while i < lines.len() {
        let mut j = i + 1;
        while j < lines.len() && lines[j].hash == lines[i].hash {
            j += 1;
        }
        group_start_size[i] = Some(j - i);
        i = j;
    }

    let mut emitted: HashSet<String> = HashSet::new();
    let mut buf = String::new();
    for (idx, line) in lines.iter().enumerate() {
        // Header: "<sha> <orig-line> <final-line> [<group-size>]".
        let (orig, final_no) = (line.line_number, line.line_number);
        match group_start_size[idx] {
            Some(n) => buf.push_str(&format!("{} {orig} {final_no} {n}\n", line.hash)),
            None => buf.push_str(&format!("{} {orig} {final_no}\n", line.hash)),
        }

        if line_porcelain || emitted.insert(line.hash.clone()) {
            let hash = ObjectHash::from_str(&line.hash).map_err(|_| {
                CliError::fatal(format!("invalid blame commit hash '{}'", line.hash))
                    .with_stable_code(StableErrorCode::RepoCorrupt)
            })?;
            let commit = load_object::<Commit>(&hash).map_err(|e| {
                CliError::fatal(format!("failed to load commit {}: {e}", line.hash))
                    .with_stable_code(StableErrorCode::RepoCorrupt)
            })?;
            // Strip any gpgsig/header block so the summary is the real subject.
            let (parsed_message, _) = crate::common_utils::parse_commit_msg(&commit.message);
            let summary = parsed_message
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            buf.push_str(&format!("author {}\n", commit.author.name));
            buf.push_str(&format!("author-mail <{}>\n", commit.author.email));
            buf.push_str(&format!("author-time {}\n", commit.author.timestamp));
            buf.push_str(&format!("author-tz {}\n", commit.author.timezone));
            buf.push_str(&format!("committer {}\n", commit.committer.name));
            buf.push_str(&format!("committer-mail <{}>\n", commit.committer.email));
            buf.push_str(&format!("committer-time {}\n", commit.committer.timestamp));
            buf.push_str(&format!("committer-tz {}\n", commit.committer.timezone));
            buf.push_str(&format!("summary {summary}\n"));
            buf.push_str(&format!("filename {file}\n"));
        }
        buf.push_str(&format!("\t{}\n", line.content));
    }

    let stdout = std::io::stdout();
    match stdout.lock().write_all(buf.as_bytes()) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(e) => Err(
            CliError::io(format!("failed to write blame porcelain output: {e}"))
                .with_stable_code(StableErrorCode::IoWriteFailed),
        ),
    }
}

fn format_blame_timestamp(timestamp: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(timestamp, 0)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| timestamp.to_string())
}

/// Parse a `-L` argument into an inclusive `(start, end)` line range.
///
/// Functional scope:
/// - Accepts `LINE`, `START,END`, and `START,+COUNT` (offset) syntaxes.
///
/// Boundary conditions:
/// - Returns `Err` for non-numeric tokens, zero indices, indices past the
///   file end, or `start > end`. Each error message is suitable for direct
///   inclusion in a [`BlameError::InvalidLineRange`].
fn parse_line_range(range_str: &str, total_lines: usize) -> Result<(usize, usize), String> {
    let parts: Vec<&str> = range_str.split(',').collect();

    match parts.len() {
        1 => {
            // Single line: "10"
            let line = parts[0]
                .parse::<usize>()
                .map_err(|_| format!("Invalid line number: {}", parts[0]))?;
            if line == 0 || line > total_lines {
                return Err(format!("Line {} is out of range (1-{})", line, total_lines));
            }
            Ok((line, line))
        }
        2 => {
            let start = parts[0]
                .parse::<usize>()
                .map_err(|_| format!("Invalid start line: {}", parts[0]))?;

            // Check if second part is relative (+N) or absolute
            let end = if parts[1].starts_with('+') {
                let offset = parts[1][1..]
                    .parse::<usize>()
                    .map_err(|_| format!("Invalid offset: {}", parts[1]))?;
                start + offset - 1
            } else {
                parts[1]
                    .parse::<usize>()
                    .map_err(|_| format!("Invalid end line: {}", parts[1]))?
            };

            if start == 0 || start > total_lines || end == 0 || end > total_lines || start > end {
                return Err(format!(
                    "Invalid range {},{} (total lines: {})",
                    start, end, total_lines
                ));
            }
            Ok((start, end))
        }
        _ => Err("Invalid range format. Use: LINE or START,END or START,+COUNT".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pin the `Display` format for the static-message and direct-
    /// message variants of [`BlameError`]. These strings are used as
    /// the `CliError` message via `From<BlameError> for CliError` and
    /// surface in both human and `--json` envelopes.
    #[test]
    fn blame_error_display_pins_each_variant() {
        assert_eq!(BlameError::NotInRepo.to_string(), "not a libra repository");
        assert_eq!(
            BlameError::InvalidRevision("HEAD~99".to_string()).to_string(),
            "invalid revision: 'HEAD~99'",
        );
        assert_eq!(
            BlameError::ObjectLoad {
                kind: "tree",
                object_id: "deadbeef".to_string(),
                detail: "object not found".to_string(),
            }
            .to_string(),
            "failed to load tree 'deadbeef': object not found",
        );
        assert_eq!(
            BlameError::FileNotFound {
                path: "src/missing.rs".to_string(),
                revision: "HEAD".to_string(),
            }
            .to_string(),
            "file 'src/missing.rs' not found in revision 'HEAD'",
        );
        assert_eq!(
            BlameError::InvalidLineRange("10,5".to_string()).to_string(),
            "invalid line range: 10,5",
        );
    }

    /// Scenario: object-store failures must surface as `RepoCorrupt` so that
    /// shell scripts and JSON consumers can distinguish "the object store is
    /// broken" from "the user typed the wrong revision".
    #[test]
    fn blame_error_mapping_reports_repo_corrupt_for_storage_failures() {
        let error = CliError::from(BlameError::ObjectLoad {
            kind: "tree",
            object_id: "abc123".to_string(),
            detail: "corrupt object".to_string(),
        });
        assert_eq!(error.stable_code(), StableErrorCode::RepoCorrupt);
    }

    /// Scenario: "file not in revision" is a user-target mistake, not
    /// corruption. Verifying the stable code keeps the error category
    /// distinct from object-load failures handled by the previous test.
    #[test]
    fn blame_error_mapping_reports_invalid_target_for_missing_file() {
        let error = CliError::from(BlameError::FileNotFound {
            path: "tracked.txt".to_string(),
            revision: "HEAD".to_string(),
        });
        assert_eq!(error.stable_code(), StableErrorCode::CliInvalidTarget);
    }
}
