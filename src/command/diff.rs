//! Provides diff command logic comparing commits, the index, and the working tree with algorithm selection, pathspec filtering, and optional file output.

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    io::{self, IsTerminal},
    path::{Path, PathBuf},
    rc::Rc,
};

use clap::Parser;
use colored::Colorize;
use git_internal::{
    Diff,
    hash::ObjectHash,
    internal::{
        index::{Index, IndexEntry, Time},
        object::{blob::Blob, commit::Commit, tree::Tree, types::ObjectType},
        pack::utils::calculate_object_hash,
    },
};
use serde::Serialize;

use crate::{
    command::{get_target_commit, load_object},
    internal::head::Head,
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        ignore::{self, IgnorePolicy},
        object_ext::TreeExt,
        output::{OutputConfig, ProgressMode, emit_json_data},
        pager::Pager,
        path, util,
    },
};

const DIFF_EXAMPLES: &str = "\
EXAMPLES:
    libra diff                              Compare index against the working tree
    libra diff --staged                     Compare HEAD against the index
    libra diff --old HEAD~1 --new HEAD      Compare two revisions
    libra diff --stat src/                  Show diff statistics under src/
    libra diff --shortstat                  Show just the files-changed/insertions/deletions line
    libra diff -s --exit-code               Status-only check: no output, exit 1 if changes
    libra diff --name-only -z               NUL-terminated changed-file list for scripts
    libra diff --cached --check             Warn about whitespace errors on added lines
    libra diff -R                           Reverse diff (swap additions and deletions)
    libra --json diff --staged              Structured JSON output for agents";

#[derive(Parser, Debug)]
#[command(after_help = DIFF_EXAMPLES)]
pub struct DiffArgs {
    /// Old commit, default is HEAD
    #[clap(long, value_name = "COMMIT")]
    pub old: Option<String>,

    /// New commit, default is working directory
    #[clap(long, value_name = "COMMIT")]
    #[clap(requires = "old", group = "op_new")]
    pub new: Option<String>,

    /// Use stage as new commit. This option is conflict with --new.
    /// `--cached` is accepted as a Git-compatible alias for `--staged`.
    #[clap(long, visible_alias = "cached")]
    #[clap(group = "op_new")]
    pub staged: bool,

    #[clap(help = "Files to compare")]
    pathspec: Vec<String>,

    /// Diff algorithm. `histogram` is currently the only implemented backend.
    #[clap(
        long,
        default_value = "histogram",
        value_name = "NAME",
        value_parser = ["histogram", "myers", "myersMinimal"],
    )]
    pub algorithm: Option<String>,

    /// Write the diff to `FILENAME` instead of stdout
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

    /// Show a condensed summary of created and deleted files
    #[clap(long)]
    pub summary: bool,

    /// Output only the last line of `--stat`: the files-changed / insertions /
    /// deletions summary.
    #[clap(long)]
    pub shortstat: bool,

    /// Exit with code 1 when there are differences, 0 when there are none
    /// (the diff is still printed, unlike `--quiet`).
    #[clap(long = "exit-code")]
    pub exit_code: bool,

    /// Suppress the patch (diff body) output. Combine with `--exit-code` for a
    /// status-only check.
    #[clap(short = 's', long = "no-patch")]
    pub no_patch: bool,

    /// NUL-terminate output records (for `--name-only`/`--name-status`/`--numstat`);
    /// `--name-status` then emits the status and path as separate NUL fields.
    #[clap(short = 'z', long = "null")]
    pub null: bool,

    /// Warn about whitespace errors on added lines instead of printing the diff.
    /// Detects trailing whitespace and space-before-tab in the indent; exits 2
    /// when any problem is found. (Git's blank-at-eof check is not performed.)
    #[clap(long = "check")]
    pub check: bool,

    /// Show the reverse diff: swap the two sides so additions become deletions
    /// and vice-versa (e.g. the patch that would undo the change).
    #[clap(short = 'R', long = "reverse")]
    pub reverse: bool,

    /// Treat all files as text. Accepted for Git parity and is a no-op: Libra's
    /// diff never performs binary detection, so it already shows the content
    /// diff of every file (it never prints "Binary files differ").
    #[clap(short = 'a', long = "text")]
    pub text: bool,

    /// Disallow external diff drivers. Accepted for Git parity and is a no-op:
    /// Libra has no external diff driver support and always uses its built-in
    /// diff engine, so external drivers are never invoked to begin with.
    #[clap(long = "no-ext-diff")]
    pub no_ext_diff: bool,

    /// Do not color moved lines differently from added/removed lines. Accepted
    /// for Git parity and is a no-op: Libra's diff never performs moved-line
    /// detection or coloring, so this already matches the default. (Git's
    /// opposite `--color-moved[=<mode>]` is not implemented.)
    #[clap(long = "no-color-moved")]
    pub no_color_moved: bool,

    /// Turn off rename detection. Accepted for Git parity and is a no-op:
    /// Libra's diff never detects renames (a rename shows as delete + create),
    /// so this already matches the default. (Git's `--renames` is not exposed.)
    #[clap(long = "no-renames")]
    pub no_renames: bool,

    /// Show paths relative to the repository root, not the current directory.
    /// Accepted for Git parity and is a no-op: Libra's diff always shows
    /// repo-root-relative paths. (Git's `--relative[=<path>]` is not exposed.)
    #[clap(long = "no-relative")]
    pub no_relative: bool,

    /// Disable the indent heuristic for hunk boundaries. Accepted for Git parity
    /// and is a no-op: Libra's diff does not apply Git's indent heuristic.
    /// (Git's `--indent-heuristic` is not exposed.)
    #[clap(long = "no-indent-heuristic")]
    pub no_indent_heuristic: bool,

    /// Do not run a textconv filter to make binary files diffable. Accepted for
    /// Git parity and is a no-op: Libra's diff has no textconv filters and
    /// always diffs the raw content. (Git's `--textconv` is not exposed.)
    #[clap(long = "no-textconv")]
    pub no_textconv: bool,
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

#[derive(Debug, thiserror::Error)]
pub(crate) enum DiffError {
    #[error("not a libra repository")]
    NotInRepo,

    #[error("invalid revision: '{0}'")]
    InvalidRevision(String),

    #[error("failed to load {kind} '{object_id}': {detail}")]
    ObjectLoad {
        kind: &'static str,
        object_id: String,
        detail: String,
    },

    #[error("failed to load index: {0}")]
    IndexLoad(String),

    #[error("failed to list working directory files: {0}")]
    WorkdirList(String),

    #[error("failed to read file '{path}': {detail}")]
    FileRead { path: String, detail: String },

    #[error("failed to write output file '{path}': {detail}")]
    OutputWrite { path: String, detail: String },

    #[error(
        "diff --algorithm={0} is not supported yet; only --algorithm=histogram is currently implemented"
    )]
    UnsupportedAlgorithm(String),
}

impl From<DiffError> for CliError {
    fn from(error: DiffError) -> Self {
        let message = error.to_string();
        match error {
            DiffError::NotInRepo => CliError::repo_not_found(),
            DiffError::InvalidRevision(_) => CliError::fatal(message)
                .with_stable_code(StableErrorCode::CliInvalidTarget)
                .with_hint("check the revision name and try again"),
            DiffError::ObjectLoad { .. } => CliError::fatal(message)
                .with_stable_code(StableErrorCode::RepoCorrupt)
                .with_hint("the object store may be corrupted; try 'libra status' to verify"),
            DiffError::IndexLoad(_) => CliError::fatal(message)
                .with_stable_code(StableErrorCode::RepoCorrupt)
                .with_hint("the index file may be corrupted"),
            DiffError::WorkdirList(_) => {
                CliError::fatal(message).with_stable_code(StableErrorCode::IoReadFailed)
            }
            DiffError::FileRead { .. } => {
                CliError::fatal(message).with_stable_code(StableErrorCode::IoReadFailed)
            }
            DiffError::OutputWrite { .. } => {
                CliError::fatal(message).with_stable_code(StableErrorCode::IoWriteFailed)
            }
            DiffError::UnsupportedAlgorithm(_) => CliError::fatal(message)
                .with_stable_code(StableErrorCode::CliInvalidArguments)
                .with_hint(
                    "omit --algorithm or use --algorithm=histogram until alternate diff backends are available",
                ),
        }
    }
}

pub async fn execute(args: DiffArgs) {
    if let Err(err) = execute_safe(args, &OutputConfig::default()).await {
        err.print_stderr();
    }
}

pub async fn execute_safe(args: DiffArgs, output: &OutputConfig) -> CliResult<()> {
    if util::require_repo().is_err() {
        return Err(CliError::from(DiffError::NotInRepo));
    }
    let mut args = args;
    normalize_diff_range(&mut args).await;
    validate_diff_algorithm(&args).map_err(CliError::from)?;
    emit_worktree_scan_progress(&args, output);
    let result = run_diff(&args).await.map_err(CliError::from)?;
    render_diff_output(&args, &result, output)
}

/// `diff A..B`: when no `--old`/`--new`/`--staged` is supplied and the first
/// positional is a two-dot range whose sides both resolve to commits, rewrite it
/// into `--old`/`--new`. `A..` diffs A against the working tree; `..B` diffs HEAD
/// against B. The rewrite only fires when the sides resolve as commits, so a real
/// path containing `..` is left untouched as a pathspec. Three-dot (`A...B`)
/// merge-base ranges are not yet handled and fall through to pathspec matching.
async fn normalize_diff_range(args: &mut DiffArgs) {
    if args.old.is_some() || args.new.is_some() || args.staged {
        return;
    }
    let Some(first) = args.pathspec.first().cloned() else {
        return;
    };
    if first.contains("...") || !first.contains("..") {
        return;
    }
    let Some((left, right)) = first.split_once("..") else {
        return;
    };
    let left_spec = if left.is_empty() { "HEAD" } else { left };
    let left_ok = crate::command::get_target_commit(left_spec).await.is_ok();
    let right_ok = right.is_empty() || crate::command::get_target_commit(right).await.is_ok();
    if !left_ok || !right_ok {
        return;
    }
    args.old = Some(left_spec.to_string());
    if !right.is_empty() {
        args.new = Some(right.to_string());
    }
    args.pathspec.remove(0);
}

fn validate_diff_algorithm(args: &DiffArgs) -> Result<(), DiffError> {
    match args.algorithm.as_deref().unwrap_or("histogram") {
        "histogram" => Ok(()),
        unsupported => Err(DiffError::UnsupportedAlgorithm(unsupported.to_string())),
    }
}

fn emit_worktree_scan_progress(args: &DiffArgs, output: &OutputConfig) {
    if output.quiet || output.is_json() || args.staged || args.new.is_some() {
        return;
    }

    match output.progress {
        ProgressMode::Text => eprintln!("Scanning working tree ..."),
        ProgressMode::Json => {
            let event = serde_json::json!({
                "event": "diff_scan.start",
                "task": "Scanning working tree",
            });
            eprintln!("{event}");
        }
        // OutputConfig resolves `--progress=auto` to None when stderr is not a
        // TTY. `diff` still emits this one-line startup signal for auto mode so
        // large ignored trees do not look hung in captured/non-interactive runs.
        ProgressMode::None
            if output.progress_preference != crate::utils::output::ProgressPreference::None =>
        {
            eprintln!("Scanning working tree ...")
        }
        ProgressMode::None => {}
    }
}

async fn run_diff(args: &DiffArgs) -> Result<DiffOutput, DiffError> {
    util::require_repo().map_err(|_| DiffError::NotInRepo)?;
    tracing::debug!("diff args: {:?}", args);
    let index = Index::load(path::index()).map_err(|e| DiffError::IndexLoad(e.to_string()))?;

    let old_side = resolve_diff_side(&args.old, args.staged, false, &index).await?;
    let new_side = resolve_diff_side(&args.new, args.staged, true, &index).await?;

    let paths: Vec<PathBuf> = args.pathspec.iter().map(util::to_workdir_path).collect();
    let worktree_entries = new_side.worktree_entries.clone();
    let worktree_cache = RefCell::new(HashMap::<ObjectHash, Vec<u8>>::new());
    let repo_cache = RefCell::new(HashMap::<ObjectHash, Vec<u8>>::new());
    let load_error = Rc::new(RefCell::new(None::<DiffError>));
    let load_error_for_read = Rc::clone(&load_error);
    // `-R`/`--reverse`: swap the two sides so the diff is computed new->old. The
    // loader resolves blobs by hash (content-addressed) and the worktree check
    // above stays correct regardless of which side a blob lands on.
    let (first_blobs, second_blobs, old_label, new_label) = if args.reverse {
        (
            new_side.blobs,
            old_side.blobs,
            new_side.label,
            old_side.label,
        )
    } else {
        (
            old_side.blobs,
            new_side.blobs,
            old_side.label,
            new_side.label,
        )
    };
    let diff_output = Diff::diff(first_blobs, second_blobs, paths, move |path, hash| {
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
        old_ref: old_label,
        new_ref: new_label,
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
) -> Result<Vec<(PathBuf, ObjectHash)>, DiffError> {
    files
        .iter()
        .filter(|path| !ignore::should_ignore(path, policy, index))
        .map(|p| {
            if let Some(hash) = index_hash_if_worktree_stat_matches(p, index) {
                return Ok((p.to_owned(), hash));
            }
            let path = util::workdir_to_absolute(p);
            let data = std::fs::read(&path).map_err(|e| DiffError::FileRead {
                path: path.display().to_string(),
                detail: e.to_string(),
            })?;
            Ok((p.to_owned(), calculate_object_hash(ObjectType::Blob, &data)))
        })
        .collect()
}

fn index_hash_if_worktree_stat_matches(path: &Path, index: &Index) -> Option<ObjectHash> {
    let entry = index.get(path.to_str()?, 0)?;
    let absolute = util::workdir_to_absolute(path);
    let metadata = std::fs::symlink_metadata(&absolute).ok()?;
    index_entry_matches_worktree_stat(entry, &metadata).then_some(entry.hash)
}

fn index_entry_matches_worktree_stat(entry: &IndexEntry, metadata: &std::fs::Metadata) -> bool {
    let Ok(size) = u32::try_from(metadata.len()) else {
        return false;
    };
    let Ok(ctime) = metadata.created().map(Time::from_system_time) else {
        return false;
    };
    let Ok(mtime) = metadata.modified().map(Time::from_system_time) else {
        return false;
    };

    entry.ctime == ctime
        && entry.mtime == mtime
        && entry.dev == index_dev_from_metadata(metadata)
        && entry.ino == index_ino_from_metadata(metadata)
        && entry.size == size
        && entry.uid == index_uid_from_metadata(metadata)
        && entry.gid == index_gid_from_metadata(metadata)
        && entry.mode == index_mode_from_metadata(metadata)
}

fn index_dev_from_metadata(metadata: &std::fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        metadata.dev() as u32
    }

    #[cfg(not(unix))]
    {
        let _ = metadata;
        0
    }
}

fn index_ino_from_metadata(metadata: &std::fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        metadata.ino() as u32
    }

    #[cfg(not(unix))]
    {
        let _ = metadata;
        0
    }
}

fn index_uid_from_metadata(metadata: &std::fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        metadata.uid()
    }

    #[cfg(not(unix))]
    {
        let _ = metadata;
        0
    }
}

fn index_gid_from_metadata(metadata: &std::fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        metadata.gid()
    }

    #[cfg(not(unix))]
    {
        let _ = metadata;
        0
    }
}

fn index_mode_from_metadata(metadata: &std::fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        match metadata.mode() & 0o170000 {
            0o100000 => match metadata.mode() & 0o111 {
                0 => 0o100644,
                _ => 0o100755,
            },
            0o120000 => 0o120000,
            _ => 0o100644,
        }
    }

    #[cfg(windows)]
    {
        if metadata.file_type().is_symlink() {
            0o120000
        } else {
            0o100644
        }
    }

    #[cfg(not(any(unix, windows)))]
    {
        0o100644
    }
}

fn get_worktree_diff_files(index: &Index) -> Result<Vec<PathBuf>, DiffError> {
    let mut seen = HashSet::new();
    let mut files = Vec::new();

    for file in util::list_workdir_files().map_err(|e| DiffError::WorkdirList(e.to_string()))? {
        if seen.insert(file.clone()) {
            files.push(file);
        }
    }

    for file in index.tracked_files() {
        let absolute = util::workdir_to_absolute(&file);
        if absolute.is_file() && seen.insert(file.clone()) {
            files.push(file);
        }
    }

    Ok(files)
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
) -> Result<DiffSide, DiffError> {
    if let Some(source) = source {
        let commit_hash = get_target_commit(source)
            .await
            .map_err(|_| DiffError::InvalidRevision(source.clone()))?;
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
            let files = get_worktree_diff_files(index)?;
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

async fn get_commit_blobs(
    commit_hash: &ObjectHash,
) -> Result<Vec<(PathBuf, ObjectHash)>, DiffError> {
    let commit = load_object::<Commit>(commit_hash).map_err(|e| DiffError::ObjectLoad {
        kind: "commit",
        object_id: commit_hash.to_string(),
        detail: e.to_string(),
    })?;
    let tree = load_object::<Tree>(&commit.tree_id).map_err(|e| DiffError::ObjectLoad {
        kind: "tree",
        object_id: commit.tree_id.to_string(),
        detail: e.to_string(),
    })?;
    Ok(tree.get_plain_items())
}

/// Render a Git-style `--stat` block for the changes between two commits'
/// trees, reusing the same diff engine and `--stat` formatter as `libra diff
/// --stat`. Used by `libra merge --stat` to show what a merge changed. Returns
/// an empty string when the two trees are identical.
pub(crate) async fn diff_stat_between_commits(
    old_commit: &ObjectHash,
    new_commit: &ObjectHash,
) -> Result<String, DiffError> {
    let old_blobs = get_commit_blobs(old_commit).await?;
    let new_blobs = get_commit_blobs(new_commit).await?;

    // Capture the first blob-read failure from the (infallible-signature) diff
    // closure and surface it after, mirroring `run_diff`.
    let load_error: RefCell<Option<DiffError>> = RefCell::new(None);
    let diff_output =
        Diff::diff(
            old_blobs,
            new_blobs,
            Vec::new(),
            |_path, hash| match load_repo_blob_content(hash) {
                Ok(data) => data,
                Err(err) => {
                    if load_error.borrow().is_none() {
                        *load_error.borrow_mut() = Some(err);
                    }
                    Vec::new()
                }
            },
        );
    if let Some(err) = load_error.borrow_mut().take() {
        return Err(err);
    }

    let files: Vec<DiffFileStat> = diff_output.iter().map(parse_diff_item).collect();
    let total_insertions = files.iter().map(|file| file.insertions).sum();
    let total_deletions = files.iter().map(|file| file.deletions).sum();
    let files_changed = files.len();
    let output = DiffOutput {
        old_ref: old_commit.to_string(),
        new_ref: new_commit.to_string(),
        files,
        total_insertions,
        total_deletions,
        files_changed,
    };
    Ok(format_diff_stat_output(&output))
}

fn load_repo_blob_content(hash: &ObjectHash) -> Result<Vec<u8>, DiffError> {
    let blob = load_object::<Blob>(hash).map_err(|e| DiffError::ObjectLoad {
        kind: "blob",
        object_id: hash.to_string(),
        detail: e.to_string(),
    })?;
    Ok(blob.data)
}

fn read_worktree_blob_content(path_buf: &PathBuf) -> Result<Vec<u8>, DiffError> {
    let absolute = util::workdir_to_absolute(path_buf);
    std::fs::read(&absolute).map_err(|e| DiffError::FileRead {
        path: absolute.display().to_string(),
        detail: e.to_string(),
    })
}

fn record_diff_content_error(slot: &Rc<RefCell<Option<DiffError>>>, error: DiffError) {
    let mut slot = slot.borrow_mut();
    if slot.is_none() {
        *slot = Some(error);
    }
}

/// Identify the first whitespace problem on an added line's content (the text
/// after the leading `+`). Returns `None` for a clean line. Checks Git's two
/// most common defaults: trailing whitespace (`blank-at-eol`) and a space
/// immediately before a tab in the leading indent (`space-before-tab`).
fn whitespace_problem(content: &str) -> Option<&'static str> {
    if content.ends_with(' ') || content.ends_with('\t') {
        return Some("trailing whitespace");
    }
    let indent: String = content
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    if indent.contains(" \t") {
        return Some("space before tab in indent");
    }
    None
}

/// Scan one file's unified diff for whitespace errors on added (`+`) lines,
/// tracking new-file line numbers from each hunk header. Returns one
/// `path:line: message` string per problem.
fn check_whitespace_in_file(path: &str, raw_diff: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let mut new_lineno = 0usize;
    for line in raw_diff.lines() {
        if line.starts_with("@@") {
            // `@@ -a,b +c,d @@`: the next added/context line is new-file line c.
            if let Some(after_plus) = line.split('+').nth(1)
                && let Some(start) = after_plus
                    .split([',', ' '])
                    .next()
                    .and_then(|s| s.parse::<usize>().ok())
            {
                new_lineno = start;
            }
        } else if line.starts_with("+++") || line.starts_with("---") {
            // File headers — not content; do not advance.
        } else if let Some(content) = line.strip_prefix('+') {
            // Added line: check whitespace, then advance the new-file counter.
            if let Some(msg) = whitespace_problem(content) {
                problems.push(format!("{path}:{new_lineno}: {msg}"));
            }
            new_lineno += 1;
        } else if line.starts_with(' ') {
            // Context line: advances the new-file counter.
            new_lineno += 1;
        }
        // Everything else — removed (`-`) lines, the `\ No newline at end of
        // file` marker, and `diff --git`/`index`/mode headers — is neither an
        // added nor a context line and does not advance the counter.
    }
    problems
}

/// `diff --check`: print whitespace warnings and exit 2 when any are found.
fn render_diff_check(result: &DiffOutput) -> CliResult<()> {
    let problems: Vec<String> = result
        .files
        .iter()
        .flat_map(|file| check_whitespace_in_file(&file.path, &file.raw_diff))
        .collect();
    if problems.is_empty() {
        return Ok(());
    }
    println!("{}", problems.join("\n"));
    Err(CliError::silent_exit(2))
}

fn render_diff_output(
    args: &DiffArgs,
    result: &DiffOutput,
    output: &OutputConfig,
) -> CliResult<()> {
    // `--check` replaces the normal diff output with whitespace-error warnings.
    if args.check {
        return render_diff_check(result);
    }
    if output.is_json() {
        emit_json_data("diff", result, output)?;
        // `--exit-code` applies regardless of output format: emit the JSON, then
        // signal differences via the process status.
        return diff_exit_result(args, result);
    }

    if output.quiet && args.output.is_none() {
        return if result.files_changed > 0 {
            Err(CliError::silent_exit(1))
        } else {
            Ok(())
        };
    }

    // --output writes are an explicit side-effect and must be honored even
    // when --quiet is set (quiet only suppresses stdout, not file writes).
    // `-z` NUL-terminates each record; `--name-status` then separates the
    // status and path with a NUL instead of a tab.
    let rendered = if args.name_only {
        join_diff_records(result.files.iter().map(|file| file.path.clone()), args.null)
    } else if args.name_status {
        let field_sep = if args.null { '\0' } else { '\t' };
        join_diff_records(
            result.files.iter().map(|file| {
                format!(
                    "{}{}{}",
                    diff_status_letter(&file.status),
                    field_sep,
                    file.path
                )
            }),
            args.null,
        )
    } else if args.numstat {
        join_diff_records(
            result
                .files
                .iter()
                .map(|file| format!("{}\t{}\t{}", file.insertions, file.deletions, file.path)),
            args.null,
        )
    } else if args.stat {
        format_diff_stat_output(result)
    } else if args.shortstat {
        format_diff_shortstat_output(result)
    } else if args.summary {
        format_diff_summary(result)
    } else if args.no_patch {
        // `-s` / `--no-patch`: suppress the patch body (used for status-only
        // checks, typically with `--exit-code`).
        String::new()
    } else {
        format_unified_diff(result)
    };

    if let Some(path) = &args.output {
        std::fs::write(path, rendered.as_bytes())
            .map_err(|e| DiffError::OutputWrite {
                path: path.clone(),
                detail: e.to_string(),
            })
            .map_err(CliError::from)?;
        if output.quiet && result.files_changed > 0 {
            return Err(CliError::silent_exit(1));
        }
        return diff_exit_result(args, result);
    }

    if output.quiet {
        if result.files_changed > 0 {
            return Err(CliError::silent_exit(1));
        }
        return Ok(());
    }

    if rendered.is_empty() {
        return diff_exit_result(args, result);
    }
    let mut pager = Pager::with_config(output)?;
    let rendered = if args.name_only
        || args.name_status
        || args.numstat
        || args.stat
        || args.shortstat
        || args.summary
    {
        rendered
    } else {
        maybe_colorize_diff(&rendered, io::stdout().is_terminal())
    };
    // `-z` records already carry their own NUL terminators, so do not append a
    // trailing newline in that case.
    let z_records = args.null && (args.name_only || args.name_status || args.numstat);
    if z_records {
        pager.write_str(&rendered)?;
    } else {
        pager.write_str(&format!("{rendered}\n"))?;
    }
    pager.finish()?;
    diff_exit_result(args, result)
}

/// Join name/numstat records: NUL-terminate each record under `-z`, otherwise
/// newline-separate them (the trailing newline is added by the caller).
fn join_diff_records(records: impl Iterator<Item = String>, null: bool) -> String {
    if null {
        records.map(|r| format!("{r}\0")).collect()
    } else {
        records.collect::<Vec<_>>().join("\n")
    }
}

/// `--exit-code`: exit 1 when the diff is non-empty, 0 otherwise. The diff
/// output (if any) has already been emitted by the time this is called, so the
/// silent exit only sets the process status (unlike `--quiet`, which also
/// suppresses output).
fn diff_exit_result(args: &DiffArgs, result: &DiffOutput) -> CliResult<()> {
    if args.exit_code && result.files_changed > 0 {
        Err(CliError::silent_exit(1))
    } else {
        Ok(())
    }
}

/// Render `--summary`: one line per created or deleted file (plain content
/// modifications produce no line), matching `git diff --summary`. Libra's diff
/// pipeline emits only `new file mode` / `deleted file mode` headers — it does
/// not perform rename detection (a rename shows as delete + create) nor surface
/// mode-only changes — so only those two summary kinds are produced.
fn format_diff_summary(result: &DiffOutput) -> String {
    result
        .files
        .iter()
        .filter_map(summary_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn summary_line(file: &DiffFileStat) -> Option<String> {
    let find = |prefix: &str| {
        file.raw_diff
            .lines()
            .find_map(|l| l.strip_prefix(prefix))
            .map(str::trim)
    };
    if let Some(mode) = find("new file mode ") {
        return Some(format!(" create mode {} {}", mode, file.path));
    }
    if let Some(mode) = find("deleted file mode ") {
        return Some(format!(" delete mode {} {}", mode, file.path));
    }
    None
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

/// Render the staged (index-vs-HEAD) changes as an uncolorized unified diff.
/// Used by `commit -v` to embed the diff into the editor template / stderr.
pub(crate) async fn staged_diff_text() -> Result<String, DiffError> {
    let args = DiffArgs {
        old: None,
        new: None,
        staged: true,
        pathspec: Vec::new(),
        algorithm: Some("histogram".to_string()),
        output: None,
        name_only: false,
        name_status: false,
        numstat: false,
        stat: false,
        summary: false,
        shortstat: false,
        exit_code: false,
        no_patch: false,
        null: false,
        check: false,
        reverse: false,
        text: false,
        no_ext_diff: false,
        no_color_moved: false,
        no_renames: false,
        no_relative: false,
        no_indent_heuristic: false,
        no_textconv: false,
    };
    let result = run_diff(&args).await?;
    Ok(format_unified_diff(&result))
}

fn maybe_colorize_diff(diff_text: &str, should_colorize: bool) -> String {
    if should_colorize {
        colorize_diff(diff_text)
    } else {
        diff_text.to_string()
    }
}

/// Render `--shortstat`: just the trailing summary line of `--stat`, omitting
/// the insertion/deletion clause when its count is zero (matching Git, which
/// shows e.g. ` 1 file changed, 2 insertions(+)` with no deletions clause).
fn format_diff_shortstat_output(result: &DiffOutput) -> String {
    if result.files.is_empty() {
        return String::new();
    }
    let mut line = format!(
        " {} file{} changed",
        result.files_changed,
        if result.files_changed == 1 { "" } else { "s" }
    );
    if result.total_insertions > 0 {
        line.push_str(&format!(
            ", {} insertion{}(+)",
            result.total_insertions,
            if result.total_insertions == 1 {
                ""
            } else {
                "s"
            }
        ));
    }
    if result.total_deletions > 0 {
        line.push_str(&format!(
            ", {} deletion{}(-)",
            result.total_deletions,
            if result.total_deletions == 1 { "" } else { "s" }
        ));
    }
    line
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
    let (insertions, deletions) = count_hunk_line_changes(&item.data);

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

fn count_hunk_line_changes(diff_text: &str) -> (usize, usize) {
    let mut insertions = 0;
    let mut deletions = 0;
    let mut in_hunk = false;

    for line in diff_text.lines() {
        if line.starts_with("@@ ") {
            in_hunk = true;
            continue;
        }

        if !in_hunk {
            continue;
        }

        if line.starts_with('+') {
            insertions += 1;
        } else if line.starts_with('-') {
            deletions += 1;
        }
    }

    (insertions, deletions)
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

#[cfg(test)]
mod test {
    use std::{fs, io::Write};

    use serial_test::serial;
    use tempfile::tempdir;

    use super::*;
    use crate::utils::test;

    struct ColorOverrideReset;

    impl Drop for ColorOverrideReset {
        fn drop(&mut self) {
            colored::control::unset_override();
        }
    }
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
            // --cached is a Git-compatible alias for --staged
            let args = DiffArgs::try_parse_from(["diff", "--cached"]).unwrap();
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
        {
            // --algorithm arg is parsed separately from execution-time support.
            let args = DiffArgs::try_parse_from([
                "diff",
                "--old",
                "old",
                "--new",
                "new",
                "--algorithm",
                "myers",
                "target paths",
            ])
            .unwrap();
            assert_eq!(args.algorithm, Some("myers".to_string()));
        }
        {
            // --algorithm defaults to the only currently supported backend.
            let args = DiffArgs::try_parse_from(["diff", "--old", "old", "target paths"]).unwrap();
            assert_eq!(args.algorithm, Some("histogram".to_string()));
        }
        {
            let args = DiffArgs::try_parse_from([
                "diff",
                "--old",
                "old",
                "--new",
                "new",
                "--algorithm",
                "myers",
                "target paths",
            ])
            .unwrap();
            let err = validate_diff_algorithm(&args).expect_err("myers is not wired yet");
            assert!(matches!(err, DiffError::UnsupportedAlgorithm(value) if value == "myers"));
        }
    }

    #[test]
    #[serial]
    fn test_maybe_colorize_diff_respects_flag() {
        let diff = "diff --git a/file.txt b/file.txt\n--- /dev/null\n+++ b/file.txt\n+line\n";
        let _guard = ColorOverrideReset;
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

    #[tokio::test]
    #[serial]
    async fn test_get_files_blobs_reuses_index_hash_when_stat_matches() {
        let temp_path = tempdir().unwrap();
        test::setup_with_new_libra_in(temp_path.path()).await;
        let _guard = test::ChangeDirGuard::new(temp_path.path());

        fs::write("tracked.txt", "worktree content").unwrap();
        let indexed_content = b"indexed content".to_vec();
        let worktree_content = b"worktree content".to_vec();
        let indexed_hash = calculate_object_hash(ObjectType::Blob, &indexed_content);
        let worktree_hash = calculate_object_hash(ObjectType::Blob, &worktree_content);
        assert_ne!(indexed_hash, worktree_hash);

        let mut index = Index::new();
        index.add(
            IndexEntry::new_from_file(Path::new("tracked.txt"), indexed_hash, temp_path.path())
                .unwrap(),
        );

        let blobs = get_files_blobs(
            &[PathBuf::from("tracked.txt")],
            &index,
            IgnorePolicy::Respect,
        )
        .unwrap();

        assert_eq!(blobs, vec![(PathBuf::from("tracked.txt"), indexed_hash)]);
    }
}
