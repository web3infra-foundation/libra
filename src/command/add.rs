//! Stages changes for commit by parsing pathspecs and modes, respecting ignore
//! policy, refreshing index entries, and writing blob objects.

use std::{
    env,
    io::{self, Write},
    path::{Path, PathBuf},
};

use clap::Parser;
use git_internal::{
    errors::GitError,
    internal::{
        index::{Index, IndexEntry},
        object::blob::Blob,
    },
};
use serde::Serialize;

use crate::{
    command::status::{self, Changes},
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        lfs,
        object_ext::BlobExt,
        output::{self, OutputConfig},
        path, util,
    },
};

/// Stage file contents for the next commit.
///
/// EXAMPLES:
///     libra add .                        Stage all changes in current directory
///     libra add src/main.rs              Stage a specific file
///     libra add src/ tests/              Stage multiple paths
///     libra add -A                       Stage all changes (adds, modifies, removes)
///     libra add -u                       Update tracked files only (no new files)
///     libra add --dry-run .              Preview what would be staged
///     libra add -f ignored_file.log      Force-add an ignored file
///     libra add --refresh                Refresh index metadata without staging
#[derive(Parser, Debug)]
pub struct AddArgs {
    /// pathspec... files & dir to add content from.
    #[clap(required = false)]
    pub pathspec: Vec<String>,

    /// Update the index not only where the working tree has a file matching pathspec but also where the index already has an entry. This adds, modifies, and removes index entries to match the working tree.
    ///
    /// If no pathspec is given when -A option is used, all files in the entire working tree are updated
    #[clap(short = 'A', long, group = "mode")]
    pub all: bool,

    /// Update the index just where it already has an entry matching **pathspec**.
    /// This removes as well as modifies index entries to match the working tree, but adds no new files.
    #[clap(short, long, group = "mode")]
    pub update: bool,

    /// Refresh index entries for all files currently in the index.
    ///
    /// This updates only the metadata (e.g. file stat information such as
    /// timestamps, file size, etc.) of existing index entries to match
    /// the working tree, without adding new files or removing entries.
    #[clap(long, group = "mode")]
    pub refresh: bool,

    /// more detailed output
    #[clap(short, long)]
    pub verbose: bool,

    /// allow adding otherwise ignored files
    #[clap(short = 'f', long)]
    pub force: bool,

    /// dry run
    #[clap(short, long)]
    pub dry_run: bool,

    /// ignore errors
    #[clap(long)]
    pub ignore_errors: bool,
}

#[derive(thiserror::Error, Debug)]
pub enum AddError {
    #[error("not a libra repository (or any of the parent directories): .libra")]
    NotInRepo,
    #[error("pathspec '{pathspec}' did not match any files")]
    PathspecNotMatched { pathspec: String },
    #[error("'{path}' is outside repository at '{repo_root}'")]
    PathOutsideRepo { path: String, repo_root: PathBuf },
    #[error("unable to read index '{path}': {source}")]
    IndexLoad { path: PathBuf, source: GitError },
    #[error("unable to write index '{path}': {source}")]
    IndexSave { path: PathBuf, source: GitError },
    #[error("failed to refresh '{path}': {source}")]
    RefreshFailed { path: PathBuf, source: GitError },
    #[error("failed to create index entry for '{path}': {source}")]
    CreateIndexEntry { path: PathBuf, source: io::Error },
    #[error("path '{path}' is not valid UTF-8")]
    InvalidPathEncoding { path: PathBuf },
    #[error("failed to determine repository working directory: {source}")]
    Workdir { source: io::Error },
    #[error("failed to inspect repository status: {source}")]
    Status { source: status::StatusError },
}

impl From<AddError> for CliError {
    fn from(error: AddError) -> Self {
        match &error {
            AddError::NotInRepo => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::RepoNotFound)
                .with_hint("run 'libra init' to create a repository"),
            AddError::PathspecNotMatched { .. } => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::CliInvalidTarget)
                .with_hint("check the path and try again.")
                .with_hint("use 'libra status' to inspect tracked and untracked files."),
            AddError::PathOutsideRepo { .. } => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::CliInvalidTarget)
                .with_hint("all paths must be within the repository working tree"),
            AddError::IndexLoad { .. } => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::RepoCorrupt)
                .with_hint("the index file may be corrupted; try 'libra status' to verify"),
            AddError::IndexSave { .. } => {
                CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::IoWriteFailed)
            }
            AddError::RefreshFailed { .. } => {
                CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::IoReadFailed)
            }
            AddError::CreateIndexEntry { .. } => {
                CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::IoWriteFailed)
            }
            AddError::InvalidPathEncoding { .. } => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::CliInvalidTarget)
                .with_hint("path contains non-UTF-8 characters"),
            AddError::Workdir { source } => {
                if source.kind() == io::ErrorKind::NotFound {
                    CliError::fatal(error.to_string())
                        .with_stable_code(StableErrorCode::RepoNotFound)
                } else {
                    CliError::fatal(error.to_string())
                        .with_stable_code(StableErrorCode::IoReadFailed)
                }
            }
            AddError::Status { .. } => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::RepoCorrupt)
                .with_hint("failed to compute working tree status"),
        }
    }
}

// ---------------------------------------------------------------------------
// Structured output types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct AddFailure {
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AddOutput {
    /// New files staged
    pub added: Vec<String>,
    /// Modified files staged
    pub modified: Vec<String>,
    /// Deleted files staged (tracked file no longer in worktree)
    pub removed: Vec<String>,
    /// Files whose metadata was refreshed (--refresh mode)
    pub refreshed: Vec<String>,
    /// Paths ignored by .libraignore (only when pathspec matches ignored files)
    pub ignored: Vec<String>,
    /// Paths that failed under --ignore-errors
    pub failed: Vec<AddFailure>,
    /// Whether this was a dry-run (no actual changes made)
    pub dry_run: bool,
}

impl AddOutput {
    fn empty(dry_run: bool) -> Self {
        Self {
            added: Vec::new(),
            modified: Vec::new(),
            removed: Vec::new(),
            refreshed: Vec::new(),
            ignored: Vec::new(),
            failed: Vec::new(),
            dry_run,
        }
    }

    fn total_staged(&self) -> usize {
        self.added.len() + self.modified.len() + self.removed.len()
    }

    fn is_empty(&self) -> bool {
        self.total_staged() == 0 && self.refreshed.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Action tracking for add_a_file
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StagedAction {
    Added,
    Modified,
    Removed,
    Unchanged,
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

#[derive(Default)]
struct ValidatedPathspecs {
    files: Vec<PathBuf>,
    ignored: Vec<String>,
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

pub async fn execute(args: AddArgs) {
    if let Err(err) = execute_safe(args, &OutputConfig::default()).await {
        err.print_stderr();
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting. Stages changes by resolving pathspecs, respecting
/// ignore policy, and writing blob objects to storage.
pub async fn execute_safe(args: AddArgs, output: &OutputConfig) -> CliResult<()> {
    let verbose = args.verbose;
    let dry_run = args.dry_run;
    let result = run_add(&args).await?;

    // --- Render output ---
    render_add_output(&result, output, verbose, dry_run)?;

    // --- Warning tracking for ignored / partial failures ---
    if !result.ignored.is_empty() || !result.failed.is_empty() {
        output::record_warning();
    }

    Ok(())
}

/// Pure execution entry point. Performs all staging logic and returns a
/// structured [`AddOutput`] without printing anything.
pub async fn run_add(args: &AddArgs) -> CliResult<AddOutput> {
    let workdir = util::try_working_dir().map_err(|source| {
        if source.kind() == io::ErrorKind::NotFound {
            AddError::NotInRepo
        } else {
            AddError::Workdir { source }
        }
    })?;
    let index_path = path::try_index().map_err(|source| {
        if source.kind() == io::ErrorKind::NotFound {
            AddError::NotInRepo
        } else {
            AddError::Workdir { source }
        }
    })?;
    let storage_path = util::try_get_storage_path(None).map_err(|source| {
        if source.kind() == io::ErrorKind::NotFound {
            AddError::NotInRepo
        } else {
            AddError::Workdir { source }
        }
    })?;

    // Resolve pathspecs
    let requested_paths: Vec<PathBuf> = if args.pathspec.is_empty() {
        if !args.all && !args.update && !args.refresh {
            return Err(CliError::command_usage("nothing specified, nothing added")
                .with_stable_code(StableErrorCode::CliInvalidArguments)
                .with_hint("maybe you wanted to say 'libra add .'?"));
        }
        vec![workdir.clone()]
    } else {
        args.pathspec.iter().map(PathBuf::from).collect()
    };

    let mut index = Index::load(&index_path).map_err(|source| AddError::IndexLoad {
        path: index_path.clone(),
        source,
    })?;
    let current_dir = env::current_dir().map_err(|source| AddError::Workdir { source })?;

    let (mut visible_changes, ignored_changes) =
        status::changes_to_be_staged_split_safe().map_err(|source| AddError::Status { source })?;
    if args.force {
        visible_changes.extend(ignored_changes.clone());
    }
    let ignored_changes = if args.force {
        Changes::default()
    } else {
        ignored_changes
    };

    let validated = validate_pathspecs(
        &args.pathspec,
        &requested_paths,
        &workdir,
        &current_dir,
        &visible_changes,
        &ignored_changes,
        &index,
    )?;

    let mut add_output = AddOutput::empty(args.dry_run);

    // Collect ignored paths into output
    if !validated.ignored.is_empty() {
        let mut sorted_ignored = validated.ignored.clone();
        sorted_ignored.sort();
        sorted_ignored.dedup();
        add_output.ignored = sorted_ignored;
    }

    // --- Refresh mode ---
    if args.refresh {
        let tracked_modified = filter_refresh_candidates(
            &visible_changes.modified,
            &validated.files,
            &workdir,
            &current_dir,
        );
        if args.dry_run {
            add_output.refreshed = tracked_modified
                .iter()
                .map(|f| f.display().to_string())
                .collect();
        } else {
            let refreshed = do_refresh_files(&mut index, &tracked_modified, &workdir)?;
            add_output.refreshed = refreshed.iter().map(|f| f.display().to_string()).collect();
            index
                .save(&index_path)
                .map_err(|source| AddError::IndexSave {
                    path: index_path.clone(),
                    source,
                })?;
        }

        return check_ignored_only_error(add_output);
    }

    // --- Normal add mode ---
    let mut files = visible_changes.modified;
    files.extend(visible_changes.deleted);
    if !args.update {
        files.extend(visible_changes.new);
    }
    files = filter_candidates(&files, &validated.files, &workdir, &current_dir);
    filter_out_current_executable(&mut files);
    files.sort();
    files.dedup();

    if args.dry_run {
        // Classify files for dry-run preview
        for file in &files {
            let status = check_file_status(file, &index, &workdir)?;
            let path_str = file.display().to_string();
            match status {
                FileStatus::New => add_output.added.push(path_str),
                FileStatus::Modified => add_output.modified.push(path_str),
                FileStatus::Deleted => add_output.removed.push(path_str),
                FileStatus::Unchanged | FileStatus::NotFound => {}
            }
        }
        return check_ignored_only_error(add_output);
    }

    // Stage each file
    for file in &files {
        match stage_a_file(file, &mut index, &workdir, &storage_path).await {
            Ok(action) => {
                let path_str = file.display().to_string();
                match action {
                    StagedAction::Added => add_output.added.push(path_str),
                    StagedAction::Modified => add_output.modified.push(path_str),
                    StagedAction::Removed => add_output.removed.push(path_str),
                    StagedAction::Unchanged => {}
                }
            }
            Err(err) => {
                if !args.ignore_errors {
                    return Err(CliError::from(err));
                }
                add_output.failed.push(AddFailure {
                    path: file.display().to_string(),
                    message: err.to_string(),
                });
            }
        }
    }

    index
        .save(&index_path)
        .map_err(|source| AddError::IndexSave {
            path: index_path.clone(),
            source,
        })?;

    check_ignored_only_error(add_output)
}

/// If the output has ignored files but nothing was staged, return an error.
fn check_ignored_only_error(output: AddOutput) -> CliResult<AddOutput> {
    if !output.ignored.is_empty() && output.is_empty() {
        let mut message =
            String::from("the following paths are ignored by one of your .libraignore files:");
        for path in &output.ignored {
            message.push('\n');
            message.push_str(path);
        }
        return Err(CliError::fatal(message)
            .with_stable_code(StableErrorCode::AddNothingStaged)
            .with_hint("use -f if you really want to add them."));
    }
    Ok(output)
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn render_add_output(
    result: &AddOutput,
    output: &OutputConfig,
    verbose: bool,
    dry_run: bool,
) -> CliResult<()> {
    // JSON / machine mode
    if output.is_json() {
        return output::emit_json_data("add", result, output);
    }

    // Quiet mode: suppress stdout, but still emit warnings to stderr
    if output.quiet {
        render_warnings_stderr(result);
        return Ok(());
    }

    let stdout = io::stdout();
    let mut w = stdout.lock();

    if dry_run {
        render_dry_run(&mut w, result)?;
    } else if !result.refreshed.is_empty() {
        render_refresh(&mut w, result, verbose)?;
    } else {
        render_normal(&mut w, result, verbose)?;
    }

    // Warnings to stderr
    render_warnings_stderr(result);

    Ok(())
}

fn render_dry_run(w: &mut impl Write, result: &AddOutput) -> CliResult<()> {
    for f in &result.added {
        writeln!(w, "add: {f}").map_err(write_err)?;
    }
    for f in &result.modified {
        writeln!(w, "add: {f}").map_err(write_err)?;
    }
    for f in &result.removed {
        writeln!(w, "remove: {f}").map_err(write_err)?;
    }
    for f in &result.refreshed {
        writeln!(w, "refresh: {f}").map_err(write_err)?;
    }
    writeln!(w, "(dry run, no files were staged)").map_err(write_err)?;
    Ok(())
}

fn render_refresh(w: &mut impl Write, result: &AddOutput, verbose: bool) -> CliResult<()> {
    if verbose {
        for f in &result.refreshed {
            writeln!(w, "refreshed: {f}").map_err(write_err)?;
        }
    }
    if result.refreshed.is_empty() {
        writeln!(w, "nothing to refresh").map_err(write_err)?;
    } else {
        let n = result.refreshed.len();
        let word = if n == 1 { "file" } else { "files" };
        writeln!(w, "refreshed {n} {word}").map_err(write_err)?;
    }
    Ok(())
}

fn render_normal(w: &mut impl Write, result: &AddOutput, verbose: bool) -> CliResult<()> {
    let total = result.total_staged();

    if total == 0 {
        writeln!(w, "nothing to add").map_err(write_err)?;
        return Ok(());
    }

    // Verbose: per-file listing
    if verbose {
        for f in &result.added {
            writeln!(w, "add(new): {f}").map_err(write_err)?;
        }
        for f in &result.modified {
            writeln!(w, "add(modified): {f}").map_err(write_err)?;
        }
        for f in &result.removed {
            writeln!(w, "removed: {f}").map_err(write_err)?;
        }
    }

    // Summary line
    if total == 1 {
        let (path, kind) = if let Some(f) = result.added.first() {
            (f.as_str(), "new file")
        } else if let Some(f) = result.modified.first() {
            (f.as_str(), "modified")
        } else if let Some(f) = result.removed.first() {
            (f.as_str(), "removed")
        } else {
            return Err(CliError::internal(
                "single-file add summary is missing a staged path",
            ));
        };
        writeln!(w, "add '{path}' ({kind})").map_err(write_err)?;
    } else {
        let mut parts = Vec::new();
        if !result.added.is_empty() {
            parts.push(format!("{} new", result.added.len()));
        }
        if !result.modified.is_empty() {
            parts.push(format!("{} modified", result.modified.len()));
        }
        if !result.removed.is_empty() {
            parts.push(format!("{} removed", result.removed.len()));
        }
        writeln!(w, "add {total} files ({})", parts.join(", ")).map_err(write_err)?;
    }

    Ok(())
}

fn render_warnings_stderr(result: &AddOutput) {
    if !result.ignored.is_empty() {
        eprintln!("warning: the following paths are ignored by one of your .libraignore files:");
        for path in &result.ignored {
            eprintln!("{path}");
        }
        eprintln!("hint: use -f if you really want to add them.");
        eprintln!("hint: use 'libra restore --staged <file>' to unstage if needed");
    }
    if !result.failed.is_empty() {
        eprintln!(
            "warning: {} path(s) failed and were skipped (--ignore-errors):",
            result.failed.len()
        );
        for failure in &result.failed {
            eprintln!("  {}: {}", failure.path, failure.message);
        }
    }
}

fn write_err(e: io::Error) -> CliError {
    CliError::io(format!("failed to write add output: {e}"))
}

// ---------------------------------------------------------------------------
// Core staging logic
// ---------------------------------------------------------------------------

fn validate_pathspecs(
    raw_pathspecs: &[String],
    requested_paths: &[PathBuf],
    workdir: &Path,
    current_dir: &Path,
    visible_changes: &Changes,
    ignored_changes: &Changes,
    index: &Index,
) -> Result<ValidatedPathspecs, AddError> {
    if raw_pathspecs.is_empty() {
        return Ok(ValidatedPathspecs {
            files: requested_paths.to_vec(),
            ignored: Vec::new(),
        });
    }

    let tracked_files = index.tracked_files();
    let change_candidates = collect_change_candidates(visible_changes);
    let ignored_candidates = collect_change_candidates(ignored_changes);

    let mut ignored = Vec::new();
    let mut files = Vec::new();
    for (raw, requested_path) in raw_pathspecs.iter().zip(requested_paths.iter()) {
        let requested_abs = resolve_pathspec(requested_path, current_dir);
        if !util::is_sub_path(&requested_abs, workdir) {
            return Err(AddError::PathOutsideRepo {
                path: raw.clone(),
                repo_root: workdir.to_path_buf(),
            });
        }

        let matches_changes = pathspec_matches_any(&requested_abs, &change_candidates, workdir);
        let matches_tracked = pathspec_matches_any(&requested_abs, &tracked_files, workdir);
        let matches_ignored = pathspec_matches_any(&requested_abs, &ignored_candidates, workdir);

        if matches_changes || matches_tracked {
            files.push(requested_path.clone());
            continue;
        }
        if matches_ignored {
            ignored.push(raw.clone());
            continue;
        }

        return Err(AddError::PathspecNotMatched {
            pathspec: raw.clone(),
        });
    }

    Ok(ValidatedPathspecs { files, ignored })
}

fn collect_change_candidates(changes: &Changes) -> Vec<PathBuf> {
    let mut files = Vec::new();
    files.extend(changes.new.iter().cloned());
    files.extend(changes.modified.iter().cloned());
    files.extend(changes.deleted.iter().cloned());
    files
}

fn resolve_pathspec(pathspec: &Path, current_dir: &Path) -> PathBuf {
    if pathspec.is_absolute() {
        pathspec.to_path_buf()
    } else {
        current_dir.join(pathspec)
    }
}

fn pathspec_matches_any(requested_abs: &Path, candidates: &[PathBuf], workdir: &Path) -> bool {
    candidates.iter().any(|candidate| {
        let candidate_abs = workdir.join(candidate);
        util::is_sub_path(&candidate_abs, requested_abs)
    })
}

fn filter_candidates(
    files: &[PathBuf],
    requested_paths: &[PathBuf],
    workdir: &Path,
    current_dir: &Path,
) -> Vec<PathBuf> {
    files
        .iter()
        .filter(|file| {
            let file_abs = workdir.join(file.as_path());
            requested_paths.iter().any(|pathspec| {
                let requested_abs = resolve_pathspec(pathspec, current_dir);
                util::is_sub_path(&file_abs, &requested_abs)
            })
        })
        .cloned()
        .collect()
}

fn filter_refresh_candidates(
    files: &[PathBuf],
    requested_paths: &[PathBuf],
    workdir: &Path,
    current_dir: &Path,
) -> Vec<PathBuf> {
    filter_candidates(files, requested_paths, workdir, current_dir)
}

fn filter_out_current_executable(files: &mut Vec<PathBuf>) {
    if let Some(exe_path) = std::env::current_exe()
        .ok()
        .and_then(|path| path.canonicalize().ok())
    {
        files.retain(|file| {
            util::try_workdir_to_absolute(file)
                .ok()
                .and_then(|path| path.canonicalize().ok())
                .is_none_or(|abs| abs != exe_path)
        });
    }
}

/// Refresh files and return the list of files actually refreshed.
fn do_refresh_files(
    index: &mut Index,
    files: &[PathBuf],
    workdir: &Path,
) -> Result<Vec<PathBuf>, AddError> {
    let mut refreshed = Vec::new();
    for file in files {
        if index
            .refresh(file, workdir)
            .map_err(|source| AddError::RefreshFailed {
                path: file.clone(),
                source,
            })?
        {
            refreshed.push(file.clone());
        }
    }
    Ok(refreshed)
}

/// Stage a single file and return the action taken.
///
/// `file` path must be relative to the working directory.
async fn stage_a_file(
    file: &Path,
    index: &mut Index,
    workdir: &Path,
    storage_path: &Path,
) -> Result<StagedAction, AddError> {
    let file_abs = workdir.join(file);
    if !util::is_sub_path(&file_abs, workdir) {
        return Err(AddError::PathOutsideRepo {
            path: file.display().to_string(),
            repo_root: workdir.to_path_buf(),
        });
    }
    if util::is_sub_path(&file_abs, storage_path) {
        return Ok(StagedAction::Unchanged);
    }

    let file_str = file.to_str().ok_or_else(|| AddError::InvalidPathEncoding {
        path: file.to_path_buf(),
    })?;
    let file_status = check_file_status(file, index, workdir)?;
    match file_status {
        FileStatus::New => {
            let blob = gen_blob_from_file(&file_abs);
            blob.save();
            index.add(
                IndexEntry::new_from_file(file, blob.id, workdir).map_err(|source| {
                    AddError::CreateIndexEntry {
                        path: file.to_path_buf(),
                        source,
                    }
                })?,
            );
            Ok(StagedAction::Added)
        }
        FileStatus::Modified => {
            if index.is_modified(file_str, 0, workdir) {
                let blob = gen_blob_from_file(&file_abs);
                if !index.verify_hash(file_str, 0, &blob.id) {
                    blob.save();
                    index.update(IndexEntry::new_from_file(file, blob.id, workdir).map_err(
                        |source| AddError::CreateIndexEntry {
                            path: file.to_path_buf(),
                            source,
                        },
                    )?);
                    return Ok(StagedAction::Modified);
                }
            }
            Ok(StagedAction::Unchanged)
        }
        FileStatus::Deleted => {
            index.remove(file_str, 0);
            Ok(StagedAction::Removed)
        }
        FileStatus::Unchanged => Ok(StagedAction::Unchanged),
        FileStatus::NotFound => Err(AddError::PathspecNotMatched {
            pathspec: file.display().to_string(),
        }),
    }
}

enum FileStatus {
    /// file is new
    New,
    /// file is modified
    Modified,
    /// file is deleted
    Deleted,
    /// file exists or is tracked but has nothing to stage
    Unchanged,
    /// file is not tracked
    NotFound,
}

fn check_file_status(file: &Path, index: &Index, workdir: &Path) -> Result<FileStatus, AddError> {
    let file_str = file.to_str().ok_or_else(|| AddError::InvalidPathEncoding {
        path: file.to_path_buf(),
    })?;
    let file_abs = workdir.join(file);
    if !file_abs.exists() {
        if index.tracked(file_str, 0) {
            Ok(FileStatus::Deleted)
        } else {
            Ok(FileStatus::NotFound)
        }
    } else if !index.tracked(file_str, 0) {
        Ok(FileStatus::New)
    } else if index.is_modified(file_str, 0, workdir) {
        Ok(FileStatus::Modified)
    } else {
        Ok(FileStatus::Unchanged)
    }
}

/// Generate a `Blob` from a file
/// - if the file is tracked by LFS, generate a `Blob` with pointer file
fn gen_blob_from_file(path: impl AsRef<Path>) -> Blob {
    if lfs::is_lfs_tracked(&path) {
        Blob::from_lfs_file(&path)
    } else {
        Blob::from_file(&path)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_args_conflict_with_refresh() {
        // "--refresh" cannot be combined with "-A", "--refresh" or "-u"
        assert!(AddArgs::try_parse_from(["test", "-A", "--refresh"]).is_err());
        assert!(AddArgs::try_parse_from(["test", "-u", "--refresh"]).is_err());
        assert!(AddArgs::try_parse_from(["test", "-A", "-u", "--refresh"]).is_err());
    }

    #[test]
    fn add_output_total_and_empty() {
        let mut out = AddOutput::empty(false);
        assert!(out.is_empty());
        assert_eq!(out.total_staged(), 0);

        out.added.push("a.rs".to_string());
        assert_eq!(out.total_staged(), 1);
        assert!(!out.is_empty());
    }
}
