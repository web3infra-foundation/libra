//! Provides diff command logic comparing commits, the index, and the working tree with algorithm selection, pathspec filtering, and optional file output.

use std::{
    cell::RefCell,
    collections::HashMap,
    fmt,
    io::{self, IsTerminal},
    path::PathBuf,
    rc::Rc,
};

use clap::Parser;
use colored::Colorize;
use git_internal::{
    Diff,
    hash::ObjectHash,
    internal::{
        index::Index,
        object::{blob::Blob, commit::Commit, tree::Tree, types::ObjectType},
        pack::utils::calculate_object_hash,
    },
};
use serde::Serialize;
use similar;

use crate::{
    command::{get_target_commit, load_object},
    internal::head::Head,
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        ignore::{self, IgnorePolicy},
        object_ext::TreeExt,
        output::{OutputConfig, emit_json_data},
        pager::Pager,
        path, util,
    },
};

#[derive(Parser, Debug)]
pub struct DiffArgs {
    /// Old commit, default is HEAD
    #[clap(long, value_name = "COMMIT")]
    pub old: Option<String>,

    /// New commit, default is working directory
    #[clap(long, value_name = "COMMIT")]
    #[clap(requires = "old", group = "op_new")]
    pub new: Option<String>,

    /// Use stage as new commit. This option is conflict with --new.
    #[clap(long)]
    #[clap(group = "op_new")]
    pub staged: bool,

    #[clap(help = "Files to compare")]
    pathspec: Vec<String>,

    // TODO: If algorithm support gets added to git-internal
    /// choose the exact diff algorithm default value is histogram
    /// support myers and myersMinimal
    #[clap(long, default_value = "histogram", value_parser=["histogram", "myers", "myersMinimal"])]
    pub algorithm: Option<String>,

    // Print the result to file
    #[clap(long, value_name = "FILENAME")]
    pub output: Option<String>,

    /// Show only changed file names
    #[clap(long)]
    pub name_only: bool,

    /// Show changed file names with status
    #[clap(long)]
    pub name_status: bool,

    /// Show insertion/deletion counts in a machine-friendly format
    #[clap(long)]
    pub numstat: bool,

    /// Show diff statistics
    #[clap(long)]
    pub stat: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiffHunk {
    pub old_start: usize,
    pub old_lines: usize,
    pub new_start: usize,
    pub new_lines: usize,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiffFileStat {
    pub path: String,
    pub status: String,
    pub insertions: usize,
    pub deletions: usize,
    pub hunks: Vec<DiffHunk>,
    #[serde(skip_serializing)]
    raw_diff: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiffOutput {
    pub old_ref: String,
    pub new_ref: String,
    pub files: Vec<DiffFileStat>,
    pub total_insertions: usize,
    pub total_deletions: usize,
    pub files_changed: usize,
}

pub async fn execute(args: DiffArgs) {
    if let Err(err) = execute_safe(args, &OutputConfig::default()).await {
        err.print_stderr();
    }
}

pub async fn execute_safe(args: DiffArgs, output: &OutputConfig) -> CliResult<()> {
    let result = run_diff(&args).await?;
    render_diff_output(&args, &result, output)
}

async fn run_diff(args: &DiffArgs) -> CliResult<DiffOutput> {
    util::require_repo().map_err(|_| CliError::repo_not_found())?;
    tracing::debug!("diff args: {:?}", args);
    let index = Index::load(path::index()).map_err(|e| {
        CliError::fatal(format!("failed to load index: {e}"))
            .with_stable_code(StableErrorCode::RepoCorrupt)
    })?;

    let old_side = resolve_diff_side(&args.old, args.staged, false, &index).await?;
    let new_side = resolve_diff_side(&args.new, args.staged, true, &index).await?;

    let paths: Vec<PathBuf> = args.pathspec.iter().map(util::to_workdir_path).collect();
    let worktree_entries = new_side.worktree_entries.clone();
    let worktree_cache = RefCell::new(HashMap::<ObjectHash, Vec<u8>>::new());
    let repo_cache = RefCell::new(HashMap::<ObjectHash, Vec<u8>>::new());
    let load_error = Rc::new(RefCell::new(None::<CliError>));
    let load_error_for_read = Rc::clone(&load_error);
    let diff_output = Diff::diff(old_side.blobs, new_side.blobs, paths, move |path, hash| {
        if worktree_entries.get(path) == Some(hash) {
            if let Some(data) = worktree_cache.borrow().get(hash).cloned() {
                return data;
            }

            match read_worktree_blob_content(path) {
                Ok(data) => {
                    worktree_cache.borrow_mut().insert(*hash, data.clone());
                    data
                }
                Err(err) => {
                    record_diff_content_error(&load_error_for_read, err);
                    Vec::new()
                }
            }
        } else {
            if let Some(data) = repo_cache.borrow().get(hash).cloned() {
                return data;
            }

            match load_repo_blob_content(hash) {
                Ok(data) => {
                    repo_cache.borrow_mut().insert(*hash, data.clone());
                    data
                }
                Err(err) => {
                    record_diff_content_error(&load_error_for_read, err);
                    Vec::new()
                }
            }
        }
    });
    if let Some(err) = load_error.borrow_mut().take() {
        return Err(err);
    }

    let files: Vec<DiffFileStat> = diff_output.iter().map(parse_diff_item).collect();
    let total_insertions = files.iter().map(|file| file.insertions).sum();
    let total_deletions = files.iter().map(|file| file.deletions).sum();
    let files_changed = files.len();

    Ok(DiffOutput {
        old_ref: old_side.label,
        new_ref: new_side.label,
        files,
        total_insertions,
        total_deletions,
        files_changed,
    })
}

#[derive(Debug)]
struct DiffSide {
    label: String,
    blobs: Vec<(PathBuf, ObjectHash)>,
    worktree_entries: HashMap<PathBuf, ObjectHash>,
}

/// diff needs to print hashes even if the files have not been staged yet.
/// This helper maps workdir paths to blob ids while applying the shared ignore policy.
fn get_files_blobs(
    files: &[PathBuf],
    index: &Index,
    policy: IgnorePolicy,
) -> CliResult<Vec<(PathBuf, ObjectHash)>> {
    files
        .iter()
        .filter(|path| !ignore::should_ignore(path, policy, index))
        .map(|p| {
            let path = util::workdir_to_absolute(p);
            let data = std::fs::read(&path).map_err(|e| {
                CliError::fatal(format!("failed to read file '{}': {e}", path.display()))
                    .with_stable_code(StableErrorCode::IoReadFailed)
            })?;
            Ok((p.to_owned(), calculate_object_hash(ObjectType::Blob, &data)))
        })
        .collect()
}

/// Returns (path, hash) pairs from the index's stored entries (stage 0).
/// Unlike `get_files_blobs`, this uses the hash already recorded in the index
/// rather than reading the current file on disk, which is essential for
/// producing a correct working-directory diff (index vs working tree).
fn get_index_blobs(index: &Index, policy: IgnorePolicy) -> Vec<(PathBuf, ObjectHash)> {
    index
        .tracked_entries(0)
        .iter()
        .filter(|entry| !ignore::should_ignore(&PathBuf::from(&entry.name), policy, index))
        .map(|entry| (PathBuf::from(&entry.name), entry.hash))
        .collect()
}

async fn resolve_diff_side(
    source: &Option<String>,
    staged: bool,
    is_new: bool,
    index: &Index,
) -> CliResult<DiffSide> {
    if let Some(source) = source {
        let commit_hash = get_target_commit(source).await.map_err(|e| {
            CliError::fatal(format!(
                "{e}, can't use as diff {} source",
                if is_new { "new" } else { "old" }
            ))
            .with_stable_code(StableErrorCode::CliInvalidTarget)
        })?;
        return Ok(DiffSide {
            label: source.clone(),
            blobs: get_commit_blobs(&commit_hash).await?,
            worktree_entries: HashMap::new(),
        });
    }

    if is_new {
        if staged {
            Ok(DiffSide {
                label: "index".to_string(),
                blobs: get_index_blobs(index, IgnorePolicy::Respect),
                worktree_entries: HashMap::new(),
            })
        } else {
            let files = util::list_workdir_files().map_err(|e| {
                CliError::fatal(format!("failed to list working directory files: {e}"))
                    .with_stable_code(StableErrorCode::IoReadFailed)
            })?;
            let blobs = get_files_blobs(&files, index, IgnorePolicy::Respect)?;
            Ok(DiffSide {
                label: "working tree".to_string(),
                worktree_entries: blobs.iter().cloned().collect(),
                blobs,
            })
        }
    } else if staged {
        match Head::current_commit().await {
            Some(commit_hash) => Ok(DiffSide {
                label: "HEAD".to_string(),
                blobs: get_commit_blobs(&commit_hash).await?,
                worktree_entries: HashMap::new(),
            }),
            None => Ok(DiffSide {
                label: "HEAD".to_string(),
                blobs: Vec::new(),
                worktree_entries: HashMap::new(),
            }),
        }
    } else {
        Ok(DiffSide {
            label: "index".to_string(),
            blobs: get_index_blobs(index, IgnorePolicy::Respect),
            worktree_entries: HashMap::new(),
        })
    }
}

async fn get_commit_blobs(commit_hash: &ObjectHash) -> CliResult<Vec<(PathBuf, ObjectHash)>> {
    let commit = load_object::<Commit>(commit_hash).map_err(|e| {
        CliError::fatal(format!("failed to load commit {}: {e}", commit_hash))
            .with_stable_code(StableErrorCode::RepoCorrupt)
    })?;
    let tree = load_object::<Tree>(&commit.tree_id).map_err(|e| {
        CliError::fatal(format!("failed to load tree {}: {e}", commit.tree_id))
            .with_stable_code(StableErrorCode::RepoCorrupt)
    })?;
    Ok(tree.get_plain_items())
}

fn load_repo_blob_content(hash: &ObjectHash) -> CliResult<Vec<u8>> {
    let blob = load_object::<Blob>(hash).map_err(|e| {
        CliError::fatal(format!("failed to load blob '{}': {e}", hash))
            .with_stable_code(StableErrorCode::RepoCorrupt)
    })?;
    Ok(blob.data)
}

fn read_worktree_blob_content(path_buf: &PathBuf) -> CliResult<Vec<u8>> {
    let absolute = util::workdir_to_absolute(path_buf);
    std::fs::read(&absolute).map_err(|e| {
        CliError::fatal(format!("failed to read file '{}': {e}", absolute.display()))
            .with_stable_code(StableErrorCode::IoReadFailed)
    })
}

fn record_diff_content_error(slot: &Rc<RefCell<Option<CliError>>>, error: CliError) {
    let mut slot = slot.borrow_mut();
    if slot.is_none() {
        *slot = Some(error);
    }
}

fn render_diff_output(
    args: &DiffArgs,
    result: &DiffOutput,
    output: &OutputConfig,
) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("diff", result, output);
    }

    // --output writes are an explicit side-effect and must be honored even
    // when --quiet is set (quiet only suppresses stdout, not file writes).
    let rendered = if args.name_only {
        result
            .files
            .iter()
            .map(|file| file.path.clone())
            .collect::<Vec<_>>()
            .join("\n")
    } else if args.name_status {
        result
            .files
            .iter()
            .map(|file| format!("{}\t{}", diff_status_letter(&file.status), file.path))
            .collect::<Vec<_>>()
            .join("\n")
    } else if args.numstat {
        result
            .files
            .iter()
            .map(|file| format!("{}\t{}\t{}", file.insertions, file.deletions, file.path))
            .collect::<Vec<_>>()
            .join("\n")
    } else if args.stat {
        format_diff_stat_output(result)
    } else {
        format_unified_diff(result)
    };

    if let Some(path) = &args.output {
        std::fs::write(path, rendered.as_bytes()).map_err(|e| {
            CliError::fatal(format!("failed to write output file '{}': {e}", path))
                .with_stable_code(StableErrorCode::IoWriteFailed)
        })?;
        return Ok(());
    }

    if output.quiet {
        return Ok(());
    }

    let mut pager = Pager::with_config(output)?;
    if rendered.is_empty() {
        return Ok(());
    }
    let rendered = if args.name_only || args.name_status || args.numstat || args.stat {
        rendered
    } else {
        maybe_colorize_diff(&rendered, io::stdout().is_terminal())
    };
    pager.write_str(&format!("{rendered}\n"))?;
    pager.finish()?;
    Ok(())
}

fn diff_status_letter(status: &str) -> &'static str {
    match status {
        "added" => "A",
        "deleted" => "D",
        _ => "M",
    }
}

fn format_unified_diff(result: &DiffOutput) -> String {
    result
        .files
        .iter()
        .map(|file| file.raw_diff.trim_end_matches('\n'))
        .collect::<Vec<_>>()
        .join("\n")
}

fn maybe_colorize_diff(diff_text: &str, should_colorize: bool) -> String {
    if should_colorize {
        colorize_diff(diff_text)
    } else {
        diff_text.to_string()
    }
}

fn format_diff_stat_output(result: &DiffOutput) -> String {
    if result.files.is_empty() {
        return String::new();
    }

    let mut lines = result
        .files
        .iter()
        .map(|file| {
            let total = file.insertions + file.deletions;
            let bar = format!(
                "{}{}",
                "+".repeat(file.insertions.min(40)),
                "-".repeat(file.deletions.min(40))
            );
            format!(" {} | {} {}", file.path, total, bar)
        })
        .collect::<Vec<_>>();
    lines.push(format!(
        " {} file{} changed, {} insertion{}(+), {} deletion{}(-)",
        result.files_changed,
        if result.files_changed == 1 { "" } else { "s" },
        result.total_insertions,
        if result.total_insertions == 1 {
            ""
        } else {
            "s"
        },
        result.total_deletions,
        if result.total_deletions == 1 { "" } else { "s" }
    ));
    lines.join("\n")
}

fn parse_diff_item(item: &git_internal::diff::DiffItem) -> DiffFileStat {
    let status = parse_diff_status(&item.data);
    let insertions = item
        .data
        .lines()
        .filter(|line| line.starts_with('+') && !line.starts_with("+++"))
        .count();
    let deletions = item
        .data
        .lines()
        .filter(|line| line.starts_with('-') && !line.starts_with("---"))
        .count();

    DiffFileStat {
        path: item.path.clone(),
        status: status.to_string(),
        insertions,
        deletions,
        hunks: parse_diff_hunks(&item.data),
        raw_diff: item.data.clone(),
    }
}

fn parse_diff_status(diff_text: &str) -> &'static str {
    for line in diff_text.lines() {
        if line.starts_with("@@ ") || line == "Binary files differ" {
            break;
        }
        if line.starts_with("new file mode ") || line == "--- /dev/null" {
            return "added";
        }
        if line.starts_with("deleted file mode ") || line == "+++ /dev/null" {
            return "deleted";
        }
    }

    "modified"
}

fn parse_diff_hunks(diff_text: &str) -> Vec<DiffHunk> {
    let mut hunks = Vec::new();
    let mut current: Option<DiffHunk> = None;

    for line in diff_text.lines() {
        if let Some(header) = line.strip_prefix("@@ ") {
            if let Some(hunk) = current.take() {
                hunks.push(hunk);
            }
            current =
                parse_hunk_header(header).map(|(old_start, old_lines, new_start, new_lines)| {
                    DiffHunk {
                        old_start,
                        old_lines,
                        new_start,
                        new_lines,
                        lines: Vec::new(),
                    }
                });
            continue;
        }

        if let Some(hunk) = &mut current
            && (line.starts_with('+')
                || line.starts_with('-')
                || line.starts_with(' ')
                || line.starts_with("\\ No newline"))
        {
            hunk.lines.push(line.to_string());
        }
    }

    if let Some(hunk) = current {
        hunks.push(hunk);
    }

    hunks
}

fn parse_hunk_header(header: &str) -> Option<(usize, usize, usize, usize)> {
    let before_suffix = header.split(" @@").next()?;
    let mut parts = before_suffix.split(' ');
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    Some((
        parse_hunk_range(old)?,
        parse_hunk_range_count(old)?,
        parse_hunk_range(new)?,
        parse_hunk_range_count(new)?,
    ))
}

fn parse_hunk_range(value: &str) -> Option<usize> {
    value.split(',').next()?.parse().ok()
}

fn parse_hunk_range_count(value: &str) -> Option<usize> {
    match value.split_once(',') {
        Some((_, count)) => count.parse().ok(),
        None => Some(1),
    }
}

fn colorize_diff(diff_text: &str) -> String {
    let mut output = String::with_capacity(diff_text.len() + 500);

    for line in diff_text.lines() {
        let colored_line = if line.starts_with("diff --git") {
            line.bold().to_string()
        } else if line.starts_with("@@") {
            line.cyan().to_string()
        } else if line.starts_with('-') && !line.starts_with("---") {
            line.red().to_string()
        } else if line.starts_with('+') && !line.starts_with("+++") {
            line.green().to_string()
        } else {
            line.to_string()
        };

        output.push_str(&colored_line);
        output.push('\n');
    }
    output
}

struct Line(Option<usize>);

impl fmt::Display for Line {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.0 {
            None => write!(f, "    "),
            Some(idx) => write!(f, "{:<4}", idx + 1),
        }
    }
}

#[allow(dead_code)]
fn similar_diff_result(old: &str, new: &str, w: &mut dyn io::Write) {
    let diff = similar::TextDiff::from_lines(old, new);
    for (idx, group) in diff.grouped_ops(3).iter().enumerate() {
        if idx > 0 {
            println!("{:-^1$}", "-", 80);
        }
        for op in group {
            for change in diff.iter_changes(op) {
                let sign = match change.tag() {
                    similar::ChangeTag::Delete => "-",
                    similar::ChangeTag::Insert => "+",
                    similar::ChangeTag::Equal => " ",
                };
                write!(
                    w,
                    "{}{} |{}",
                    Line(change.old_index()),
                    Line(change.new_index()),
                    sign
                )
                .unwrap();
                write!(w, "{}", change.value()).unwrap();
                if change.missing_newline() {
                    writeln!(w).unwrap();
                }
            }
        }
    }
}

#[cfg(test)]
mod test {
    use std::{fs, io::Write};

    use serial_test::serial;
    use tempfile::tempdir;

    use super::*;
    use crate::utils::test;
    #[test]
    /// Tests command line argument parsing for the diff command with various parameter combinations.
    /// Verifies parameter requirements, conflicts and default values are handled correctly.
    fn test_args() {
        {
            let args = DiffArgs::try_parse_from(["diff", "--old", "old", "--new", "new", "paths"]);
            assert!(args.is_ok());
            let args = args.unwrap();
            assert_eq!(args.old, Some("old".to_string()));
            assert_eq!(args.new, Some("new".to_string()));
            assert_eq!(args.pathspec, vec!["paths".to_string()]);
        }
        {
            // --staged didn't require --old
            let args =
                DiffArgs::try_parse_from(["diff", "--staged", "pathspec", "--output", "output"]);
            let args = args.unwrap();
            assert_eq!(args.old, None);
            assert!(args.staged);
        }
        {
            // --staged conflicts with --new
            let args = DiffArgs::try_parse_from([
                "diff", "--old", "old", "--new", "new", "--staged", "paths",
            ]);
            assert!(args.is_err());
            assert!(args.err().unwrap().kind() == clap::error::ErrorKind::ArgumentConflict);
        }
        {
            // --new requires --old
            let args = DiffArgs::try_parse_from([
                "diff", "--new", "new", "pathspec", "--output", "output",
            ]);
            assert!(args.is_err());
            assert!(args.err().unwrap().kind() == clap::error::ErrorKind::MissingRequiredArgument);
        }
        // TODO: Enable these tests when --algorithm arg is fully implemented
        // {
        //     // --algorithm arg
        //     let args = DiffArgs::try_parse_from([
        //         "diff",
        //         "--old",
        //         "old",
        //         "--new",
        //         "new",
        //         "--algorithm",
        //         "myers",
        //         "target paths",
        //     ])
        //     .unwrap();
        //     assert_eq!(args.algorithm, Some("myers".to_string()));
        // }
        // {
        //     // --algorithm arg with default value
        //     let args = DiffArgs::try_parse_from(["diff", "--old", "old", "target paths"]).unwrap();
        //     assert_eq!(args.algorithm, Some("histogram".to_string()));
        // }
    }

    #[test]
    /// Tests the functionality of the `similar_diff_result` function.
    /// Verifies that it correctly generates a diff between two text inputs.
    fn test_similar_diff_result() {
        let old = "Hello World\nThis is the second line.\nThis is the third.";
        let new = "Hallo Welt\nThis is the second line.\nThis is life.\nMoar and more";
        let mut buf = Vec::new();
        similar_diff_result(old, new, &mut buf);
        let result = String::from_utf8(buf).unwrap();
        println!("{result}");
    }

    #[test]
    fn test_maybe_colorize_diff_respects_flag() {
        let diff = "diff --git a/file.txt b/file.txt\n--- /dev/null\n+++ b/file.txt\n+line\n";
        colored::control::set_override(true);

        let plain = maybe_colorize_diff(diff, false);
        let colored = maybe_colorize_diff(diff, true);

        assert!(
            !plain.contains("\u{1b}["),
            "plain output should not contain ANSI escapes"
        );
        assert!(
            colored.contains("\u{1b}["),
            "colored output should contain ANSI escapes"
        );

        colored::control::unset_override();
    }

    #[tokio::test]
    #[serial]
    /// Tests that the get_files_blobs function properly respects .libraignore patterns.
    /// Verifies ignored files are correctly excluded from the blob collection process.
    async fn test_get_files_blob_gitignore() {
        let temp_path = tempdir().unwrap();
        test::setup_with_new_libra_in(temp_path.path()).await;
        let _guard = test::ChangeDirGuard::new(temp_path.path());

        let mut gitignore_file = fs::File::create(".libraignore").unwrap();
        gitignore_file.write_all(b"should_ignore").unwrap();

        fs::File::create("should_ignore").unwrap();
        fs::File::create("not_ignore").unwrap();

        let index = Index::load(path::index()).unwrap();
        let blob = get_files_blobs(
            &[PathBuf::from("should_ignore"), PathBuf::from("not_ignore")],
            &index,
            IgnorePolicy::Respect,
        )
        .unwrap();
        assert_eq!(blob.len(), 1);
        assert_eq!(blob[0].0, PathBuf::from("not_ignore"));
    }
}
