//! Implements status reporting with ignore policy support, computing staged/unstaged/untracked sets and printing concise summaries.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    io,
    io::Write,
    path::{Path, PathBuf},
};

use clap::{Parser, ValueEnum};
use colored::Colorize;
use git_internal::{
    errors::GitError,
    hash::{ObjectHash, get_hash_kind},
    internal::{
        index::Index,
        object::{
            blob::Blob,
            commit::Commit,
            tree::{Tree, TreeItemMode},
        },
    },
};
use serde::Serialize;

use super::{bisect, cherry_pick, merge, rebase, stash};
use crate::{
    command::{calc_file_blob_hash, load_object},
    internal::{
        branch::{Branch, BranchStoreError},
        config::ConfigKv,
        head::Head,
    },
    utils::{
        error::{CliError, CliResult, StableErrorCode, emit_warning},
        ignore::IgnorePolicy,
        object_ext::{CommitExt, TreeExt},
        output::{ColorChoice, OutputConfig, emit_json_data},
        path, util,
    },
};

// ---------------------------------------------------------------------------
// Args & enums
// ---------------------------------------------------------------------------

const STATUS_EXAMPLES: &str = "\
EXAMPLES:
    libra status                       Show working tree status
    libra status -s                    Short format output
    libra status --porcelain           Machine-readable output (v1)
    libra status --porcelain v2        Extended machine-readable output
    libra status --branch              Include branch info in short/porcelain
    libra status --show-stash          Show stash count
    libra status --ignored             Include ignored files
    libra status --untracked-files=no  Hide untracked files
    libra status --json                Structured JSON output for agents
    libra status --exit-code           Exit 1 if working tree is dirty
    libra status --quiet --exit-code   Silent dirty check for scripts";

/// Show the working tree status.
// EXAMPLES are wired via `#[command(after_help = STATUS_EXAMPLES)]` and render
// at the bottom of `libra status --help`. The meta-commentary that used to
// live here as a `///` line leaked into clap's `--help` body (see
// `tests/command/status_test.rs::test_status_help_does_not_leak_impl_meta`).
#[derive(Parser, Debug, Default)]
#[command(after_help = STATUS_EXAMPLES)]
pub struct StatusArgs {
    /// Output in a machine-readable format (default v1). Use v2 for extended format.
    #[clap(
        long = "porcelain",
        value_name = "VERSION",
        num_args = 0..=1,
        default_missing_value = "v1",
        conflicts_with = "short"
    )]
    pub porcelain: Option<PorcelainVersion>,

    /// Give the output in the short-format
    #[clap(short = 's', long = "short", conflicts_with = "porcelain")]
    pub short: bool,

    /// Output with branch info (short or porcelain mode)
    #[clap(long = "branch")]
    pub branch: bool,

    /// Output with stash info (only in standard mode)
    #[clap(long = "show-stash")]
    pub show_stash: bool,

    /// Show ignored files
    #[clap(long = "ignored")]
    pub ignored: bool,

    /// Control untracked files display (normal|all|no)
    #[clap(long = "untracked-files", value_name = "MODE")]
    pub untracked_files: Option<UntrackedFiles>,

    /// Exit with code 1 if the working tree has changes.
    /// Can be combined with --quiet for silent dirty checking.
    #[clap(long = "exit-code")]
    pub exit_code: bool,

    /// Terminate porcelain entries with a NUL byte instead of a newline.
    /// Implies `--porcelain=v1` when no other format is given.
    #[clap(short = 'z')]
    pub z: bool,

    /// Show only changes for paths matching this pattern (Phase 1 enhancement)
    #[clap(
        value_name = "pathspec",
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    pub pathspec: Vec<String>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum PorcelainVersion {
    #[clap(name = "v1")]
    V1,
    #[clap(name = "v2")]
    V2,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum, Default)]
pub enum UntrackedFiles {
    /// Show untracked files (default): only list untracked directories, not their contents.
    #[default]
    Normal,
    /// Show all untracked files, recursively listing files within untracked directories.
    All,
    /// Do not show untracked files
    No,
}

// ---------------------------------------------------------------------------
// Changes
// ---------------------------------------------------------------------------

/// path: to workdir
#[derive(Debug, Default, Clone)]
pub struct Changes {
    pub new: Vec<PathBuf>,
    pub modified: Vec<PathBuf>,
    pub deleted: Vec<PathBuf>,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.new.is_empty() && self.modified.is_empty() && self.deleted.is_empty()
    }

    /// to relative path(to cur_dir)
    pub fn to_relative(&self) -> Changes {
        let mut change = self.clone();
        [&mut change.new, &mut change.modified, &mut change.deleted]
            .into_iter()
            .for_each(|paths| {
                *paths = paths
                    .iter()
                    .map(relative_path_preserving_collapsed_dir)
                    .collect();
            });
        change
    }
    pub fn polymerization(&self) -> Vec<PathBuf> {
        let mut poly = self.new.clone();
        poly.extend(self.modified.clone());
        poly.extend(self.deleted.clone());
        poly
    }

    pub fn extend(&mut self, other: Changes) {
        self.new.extend(other.new);
        self.modified.extend(other.modified);
        self.deleted.extend(other.deleted);
    }
}

fn relative_path_preserving_collapsed_dir(path: &PathBuf) -> PathBuf {
    let collapsed_dir = path.to_string_lossy().ends_with('/');
    let relative = util::workdir_to_current(path);
    if collapsed_dir {
        path_with_trailing_separator(&relative)
    } else {
        relative
    }
}

// ---------------------------------------------------------------------------
// StatusError + CliError mapping
// ---------------------------------------------------------------------------

#[derive(thiserror::Error, Debug)]
pub enum StatusError {
    #[error("failed to open index '{path}': {source}")]
    IndexLoad { path: PathBuf, source: GitError },
    #[error("path '{path}' is not valid UTF-8")]
    InvalidPathEncoding { path: PathBuf },
    #[error("failed to hash '{path}': {source}")]
    FileHash { path: PathBuf, source: io::Error },
    #[error("failed to list files in '{path}': {source}")]
    ListWorkdirFiles { path: PathBuf, source: io::Error },
    #[error("failed to determine working directory: {source}")]
    Workdir { source: io::Error },
}

impl From<StatusError> for CliError {
    fn from(error: StatusError) -> Self {
        let msg = format!("failed to determine working tree status: {error}");
        match &error {
            StatusError::IndexLoad { .. } => CliError::fatal(msg)
                .with_stable_code(StableErrorCode::RepoCorrupt)
                .with_hint("the index file may be corrupted"),
            StatusError::InvalidPathEncoding { .. } => CliError::fatal(msg)
                .with_stable_code(StableErrorCode::CliInvalidTarget)
                .with_hint("path contains non-UTF-8 characters"),
            StatusError::FileHash { .. } => {
                CliError::fatal(msg).with_stable_code(StableErrorCode::IoReadFailed)
            }
            StatusError::ListWorkdirFiles { .. } => {
                CliError::fatal(msg).with_stable_code(StableErrorCode::IoReadFailed)
            }
            StatusError::Workdir { .. } => {
                CliError::fatal(msg).with_stable_code(StableErrorCode::RepoNotFound)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// UpstreamInfo
// ---------------------------------------------------------------------------

/// Upstream tracking information for the current branch.
#[derive(Debug, Clone, Serialize)]
pub struct UpstreamInfo {
    /// Tracking ref display name, e.g. "origin/main"
    pub remote_ref: String,
    /// Commits ahead of upstream (None when gone)
    pub ahead: Option<usize>,
    /// Commits behind upstream (None when gone)
    pub behind: Option<usize>,
    /// True when upstream is configured but tracking ref no longer exists
    pub gone: bool,
}

/// In-progress merge metadata surfaced by `status` for recovery guidance.
#[derive(Debug, Clone, Serialize)]
pub struct MergeStatusInfo {
    pub target_ref: String,
    pub conflicted_paths: Vec<String>,
}

#[derive(Debug, Clone)]
enum RepoState {
    Merge,
    Rebase { onto: String, head_name: String },
    Bisect,
    CherryPick,
}

impl RepoState {
    const fn json_name(&self) -> &'static str {
        match self {
            RepoState::Merge => "merge",
            RepoState::Rebase { .. } => "rebase",
            RepoState::Bisect => "bisect",
            RepoState::CherryPick => "cherry-pick",
        }
    }
}

const STATUS_RENAME_THRESHOLD: f64 = 0.5;
const STATUS_RENAME_MAX_BLOB_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone)]
struct RenameEntry {
    from: PathBuf,
    to: PathBuf,
    score: u32,
    unstaged_status: char,
}

#[derive(Debug, Default, Clone)]
struct TypeChangeSet {
    staged: HashSet<PathBuf>,
    unstaged: HashSet<PathBuf>,
}

#[derive(Debug, Clone)]
struct UnmergedStageInfo {
    mode: u32,
    hash: String,
}

#[derive(Debug, Clone)]
struct UnmergedEntry {
    path: PathBuf,
    xy: &'static str,
    base: Option<UnmergedStageInfo>,
    ours: Option<UnmergedStageInfo>,
    theirs: Option<UnmergedStageInfo>,
    worktree_mode: u32,
}

#[derive(Debug, Clone)]
enum StatusLineKind {
    Ordinary,
    Rename { from: PathBuf },
    Unmerged,
}

#[derive(Debug, Clone)]
struct StatusLine {
    path: PathBuf,
    staged_status: char,
    unstaged_status: char,
    kind: StatusLineKind,
}

#[derive(Copy, Clone)]
struct StatusRenderDetails<'a> {
    renames: &'a [RenameEntry],
    typechanges: &'a TypeChangeSet,
    unmerged_entries: &'a [UnmergedEntry],
}

#[derive(Debug, Clone, Copy)]
struct EffectiveStatusOptions {
    branch: bool,
    short: bool,
    untracked_files: UntrackedFiles,
}

// ---------------------------------------------------------------------------
// StatusData — shared data layer
// ---------------------------------------------------------------------------

/// Pre-computed status data shared across all renderers (human/JSON/short/porcelain).
struct StatusData {
    head: Head,
    head_oid: Option<ObjectHash>,
    has_commits: bool,
    staged: Changes,
    unstaged: Changes,
    ignored_files: Vec<PathBuf>,
    renames: Vec<RenameEntry>,
    typechanges: TypeChangeSet,
    unmerged_entries: Vec<UnmergedEntry>,
    stash_count: Option<usize>,
    upstream: Option<UpstreamInfo>,
    merge_state: Option<MergeStatusInfo>,
    repo_state: Option<RepoState>,
    porcelain_v2: Option<PorcelainV2Data>,
}

impl StatusData {
    fn is_dirty(&self) -> bool {
        !self.staged.is_empty()
            || !self.unstaged.is_empty()
            || !self.unmerged_entries.is_empty()
            || self.merge_state.is_some()
    }
}

/// Collect all status data in one pass, eliminating duplicate computation
/// between human/JSON/short/porcelain renderers.
async fn collect_status_data(
    args: &StatusArgs,
    options: &EffectiveStatusOptions,
) -> CliResult<StatusData> {
    if is_bare_repository().await {
        return Err(CliError::fatal("this operation must be run in a work tree")
            .with_stable_code(StableErrorCode::RepoStateInvalid)
            .with_hint("this command requires a working tree; bare repositories do not have one"));
    }

    let head = Head::current_result()
        .await
        .map_err(|error| status_branch_store_error("resolve HEAD", error))?;
    let head_oid = Head::current_commit_result()
        .await
        .map_err(|error| status_branch_store_error("resolve HEAD commit", error))?;
    let has_commits = head_oid.is_some();

    let staged = changes_to_be_committed_safe()
        .await
        .map(|c| c.to_relative())
        .map_err(CliError::from)?;
    let mut unstaged = changes_to_be_staged_for_status(options.untracked_files)
        .map(|c| c.to_relative())
        .map_err(CliError::from)?;
    let status_index = load_status_index()?;
    let head_tree_items = build_head_tree_items(head_oid.as_ref());
    let index_items = build_index_items(&status_index);
    let renames = detect_status_renames(&staged, &unstaged, &head_tree_items, &index_items);
    let typechanges = detect_typechanges(&head_tree_items, &index_items);
    let unmerged_entries = collect_unmerged_entries(&status_index);
    let mut ignored_files =
        if args.ignored && !matches!(options.untracked_files, UntrackedFiles::No) {
            list_ignored_files()
                .map(|c| c.to_relative().new)
                .map_err(CliError::from)?
        } else {
            vec![]
        };
    // Apply untracked-files filter
    match options.untracked_files {
        UntrackedFiles::No => {
            unstaged.new.clear();
            ignored_files.clear();
        }
        UntrackedFiles::Normal => {
            ignored_files = collapse_untracked_directories(ignored_files, &status_index);
        }
        UntrackedFiles::All => {}
    }

    let stash_count = if args.show_stash {
        Some(stash::get_stash_num().unwrap_or(0))
    } else {
        None
    };

    // Resolve upstream tracking info
    let upstream = resolve_upstream_info(&head, head_oid.as_ref()).await?;
    let merge_state = match merge::MergeState::load_optional_sync().map_err(|detail| {
        CliError::fatal(format!("failed to inspect merge state: {detail}"))
            .with_stable_code(StableErrorCode::IoReadFailed)
    })? {
        Some(state) => Some(MergeStatusInfo {
            target_ref: state.target_ref,
            conflicted_paths: merge::unresolved_conflicted_paths(
                &status_index,
                &state.conflicted_paths,
            ),
        }),
        None => None,
    };
    let repo_state = detect_repo_state(merge_state.as_ref()).await?;

    // Apply pathspec filtering if provided
    let (staged, unstaged, ignored_files) = if !args.pathspec.is_empty() {
        (
            Changes {
                new: filter_paths_by_pathspec(&staged.new, &args.pathspec),
                modified: filter_paths_by_pathspec(&staged.modified, &args.pathspec),
                deleted: filter_paths_by_pathspec(&staged.deleted, &args.pathspec),
            },
            Changes {
                new: filter_paths_by_pathspec(&unstaged.new, &args.pathspec),
                modified: filter_paths_by_pathspec(&unstaged.modified, &args.pathspec),
                deleted: filter_paths_by_pathspec(&unstaged.deleted, &args.pathspec),
            },
            filter_paths_by_pathspec(&ignored_files, &args.pathspec),
        )
    } else {
        (staged, unstaged, ignored_files)
    };

    let porcelain_v2 = if matches!(args.porcelain, Some(PorcelainVersion::V2)) {
        Some(PorcelainV2Data {
            index: status_index,
            head_tree_items,
        })
    } else {
        None
    };

    Ok(StatusData {
        head,
        head_oid,
        has_commits,
        staged,
        unstaged,
        ignored_files,
        renames,
        typechanges,
        unmerged_entries,
        stash_count,
        upstream,
        merge_state,
        repo_state,
        porcelain_v2,
    })
}

async fn detect_repo_state(merge_state: Option<&MergeStatusInfo>) -> CliResult<Option<RepoState>> {
    if rebase::RebaseState::is_in_progress()
        .await
        .map_err(|detail| repo_state_inspect_error("inspect rebase state", detail))?
    {
        let state = rebase::RebaseState::load()
            .await
            .map_err(|detail| repo_state_inspect_error("load rebase state", detail))?;
        return Ok(Some(RepoState::Rebase {
            onto: state.onto.to_string(),
            head_name: state.head_name,
        }));
    }

    if bisect::BisectState::is_in_progress()
        .await
        .map_err(|detail| repo_state_inspect_error("inspect bisect state", detail))?
    {
        return Ok(Some(RepoState::Bisect));
    }

    if merge_state.is_some() {
        return Ok(Some(RepoState::Merge));
    }

    if cherry_pick::CherryPickState::is_in_progress()
        .await
        .map_err(|detail| repo_state_inspect_error("inspect cherry-pick state", detail))?
    {
        return Ok(Some(RepoState::CherryPick));
    }

    Ok(None)
}

async fn resolve_status_options(args: &StatusArgs) -> CliResult<EffectiveStatusOptions> {
    let configured_untracked = match args.untracked_files {
        Some(mode) => mode,
        None => read_status_untracked_config().await?,
    };

    let configured_branch = if args.branch {
        true
    } else {
        read_status_bool_config("status.branch", false).await?
    };

    let configured_short = if args.short {
        true
    } else if args.porcelain.is_some() || args.z {
        false
    } else {
        read_status_bool_config("status.short", false).await?
    };

    Ok(EffectiveStatusOptions {
        branch: configured_branch,
        short: configured_short,
        untracked_files: configured_untracked,
    })
}

async fn read_status_untracked_config() -> CliResult<UntrackedFiles> {
    let Some(value) = read_status_config_value("status.showUntrackedFiles").await? else {
        return Ok(UntrackedFiles::Normal);
    };

    match value.as_str() {
        "no" => Ok(UntrackedFiles::No),
        "normal" => Ok(UntrackedFiles::Normal),
        "all" => Ok(UntrackedFiles::All),
        _ => {
            emit_warning(format!(
                "invalid status.showUntrackedFiles '{value}', using default"
            ));
            Ok(UntrackedFiles::Normal)
        }
    }
}

async fn read_status_bool_config(key: &str, default: bool) -> CliResult<bool> {
    let Some(value) = read_status_config_value(key).await? else {
        return Ok(default);
    };

    match value.to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Ok(true),
        "false" | "no" | "off" | "0" => Ok(false),
        _ => {
            emit_warning(format!("invalid {key} '{value}', using default"));
            Ok(default)
        }
    }
}

async fn read_status_config_value(key: &str) -> CliResult<Option<String>> {
    ConfigKv::get(key)
        .await
        .map(|entry| entry.map(|entry| entry.value))
        .map_err(|error| status_config_read_error(&format!("read {key}"), error))
}

fn repo_state_inspect_error(action: &str, detail: String) -> CliError {
    CliError::fatal(format!("failed to {action}: {detail}"))
        .with_stable_code(StableErrorCode::RepoStateInvalid)
        .with_hint("inspect the in-progress repository operation and retry status")
}

fn load_status_index() -> CliResult<Index> {
    let index_path =
        path::try_index().map_err(|source| CliError::from(StatusError::Workdir { source }))?;
    Index::load(&index_path).map_err(|source| {
        CliError::from(StatusError::IndexLoad {
            path: index_path,
            source,
        })
    })
}

fn build_head_tree_items(head_oid: Option<&ObjectHash>) -> HashMap<PathBuf, FileInfo> {
    if let Some(commit_hash) = head_oid {
        let commit = Commit::load(commit_hash);
        let tree = Tree::load(&commit.tree_id);
        tree.get_plain_items_with_mode()
            .into_iter()
            .map(|(path, hash, mode)| {
                (
                    path,
                    FileInfo {
                        mode: tree_item_mode_to_u32(mode),
                        hash: hash.to_string(),
                    },
                )
            })
            .collect()
    } else {
        HashMap::new()
    }
}

fn build_index_items(index: &Index) -> HashMap<PathBuf, FileInfo> {
    index
        .tracked_entries(0)
        .into_iter()
        .map(|entry| {
            (
                PathBuf::from(&entry.name),
                FileInfo {
                    mode: entry.mode,
                    hash: entry.hash.to_string(),
                },
            )
        })
        .collect()
}

struct RenameCandidate {
    path: PathBuf,
    hash: ObjectHash,
    mode: u32,
    data: Vec<u8>,
}

fn load_small_blob(hash: &ObjectHash) -> Option<Vec<u8>> {
    let blob = load_object::<Blob>(hash).ok()?;
    if blob.data.len() > STATUS_RENAME_MAX_BLOB_BYTES {
        return None;
    }
    Some(blob.data)
}

fn object_hash_from_hex(hex: &str) -> Option<ObjectHash> {
    hex.parse::<ObjectHash>().ok()
}

fn head_rename_candidate(
    path: &Path,
    head_tree_items: &HashMap<PathBuf, FileInfo>,
) -> Option<RenameCandidate> {
    let workdir_path = current_to_workdir(path);
    let info = head_tree_items.get(&workdir_path)?;
    let hash = object_hash_from_hex(&info.hash)?;
    let data = load_small_blob(&hash)?;
    Some(RenameCandidate {
        path: path.to_path_buf(),
        hash,
        mode: info.mode,
        data,
    })
}

fn index_rename_candidate(
    path: &Path,
    index_items: &HashMap<PathBuf, FileInfo>,
) -> Option<RenameCandidate> {
    let workdir_path = current_to_workdir(path);
    let info = index_items.get(&workdir_path)?;
    let hash = object_hash_from_hex(&info.hash)?;
    let data = load_small_blob(&hash)?;
    Some(RenameCandidate {
        path: path.to_path_buf(),
        hash,
        mode: info.mode,
        data,
    })
}

fn detect_status_renames(
    staged: &Changes,
    unstaged: &Changes,
    head_tree_items: &HashMap<PathBuf, FileInfo>,
    index_items: &HashMap<PathBuf, FileInfo>,
) -> Vec<RenameEntry> {
    let mut deleted: Vec<_> = staged
        .deleted
        .iter()
        .filter_map(|path| head_rename_candidate(path, head_tree_items))
        .collect();
    let mut added: Vec<_> = staged
        .new
        .iter()
        .filter_map(|path| index_rename_candidate(path, index_items))
        .collect();
    deleted.sort_by(|left, right| left.path.cmp(&right.path));
    added.sort_by(|left, right| left.path.cmp(&right.path));

    let mut consumed_deleted = HashSet::new();
    let mut renames = Vec::new();
    for new_candidate in &added {
        let mut best: Option<(usize, f64)> = None;
        for (old_index, old_candidate) in deleted.iter().enumerate() {
            if consumed_deleted.contains(&old_index) || old_candidate.mode == 0o160000 {
                continue;
            }
            let similarity = if old_candidate.hash == new_candidate.hash {
                1.0
            } else {
                crate::utils::blob_similarity::blob_line_similarity(
                    &old_candidate.data,
                    &new_candidate.data,
                )
            };
            if similarity >= STATUS_RENAME_THRESHOLD
                && best.is_none_or(|(_, best_similarity)| similarity > best_similarity)
            {
                best = Some((old_index, similarity));
            }
        }
        if let Some((old_index, similarity)) = best {
            consumed_deleted.insert(old_index);
            let old_candidate = &deleted[old_index];
            renames.push(RenameEntry {
                from: old_candidate.path.clone(),
                to: new_candidate.path.clone(),
                score: ((similarity * 100.0).round() as u32).min(100),
                unstaged_status: unstaged_status_for_path(unstaged, &new_candidate.path),
            });
        }
    }
    renames
}

fn unstaged_status_for_path(unstaged: &Changes, path: &Path) -> char {
    if unstaged.modified.iter().any(|candidate| candidate == path) {
        'M'
    } else if unstaged.deleted.iter().any(|candidate| candidate == path) {
        'D'
    } else {
        ' '
    }
}

fn mode_kind(mode: u32) -> u32 {
    mode & 0o170000
}

fn is_typechange(before: u32, after: u32) -> bool {
    before != 0 && after != 0 && mode_kind(before) != mode_kind(after)
}

fn detect_typechanges(
    head_tree_items: &HashMap<PathBuf, FileInfo>,
    index_items: &HashMap<PathBuf, FileInfo>,
) -> TypeChangeSet {
    let mut changes = TypeChangeSet::default();
    for (workdir_path, index_info) in index_items {
        let display_path = util::workdir_to_current(workdir_path);
        if let Some(head_info) = head_tree_items.get(workdir_path)
            && is_typechange(head_info.mode, index_info.mode)
        {
            changes.staged.insert(display_path.clone());
        }
        if let Some(worktree_mode) = try_get_worktree_mode(&display_path)
            && is_typechange(index_info.mode, worktree_mode)
        {
            changes.unstaged.insert(display_path);
        }
    }
    changes
}

pub fn unmerged_xy_for_stage_presence(
    base: bool,
    ours: bool,
    theirs: bool,
) -> Option<&'static str> {
    match (base, ours, theirs) {
        (false, true, true) => Some("AA"),
        (true, false, false) => Some("DD"),
        (false, true, false) => Some("AU"),
        (false, false, true) => Some("UA"),
        (true, false, true) => Some("DU"),
        (true, true, false) => Some("UD"),
        (true, true, true) => Some("UU"),
        (false, false, false) => None,
    }
}

fn collect_unmerged_entries(index: &Index) -> Vec<UnmergedEntry> {
    let mut paths = HashSet::new();
    for stage in 1..=3 {
        for entry in index.tracked_entries(stage) {
            if !index.tracked(&entry.name, 0) {
                paths.insert(entry.name.clone());
            }
        }
    }
    let mut paths: Vec<_> = paths.into_iter().collect();
    paths.sort();

    paths
        .into_iter()
        .filter_map(|path_name| {
            let has_base = index.tracked(&path_name, 1);
            let has_ours = index.tracked(&path_name, 2);
            let has_theirs = index.tracked(&path_name, 3);
            let xy = unmerged_xy_for_stage_presence(has_base, has_ours, has_theirs)?;
            let display_path = util::workdir_to_current(Path::new(&path_name));
            Some(UnmergedEntry {
                worktree_mode: try_get_worktree_mode(&display_path).unwrap_or(0),
                path: display_path,
                xy,
                base: unmerged_stage_info(index, &path_name, 1),
                ours: unmerged_stage_info(index, &path_name, 2),
                theirs: unmerged_stage_info(index, &path_name, 3),
            })
        })
        .collect()
}

fn unmerged_stage_info(index: &Index, path_name: &str, stage: u8) -> Option<UnmergedStageInfo> {
    let entry = index.get(path_name, stage)?;
    Some(UnmergedStageInfo {
        mode: entry.mode,
        hash: entry.hash.to_string(),
    })
}

/// Filter paths by pathspec patterns (Phase 1: basic glob-style matching).
fn filter_paths_by_pathspec(paths: &[PathBuf], pathspecs: &[String]) -> Vec<PathBuf> {
    if pathspecs.is_empty() {
        return paths.to_vec();
    }

    paths
        .iter()
        .filter(|path| {
            let path_str = path.to_string_lossy();
            pathspecs.iter().any(|pattern| {
                path_str.contains(pattern.as_str()) || path_str.starts_with(pattern.as_str())
            })
        })
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Collect repository status and render it inside the same `{ok, command,
/// data}` envelope that `libra status --json` prints, so `/api/repo/status`
/// stays byte-compatible with the CLI output.
///
/// Internally re-uses [`collect_status_data`] + [`build_status_json`] with a
/// default [`StatusArgs`] (untracked files in normal mode, no porcelain v2,
/// no ignored files, no stash count).
///
/// Status collection currently resolves storage from the process working
/// directory; the embedded web server expects to be launched from (or with
/// `--cwd`/`--repo` already chdir'd to) the repository root. Callers that
/// need to scope to a specific path should pass it via `working_dir`.
pub async fn collect_status_json_envelope_for_api(
    working_dir: &std::path::Path,
) -> CliResult<serde_json::Value> {
    use std::path::PathBuf;

    let args = StatusArgs::default();
    let canon_working =
        std::fs::canonicalize(working_dir).unwrap_or_else(|_| PathBuf::from(working_dir));
    let canon_cwd = std::env::current_dir()
        .ok()
        .and_then(|cwd| std::fs::canonicalize(&cwd).ok());
    if canon_cwd.as_deref() != Some(canon_working.as_path()) {
        return Err(CliError::fatal(format!(
            "/api/repo/status currently requires the libra process to run inside its repository root. Expected '{}', found '{}'. Re-launch `libra code` from the repo or open an issue if you need cross-directory status.",
            canon_working.display(),
            canon_cwd
                .as_deref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "<unavailable>".to_string()),
        )));
    }

    let options = resolve_status_options(&args).await?;
    let data = collect_status_data(&args, &options).await?;
    let inner = build_status_json(&data, &args);
    Ok(serde_json::json!({
        "ok": true,
        "command": "status",
        "data": inner,
    }))
}

pub async fn execute(args: StatusArgs) {
    if let Err(err) = execute_to(args, &mut std::io::stdout()).await {
        err.print_stderr();
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting. JSON mode propagates status-computation failures as
/// structured CLI errors; text mode uses the same structured error contract.
pub async fn execute_safe(args: StatusArgs, output: &OutputConfig) -> CliResult<()> {
    util::require_repo().map_err(|_| CliError::repo_not_found())?;

    let options = resolve_status_options(&args).await?;
    let data = collect_status_data(&args, &options).await?;

    if output.is_json() {
        let json_data = build_status_json(&data, &args);
        emit_json_data("status", &json_data, output)?;
    } else if !output.quiet {
        let mut stdout = std::io::stdout();
        render_status_to_writer(&data, &args, &options, output, &mut stdout).await?;
    }

    // --exit-code: dirty → exit 1 (silent; do not emit an error line)
    if args.exit_code && data.is_dirty() {
        return Err(CliError::silent_exit(1));
    }

    Ok(())
}

/// Legacy entry point that writes status to the given writer.
/// Used by the old `execute()` path and tests.
pub async fn execute_to(args: StatusArgs, writer: &mut impl Write) -> CliResult<()> {
    util::require_repo().map_err(|_| CliError::repo_not_found())?;

    let options = resolve_status_options(&args).await?;
    let data = collect_status_data(&args, &options).await?;
    let output = OutputConfig::default();
    render_status_to_writer(&data, &args, &options, &output, writer).await
}

// ---------------------------------------------------------------------------
// Rendering dispatcher
// ---------------------------------------------------------------------------

async fn render_status_to_writer(
    data: &StatusData,
    args: &StatusArgs,
    options: &EffectiveStatusOptions,
    output: &OutputConfig,
    writer: &mut impl Write,
) -> CliResult<()> {
    let write_error =
        |err: io::Error| CliError::io(format!("failed to write status output: {err}"));
    let mut buffer = Vec::new();

    // Porcelain modes. `-z` implies porcelain v1 when no explicit format is
    // given, and switches the entry terminator from newline to NUL.
    let porcelain = args.porcelain.or(if args.z && !options.short {
        Some(PorcelainVersion::V1)
    } else {
        None
    });
    match porcelain {
        Some(PorcelainVersion::V2) => {
            if options.branch {
                write_branch_info_v2(
                    &data.head,
                    data.head_oid.as_ref(),
                    data.upstream.as_ref(),
                    &mut buffer,
                )?;
            }
            output_porcelain_v2(
                &data.staged,
                &data.unstaged,
                &data.ignored_files,
                StatusRenderDetails {
                    renames: &data.renames,
                    typechanges: &data.typechanges,
                    unmerged_entries: &data.unmerged_entries,
                },
                data.porcelain_v2.as_ref(),
                args.z,
                &mut buffer,
            )?;
            if args.z {
                nul_terminate_lines(&mut buffer);
            }
            writer.write_all(&buffer).map_err(write_error)?;
            return Ok(());
        }
        Some(PorcelainVersion::V1) => {
            if options.branch {
                print_branch_info(&data.head, data.upstream.as_ref(), &mut buffer)?;
            }
            output_porcelain(&data.staged, &data.unstaged, &mut buffer)?;
            if args.ignored && !data.ignored_files.is_empty() {
                for file in &data.ignored_files {
                    writeln!(&mut buffer, "!! {}", file.display()).map_err(write_error)?;
                }
            }
            if args.z {
                nul_terminate_lines(&mut buffer);
            }
            writer.write_all(&buffer).map_err(write_error)?;
            return Ok(());
        }
        None => {}
    };

    // Short format
    if options.short {
        if options.branch {
            print_branch_info(&data.head, data.upstream.as_ref(), &mut buffer)?;
        }
        output_short_format_with_details(
            &data.staged,
            &data.unstaged,
            StatusRenderDetails {
                renames: &data.renames,
                typechanges: &data.typechanges,
                unmerged_entries: &data.unmerged_entries,
            },
            output,
            args.z,
            &mut buffer,
        )
        .await?;
        if args.ignored {
            for file in &data.ignored_files {
                writeln!(&mut buffer, "!! {}", file.display()).map_err(write_error)?;
            }
        }
        if args.z {
            nul_terminate_lines(&mut buffer);
        }
        writer.write_all(&buffer).map_err(write_error)?;
        return Ok(());
    }

    // Standard human format
    render_human_status(data, args, &mut buffer)?;
    writer.write_all(&buffer).map_err(write_error)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Human standard format
// ---------------------------------------------------------------------------

fn render_human_status(
    data: &StatusData,
    args: &StatusArgs,
    buffer: &mut Vec<u8>,
) -> CliResult<()> {
    let write_error =
        |err: io::Error| CliError::io(format!("failed to write status output: {err}"));

    // Branch header
    match &data.head {
        Head::Detached(commit_hash) => {
            writeln!(buffer, "HEAD detached at {}", &commit_hash.to_string()[..8])
                .map_err(write_error)?;
        }
        Head::Branch(branch) => {
            writeln!(buffer, "On branch {branch}").map_err(write_error)?;
        }
    }

    // Upstream tracking info
    if let Some(upstream) = &data.upstream {
        render_upstream_human(upstream, buffer)?;
    }

    if let Some(repo_state) = &data.repo_state {
        render_repo_state_human(repo_state, buffer)?;
    }

    if let Some(merge_state) = &data.merge_state {
        render_merge_state_human(merge_state, buffer)?;
    }

    if !data.has_commits {
        writeln!(buffer, "\nNo commits yet\n").map_err(write_error)?;
    }

    // Stash info
    if let Some(stash_count) = data.stash_count
        && stash_count > 0
    {
        let entry_text = if stash_count == 1 { "entry" } else { "entries" };
        writeln!(
            buffer,
            "Your stash currently has {stash_count} {entry_text}"
        )
        .map_err(write_error)?;
    }

    // Clean tree
    if data.staged.is_empty() && data.unstaged.is_empty() && data.unmerged_entries.is_empty() {
        writeln!(buffer, "nothing to commit, working tree clean").map_err(write_error)?;
        return Ok(());
    }

    // Staged changes
    if !data.staged.is_empty() {
        writeln!(buffer, "Changes to be committed:").map_err(write_error)?;
        writeln!(
            buffer,
            "  use \"libra restore --staged <file>...\" to unstage"
        )
        .map_err(write_error)?;
        for f in &data.staged.deleted {
            let str = format!("\tdeleted: {}", f.display());
            writeln!(buffer, "{}", str.bright_green()).map_err(write_error)?;
        }
        for f in &data.staged.modified {
            let str = format!("\tmodified: {}", f.display());
            writeln!(buffer, "{}", str.bright_green()).map_err(write_error)?;
        }
        for f in &data.staged.new {
            let str = format!("\tnew file: {}", f.display());
            writeln!(buffer, "{}", str.bright_green()).map_err(write_error)?;
        }
    }

    // Unstaged changes (modified + deleted)
    if !data.unstaged.deleted.is_empty() || !data.unstaged.modified.is_empty() {
        writeln!(buffer, "Changes not staged for commit:").map_err(write_error)?;
        writeln!(
            buffer,
            "  use \"libra add <file>...\" to update what will be committed"
        )
        .map_err(write_error)?;
        writeln!(
            buffer,
            "  use \"libra restore <file>...\" to discard changes in working directory"
        )
        .map_err(write_error)?;
        for f in &data.unstaged.deleted {
            let str = format!("\tdeleted: {}", f.display());
            writeln!(buffer, "{}", str.bright_red()).map_err(write_error)?;
        }
        for f in &data.unstaged.modified {
            let str = format!("\tmodified: {}", f.display());
            writeln!(buffer, "{}", str.bright_red()).map_err(write_error)?;
        }
    }

    // Untracked
    if !data.unstaged.new.is_empty() {
        writeln!(buffer, "Untracked files:").map_err(write_error)?;
        writeln!(
            buffer,
            "  use \"libra add <file>...\" to include in what will be committed"
        )
        .map_err(write_error)?;
        for f in &data.unstaged.new {
            let str = format!("\t{}", f.display());
            writeln!(buffer, "{}", str.bright_red()).map_err(write_error)?;
        }
    }

    // Ignored
    if args.ignored && !data.ignored_files.is_empty() {
        writeln!(buffer, "Ignored files:").map_err(write_error)?;
        writeln!(
            buffer,
            "  (modify .libraignore to change which files are ignored)"
        )
        .map_err(write_error)?;
        for f in &data.ignored_files {
            let str = format!("\t{}", f.display());
            writeln!(buffer, "{}", str.bright_red()).map_err(write_error)?;
        }
    }

    Ok(())
}

fn render_repo_state_human(repo_state: &RepoState, buffer: &mut Vec<u8>) -> CliResult<()> {
    let write_error =
        |err: io::Error| CliError::io(format!("failed to write status output: {err}"));

    match repo_state {
        RepoState::Merge => {}
        RepoState::Rebase { onto, head_name } => {
            let onto_display = if onto.len() > 8 { &onto[..8] } else { onto };
            writeln!(
                buffer,
                "rebase in progress; onto {onto_display} (branch {head_name})"
            )
            .map_err(write_error)?;
            writeln!(buffer, "  (use \"libra rebase --continue\" to resume)")
                .map_err(write_error)?;
            writeln!(
                buffer,
                "  (use \"libra rebase --abort\" to check out the original branch)"
            )
            .map_err(write_error)?;
        }
        RepoState::Bisect => {
            writeln!(buffer, "bisect in progress").map_err(write_error)?;
        }
        RepoState::CherryPick => {
            writeln!(buffer, "cherry-pick in progress").map_err(write_error)?;
            writeln!(buffer, "  (use \"libra cherry-pick --continue\" to resume)")
                .map_err(write_error)?;
            writeln!(buffer, "  (use \"libra cherry-pick --abort\" to cancel)")
                .map_err(write_error)?;
        }
    }
    Ok(())
}

fn render_merge_state_human(merge_state: &MergeStatusInfo, buffer: &mut Vec<u8>) -> CliResult<()> {
    let write_error =
        |err: io::Error| CliError::io(format!("failed to write status output: {err}"));

    writeln!(
        buffer,
        "You are in the middle of a merge with '{}'.",
        merge_state.target_ref
    )
    .map_err(write_error)?;
    if merge_state.conflicted_paths.is_empty() {
        writeln!(
            buffer,
            "  (all conflicts fixed: run \"libra merge --continue\")"
        )
        .map_err(write_error)?;
    } else {
        writeln!(
            buffer,
            "  (fix conflicts and run \"libra merge --continue\")"
        )
        .map_err(write_error)?;
    }
    writeln!(buffer, "  (use \"libra merge --abort\" to abort the merge)").map_err(write_error)?;
    Ok(())
}

fn render_upstream_human(upstream: &UpstreamInfo, buffer: &mut Vec<u8>) -> CliResult<()> {
    let write_error =
        |err: io::Error| CliError::io(format!("failed to write status output: {err}"));

    if upstream.gone {
        writeln!(
            buffer,
            "Your branch is based on '{}', but the upstream is gone.",
            upstream.remote_ref
        )
        .map_err(write_error)?;
        return Ok(());
    }

    // ahead/behind are None on an unborn branch (no local commit to compare).
    let (ahead, behind) = match (upstream.ahead, upstream.behind) {
        (Some(a), Some(b)) => (a, b),
        _ => {
            // Unborn branch: upstream exists but no local commits yet.
            return Ok(());
        }
    };

    if ahead == 0 && behind == 0 {
        writeln!(
            buffer,
            "Your branch is up to date with '{}'.",
            upstream.remote_ref
        )
        .map_err(write_error)?;
    } else if ahead > 0 && behind == 0 {
        writeln!(
            buffer,
            "Your branch is ahead of '{}' by {} commit{}.",
            upstream.remote_ref,
            ahead,
            if ahead == 1 { "" } else { "s" }
        )
        .map_err(write_error)?;
        writeln!(
            buffer,
            "  (use \"libra push\" to publish your local commits)"
        )
        .map_err(write_error)?;
    } else if ahead == 0 && behind > 0 {
        writeln!(
            buffer,
            "Your branch is behind '{}' by {} commit{}.",
            upstream.remote_ref,
            behind,
            if behind == 1 { "" } else { "s" }
        )
        .map_err(write_error)?;
        writeln!(buffer, "  (use \"libra pull\" to update your local branch)")
            .map_err(write_error)?;
    } else {
        writeln!(
            buffer,
            "Your branch and '{}' have diverged,",
            upstream.remote_ref
        )
        .map_err(write_error)?;
        writeln!(
            buffer,
            "and have {ahead} and {behind} different commits each, respectively."
        )
        .map_err(write_error)?;
        writeln!(
            buffer,
            "  (use \"libra pull\" to merge the remote branch into yours)"
        )
        .map_err(write_error)?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// JSON rendering
// ---------------------------------------------------------------------------

fn build_status_json(data: &StatusData, _args: &StatusArgs) -> serde_json::Value {
    let paths_to_json = |paths: &[PathBuf]| -> Vec<serde_json::Value> {
        paths
            .iter()
            .map(|p| serde_json::Value::String(p.display().to_string()))
            .collect()
    };

    let head = match &data.head {
        Head::Branch(name) => serde_json::json!({"type": "branch", "name": name}),
        Head::Detached(hash) => {
            serde_json::json!({"type": "detached", "oid": hash.to_string()})
        }
    };

    let upstream_json = match &data.upstream {
        Some(u) => serde_json::json!({
            "remote_ref": u.remote_ref,
            "ahead": u.ahead,
            "behind": u.behind,
            "gone": u.gone,
        }),
        None => serde_json::Value::Null,
    };

    let mut json_data = serde_json::json!({
        "head": head,
        "has_commits": data.has_commits,
        "upstream": upstream_json,
        "staged": {
            "new": paths_to_json(&data.staged.new),
            "modified": paths_to_json(&data.staged.modified),
            "deleted": paths_to_json(&data.staged.deleted),
        },
        "unstaged": {
            "modified": paths_to_json(&data.unstaged.modified),
            "deleted": paths_to_json(&data.unstaged.deleted),
        },
        "untracked": paths_to_json(&data.unstaged.new),
        "ignored": paths_to_json(&data.ignored_files),
        "renames": data.renames.iter().map(|rename| serde_json::json!({
            "from": rename.from.display().to_string(),
            "to": rename.to.display().to_string(),
            "score": rename.score,
        })).collect::<Vec<_>>(),
        "repo_state": data.repo_state.as_ref().map(RepoState::json_name),
        "is_clean": !data.is_dirty(),
    });

    if let Some(merge_state) = &data.merge_state
        && let Some(map) = json_data.as_object_mut()
    {
        map.insert(
            "merge_state".to_string(),
            serde_json::json!({
                "target_ref": merge_state.target_ref,
                "conflicted_paths": merge_state.conflicted_paths,
            }),
        );
    }

    if let Some(stash_count) = data.stash_count
        && let Some(map) = json_data.as_object_mut()
    {
        map.insert("stash_entries".to_string(), serde_json::json!(stash_count));
    }

    json_data
}

// ---------------------------------------------------------------------------
// Porcelain v1
// ---------------------------------------------------------------------------

/// Rewrite newline entry terminators to NUL for `status -z`. Porcelain output
/// emits one entry per line, so replacing every `\n` yields NUL-terminated
/// records (Git's `-z` machine-readable form).
fn nul_terminate_lines(buffer: &mut [u8]) {
    for byte in buffer.iter_mut() {
        if *byte == b'\n' {
            *byte = 0;
        }
    }
}

pub fn output_porcelain(
    staged: &Changes,
    unstaged: &Changes,
    writer: &mut impl Write,
) -> CliResult<()> {
    let status_list = generate_short_format_status(staged, unstaged);
    for (file, staged_status, unstaged_status) in status_list {
        writeln!(
            writer,
            "{}{} {}",
            staged_status,
            unstaged_status,
            file.display()
        )
        .map_err(|e| CliError::io(format!("failed to write status output: {e}")))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Porcelain v2
// ---------------------------------------------------------------------------

/// File information from HEAD tree for porcelain v2 output.
struct FileInfo {
    mode: u32,
    hash: String,
}

struct PorcelainV2Data {
    index: Index,
    head_tree_items: HashMap<PathBuf, FileInfo>,
}

fn tree_item_mode_to_u32(mode: TreeItemMode) -> u32 {
    match mode {
        TreeItemMode::Blob => 0o100644,
        TreeItemMode::BlobExecutable => 0o100755,
        TreeItemMode::Link => 0o120000,
        TreeItemMode::Tree => 0o040000,
        TreeItemMode::Commit => 0o160000,
    }
}

fn format_mode(mode: u32) -> String {
    format!("{:06o}", mode)
}

fn current_to_workdir(path: &std::path::Path) -> PathBuf {
    let abs_path = util::cur_dir().join(path);
    util::to_workdir_path(&abs_path)
}

#[cfg(unix)]
fn try_get_worktree_mode(file_path: &std::path::Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    let workdir_path = current_to_workdir(file_path);
    let abs_path = util::workdir_to_absolute(&workdir_path);
    let metadata = std::fs::symlink_metadata(&abs_path).ok()?;
    Some(if metadata.file_type().is_symlink() {
        0o120000
    } else if metadata.permissions().mode() & 0o111 != 0 {
        0o100755
    } else {
        0o100644
    })
}

#[cfg(unix)]
fn get_worktree_mode(file_path: &std::path::Path) -> u32 {
    try_get_worktree_mode(file_path).unwrap_or(0o100644)
}

#[cfg(not(unix))]
fn try_get_worktree_mode(file_path: &std::path::Path) -> Option<u32> {
    let workdir_path = current_to_workdir(file_path);
    let abs_path = util::workdir_to_absolute(&workdir_path);
    abs_path.exists().then_some(0o100644)
}

#[cfg(not(unix))]
fn get_worktree_mode(_file_path: &std::path::Path) -> u32 {
    0o100644
}

fn is_submodule_mode(mode: u32) -> bool {
    mode == 0o160000
}

fn get_submodule_status(_file_path: &std::path::Path) -> String {
    "S...".to_string()
}

/// Output porcelain v2 format using metadata collected during status computation.
fn output_porcelain_v2(
    staged: &Changes,
    unstaged: &Changes,
    ignored: &[PathBuf],
    details: StatusRenderDetails<'_>,
    metadata: Option<&PorcelainV2Data>,
    nul_terminated: bool,
    writer: &mut impl Write,
) -> CliResult<()> {
    let metadata =
        metadata.ok_or_else(|| CliError::internal("missing porcelain v2 metadata for status"))?;
    let status_list = build_status_lines(staged, unstaged, details);
    for status in status_list {
        match &status.kind {
            StatusLineKind::Ordinary => {
                write_porcelain_v2_ordinary(&status, metadata, writer)?;
            }
            StatusLineKind::Rename { from } => {
                write_porcelain_v2_rename(
                    &status,
                    from,
                    details.renames,
                    metadata,
                    nul_terminated,
                    writer,
                )?;
            }
            StatusLineKind::Unmerged => {
                if let Some(entry) = details
                    .unmerged_entries
                    .iter()
                    .find(|entry| entry.path == status.path)
                {
                    write_porcelain_v2_unmerged(entry, writer)?;
                } else {
                    return Err(CliError::internal(format!(
                        "missing unmerged metadata for '{}'",
                        status.path.display()
                    )));
                }
            }
        }
    }

    for file in ignored {
        writeln!(writer, "! {}", file.display())
            .map_err(|e| CliError::io(format!("failed to write status output: {e}")))?;
    }
    Ok(())
}

fn write_porcelain_v2_ordinary(
    status: &StatusLine,
    metadata: &PorcelainV2Data,
    writer: &mut impl Write,
) -> CliResult<()> {
    if status.staged_status == '?' && status.unstaged_status == '?' {
        writeln!(writer, "? {}", status.path.display())
            .map_err(|e| CliError::io(format!("failed to write status output: {e}")))?;
        return Ok(());
    }

    let zero_hash = zero_hash_str();
    let workdir_path = current_to_workdir(&status.path);
    let file_str = workdir_path.to_str().unwrap_or_default();

    let (mode_index, hash_index) = if let Some(entry) = metadata.index.get(file_str, 0) {
        (entry.mode, entry.hash.to_string())
    } else {
        (0o100644, zero_hash.clone())
    };

    let (mode_head, hash_head) = if status.staged_status == 'A' {
        (0, zero_hash.clone())
    } else if let Some(info) = metadata.head_tree_items.get(&workdir_path) {
        (info.mode, info.hash.clone())
    } else {
        (0, zero_hash.clone())
    };

    let mode_worktree = if status.unstaged_status == 'D' {
        0
    } else {
        get_worktree_mode(&status.path)
    };
    let sub = porcelain_v2_submodule_status(mode_index, mode_head, &status.path);

    writeln!(
        writer,
        "1 {}{} {} {} {} {} {} {} {}",
        status.staged_status,
        status.unstaged_status,
        sub,
        format_mode(mode_head),
        format_mode(mode_index),
        format_mode(mode_worktree),
        hash_head,
        hash_index,
        status.path.display()
    )
    .map_err(|e| CliError::io(format!("failed to write status output: {e}")))?;
    Ok(())
}

fn write_porcelain_v2_rename(
    status: &StatusLine,
    from: &Path,
    renames: &[RenameEntry],
    metadata: &PorcelainV2Data,
    nul_terminated: bool,
    writer: &mut impl Write,
) -> CliResult<()> {
    let zero_hash = zero_hash_str();
    let to_workdir = current_to_workdir(&status.path);
    let from_workdir = current_to_workdir(from);
    let to_str = to_workdir.to_str().unwrap_or_default();

    let (mode_index, hash_index) = if let Some(entry) = metadata.index.get(to_str, 0) {
        (entry.mode, entry.hash.to_string())
    } else {
        (0o100644, zero_hash.clone())
    };
    let (mode_head, hash_head) = if let Some(info) = metadata.head_tree_items.get(&from_workdir) {
        (info.mode, info.hash.clone())
    } else {
        (0, zero_hash.clone())
    };
    let mode_worktree = if status.unstaged_status == 'D' {
        0
    } else {
        get_worktree_mode(&status.path)
    };
    let sub = porcelain_v2_submodule_status(mode_index, mode_head, &status.path);
    let score = renames
        .iter()
        .find(|rename| rename.from == from && rename.to == status.path)
        .map_or(100, |rename| rename.score);

    if nul_terminated {
        writeln!(
            writer,
            "2 {}{} {} {} {} {} {} {} R{} {}\0{}",
            status.staged_status,
            status.unstaged_status,
            sub,
            format_mode(mode_head),
            format_mode(mode_index),
            format_mode(mode_worktree),
            hash_head,
            hash_index,
            score,
            status.path.display(),
            from.display()
        )
        .map_err(|e| CliError::io(format!("failed to write status output: {e}")))?;
    } else {
        writeln!(
            writer,
            "2 {}{} {} {} {} {} {} {} R{} {}\t{}",
            status.staged_status,
            status.unstaged_status,
            sub,
            format_mode(mode_head),
            format_mode(mode_index),
            format_mode(mode_worktree),
            hash_head,
            hash_index,
            score,
            status.path.display(),
            from.display()
        )
        .map_err(|e| CliError::io(format!("failed to write status output: {e}")))?;
    }
    Ok(())
}

fn write_porcelain_v2_unmerged(entry: &UnmergedEntry, writer: &mut impl Write) -> CliResult<()> {
    let zero_hash = zero_hash_str();
    let mode_worktree = if entry.worktree_mode == 0 {
        "000000".to_string()
    } else {
        format_mode(entry.worktree_mode)
    };
    let stage_mode = |stage: &Option<UnmergedStageInfo>| -> String {
        stage
            .as_ref()
            .map_or_else(|| "000000".to_string(), |info| format_mode(info.mode))
    };
    let stage_hash = |stage: &Option<UnmergedStageInfo>| -> String {
        stage
            .as_ref()
            .map_or_else(|| zero_hash.clone(), |info| info.hash.clone())
    };

    writeln!(
        writer,
        "u {} N... {} {} {} {} {} {} {} {}",
        entry.xy,
        stage_mode(&entry.base),
        stage_mode(&entry.ours),
        stage_mode(&entry.theirs),
        mode_worktree,
        stage_hash(&entry.base),
        stage_hash(&entry.ours),
        stage_hash(&entry.theirs),
        entry.path.display()
    )
    .map_err(|e| CliError::io(format!("failed to write status output: {e}")))?;
    Ok(())
}

fn porcelain_v2_submodule_status(mode_index: u32, mode_head: u32, file_path: &Path) -> String {
    if is_submodule_mode(mode_index) || is_submodule_mode(mode_head) {
        get_submodule_status(file_path)
    } else {
        "N...".to_string()
    }
}

fn zero_hash_str() -> String {
    ObjectHash::zero_str(get_hash_kind())
}

// ---------------------------------------------------------------------------
// Short format
// ---------------------------------------------------------------------------

/// Core logic for generating short format status without color (for testing)
pub fn generate_short_format_status(
    staged: &Changes,
    unstaged: &Changes,
) -> Vec<(std::path::PathBuf, char, char)> {
    let mut file_status: HashMap<PathBuf, (char, char)> = HashMap::new();

    for file in &staged.new {
        file_status.insert(file.clone(), ('A', ' '));
    }
    for file in &staged.modified {
        file_status.insert(file.clone(), ('M', ' '));
    }
    for file in &staged.deleted {
        file_status.insert(file.clone(), ('D', ' '));
    }

    fn process_unstaged_changes(
        files: &[PathBuf],
        file_status: &mut HashMap<PathBuf, (char, char)>,
        unstaged_char: char,
    ) {
        for file in files {
            let staged_status = file_status.get(file).map(|(s, _)| *s);
            if let Some(status) = staged_status {
                file_status.insert(file.clone(), (status, unstaged_char));
            } else {
                file_status.insert(file.clone(), (' ', unstaged_char));
            }
        }
    }

    process_unstaged_changes(&unstaged.modified, &mut file_status, 'M');
    process_unstaged_changes(&unstaged.deleted, &mut file_status, 'D');

    for file in &unstaged.new {
        file_status.insert(file.clone(), ('?', '?'));
    }

    let mut sorted_files: Vec<_> = file_status.iter().collect();
    sorted_files.sort_by(|a, b| a.0.cmp(b.0));

    sorted_files
        .into_iter()
        .map(|(file, (staged_status, unstaged_status))| {
            (file.clone(), *staged_status, *unstaged_status)
        })
        .collect()
}

/// Short format output — legacy public API used by tests.
pub async fn output_short_format(
    staged: &Changes,
    unstaged: &Changes,
    writer: &mut impl Write,
) -> CliResult<()> {
    output_short_format_with_config(staged, unstaged, &OutputConfig::default(), writer).await
}

/// Short format output with color controlled by OutputConfig.
async fn output_short_format_with_config(
    staged: &Changes,
    unstaged: &Changes,
    output: &OutputConfig,
    writer: &mut impl Write,
) -> CliResult<()> {
    let typechanges = TypeChangeSet::default();
    output_short_format_with_details(
        staged,
        unstaged,
        StatusRenderDetails {
            renames: &[],
            typechanges: &typechanges,
            unmerged_entries: &[],
        },
        output,
        false,
        writer,
    )
    .await
}

async fn output_short_format_with_details(
    staged: &Changes,
    unstaged: &Changes,
    details: StatusRenderDetails<'_>,
    output: &OutputConfig,
    nul_terminated: bool,
    writer: &mut impl Write,
) -> CliResult<()> {
    let use_colors = !nul_terminated && should_use_colors(output).await;
    let status_list = build_status_lines(staged, unstaged, details);

    for status in status_list {
        if nul_terminated {
            write_short_status_record_z(&status, writer)?;
            continue;
        }
        let rendered = if use_colors {
            format_colored_status_line(&status)
        } else {
            format_short_status_line(&status)
        };
        writeln!(writer, "{rendered}")
            .map_err(|e| CliError::io(format!("failed to write status output: {e}")))?;
    }
    Ok(())
}

fn write_short_status_record_z(status: &StatusLine, writer: &mut impl Write) -> CliResult<()> {
    match &status.kind {
        StatusLineKind::Rename { from } => {
            write!(
                writer,
                "{}{} {}\0{}\0",
                status.staged_status,
                status.unstaged_status,
                status.path.display(),
                from.display()
            )
        }
        StatusLineKind::Ordinary | StatusLineKind::Unmerged => {
            write!(
                writer,
                "{}{} {}\0",
                status.staged_status,
                status.unstaged_status,
                status.path.display()
            )
        }
    }
    .map_err(|e| CliError::io(format!("failed to write status output: {e}")))?;
    Ok(())
}

fn build_status_lines(
    staged: &Changes,
    unstaged: &Changes,
    details: StatusRenderDetails<'_>,
) -> Vec<StatusLine> {
    let mut skip_paths = HashSet::new();
    for rename in details.renames {
        skip_paths.insert(rename.from.clone());
        skip_paths.insert(rename.to.clone());
    }
    for entry in details.unmerged_entries {
        skip_paths.insert(entry.path.clone());
    }

    let mut lines = Vec::new();
    let mut emitted_paths = HashSet::new();
    for (path, mut staged_status, mut unstaged_status) in
        generate_short_format_status(staged, unstaged)
    {
        if skip_paths.contains(&path) {
            continue;
        }
        if details.typechanges.staged.contains(&path) {
            staged_status = 'T';
        }
        if details.typechanges.unstaged.contains(&path) {
            unstaged_status = 'T';
        }
        lines.push(StatusLine {
            path: path.clone(),
            staged_status,
            unstaged_status,
            kind: StatusLineKind::Ordinary,
        });
        emitted_paths.insert(path);
    }

    for path in details
        .typechanges
        .staged
        .iter()
        .chain(details.typechanges.unstaged.iter())
    {
        if skip_paths.contains(path) || emitted_paths.contains(path) {
            continue;
        }
        lines.push(StatusLine {
            path: path.clone(),
            staged_status: if details.typechanges.staged.contains(path) {
                'T'
            } else {
                ' '
            },
            unstaged_status: if details.typechanges.unstaged.contains(path) {
                'T'
            } else {
                ' '
            },
            kind: StatusLineKind::Ordinary,
        });
        emitted_paths.insert(path.clone());
    }

    for rename in details.renames {
        let mut unstaged_status = rename.unstaged_status;
        if details.typechanges.unstaged.contains(&rename.to) {
            unstaged_status = 'T';
        }
        lines.push(StatusLine {
            path: rename.to.clone(),
            staged_status: 'R',
            unstaged_status,
            kind: StatusLineKind::Rename {
                from: rename.from.clone(),
            },
        });
    }

    for entry in details.unmerged_entries {
        let mut chars = entry.xy.chars();
        let staged_status = chars.next().unwrap_or('U');
        let unstaged_status = chars.next().unwrap_or('U');
        lines.push(StatusLine {
            path: entry.path.clone(),
            staged_status,
            unstaged_status,
            kind: StatusLineKind::Unmerged,
        });
    }

    lines.sort_by(|left, right| left.path.cmp(&right.path));
    lines
}

fn format_short_status_line(status: &StatusLine) -> String {
    match &status.kind {
        StatusLineKind::Rename { from } => format!(
            "{}{} {} -> {}",
            status.staged_status,
            status.unstaged_status,
            from.display(),
            status.path.display()
        ),
        StatusLineKind::Ordinary | StatusLineKind::Unmerged => format!(
            "{}{} {}",
            status.staged_status,
            status.unstaged_status,
            status.path.display()
        ),
    }
}

fn format_colored_status_line(status: &StatusLine) -> String {
    match &status.kind {
        StatusLineKind::Rename { from } => {
            let prefix = format_colored_status_prefix(status.staged_status, status.unstaged_status);
            format!("{} {} -> {}", prefix, from.display(), status.path.display())
        }
        StatusLineKind::Ordinary | StatusLineKind::Unmerged => {
            format_colored_status(status.staged_status, status.unstaged_status, &status.path)
        }
    }
}

fn format_colored_status_prefix(staged_status: char, unstaged_status: char) -> String {
    format_colored_status(staged_status, unstaged_status, Path::new(""))
        .trim_end()
        .to_string()
}

// ---------------------------------------------------------------------------
// Color control — unified with OutputConfig
// ---------------------------------------------------------------------------

/// Check if colors should be used, respecting OutputConfig overrides first,
/// then falling back to config-based / TTY detection.
async fn should_use_colors(output: &OutputConfig) -> bool {
    use std::io::IsTerminal;

    match output.color {
        ColorChoice::Never => return false,
        ColorChoice::Always => return true,
        ColorChoice::Auto => {}
    }

    // Auto: check git-style config, then TTY
    if let Some(color_setting) = ConfigKv::get("color.status.short")
        .await
        .ok()
        .flatten()
        .map(|e| e.value)
    {
        match color_setting.as_str() {
            "always" => return true,
            "never" | "false" => return false,
            "auto" | "true" => return io::stdout().is_terminal(),
            _ => return false,
        }
    }

    if let Some(color_setting) = ConfigKv::get("color.ui")
        .await
        .ok()
        .flatten()
        .map(|e| e.value)
    {
        match color_setting.as_str() {
            "always" => return true,
            "never" | "false" => return false,
            "auto" | "true" => return io::stdout().is_terminal(),
            _ => return false,
        }
    }

    io::stdout().is_terminal()
}

fn format_colored_status(
    staged_status: char,
    unstaged_status: char,
    file: &std::path::Path,
) -> String {
    use colored::Colorize;

    let colored_staged = match staged_status {
        'A' => staged_status.to_string().green(),
        'M' => staged_status.to_string().green(),
        'D' => staged_status.to_string().red(),
        'R' => staged_status.to_string().yellow(),
        'C' => staged_status.to_string().yellow(),
        'U' => staged_status.to_string().red(),
        '?' => staged_status.to_string().bright_red(),
        ' ' => staged_status.to_string().into(),
        _ => staged_status.to_string().into(),
    };

    let colored_unstaged = match unstaged_status {
        'M' => unstaged_status.to_string().red(),
        'D' => unstaged_status.to_string().red(),
        'U' => unstaged_status.to_string().red(),
        '?' => unstaged_status.to_string().bright_red(),
        '!' => unstaged_status.to_string().bright_red(),
        ' ' => unstaged_status.to_string().into(),
        _ => unstaged_status.to_string().into(),
    };

    format!("{}{} {}", colored_staged, colored_unstaged, file.display())
}

// ---------------------------------------------------------------------------
// Branch info helpers (short / porcelain)
// ---------------------------------------------------------------------------

/// Print branch info line for short / porcelain v1 `--branch`.
fn print_branch_info(
    head: &Head,
    upstream: Option<&UpstreamInfo>,
    writer: &mut impl Write,
) -> CliResult<()> {
    match head {
        Head::Detached(commit_hash) => {
            writeln!(
                writer,
                "## HEAD (detached at {})",
                &commit_hash.to_string()[..8]
            )
            .map_err(|e| CliError::io(format!("failed to write status output: {e}")))?;
        }
        Head::Branch(branch) => {
            if let Some(u) = upstream {
                let tracking = format!("{}...{}", branch, u.remote_ref);
                if u.gone {
                    writeln!(writer, "## {tracking} [gone]")
                        .map_err(|e| CliError::io(format!("failed to write status output: {e}")))?;
                } else {
                    let ahead = u.ahead.unwrap_or(0);
                    let behind = u.behind.unwrap_or(0);
                    if ahead > 0 && behind > 0 {
                        writeln!(writer, "## {tracking} [ahead {ahead}, behind {behind}]")
                            .map_err(|e| {
                                CliError::io(format!("failed to write status output: {e}"))
                            })?;
                    } else if ahead > 0 {
                        writeln!(writer, "## {tracking} [ahead {ahead}]").map_err(|e| {
                            CliError::io(format!("failed to write status output: {e}"))
                        })?;
                    } else if behind > 0 {
                        writeln!(writer, "## {tracking} [behind {behind}]").map_err(|e| {
                            CliError::io(format!("failed to write status output: {e}"))
                        })?;
                    } else {
                        writeln!(writer, "## {tracking}").map_err(|e| {
                            CliError::io(format!("failed to write status output: {e}"))
                        })?;
                    }
                }
            } else {
                writeln!(writer, "## {branch}")
                    .map_err(|e| CliError::io(format!("failed to write status output: {e}")))?;
            }
        }
    }
    Ok(())
}

/// Write branch information in porcelain v2 style.
fn write_branch_info_v2(
    head: &Head,
    head_oid: Option<&ObjectHash>,
    upstream: Option<&UpstreamInfo>,
    writer: &mut impl Write,
) -> CliResult<()> {
    let write_err = |e: io::Error| CliError::io(format!("failed to write status output: {e}"));

    match head {
        Head::Detached(_) => {
            writeln!(writer, "# branch.head (detached)").map_err(write_err)?;
        }
        Head::Branch(name) => {
            writeln!(writer, "# branch.head {}", name).map_err(write_err)?;
        }
    }

    if let Some(oid) = head_oid {
        writeln!(writer, "# branch.oid {oid}").map_err(write_err)?;
    } else {
        writeln!(writer, "# branch.oid (initial)").map_err(write_err)?;
    }

    if let Some(u) = upstream {
        writeln!(writer, "# branch.upstream {}", u.remote_ref).map_err(write_err)?;
        if !u.gone {
            let ahead = u.ahead.unwrap_or(0);
            let behind = u.behind.unwrap_or(0);
            writeln!(writer, "# branch.ab +{ahead} -{behind}").map_err(write_err)?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Upstream tracking resolution
// ---------------------------------------------------------------------------

fn status_branch_store_error(context: &str, error: BranchStoreError) -> CliError {
    match error {
        BranchStoreError::Query(detail) => {
            CliError::fatal(format!("failed to {context}: {detail}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        }
        other => CliError::fatal(format!("failed to {context}: {other}"))
            .with_stable_code(StableErrorCode::RepoCorrupt),
    }
}

fn status_config_read_error(context: &str, error: anyhow::Error) -> CliError {
    CliError::fatal(format!("failed to {context}: {error}"))
        .with_stable_code(StableErrorCode::IoReadFailed)
}

async fn resolve_upstream_info(
    head: &Head,
    local_commit: Option<&ObjectHash>,
) -> CliResult<Option<UpstreamInfo>> {
    let branch_name = match head {
        Head::Branch(name) => name.clone(),
        Head::Detached(_) => return Ok(None),
    };

    let branch_config = match ConfigKv::branch_config(&branch_name).await {
        Ok(Some(config)) => config,
        Ok(None) => return Ok(None),
        Err(error) => {
            return Err(status_config_read_error(
                &format!("read branch configuration for '{branch_name}'"),
                error,
            ));
        }
    };

    let remote = &branch_config.remote;
    let merge_branch = &branch_config.merge;
    let remote_ref_display = format!("{remote}/{merge_branch}");

    let tracking_branch = Branch::find_branch_result(merge_branch, Some(remote))
        .await
        .map_err(|error| status_branch_store_error("resolve upstream branch", error))?;

    let tracking_commit = match tracking_branch {
        Some(b) => b.commit,
        None => {
            // Upstream configured but tracking ref doesn't exist → gone
            return Ok(Some(UpstreamInfo {
                remote_ref: remote_ref_display,
                ahead: None,
                behind: None,
                gone: true,
            }));
        }
    };

    let local_commit = match local_commit {
        Some(commit) => commit,
        None => {
            // Unborn branch: no local commit to compare against.
            // Return None for ahead/behind — numeric counts would imply
            // a comparison that never happened.
            return Ok(Some(UpstreamInfo {
                remote_ref: remote_ref_display,
                ahead: None,
                behind: None,
                gone: false,
            }));
        }
    };

    let (ahead, behind) = compute_ahead_behind(local_commit, &tracking_commit);

    Ok(Some(UpstreamInfo {
        remote_ref: remote_ref_display,
        ahead: Some(ahead),
        behind: Some(behind),
        gone: false,
    }))
}

/// Compute the number of commits ahead/behind between two refs.
///
/// Performs a bidirectional BFS from both tips, classifying each commit as
/// local-only, remote-only, or common (reachable from both sides).  Once a
/// commit is found from the opposite side it is reclassified as common and
/// its ancestors are not enqueued again, which reduces redundant work when
/// the histories share a recent merge-base.
///
/// **Complexity**: proportional to the number of commits reachable from
/// both tips until the queues are drained.  For disjoint histories (no
/// common ancestor) this visits all reachable commits from both sides.
/// Falls back gracefully when a commit object is missing or corrupt
/// (e.g. shallow clone) by stopping traversal on that branch.
fn compute_ahead_behind(local: &ObjectHash, remote: &ObjectHash) -> (usize, usize) {
    if local == remote {
        return (0, 0);
    }

    let mut local_only: HashSet<ObjectHash> = HashSet::new();
    let mut remote_only: HashSet<ObjectHash> = HashSet::new();
    let mut common: HashSet<ObjectHash> = HashSet::new();
    let mut local_queue: VecDeque<ObjectHash> = VecDeque::new();
    let mut remote_queue: VecDeque<ObjectHash> = VecDeque::new();

    local_queue.push_back(*local);
    remote_queue.push_back(*remote);

    while !local_queue.is_empty() || !remote_queue.is_empty() {
        // Expand one commit from the local side.
        if let Some(hash) = local_queue.pop_front() {
            if common.contains(&hash) {
                // Already common — skip without expanding parents.
                continue;
            } else if remote_only.remove(&hash) {
                // Discovered from the remote side too → merge-base.
                common.insert(hash);
            } else if local_only.insert(hash)
                && let Some(commit) = Commit::try_load(&hash)
            {
                for parent in &commit.parent_commit_ids {
                    if !common.contains(parent) {
                        local_queue.push_back(*parent);
                    }
                }
            }
        }

        // Expand one commit from the remote side.
        if let Some(hash) = remote_queue.pop_front() {
            if common.contains(&hash) {
                continue;
            } else if local_only.remove(&hash) {
                common.insert(hash);
            } else if remote_only.insert(hash)
                && let Some(commit) = Commit::try_load(&hash)
            {
                for parent in &commit.parent_commit_ids {
                    if !common.contains(parent) {
                        remote_queue.push_back(*parent);
                    }
                }
            }
        }
    }

    (local_only.len(), remote_only.len())
}

// ---------------------------------------------------------------------------
// Bare repository detection
// ---------------------------------------------------------------------------

async fn is_bare_repository() -> bool {
    matches!(
        ConfigKv::get("core.bare").await.ok().flatten().map(|e| e.value),
        Some(value) if value.eq_ignore_ascii_case("true")
    )
}

// ---------------------------------------------------------------------------
// Untracked directory collapsing
// ---------------------------------------------------------------------------

fn collapse_untracked_directories(untracked_files: Vec<PathBuf>, index: &Index) -> Vec<PathBuf> {
    use std::collections::BTreeSet;

    if untracked_files.is_empty() {
        return untracked_files;
    }

    let mut dir_files: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    let mut root_files: Vec<PathBuf> = Vec::new();

    for file in &untracked_files {
        let components: Vec<_> = file.components().collect();
        if components.len() > 1 {
            let top_dir = PathBuf::from(components[0].as_os_str());
            dir_files.entry(top_dir).or_default().push(file.clone());
        } else {
            root_files.push(file.clone());
        }
    }

    let mut result: BTreeSet<PathBuf> = BTreeSet::new();

    for file in root_files {
        result.insert(file);
    }

    for (dir, files) in dir_files {
        let dir_prefix = format!("{}/", dir.display());
        let has_tracked_files = index.tracked_files().iter().any(|f| {
            f.to_str()
                .map(|s| s.starts_with(&dir_prefix))
                .unwrap_or(false)
        });

        if has_tracked_files {
            for file in files {
                result.insert(file);
            }
        } else {
            let dir_str = format!("{}/", dir.display());
            let dir_path = PathBuf::from(dir_str);
            result.insert(dir_path);
        }
    }

    result.into_iter().collect()
}

// ---------------------------------------------------------------------------
// Clean check
// ---------------------------------------------------------------------------

/// Check if the working tree is clean.
///
/// Returns `false` when the status cannot be determined (e.g. corrupt index).
pub async fn is_clean() -> bool {
    let staged = match changes_to_be_committed_safe().await {
        Ok(c) => c,
        Err(err) => {
            tracing::error!("failed to calculate committed changes: {err}");
            return false;
        }
    };
    let unstaged = match changes_to_be_staged() {
        Ok(c) => c,
        Err(err) => {
            tracing::error!("failed to calculate staged changes: {err}");
            return false;
        }
    };
    staged.is_empty() && unstaged.is_empty()
}

// ---------------------------------------------------------------------------
// Status computation (public API preserved)
// ---------------------------------------------------------------------------

/// Convenience wrapper around [`changes_to_be_committed_safe`].
///
/// On error (e.g. corrupt index), logs the failure and returns an empty
/// [`Changes`] set instead of panicking.
pub async fn changes_to_be_committed() -> Changes {
    match changes_to_be_committed_safe().await {
        Ok(changes) => changes,
        Err(err) => {
            tracing::error!("changes_to_be_committed failed: {err}");
            Changes::default()
        }
    }
}

pub async fn changes_to_be_committed_safe() -> Result<Changes, StatusError> {
    let mut changes = Changes::default();
    let index_path = path::try_index().map_err(|source| StatusError::Workdir { source })?;
    let index = Index::load(&index_path).map_err(|source| StatusError::IndexLoad {
        path: index_path.clone(),
        source,
    })?;
    let head_commit = Head::current_commit().await;
    let tracked_files = index.tracked_files();

    if head_commit.is_none() {
        changes.new = tracked_files;
        return Ok(changes);
    }

    let head_commit = match head_commit {
        Some(head_commit) => head_commit,
        None => return Ok(changes),
    };
    let commit = Commit::load(&head_commit);
    let tree = Tree::load(&commit.tree_id);
    let tree_files = tree.get_plain_items();

    for (item_path, item_hash) in tree_files.iter() {
        let item_str = item_path
            .to_str()
            .ok_or_else(|| StatusError::InvalidPathEncoding {
                path: item_path.clone(),
            })?;
        if index.tracked(item_str, 0) {
            if !index.verify_hash(item_str, 0, item_hash) {
                changes.modified.push(item_path.clone());
            }
        } else {
            changes.deleted.push(item_path.clone());
        }
    }
    let tree_files_set: HashSet<PathBuf> = tree_files.into_iter().map(|(path, _)| path).collect();
    changes.new = tracked_files
        .into_iter()
        .filter(|path| !tree_files_set.contains(path))
        .collect();

    Ok(changes)
}

/// Compare the difference between `index` and the `workdir` using the default ignore rules.
pub fn changes_to_be_staged() -> Result<Changes, StatusError> {
    changes_to_be_staged_with_policy(IgnorePolicy::Respect)
}

fn changes_to_be_staged_for_status(untracked_mode: UntrackedFiles) -> Result<Changes, StatusError> {
    let workdir = util::try_working_dir().map_err(|source| StatusError::Workdir { source })?;
    let index_path = path::try_index().map_err(|source| StatusError::Workdir { source })?;
    let index = Index::load(&index_path).map_err(|source| StatusError::IndexLoad {
        path: index_path.clone(),
        source,
    })?;
    let mut visible = collect_tracked_worktree_changes(&workdir, &index)?;

    match untracked_mode {
        UntrackedFiles::No => {}
        UntrackedFiles::Normal => {
            let files = list_workdir_files_normal_safe(&workdir, &index).map_err(|source| {
                StatusError::ListWorkdirFiles {
                    path: workdir.clone(),
                    source,
                }
            })?;
            push_untracked_files(&mut visible, files, &index)?;
        }
        UntrackedFiles::All => {
            let (files, _) = list_workdir_files_split_safe(&workdir).map_err(|source| {
                StatusError::ListWorkdirFiles {
                    path: workdir.clone(),
                    source,
                }
            })?;
            push_untracked_files(&mut visible, files, &index)?;
        }
    }

    Ok(visible)
}

/// Variant of [`changes_to_be_staged`] that lets callers pick the ignore strategy explicitly.
/// Commands such as `add --force` or `status --ignored` can switch policies as needed.
pub fn changes_to_be_staged_with_policy(policy: IgnorePolicy) -> Result<Changes, StatusError> {
    let workdir = util::try_working_dir().map_err(|source| StatusError::Workdir { source })?;
    let index_path = path::try_index().map_err(|source| StatusError::Workdir { source })?;
    let index = Index::load(&index_path).map_err(|source| StatusError::IndexLoad {
        path: index_path.clone(),
        source,
    })?;
    let (mut visible, ignored) = changes_to_be_staged_split_with_index(&workdir, &index)?;
    match policy {
        IgnorePolicy::Respect => Ok(visible),
        IgnorePolicy::OnlyIgnored => Ok(ignored),
        IgnorePolicy::IncludeIgnored => {
            visible.extend(ignored);
            Ok(visible)
        }
    }
}

pub fn changes_to_be_staged_split_safe() -> Result<(Changes, Changes), StatusError> {
    let workdir = util::try_working_dir().map_err(|source| StatusError::Workdir { source })?;
    let index_path = path::try_index().map_err(|source| StatusError::Workdir { source })?;
    let index = Index::load(&index_path).map_err(|source| StatusError::IndexLoad {
        path: index_path.clone(),
        source,
    })?;
    changes_to_be_staged_split_with_index(&workdir, &index)
}

/// List changes to be staged with --force semantics (recurse into ignored directories)
pub fn changes_to_be_staged_split_force() -> Result<(Changes, Changes), StatusError> {
    let workdir = util::try_working_dir().map_err(|source| StatusError::Workdir { source })?;
    let index_path = path::try_index().map_err(|source| StatusError::Workdir { source })?;
    let index = Index::load(&index_path).map_err(|source| StatusError::IndexLoad {
        path: index_path.clone(),
        source,
    })?;
    changes_to_be_staged_split_force_with_index(&workdir, &index)
}

fn changes_to_be_staged_split_force_with_index(
    workdir: &PathBuf,
    index: &Index,
) -> Result<(Changes, Changes), StatusError> {
    let mut visible = Changes::default();
    let mut ignored = Changes::default();
    let tracked_files = index.tracked_files();
    for file in tracked_files.iter() {
        let file_str = file
            .to_str()
            .ok_or_else(|| StatusError::InvalidPathEncoding { path: file.clone() })?;
        let file_abs = workdir.join(file);
        if !file_abs.exists() {
            visible.deleted.push(file.clone());
        } else if index.is_modified(file_str, 0, workdir) {
            let file_hash =
                calc_file_blob_hash(&file_abs).map_err(|source| StatusError::FileHash {
                    path: file_abs.clone(),
                    source,
                })?;
            if !index.verify_hash(file_str, 0, &file_hash) {
                visible.modified.push(file.clone());
            }
        }
    }
    let (files, ignored_files) = list_workdir_files_split_force(workdir).map_err(|source| {
        StatusError::ListWorkdirFiles {
            path: workdir.clone(),
            source,
        }
    })?;
    for file in files {
        let file_str = file
            .to_str()
            .ok_or_else(|| StatusError::InvalidPathEncoding { path: file.clone() })?;
        if !index.tracked(file_str, 0) {
            visible.new.push(file);
        }
    }
    for file in ignored_files {
        let file_str = file
            .to_str()
            .ok_or_else(|| StatusError::InvalidPathEncoding { path: file.clone() })?;
        if !index.tracked(file_str, 0) {
            ignored.new.push(file);
        }
    }
    Ok((visible, ignored))
}

fn changes_to_be_staged_split_with_index(
    workdir: &PathBuf,
    index: &Index,
) -> Result<(Changes, Changes), StatusError> {
    let mut visible = collect_tracked_worktree_changes(workdir, index)?;
    let mut ignored = Changes::default();
    let (files, ignored_files) =
        list_workdir_files_split_safe(workdir).map_err(|source| StatusError::ListWorkdirFiles {
            path: workdir.clone(),
            source,
        })?;
    push_untracked_files(&mut visible, files, index)?;
    push_untracked_files(&mut ignored, ignored_files, index)?;
    Ok((visible, ignored))
}

fn collect_tracked_worktree_changes(workdir: &Path, index: &Index) -> Result<Changes, StatusError> {
    let mut changes = Changes::default();
    let tracked_files = index.tracked_files();
    for file in &tracked_files {
        let file_str = file
            .to_str()
            .ok_or_else(|| StatusError::InvalidPathEncoding { path: file.clone() })?;
        let file_abs = workdir.join(file);
        if !file_abs.exists() {
            changes.deleted.push(file.clone());
        } else if index.is_modified(file_str, 0, workdir) {
            let file_hash =
                calc_file_blob_hash(&file_abs).map_err(|source| StatusError::FileHash {
                    path: file_abs.clone(),
                    source,
                })?;
            if !index.verify_hash(file_str, 0, &file_hash) {
                changes.modified.push(file.clone());
            }
        }
    }
    Ok(changes)
}

fn push_untracked_files(
    changes: &mut Changes,
    files: Vec<PathBuf>,
    index: &Index,
) -> Result<(), StatusError> {
    for file in files {
        let file_str = file
            .to_str()
            .ok_or_else(|| StatusError::InvalidPathEncoding { path: file.clone() })?;
        if !index.tracked(file_str, 0) {
            changes.new.push(file);
        }
    }
    Ok(())
}

fn list_workdir_files_split_safe(workdir: &PathBuf) -> io::Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    let mut files = Vec::new();
    let mut ignored = Vec::new();
    let mut pending_dirs = vec![workdir.clone()];

    while let Some(dir) = pending_dirs.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if should_skip_status_workdir_entry(&entry.file_name()) {
                continue;
            }

            let file_type = entry.file_type()?;
            let relative = path
                .strip_prefix(workdir)
                .map_err(|err| io::Error::other(err.to_string()))?
                .to_path_buf();
            if file_type.is_dir() {
                if util::check_gitignore(workdir, &path) {
                    ignored.push(relative);
                } else {
                    pending_dirs.push(path);
                }
            } else if file_type.is_file() {
                if util::check_gitignore(workdir, &path) {
                    ignored.push(relative);
                } else {
                    files.push(relative);
                }
            }
        }
    }

    Ok((files, ignored))
}

fn list_workdir_files_normal_safe(workdir: &PathBuf, index: &Index) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut pending_dirs = vec![workdir.clone()];

    while let Some(dir) = pending_dirs.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if should_skip_status_workdir_entry(&entry.file_name()) {
                continue;
            }

            let file_type = entry.file_type()?;
            let relative = path
                .strip_prefix(workdir)
                .map_err(|err| io::Error::other(err.to_string()))?
                .to_path_buf();
            if file_type.is_dir() {
                if util::check_gitignore(workdir, &path) {
                    continue;
                }
                if untracked_dir_has_tracked_descendant(index, &relative) {
                    pending_dirs.push(path);
                } else {
                    files.push(path_with_trailing_separator(&relative));
                }
            } else if file_type.is_file() && !util::check_gitignore(workdir, &path) {
                files.push(relative);
            }
        }
    }

    Ok(files)
}

fn untracked_dir_has_tracked_descendant(index: &Index, dir: &Path) -> bool {
    let prefix = format!("{}/", dir.display());
    index.tracked_files().iter().any(|tracked| {
        tracked
            .to_str()
            .map(|path| path.starts_with(&prefix))
            .unwrap_or(false)
    })
}

fn path_with_trailing_separator(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}/", path.display()))
}

fn should_skip_status_workdir_entry(file_name: &std::ffi::OsStr) -> bool {
    if file_name == std::ffi::OsStr::new(util::ROOT_DIR) {
        return true;
    }

    std::env::var_os(crate::utils::pager::LIBRA_TEST_ENV).is_some()
        && file_name == std::ffi::OsStr::new(".libra-test-home")
}

/// List workdir files with --force semantics: recurse into ignored directories
/// and include their files in the ignored list
fn list_workdir_files_split_force(workdir: &PathBuf) -> io::Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    let mut files = Vec::new();
    let mut ignored = Vec::new();
    let mut pending_dirs = vec![workdir.clone()];

    while let Some(dir) = pending_dirs.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if should_skip_status_workdir_entry(&entry.file_name()) {
                continue;
            }

            let file_type = entry.file_type()?;
            let relative = path
                .strip_prefix(workdir)
                .map_err(|err| io::Error::other(err.to_string()))?
                .to_path_buf();
            if file_type.is_dir() {
                // Always recurse into directories, even ignored ones.
                // We never push the directory entry itself — only its files
                // — so `add --force` sees concrete blobs, not a path that
                // would panic when `Blob::from_file` tries to read it.
                pending_dirs.push(path.clone());
            } else if file_type.is_file() {
                if util::check_gitignore(workdir, &path) {
                    ignored.push(relative);
                } else {
                    files.push(relative);
                }
            }
        }
    }

    Ok((files, ignored))
}

/// List ignored files (not tracked by index, but ignored by .libraignore) under workdir
pub fn list_ignored_files() -> Result<Changes, StatusError> {
    changes_to_be_staged_with_policy(IgnorePolicy::OnlyIgnored)
}

#[cfg(test)]
mod test {
    use sea_orm::{ConnectionTrait, Statement};
    use serial_test::serial;
    use tempfile::tempdir;

    use super::*;
    use crate::{
        internal::db::{get_db_conn_instance, reset_db_conn_instance_for_path},
        utils::{
            error::StableErrorCode,
            test::{self, ChangeDirGuard},
        },
    };

    /// Pin the `Display` format for the static-message variants of
    /// [`StatusError`]. Only `InvalidPathEncoding` has a fully static
    /// pattern — the others are all source-chained (`{source}`) and
    /// owned by their wrapped error type, so they're intentionally
    /// skipped. The CliError mapping above prefixes "failed to determine
    /// working tree status: " in front of every variant before sending
    /// it to the human / --json envelope, so direct-Display matters
    /// less for this enum than for typed errors with more variants.
    #[test]
    fn status_error_display_pins_invalid_path_encoding_variant() {
        assert_eq!(
            StatusError::InvalidPathEncoding {
                path: PathBuf::from("src/foo"),
            }
            .to_string(),
            "path 'src/foo' is not valid UTF-8",
        );
    }

    #[test]
    fn list_workdir_files_prunes_ignored_directories() {
        let repo = tempdir().expect("failed to create temp repo");
        let workdir = repo.path().to_path_buf();
        std::fs::write(workdir.join(".libraignore"), "ignored-dir/\n")
            .expect("failed to write ignore file");
        std::fs::create_dir_all(workdir.join("ignored-dir/nested"))
            .expect("failed to create ignored directory");
        std::fs::write(workdir.join("ignored-dir/nested/file.txt"), "ignored")
            .expect("failed to write ignored file");
        std::fs::write(workdir.join("visible.txt"), "visible").expect("failed to write file");

        let (visible, ignored) =
            list_workdir_files_split_safe(&workdir).expect("failed to list workdir files");

        assert!(visible.contains(&PathBuf::from(".libraignore")));
        assert!(visible.contains(&PathBuf::from("visible.txt")));
        assert!(ignored.contains(&PathBuf::from("ignored-dir")));
        assert!(!visible.contains(&PathBuf::from("ignored-dir/nested/file.txt")));
        assert!(!ignored.contains(&PathBuf::from("ignored-dir/nested/file.txt")));
    }

    #[tokio::test]
    #[serial]
    async fn resolve_upstream_info_surfaces_branch_config_query_failures() {
        let repo = tempdir().expect("failed to create temp repo");
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = ChangeDirGuard::new(repo.path());
        let db_path = repo.path().join(".libra").join("libra.db");

        let db = get_db_conn_instance().await;
        db.execute(Statement::from_string(
            db.get_database_backend(),
            "DROP TABLE config_kv",
        ))
        .await
        .expect("dropping config_kv table should succeed");

        let err = resolve_upstream_info(&Head::Branch("main".to_string()), None)
            .await
            .expect_err("missing config_kv table should surface as an error");

        assert_eq!(err.stable_code(), StableErrorCode::IoReadFailed);
        assert!(
            err.to_string()
                .contains("failed to read branch configuration for 'main'"),
            "unexpected error: {err}"
        );

        reset_db_conn_instance_for_path(&db_path).await;
    }
}
