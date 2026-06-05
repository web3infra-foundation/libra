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
        object::{blob::Blob, commit::Commit, tree::Tree, types::ObjectType},
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
    emit_worktree_scan_progress(&args, output);
    let result = run_diff(&args).await.map_err(CliError::from)?;
    render_diff_output(&args, &result, output)
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

    // Resolve the unified-context radius (`-U` > `diff.context` > 3) and the
    // whitespace-comparison mode before choosing a diff backend.
    let context = resolve_diff_context(args).await?;
    let ws = WsMode::from_args(args);
    let use_native = ws != WsMode::None || args.ignore_blank_lines || context != DEFAULT_CONTEXT;

    let worktree_entries = new_side.worktree_entries.clone();
    let worktree_cache = RefCell::new(HashMap::<ObjectHash, Vec<u8>>::new());
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
            &read,
        )
    } else {
        Diff::diff(old_side.blobs, new_side.blobs, paths, &read)
    };
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

    match (
        std::str::from_utf8(old_bytes),
        std::str::from_utf8(new_bytes),
    ) {
        (Ok(old_text), Ok(new_text)) => {
            let (old_pref, new_pref) = if old_text.is_empty() {
                ("/dev/null".to_string(), format!("b/{}", file.display()))
            } else if new_text.is_empty() {
                (format!("a/{}", file.display()), "/dev/null".to_string())
            } else {
                (
                    format!("a/{}", file.display()),
                    format!("b/{}", file.display()),
                )
            };
            let _ = writeln!(out, "--- {old_pref}");
            let _ = writeln!(out, "+++ {new_pref}");
            out.push_str(&compute_unified_diff_native(
                old_text,
                new_text,
                context,
                ws,
                ignore_blank_lines,
            ));
        }
        _ => {
            let _ = writeln!(out, "Binary files differ");
        }
    }
    out
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
fn render_diff_text(args: &DiffArgs, result: &DiffOutput) -> String {
    if args.name_only {
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
    }
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
    };
    match run_diff(&args).await {
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

        let cases: &[(&str, &str)] = &[
            ("a\nb\nc\n", "a\nB\nc\nd\n"),
            (
                "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\n",
                "l1\nl2\nl3\nl4\nL5\nl6\nl7\nl8\nl9\n",
            ),
            ("", "x\ny\n"),
            ("x\ny\n", ""),
            (
                "one\ntwo\nthree\nfour\nfive\n",
                "one\nthree\nfour\nfive\nsix\n",
            ),
            ("trailing\nno newline", "trailing\nno newline!"),
        ];
        let file = PathBuf::from("f.txt");
        for (old, new) in cases {
            let old_bytes = old.as_bytes().to_vec();
            let new_bytes = new.as_bytes().to_vec();
            let old_hash = calculate_object_hash(ObjectType::Blob, &old_bytes);
            let new_hash = calculate_object_hash(ObjectType::Blob, &new_bytes);

            let mut old_map = HashMap::new();
            let mut new_map = HashMap::new();
            old_map.insert(file.clone(), old_hash);
            new_map.insert(file.clone(), new_hash);
            let mut store = HashMap::new();
            store.insert(old_hash, old_bytes.clone());
            store.insert(new_hash, new_bytes.clone());
            let reader = |_: &PathBuf, h: &ObjectHash| -> Vec<u8> {
                store.get(h).cloned().unwrap_or_default()
            };

            let git_internal_out = Diff::diff_for_file_string(&file, &old_map, &new_map, &reader);
            let native_out = native_diff_for_file(
                file.as_path(),
                Some(&old_hash),
                Some(&new_hash),
                &old_bytes,
                &new_bytes,
                DEFAULT_CONTEXT,
                WsMode::None,
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
