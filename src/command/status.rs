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
            commit::Commit,
            tree::{Tree, TreeItemMode},
        },
    },
};
use serde::Serialize;

use super::stash;
use crate::{
    command::calc_file_blob_hash,
    internal::{
        branch::{Branch, BranchStoreError},
        config::ConfigKv,
        head::Head,
    },
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        ignore::IgnorePolicy,
        object_ext::{CommitExt, TreeExt},
        output::{ColorChoice, OutputConfig, emit_json_data},
        path, util,
    },
};

// ---------------------------------------------------------------------------
// Args & enums
// ---------------------------------------------------------------------------

/// Show the working tree status.
///
/// EXAMPLES:
///     libra status                       Show working tree status
///     libra status -s                    Short format output
///     libra status --porcelain           Machine-readable output (v1)
///     libra status --porcelain v2        Extended machine-readable output
///     libra status --branch              Include branch info in short/porcelain
///     libra status --show-stash          Show stash count
///     libra status --ignored             Include ignored files
///     libra status --untracked-files=no  Hide untracked files
///     libra status --json                Structured JSON output for agents
///     libra status --exit-code           Exit 1 if working tree is dirty
///     libra status --quiet --exit-code   Silent dirty check for scripts
#[derive(Parser, Debug, Default)]
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
    #[clap(
        long = "untracked-files",
        value_name = "MODE",
        default_value = "normal"
    )]
    pub untracked_files: UntrackedFiles,

    /// Exit with code 1 if the working tree has changes.
    /// Can be combined with --quiet for silent dirty checking.
    #[clap(long = "exit-code")]
    pub exit_code: bool,
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
                *paths = paths.iter().map(util::workdir_to_current).collect();
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
    stash_count: Option<usize>,
    upstream: Option<UpstreamInfo>,
    porcelain_v2: Option<PorcelainV2Data>,
}

impl StatusData {
    fn is_dirty(&self) -> bool {
        !self.staged.is_empty() || !self.unstaged.is_empty()
    }
}

/// Collect all status data in one pass, eliminating duplicate computation
/// between human/JSON/short/porcelain renderers.
async fn collect_status_data(args: &StatusArgs) -> CliResult<StatusData> {
    if is_bare_repository().await {
        return Err(CliError::fatal("this operation must be run in a work tree")
            .with_stable_code(StableErrorCode::RepoStateInvalid)
            .with_hint("this command requires a working tree; bare repositories do not have one"));
    }

    let head = Head::current().await;
    let head_oid = Head::current_commit().await;
    let has_commits = head_oid.is_some();

    let staged = changes_to_be_committed_safe()
        .await
        .map(|c| c.to_relative())
        .map_err(CliError::from)?;
    let mut unstaged = changes_to_be_staged()
        .map(|c| c.to_relative())
        .map_err(CliError::from)?;
    let mut ignored_files = if args.ignored && !matches!(args.untracked_files, UntrackedFiles::No) {
        list_ignored_files()
            .map(|c| c.to_relative().new)
            .map_err(CliError::from)?
    } else {
        vec![]
    };
    let needs_index = matches!(args.untracked_files, UntrackedFiles::Normal)
        || matches!(args.porcelain, Some(PorcelainVersion::V2));
    let mut maybe_index = if needs_index {
        Some(load_status_index()?)
    } else {
        None
    };

    // Apply untracked-files filter
    match args.untracked_files {
        UntrackedFiles::No => {
            unstaged.new.clear();
            ignored_files.clear();
        }
        UntrackedFiles::Normal => {
            let index = maybe_index
                .as_ref()
                .ok_or_else(|| CliError::internal("status index should be loaded"))?;
            unstaged.new = collapse_untracked_directories(unstaged.new, index);
            ignored_files = collapse_untracked_directories(ignored_files, index);
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
    let porcelain_v2 = if matches!(args.porcelain, Some(PorcelainVersion::V2)) {
        let index = maybe_index
            .take()
            .ok_or_else(|| CliError::internal("porcelain v2 metadata should be loaded"))?;
        Some(build_porcelain_v2_data(index, head_oid.as_ref()))
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
        stash_count,
        upstream,
        porcelain_v2,
    })
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

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

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

    let data = collect_status_data(&args).await?;

    if output.is_json() {
        let json_data = build_status_json(&data, &args);
        emit_json_data("status", &json_data, output)?;
    } else if !output.quiet {
        let mut stdout = std::io::stdout();
        render_status_to_writer(&data, &args, output, &mut stdout).await?;
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

    let data = collect_status_data(&args).await?;
    let output = OutputConfig::default();
    render_status_to_writer(&data, &args, &output, writer).await
}

// ---------------------------------------------------------------------------
// Rendering dispatcher
// ---------------------------------------------------------------------------

async fn render_status_to_writer(
    data: &StatusData,
    args: &StatusArgs,
    output: &OutputConfig,
    writer: &mut impl Write,
) -> CliResult<()> {
    let write_error =
        |err: io::Error| CliError::io(format!("failed to write status output: {err}"));
    let mut buffer = Vec::new();

    // Porcelain modes
    match args.porcelain {
        Some(PorcelainVersion::V2) => {
            if args.branch {
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
                data.porcelain_v2.as_ref(),
                &mut buffer,
            )?;
            writer.write_all(&buffer).map_err(write_error)?;
            return Ok(());
        }
        Some(PorcelainVersion::V1) => {
            if args.branch {
                print_branch_info(&data.head, data.upstream.as_ref(), &mut buffer)?;
            }
            output_porcelain(&data.staged, &data.unstaged, &mut buffer)?;
            if args.ignored && !data.ignored_files.is_empty() {
                for file in &data.ignored_files {
                    writeln!(&mut buffer, "!! {}", file.display()).map_err(write_error)?;
                }
            }
            writer.write_all(&buffer).map_err(write_error)?;
            return Ok(());
        }
        None => {}
    };

    // Short format
    if args.short {
        if args.branch {
            print_branch_info(&data.head, data.upstream.as_ref(), &mut buffer)?;
        }
        output_short_format_with_config(&data.staged, &data.unstaged, output, &mut buffer).await?;
        if args.ignored {
            for file in &data.ignored_files {
                writeln!(&mut buffer, "!! {}", file.display()).map_err(write_error)?;
            }
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
    if data.staged.is_empty() && data.unstaged.is_empty() {
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
        "is_clean": !data.is_dirty(),
    });

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
fn get_worktree_mode(file_path: &std::path::Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    let workdir_path = current_to_workdir(file_path);
    let abs_path = util::workdir_to_absolute(&workdir_path);
    if let Ok(metadata) = std::fs::symlink_metadata(&abs_path) {
        if metadata.file_type().is_symlink() {
            0o120000
        } else if metadata.permissions().mode() & 0o111 != 0 {
            0o100755
        } else {
            0o100644
        }
    } else {
        0o100644
    }
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

fn build_porcelain_v2_data(index: Index, head_oid: Option<&ObjectHash>) -> PorcelainV2Data {
    let head_tree_items = if let Some(commit_hash) = head_oid {
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
    };

    PorcelainV2Data {
        index,
        head_tree_items,
    }
}

/// Output porcelain v2 format using metadata collected during status computation.
fn output_porcelain_v2(
    staged: &Changes,
    unstaged: &Changes,
    ignored: &[PathBuf],
    metadata: Option<&PorcelainV2Data>,
    writer: &mut impl Write,
) -> CliResult<()> {
    let metadata =
        metadata.ok_or_else(|| CliError::internal("missing porcelain v2 metadata for status"))?;
    let zero_hash = zero_hash_str();

    let status_list = generate_short_format_status(staged, unstaged);
    for (file, staged_status, unstaged_status) in status_list {
        if staged_status == '?' && unstaged_status == '?' {
            writeln!(writer, "? {}", file.display())
                .map_err(|e| CliError::io(format!("failed to write status output: {e}")))?;
            continue;
        }

        let workdir_path = current_to_workdir(&file);
        let file_str = workdir_path.to_str().unwrap_or_default();

        let (mode_index, hash_index) = if let Some(entry) = metadata.index.get(file_str, 0) {
            (entry.mode, entry.hash.to_string())
        } else {
            (0o100644, zero_hash.clone())
        };

        let (mode_head, hash_head) = if staged_status == 'A' {
            (0, zero_hash.clone())
        } else if let Some(info) = metadata.head_tree_items.get(&workdir_path) {
            (info.mode, info.hash.clone())
        } else {
            (0, zero_hash.clone())
        };

        let mode_worktree = if unstaged_status == 'D' {
            0
        } else {
            get_worktree_mode(&file)
        };

        let sub = if is_submodule_mode(mode_index) || is_submodule_mode(mode_head) {
            get_submodule_status(&file)
        } else {
            "N...".to_string()
        };

        writeln!(
            writer,
            "1 {}{} {} {} {} {} {} {} {}",
            staged_status,
            unstaged_status,
            sub,
            format_mode(mode_head),
            format_mode(mode_index),
            format_mode(mode_worktree),
            hash_head,
            hash_index,
            file.display()
        )
        .map_err(|e| CliError::io(format!("failed to write status output: {e}")))?;
    }

    for file in ignored {
        writeln!(writer, "! {}", file.display())
            .map_err(|e| CliError::io(format!("failed to write status output: {e}")))?;
    }
    Ok(())
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
    let use_colors = should_use_colors(output).await;

    let status_list = generate_short_format_status(staged, unstaged);

    for (file, staged_status, unstaged_status) in status_list {
        if use_colors {
            let colored_output = format_colored_status(staged_status, unstaged_status, &file);
            writeln!(writer, "{}", colored_output)
                .map_err(|e| CliError::io(format!("failed to write status output: {e}")))?;
        } else {
            writeln!(
                writer,
                "{}{} {}",
                staged_status,
                unstaged_status,
                file.display()
            )
            .map_err(|e| CliError::io(format!("failed to write status output: {e}")))?;
        }
    }
    Ok(())
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

fn changes_to_be_staged_split_with_index(
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
    let files =
        list_workdir_files_safe(workdir).map_err(|source| StatusError::ListWorkdirFiles {
            path: workdir.clone(),
            source,
        })?;
    for file in files {
        let file_str = file
            .to_str()
            .ok_or_else(|| StatusError::InvalidPathEncoding { path: file.clone() })?;
        if !index.tracked(file_str, 0) {
            let file_abs = workdir.join(&file);
            if util::check_gitignore(workdir, &file_abs) {
                ignored.new.push(file);
            } else {
                visible.new.push(file);
            }
        }
    }
    Ok((visible, ignored))
}

fn list_workdir_files_safe(workdir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();

    for entry in walkdir::WalkDir::new(workdir)
        .into_iter()
        .filter_entry(|entry| {
            entry.path() == workdir || entry.file_name() != std::ffi::OsStr::new(util::ROOT_DIR)
        })
    {
        let entry = entry.map_err(|err| {
            let err_text = err.to_string();
            err.into_io_error()
                .unwrap_or_else(|| io::Error::other(err_text))
        })?;
        let path = entry.path();

        if entry.file_type().is_file() {
            let relative = path
                .strip_prefix(workdir)
                .map_err(|err| io::Error::other(err.to_string()))?;
            files.push(relative.to_path_buf());
        }
    }

    Ok(files)
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
