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
    env, fs,
    io::{self, BufRead, Read, Write},
    path::{Path, PathBuf},
};

use clap::Parser;
use git_internal::{
    errors::GitError,
    internal::{
        index::{Index, IndexEntry},
        object::{ObjectTrait, blob::Blob},
    },
};
use serde::Serialize;

use crate::{
    command::status::{self, Changes},
    internal::ai::automation::{VCS_EVENT_POST_ADD, dispatch_current_repo_vcs_event_to_history},
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        lfs,
        output::{self, OutputConfig},
        path, util,
    },
};

const ADD_EXAMPLES: &str = "\
EXAMPLES:
    libra add .                        Stage all changes in current directory
    libra add src/main.rs              Stage a specific file
    libra add src/ tests/              Stage multiple paths
    libra add -A                       Stage all changes (adds, modifies, removes)
    libra add -u                       Update tracked files only (no new files)
    libra add --dry-run .              Preview what would be staged
    libra add -f ignored_file.log      Force-add an ignored file
    libra add --refresh                Refresh index metadata without staging
    libra add --chmod=+x build.sh      Record the executable bit in the index (not the worktree)
    libra add --renormalize .          Re-stage tracked files (force-rewrite their blobs)
    libra add --pathspec-from-file list.txt   Stage paths read from a file ('-' for stdin)
    libra add --pathspec-from-file=- --pathspec-file-nul   Read NUL-separated paths from stdin
    libra add --dry-run --ignore-missing x    Preview; skip paths missing from the worktree";

/// Stage file contents for the next commit.
// EXAMPLES are wired via `#[command(after_help = ADD_EXAMPLES)]` and render
// at the bottom of `libra add --help`. The meta-commentary that used to live
// here as a `///` line leaked into clap's `--help` body (see
// `tests/command/add_test.rs::test_add_help_does_not_leak_impl_meta`).
#[derive(Parser, Debug, Default)]
#[command(after_help = ADD_EXAMPLES)]
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
    #[clap(long, overrides_with = "no_ignore_errors")]
    pub ignore_errors: bool,

    /// Force `--ignore-errors` off for this run, overriding a configured
    /// `add.ignoreErrors = true`. The effective value is: CLI flag > config >
    /// default (false).
    #[clap(long = "no-ignore-errors", overrides_with = "ignore_errors")]
    pub no_ignore_errors: bool,

    /// Override the executable bit recorded in the index for the staged
    /// entries: `+x` records mode 0o100755, `-x` records 0o100644.
    ///
    /// Only the index entry's mode is changed; working-tree file permissions
    /// are never modified.
    #[clap(long, value_name = "(+|-)x")]
    pub chmod: Option<String>,

    /// Re-stage already-tracked files matching the pathspec, bypassing the
    /// unchanged/modified short-circuit. Implies `-u` (tracked files only).
    ///
    /// Note: libra has no clean/CRLF filter, so this force-rewrites the tracked
    /// entries' blobs rather than normalizing line endings.
    #[clap(long)]
    pub renormalize: bool,

    /// Read pathspecs from the given file (`-` for stdin). Separated by newlines
    /// unless `--pathspec-file-nul` is given.
    #[clap(long, value_name = "FILE", conflicts_with = "pathspec")]
    pub pathspec_from_file: Option<String>,

    /// Treat `--pathspec-from-file` input as NUL-separated instead of
    /// line-separated. (Git's `add` has no `-z` short option.)
    #[clap(long, requires = "pathspec_from_file")]
    pub pathspec_file_nul: bool,

    /// Check whether the given paths would be ignored. Only valid together with
    /// `--dry-run`; missing paths are skipped with a warning instead of erroring.
    #[clap(long, requires = "dry_run")]
    pub ignore_missing: bool,

    /// (declined) Restrict to the sparse-checkout cone — not supported by libra.
    #[clap(long)]
    pub sparse: bool,

    /// (declined) Record an intent-to-add entry — libra's on-disk index cannot
    /// model it yet.
    #[clap(short = 'N', long = "intent-to-add")]
    pub intent_to_add: bool,

    /// (declined) Interactive patch-selection UI — not supported in Libra.
    /// Use `libra add <path>` to stage whole files, or the `libra code` TUI
    /// for interactive selection.
    #[clap(short = 'p', long = "patch")]
    pub patch: bool,
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
    /// `--sparse` was requested. Libra has no sparse-checkout support, so the
    /// flag is declined with a friendly usage error.
    #[error("sparse checkout is not supported by libra add")]
    SparseDeclined,
    /// `-N` / `--intent-to-add` was requested. The on-disk index format used by
    /// `git-internal` cannot model an intent-to-add entry, so it is declined.
    #[error("intent-to-add (-N/--intent-to-add) is not supported")]
    IntentToAddDeclined,
    /// `-p` / `--patch` was requested. Interactive patch-selection UI is a
    /// non-goal in Libra; declined per docs/improvement/compatibility/declined.md.
    #[error("add -p/--patch is not supported in Libra: interactive patch-selection UI is out of scope.")]
    PatchUiDeclined,
    /// `--chmod` was given a value other than `+x` / `-x`.
    #[error("invalid --chmod value '{spec}' (expected '+x' or '-x')")]
    InvalidChmod { spec: String },
    /// Reading a worktree file to build its blob failed (the fallible
    /// replacement for [`BlobExt::from_file`]'s panic path).
    #[error("failed to read '{path}': {source}")]
    BlobRead { path: PathBuf, source: io::Error },
    /// Writing a blob object (or its LFS backup) to storage failed (the
    /// fallible replacement for `BlobExt::save` / `BlobExt::from_lfs_file`).
    #[error("failed to write object for '{path}': {source}")]
    BlobSave { path: PathBuf, source: io::Error },
    /// The atomic index save failed at the fsync or rename step (an `io::Error`
    /// rather than the `GitError` produced by the temp-file write in
    /// [`IndexSave`](AddError::IndexSave)).
    #[error("unable to write index '{path}': {source}")]
    IndexSaveIo { path: PathBuf, source: io::Error },
    /// Reading the `--pathspec-from-file` input (a file or stdin) failed, or it
    /// exceeded the size limit. Mapped to [`StableErrorCode::IoReadFailed`].
    #[error("failed to read pathspec file '{path}': {source}")]
    PathspecFileRead { path: String, source: io::Error },
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
            // Declined flags reuse CliInvalidTarget (Cli category -> exit 129)
            // and carry a specific hint so users understand why the flag was
            // refused rather than silently ignored.
            AddError::SparseDeclined => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::CliInvalidTarget)
                .with_hint("libra does not support sparse checkout; remove --sparse"),
            AddError::IntentToAddDeclined => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::CliInvalidTarget)
                .with_hint(
                    "intent-to-add needs extended index capabilities, currently unsupported",
                ),
            AddError::PatchUiDeclined => CliError::command_usage(error.to_string())
                .with_stable_code(StableErrorCode::Unsupported)
                .with_hint(
                    "use 'libra add <path>' to stage whole files; see docs/improvement/compatibility/declined.md for details",
                ),
            AddError::InvalidChmod { .. } => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::CliInvalidTarget)
                .with_hint("only '+x' and '-x' are accepted"),
            AddError::BlobRead { .. } => {
                CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::IoReadFailed)
            }
            AddError::BlobSave { .. } => {
                CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::IoWriteFailed)
            }
            AddError::IndexSaveIo { .. } => {
                CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::IoWriteFailed)
            }
            AddError::PathspecFileRead { .. } => {
                CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::IoReadFailed)
            }
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

    fn wrote_index(&self) -> bool {
        !self.dry_run && !self.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Action tracking for add_a_file
// ---------------------------------------------------------------------------

/// The outcome of staging a single path. Returned by [`stage_a_file`] and
/// [`renormalize_entry`] so the caller can sort each path into the correct
/// [`AddOutput`] bucket. Public so the `--renormalize` rewrite can be asserted
/// directly in tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StagedAction {
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
    /// Pathspecs skipped under `--ignore-missing` because they do not exist in
    /// the working tree (dry-run only). Reported as stderr warnings, never in a
    /// JSON list.
    missing: Vec<String>,
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
/// # Side Effects
/// - Runs the staging pipeline via [`run_add`].
/// - Persists index updates unless `--dry-run` or `--refresh` short-circuits the
///   write path.
/// - Renders success output and records process-level warnings for ignored or
///   partially failed pathspecs.
///
/// # Errors
/// Returns [`CliError`] when repository discovery fails, pathspec validation
/// fails, ignored paths block staging, object/index I/O fails, or output
/// rendering fails.
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
    if result.wrote_index() {
        dispatch_current_repo_vcs_event_to_history(VCS_EVENT_POST_ADD).await;
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

    // --- Declined flags: surfaced as a friendly usage error (exit 129) now
    // that repository discovery has succeeded. Outside a repo, the lookups
    // above already returned NotInRepo (128) — matching `git add --sparse`
    // outside a repository.
    if args.sparse {
        return Err(AddError::SparseDeclined.into());
    }
    if args.intent_to_add {
        return Err(AddError::IntentToAddDeclined.into());
    }
    if args.patch {
        return Err(AddError::PatchUiDeclined.into());
    }
    if args.ignore_missing && !args.dry_run {
        return Err(
            CliError::command_usage("--ignore-missing can only be used with --dry-run")
                .with_stable_code(StableErrorCode::CliInvalidArguments)
                .with_hint("add --dry-run when checking paths that may be missing"),
        );
    }
    // Validate the --chmod value up front (whitelist: +x / -x). The mode change
    // itself is applied during staging.
    if let Some(spec) = args.chmod.as_deref() {
        validate_chmod_spec(spec)?;
    }
    // Effective --ignore-errors: explicit CLI flag > config `add.ignoreErrors`
    // > default false.
    let ignore_errors = resolve_ignore_errors(args).await;

    // Effective pathspec list: from --pathspec-from-file (or `-` for stdin) when
    // given, otherwise the positional pathspecs. clap guarantees the two are
    // mutually exclusive.
    let effective_pathspec: Vec<String> = match args.pathspec_from_file.as_deref() {
        Some(file) => read_pathspec_from_file(file, args.pathspec_file_nul)?,
        None => args.pathspec.clone(),
    };

    // Nothing-specified guard: an empty pathspec set is only valid for the
    // whole-tree modes. `--renormalize` implies `-u`, so (like `-A` / `-u` /
    // `--refresh`) it may run without an explicit pathspec over all tracked
    // files; `validate_pathspecs` maps an empty set to the workdir root.
    if effective_pathspec.is_empty()
        && !args.all
        && !args.update
        && !args.refresh
        && !args.renormalize
    {
        return Err(CliError::command_usage("nothing specified, nothing added")
            .with_stable_code(StableErrorCode::CliInvalidArguments)
            .with_hint("maybe you wanted to say 'libra add .'?"));
    }

    let mut index = Index::load(&index_path).map_err(|source| AddError::IndexLoad {
        path: index_path.clone(),
        source,
    })?;
    let current_dir = env::current_dir().map_err(|source| AddError::Workdir { source })?;

    let (mut visible_changes, mut ignored_changes) = if args.force {
        status::changes_to_be_staged_split_force().map_err(|source| AddError::Status { source })?
    } else {
        status::changes_to_be_staged_split_safe().map_err(|source| AddError::Status { source })?
    };
    if args.force {
        visible_changes.extend(ignored_changes.clone());
        ignored_changes = Changes::default();
    }

    let validated = validate_pathspecs(
        &effective_pathspec,
        &workdir,
        &current_dir,
        &visible_changes,
        &ignored_changes,
        &index,
        args.ignore_missing,
    )?;

    let mut add_output = AddOutput::empty(args.dry_run);

    // Collect ignored paths into output
    if !validated.ignored.is_empty() {
        let mut sorted_ignored = validated.ignored.clone();
        sorted_ignored.sort();
        sorted_ignored.dedup();
        add_output.ignored = sorted_ignored;
    }

    // --ignore-missing (dry-run only): pathspecs that do not exist in the
    // working tree are skipped with a stderr warning instead of erroring. They
    // are intentionally NOT added to any JSON list (stderr-only), and the
    // warning drives `--exit-code-on-warning`.
    if !validated.missing.is_empty() {
        for pathspec in &validated.missing {
            eprintln!(
                "warning: pathspec '{pathspec}' did not match any files and was skipped (--ignore-missing)"
            );
        }
        output::record_warning();
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
            save_index_atomic(&index, &index_path)?;
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
        if args.renormalize {
            // Preview the renormalize set the same way the real run computes it:
            // tracked entries matching the pathspec would be force-rewritten
            // (Modified), or staged as deletions (Removed) if removed from the
            // working tree. Keeps `--dry-run --renormalize` consistent with the
            // non-dry-run path instead of showing the (often empty) status set.
            let targets = filter_candidates(
                &index.tracked_files(),
                &validated.files,
                &workdir,
                &current_dir,
            );
            for file in &targets {
                let path_str = file.display().to_string();
                if workdir.join(file).exists() {
                    add_output.modified.push(path_str);
                } else {
                    add_output.removed.push(path_str);
                }
            }
        } else {
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
        }
        if let Some(spec) = args.chmod.as_deref() {
            let executable = validate_chmod_spec(spec)?;
            append_chmod_preview(
                &mut add_output,
                &index,
                &validated.files,
                &workdir,
                &current_dir,
                executable,
            );
        }
        return check_ignored_only_error(add_output);
    }

    // Stage each file. `--renormalize` implies `-u`: it force-rewrites the blobs
    // of already-tracked entries matching the pathspec (bypassing the
    // unchanged/modified short-circuit) and stages deletions for tracked files
    // removed from the working tree — untracked files are never added.
    let staging_files: Vec<PathBuf> = if args.renormalize {
        filter_candidates(
            &index.tracked_files(),
            &validated.files,
            &workdir,
            &current_dir,
        )
    } else {
        files
    };
    for file in &staging_files {
        let staged = if args.renormalize {
            renormalize_entry(file, &mut index, &workdir).await
        } else {
            stage_a_file(file, &mut index, &workdir, &storage_path).await
        };
        match staged {
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
                if !ignore_errors {
                    return Err(CliError::from(err));
                }
                add_output.failed.push(AddFailure {
                    path: file.display().to_string(),
                    message: err.to_string(),
                });
            }
        }
    }

    // --- Apply --chmod to matching tracked index entries. The candidate set is
    // every entry already in `.libra/index` that matches a requested pathspec —
    // not just the status-change set — so unchanged tracked files also have
    // their recorded index mode updated, matching `git add --chmod`. Only the
    // index mode is changed; working-tree permissions are never touched.
    if let Some(spec) = args.chmod.as_deref() {
        let executable = validate_chmod_spec(spec)?;
        let targets = filter_candidates(
            &index.tracked_files(),
            &validated.files,
            &workdir,
            &current_dir,
        );
        let mut chmod_changed = false;
        for rel in &targets {
            let Some(name) = rel.to_str() else { continue };
            // remove + re-insert (keyed by name) avoids borrowing the index
            // while mutating it and needs no Clone on IndexEntry.
            if let Some(mut entry) = index.remove(name, 0) {
                if apply_chmod(&mut entry, executable) {
                    chmod_changed = true;
                    // Report the path as modified only if staging did not
                    // already account for it (a newly added / content-modified /
                    // removed entry must not be double-counted by the chmod pass).
                    let path_str = rel.display().to_string();
                    let already_reported = add_output.added.contains(&path_str)
                        || add_output.modified.contains(&path_str)
                        || add_output.removed.contains(&path_str);
                    if !already_reported {
                        add_output.modified.push(path_str);
                    }
                }
                index.add(entry);
            }
        }
        // `core.fileMode = false` weakens executable-bit semantics, but Git (and
        // libra) still record the requested index mode. Warn on stderr — the
        // warning text is not suppressed by `-q` and drives
        // `--exit-code-on-warning`.
        if chmod_changed
            && matches!(
                read_cascaded_bool_case_variant("core.fileMode").await,
                Some(false)
            )
        {
            eprintln!(
                "warning: core.fileMode is false; --chmod still updates the recorded index mode"
            );
            output::record_warning();
        }
    }

    save_index_atomic(&index, &index_path)?;

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
/// - When `raw_pathspecs` is empty, returns the workdir root as the single
///   implicit pathspec (the whole-tree modes `-A` / `-u` / `--refresh` /
///   `--renormalize`).
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
    workdir: &Path,
    current_dir: &Path,
    visible_changes: &Changes,
    ignored_changes: &Changes,
    index: &Index,
    ignore_missing: bool,
) -> Result<ValidatedPathspecs, AddError> {
    if raw_pathspecs.is_empty() {
        return Ok(ValidatedPathspecs {
            files: vec![workdir.to_path_buf()],
            ignored: Vec::new(),
            missing: Vec::new(),
        });
    }

    let tracked_files = index.tracked_files();
    let change_candidates = collect_change_candidates(visible_changes);
    let ignored_candidates = collect_change_candidates(ignored_changes);

    let mut ignored = Vec::new();
    let mut files = Vec::new();
    let mut missing = Vec::new();
    for raw in raw_pathspecs {
        let requested_path = PathBuf::from(raw);
        let requested_abs = resolve_pathspec(&requested_path, current_dir);
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

        // Nothing matched. With --ignore-missing (dry-run only), a pathspec that
        // does not exist in the working tree is skipped with a warning; an
        // existing-but-unmatched pathspec still errors, matching Git's
        // "ignored-even-if-missing" being scoped to genuinely missing paths.
        if ignore_missing && !requested_abs.exists() {
            missing.push(raw.clone());
            continue;
        }

        return Err(AddError::PathspecNotMatched {
            pathspec: raw.clone(),
        });
    }

    Ok(ValidatedPathspecs {
        files,
        ignored,
        missing,
    })
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

    // Skip directories - they cannot be staged as blobs
    if file_abs.is_dir() {
        return Ok(StagedAction::Unchanged);
    }

    let file_status = check_file_status(file, index, workdir)?;
    match file_status {
        FileStatus::New => {
            let blob = gen_blob_from_file(&file_abs)?;
            save_blob(&blob, &file_abs)?;
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
                let blob = gen_blob_from_file(&file_abs)?;
                if !index.verify_hash(file_str, 0, &blob.id) {
                    save_blob(&blob, &file_abs)?;
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

/// Build a [`Blob`] from a worktree file, fallibly.
///
/// This is the fallible replacement for the previous panic path
/// (`BlobExt::from_file` / `from_lfs_file`): a missing/unreadable file becomes
/// [`AddError::BlobRead`] and an LFS backup failure becomes
/// [`AddError::BlobSave`], so `--ignore-errors` can skip the file instead of
/// aborting the whole command. For LFS-tracked paths we first probe readability
/// with `File::open`, because `lfs::generate_pointer_file` opens the file and
/// panics on failure (a `stat` succeeds on a `chmod 000` file, an `open` does
/// not).
fn gen_blob_from_file(path: impl AsRef<Path>) -> Result<Blob, AddError> {
    let path = path.as_ref();
    if lfs::is_lfs_tracked(path) {
        let oid = lfs::calc_lfs_file_hash(path).map_err(|source| AddError::BlobRead {
            path: path.to_path_buf(),
            source,
        })?;
        let size = path
            .metadata()
            .map_err(|source| AddError::BlobRead {
                path: path.to_path_buf(),
                source,
            })?
            .len();
        let pointer = lfs::format_pointer_string(&oid, size);
        lfs::backup_lfs_file(path, &oid).map_err(|source| AddError::BlobSave {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(Blob::from_content(&pointer))
    } else {
        let data = fs::read(path).map_err(|source| AddError::BlobRead {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(Blob::from_content_bytes(data))
    }
}

/// Persist `blob` to object storage if it is not already present.
///
/// The fallible replacement for `BlobExt::save`'s panic path: a `storage.put`
/// failure becomes [`AddError::BlobSave`] (carrying the worktree `path` for
/// context) so `--ignore-errors` can skip it.
fn save_blob(blob: &Blob, path: &Path) -> Result<(), AddError> {
    let storage = util::objects_storage();
    if !storage.exist(&blob.id) {
        storage
            .put(&blob.id, &blob.data, blob.get_type())
            .map_err(|source| AddError::BlobSave {
                path: path.to_path_buf(),
                source,
            })?;
    }
    Ok(())
}

/// Persist `index` to `index_path` atomically: write to a sibling temp file,
/// fsync it, then rename over the target.
///
/// `git-internal`'s own `Index::save` uses `File::create` (truncate-in-place,
/// non-atomic), so a crash or I/O error mid-write could leave a partially
/// written `.libra/index`. Writing to a temp file in the same directory and
/// renaming makes the replacement atomic: on any failure (temp write, fsync, or
/// rename) the original index is left untouched and the temp file is cleaned up.
///
/// Exposed (`pub`) so failure-injection tests can assert the no-partial-write
/// guarantee directly.
pub fn save_index_atomic(index: &Index, index_path: &Path) -> Result<(), AddError> {
    let dir = index_path.parent().unwrap_or_else(|| Path::new("."));
    let tmp_path = dir.join(format!("index.{}.tmp", std::process::id()));

    if let Err(source) = index.save(&tmp_path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(AddError::IndexSave {
            path: tmp_path.clone(),
            source,
        });
    }

    // fsync the temp file so a crash between write and rename cannot expose a
    // half-flushed index.
    if let Err(source) = fs::File::open(&tmp_path).and_then(|f| f.sync_all()) {
        let _ = fs::remove_file(&tmp_path);
        return Err(AddError::IndexSaveIo {
            path: index_path.to_path_buf(),
            source,
        });
    }

    if let Err(source) = fs::rename(&tmp_path, index_path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(AddError::IndexSaveIo {
            path: index_path.to_path_buf(),
            source,
        });
    }

    // fsync the parent directory so the rename itself is durable across a crash.
    // Opening a directory as a file is not portable (e.g. Windows), so the open
    // is best-effort; a genuine sync failure is surfaced.
    if let Ok(dir_handle) = fs::File::open(dir) {
        dir_handle
            .sync_all()
            .map_err(|source| AddError::IndexSaveIo {
                path: index_path.to_path_buf(),
                source,
            })?;
    }
    Ok(())
}

/// Set the executable bit on a staged index entry's mode: `+x` -> `0o100755`,
/// `-x` -> `0o100644`. Returns whether the mode actually changed (so callers can
/// decide whether to report the path as modified). Only the index entry's mode
/// is touched; working-tree file permissions are never modified.
fn apply_chmod(entry: &mut IndexEntry, executable: bool) -> bool {
    let new_mode = chmod_mode(executable);
    let changed = entry.mode != new_mode;
    entry.mode = new_mode;
    changed
}

fn chmod_mode(executable: bool) -> u32 {
    if executable { 0o100755 } else { 0o100644 }
}

fn append_chmod_preview(
    output: &mut AddOutput,
    index: &Index,
    requested_paths: &[PathBuf],
    workdir: &Path,
    current_dir: &Path,
    executable: bool,
) {
    let target_mode = chmod_mode(executable);
    let targets = filter_candidates(
        &index.tracked_files(),
        requested_paths,
        workdir,
        current_dir,
    );
    for rel in &targets {
        let Some(name) = rel.to_str() else { continue };
        let Some(entry) = index.get(name, 0) else {
            continue;
        };
        if entry.mode == target_mode {
            continue;
        }
        let path_str = rel.display().to_string();
        let already_reported = output.added.contains(&path_str)
            || output.modified.contains(&path_str)
            || output.removed.contains(&path_str);
        if !already_reported {
            output.modified.push(path_str);
        }
    }
}

/// Upper bound on `--pathspec-from-file` input (file or stdin) to guard against
/// OOM / DoS from a pathological input.
const MAX_PATHSPEC_FILE_BYTES: u64 = 128 * 1024 * 1024;

/// Read pathspecs from `path` (a file, or `-` for stdin), streaming.
///
/// Items are separated by NUL when `nul` is set (`--pathspec-file-nul`),
/// otherwise by newline (a trailing `\r` is stripped so CRLF files work). Empty
/// items are dropped. Input is read incrementally via [`BufRead::read_until`]
/// and bounded at [`MAX_PATHSPEC_FILE_BYTES`] as it is consumed, so even an
/// unbounded stdin pipe cannot exhaust memory; exceeding the cap (or any read
/// failure) returns [`AddError::PathspecFileRead`] and non-UTF-8 input returns
/// [`AddError::InvalidPathEncoding`]. Returned items are raw pathspecs; the
/// caller still normalises and bounds-checks each one via [`validate_pathspecs`].
fn read_pathspec_from_file(path: &str, nul: bool) -> Result<Vec<String>, AddError> {
    let separator = if nul { b'\0' } else { b'\n' };
    let (label, reader): (String, Box<dyn Read>) = if path == "-" {
        ("<stdin>".to_string(), Box::new(io::stdin().lock()))
    } else {
        // Fail fast on an oversized file without opening/reading it.
        let meta = fs::metadata(path).map_err(|source| AddError::PathspecFileRead {
            path: path.to_string(),
            source,
        })?;
        if meta.len() > MAX_PATHSPEC_FILE_BYTES {
            return Err(AddError::PathspecFileRead {
                path: path.to_string(),
                source: io::Error::new(
                    io::ErrorKind::InvalidData,
                    "pathspec file exceeds the 128 MiB limit",
                ),
            });
        }
        let file = fs::File::open(path).map_err(|source| AddError::PathspecFileRead {
            path: path.to_string(),
            source,
        })?;
        (path.to_string(), Box::new(file))
    };

    // `take` bounds the total read so an unbounded stdin pipe cannot exhaust
    // memory; `total` enforces the cap precisely as bytes are consumed.
    let mut reader = io::BufReader::new(reader.take(MAX_PATHSPEC_FILE_BYTES + 1));
    let mut items = Vec::new();
    let mut chunk = Vec::new();
    let mut total: u64 = 0;
    loop {
        chunk.clear();
        let read = reader.read_until(separator, &mut chunk).map_err(|source| {
            AddError::PathspecFileRead {
                path: label.clone(),
                source,
            }
        })?;
        if read == 0 {
            break;
        }
        total += read as u64;
        if total > MAX_PATHSPEC_FILE_BYTES {
            return Err(AddError::PathspecFileRead {
                path: label.clone(),
                source: io::Error::new(
                    io::ErrorKind::InvalidData,
                    "pathspec input exceeds the 128 MiB limit",
                ),
            });
        }
        if chunk.last() == Some(&separator) {
            chunk.pop();
        }
        if !nul && chunk.last() == Some(&b'\r') {
            chunk.pop();
        }
        if chunk.is_empty() {
            continue;
        }
        let item = std::str::from_utf8(&chunk).map_err(|_| AddError::InvalidPathEncoding {
            path: PathBuf::from(&label),
        })?;
        items.push(item.to_string());
    }
    Ok(items)
}

/// Force-rewrite an already-tracked entry for `--renormalize` (implies `-u`).
///
/// Unlike [`stage_a_file`], this bypasses the `FileStatus::Unchanged` /
/// `is_modified` / `verify_hash` short-circuits: it unconditionally regenerates
/// the blob for a tracked file and replaces the index entry, so even a file
/// whose stat/content appears unchanged is rewritten. A tracked file deleted
/// from the working tree is staged as a deletion (the `-u` part). Because libra
/// has no clean/CRLF filter, this is a force-rewrite, not EOL normalisation.
///
/// `file` must be the workdir-relative path of an entry already in the index;
/// callers restrict the candidate set to tracked files matching the pathspec.
/// Public so the rewrite action can be asserted directly in tests.
pub async fn renormalize_entry(
    file: &Path,
    index: &mut Index,
    workdir: &Path,
) -> Result<StagedAction, AddError> {
    let file_str = file.to_str().ok_or_else(|| AddError::InvalidPathEncoding {
        path: file.to_path_buf(),
    })?;
    let file_abs = workdir.join(file);

    if !file_abs.exists() {
        // Tracked but deleted in the working tree -> stage the deletion.
        index.remove(file_str, 0);
        return Ok(StagedAction::Removed);
    }
    if file_abs.is_dir() {
        return Ok(StagedAction::Unchanged);
    }

    let blob = gen_blob_from_file(&file_abs)?;
    save_blob(&blob, &file_abs)?;
    index.update(
        IndexEntry::new_from_file(file, blob.id, workdir).map_err(|source| {
            AddError::CreateIndexEntry {
                path: file.to_path_buf(),
                source,
            }
        })?,
    );
    Ok(StagedAction::Modified)
}

/// Validate a `--chmod` spec. Only `+x` and `-x` are accepted (matching Git's
/// `git add --chmod`). Returns `true` for `+x` (executable; index mode
/// `0o100755`) and `false` for `-x` (index mode `0o100644`).
///
/// Any other value is rejected with [`AddError::InvalidChmod`]; the echoed spec
/// is capped at 8 characters so pathological input cannot bloat the message.
fn validate_chmod_spec(spec: &str) -> Result<bool, AddError> {
    match spec {
        "+x" => Ok(true),
        "-x" => Ok(false),
        _ => Err(AddError::InvalidChmod {
            spec: spec.chars().take(8).collect(),
        }),
    }
}

/// Resolve the effective `--ignore-errors` setting.
///
/// Precedence: an explicit CLI flag (`--ignore-errors` / `--no-ignore-errors`)
/// wins; otherwise the `add.ignoreErrors` config value (local scope first, then
/// global) applies; otherwise the default is `false`. A config read failure is
/// treated as "unset" so a broken config never blocks staging.
///
/// Exposed (`pub`) so integration tests can assert the CLI > config > default
/// precedence directly without depending on an actual staging failure.
pub async fn resolve_ignore_errors(args: &AddArgs) -> bool {
    if args.no_ignore_errors {
        return false;
    }
    if args.ignore_errors {
        return true;
    }
    read_cascaded_bool_case_variant("add.ignoreErrors")
        .await
        .unwrap_or(false)
}

async fn read_cascaded_bool_case_variant(key: &str) -> Option<bool> {
    if let Ok(Some(value)) = crate::internal::config::read_cascaded_bool(key).await {
        return Some(value);
    }

    let lowercase = key.to_ascii_lowercase();
    if lowercase == key {
        return None;
    }

    crate::internal::config::read_cascaded_bool(&lowercase)
        .await
        .ok()
        .flatten()
}

#[cfg(test)]
mod test {
    use super::*;

    /// Pin the `Display` format for the static-message and direct-message
    /// variants of [`AddError`]. These strings are used as the `CliError`
    /// message via `From<AddError> for CliError` and surface in both
    /// human and `--json` envelopes.
    ///
    /// Source-chained variants (IndexLoad, IndexSave, RefreshFailed,
    /// CreateIndexEntry, Workdir, Status) wrap upstream error sources
    /// and are intentionally skipped — their `{source}` slot is owned
    /// by the wrapped error type.
    #[test]
    fn add_error_display_pins_static_message_variants() {
        assert_eq!(
            AddError::NotInRepo.to_string(),
            "not a libra repository (or any of the parent directories): .libra",
        );
        assert_eq!(
            AddError::PathspecNotMatched {
                pathspec: "src/missing.rs".to_string(),
            }
            .to_string(),
            "pathspec 'src/missing.rs' did not match any files",
        );
        assert_eq!(
            AddError::PathOutsideRepo {
                path: "/tmp/elsewhere".to_string(),
                repo_root: PathBuf::from("/home/user/repo"),
            }
            .to_string(),
            "'/tmp/elsewhere' is outside repository at '/home/user/repo'",
        );
        assert_eq!(
            AddError::InvalidPathEncoding {
                path: PathBuf::from("src/foo"),
            }
            .to_string(),
            "path 'src/foo' is not valid UTF-8",
        );
        assert_eq!(
            AddError::SparseDeclined.to_string(),
            "sparse checkout is not supported by libra add",
        );
        assert_eq!(
            AddError::IntentToAddDeclined.to_string(),
            "intent-to-add (-N/--intent-to-add) is not supported",
        );
        assert_eq!(
            AddError::InvalidChmod {
                spec: "foo".to_string(),
            }
            .to_string(),
            "invalid --chmod value 'foo' (expected '+x' or '-x')",
        );
    }

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

    /// Scenario: `libra add -h` must surface every newly added flag so users
    /// can discover them. Renders the clap help and asserts each long flag.
    #[test]
    fn test_add_help_lists_new_flags() {
        use clap::CommandFactory;
        let help = AddArgs::command().render_long_help().to_string();
        for flag in [
            "--chmod",
            "--renormalize",
            "--pathspec-from-file",
            "--pathspec-file-nul",
            "--ignore-missing",
            "--sparse",
            "--intent-to-add",
        ] {
            assert!(help.contains(flag), "help is missing {flag}:\n{help}");
        }
    }

    /// Scenario: `--chmod` only accepts `+x` / `-x`; any other value is a
    /// validation error and the echoed spec is length-capped.
    #[test]
    fn test_add_chmod_rejects_invalid_value() {
        assert!(validate_chmod_spec("+x").expect("+x is valid"));
        assert!(!validate_chmod_spec("-x").expect("-x is valid"));
        assert!(matches!(
            validate_chmod_spec("foo"),
            Err(AddError::InvalidChmod { .. })
        ));
        // Length cap: an over-long spec is rejected and truncated to <= 8 chars
        // in the echoed message.
        match validate_chmod_spec("+xxxxxxxxxxxxxxxx") {
            Err(AddError::InvalidChmod { spec }) => assert!(spec.chars().count() <= 8),
            other => panic!("expected InvalidChmod, got {other:?}"),
        }
    }

    /// Scenario: `--pathspec-file-nul` is meaningless without
    /// `--pathspec-from-file`; clap `requires` must reject the lone flag.
    #[test]
    fn test_add_pathspec_file_nul_requires_from_file() {
        assert!(AddArgs::try_parse_from(["add", "--pathspec-file-nul"]).is_err());
        assert!(
            AddArgs::try_parse_from([
                "add",
                "--pathspec-from-file",
                "list.txt",
                "--pathspec-file-nul",
            ])
            .is_ok()
        );
    }

    /// Scenario: `--pathspec-from-file` conflicts with positional pathspecs
    /// (Git rejects mixing the two).
    #[test]
    fn test_add_pathspec_from_file_conflicts_positional() {
        assert!(
            AddArgs::try_parse_from(["add", "--pathspec-from-file", "list.txt", "foo.rs"]).is_err()
        );
        // The flag alone (no positional) parses fine.
        assert!(AddArgs::try_parse_from(["add", "--pathspec-from-file", "list.txt"]).is_ok());
    }

    /// Scenario: `--ignore-missing` is only valid with `--dry-run` (Git
    /// semantics); clap `requires` must enforce this.
    #[test]
    fn test_add_ignore_missing_requires_dry_run() {
        assert!(AddArgs::try_parse_from(["add", "--ignore-missing", "f"]).is_err());
        assert!(AddArgs::try_parse_from(["add", "--ignore-missing", "--dry-run", "f"]).is_ok());
    }

    /// Scenario: `--no-ignore-errors` and `--ignore-errors` override each other
    /// so the last one on the command line wins (drives the tri-state in
    /// [`resolve_ignore_errors`]).
    #[test]
    fn test_add_ignore_errors_negation_overrides() {
        let a = AddArgs::try_parse_from(["add", "--ignore-errors", "--no-ignore-errors", "f"])
            .expect("parses");
        assert!(!a.ignore_errors && a.no_ignore_errors);
        let b = AddArgs::try_parse_from(["add", "--no-ignore-errors", "--ignore-errors", "f"])
            .expect("parses");
        assert!(b.ignore_errors && !b.no_ignore_errors);
    }
}
