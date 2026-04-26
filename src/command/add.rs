//! Stages changes for the next commit (`libra add`).
//!
//! Implements the `add` subcommand: parses pathspecs and mode flags, applies
//! ignore policy (`.libraignore`), classifies each path against the working
//! tree and the on-disk index, writes new blob objects under the repository's
//! object storage, and finally persists the updated index.
//!
//! Non-obvious responsibilities:
//! - Maps low-level [`GitError`] / [`io::Error`] variants into structured
//!   [`AddError`] cases that each carry stable error codes and human-readable
//!   hints (see the `From<AddError> for CliError` impl).
//! - Supports four output channels in [`render_add_output`]: JSON, quiet
//!   (warnings only on stderr), normal (summary), and verbose (per-path).
//! - Provides a "refresh-only" mode that updates index stat metadata without
//!   rewriting blobs.
//! - Filters the running `libra` executable from staging candidates so a
//!   self-build does not accidentally stage its own binary.
//! - Honors the `force` flag by folding ignored paths back into the visible
//!   change set before pathspec validation runs.

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

/// Domain error for `libra add`.
///
/// Each variant maps to a specific failure mode of the staging pipeline and is
/// translated into a [`CliError`] (with a stable code and hints) by the
/// `From<AddError> for CliError` impl below. Variants are not numbered in the
/// public API; classification happens inside that impl.
#[derive(thiserror::Error, Debug)]
pub enum AddError {
    /// No `.libra` directory was found walking up from the CWD. Surfaced as
    /// [`StableErrorCode::RepoNotFound`].
    #[error("not a libra repository (or any of the parent directories): .libra")]
    NotInRepo,
    /// A user-supplied pathspec matched neither tracked files, working-tree
    /// changes, nor an ignored entry — typically a typo. Mapped to
    /// [`StableErrorCode::CliInvalidTarget`].
    #[error("pathspec '{pathspec}' did not match any files")]
    PathspecNotMatched { pathspec: String },
    /// The (canonical) pathspec resolves outside the repository working tree,
    /// for example via `..` traversal or an absolute path to another repo.
    #[error("'{path}' is outside repository at '{repo_root}'")]
    PathOutsideRepo { path: String, repo_root: PathBuf },
    /// `Index::load` failed — usually means a corrupt or truncated
    /// `.libra/index`. Mapped to [`StableErrorCode::RepoCorrupt`].
    #[error("unable to read index '{path}': {source}")]
    IndexLoad { path: PathBuf, source: GitError },
    /// Persisting the updated index back to disk failed (e.g. permission
    /// denied or out of space).
    #[error("unable to write index '{path}': {source}")]
    IndexSave { path: PathBuf, source: GitError },
    /// `Index::refresh` could not stat a tracked file in `--refresh` mode.
    #[error("failed to refresh '{path}': {source}")]
    RefreshFailed { path: PathBuf, source: GitError },
    /// Building an [`IndexEntry`] from a worktree file failed (typically an
    /// `lstat`/`open` error).
    #[error("failed to create index entry for '{path}': {source}")]
    CreateIndexEntry { path: PathBuf, source: io::Error },
    /// Path bytes are not valid UTF-8 — Libra's index does not yet preserve
    /// non-UTF-8 paths verbatim.
    #[error("path '{path}' is not valid UTF-8")]
    InvalidPathEncoding { path: PathBuf },
    /// Failure resolving the working directory (CWD missing, permission
    /// denied, etc.). The `From` impl below distinguishes "missing" (treated
    /// as `RepoNotFound`) from other I/O errors.
    #[error("failed to determine repository working directory: {source}")]
    Workdir { source: io::Error },
    /// The status engine failed before staging could proceed; the underlying
    /// [`status::StatusError`] is preserved as a source.
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

/// One entry in [`AddOutput::failed`]: a path that could not be staged when
/// `--ignore-errors` was set. The `message` is the rendered [`AddError`].
#[derive(Debug, Clone, Serialize)]
pub struct AddFailure {
    pub path: String,
    pub message: String,
}

/// Structured result of a single `libra add` invocation.
///
/// Built by [`run_add`] and consumed by [`render_add_output`] (text mode) or
/// emitted directly through `output::emit_json_data` (JSON mode). The fields
/// always reference paths relative to the working directory.
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
    /// Construct an empty result, preserving the user's `--dry-run` choice so
    /// downstream rendering can switch on it.
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

    /// Sum of paths that produced an actual index change. Excludes
    /// `refreshed`, since refreshing only updates stat metadata.
    ///
    /// See: tests::add_output_total_and_empty in src/command/add.rs:840.
    fn total_staged(&self) -> usize {
        self.added.len() + self.modified.len() + self.removed.len()
    }

    /// True when no path was staged or refreshed. Used together with
    /// [`Self::ignored`] in [`check_ignored_only_error`] to detect the
    /// "everything was filtered out" failure mode.
    fn is_empty(&self) -> bool {
        self.total_staged() == 0 && self.refreshed.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Action tracking for add_a_file
// ---------------------------------------------------------------------------

/// The outcome of staging a single path. Returned by [`stage_a_file`] so the
/// caller can sort each path into the correct [`AddOutput`] bucket.
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

/// Result of [`validate_pathspecs`]: the canonicalised set of pathspecs that
/// should drive staging, plus any pathspecs that only matched
/// `.libraignore`d entries (reported as warnings).
#[derive(Default)]
struct ValidatedPathspecs {
    files: Vec<PathBuf>,
    ignored: Vec<String>,
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Fire-and-forget entry used by the simple CLI dispatcher.
///
/// Functional scope:
/// - Delegates to [`execute_safe`] using the default [`OutputConfig`].
/// - On error, prints the rendered [`CliError`] to stderr and returns; the
///   process exit code is the dispatcher's responsibility.
///
/// Boundary conditions:
/// - Does not propagate errors, so callers that care about the exit status
///   should call [`execute_safe`] directly.
pub async fn execute(args: AddArgs) {
    if let Err(err) = execute_safe(args, &OutputConfig::default()).await {
        err.print_stderr();
    }
}

/// Structured entry point used by `cli::parse` and integration tests.
///
/// Functional scope:
/// - Runs the full staging pipeline via [`run_add`].
/// - Renders the [`AddOutput`] in the format the user requested
///   (`OutputConfig::is_json`, `quiet`, normal, verbose).
/// - Records a process-level warning (via [`output::record_warning`]) when any
///   path was ignored or fell through `--ignore-errors`.
///
/// Boundary conditions:
/// - Returns the same `Err(CliError)` produced by [`run_add`]; rendering only
///   runs after a successful staging pass.
///
/// See: tests::test_add_single_file in tests/command/add_test.rs:12.
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

/// Pure staging implementation that produces [`AddOutput`] without printing.
///
/// Functional scope:
/// - Resolves repository paths (`workdir`, `.libra/index`, object storage),
///   loads the index, and runs `status::changes_to_be_staged_split_safe`.
/// - Validates pathspecs, optionally folding ignored paths in when `--force`
///   is set, and short-circuits to refresh-mode when `--refresh` is set.
/// - Filters tree changes against the requested pathspec set, then either
///   classifies (dry-run) or stages each file via [`stage_a_file`].
/// - Persists the index back to disk on the non-dry-run path.
///
/// Boundary conditions:
/// - Returns [`AddError::NotInRepo`] when the working dir, index, or storage
///   path lookups raise [`io::ErrorKind::NotFound`]; other I/O errors map to
///   [`AddError::Workdir`].
/// - Returns a `CliError::command_usage` (stable code
///   `CliInvalidArguments`) when no pathspec is given and none of `-A`,
///   `-u`, `--refresh` is set — see
///   `tests::test_add_without_path_should_error` in
///   `tests/command/add_test.rs:518`.
/// - Returns `Err(AddError::PathspecNotMatched)` for unknown pathspecs unless
///   `--ignore-errors` was set during the per-file staging loop.
///
/// See: tests::test_add_all_flag in tests/command/add_test.rs:100;
/// tests::test_add_force_tracks_ignored_file in tests/command/add_test.rs:319.
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

/// Convert "all paths ignored, nothing staged" into a hard error.
///
/// Functional scope:
/// - When `output.ignored` is non-empty *and* nothing else was staged or
///   refreshed, builds an error message listing each ignored path and
///   attaches a hint to use `-f`.
/// - Otherwise returns the input unchanged.
///
/// Boundary conditions:
/// - Always passes through when [`AddOutput::is_empty`] is false, even if
///   some paths were ignored — those become warnings instead.
/// - Stable code is [`StableErrorCode::AddNothingStaged`].
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

/// Top-level dispatcher for the four output modes (JSON, quiet, dry-run,
/// refresh, normal).
///
/// Functional scope:
/// - Picks one body renderer based on flags and writes the result to stdout.
/// - Always emits warnings to stderr last, regardless of mode, so that users
///   who pipe stdout still see ignore/skip notices.
///
/// Boundary conditions:
/// - In quiet mode, stdout is suppressed entirely but stderr warnings still
///   flow.
/// - JSON mode bypasses stdout-locking and short-circuits with the structured
///   payload via [`output::emit_json_data`].
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

/// Render the `--dry-run` preview: one line per would-be-changed path,
/// suffixed with the explicit `(dry run, no files were staged)` footer.
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

/// Render the output of `--refresh`. In verbose mode each refreshed file is
/// printed; otherwise just a `refreshed N file(s)` summary is emitted.
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

/// Render the default text output: optional per-file lines (verbose) followed
/// by either a single-file message or a multi-file summary.
///
/// Boundary conditions:
/// - Returns [`CliError::internal`] if `total == 1` but every bucket is empty
///   — this is an internal invariant violation, not a user-visible state.
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

/// Emit the always-on warning footer: which paths were ignored, which paths
/// were skipped under `--ignore-errors`. Output goes to stderr so it survives
/// stdout redirection.
fn render_warnings_stderr(result: &AddOutput) {
    if !result.ignored.is_empty() {
        eprintln!("warning: the following paths are ignored by one of your .libraignore files:");
        for path in &result.ignored {
            eprintln!("{path}");
        }
        eprintln!();
        eprintln!("Hint: use -f if you really want to add them.");
        eprintln!("Hint: use 'libra restore --staged <file>' to unstage if needed");
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

/// Convert a `writeln!` failure into the standardized I/O [`CliError`] so the
/// caller does not need to repeat the format string at every call site.
fn write_err(e: io::Error) -> CliError {
    CliError::io(format!("failed to write add output: {e}"))
}

// ---------------------------------------------------------------------------
// Core staging logic
// ---------------------------------------------------------------------------

/// Resolve, canonicalise and classify each user-supplied pathspec.
///
/// Functional scope:
/// - When `raw_pathspecs` is empty, returns `requested_paths` unchanged
///   (caller passes the workdir as the implicit pathspec for `-A` / `-u`).
/// - For each pathspec, makes the path absolute, rejects anything outside
///   `workdir`, and probes three candidate sets in order: visible changes,
///   tracked files in the index, and ignored changes.
/// - Pathspecs that match only an ignored entry are returned in
///   [`ValidatedPathspecs::ignored`] so they can be reported as warnings.
///
/// Boundary conditions:
/// - Returns [`AddError::PathOutsideRepo`] for any pathspec resolving outside
///   the working tree (including via `..`).
/// - Returns [`AddError::PathspecNotMatched`] for the first pathspec that
///   matches no candidate at all — `--ignore-errors` does not affect this
///   pre-flight stage.
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

/// Flatten the three change buckets (`new`, `modified`, `deleted`) into a
/// single ordered candidate list for pathspec matching.
fn collect_change_candidates(changes: &Changes) -> Vec<PathBuf> {
    let mut files = Vec::new();
    files.extend(changes.new.iter().cloned());
    files.extend(changes.modified.iter().cloned());
    files.extend(changes.deleted.iter().cloned());
    files
}

/// Make a user-supplied pathspec absolute by joining onto `current_dir` when
/// it is relative. Mirrors how Git's pathspec parser anchors specs to the
/// invoking shell's CWD rather than to the worktree root.
fn resolve_pathspec(pathspec: &Path, current_dir: &Path) -> PathBuf {
    if pathspec.is_absolute() {
        pathspec.to_path_buf()
    } else {
        current_dir.join(pathspec)
    }
}

/// True iff any path in `candidates` (interpreted relative to `workdir`) is a
/// subpath of `requested_abs`. Used both for tracked-file matching and for
/// status-change matching.
fn pathspec_matches_any(requested_abs: &Path, candidates: &[PathBuf], workdir: &Path) -> bool {
    candidates.iter().any(|candidate| {
        let candidate_abs = workdir.join(candidate);
        util::is_sub_path(&candidate_abs, requested_abs)
    })
}

/// Restrict `files` (workdir-relative) to entries that fall under at least
/// one of the user's pathspecs. Used to scope `-A`/`-u`-derived candidate
/// sets to the explicit positional arguments.
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

/// Alias of [`filter_candidates`] used in `--refresh` mode. Kept separate so
/// future divergence in semantics (e.g. submodule handling) only needs to
/// touch one branch.
fn filter_refresh_candidates(
    files: &[PathBuf],
    requested_paths: &[PathBuf],
    workdir: &Path,
    current_dir: &Path,
) -> Vec<PathBuf> {
    filter_candidates(files, requested_paths, workdir, current_dir)
}

/// Remove the running `libra` binary from the candidate list.
///
/// Functional scope:
/// - Detects the executable via `current_exe` + `canonicalize`, and drops any
///   candidate whose absolute, canonicalised path matches.
///
/// Boundary conditions:
/// - Silent no-op when `current_exe()` or `canonicalize()` fail; we never
///   skip files based on speculative information.
/// - Important when running `libra add .` from inside a Libra checkout that
///   has compiled the binary into a tracked location (`target/`), which would
///   otherwise stage the freshly produced executable.
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
///
/// Functional scope:
/// - Calls `Index::refresh` for each file. The underlying call returns
///   `true` only when the index entry's stat info actually changed; entries
///   whose mtime/size still match are silently skipped (and not added to the
///   returned vector).
///
/// Boundary conditions:
/// - The first refresh failure short-circuits the loop with
///   [`AddError::RefreshFailed`]; no rollback is performed on the index.
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
/// Functional scope:
/// - Translates the file's [`FileStatus`] into the corresponding index
///   mutation: writes a new blob and inserts an [`IndexEntry`] for `New`,
///   updates the entry only when the on-disk hash differs for `Modified`,
///   and removes the entry for `Deleted`.
/// - Skips files that live inside `storage_path` (the `.libra/` storage
///   directory) by returning `Unchanged` without touching the index.
///
/// Boundary conditions:
/// - `file` must be relative to `workdir`. Absolute paths or paths that
///   resolve outside the worktree return [`AddError::PathOutsideRepo`].
/// - Non-UTF-8 paths return [`AddError::InvalidPathEncoding`].
/// - LFS-tracked files are written as pointer blobs through
///   [`gen_blob_from_file`].
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

/// Internal classification of a path relative to the index. Drives the
/// branching in [`stage_a_file`] and the dry-run preview in [`run_add`].
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

/// Compute a [`FileStatus`] for `file` (relative to `workdir`) using the
/// in-memory `index`.
///
/// Functional scope:
/// - Uses `index.tracked` and `index.is_modified` to discriminate the four
///   live states; missing files are reported as `Deleted` when tracked, else
///   `NotFound`.
///
/// Boundary conditions:
/// - Returns [`AddError::InvalidPathEncoding`] when `file` is not UTF-8.
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

/// Generate a `Blob` from a file.
///
/// Functional scope:
/// - When the file matches a `.libraattributes` LFS filter, returns a pointer
///   blob via [`Blob::from_lfs_file`]; otherwise reads the file content
///   verbatim into a regular blob.
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

    /// Scenario: clap should reject incompatible mode flags up front so the
    /// user gets a parse-time error rather than ambiguous staging behavior.
    /// The `mode` clap group ties `-A`, `-u`, and `--refresh` together.
    #[test]
    fn test_args_conflict_with_refresh() {
        // "--refresh" cannot be combined with "-A", "--refresh" or "-u"
        assert!(AddArgs::try_parse_from(["test", "-A", "--refresh"]).is_err());
        assert!(AddArgs::try_parse_from(["test", "-u", "--refresh"]).is_err());
        assert!(AddArgs::try_parse_from(["test", "-A", "-u", "--refresh"]).is_err());
    }

    /// Scenario: smoke-test `total_staged` and `is_empty` because every
    /// rendering branch keys off these helpers — a regression here would
    /// produce wrong summary lines or wrong "nothing to add" detection.
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
