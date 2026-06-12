//! Provides diff command logic comparing commits, the index, and the working tree with algorithm selection, pathspec filtering, and optional file output.

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::{
    borrow::Cow,
    cell::RefCell,
    collections::{HashMap, HashSet, VecDeque},
    fmt::Write as _,
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
        object::{
            blob::Blob,
            commit::Commit,
            tree::{Tree, TreeItemMode},
            types::ObjectType,
        },
        pack::utils::calculate_object_hash,
    },
};
use serde::Serialize;

use crate::{
    command::{get_target_commit, load_object},
    internal::{config::ConfigKv, head::Head},
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
    libra diff -w                           Ignore all whitespace changes
    libra diff -U5                          Show 5 lines of context (or diff.context)
    libra diff --exit-code                  Exit 1 if there are differences
    libra diff -M                           Detect renames (-C also detects copies)
    libra diff --relative=src               Diff only src/, with paths relative to it
    libra diff --word-diff=plain            Inline word-level diff markers
    libra diff --raw                        Machine-readable raw format
    libra diff -W                           Expand hunks to whole functions
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
    #[clap(long)]
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

    /// Ignore changes in the amount of whitespace (trailing whitespace and runs
    /// of whitespace are treated as a single space for comparison).
    #[clap(short = 'b', long = "ignore-space-change")]
    pub ignore_space_change: bool,

    /// Ignore all whitespace when comparing lines.
    #[clap(short = 'w', long = "ignore-all-space")]
    pub ignore_all_space: bool,

    /// Ignore changes whose lines are all blank.
    #[clap(long = "ignore-blank-lines")]
    pub ignore_blank_lines: bool,

    /// Generate diffs with <n> lines of context (default 3, or `diff.context`).
    // No `default_value`: it must stay `None` when omitted so `diff.context`
    // can supply the default. Priority is `-U` > `diff.context` > 3.
    #[clap(short = 'U', long = "unified", value_name = "N")]
    pub unified: Option<usize>,

    /// Exit with status 1 if there are differences, 0 otherwise (output is
    /// still written, unlike `--quiet`).
    #[clap(long = "exit-code")]
    pub exit_code: bool,

    /// Detect renames. An optional similarity threshold may be attached
    /// (`-M80`, `-M80%`, `--find-renames=80%`); the default is 50%.
    #[clap(short = 'M', long = "find-renames", value_name = "N", num_args = 0..=1, require_equals = true)]
    pub find_renames: Option<Option<String>>,

    /// Detect copies (basic). An optional similarity threshold may be attached
    /// (`-C80`, `--find-copies=80%`); the default is 50%.
    #[clap(short = 'C', long = "find-copies", value_name = "N", num_args = 0..=1, require_equals = true)]
    pub find_copies: Option<Option<String>>,

    /// Disable rename (and copy) detection, even if enabled by config.
    #[clap(long = "no-renames")]
    pub no_renames: bool,

    /// Restrict the diff to a subdirectory and show paths relative to it. With
    /// no value, the current working directory (relative to the repo) is used.
    #[clap(long = "relative", value_name = "PATH", num_args = 0..=1, require_equals = true)]
    pub relative: Option<Option<String>>,

    /// Output the diff in Git's raw format.
    #[clap(long = "raw")]
    pub raw: bool,

    /// Show a word-level diff. Mode is `plain` (default) or `color`.
    #[clap(long = "word-diff", value_name = "MODE", num_args = 0..=1, require_equals = true)]
    pub word_diff: Option<Option<String>>,

    /// Regex describing what a word is for `--word-diff` (max 4096 bytes).
    #[clap(long = "word-diff-regex", value_name = "REGEX")]
    pub word_diff_regex: Option<String>,

    /// Expand each hunk's context to the surrounding function boundaries.
    #[clap(short = 'W', long = "function-context")]
    pub function_context: bool,

    /// Generate combined diff for merge commits (Phase 2 enhancement)
    #[clap(long = "cc", alias = "combined")]
    pub combined: bool,
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
    /// Source path for `renamed` / `copied` entries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    /// Similarity percentage (0–100) for `renamed` / `copied` entries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub similarity: Option<u32>,
    /// File mode on the old/new side (octal, e.g. `0o100644`), surfaced for
    /// rename mode-change headers and `--raw`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_mode: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_mode: Option<u32>,
    /// Full object id on the old/new side (populated for `--raw`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_sha: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_sha: Option<String>,
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
enum DiffError {
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

    #[error("invalid value '{value}' for '{key}': expected a non-negative integer")]
    InvalidConfig { key: String, value: String },

    #[error("{0}")]
    InvalidArgument(String),
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
            DiffError::InvalidConfig { .. } => CliError::fatal(message)
                .with_stable_code(StableErrorCode::CliInvalidArguments)
                .with_hint("set the config value to a non-negative integer"),
            DiffError::InvalidArgument(_) => {
                CliError::fatal(message).with_stable_code(StableErrorCode::CliInvalidArguments)
            }
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
    validate_diff_algorithm(&args).map_err(CliError::from)?;
    validate_diff_word_diff_args(&args).map_err(CliError::from)?;

    // Validate --cc / --combined flag: requires comparing two commits with merge detection
    if args.combined {
        // For now, --cc is accepted but requires explicit commit specification
        // Full implementation with merge detection deferred
        if args.old.is_none() {
            return Err(CliError::from(DiffError::InvalidArgument(
                "--cc/--combined requires explicit --old and --new commit specification"
                    .to_string(),
            )));
        }
    }

    emit_worktree_scan_progress(&args, output);
    let result = run_diff(&args, output.is_json())
        .await
        .map_err(CliError::from)?;
    render_diff_output(&args, &result, output)
}

fn validate_diff_algorithm(args: &DiffArgs) -> Result<(), DiffError> {
    match args.algorithm.as_deref().unwrap_or("histogram") {
        "histogram" => Ok(()),
        unsupported => Err(DiffError::UnsupportedAlgorithm(unsupported.to_string())),
    }
}

fn validate_diff_word_diff_args(args: &DiffArgs) -> Result<(), DiffError> {
    if let Some(Some(mode)) = &args.word_diff
        && !matches!(mode.as_str(), "plain" | "color")
    {
        return Err(DiffError::InvalidArgument(format!(
            "--word-diff mode '{mode}' is not supported; use 'plain' or 'color'"
        )));
    }
    if let Some(re) = &args.word_diff_regex
        && re.len() > 4096
    {
        return Err(DiffError::InvalidArgument(
            "--word-diff-regex must be at most 4096 bytes".to_string(),
        ));
    }
    if let Some(re) = &args.word_diff_regex
        && let Err(err) = regex::Regex::new(re)
    {
        return Err(DiffError::InvalidArgument(format!(
            "--word-diff-regex is invalid: {err}"
        )));
    }
    Ok(())
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

async fn run_diff(args: &DiffArgs, structured_output: bool) -> Result<DiffOutput, DiffError> {
    util::require_repo().map_err(|_| DiffError::NotInRepo)?;
    tracing::debug!("diff args: {:?}", args);
    let index = Index::load(path::index()).map_err(|e| DiffError::IndexLoad(e.to_string()))?;

    let old_side = resolve_diff_side(&args.old, args.staged, false, &index).await?;
    let new_side = resolve_diff_side(&args.new, args.staged, true, &index).await?;

    let paths: Vec<PathBuf> = args.pathspec.iter().map(util::to_workdir_path).collect();

    // Resolve the unified-context radius (`-U` > `diff.context` > 3) and the
    // whitespace-comparison mode before choosing a diff backend.
    let context = resolve_diff_context(args).await?;
    let ws = WsMode::from_args(args);
    // `-W` needs whole-file access to find function boundaries, so it also forces
    // the libra-native generator.
    let use_native = ws != WsMode::None
        || args.ignore_blank_lines
        || args.function_context
        || context != DEFAULT_CONTEXT;

    // Snapshot per-side path→hash maps before the blobs are consumed by the diff
    // backend; rename/copy detection (below) needs them afterwards.
    let old_map: HashMap<PathBuf, ObjectHash> = old_side.blobs.iter().cloned().collect();
    let new_map: HashMap<PathBuf, ObjectHash> = new_side.blobs.iter().cloned().collect();

    let worktree_entries = new_side.worktree_entries.clone();
    let worktree_cache = RefCell::new(new_side.worktree_contents.clone());
    let repo_cache = RefCell::new(HashMap::<ObjectHash, Vec<u8>>::new());
    let load_error = Rc::new(RefCell::new(None::<DiffError>));
    let load_error_for_read = Rc::clone(&load_error);
    let read = move |path: &PathBuf, hash: &ObjectHash| -> Vec<u8> {
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
                    crate::utils::error::emit_warning(format!("{err}; skipping it in diff"));
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
    };

    // Default path (context 3, no whitespace handling) stays on git-internal's
    // `Diff::diff` so output is byte-identical to the established baseline; the
    // libra-native generator only runs when a Wave-0 feature is requested.
    let diff_output = if use_native {
        native_diff(
            &old_side.blobs,
            &new_side.blobs,
            &paths,
            context,
            ws,
            args.ignore_blank_lines,
            args.function_context,
            &read,
        )
    } else {
        Diff::diff(old_side.blobs, new_side.blobs, paths, &read)
    };
    if let Some(err) = load_error.borrow_mut().take() {
        return Err(err);
    }

    let mut files: Vec<DiffFileStat> = diff_output.iter().map(parse_diff_item).collect();
    add_mode_only_diffs(
        &mut files,
        &old_map,
        &new_map,
        &old_side.modes,
        &new_side.modes,
    );

    // Rename / copy detection (Wave 1): rewrites added/deleted pairs into
    // `renamed` / `copied` entries when content is similar enough.
    if let Some(opts) = resolve_rename_options(args).await {
        detect_renames_and_copies(
            &mut files,
            &old_map,
            &new_map,
            &old_side.modes,
            &new_side.modes,
            context,
            ws,
            args.ignore_blank_lines,
            args.function_context,
            &opts,
            &read,
        );
        if let Some(err) = load_error.borrow_mut().take() {
            return Err(err);
        }
    }

    // Populate full sha/mode for plain entries (renames/copies already carry
    // them) so `--raw` has complete per-file metadata.
    enrich_file_metadata(
        &mut files,
        &old_map,
        &new_map,
        &old_side.modes,
        &new_side.modes,
    );

    // `--relative` filters the diff to a subdirectory (before counts) and strips
    // that prefix from every displayed path; `diff.noPrefix` drops `a/`/`b/`.
    let relative_base = resolve_relative_base(args)?;
    let no_prefix = read_no_prefix_config().await;
    apply_path_display_transforms(&mut files, relative_base.as_deref(), no_prefix);

    // `--word-diff` rewrites unified hunk bodies into inline `[-del-]`/`{+add+}`
    // (or ANSI-color) markers; it is a submode of the unified format.
    if !structured_output
        && args.word_diff.is_some()
        && matches!(select_output_kind(args), OutputKind::Unified)
    {
        apply_word_diff(&mut files, args).await;
    }

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
    worktree_contents: HashMap<ObjectHash, Vec<u8>>,
    /// File mode (octal `u32`) per path on this side, used for rename
    /// mode-change headers and `--raw`.
    modes: HashMap<PathBuf, u32>,
}

/// Convert a git tree item mode to the octal `u32` representation Git writes in
/// diff headers (`100644`, `100755`, `120000`, …).
fn tree_item_mode_to_u32(mode: TreeItemMode) -> u32 {
    match mode {
        TreeItemMode::Blob => 0o100644,
        TreeItemMode::BlobExecutable => 0o100755,
        TreeItemMode::Link => 0o120000,
        TreeItemMode::Tree => 0o040000,
        TreeItemMode::Commit => 0o160000,
    }
}

/// diff needs to print hashes even if the files have not been staged yet.
/// This helper maps workdir paths to blob ids while applying the shared ignore policy.
type BlobsAndModes = (
    Vec<(PathBuf, ObjectHash)>,
    HashMap<PathBuf, u32>,
    HashMap<ObjectHash, Vec<u8>>,
);

fn get_files_blobs(
    files: &[PathBuf],
    index: &Index,
    policy: IgnorePolicy,
) -> Result<BlobsAndModes, DiffError> {
    let mut blobs = Vec::new();
    let mut modes = HashMap::new();
    let mut contents = HashMap::new();
    for p in files {
        if ignore::should_ignore(p, policy, index) {
            continue;
        }
        let absolute = util::workdir_to_absolute(p);
        let mode = std::fs::symlink_metadata(&absolute)
            .map(|m| index_mode_from_metadata(&m))
            .unwrap_or(0o100644);
        let hash = if let Some(hash) = index_hash_if_worktree_stat_matches(p, index) {
            hash
        } else {
            match std::fs::read(&absolute) {
                Ok(data) => {
                    let hash = calculate_object_hash(ObjectType::Blob, &data);
                    contents.insert(hash, data);
                    hash
                }
                Err(error) => {
                    crate::utils::error::emit_warning(format!(
                        "failed to read file '{}': {}; skipping it in diff",
                        absolute.display(),
                        error
                    ));
                    let Some(entry) = index_entry_for_path(p, index) else {
                        continue;
                    };
                    modes.insert(p.to_owned(), entry.mode);
                    blobs.push((p.to_owned(), entry.hash));
                    continue;
                }
            }
        };
        modes.insert(p.to_owned(), mode);
        blobs.push((p.to_owned(), hash));
    }
    Ok((blobs, modes, contents))
}

fn index_entry_for_path<'a>(path: &Path, index: &'a Index) -> Option<&'a IndexEntry> {
    index.get(path.to_str()?, 0)
}

fn index_hash_if_worktree_stat_matches(path: &Path, index: &Index) -> Option<ObjectHash> {
    let entry = index_entry_for_path(path, index)?;
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
fn get_index_blobs(index: &Index, policy: IgnorePolicy) -> BlobsAndModes {
    let mut blobs = Vec::new();
    let mut modes = HashMap::new();
    for entry in index.tracked_entries(0).iter() {
        let path = PathBuf::from(&entry.name);
        if ignore::should_ignore(&path, policy, index) {
            continue;
        }
        modes.insert(path.clone(), entry.mode);
        blobs.push((path, entry.hash));
    }
    (blobs, modes, HashMap::new())
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
        let (blobs, modes, worktree_contents) = get_commit_blobs(&commit_hash).await?;
        return Ok(DiffSide {
            label: source.clone(),
            blobs,
            worktree_entries: HashMap::new(),
            worktree_contents,
            modes,
        });
    }

    if is_new {
        if staged {
            let (blobs, modes, worktree_contents) = get_index_blobs(index, IgnorePolicy::Respect);
            Ok(DiffSide {
                label: "index".to_string(),
                blobs,
                worktree_entries: HashMap::new(),
                worktree_contents,
                modes,
            })
        } else {
            let files = get_worktree_diff_files(index)?;
            let (blobs, modes, worktree_contents) =
                get_files_blobs(&files, index, IgnorePolicy::Respect)?;
            Ok(DiffSide {
                label: "working tree".to_string(),
                worktree_entries: blobs.iter().cloned().collect(),
                blobs,
                worktree_contents,
                modes,
            })
        }
    } else if staged {
        match Head::current_commit().await {
            Some(commit_hash) => {
                let (blobs, modes, worktree_contents) = get_commit_blobs(&commit_hash).await?;
                Ok(DiffSide {
                    label: "HEAD".to_string(),
                    blobs,
                    worktree_entries: HashMap::new(),
                    worktree_contents,
                    modes,
                })
            }
            None => Ok(DiffSide {
                label: "HEAD".to_string(),
                blobs: Vec::new(),
                worktree_entries: HashMap::new(),
                worktree_contents: HashMap::new(),
                modes: HashMap::new(),
            }),
        }
    } else {
        let (blobs, modes, worktree_contents) = get_index_blobs(index, IgnorePolicy::Respect);
        Ok(DiffSide {
            label: "index".to_string(),
            blobs,
            worktree_entries: HashMap::new(),
            worktree_contents,
            modes,
        })
    }
}

async fn get_commit_blobs(commit_hash: &ObjectHash) -> Result<BlobsAndModes, DiffError> {
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
    let mut blobs = Vec::new();
    let mut modes = HashMap::new();
    for (path, hash, mode) in tree.get_plain_items_with_mode() {
        modes.insert(path.clone(), tree_item_mode_to_u32(mode));
        blobs.push((path, hash));
    }
    Ok((blobs, modes, HashMap::new()))
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

/// Default unified-diff context radius, matching Git and git-internal.
const DEFAULT_CONTEXT: usize = 3;

/// Safety cap mirroring git-internal: pathological inputs are rendered as a
/// large-file marker rather than diffed line-by-line.
const NATIVE_MAX_DIFF_LINES: usize = 10_000;

/// Whitespace-comparison mode selected by `-b` / `-w`. It only affects how two
/// lines are judged equal; rendered hunk lines always use original content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WsMode {
    None,
    /// `-b`: ignore trailing whitespace and collapse internal whitespace runs.
    SpaceChange,
    /// `-w`: ignore all whitespace.
    AllSpace,
}

impl WsMode {
    fn from_args(args: &DiffArgs) -> Self {
        // `-w` is the stronger rule and wins when both are supplied (matches Git).
        if args.ignore_all_space {
            WsMode::AllSpace
        } else if args.ignore_space_change {
            WsMode::SpaceChange
        } else {
            WsMode::None
        }
    }
}

/// Resolve the unified-context radius: explicit `-U<n>` wins, then `diff.context`
/// config, then the built-in default of 3. A non-numeric `diff.context` is a
/// usage error (129), matching Git's `bad numeric config value`.
async fn resolve_diff_context(args: &DiffArgs) -> Result<usize, DiffError> {
    if let Some(n) = args.unified {
        return Ok(n);
    }
    match ConfigKv::get("diff.context").await {
        Ok(Some(entry)) => {
            entry
                .value
                .trim()
                .parse::<usize>()
                .map_err(|_| DiffError::InvalidConfig {
                    key: "diff.context".to_string(),
                    value: entry.value.clone(),
                })
        }
        _ => Ok(DEFAULT_CONTEXT),
    }
}

/// Exit status for `--exit-code` / `--quiet`: 1 when there are differences, else
/// 0. The diff body is still rendered (the caller decides whether to suppress
/// stdout for `--quiet`); this value is the command's semantic exit code, not an
/// error in the 9/128/129 family.
fn diff_exit_status(result: &DiffOutput, exit_code: bool, quiet: bool) -> i32 {
    if (exit_code || quiet) && result.files_changed > 0 {
        1
    } else {
        0
    }
}

/// Normalize a line for whitespace-insensitive comparison. The result is only a
/// comparison key; the original line text is what gets rendered.
fn normalize_line(line: &str, ws: WsMode) -> Cow<'_, str> {
    match ws {
        WsMode::None => Cow::Borrowed(line),
        WsMode::AllSpace => Cow::Owned(line.chars().filter(|c| !c.is_whitespace()).collect()),
        WsMode::SpaceChange => {
            let trimmed = line.trim_end();
            let mut result = String::with_capacity(trimmed.len());
            let mut last_was_space = false;
            for c in trimmed.chars() {
                if c.is_whitespace() {
                    if !last_was_space {
                        result.push(' ');
                        last_was_space = true;
                    }
                } else {
                    result.push(c);
                    last_was_space = false;
                }
            }
            Cow::Owned(result)
        }
    }
}

/// Split text into lines, keeping each line's trailing newline. This matches
/// `similar`'s line tokenizer so the no-whitespace path produces the same Myers
/// edit script (and therefore the same output) as git-internal's `Diff::diff`.
fn tokenize_lines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, &b) in text.as_bytes().iter().enumerate() {
        if b == b'\n' {
            out.push(&text[start..=i]);
            start = i + 1;
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// One physical diff line while a hunk is being assembled. Text is the original
/// (newline-trimmed) content; line numbers are 1-based.
#[derive(Debug, Clone, Copy)]
enum EditLine<'a> {
    Context(usize, usize, &'a str),
    Delete(usize, &'a str),
    Insert(usize, &'a str),
}

/// Compute a single file's unified-diff body with a configurable context radius,
/// whitespace-insensitive comparison and optional blank-line ignoring. Ported
/// from git-internal's streaming hunk algorithm so the no-whitespace, context-3
/// path is byte-identical; comparison uses normalized keys while rendering uses
/// the original lines.
fn compute_unified_diff_native(
    old_text: &str,
    new_text: &str,
    context: usize,
    ws: WsMode,
    ignore_blank_lines: bool,
) -> String {
    use similar::{Algorithm, ChangeTag, TextDiff};

    let old_tokens = tokenize_lines(old_text);
    let new_tokens = tokenize_lines(new_text);
    let old_render: Vec<&str> = old_tokens
        .iter()
        .map(|t| t.trim_end_matches(['\r', '\n']))
        .collect();
    let new_render: Vec<&str> = new_tokens
        .iter()
        .map(|t| t.trim_end_matches(['\r', '\n']))
        .collect();

    // Comparison keys: keep the raw token (with newline) when not ignoring
    // whitespace so the Myers result matches git-internal exactly; otherwise key
    // on the normalized, newline-trimmed line.
    let make_key = |token: &str, render: &str| -> String {
        match ws {
            WsMode::None => token.to_string(),
            _ => normalize_line(render, ws).into_owned(),
        }
    };
    let old_keys: Vec<String> = old_tokens
        .iter()
        .zip(&old_render)
        .map(|(t, r)| make_key(t, r))
        .collect();
    let new_keys: Vec<String> = new_tokens
        .iter()
        .zip(&new_render)
        .map(|(t, r)| make_key(t, r))
        .collect();
    let old_key_refs: Vec<&str> = old_keys.iter().map(String::as_str).collect();
    let new_key_refs: Vec<&str> = new_keys.iter().map(String::as_str).collect();

    let diff = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .diff_slices(&old_key_refs, &new_key_refs);

    let mut out = String::new();
    let mut prefix_ctx: VecDeque<EditLine> = VecDeque::with_capacity(context);
    let mut cur_hunk: Vec<EditLine> = Vec::new();
    let mut eq_run: Vec<EditLine> = Vec::new();
    let mut in_hunk = false;
    let mut last_old_seen = 0usize;
    let mut last_new_seen = 0usize;

    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Equal => {
                let oi = change.old_index().unwrap_or(0);
                let ni = change.new_index().unwrap_or(0);
                last_old_seen = oi + 1;
                last_new_seen = ni + 1;
                let entry = EditLine::Context(oi + 1, ni + 1, new_render[ni]);
                if in_hunk {
                    eq_run.push(entry);
                    if eq_run.len() > context * 2 {
                        flush_native_hunk(
                            &mut out,
                            &mut cur_hunk,
                            &mut eq_run,
                            &mut prefix_ctx,
                            context,
                            &mut last_old_seen,
                            &mut last_new_seen,
                            ignore_blank_lines,
                        );
                        in_hunk = false;
                    }
                } else if context > 0 {
                    if prefix_ctx.len() == context {
                        prefix_ctx.pop_front();
                    }
                    prefix_ctx.push_back(entry);
                }
            }
            ChangeTag::Delete => {
                let oi = change.old_index().unwrap_or(0);
                let entry = EditLine::Delete(oi + 1, old_render[oi]);
                if !in_hunk {
                    cur_hunk.extend(prefix_ctx.iter().copied());
                    prefix_ctx.clear();
                    in_hunk = true;
                }
                if !eq_run.is_empty() {
                    cur_hunk.append(&mut eq_run);
                }
                cur_hunk.push(entry);
            }
            ChangeTag::Insert => {
                let ni = change.new_index().unwrap_or(0);
                let entry = EditLine::Insert(ni + 1, new_render[ni]);
                if !in_hunk {
                    cur_hunk.extend(prefix_ctx.iter().copied());
                    prefix_ctx.clear();
                    in_hunk = true;
                }
                if !eq_run.is_empty() {
                    cur_hunk.append(&mut eq_run);
                }
                cur_hunk.push(entry);
            }
        }
    }

    if in_hunk {
        flush_native_hunk(
            &mut out,
            &mut cur_hunk,
            &mut eq_run,
            &mut prefix_ctx,
            context,
            &mut last_old_seen,
            &mut last_new_seen,
            ignore_blank_lines,
        );
    }

    out
}

/// Emit one assembled hunk, trimming trailing context to `context` lines and
/// (when `ignore_blank_lines`) dropping hunks whose every changed line is blank.
#[allow(clippy::too_many_arguments)]
fn flush_native_hunk<'a>(
    out: &mut String,
    cur_hunk: &mut Vec<EditLine<'a>>,
    eq_run: &mut Vec<EditLine<'a>>,
    prefix_ctx: &mut VecDeque<EditLine<'a>>,
    context: usize,
    last_old_seen: &mut usize,
    last_new_seen: &mut usize,
    ignore_blank_lines: bool,
) {
    let trail_to_take = eq_run.len().min(context);
    for entry in eq_run.iter().take(trail_to_take) {
        cur_hunk.push(*entry);
    }

    if ignore_blank_lines {
        let only_blank_changes = cur_hunk.iter().all(|e| match e {
            EditLine::Delete(_, txt) | EditLine::Insert(_, txt) => txt.trim().is_empty(),
            EditLine::Context(..) => true,
        });
        let has_change = cur_hunk
            .iter()
            .any(|e| matches!(e, EditLine::Delete(..) | EditLine::Insert(..)));
        if has_change && only_blank_changes {
            prefix_ctx.clear();
            if context > 0 {
                let keep_start = eq_run.len().saturating_sub(context);
                for entry in eq_run.iter().skip(keep_start) {
                    prefix_ctx.push_back(*entry);
                }
            }
            cur_hunk.clear();
            eq_run.clear();
            return;
        }
    }

    let mut old_first: Option<usize> = None;
    let mut old_count = 0usize;
    let mut new_first: Option<usize> = None;
    let mut new_count = 0usize;
    for e in cur_hunk.iter() {
        match *e {
            EditLine::Context(o, n, _) => {
                if old_first.is_none() {
                    old_first = Some(o);
                }
                old_count += 1;
                if new_first.is_none() {
                    new_first = Some(n);
                }
                new_count += 1;
            }
            EditLine::Delete(o, _) => {
                if old_first.is_none() {
                    old_first = Some(o);
                }
                old_count += 1;
            }
            EditLine::Insert(n, _) => {
                if new_first.is_none() {
                    new_first = Some(n);
                }
                new_count += 1;
            }
        }
    }

    if old_count == 0 && new_count == 0 {
        cur_hunk.clear();
        eq_run.clear();
        return;
    }

    let old_start = old_first.unwrap_or(*last_old_seen + 1);
    let new_start = new_first.unwrap_or(*last_new_seen + 1);
    let _ = writeln!(
        out,
        "@@ -{old_start},{old_count} +{new_start},{new_count} @@"
    );

    for &e in cur_hunk.iter() {
        match e {
            EditLine::Context(o, n, txt) => {
                let _ = writeln!(out, " {txt}");
                *last_old_seen = (*last_old_seen).max(o);
                *last_new_seen = (*last_new_seen).max(n);
            }
            EditLine::Delete(o, txt) => {
                let _ = writeln!(out, "-{txt}");
                *last_old_seen = (*last_old_seen).max(o);
            }
            EditLine::Insert(n, txt) => {
                let _ = writeln!(out, "+{txt}");
                *last_new_seen = (*last_new_seen).max(n);
            }
        }
    }

    prefix_ctx.clear();
    if context > 0 {
        let keep_start = eq_run.len().saturating_sub(context);
        for entry in eq_run.iter().skip(keep_start) {
            prefix_ctx.push_back(*entry);
        }
    }
    cur_hunk.clear();
    eq_run.clear();
}

// ----- Wave 2: -W / --function-context -----

/// Heuristic for a function/section header (Git's default funcname): a line
/// whose first byte is a letter or `_` — declarations start at column 0, bodies
/// are indented.
fn is_function_header(line: &str) -> bool {
    matches!(line.bytes().next(), Some(b) if b.is_ascii_alphabetic() || b == b'_')
}

/// Upper bound on how far `-W` expands a single block, so a missing header can't
/// pull in an unbounded amount of context.
const MAX_FUNCTION_CONTEXT: usize = 400;

/// `-W` body: expand every change's context to the enclosing function block
/// (nearest preceding header line through just before the next header), merging
/// overlapping blocks.
fn compute_function_context_diff(old_text: &str, new_text: &str, ws: WsMode) -> String {
    use similar::{Algorithm, ChangeTag, TextDiff};

    let old_tokens = tokenize_lines(old_text);
    let new_tokens = tokenize_lines(new_text);
    let old_render: Vec<&str> = old_tokens
        .iter()
        .map(|t| t.trim_end_matches(['\r', '\n']))
        .collect();
    let new_render: Vec<&str> = new_tokens
        .iter()
        .map(|t| t.trim_end_matches(['\r', '\n']))
        .collect();
    let make_key = |token: &str, render: &str| -> String {
        match ws {
            WsMode::None => token.to_string(),
            _ => normalize_line(render, ws).into_owned(),
        }
    };
    let old_keys: Vec<String> = old_tokens
        .iter()
        .zip(&old_render)
        .map(|(t, r)| make_key(t, r))
        .collect();
    let new_keys: Vec<String> = new_tokens
        .iter()
        .zip(&new_render)
        .map(|(t, r)| make_key(t, r))
        .collect();
    let old_key_refs: Vec<&str> = old_keys.iter().map(String::as_str).collect();
    let new_key_refs: Vec<&str> = new_keys.iter().map(String::as_str).collect();
    let diff = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .diff_slices(&old_key_refs, &new_key_refs);

    let mut entries: Vec<EditLine> = Vec::new();
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Equal => {
                let oi = change.old_index().unwrap_or(0);
                let ni = change.new_index().unwrap_or(0);
                entries.push(EditLine::Context(oi + 1, ni + 1, new_render[ni]));
            }
            ChangeTag::Delete => {
                let oi = change.old_index().unwrap_or(0);
                entries.push(EditLine::Delete(oi + 1, old_render[oi]));
            }
            ChangeTag::Insert => {
                let ni = change.new_index().unwrap_or(0);
                entries.push(EditLine::Insert(ni + 1, new_render[ni]));
            }
        }
    }
    if entries.is_empty() {
        return String::new();
    }

    let n = entries.len();
    let is_change = |e: &EditLine| matches!(e, EditLine::Delete(..) | EditLine::Insert(..));
    let header_at =
        |idx: usize| matches!(entries[idx], EditLine::Context(_, _, t) if is_function_header(t));

    let mut blocks: Vec<(usize, usize)> = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        if !is_change(entry) {
            continue;
        }
        let mut start = i;
        let mut steps = 0;
        while start > 0 {
            if header_at(start) {
                break;
            }
            start -= 1;
            steps += 1;
            if steps >= MAX_FUNCTION_CONTEXT {
                break;
            }
        }
        let mut end = i + 1;
        let mut steps = 0;
        while end < n {
            if header_at(end) {
                break;
            }
            end += 1;
            steps += 1;
            if steps >= MAX_FUNCTION_CONTEXT {
                break;
            }
        }
        blocks.push((start, end));
    }

    blocks.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in blocks {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }

    let mut out = String::new();
    for (start, end) in merged {
        write_hunk_from_entries(&mut out, &entries[start..end]);
    }
    out
}

/// Write one hunk (header + lines) for a contiguous slice of edit entries.
fn write_hunk_from_entries(out: &mut String, hunk: &[EditLine]) {
    let mut old_first = None;
    let mut old_count = 0usize;
    let mut new_first = None;
    let mut new_count = 0usize;
    for entry in hunk {
        match *entry {
            EditLine::Context(o, n, _) => {
                old_first.get_or_insert(o);
                old_count += 1;
                new_first.get_or_insert(n);
                new_count += 1;
            }
            EditLine::Delete(o, _) => {
                old_first.get_or_insert(o);
                old_count += 1;
            }
            EditLine::Insert(n, _) => {
                new_first.get_or_insert(n);
                new_count += 1;
            }
        }
    }
    if old_count == 0 && new_count == 0 {
        return;
    }
    let old_start = old_first.unwrap_or(1);
    let new_start = new_first.unwrap_or(1);
    let _ = writeln!(
        out,
        "@@ -{old_start},{old_count} +{new_start},{new_count} @@"
    );
    for entry in hunk {
        match *entry {
            EditLine::Context(_, _, txt) => {
                let _ = writeln!(out, " {txt}");
            }
            EditLine::Delete(_, txt) => {
                let _ = writeln!(out, "-{txt}");
            }
            EditLine::Insert(_, txt) => {
                let _ = writeln!(out, "+{txt}");
            }
        }
    }
}

/// Shorten an object hash to 7 hex chars (or 7 zeros when absent), matching the
/// `index` line format git-internal writes.
fn short_hash_native(hash: Option<&ObjectHash>) -> String {
    hash.map(|h| {
        let hex = h.to_string();
        let take = 7.min(hex.len());
        hex[..take].to_string()
    })
    .unwrap_or_else(|| "0".repeat(7))
}

/// Render one file's full unified diff (headers + body) using the native
/// generator, mirroring git-internal's `diff_for_file_preloaded` layout.
#[allow(clippy::too_many_arguments)]
fn native_diff_for_file(
    file: &Path,
    old_hash: Option<&ObjectHash>,
    new_hash: Option<&ObjectHash>,
    old_bytes: &[u8],
    new_bytes: &[u8],
    context: usize,
    ws: WsMode,
    ignore_blank_lines: bool,
    function_context: bool,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "diff --git a/{} b/{}", file.display(), file.display());
    if old_hash.is_none() {
        let _ = writeln!(out, "new file mode 100644");
    } else if new_hash.is_none() {
        let _ = writeln!(out, "deleted file mode 100644");
    }
    let _ = writeln!(
        out,
        "index {}..{} 100644",
        short_hash_native(old_hash),
        short_hash_native(new_hash)
    );

    match text_pair(old_bytes, new_bytes) {
        Some((old_text, new_text)) => {
            let (old_pref, new_pref) = if old_hash.is_none() {
                ("/dev/null".to_string(), format!("b/{}", file.display()))
            } else if new_hash.is_none() {
                (format!("a/{}", file.display()), "/dev/null".to_string())
            } else {
                (
                    format!("a/{}", file.display()),
                    format!("b/{}", file.display()),
                )
            };
            let _ = writeln!(out, "--- {old_pref}");
            let _ = writeln!(out, "+++ {new_pref}");
            let body = if function_context {
                compute_function_context_diff(old_text, new_text, ws)
            } else {
                compute_unified_diff_native(old_text, new_text, context, ws, ignore_blank_lines)
            };
            out.push_str(&body);
        }
        None => {
            let _ = writeln!(out, "Binary files differ");
        }
    }
    out
}

/// Treat content as binary when either side contains a NUL byte in its first
/// 8 KiB (matching Git) or is not valid UTF-8; otherwise return the two texts.
fn text_pair<'a>(old_bytes: &'a [u8], new_bytes: &'a [u8]) -> Option<(&'a str, &'a str)> {
    if is_binary_content(old_bytes) || is_binary_content(new_bytes) {
        return None;
    }
    match (
        std::str::from_utf8(old_bytes),
        std::str::from_utf8(new_bytes),
    ) {
        (Ok(old_text), Ok(new_text)) => Some((old_text, new_text)),
        _ => None,
    }
}

fn is_binary_content(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|&b| b == 0)
}

/// libra-native counterpart to `Diff::diff`: pairs old/new blobs by path, applies
/// the pathspec filter, and renders each differing file with a configurable
/// context radius and whitespace handling. Files whose only differences are
/// normalized away (whitespace/blank-line) produce no hunk and are dropped from
/// the result so counts and `--exit-code` stay accurate.
#[allow(clippy::too_many_arguments)]
fn native_diff(
    old_blobs: &[(PathBuf, ObjectHash)],
    new_blobs: &[(PathBuf, ObjectHash)],
    filter: &[PathBuf],
    context: usize,
    ws: WsMode,
    ignore_blank_lines: bool,
    function_context: bool,
    read_content: &dyn Fn(&PathBuf, &ObjectHash) -> Vec<u8>,
) -> Vec<git_internal::diff::DiffItem> {
    let old_map: HashMap<&PathBuf, &ObjectHash> = old_blobs.iter().map(|(p, h)| (p, h)).collect();
    let new_map: HashMap<&PathBuf, &ObjectHash> = new_blobs.iter().map(|(p, h)| (p, h)).collect();

    let mut files: Vec<&PathBuf> = old_map.keys().chain(new_map.keys()).copied().collect();
    files.sort();
    files.dedup();

    let mut results = Vec::new();
    for file in files {
        if !filter.is_empty() && !filter.iter().any(|p| path_is_sub_of(file, p)) {
            continue;
        }
        let old_hash = old_map.get(file).copied();
        let new_hash = new_map.get(file).copied();
        if old_hash == new_hash {
            continue;
        }

        let old_bytes = old_hash.map_or_else(Vec::new, |h| read_content(file, h));
        let new_bytes = new_hash.map_or_else(Vec::new, |h| read_content(file, h));

        let old_lines = String::from_utf8_lossy(&old_bytes).lines().count();
        let new_lines = String::from_utf8_lossy(&new_bytes).lines().count();
        if old_lines + new_lines > NATIVE_MAX_DIFF_LINES {
            results.push(git_internal::diff::DiffItem {
                path: file.to_string_lossy().to_string(),
                data: format!(
                    "<LargeFile>{}:{}:{}</LargeFile>\n",
                    file.display(),
                    old_lines + new_lines,
                    NATIVE_MAX_DIFF_LINES
                ),
            });
            continue;
        }

        let data = native_diff_for_file(
            file,
            old_hash,
            new_hash,
            &old_bytes,
            &new_bytes,
            context,
            ws,
            ignore_blank_lines,
            function_context,
        );

        // A pure modify whose differences were all normalized away (whitespace
        // or blank-line) yields no hunk; drop it so it does not count as changed.
        let has_hunk = data.contains("@@ ");
        let is_add_or_delete = old_hash.is_none() || new_hash.is_none();
        let is_binary = data.contains("Binary files differ");
        if !has_hunk && !is_add_or_delete && !is_binary {
            continue;
        }
        results.push(git_internal::diff::DiffItem {
            path: file.to_string_lossy().to_string(),
            data,
        });
    }
    results
}

/// Whether `path` lies under `parent` once both are absolutized, matching
/// git-internal's pathspec containment check.
fn path_is_sub_of(path: &Path, parent: &Path) -> bool {
    use path_absolutize::Absolutize;
    match (path.absolutize(), parent.absolutize()) {
        (Ok(p), Ok(par)) => p.starts_with(par),
        _ => false,
    }
}

// ----- Rename / copy detection (Wave 1) -----

/// Default rename/copy similarity threshold (50%), matching Git.
const DEFAULT_RENAME_THRESHOLD: f64 = 0.5;
/// Default `diff.renameLimit` (deleted × added product cap), matching Git.
const DEFAULT_RENAME_LIMIT: usize = 1000;

/// `diff.renames` config value (not a plain bool — Git accepts copy/copies).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RenamesConfig {
    Off,
    Renames,
    Copies,
}

/// Resolved rename/copy detection parameters for a single diff run.
struct RenameOptions {
    rename_threshold: f64,
    copy_threshold: Option<f64>,
    rename_limit: usize,
}

/// A matched rename/copy: which source entry, the similarity, and whether it is
/// a copy (source preserved) rather than a rename (source consumed).
struct RenameMatch {
    source_index: usize,
    similarity: f64,
    is_copy: bool,
}

/// Parse a similarity flag value: `-M`/absent value → default 50%; `80`/`80%`
/// → 0.80; a dotted value like `0.8` (no `%`) is treated as an already-fractional
/// threshold. Unparseable values fall back to the default.
fn parse_similarity(flag: &Option<String>) -> f64 {
    match flag {
        None => DEFAULT_RENAME_THRESHOLD,
        Some(s) => {
            let trimmed = s.trim();
            let had_percent = trimmed.ends_with('%');
            let core = trimmed.strip_suffix('%').unwrap_or(trimmed);
            match core.parse::<f64>() {
                Ok(n) if core.contains('.') && !had_percent => n.clamp(0.0, 1.0),
                Ok(n) => (n / 100.0).clamp(0.0, 1.0),
                Err(_) => DEFAULT_RENAME_THRESHOLD,
            }
        }
    }
}

async fn read_renames_config() -> RenamesConfig {
    match ConfigKv::get("diff.renames").await {
        Ok(Some(entry)) => match entry.value.trim().to_ascii_lowercase().as_str() {
            "false" | "no" | "off" | "0" => RenamesConfig::Off,
            "copy" | "copies" => RenamesConfig::Copies,
            "true" | "yes" | "on" | "1" => RenamesConfig::Renames,
            // Unrecognized values fall back to rename detection, matching Git.
            _ => RenamesConfig::Renames,
        },
        // Absent config keeps detection off so default `libra diff` is unchanged.
        _ => RenamesConfig::Off,
    }
}

async fn read_rename_limit() -> usize {
    match ConfigKv::get("diff.renameLimit").await {
        Ok(Some(entry)) => entry
            .value
            .trim()
            .parse::<usize>()
            .ok()
            .filter(|&n| n > 0)
            .unwrap_or(DEFAULT_RENAME_LIMIT),
        _ => DEFAULT_RENAME_LIMIT,
    }
}

/// Resolve whether rename/copy detection runs, and with what thresholds. Returns
/// `None` when detection is disabled (`--no-renames`, or no flag and config off).
async fn resolve_rename_options(args: &DiffArgs) -> Option<RenameOptions> {
    if args.no_renames {
        return None;
    }
    let config = read_renames_config().await;
    let renames_on = args.find_renames.is_some()
        || args.find_copies.is_some()
        || matches!(config, RenamesConfig::Renames | RenamesConfig::Copies);
    if !renames_on {
        return None;
    }

    let rename_threshold = args
        .find_renames
        .as_ref()
        .map(parse_similarity)
        .unwrap_or(DEFAULT_RENAME_THRESHOLD);
    let copy_threshold = if let Some(flag) = &args.find_copies {
        Some(parse_similarity(flag))
    } else if matches!(config, RenamesConfig::Copies) {
        Some(DEFAULT_RENAME_THRESHOLD)
    } else {
        None
    };

    Some(RenameOptions {
        rename_threshold,
        copy_threshold,
        rename_limit: read_rename_limit().await,
    })
}

/// Rewrite `files` in place, turning added/deleted pairs into `renamed` entries
/// (and added/source pairs into `copied` entries) when content is similar enough.
#[allow(clippy::too_many_arguments)]
fn detect_renames_and_copies(
    files: &mut Vec<DiffFileStat>,
    old_map: &HashMap<PathBuf, ObjectHash>,
    new_map: &HashMap<PathBuf, ObjectHash>,
    old_modes: &HashMap<PathBuf, u32>,
    new_modes: &HashMap<PathBuf, u32>,
    context: usize,
    ws: WsMode,
    ignore_blank_lines: bool,
    function_context: bool,
    opts: &RenameOptions,
    read: &dyn Fn(&PathBuf, &ObjectHash) -> Vec<u8>,
) {
    let added: Vec<usize> = files
        .iter()
        .enumerate()
        .filter(|(_, f)| f.status == "added")
        .map(|(i, _)| i)
        .collect();
    let deleted: Vec<usize> = files
        .iter()
        .enumerate()
        .filter(|(_, f)| f.status == "deleted")
        .map(|(i, _)| i)
        .collect();
    if added.is_empty() {
        return;
    }

    let old_hash_of = |idx: usize| -> Option<ObjectHash> {
        old_map.get(&PathBuf::from(&files[idx].path)).copied()
    };
    let new_hash_of = |idx: usize| -> Option<ObjectHash> {
        new_map.get(&PathBuf::from(&files[idx].path)).copied()
    };
    let old_content = |idx: usize| -> Vec<u8> {
        let path = PathBuf::from(&files[idx].path);
        match old_map.get(&path) {
            Some(h) => read(&path, h),
            None => Vec::new(),
        }
    };
    let new_content = |idx: usize| -> Vec<u8> {
        let path = PathBuf::from(&files[idx].path);
        match new_map.get(&path) {
            Some(h) => read(&path, h),
            None => Vec::new(),
        }
    };

    let mut matches: HashMap<usize, RenameMatch> = HashMap::new();
    let mut consumed_deleted: HashSet<usize> = HashSet::new();

    // Stage 1: exact-hash renames (O(N), no similarity computation).
    for &ai in &added {
        let Some(a_hash) = new_hash_of(ai) else {
            continue;
        };
        for &di in &deleted {
            if consumed_deleted.contains(&di) {
                continue;
            }
            if old_hash_of(di) == Some(a_hash) {
                consumed_deleted.insert(di);
                matches.insert(
                    ai,
                    RenameMatch {
                        source_index: di,
                        similarity: 1.0,
                        is_copy: false,
                    },
                );
                break;
            }
        }
    }

    // Stage 2: similarity renames, bounded by `diff.renameLimit`.
    let unmatched_added: Vec<usize> = added
        .iter()
        .copied()
        .filter(|ai| !matches.contains_key(ai))
        .collect();
    let unconsumed_deleted: Vec<usize> = deleted
        .iter()
        .copied()
        .filter(|di| !consumed_deleted.contains(di))
        .collect();
    if !unmatched_added.is_empty() && !unconsumed_deleted.is_empty() {
        if unmatched_added
            .len()
            .saturating_mul(unconsumed_deleted.len())
            > opts.rename_limit
        {
            crate::utils::error::emit_warning(format!(
                "inexact rename detection was skipped due to too many files (limit {}); set diff.renameLimit higher to enable it",
                opts.rename_limit
            ));
        } else {
            for &ai in &unmatched_added {
                let a_content = new_content(ai);
                let mut best: Option<(usize, f64)> = None;
                for &di in &unconsumed_deleted {
                    if consumed_deleted.contains(&di) {
                        continue;
                    }
                    let sim = crate::utils::blob_similarity::blob_line_similarity(
                        &old_content(di),
                        &a_content,
                    );
                    if sim >= opts.rename_threshold
                        && best.is_none_or(|(_, best_sim)| sim > best_sim)
                    {
                        best = Some((di, sim));
                    }
                }
                if let Some((di, sim)) = best {
                    consumed_deleted.insert(di);
                    matches.insert(
                        ai,
                        RenameMatch {
                            source_index: di,
                            similarity: sim,
                            is_copy: false,
                        },
                    );
                }
            }
        }
    }

    // Stage 3: copy detection — copy source pool = modified/deleted old-side files.
    // A delete already consumed as a rename source is still a valid copy source
    // (its old-side content existed and may be copied), so `consumed_deleted` is
    // intentionally not excluded here — only the copy match itself is
    // non-consuming.
    if let Some(copy_threshold) = opts.copy_threshold {
        let sources: Vec<usize> = files
            .iter()
            .enumerate()
            .filter(|(_, f)| f.status == "modified" || f.status == "deleted")
            .map(|(i, _)| i)
            .collect();
        let still_unmatched: Vec<usize> = added
            .iter()
            .copied()
            .filter(|ai| !matches.contains_key(ai))
            .collect();
        if !still_unmatched.is_empty()
            && !sources.is_empty()
            && still_unmatched.len().saturating_mul(sources.len()) <= opts.rename_limit
        {
            for &ai in &still_unmatched {
                let a_content = new_content(ai);
                let mut best: Option<(usize, f64)> = None;
                for &si in &sources {
                    let sim = crate::utils::blob_similarity::blob_line_similarity(
                        &old_content(si),
                        &a_content,
                    );
                    if sim >= copy_threshold && best.is_none_or(|(_, best_sim)| sim > best_sim) {
                        best = Some((si, sim));
                    }
                }
                if let Some((si, sim)) = best {
                    matches.insert(
                        ai,
                        RenameMatch {
                            source_index: si,
                            similarity: sim,
                            is_copy: true,
                        },
                    );
                }
            }
        }
    }

    if matches.is_empty() {
        return;
    }

    // Rebuild the file list: drop consumed deletes, rewrite matched adds.
    let mut rebuilt: Vec<DiffFileStat> = Vec::with_capacity(files.len());
    for (i, file) in files.iter().enumerate() {
        if consumed_deleted.contains(&i) {
            continue;
        }
        if let Some(m) = matches.get(&i) {
            let source_path = files[m.source_index].path.clone();
            rebuilt.push(build_rename_entry(
                file.path.clone(),
                source_path,
                m,
                old_map,
                new_map,
                old_modes,
                new_modes,
                context,
                ws,
                ignore_blank_lines,
                function_context,
                read,
            ));
        } else {
            rebuilt.push(file.clone());
        }
    }
    *files = rebuilt;
}

/// Render a single `renamed` / `copied` entry, including the similarity header,
/// optional mode-change lines, and a content diff body when the blobs differ.
#[allow(clippy::too_many_arguments)]
fn build_rename_entry(
    new_path: String,
    old_path: String,
    m: &RenameMatch,
    old_map: &HashMap<PathBuf, ObjectHash>,
    new_map: &HashMap<PathBuf, ObjectHash>,
    old_modes: &HashMap<PathBuf, u32>,
    new_modes: &HashMap<PathBuf, u32>,
    context: usize,
    ws: WsMode,
    ignore_blank_lines: bool,
    function_context: bool,
    read: &dyn Fn(&PathBuf, &ObjectHash) -> Vec<u8>,
) -> DiffFileStat {
    let old_pb = PathBuf::from(&old_path);
    let new_pb = PathBuf::from(&new_path);
    let old_hash = old_map.get(&old_pb).copied();
    let new_hash = new_map.get(&new_pb).copied();
    let old_mode = old_modes.get(&old_pb).copied();
    let new_mode = new_modes.get(&new_pb).copied();
    let sim_pct = (m.similarity * 100.0).round() as u32;
    let verb = if m.is_copy { "copy" } else { "rename" };

    let mut raw = String::new();
    let _ = writeln!(raw, "diff --git a/{old_path} b/{new_path}");
    if let (Some(om), Some(nm)) = (old_mode, new_mode)
        && om != nm
    {
        let _ = writeln!(raw, "old mode {om:o}");
        let _ = writeln!(raw, "new mode {nm:o}");
    }
    let _ = writeln!(raw, "similarity index {sim_pct}%");
    let _ = writeln!(raw, "{verb} from {old_path}");
    let _ = writeln!(raw, "{verb} to {new_path}");

    let mut hunks = Vec::new();
    let mut insertions = 0;
    let mut deletions = 0;

    if old_hash != new_hash {
        let old_bytes = old_hash.map(|h| read(&old_pb, &h)).unwrap_or_default();
        let new_bytes = new_hash.map(|h| read(&new_pb, &h)).unwrap_or_default();
        match text_pair(&old_bytes, &new_bytes) {
            Some((old_text, new_text)) => {
                let body = if function_context {
                    compute_function_context_diff(old_text, new_text, ws)
                } else {
                    compute_unified_diff_native(old_text, new_text, context, ws, ignore_blank_lines)
                };
                if !body.is_empty() {
                    let _ = writeln!(
                        raw,
                        "index {}..{} 100644",
                        short_hash_native(old_hash.as_ref()),
                        short_hash_native(new_hash.as_ref())
                    );
                    let _ = writeln!(raw, "--- a/{old_path}");
                    let _ = writeln!(raw, "+++ b/{new_path}");
                    raw.push_str(&body);
                    hunks = parse_diff_hunks(&body);
                    let counts = count_hunk_line_changes(&body);
                    insertions = counts.0;
                    deletions = counts.1;
                }
            }
            None => {
                let _ = writeln!(
                    raw,
                    "index {}..{} 100644",
                    short_hash_native(old_hash.as_ref()),
                    short_hash_native(new_hash.as_ref())
                );
                let _ = writeln!(raw, "Binary files differ");
            }
        }
    }

    DiffFileStat {
        path: new_path,
        status: if m.is_copy { "copied" } else { "renamed" }.to_string(),
        insertions,
        deletions,
        hunks,
        old_path: Some(old_path),
        similarity: Some(sim_pct),
        old_mode,
        new_mode,
        old_sha: old_hash.map(|h| h.to_string()),
        new_sha: new_hash.map(|h| h.to_string()),
        raw_diff: raw,
    }
}

// ----- Wave 2: --raw metadata, --relative, diff.noPrefix -----

/// Fill `old_sha`/`new_sha`/`old_mode`/`new_mode` for plain entries (renames and
/// copies already carry them) so `--raw` has full per-file metadata.
fn enrich_file_metadata(
    files: &mut [DiffFileStat],
    old_map: &HashMap<PathBuf, ObjectHash>,
    new_map: &HashMap<PathBuf, ObjectHash>,
    old_modes: &HashMap<PathBuf, u32>,
    new_modes: &HashMap<PathBuf, u32>,
) {
    for file in files.iter_mut() {
        if file.old_sha.is_some()
            || file.new_sha.is_some()
            || file.old_mode.is_some()
            || file.new_mode.is_some()
        {
            continue;
        }
        let path = PathBuf::from(&file.path);
        file.old_sha = old_map.get(&path).map(|h| h.to_string());
        file.new_sha = new_map.get(&path).map(|h| h.to_string());
        file.old_mode = old_modes.get(&path).copied();
        file.new_mode = new_modes.get(&path).copied();
    }
}

fn add_mode_only_diffs(
    files: &mut Vec<DiffFileStat>,
    old_map: &HashMap<PathBuf, ObjectHash>,
    new_map: &HashMap<PathBuf, ObjectHash>,
    old_modes: &HashMap<PathBuf, u32>,
    new_modes: &HashMap<PathBuf, u32>,
) {
    let changed_paths: HashSet<PathBuf> =
        files.iter().map(|file| PathBuf::from(&file.path)).collect();

    for (path, old_hash) in old_map {
        if changed_paths.contains(path) {
            continue;
        }
        let Some(new_hash) = new_map.get(path) else {
            continue;
        };
        if old_hash != new_hash {
            continue;
        }
        let (Some(old_mode), Some(new_mode)) = (old_modes.get(path), new_modes.get(path)) else {
            continue;
        };
        if old_mode == new_mode {
            continue;
        }

        let display_path = path.to_string_lossy().to_string();
        let mut raw = String::new();
        let _ = writeln!(raw, "diff --git a/{display_path} b/{display_path}");
        let _ = writeln!(raw, "old mode {old_mode:o}");
        let _ = writeln!(raw, "new mode {new_mode:o}");
        files.push(DiffFileStat {
            path: display_path,
            status: "typechange".to_string(),
            insertions: 0,
            deletions: 0,
            hunks: Vec::new(),
            old_path: None,
            similarity: None,
            old_mode: Some(*old_mode),
            new_mode: Some(*new_mode),
            old_sha: Some(old_hash.to_string()),
            new_sha: Some(new_hash.to_string()),
            raw_diff: raw,
        });
    }
}

/// Resolve `--relative` to a repo-root-relative directory (empty string for the
/// repo root), or `None` when the flag is absent. A path escaping the repository
/// is a usage error (129).
fn resolve_relative_base(args: &DiffArgs) -> Result<Option<String>, DiffError> {
    let raw = match &args.relative {
        None => return Ok(None),
        Some(None) => ".".to_string(),
        Some(Some(p)) => p.clone(),
    };
    let workdir_rel = util::to_workdir_path(&raw);
    if workdir_rel
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(DiffError::InvalidArgument(format!(
            "--relative path '{raw}' is outside the repository"
        )));
    }
    let base = workdir_rel.to_string_lossy().trim_matches('/').to_string();
    Ok(Some(if base == "." { String::new() } else { base }))
}

async fn read_no_prefix_config() -> bool {
    matches!(
        ConfigKv::get("diff.noPrefix").await,
        Ok(Some(entry)) if matches!(
            entry.value.trim().to_ascii_lowercase().as_str(),
            "true" | "yes" | "on" | "1"
        )
    )
}

/// Apply `--relative` (filter to a subtree, then strip its prefix from every
/// displayed path) and `diff.noPrefix` (drop `a/`/`b/`) across file fields and
/// the unified-diff header lines.
fn apply_path_display_transforms(
    files: &mut Vec<DiffFileStat>,
    relative_base: Option<&str>,
    no_prefix: bool,
) {
    let active_base = relative_base.filter(|b| !b.is_empty());

    if let Some(base) = active_base {
        let prefix = format!("{base}/");
        files.retain(|f| f.path == base || f.path.starts_with(&prefix));
    }

    if active_base.is_none() && !no_prefix {
        return;
    }

    for file in files.iter_mut() {
        if let Some(base) = active_base {
            let prefix = format!("{base}/");
            if let Some(rest) = file.path.strip_prefix(&prefix) {
                file.path = rest.to_string();
            }
            if let Some(old) = &file.old_path
                && let Some(rest) = old.strip_prefix(&prefix)
            {
                file.old_path = Some(rest.to_string());
            }
        }
        file.raw_diff = transform_diff_header_paths(&file.raw_diff, active_base, no_prefix);
    }
}

/// Rewrite the path tokens in a unified diff's header lines (everything before
/// the first `@@`), leaving the hunk body untouched.
fn transform_diff_header_paths(
    raw_diff: &str,
    relative_base: Option<&str>,
    no_prefix: bool,
) -> String {
    let trailing_nl = raw_diff.ends_with('\n');
    let mut in_body = false;
    let mut lines: Vec<String> = Vec::new();
    for line in raw_diff.lines() {
        if line.starts_with("@@ ") {
            in_body = true;
        }
        if in_body {
            lines.push(line.to_string());
        } else {
            lines.push(transform_header_line(line, relative_base, no_prefix));
        }
    }
    let mut out = lines.join("\n");
    if trailing_nl {
        out.push('\n');
    }
    out
}

fn transform_header_line(line: &str, relative_base: Option<&str>, no_prefix: bool) -> String {
    if let Some(rest) = line.strip_prefix("diff --git ") {
        if let Some(idx) = rest.find(" b/") {
            let left = transform_path_token(&rest[..idx], relative_base, no_prefix);
            let right = transform_path_token(&rest[idx + 1..], relative_base, no_prefix);
            return format!("diff --git {left} {right}");
        }
        return line.to_string();
    }
    for kw in ["--- ", "+++ "] {
        if let Some(rest) = line.strip_prefix(kw) {
            return format!(
                "{kw}{}",
                transform_path_token(rest, relative_base, no_prefix)
            );
        }
    }
    for kw in ["rename from ", "rename to ", "copy from ", "copy to "] {
        if let Some(rest) = line.strip_prefix(kw) {
            return format!(
                "{kw}{}",
                transform_path_token(rest, relative_base, no_prefix)
            );
        }
    }
    line.to_string()
}

/// Transform a single path token (`a/src/x`, `b/src/x`, `src/x`, or `/dev/null`)
/// by stripping the `--relative` base and/or the `a/`/`b/` prefix.
fn transform_path_token(token: &str, relative_base: Option<&str>, no_prefix: bool) -> String {
    if token == "/dev/null" {
        return token.to_string();
    }
    let (side, inner) = if let Some(rest) = token.strip_prefix("a/") {
        ("a/", rest)
    } else if let Some(rest) = token.strip_prefix("b/") {
        ("b/", rest)
    } else {
        ("", token)
    };
    let stripped = match relative_base {
        Some(base) if !base.is_empty() => inner.strip_prefix(&format!("{base}/")).unwrap_or(inner),
        _ => inner,
    };
    if no_prefix {
        stripped.to_string()
    } else {
        format!("{side}{stripped}")
    }
}

// ----- Wave 2: --word-diff -----

/// Default word boundary: a run of word chars, a run of whitespace, or a single
/// other character (matches Git's default and covers the whole string).
const DEFAULT_WORD_REGEX: &str = r"\w+|\s+|[^\w\s]";
/// Files larger than this skip word tokenization and keep the line diff.
const WORD_DIFF_MAX_BYTES: usize = 10 * 1024 * 1024;

/// Rewrite each file's unified hunk body into a word-level diff. Files above
/// [`WORD_DIFF_MAX_BYTES`] fall back to the line diff with a warning.
async fn apply_word_diff(files: &mut [DiffFileStat], args: &DiffArgs) {
    let color = matches!(
        args.word_diff.as_ref().and_then(|m| m.as_deref()),
        Some("color")
    );
    let pattern = resolve_word_regex(args).await;
    let re = match regex::Regex::new(&pattern).or_else(|_| regex::Regex::new(DEFAULT_WORD_REGEX)) {
        Ok(re) => re,
        // Even the default failed to compile (not expected); leave the line diff.
        Err(_) => return,
    };

    for file in files.iter_mut() {
        if file.raw_diff.len() > WORD_DIFF_MAX_BYTES {
            crate::utils::error::emit_warning(format!(
                "word-diff skipped for '{}' (larger than {WORD_DIFF_MAX_BYTES} bytes); showing line diff",
                file.path
            ));
            continue;
        }
        file.raw_diff = apply_word_diff_to_raw(&file.raw_diff, &re, color);
    }
}

async fn resolve_word_regex(args: &DiffArgs) -> String {
    if let Some(re) = &args.word_diff_regex {
        return re.clone();
    }
    if let Ok(Some(entry)) = ConfigKv::get("diff.wordRegex").await {
        let value = entry.value.trim();
        if !value.is_empty() {
            return value.to_string();
        }
    }
    DEFAULT_WORD_REGEX.to_string()
}

/// Convert a single file's unified diff body into the word-diff rendering,
/// merging each run of deletions/insertions into one inline change.
fn apply_word_diff_to_raw(raw_diff: &str, re: &regex::Regex, color: bool) -> String {
    let trailing_nl = raw_diff.ends_with('\n');
    let mut out: Vec<String> = Vec::new();
    let mut del: Vec<&str> = Vec::new();
    let mut ins: Vec<&str> = Vec::new();
    let mut in_body = false;

    for line in raw_diff.lines() {
        if line.starts_with("@@ ") {
            flush_word_change(&mut out, &mut del, &mut ins, re, color);
            out.push(line.to_string());
            in_body = true;
            continue;
        }
        if !in_body {
            out.push(line.to_string());
            continue;
        }
        if let Some(rest) = line.strip_prefix(' ') {
            flush_word_change(&mut out, &mut del, &mut ins, re, color);
            out.push(rest.to_string());
        } else if let Some(rest) = line.strip_prefix('-') {
            del.push(rest);
        } else if let Some(rest) = line.strip_prefix('+') {
            ins.push(rest);
        } else {
            flush_word_change(&mut out, &mut del, &mut ins, re, color);
            out.push(line.to_string());
        }
    }
    flush_word_change(&mut out, &mut del, &mut ins, re, color);

    let mut joined = out.join("\n");
    if trailing_nl {
        joined.push('\n');
    }
    joined
}

fn flush_word_change<'a>(
    out: &mut Vec<String>,
    del: &mut Vec<&'a str>,
    ins: &mut Vec<&'a str>,
    re: &regex::Regex,
    color: bool,
) {
    if del.is_empty() && ins.is_empty() {
        return;
    }
    out.push(word_diff_segment(
        &del.join("\n"),
        &ins.join("\n"),
        re,
        color,
    ));
    del.clear();
    ins.clear();
}

/// Word-level diff of one change region, with deleted/inserted runs wrapped in
/// `[-...-]`/`{+...+}` (plain) or red/green ANSI (color).
fn word_diff_segment(old_text: &str, new_text: &str, re: &regex::Regex, color: bool) -> String {
    use similar::{Algorithm, ChangeTag, TextDiff};

    let old_words: Vec<&str> = re.find_iter(old_text).map(|m| m.as_str()).collect();
    let new_words: Vec<&str> = re.find_iter(new_text).map(|m| m.as_str()).collect();
    let diff = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .diff_slices(&old_words, &new_words);

    let mut out = String::new();
    let mut del = String::new();
    let mut ins = String::new();
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Equal => {
                flush_word_markers(&mut out, &mut del, &mut ins, color);
                out.push_str(change.value());
            }
            ChangeTag::Delete => del.push_str(change.value()),
            ChangeTag::Insert => ins.push_str(change.value()),
        }
    }
    flush_word_markers(&mut out, &mut del, &mut ins, color);
    out
}

fn flush_word_markers(out: &mut String, del: &mut String, ins: &mut String, color: bool) {
    if !del.is_empty() {
        if color {
            out.push_str(&format!("\x1b[31m{del}\x1b[0m"));
        } else {
            out.push_str(&format!("[-{del}-]"));
        }
        del.clear();
    }
    if !ins.is_empty() {
        if color {
            out.push_str(&format!("\x1b[32m{ins}\x1b[0m"));
        } else {
            out.push_str(&format!("{{+{ins}+}}"));
        }
        ins.clear();
    }
}

fn render_diff_output(
    args: &DiffArgs,
    result: &DiffOutput,
    output: &OutputConfig,
) -> CliResult<()> {
    if output.is_json() {
        emit_json_data("diff", result, output)?;
        // On the structured path the JSON body is the result, so only an explicit
        // `--exit-code` signals via the exit code; `--machine` implies `--quiet`
        // but must not, on its own, turn the exit code non-zero.
        return finish_with_exit_status(diff_exit_status(result, args.exit_code, false));
    }

    // For human output, `--quiet` doubles as an exit-code signal alongside the
    // explicit `--exit-code` flag.
    let exit_status = diff_exit_status(result, args.exit_code, output.quiet);
    let rendered = render_diff_text(args, result);

    // --output writes are an explicit side-effect and must be honored even when
    // --quiet is set (quiet only suppresses stdout, not file writes).
    if let Some(path) = &args.output {
        std::fs::write(path, rendered.as_bytes())
            .map_err(|e| DiffError::OutputWrite {
                path: path.clone(),
                detail: e.to_string(),
            })
            .map_err(CliError::from)?;
        return finish_with_exit_status(exit_status);
    }

    // --quiet suppresses stdout entirely; only the exit status is observable.
    if output.quiet {
        return finish_with_exit_status(exit_status);
    }

    if !rendered.is_empty() {
        let mut pager = Pager::with_config(output)?;
        let rendered = if args.name_only || args.name_status || args.numstat || args.stat {
            rendered
        } else {
            maybe_colorize_diff(&rendered, io::stdout().is_terminal())
        };
        pager.write_str(&format!("{rendered}\n"))?;
        pager.finish()?;
    }
    finish_with_exit_status(exit_status)
}

/// Translate a diff semantic exit code into a `CliResult`: a non-zero code maps
/// to a silent exit so the rendered diff is preserved on stdout.
fn finish_with_exit_status(code: i32) -> CliResult<()> {
    if code != 0 {
        Err(CliError::silent_exit(code))
    } else {
        Ok(())
    }
}

/// Select and render the human-facing diff text for the active output format.
/// The mutually exclusive human output formats, decided centrally so flag
/// precedence is explicit rather than implied by an `if` chain's order.
enum OutputKind {
    Unified,
    Stat,
    NameOnly,
    NameStatus,
    NumStat,
    Raw,
}

fn select_output_kind(args: &DiffArgs) -> OutputKind {
    if args.raw {
        OutputKind::Raw
    } else if args.name_only {
        OutputKind::NameOnly
    } else if args.name_status {
        OutputKind::NameStatus
    } else if args.numstat {
        OutputKind::NumStat
    } else if args.stat {
        OutputKind::Stat
    } else {
        OutputKind::Unified
    }
}

fn render_diff_text(args: &DiffArgs, result: &DiffOutput) -> String {
    match select_output_kind(args) {
        OutputKind::Raw => format_raw_diff(result),
        OutputKind::NameOnly => result
            .files
            .iter()
            .map(|file| file.path.clone())
            .collect::<Vec<_>>()
            .join("\n"),
        OutputKind::NameStatus => result
            .files
            .iter()
            .map(format_name_status_line)
            .collect::<Vec<_>>()
            .join("\n"),
        OutputKind::NumStat => result
            .files
            .iter()
            .map(|file| format!("{}\t{}\t{}", file.insertions, file.deletions, file.path))
            .collect::<Vec<_>>()
            .join("\n"),
        OutputKind::Stat => format_diff_stat_output(result),
        OutputKind::Unified => format_unified_diff(result),
    }
}

fn format_name_status_line(file: &DiffFileStat) -> String {
    match file.status.as_str() {
        "renamed" | "copied" => format!(
            "{}{}\t{}\t{}",
            diff_status_letter(&file.status),
            file.similarity
                .map(|s| format!("{s:03}"))
                .unwrap_or_default(),
            file.old_path.clone().unwrap_or_default(),
            file.path
        ),
        _ => format!("{}\t{}", diff_status_letter(&file.status), file.path),
    }
}

/// Render Git's `--raw` format. Modes are octal; object ids are abbreviated to
/// 7 hex chars (zeros when absent). Renames/copies emit `R<score>`/`C<score>`
/// and both old and new paths.
fn format_raw_diff(result: &DiffOutput) -> String {
    result
        .files
        .iter()
        .map(format_raw_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_raw_line(file: &DiffFileStat) -> String {
    let old_mode = file
        .old_mode
        .map_or_else(|| "000000".to_string(), |m| format!("{m:o}"));
    let new_mode = file
        .new_mode
        .map_or_else(|| "000000".to_string(), |m| format!("{m:o}"));
    let old_sha = abbreviate_raw_sha(file.old_sha.as_deref());
    let new_sha = abbreviate_raw_sha(file.new_sha.as_deref());
    let head = format!(":{old_mode} {new_mode} {old_sha} {new_sha} ");
    match file.status.as_str() {
        "renamed" | "copied" => format!(
            "{head}{}{:03}\t{}\t{}",
            diff_status_letter(&file.status),
            file.similarity.unwrap_or(100),
            file.old_path.clone().unwrap_or_default(),
            file.path
        ),
        "added" => format!("{head}A\t{}", file.path),
        "deleted" => format!("{head}D\t{}", file.path),
        "typechange" => format!("{head}T\t{}", file.path),
        _ => format!("{head}M\t{}", file.path),
    }
}

fn abbreviate_raw_sha(sha: Option<&str>) -> String {
    match sha {
        Some(s) => s.chars().take(7).collect(),
        None => "0".repeat(7),
    }
}

fn diff_status_letter(status: &str) -> &'static str {
    match status {
        "added" => "A",
        "deleted" => "D",
        "renamed" => "R",
        "copied" => "C",
        "typechange" => "T",
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

/// Produce the unified diff text of staged changes (index vs HEAD), for
/// `commit --verbose` to embed under the scissors cut line. Best-effort: returns
/// an empty string when the diff cannot be computed.
pub async fn staged_diff_text() -> String {
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
        ignore_space_change: false,
        ignore_all_space: false,
        ignore_blank_lines: false,
        unified: None,
        exit_code: false,
        find_renames: None,
        find_copies: None,
        no_renames: false,
        relative: None,
        raw: false,
        word_diff: None,
        word_diff_regex: None,
        function_context: false,
        combined: false,
    };
    match run_diff(&args, false).await {
        Ok(result) => format_unified_diff(&result),
        Err(_) => String::new(),
    }
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
    let (insertions, deletions) = count_hunk_line_changes(&item.data);

    DiffFileStat {
        path: item.path.clone(),
        status: status.to_string(),
        insertions,
        deletions,
        hunks: parse_diff_hunks(&item.data),
        old_path: None,
        similarity: None,
        old_mode: None,
        new_mode: None,
        old_sha: None,
        new_sha: None,
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

    /// The native generator at context 3 with no whitespace handling must be
    /// byte-identical to git-internal's `Diff::diff`, so switching backends per
    /// feature flag never changes the default diff output.
    #[test]
    #[serial]
    fn native_diff_matches_git_internal_at_context_3() {
        use git_internal::hash::{HashKind, set_hash_kind_for_test};
        let _guard = set_hash_kind_for_test(HashKind::Sha256);

        let cases: &[(Option<&str>, Option<&str>)] = &[
            (Some("a\nb\nc\n"), Some("a\nB\nc\nd\n")),
            (
                Some("l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\n"),
                Some("l1\nl2\nl3\nl4\nL5\nl6\nl7\nl8\nl9\n"),
            ),
            (None, Some("x\ny\n")),
            (Some("x\ny\n"), None),
            (
                Some("one\ntwo\nthree\nfour\nfive\n"),
                Some("one\nthree\nfour\nfive\nsix\n"),
            ),
            (Some("trailing\nno newline"), Some("trailing\nno newline!")),
        ];
        let file = PathBuf::from("f.txt");
        for (old, new) in cases {
            let old_bytes = old.map(str::as_bytes).unwrap_or_default().to_vec();
            let new_bytes = new.map(str::as_bytes).unwrap_or_default().to_vec();
            let old_hash = old.map(|_| calculate_object_hash(ObjectType::Blob, &old_bytes));
            let new_hash = new.map(|_| calculate_object_hash(ObjectType::Blob, &new_bytes));

            let mut old_map = HashMap::new();
            let mut new_map = HashMap::new();
            if let Some(old_hash) = old_hash {
                old_map.insert(file.clone(), old_hash);
            }
            if let Some(new_hash) = new_hash {
                new_map.insert(file.clone(), new_hash);
            }
            let mut store = HashMap::new();
            if let Some(old_hash) = old_hash {
                store.insert(old_hash, old_bytes.clone());
            }
            if let Some(new_hash) = new_hash {
                store.insert(new_hash, new_bytes.clone());
            }
            let reader = |_: &PathBuf, h: &ObjectHash| -> Vec<u8> {
                store.get(h).cloned().unwrap_or_default()
            };

            let git_internal_out = Diff::diff_for_file_string(&file, &old_map, &new_map, &reader);
            let old_hash = old_map.get(&file);
            let new_hash = new_map.get(&file);
            let native_out = native_diff_for_file(
                file.as_path(),
                old_hash,
                new_hash,
                &old_bytes,
                &new_bytes,
                DEFAULT_CONTEXT,
                WsMode::None,
                false,
                false,
            );
            assert_eq!(
                native_out, git_internal_out,
                "native diff diverged from git-internal for old={old:?} new={new:?}"
            );
        }
    }

    #[test]
    fn native_zero_context_emits_only_changed_lines() {
        let out = compute_unified_diff_native(
            "l1\nl2\nl3\nl4\nl5\n",
            "l1\nl2\nL3\nl4\nl5\n",
            0,
            WsMode::None,
            false,
        );
        assert!(out.contains("@@ -3,1 +3,1 @@"), "header mismatch: {out:?}");
        assert_eq!(
            out.lines().filter(|l| l.starts_with(' ')).count(),
            0,
            "context lines should be absent at -U0: {out:?}"
        );
        assert!(out.contains("-l3") && out.contains("+L3"), "out={out:?}");
    }

    #[test]
    fn native_ignore_all_space_drops_whitespace_only_change() {
        let out = compute_unified_diff_native("a b\n", "a    b\n", 3, WsMode::AllSpace, false);
        assert!(
            out.is_empty(),
            "whitespace-only change should be empty: {out:?}"
        );
    }

    #[test]
    fn native_ignore_space_change_collapses_runs() {
        let out =
            compute_unified_diff_native("a b c\n", "a   b   c \n", 3, WsMode::SpaceChange, false);
        assert!(
            out.is_empty(),
            "space-change-only diff should be empty: {out:?}"
        );
    }

    #[test]
    fn native_ignore_blank_lines_drops_blank_only_hunk() {
        let out = compute_unified_diff_native("a\nb\n", "a\n\nb\n", 3, WsMode::None, true);
        assert!(!out.contains("@@"), "blank-only insertion ignored: {out:?}");
    }

    #[test]
    fn native_function_context_expands_to_enclosing_function() {
        let old = "fn alpha() {\n    let a = 1;\n    let b = 2;\n    let c = 3;\n    let d = 4;\n    return a;\n}\nfn beta() {\n    let z = 0;\n}\n";
        let new = "fn alpha() {\n    let a = 1;\n    let b = 2;\n    let c = 30;\n    let d = 4;\n    return a;\n}\nfn beta() {\n    let z = 0;\n}\n";
        let out = compute_function_context_diff(old, new, WsMode::None);
        assert!(
            out.contains(" fn alpha() {"),
            "header expands into context: {out}"
        );
        assert!(out.contains("-    let c = 3;"), "{out}");
        assert!(out.contains("+    let c = 30;"), "{out}");
        assert!(
            out.contains(" }"),
            "closing brace stays in the block: {out}"
        );
        // beta is a separate, unchanged function and must not be pulled in.
        assert!(
            !out.contains("let z = 0;"),
            "untouched function excluded: {out}"
        );
    }

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
        let (blobs, _modes, _contents) = get_files_blobs(
            &[PathBuf::from("should_ignore"), PathBuf::from("not_ignore")],
            &index,
            IgnorePolicy::Respect,
        )
        .unwrap();
        assert_eq!(blobs.len(), 1);
        assert_eq!(blobs[0].0, PathBuf::from("not_ignore"));
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

        let (blobs, _modes, _contents) = get_files_blobs(
            &[PathBuf::from("tracked.txt")],
            &index,
            IgnorePolicy::Respect,
        )
        .unwrap();

        assert_eq!(blobs, vec![(PathBuf::from("tracked.txt"), indexed_hash)]);
    }
}
