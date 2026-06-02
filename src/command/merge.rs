//! Merge command orchestration that resolves base/target commits, performs recursive merge, stages results, and updates refs or surfaces conflicts.

#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet, VecDeque},
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

use clap::{Parser, ValueEnum};
use git_internal::{
    hash::{ObjectHash, get_hash_kind},
    internal::{
        index::{Index, IndexEntry},
        object::{
            blob::Blob,
            commit::Commit,
            tree::{Tree, TreeItem, TreeItemMode},
        },
    },
};
use serde::{Deserialize, Serialize};

use super::{
    get_target_commit, load_object, log, reset,
    restore::{self, RestoreArgs},
    save_object, stash, status, switch,
};
use crate::{
    common_utils::format_commit_msg,
    info_println,
    internal::{
        branch::{Branch, BranchStoreError},
        config::{LocalIdentityTarget, env_first_non_empty, read_cascaded_config_value},
        db::get_db_conn_instance,
        head::Head,
        reflog::{ReflogAction, ReflogContext, with_reflog},
    },
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        object_ext::TreeExt,
        output::{OutputConfig, emit_json_data},
        path, util, worktree,
    },
};

/// `--help` examples shown in `libra merge --help` output.
///
pub const MERGE_EXAMPLES: &str = "\
EXAMPLES:
    libra merge feature-x          Fast-forward current branch onto feature-x if possible
    libra merge origin/main        Fast-forward onto a remote-tracking branch
    libra merge --ff-only feature  Refuse unless feature can fast-forward HEAD
    libra merge --no-ff feature    Always create a merge commit when possible
    libra merge -m 'Merge topic' feature
    libra merge --continue         Finish an in-progress merge after resolving conflicts
    libra merge --abort            Restore the pre-merge HEAD, index, and worktree
    libra merge --quit             Forget merge state but keep index/worktree as-is
    libra merge --json feature-x   Structured JSON output for agents

NOTES:
    Divergent single-head merges create a merge commit when paths do not
    conflict. Conflicts write markers and can be finished with --continue
    or restored with --abort.";

#[derive(Parser, Debug)]
#[command(after_help = MERGE_EXAMPLES)]
pub struct MergeArgs {
    /// Branches or commits to merge into the current branch
    pub branches: Vec<String>,

    /// Continue an in-progress merge after resolving conflicts
    #[arg(long = "continue", conflicts_with_all = ["abort", "quit"])]
    pub continue_merge: bool,

    /// Abort an in-progress merge and restore the pre-merge state
    #[arg(long, conflicts_with_all = ["continue_merge", "quit"])]
    pub abort: bool,

    /// Forget an in-progress merge while leaving index and working tree untouched
    #[arg(long, conflicts_with_all = ["continue_merge", "abort"])]
    pub quit: bool,

    /// Refuse to merge unless HEAD can be fast-forwarded
    #[arg(long, conflicts_with = "no_ff")]
    pub ff_only: bool,

    /// Create a merge commit even when the merge could fast-forward
    #[arg(long = "no-ff", conflicts_with = "ff_only")]
    pub no_ff: bool,

    /// Squash changes into the index and working tree without creating a merge commit
    #[arg(long)]
    pub squash: bool,

    /// Perform the merge but stop before creating a merge commit
    #[arg(long = "no-commit", conflicts_with = "commit")]
    pub no_commit: bool,

    /// Create a merge commit after a clean merge
    #[arg(long)]
    pub commit: bool,

    /// Allow merging histories with no common ancestor
    #[arg(long)]
    pub allow_unrelated_histories: bool,

    /// Stash local changes before merging and reapply them afterward
    #[arg(long, conflicts_with = "no_autostash")]
    pub autostash: bool,

    /// Do not autostash local changes (default; overrides merge.autoStash)
    #[arg(long = "no-autostash", conflicts_with = "autostash")]
    pub no_autostash: bool,

    /// Use the given merge commit message
    #[arg(short, long, conflicts_with = "file")]
    pub message: Option<String>,

    /// Read merge commit message from file
    #[arg(short = 'F', long, conflicts_with = "message")]
    pub file: Option<String>,

    /// Add Signed-off-by trailer to the merge commit message
    #[arg(long)]
    pub signoff: bool,

    /// Merge strategy to use (currently only 'ours')
    #[arg(short = 's', long, value_enum)]
    pub strategy: Option<MergeStrategy>,

    /// Pass a strategy option (currently ours or theirs) to the three-way merge
    #[arg(short = 'X', long = "strategy-option", value_enum)]
    pub strategy_option: Option<MergeFavor>,

    /// Append up to n one-line commit summaries to the merge commit message
    #[arg(long, num_args = 0..=1, default_missing_value = "20")]
    pub log: Option<usize>,

    /// Do not append a shortlog to the merge commit message (overrides --log)
    #[arg(long = "no-log", conflicts_with = "log")]
    pub no_log: bool,

    /// Do not add a Signed-off-by trailer (overrides --signoff)
    #[arg(long = "no-signoff", conflicts_with = "signoff")]
    pub no_signoff: bool,

    /// Create a merge commit instead of squashing (default; overrides --squash)
    #[arg(long = "no-squash", conflicts_with = "squash")]
    pub no_squash: bool,

    /// Override the branch name recorded in the merge commit message
    #[arg(long = "into-name", value_name = "NAME")]
    pub into_name: Option<String>,

    /// Show a diffstat of what the merge brought in
    #[arg(long, visible_alias = "summary", conflicts_with = "no_stat")]
    pub stat: bool,

    /// Suppress the merge diffstat
    #[arg(
        short = 'n',
        long = "no-stat",
        visible_alias = "no-summary",
        conflicts_with = "stat"
    )]
    pub no_stat: bool,

    /// Conflict marker style
    #[arg(long, value_enum)]
    pub conflict: Option<MergeConflictStyle>,

    /// Diff algorithm to use for content merges (myers, histogram, patience, minimal)
    #[arg(long = "diff-algorithm", value_name = "ALGO")]
    pub diff_algorithm: Option<String>,

    /// How to clean up the merge commit message (accepted for Git compatibility)
    #[arg(long, value_name = "MODE")]
    pub cleanup: Option<String>,

    /// Skip pre-merge/commit-msg hooks (accepted; Libra runs no merge hooks yet)
    #[arg(long = "no-verify")]
    pub no_verify: bool,

    /// Overwrite ignored files when merging (default; accepted for Git compatibility)
    #[arg(long = "overwrite-ignore", conflicts_with = "no_overwrite_ignore")]
    pub overwrite_ignore: bool,

    /// Do not overwrite ignored files when merging (accepted for Git compatibility)
    #[arg(long = "no-overwrite-ignore", conflicts_with = "overwrite_ignore")]
    pub no_overwrite_ignore: bool,

    /// Update the rerere resolution database (accepted; Libra has no rerere store)
    #[arg(long = "rerere-autoupdate", conflicts_with = "no_rerere_autoupdate")]
    pub rerere_autoupdate: bool,

    /// Do not update the rerere database (default; accepted for Git compatibility)
    #[arg(long = "no-rerere-autoupdate", conflicts_with = "rerere_autoupdate")]
    pub no_rerere_autoupdate: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum MergeStrategy {
    Ours,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum MergeFavor {
    Ours,
    Theirs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize, Default)]
pub enum MergeConflictStyle {
    #[default]
    Merge,
    Diff3,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PullMergeSummary {
    pub strategy: String,
    /// The previous HEAD commit before merge (None for root commits).
    pub old_commit: Option<String>,
    pub commit: Option<String>,
    pub files_changed: usize,
    pub up_to_date: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parents: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflicted_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub aborted: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub continued: bool,
}

pub(crate) type MergeOutput = PullMergeSummary;

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Default)]
pub(crate) struct PullMergeOptions {
    pub ff_only: bool,
    pub no_ff: bool,
    pub squash: bool,
    pub no_commit: bool,
    pub allow_unrelated_histories: bool,
    pub message: Option<String>,
    pub signoff: bool,
    pub strategy: Option<MergeStrategy>,
    pub strategy_option: Option<MergeFavor>,
    pub log: Option<usize>,
    pub conflict_style: MergeConflictStyle,
    pub into_name: Option<String>,
    pub autostash: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct MergeState {
    pub head_name: String,
    pub orig_head: String,
    pub target: String,
    pub target_ref: String,
    pub base: String,
    pub conflicted_paths: Vec<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub signoff: bool,
    #[serde(default)]
    pub log: Option<usize>,
    #[serde(default)]
    pub conflict_style: MergeConflictStyle,
    /// Stash id saved by `--autostash`, reapplied on `--continue`/`--abort`.
    #[serde(default)]
    pub autostash: Option<String>,
}

impl MergeState {
    fn path() -> PathBuf {
        util::storage_path().join("merge-state.json")
    }

    pub(crate) fn load_optional_sync() -> Result<Option<Self>, String> {
        let path = Self::path();
        if !path.exists() {
            return Ok(None);
        }
        let data = fs::read_to_string(&path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        serde_json::from_str(&data)
            .map(Some)
            .map_err(|error| format!("failed to parse {}: {error}", path.display()))
    }

    fn load_required() -> Result<Self, PullMergeError> {
        Self::load_optional_sync()
            .map_err(PullMergeError::StateLoad)?
            .ok_or(PullMergeError::NoMergeInProgress)
    }

    fn save(&self) -> Result<(), PullMergeError> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                PullMergeError::StateSave(format!("failed to create {}: {error}", parent.display()))
            })?;
        }
        let data = serde_json::to_vec_pretty(self)
            .map_err(|error| PullMergeError::StateSave(error.to_string()))?;
        fs::write(&path, data)
            .map_err(|error| PullMergeError::StateSave(format!("{}: {error}", path.display())))
    }

    fn cleanup() -> Result<(), PullMergeError> {
        let path = Self::path();
        if !path.exists() {
            return Ok(());
        }
        fs::remove_file(&path)
            .map_err(|error| PullMergeError::StateCleanup(format!("{}: {error}", path.display())))
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum PullMergeError {
    #[error("merge requires a branch argument, --continue, or --abort")]
    MissingAction,
    #[error("merge accepts either a branch argument, --continue, or --abort")]
    ConflictingAction,
    #[error("--squash cannot be combined with --no-ff")]
    SquashNoFf,
    #[error("--squash cannot be combined with --commit")]
    SquashCommit,
    #[error("invalid merge.ff value '{value}'")]
    InvalidMergeFfConfig { value: String },
    #[error("unknown diff algorithm '{value}' (expected myers, histogram, patience, or minimal)")]
    InvalidDiffAlgorithm { value: String },
    #[error(
        "unknown cleanup mode '{value}' (expected strip, whitespace, verbatim, scissors, or default)"
    )]
    InvalidCleanupMode { value: String },
    #[error("octopus merge refused: {detail}")]
    OctopusConflict { detail: String },
    #[error("directory/file conflict in merge: {path}")]
    DirectoryFileConflict { path: String },
    #[error("failed to read merge message file '{path}': {detail}")]
    MessageFileRead { path: String, detail: String },
    #[error("merge --signoff requires configured user.name and user.email")]
    SignoffIdentity,
    #[error("{0} - not something we can merge")]
    InvalidTarget(String),
    #[error("failed to load merge target '{commit_id}': {detail}")]
    TargetLoad { commit_id: String, detail: String },
    #[error("failed to load current commit '{commit_id}': {detail}")]
    CurrentLoad { commit_id: String, detail: String },
    #[error("failed to inspect merge history: {0}")]
    History(String),
    #[error("refusing to merge unrelated histories")]
    UnrelatedHistories,
    #[error("merge has conflicts in {paths}")]
    Conflicts { paths: String },
    #[error("no merge in progress")]
    NoMergeInProgress,
    #[error("merge already in progress")]
    MergeInProgress,
    #[error("you must resolve all merge conflicts before continuing")]
    UnresolvedConflicts,
    #[error("uncommitted changes, cannot merge")]
    DirtyWorktree,
    #[error("untracked working tree file would be overwritten by merge: {path}")]
    UntrackedOverwrite { path: String },
    #[error("non-fast-forward merge refused (current {current}, target {target})")]
    NonFastForward { current: String, target: String },
    #[error("failed to load merge state: {0}")]
    StateLoad(String),
    #[error("failed to save merge state: {0}")]
    StateSave(String),
    #[error("failed to clean up merge state: {0}")]
    StateCleanup(String),
    #[error("autostash failed: {0}")]
    Autostash(String),
    #[error("failed to load index: {0}")]
    IndexLoad(String),
    #[error("failed to save index: {0}")]
    IndexSave(String),
    #[error("failed to create merge tree: {0}")]
    TreeCreate(String),
    #[error("failed to save merge commit: {0}")]
    CommitSave(String),
    #[error("failed to reset working tree after merge: {0}")]
    WorkdirReset(String),
    #[error("failed to load tree '{tree_id}': {detail}")]
    TreeLoad { tree_id: String, detail: String },
    #[error("failed to load object '{object_id}': {detail}")]
    ObjectLoad { object_id: String, detail: String },
    #[error("failed to resolve HEAD state: {0}")]
    HeadResolve(String),
    #[error("failed to update HEAD during merge: {0}")]
    HeadUpdate(String),
    #[error("failed to restore working tree after merge: {0}")]
    Restore(String),
}

pub(crate) type MergeError = PullMergeError;

impl From<PullMergeError> for CliError {
    fn from(error: PullMergeError) -> Self {
        match &error {
            PullMergeError::MissingAction
            | PullMergeError::ConflictingAction
            | PullMergeError::SquashNoFf
            | PullMergeError::SquashCommit
            | PullMergeError::InvalidMergeFfConfig { .. }
            | PullMergeError::InvalidDiffAlgorithm { .. }
            | PullMergeError::InvalidCleanupMode { .. } => {
                CliError::command_usage(error.to_string())
                    .with_stable_code(StableErrorCode::CliInvalidArguments)
            }
            PullMergeError::MessageFileRead { .. } => {
                CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::IoReadFailed)
            }
            PullMergeError::SignoffIdentity => CliError::failure(error.to_string())
                .with_stable_code(StableErrorCode::RepoStateInvalid)
                .with_hint("configure user.name and user.email before using --signoff"),
            PullMergeError::InvalidTarget(..) => CliError::command_usage(error.to_string())
                .with_stable_code(StableErrorCode::CliInvalidTarget),
            PullMergeError::TargetLoad { .. }
            | PullMergeError::CurrentLoad { .. }
            | PullMergeError::History(..)
            | PullMergeError::TreeLoad { .. }
            | PullMergeError::ObjectLoad { .. } => {
                CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::RepoCorrupt)
            }
            PullMergeError::UnrelatedHistories => CliError::failure(error.to_string())
                .with_stable_code(StableErrorCode::RepoStateInvalid),
            PullMergeError::NonFastForward { .. } => CliError::failure(error.to_string())
                .with_stable_code(StableErrorCode::ConflictOperationBlocked)
                .with_hint("run 'libra pull' without --ff-only to allow a merge commit")
                .with_hint("or run 'libra pull --rebase' to replay local commits"),
            PullMergeError::Conflicts { .. }
            | PullMergeError::OctopusConflict { .. }
            | PullMergeError::DirectoryFileConflict { .. }
            | PullMergeError::DirtyWorktree
            | PullMergeError::UntrackedOverwrite { .. }
            | PullMergeError::MergeInProgress
            | PullMergeError::UnresolvedConflicts => CliError::failure(error.to_string())
                .with_stable_code(StableErrorCode::ConflictOperationBlocked)
                .with_hint("resolve conflicts, then run 'libra merge --continue'")
                .with_hint("or run 'libra merge --abort' to restore the pre-merge state"),
            PullMergeError::NoMergeInProgress => CliError::failure(error.to_string())
                .with_stable_code(StableErrorCode::RepoStateInvalid),
            PullMergeError::StateLoad(..) | PullMergeError::IndexLoad(..) => {
                CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::IoReadFailed)
            }
            PullMergeError::StateSave(..)
            | PullMergeError::StateCleanup(..)
            | PullMergeError::Autostash(..)
            | PullMergeError::IndexSave(..)
            | PullMergeError::TreeCreate(..)
            | PullMergeError::CommitSave(..)
            | PullMergeError::WorkdirReset(..) => {
                CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::IoWriteFailed)
            }
            PullMergeError::HeadResolve(..) => {
                CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::IoReadFailed)
            }
            PullMergeError::HeadUpdate(..) | PullMergeError::Restore(..) => {
                CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::IoWriteFailed)
            }
        }
    }
}

pub async fn execute(args: MergeArgs) {
    if let Err(err) = execute_safe(args, &OutputConfig::default()).await {
        err.print_stderr();
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting.
///
/// # Side Effects
/// - Resolves and reads the current and target commits.
/// - Performs a fast-forward merge for supported cases.
/// - Updates HEAD/current branch and restores the working tree to the merged
///   tree state.
/// - Emits merge status text through [`OutputConfig`].
///
/// # Errors
/// Returns [`CliError`] when the target is invalid, histories are unrelated,
/// conflicts need resolution, objects cannot be read, or HEAD/worktree updates fail.
pub async fn execute_safe(args: MergeArgs, output: &OutputConfig) -> CliResult<()> {
    let want_stat = resolve_merge_stat(&args).await;
    let result = run_merge(args, output).await.map_err(merge_error_to_cli)?;
    render_merge_output(&result, want_stat, output)
}

/// Resolve whether to print a diffstat after the merge. `--stat`/`--no-stat`
/// (and the `--summary`/`--no-summary` aliases) win over the `merge.stat`
/// config key. Unlike Git, Libra defaults the diffstat off so existing merge
/// output stays stable unless explicitly requested.
async fn resolve_merge_stat(args: &MergeArgs) -> bool {
    if args.no_stat {
        return false;
    }
    if args.stat {
        return true;
    }
    read_cascaded_config_value(LocalIdentityTarget::CurrentRepo, "merge.stat")
        .await
        .ok()
        .flatten()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "true" | "yes" | "on" | "1"
            )
        })
        .unwrap_or(false)
}

async fn run_merge(args: MergeArgs, output: &OutputConfig) -> Result<MergeOutput, MergeError> {
    let options = merge_options_from_args(&args).await?;
    match (
        args.branches.as_slice(),
        args.continue_merge,
        args.abort,
        args.quit,
    ) {
        ([branch], false, false, false) => {
            let stash_id = maybe_autostash_push(&options).await?;
            let result = run_merge_for_pull_with_options(branch, branch, output, options).await;
            finalize_autostash(stash_id, result).await
        }
        (branches, false, false, false) if branches.len() > 1 => {
            let stash_id = maybe_autostash_push(&options).await?;
            let result = run_octopus_merge(branches, output, options).await;
            finalize_autostash(stash_id, result).await
        }
        ([], true, false, false) => run_merge_continue(output, options).await,
        ([], false, true, false) => run_merge_abort(output).await,
        ([], false, false, true) => run_merge_quit().await,
        ([], false, false, false) => Err(MergeError::MissingAction),
        _ => Err(MergeError::ConflictingAction),
    }
}

/// Stash the working tree before a merge when `--autostash` is in effect.
/// Returns the saved stash id, or `None` when autostash is off or the tree is
/// already clean.
async fn maybe_autostash_push(options: &PullMergeOptions) -> Result<Option<String>, MergeError> {
    if !options.autostash {
        return Ok(None);
    }
    stash::autostash_push()
        .await
        .map_err(PullMergeError::Autostash)
}

/// Reapply (or defer) an autostash once the merge settles. A merge that left
/// state behind (conflict or `--no-commit`) records the stash id so
/// `--continue`/`--abort` can reapply it; otherwise the stash is popped now.
async fn finalize_autostash(
    stash_id: Option<String>,
    result: Result<MergeOutput, MergeError>,
) -> Result<MergeOutput, MergeError> {
    let Some(stash_id) = stash_id else {
        return result;
    };
    if let Some(mut state) = MergeState::load_optional_sync().map_err(PullMergeError::StateLoad)? {
        state.autostash = Some(stash_id);
        state.save()?;
        return result;
    }
    if let Err(error) = stash::autostash_pop().await {
        eprintln!("warning: failed to reapply autostashed changes: {error}");
    }
    result
}

async fn merge_options_from_args(args: &MergeArgs) -> Result<PullMergeOptions, MergeError> {
    if args.squash && args.no_ff {
        return Err(MergeError::SquashNoFf);
    }
    if args.squash && args.commit {
        return Err(MergeError::SquashCommit);
    }
    validate_diff_algorithm(args.diff_algorithm.as_deref())?;
    validate_cleanup_mode(args.cleanup.as_deref())?;
    let mut ff_only = args.ff_only;
    let mut no_ff = args.no_ff;
    if !ff_only && !no_ff {
        apply_merge_ff_config(&mut ff_only, &mut no_ff).await?;
    }
    Ok(PullMergeOptions {
        ff_only,
        no_ff,
        squash: args.squash,
        no_commit: args.no_commit,
        allow_unrelated_histories: args.allow_unrelated_histories,
        message: read_merge_message(args)?,
        signoff: args.signoff,
        strategy: args.strategy,
        strategy_option: args.strategy_option,
        log: if args.no_log { None } else { args.log },
        conflict_style: resolve_conflict_style(args.conflict).await?,
        into_name: args.into_name.clone(),
        autostash: resolve_autostash(args).await,
    })
}

/// Resolve `--autostash`/`--no-autostash`, falling back to the
/// `merge.autoStash` config key (default off).
async fn resolve_autostash(args: &MergeArgs) -> bool {
    if args.no_autostash {
        return false;
    }
    if args.autostash {
        return true;
    }
    read_cascaded_config_value(LocalIdentityTarget::CurrentRepo, "merge.autoStash")
        .await
        .ok()
        .flatten()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "true" | "yes" | "on" | "1"
            )
        })
        .unwrap_or(false)
}

/// Accept Git's `--diff-algorithm` flag for content merges. Libra's blob
/// merge currently uses a single Myers-style backend, so we validate the
/// requested algorithm name and proceed with that backend rather than
/// silently ignoring an unknown value.
fn validate_diff_algorithm(algorithm: Option<&str>) -> Result<(), MergeError> {
    match algorithm {
        None => Ok(()),
        Some(name) => match name.trim().to_ascii_lowercase().as_str() {
            "myers" | "histogram" | "patience" | "minimal" => Ok(()),
            other => Err(MergeError::InvalidDiffAlgorithm {
                value: other.to_string(),
            }),
        },
    }
}

/// Accept Git's `--cleanup=<mode>` flag for the merge message. The mode is
/// validated against Git's documented set; the actual message body produced
/// by Libra is already trimmed, so non-`verbatim` modes are equivalent here.
fn validate_cleanup_mode(mode: Option<&str>) -> Result<(), MergeError> {
    match mode {
        None => Ok(()),
        Some(value) => match value.trim().to_ascii_lowercase().as_str() {
            "strip" | "whitespace" | "verbatim" | "scissors" | "default" => Ok(()),
            other => Err(MergeError::InvalidCleanupMode {
                value: other.to_string(),
            }),
        },
    }
}

async fn resolve_conflict_style(
    cli_style: Option<MergeConflictStyle>,
) -> Result<MergeConflictStyle, PullMergeError> {
    if let Some(style) = cli_style {
        return Ok(style);
    }
    let Some(value) =
        read_cascaded_config_value(LocalIdentityTarget::CurrentRepo, "merge.conflictstyle")
            .await
            .ok()
            .flatten()
    else {
        return Ok(MergeConflictStyle::Merge);
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "merge" => Ok(MergeConflictStyle::Merge),
        "diff3" => Ok(MergeConflictStyle::Diff3),
        _ => Ok(MergeConflictStyle::Merge),
    }
}

async fn apply_merge_ff_config(ff_only: &mut bool, no_ff: &mut bool) -> Result<(), PullMergeError> {
    let Some(value) = read_cascaded_config_value(LocalIdentityTarget::CurrentRepo, "merge.ff")
        .await
        .ok()
        .flatten()
    else {
        return Ok(());
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Ok(()),
        "false" | "no" | "off" | "0" => {
            *no_ff = true;
            Ok(())
        }
        "only" => {
            *ff_only = true;
            Ok(())
        }
        _ => Err(PullMergeError::InvalidMergeFfConfig { value }),
    }
}

fn read_merge_message(args: &MergeArgs) -> Result<Option<String>, MergeError> {
    if let Some(message) = &args.message {
        return Ok(Some(message.clone()));
    }
    if let Some(file_path) = &args.file {
        return fs::read_to_string(file_path).map(Some).map_err(|error| {
            MergeError::MessageFileRead {
                path: file_path.clone(),
                detail: error.to_string(),
            }
        });
    }
    Ok(None)
}

fn render_merge_output(
    result: &MergeOutput,
    want_stat: bool,
    output: &OutputConfig,
) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("merge", result, output);
    }
    if output.quiet {
        return Ok(());
    }

    if want_stat
        && !result.up_to_date
        && !result.aborted
        && result.conflicted_paths.is_empty()
        && let Some(stat) = render_merge_diffstat(result)
    {
        info_println!(output, "{stat}");
    }

    if result.up_to_date {
        info_println!(output, "Already up to date.");
    } else if result.aborted {
        info_println!(output, "Merge aborted.");
    } else if result.continued {
        info_println!(output, "Merge completed.");
    } else if !result.conflicted_paths.is_empty() {
        info_println!(
            output,
            "Automatic merge failed; fix conflicts and then commit the result."
        );
    } else {
        match result.strategy.as_str() {
            "three-way" => info_println!(output, "Merge made by the 'three-way' strategy."),
            "octopus" => info_println!(output, "Merge made by the 'octopus' strategy."),
            "squash" => info_println!(output, "Squash commit -- not updating HEAD."),
            "no-commit" => info_println!(
                output,
                "Automatic merge went well; stopped before committing as requested."
            ),
            _ => info_println!(output, "Fast-forward"),
        }
    }
    Ok(())
}

fn merge_error_to_cli(error: MergeError) -> CliError {
    match error {
        MergeError::Conflicts { .. } => CliError::from(error)
            .with_priority_hint("resolve conflicts, then run 'libra merge --continue'")
            .with_hint("or run 'libra merge --abort' to restore the pre-merge state"),
        error => CliError::from(error),
    }
}

pub(crate) async fn run_merge_for_pull_with_options(
    target_ref: &str,
    upstream: &str,
    output: &OutputConfig,
    mut options: PullMergeOptions,
) -> Result<PullMergeSummary, PullMergeError> {
    if !options.ff_only && !options.no_ff {
        apply_merge_ff_config(&mut options.ff_only, &mut options.no_ff).await?;
    }
    if MergeState::load_optional_sync()
        .map_err(PullMergeError::StateLoad)?
        .is_some()
    {
        return Err(PullMergeError::MergeInProgress);
    }

    let commit_hash = resolve_merge_target(target_ref)
        .await
        .map_err(|_| PullMergeError::InvalidTarget(upstream.to_string()))?;
    let target_commit: Commit =
        load_object(&commit_hash).map_err(|error| PullMergeError::TargetLoad {
            commit_id: commit_hash.to_string(),
            detail: error.to_string(),
        })?;

    let Some(current_commit_id) = Head::current_commit().await else {
        let files_changed = count_changed_files(None, &target_commit)?;
        apply_fast_forward_merge(target_commit.clone(), upstream, output).await?;
        return Ok(PullMergeSummary {
            strategy: "fast-forward".to_string(),
            old_commit: None,
            commit: Some(target_commit.id.to_string()),
            files_changed,
            up_to_date: false,
            parents: Vec::new(),
            conflicted_paths: Vec::new(),
            aborted: false,
            continued: false,
        });
    };
    let current_commit: Commit =
        load_object(&current_commit_id).map_err(|error| PullMergeError::CurrentLoad {
            commit_id: current_commit_id.to_string(),
            detail: error.to_string(),
        })?;

    let lca = lca_commit(&current_commit, &target_commit)
        .await
        .map_err(|error| PullMergeError::History(error.to_string()))?;

    if lca.is_none() && !options.allow_unrelated_histories {
        return Err(PullMergeError::UnrelatedHistories);
    }

    if lca.as_ref().is_some_and(|base| base.id == target_commit.id) {
        return Ok(PullMergeSummary {
            strategy: "already-up-to-date".to_string(),
            old_commit: Some(current_commit_id.to_string()),
            commit: None,
            files_changed: 0,
            up_to_date: true,
            parents: Vec::new(),
            conflicted_paths: Vec::new(),
            aborted: false,
            continued: false,
        });
    }

    if lca
        .as_ref()
        .is_some_and(|base| base.id == current_commit.id)
        && !options.no_ff
    {
        let files_changed = count_changed_files(Some(&current_commit), &target_commit)?;
        if options.squash {
            apply_squash_merge(&target_commit)?;
            return Ok(PullMergeSummary {
                strategy: "squash".to_string(),
                old_commit: Some(current_commit_id.to_string()),
                commit: None,
                files_changed,
                up_to_date: false,
                parents: Vec::new(),
                conflicted_paths: Vec::new(),
                aborted: false,
                continued: false,
            });
        }
        apply_fast_forward_merge(target_commit.clone(), upstream, output).await?;
        return Ok(PullMergeSummary {
            strategy: "fast-forward".to_string(),
            old_commit: Some(current_commit_id.to_string()),
            commit: Some(target_commit.id.to_string()),
            files_changed,
            up_to_date: false,
            parents: Vec::new(),
            conflicted_paths: Vec::new(),
            aborted: false,
            continued: false,
        });
    }

    if options.ff_only {
        return Err(PullMergeError::NonFastForward {
            current: current_commit.id.to_string(),
            target: target_commit.id.to_string(),
        });
    }

    perform_three_way_merge(
        current_commit,
        target_commit,
        lca,
        upstream,
        output,
        options,
    )
    .await
}

async fn run_octopus_merge(
    branches: &[String],
    output: &OutputConfig,
    options: PullMergeOptions,
) -> Result<PullMergeSummary, PullMergeError> {
    if options.squash
        || options.no_commit
        || options.strategy.is_some()
        || options.strategy_option.is_some()
    {
        return Err(PullMergeError::OctopusConflict {
            detail: "advanced merge options are only supported for single-head merges".to_string(),
        });
    }
    switch::ensure_clean_status(output)
        .await
        .map_err(|_| PullMergeError::DirtyWorktree)?;
    let current_commit_id =
        Head::current_commit()
            .await
            .ok_or_else(|| PullMergeError::OctopusConflict {
                detail: "current branch has no commits".to_string(),
            })?;
    let current_commit: Commit =
        load_object(&current_commit_id).map_err(|error| PullMergeError::CurrentLoad {
            commit_id: current_commit_id.to_string(),
            detail: error.to_string(),
        })?;
    let current_items = commit_tree_items(&current_commit)?;
    let mut merged_items = current_items.clone();
    let mut parents = vec![current_commit.id];
    let mut changed_paths = HashSet::new();
    let mut target_names = Vec::new();

    for branch in branches {
        let commit_hash = resolve_merge_target(branch)
            .await
            .map_err(|_| PullMergeError::InvalidTarget(branch.clone()))?;
        let target_commit: Commit =
            load_object(&commit_hash).map_err(|error| PullMergeError::TargetLoad {
                commit_id: commit_hash.to_string(),
                detail: error.to_string(),
            })?;
        let lca = lca_commit(&current_commit, &target_commit)
            .await
            .map_err(|error| PullMergeError::History(error.to_string()))?;
        if lca.as_ref().is_none_or(|base| base.id != current_commit.id) {
            return Err(PullMergeError::OctopusConflict {
                detail: format!("target '{branch}' is not a clean descendant of HEAD"),
            });
        }
        let target_items = commit_tree_items(&target_commit)?;
        for (path, target_entry) in &target_items {
            if current_items.get(path) == Some(target_entry) {
                continue;
            }
            if let Some(existing) = merged_items.get(path)
                && existing != target_entry
                && changed_paths.contains(path)
            {
                return Err(PullMergeError::OctopusConflict {
                    detail: format!("path '{}' changed by multiple heads", path.display()),
                });
            }
            changed_paths.insert(path.clone());
            merged_items.insert(path.clone(), *target_entry);
        }
        for path in current_items.keys() {
            if target_items.contains_key(path) {
                continue;
            }
            if changed_paths.contains(path) {
                return Err(PullMergeError::OctopusConflict {
                    detail: format!("path '{}' changed by multiple heads", path.display()),
                });
            }
            changed_paths.insert(path.clone());
            merged_items.remove(path);
        }
        parents.push(target_commit.id);
        target_names.push(branch.clone());
    }

    ensure_no_directory_file_conflicts(merged_items.keys())?;
    let current_index =
        Index::load(path::index()).map_err(|error| PullMergeError::IndexLoad(error.to_string()))?;
    let paths_to_write: Vec<PathBuf> = merged_items.keys().cloned().collect();
    ensure_no_untracked_conflicts(&current_index, &paths_to_write)?;
    let tree_id = create_tree_from_items_map(&merged_items).map_err(PullMergeError::TreeCreate)?;
    let head_name = current_head_name().await?;
    let into_label = options.into_name.as_deref().unwrap_or(&head_name);
    let message = options.message.clone().unwrap_or_else(|| {
        format!(
            "Merge branches {} into {into_label}",
            target_names.join(", ")
        )
    });
    let merge_commit =
        Commit::from_tree_id(tree_id, parents.clone(), &format_commit_msg(&message, None));
    save_object(&merge_commit, &merge_commit.id)
        .map_err(|error| PullMergeError::CommitSave(error.to_string()))?;
    update_head_with_reflog(
        &head_name,
        merge_commit.id,
        &target_names.join(","),
        "octopus",
    )
    .await?;
    reset_index_and_workdir_to_tree(&tree_id)?;
    let files_changed = count_item_map_changes(&current_items, &merged_items);

    Ok(PullMergeSummary {
        strategy: "octopus".to_string(),
        old_commit: Some(current_commit.id.to_string()),
        commit: Some(merge_commit.id.to_string()),
        files_changed,
        up_to_date: false,
        parents: parents
            .into_iter()
            .map(|parent| parent.to_string())
            .collect(),
        conflicted_paths: Vec::new(),
        aborted: false,
        continued: false,
    })
}

struct ThreeWayMergeResult {
    merged_items: HashMap<PathBuf, MergeTreeEntry>,
    conflicts: Vec<(PathBuf, ConflictKind)>,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
struct MergeTreeEntry {
    hash: ObjectHash,
    mode: TreeItemMode,
}

async fn perform_three_way_merge(
    current_commit: Commit,
    target_commit: Commit,
    base_commit: Option<Commit>,
    upstream: &str,
    output: &OutputConfig,
    options: PullMergeOptions,
) -> Result<PullMergeSummary, PullMergeError> {
    switch::ensure_clean_status(output)
        .await
        .map_err(|_| PullMergeError::DirtyWorktree)?;

    let head_name = current_head_name().await?;
    let base_items = match &base_commit {
        Some(commit) => commit_tree_items(commit)?,
        None => HashMap::new(),
    };
    let our_items = commit_tree_items(&current_commit)?;
    let their_items = commit_tree_items(&target_commit)?;
    let merge_result = if options.strategy == Some(MergeStrategy::Ours) {
        ThreeWayMergeResult {
            merged_items: our_items.clone(),
            conflicts: Vec::new(),
        }
    } else {
        merge_tree_items_with_options(&base_items, &our_items, &their_items, &options)?
    };
    let files_changed = count_item_map_changes(&our_items, &merge_result.merged_items);
    ensure_no_directory_file_conflicts(
        merge_result
            .merged_items
            .keys()
            .chain(merge_result.conflicts.iter().map(|(path, _)| path)),
    )?;

    if !merge_result.conflicts.is_empty() {
        write_conflicted_merge_state(MergeConflictInput {
            head_name: head_name.clone(),
            upstream: upstream.to_string(),
            base: base_commit
                .as_ref()
                .map(|commit| commit.id)
                .unwrap_or_else(zero_object_hash),
            ours: current_commit.id,
            theirs: target_commit.id,
            merged_items: merge_result.merged_items,
            conflicts: merge_result.conflicts,
            base_items,
            our_items,
            their_items,
            message: merge_commit_message(
                upstream,
                &head_name,
                Some((&current_commit, &target_commit)),
                &options,
            )
            .await?,
            signoff: options.signoff,
            log: options.log,
            conflict_style: options.conflict_style,
        })?;
        let paths = MergeState::load_required()?.conflicted_paths.join(", ");
        return Err(PullMergeError::Conflicts { paths });
    }

    let current_index =
        Index::load(path::index()).map_err(|error| PullMergeError::IndexLoad(error.to_string()))?;
    let paths_to_write: Vec<PathBuf> = merge_result.merged_items.keys().cloned().collect();
    ensure_no_untracked_conflicts(&current_index, &paths_to_write)?;

    let tree_id = create_tree_from_items_map(&merge_result.merged_items)
        .map_err(PullMergeError::TreeCreate)?;
    if options.squash {
        reset_index_and_workdir_to_tree(&tree_id)?;
        return Ok(PullMergeSummary {
            strategy: "squash".to_string(),
            old_commit: Some(current_commit.id.to_string()),
            commit: None,
            files_changed,
            up_to_date: false,
            parents: Vec::new(),
            conflicted_paths: Vec::new(),
            aborted: false,
            continued: false,
        });
    }
    let message = merge_commit_message(
        upstream,
        &head_name,
        Some((&current_commit, &target_commit)),
        &options,
    )
    .await?;
    if options.no_commit {
        save_clean_merge_state(CleanMergeStateInput {
            head_name: head_name.clone(),
            upstream: upstream.to_string(),
            base: base_commit
                .as_ref()
                .map(|commit| commit.id)
                .unwrap_or_else(zero_object_hash),
            ours: current_commit.id,
            theirs: target_commit.id,
            message: message.clone(),
            signoff: options.signoff,
            log: options.log,
            conflict_style: options.conflict_style,
        })?;
        reset_index_and_workdir_to_tree(&tree_id)?;
        return Ok(PullMergeSummary {
            strategy: "no-commit".to_string(),
            old_commit: Some(current_commit.id.to_string()),
            commit: None,
            files_changed,
            up_to_date: false,
            parents: vec![current_commit.id.to_string(), target_commit.id.to_string()],
            conflicted_paths: Vec::new(),
            aborted: false,
            continued: false,
        });
    }
    let merge_commit = Commit::from_tree_id(
        tree_id,
        vec![current_commit.id, target_commit.id],
        &format_commit_msg(&message, None),
    );
    save_object(&merge_commit, &merge_commit.id)
        .map_err(|error| PullMergeError::CommitSave(error.to_string()))?;
    update_head_with_reflog(&head_name, merge_commit.id, upstream, "three-way").await?;
    reset_index_and_workdir_to_tree(&tree_id)?;

    Ok(PullMergeSummary {
        strategy: "three-way".to_string(),
        old_commit: Some(current_commit.id.to_string()),
        commit: Some(merge_commit.id.to_string()),
        files_changed,
        up_to_date: false,
        parents: vec![current_commit.id.to_string(), target_commit.id.to_string()],
        conflicted_paths: Vec::new(),
        aborted: false,
        continued: false,
    })
}

struct MergeConflictInput {
    head_name: String,
    upstream: String,
    base: ObjectHash,
    ours: ObjectHash,
    theirs: ObjectHash,
    merged_items: HashMap<PathBuf, MergeTreeEntry>,
    conflicts: Vec<(PathBuf, ConflictKind)>,
    base_items: HashMap<PathBuf, MergeTreeEntry>,
    our_items: HashMap<PathBuf, MergeTreeEntry>,
    their_items: HashMap<PathBuf, MergeTreeEntry>,
    message: String,
    signoff: bool,
    log: Option<usize>,
    conflict_style: MergeConflictStyle,
}

struct CleanMergeStateInput {
    head_name: String,
    upstream: String,
    base: ObjectHash,
    ours: ObjectHash,
    theirs: ObjectHash,
    message: String,
    signoff: bool,
    log: Option<usize>,
    conflict_style: MergeConflictStyle,
}

fn save_clean_merge_state(input: CleanMergeStateInput) -> Result<(), PullMergeError> {
    MergeState {
        head_name: input.head_name,
        orig_head: input.ours.to_string(),
        target: input.theirs.to_string(),
        target_ref: input.upstream,
        base: input.base.to_string(),
        conflicted_paths: Vec::new(),
        message: Some(input.message),
        signoff: input.signoff,
        log: input.log,
        conflict_style: input.conflict_style,
        autostash: None,
    }
    .save()
}

fn write_conflicted_merge_state(input: MergeConflictInput) -> Result<(), PullMergeError> {
    let current_index =
        Index::load(path::index()).map_err(|error| PullMergeError::IndexLoad(error.to_string()))?;

    let conflict_paths: Vec<PathBuf> = input
        .conflicts
        .iter()
        .map(|(path, _)| path.clone())
        .collect();
    let paths_to_write: Vec<PathBuf> = input
        .merged_items
        .keys()
        .cloned()
        .chain(conflict_paths.iter().cloned())
        .collect();
    ensure_no_untracked_conflicts(&current_index, &paths_to_write)?;

    let conflict_set: HashSet<PathBuf> = conflict_paths.iter().cloned().collect();
    let workdir = util::working_dir();
    let marker_eol = conflict_marker_eol();
    let theirs_abbrev = short_object_id(&input.theirs);

    let mut index = Index::new();
    for (path, entry) in &input.merged_items {
        add_blob_index_entry(&mut index, path, *entry, 0)?;
    }
    for path in &conflict_paths {
        if let Some(entry) = input.base_items.get(path) {
            add_blob_index_entry(&mut index, path, *entry, 1)?;
        }
        if let Some(entry) = input.our_items.get(path) {
            add_blob_index_entry(&mut index, path, *entry, 2)?;
        }
        if let Some(entry) = input.their_items.get(path) {
            add_blob_index_entry(&mut index, path, *entry, 3)?;
        }
    }

    let state = MergeState {
        head_name: input.head_name,
        orig_head: input.ours.to_string(),
        target: input.theirs.to_string(),
        target_ref: input.upstream,
        base: input.base.to_string(),
        conflicted_paths: conflict_paths
            .iter()
            .map(|path| path.display().to_string())
            .collect(),
        message: Some(input.message),
        signoff: input.signoff,
        log: input.log,
        conflict_style: input.conflict_style,
        autostash: None,
    };
    state.save()?;

    if let Err(error) = index.save(path::index()) {
        let _ = MergeState::cleanup();
        return Err(PullMergeError::IndexSave(error.to_string()));
    }

    for (path, entry) in &input.merged_items {
        let blob: Blob = load_object(&entry.hash).map_err(|error| {
            PullMergeError::WorkdirReset(format!(
                "failed to load merged blob {} for '{}': {error}",
                entry.hash,
                path.display()
            ))
        })?;
        write_workdir_blob(&workdir, path, entry.mode, &blob.data)
            .map_err(PullMergeError::WorkdirReset)?;
    }

    let mut tracked_paths: HashSet<PathBuf> = current_index.tracked_files().into_iter().collect();
    tracked_paths.extend(input.base_items.keys().cloned());
    tracked_paths.extend(input.our_items.keys().cloned());
    tracked_paths.extend(input.their_items.keys().cloned());
    for path in tracked_paths {
        if conflict_set.contains(&path) || input.merged_items.contains_key(&path) {
            continue;
        }
        let full_path = workdir.join(&path);
        if full_path.exists() {
            fs::remove_file(&full_path).map_err(|error| {
                PullMergeError::WorkdirReset(format!(
                    "failed to remove {}: {error}",
                    path.display()
                ))
            })?;
        }
    }

    for (path, kind) in &input.conflicts {
        let base = input.base_items.get(path).map(|entry| entry.hash);
        write_conflict_markers(
            &workdir,
            path,
            marker_eol,
            &theirs_abbrev,
            *kind,
            base,
            input.conflict_style,
        )
        .map_err(PullMergeError::WorkdirReset)?;
    }

    Ok(())
}

async fn run_merge_continue(
    _output: &OutputConfig,
    options: PullMergeOptions,
) -> Result<MergeOutput, MergeError> {
    let state = MergeState::load_required()?;
    ensure_no_unstaged_changes_for_continue()?;
    let index =
        Index::load(path::index()).map_err(|error| MergeError::IndexLoad(error.to_string()))?;
    if has_unmerged_entries(&index) {
        return Err(MergeError::UnresolvedConflicts);
    }

    let orig_head = object_hash_from_state("orig_head", &state.orig_head)?;
    let target = object_hash_from_state("target", &state.target)?;
    let original_commit: Commit =
        load_object(&orig_head).map_err(|error| MergeError::CurrentLoad {
            commit_id: orig_head.to_string(),
            detail: error.to_string(),
        })?;
    let original_items = commit_tree_items(&original_commit)?;
    let index_items = index_tree_items(&index)?;
    let files_changed = count_item_map_changes(&original_items, &index_items);
    let tree_id = create_tree_from_items_map(&index_items).map_err(MergeError::TreeCreate)?;
    let message = match options.message.clone().or(state.message) {
        Some(message) => message,
        None => merge_commit_message(&state.target_ref, &state.head_name, None, &options).await?,
    };
    let merge_commit = Commit::from_tree_id(
        tree_id,
        vec![orig_head, target],
        &format_commit_msg(&message, None),
    );
    save_object(&merge_commit, &merge_commit.id)
        .map_err(|error| MergeError::CommitSave(error.to_string()))?;
    update_head_with_reflog(
        &state.head_name,
        merge_commit.id,
        &state.target_ref,
        "three-way",
    )
    .await?;
    reset_index_and_workdir_to_tree(&tree_id)?;
    MergeState::cleanup()?;
    reapply_recorded_autostash(state.autostash.as_deref()).await;

    Ok(PullMergeSummary {
        strategy: "three-way".to_string(),
        old_commit: Some(orig_head.to_string()),
        commit: Some(merge_commit.id.to_string()),
        files_changed,
        up_to_date: false,
        parents: vec![orig_head.to_string(), target.to_string()],
        conflicted_paths: Vec::new(),
        aborted: false,
        continued: true,
    })
}

/// Reapply an autostash recorded in the merge state once the merge finishes
/// via `--continue`/`--abort`. Failures are surfaced as a warning rather than
/// failing the completed operation, mirroring Git.
async fn reapply_recorded_autostash(autostash: Option<&str>) {
    if autostash.is_none() {
        return;
    }
    if let Err(error) = stash::autostash_pop().await {
        eprintln!("warning: failed to reapply autostashed changes: {error}");
    }
}

async fn run_merge_quit() -> Result<MergeOutput, MergeError> {
    MergeState::load_required()?;
    MergeState::cleanup()?;
    Ok(PullMergeSummary {
        strategy: "quit".to_string(),
        old_commit: None,
        commit: None,
        files_changed: 0,
        up_to_date: false,
        parents: Vec::new(),
        conflicted_paths: Vec::new(),
        aborted: false,
        continued: false,
    })
}

fn ensure_no_unstaged_changes_for_continue() -> Result<(), PullMergeError> {
    let unstaged = status::changes_to_be_staged()
        .map_err(|error| PullMergeError::IndexLoad(error.to_string()))?;
    if !unstaged.modified.is_empty() || !unstaged.deleted.is_empty() {
        return Err(PullMergeError::DirtyWorktree);
    }
    Ok(())
}

async fn run_merge_abort(_output: &OutputConfig) -> Result<MergeOutput, MergeError> {
    let state = MergeState::load_required()?;
    let orig_head = object_hash_from_state("orig_head", &state.orig_head)?;
    update_head_with_reflog(&state.head_name, orig_head, &state.target_ref, "abort").await?;
    let original_commit: Commit =
        load_object(&orig_head).map_err(|error| MergeError::CurrentLoad {
            commit_id: orig_head.to_string(),
            detail: error.to_string(),
        })?;
    reset_index_and_workdir_to_tree(&original_commit.tree_id)?;
    MergeState::cleanup()?;
    reapply_recorded_autostash(state.autostash.as_deref()).await;

    Ok(PullMergeSummary {
        strategy: "abort".to_string(),
        old_commit: Some(orig_head.to_string()),
        commit: Some(orig_head.to_string()),
        files_changed: 0,
        up_to_date: false,
        parents: Vec::new(),
        conflicted_paths: Vec::new(),
        aborted: true,
        continued: false,
    })
}

async fn resolve_merge_target(target_ref: &str) -> Result<ObjectHash, Box<dyn std::error::Error>> {
    if let Some(remote) = target_ref.strip_prefix("refs/remotes/")
        && let Some((remote_name, _)) = remote.split_once('/')
        && let Some(branch) = Branch::find_branch_result(target_ref, Some(remote_name))
            .await
            .map_err(|error: BranchStoreError| Box::new(error) as Box<dyn std::error::Error>)?
    {
        return Ok(branch.commit);
    }

    get_target_commit(target_ref).await
}

async fn lca_commit(lhs: &Commit, rhs: &Commit) -> Result<Option<Commit>, CliError> {
    let lhs_reachable = log::get_reachable_commits(lhs.id.to_string(), None).await?;
    let rhs_reachable = log::get_reachable_commits(rhs.id.to_string(), None).await?;
    Ok(best_common_ancestor(lhs_reachable, rhs_reachable))
}

fn best_common_ancestor(lhs_reachable: Vec<Commit>, rhs_reachable: Vec<Commit>) -> Option<Commit> {
    let lhs_distance = commit_distances(&lhs_reachable);
    let rhs_distance = commit_distances(&rhs_reachable);
    let lhs_by_id: HashMap<ObjectHash, Commit> = lhs_reachable
        .into_iter()
        .map(|commit| (commit.id, commit))
        .collect();
    let parent_map: HashMap<ObjectHash, Vec<ObjectHash>> = lhs_by_id
        .values()
        .chain(rhs_reachable.iter())
        .map(|commit| (commit.id, commit.parent_commit_ids.clone()))
        .collect();
    let common: Vec<ObjectHash> = lhs_distance
        .keys()
        .filter(|id| rhs_distance.contains_key(id))
        .copied()
        .collect();
    let best_ids: Vec<ObjectHash> = common
        .iter()
        .copied()
        .filter(|candidate| {
            !common
                .iter()
                .any(|other| other != candidate && commit_reaches(*other, *candidate, &parent_map))
        })
        .collect();

    best_ids
        .into_iter()
        .min_by_key(|id| {
            let lhs = lhs_distance.get(id).copied().unwrap_or(usize::MAX);
            let rhs = rhs_distance.get(id).copied().unwrap_or(usize::MAX);
            (lhs.max(rhs), lhs + rhs, id.to_string())
        })
        .and_then(|id| lhs_by_id.get(&id).cloned())
}

fn commit_distances(commits: &[Commit]) -> HashMap<ObjectHash, usize> {
    commits
        .iter()
        .enumerate()
        .map(|(distance, commit)| (commit.id, distance))
        .collect()
}

fn commit_reaches(
    start: ObjectHash,
    target: ObjectHash,
    parent_map: &HashMap<ObjectHash, Vec<ObjectHash>>,
) -> bool {
    let mut seen = HashSet::new();
    let mut queue = VecDeque::from([start]);
    while let Some(id) = queue.pop_front() {
        if id == target {
            return true;
        }
        if !seen.insert(id) {
            continue;
        }
        if let Some(parents) = parent_map.get(&id) {
            queue.extend(parents.iter().copied());
        }
    }
    false
}

async fn apply_fast_forward_merge(
    target_commit: Commit,
    target_branch_name: &str,
    output: &OutputConfig,
) -> Result<(), PullMergeError> {
    switch::ensure_clean_status(output)
        .await
        .map_err(|_| PullMergeError::DirtyWorktree)?;
    let target_items = commit_tree_items(&target_commit)?;
    let current_index =
        Index::load(path::index()).map_err(|error| PullMergeError::IndexLoad(error.to_string()))?;
    let paths_to_write: Vec<PathBuf> = target_items.keys().cloned().collect();
    ensure_no_untracked_conflicts(&current_index, &paths_to_write)?;

    let db = get_db_conn_instance().await;

    let old_oid_opt = Head::current_commit_result_with_conn(&db)
        .await
        .map_err(|e| PullMergeError::HeadResolve(e.to_string()))?;
    let current_head_state = Head::current_result_with_conn(&db)
        .await
        .map_err(|e| PullMergeError::HeadResolve(e.to_string()))?;

    let action = ReflogAction::Merge {
        branch: target_branch_name.to_string(),
        policy: "fast-forward".to_string(),
    };
    let context = ReflogContext {
        // If there was no previous commit, this is an initial commit merge (e.g., on an empty branch).
        // Use the zero-hash in that case.
        old_oid: old_oid_opt.map_or(ObjectHash::zero_str(get_hash_kind()).to_string(), |id| {
            id.to_string()
        }),
        new_oid: target_commit.id.to_string(),
        action,
    };

    // Use `with_reflog`. A merge operation should log for the branch.
    if let Err(e) = with_reflog(
        context,
        move |txn: &sea_orm::DatabaseTransaction| {
            Box::pin(async move {
                match &current_head_state {
                    Head::Branch(branch_name) => {
                        Branch::update_branch_with_conn(
                            txn,
                            branch_name,
                            &target_commit.id.to_string(),
                            None,
                        )
                        .await?;
                    }
                    Head::Detached(_) => {
                        // Merging into a detached HEAD is unusual but possible. We just move HEAD.
                        Head::update_with_conn(txn, Head::Detached(target_commit.id), None).await;
                    }
                }
                Ok(())
            })
        },
        true,
    )
    .await
    {
        return Err(PullMergeError::HeadUpdate(e.to_string()));
    }

    // Only restore the working directory *after* the pointers have been updated.
    restore::execute_safe(
        RestoreArgs {
            worktree: true,
            staged: true,
            source: None, // `restore` without source defaults to HEAD, which is now correct.
            pathspec: vec![util::working_dir_string()],
        },
        &output.child_output_config(),
    )
    .await
    .map_err(|error| PullMergeError::Restore(error.to_string()))?;
    Ok(())
}

fn apply_squash_merge(target_commit: &Commit) -> Result<(), PullMergeError> {
    reset_index_and_workdir_to_tree(&target_commit.tree_id)
}

fn count_changed_files(
    current_commit: Option<&Commit>,
    target_commit: &Commit,
) -> Result<usize, PullMergeError> {
    let target_items = commit_tree_items(target_commit)?;
    let current_items = match current_commit {
        Some(commit) => commit_tree_items(commit)?,
        None => HashMap::new(),
    };

    let mut paths: HashSet<PathBuf> = current_items.keys().cloned().collect();
    paths.extend(target_items.keys().cloned());

    Ok(paths
        .into_iter()
        .filter(|path| current_items.get(path) != target_items.get(path))
        .count())
}

fn commit_tree_items(commit: &Commit) -> Result<HashMap<PathBuf, MergeTreeEntry>, PullMergeError> {
    let tree: Tree = load_object(&commit.tree_id).map_err(|error| PullMergeError::TreeLoad {
        tree_id: commit.tree_id.to_string(),
        detail: error.to_string(),
    })?;
    Ok(tree
        .get_plain_items_with_mode()
        .into_iter()
        .filter_map(|(path, hash, mode)| {
            if mode == TreeItemMode::Commit {
                None
            } else {
                Some((path, MergeTreeEntry { hash, mode }))
            }
        })
        .collect())
}

async fn current_head_name() -> Result<String, PullMergeError> {
    Head::current_result()
        .await
        .map_err(|error| PullMergeError::HeadResolve(error.to_string()))
        .map(|head| match head {
            Head::Branch(name) => name,
            Head::Detached(_) => "HEAD".to_string(),
        })
}

async fn update_head_with_reflog(
    head_name: &str,
    new_oid: ObjectHash,
    target_branch_name: &str,
    policy: &str,
) -> Result<(), PullMergeError> {
    let db = get_db_conn_instance().await;
    let old_oid_opt = Head::current_commit_result_with_conn(&db)
        .await
        .map_err(|error| PullMergeError::HeadResolve(error.to_string()))?;
    let action = ReflogAction::Merge {
        branch: target_branch_name.to_string(),
        policy: policy.to_string(),
    };
    let context = ReflogContext {
        old_oid: old_oid_opt.map_or(ObjectHash::zero_str(get_hash_kind()).to_string(), |id| {
            id.to_string()
        }),
        new_oid: new_oid.to_string(),
        action,
    };

    let head_name = head_name.to_string();
    with_reflog(
        context,
        move |txn: &sea_orm::DatabaseTransaction| {
            let head_name = head_name.clone();
            Box::pin(async move {
                if head_name == "HEAD" {
                    Head::update_with_conn(txn, Head::Detached(new_oid), None).await;
                } else {
                    Branch::update_branch_with_conn(txn, &head_name, &new_oid.to_string(), None)
                        .await?;
                }
                Ok(())
            })
        },
        true,
    )
    .await
    .map_err(|error| PullMergeError::HeadUpdate(error.to_string()))
}

fn object_hash_from_state(field: &str, value: &str) -> Result<ObjectHash, PullMergeError> {
    ObjectHash::from_str(value)
        .map_err(|error| PullMergeError::StateLoad(format!("invalid {field} '{value}': {error}")))
}

#[derive(Debug, Copy, Clone)]
enum MergeResolution {
    Use(MergeTreeEntry),
    Delete,
    Conflict(ConflictKind),
}

#[derive(Debug, Copy, Clone)]
enum ConflictKind {
    BothChanged {
        ours: ObjectHash,
        theirs: ObjectHash,
    },
    OursModifiedTheirsDeleted {
        ours: ObjectHash,
    },
    TheirsModifiedOursDeleted {
        theirs: ObjectHash,
    },
}

#[derive(Debug, Copy, Clone)]
enum RelativeState {
    Same(MergeTreeEntry),
    Modified(MergeTreeEntry),
    Deleted,
    Added(MergeTreeEntry),
    Missing,
}

fn classify_relative_to_base(
    base: Option<&MergeTreeEntry>,
    side: Option<&MergeTreeEntry>,
) -> RelativeState {
    match (base, side) {
        (Some(base), Some(side)) if base == side => RelativeState::Same(*side),
        (Some(_), Some(side)) => RelativeState::Modified(*side),
        (Some(_), None) => RelativeState::Deleted,
        (None, Some(side)) => RelativeState::Added(*side),
        (None, None) => RelativeState::Missing,
    }
}

fn resolve_three_way_with_options(
    base: Option<&MergeTreeEntry>,
    ours: Option<&MergeTreeEntry>,
    theirs: Option<&MergeTreeEntry>,
    options: &PullMergeOptions,
) -> Result<MergeResolution, PullMergeError> {
    let base_present = base.is_some();
    let ours_state = classify_relative_to_base(base, ours);
    let theirs_state = classify_relative_to_base(base, theirs);

    Ok(match (base_present, ours_state, theirs_state) {
        (false, RelativeState::Missing, RelativeState::Missing) => MergeResolution::Delete,
        (false, RelativeState::Added(ours), RelativeState::Missing) => MergeResolution::Use(ours),
        (false, RelativeState::Missing, RelativeState::Added(theirs)) => {
            MergeResolution::Use(theirs)
        }
        (false, RelativeState::Added(ours), RelativeState::Added(theirs)) => {
            if ours == theirs {
                MergeResolution::Use(theirs)
            } else {
                conflict_or_favor(
                    options.strategy_option,
                    ours_state,
                    theirs_state,
                    ConflictKind::BothChanged {
                        ours: ours.hash,
                        theirs: theirs.hash,
                    },
                )
            }
        }
        (true, RelativeState::Same(ours), RelativeState::Same(_)) => MergeResolution::Use(ours),
        (true, RelativeState::Same(_), RelativeState::Modified(theirs)) => {
            MergeResolution::Use(theirs)
        }
        (true, RelativeState::Modified(ours), RelativeState::Same(_)) => MergeResolution::Use(ours),
        (true, RelativeState::Modified(ours), RelativeState::Modified(theirs)) => {
            // Identical content (even when only the file mode differs) needs no
            // textual merge; both sides agree on the blob bytes.
            if ours.hash == theirs.hash {
                MergeResolution::Use(theirs)
            } else if let Some(base) = base
                && let Some(merged) = try_merge_blob_contents(base, ours, theirs)?
            {
                MergeResolution::Use(merged)
            } else {
                conflict_or_favor(
                    options.strategy_option,
                    ours_state,
                    theirs_state,
                    ConflictKind::BothChanged {
                        ours: ours.hash,
                        theirs: theirs.hash,
                    },
                )
            }
        }
        (true, RelativeState::Deleted, RelativeState::Same(_)) => MergeResolution::Delete,
        (true, RelativeState::Same(_), RelativeState::Deleted) => MergeResolution::Delete,
        (true, RelativeState::Deleted, RelativeState::Deleted) => MergeResolution::Delete,
        (true, RelativeState::Deleted, RelativeState::Modified(theirs)) => conflict_or_favor(
            options.strategy_option,
            ours_state,
            theirs_state,
            ConflictKind::TheirsModifiedOursDeleted {
                theirs: theirs.hash,
            },
        ),
        (true, RelativeState::Modified(ours), RelativeState::Deleted) => conflict_or_favor(
            options.strategy_option,
            ours_state,
            theirs_state,
            ConflictKind::OursModifiedTheirsDeleted { ours: ours.hash },
        ),
        _ => MergeResolution::Delete,
    })
}

fn conflict_or_favor(
    favor: Option<MergeFavor>,
    ours_state: RelativeState,
    theirs_state: RelativeState,
    kind: ConflictKind,
) -> MergeResolution {
    match favor {
        Some(MergeFavor::Ours) => state_to_resolution(ours_state),
        Some(MergeFavor::Theirs) => state_to_resolution(theirs_state),
        None => MergeResolution::Conflict(kind),
    }
}

fn state_to_resolution(state: RelativeState) -> MergeResolution {
    match state {
        RelativeState::Same(entry)
        | RelativeState::Modified(entry)
        | RelativeState::Added(entry) => MergeResolution::Use(entry),
        RelativeState::Deleted | RelativeState::Missing => MergeResolution::Delete,
    }
}

fn try_merge_blob_contents(
    base: &MergeTreeEntry,
    ours: MergeTreeEntry,
    theirs: MergeTreeEntry,
) -> Result<Option<MergeTreeEntry>, PullMergeError> {
    if base.mode != ours.mode
        || base.mode != theirs.mode
        || !matches!(base.mode, TreeItemMode::Blob | TreeItemMode::BlobExecutable)
    {
        return Ok(None);
    }

    let base_blob = load_merge_blob(base.hash)?;
    let ours_blob = load_merge_blob(ours.hash)?;
    let theirs_blob = load_merge_blob(theirs.hash)?;

    if is_binary_blob(&base_blob.data)
        || is_binary_blob(&ours_blob.data)
        || is_binary_blob(&theirs_blob.data)
    {
        return Ok(None);
    }

    let Ok(merged_bytes) = diffy::merge_bytes(&base_blob.data, &ours_blob.data, &theirs_blob.data)
    else {
        return Ok(None);
    };

    let merged_blob = Blob::from_content_bytes(merged_bytes);
    save_object(&merged_blob, &merged_blob.id).map_err(|error| {
        PullMergeError::TreeCreate(format!(
            "failed to save auto-merged blob {}: {error}",
            merged_blob.id
        ))
    })?;

    Ok(Some(MergeTreeEntry {
        hash: merged_blob.id,
        mode: ours.mode,
    }))
}

fn is_binary_blob(data: &[u8]) -> bool {
    data.contains(&0)
}

fn load_merge_blob(hash: ObjectHash) -> Result<Blob, PullMergeError> {
    load_object(&hash).map_err(|error| PullMergeError::ObjectLoad {
        object_id: hash.to_string(),
        detail: error.to_string(),
    })
}

#[cfg(test)]
fn merge_tree_items(
    base_items: &HashMap<PathBuf, MergeTreeEntry>,
    our_items: &HashMap<PathBuf, MergeTreeEntry>,
    their_items: &HashMap<PathBuf, MergeTreeEntry>,
) -> Result<ThreeWayMergeResult, PullMergeError> {
    merge_tree_items_with_options(
        base_items,
        our_items,
        their_items,
        &PullMergeOptions::default(),
    )
}

fn merge_tree_items_with_options(
    base_items: &HashMap<PathBuf, MergeTreeEntry>,
    our_items: &HashMap<PathBuf, MergeTreeEntry>,
    their_items: &HashMap<PathBuf, MergeTreeEntry>,
    options: &PullMergeOptions,
) -> Result<ThreeWayMergeResult, PullMergeError> {
    let mut all_paths: HashSet<PathBuf> = base_items.keys().cloned().collect();
    all_paths.extend(our_items.keys().cloned());
    all_paths.extend(their_items.keys().cloned());

    let mut merged_items = HashMap::new();
    let mut conflicts = Vec::new();
    for path in all_paths {
        match resolve_three_way_with_options(
            base_items.get(&path),
            our_items.get(&path),
            their_items.get(&path),
            options,
        )? {
            MergeResolution::Use(hash) => {
                merged_items.insert(path, hash);
            }
            MergeResolution::Delete => {}
            MergeResolution::Conflict(kind) => conflicts.push((path, kind)),
        }
    }

    Ok(ThreeWayMergeResult {
        merged_items,
        conflicts,
    })
}

fn count_item_map_changes(
    before: &HashMap<PathBuf, MergeTreeEntry>,
    after: &HashMap<PathBuf, MergeTreeEntry>,
) -> usize {
    let mut paths: HashSet<PathBuf> = before.keys().cloned().collect();
    paths.extend(after.keys().cloned());
    paths
        .into_iter()
        .filter(|path| before.get(path) != after.get(path))
        .count()
}

/// Build the diffstat shown by `--stat` from the pre-merge HEAD tree to the
/// merge result (commit tree for committed merges, staged index for
/// `--squash`/`--no-commit`). Returns `None` when nothing changed or the
/// trees cannot be loaded.
fn render_merge_diffstat(result: &PullMergeSummary) -> Option<String> {
    let old_items = match &result.old_commit {
        Some(id) => commit_items_for_stat(id)?,
        None => HashMap::new(),
    };
    let new_items = match &result.commit {
        Some(id) => commit_items_for_stat(id)?,
        None => {
            // Squash / no-commit leave the result staged rather than committed.
            let index = Index::load(path::index()).ok()?;
            index_tree_items(&index).ok()?
        }
    };
    merge_diffstat(&old_items, &new_items)
}

fn commit_items_for_stat(commit_id: &str) -> Option<HashMap<PathBuf, MergeTreeEntry>> {
    let oid = ObjectHash::from_str(commit_id).ok()?;
    let commit: Commit = load_object(&oid).ok()?;
    commit_tree_items(&commit).ok()
}

/// Format a Git-style diffstat (`path | N +++--` lines plus a summary line)
/// for the files that differ between two tree-item maps.
fn merge_diffstat(
    old: &HashMap<PathBuf, MergeTreeEntry>,
    new: &HashMap<PathBuf, MergeTreeEntry>,
) -> Option<String> {
    let mut paths: Vec<PathBuf> = old.keys().chain(new.keys()).cloned().collect();
    paths.sort();
    paths.dedup();

    struct StatRow {
        path: String,
        total: usize,
        insertions: usize,
        deletions: usize,
        binary: bool,
    }

    let mut rows = Vec::new();
    let mut total_insertions = 0usize;
    let mut total_deletions = 0usize;
    for path in paths {
        let (old_entry, new_entry) = (old.get(&path), new.get(&path));
        if old_entry == new_entry {
            continue;
        }
        let (insertions, deletions, binary) = file_line_delta(old_entry, new_entry);
        total_insertions += insertions;
        total_deletions += deletions;
        rows.push(StatRow {
            path: path.display().to_string(),
            total: insertions + deletions,
            insertions,
            deletions,
            binary,
        });
    }
    if rows.is_empty() {
        return None;
    }

    let name_width = rows.iter().map(|row| row.path.len()).max().unwrap_or(0);
    let count_width = rows
        .iter()
        .map(|row| row.total.to_string().len())
        .max()
        .unwrap_or(1);
    // Scale the +/- bar so the widest row fits in ~60 columns, matching Git's
    // capped histogram behaviour.
    let max_total = rows.iter().map(|row| row.total).max().unwrap_or(0);
    let scale = if max_total > 60 {
        60.0 / max_total as f64
    } else {
        1.0
    };

    let mut out = String::new();
    for row in &rows {
        if row.binary {
            out.push_str(&format!(" {:<name_width$} | Bin\n", row.path));
            continue;
        }
        let plus = ((row.insertions as f64) * scale).round() as usize;
        let minus = ((row.deletions as f64) * scale).round() as usize;
        let plus = if row.insertions > 0 { plus.max(1) } else { 0 };
        let minus = if row.deletions > 0 { minus.max(1) } else { 0 };
        out.push_str(&format!(
            " {:<name_width$} | {:>count_width$} {}{}\n",
            row.path,
            row.total,
            "+".repeat(plus),
            "-".repeat(minus),
        ));
    }
    let mut summary = format!(
        " {} file{} changed",
        rows.len(),
        if rows.len() == 1 { "" } else { "s" }
    );
    if total_insertions > 0 {
        summary.push_str(&format!(
            ", {} insertion{}(+)",
            total_insertions,
            if total_insertions == 1 { "" } else { "s" }
        ));
    }
    if total_deletions > 0 {
        summary.push_str(&format!(
            ", {} deletion{}(-)",
            total_deletions,
            if total_deletions == 1 { "" } else { "s" }
        ));
    }
    out.push_str(&summary);
    Some(out)
}

/// Count inserted/deleted lines between the blob at two tree entries. Returns
/// `(insertions, deletions, is_binary)`; binary differences report `(0, 0, true)`.
fn file_line_delta(
    old: Option<&MergeTreeEntry>,
    new: Option<&MergeTreeEntry>,
) -> (usize, usize, bool) {
    let load = |entry: Option<&MergeTreeEntry>| -> Option<Vec<u8>> {
        let entry = entry?;
        let blob: Blob = load_object(&entry.hash).ok()?;
        Some(blob.data)
    };
    let old_data = load(old);
    let new_data = load(new);
    let is_binary = old_data.as_deref().map(is_binary_blob).unwrap_or(false)
        || new_data.as_deref().map(is_binary_blob).unwrap_or(false);
    if is_binary {
        return (0, 0, true);
    }
    let old_lines = old_data.as_deref().map(bytes_to_lines).unwrap_or_default();
    let new_lines = new_data.as_deref().map(bytes_to_lines).unwrap_or_default();
    let ops = git_internal::diff::compute_diff(&old_lines, &new_lines);
    let insertions = ops
        .iter()
        .filter(|op| matches!(op, git_internal::diff::DiffOperation::Insert { .. }))
        .count();
    let deletions = ops
        .iter()
        .filter(|op| matches!(op, git_internal::diff::DiffOperation::Delete { .. }))
        .count();
    (insertions, deletions, false)
}

fn bytes_to_lines(data: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(data)
        .lines()
        .map(String::from)
        .collect()
}

fn add_blob_index_entry(
    index: &mut Index,
    path: &Path,
    item: MergeTreeEntry,
    stage: u8,
) -> Result<(), PullMergeError> {
    let blob: Blob = load_object(&item.hash).map_err(|error| {
        PullMergeError::IndexSave(format!(
            "failed to load blob {} for index entry '{}': {error}",
            item.hash,
            path.display()
        ))
    })?;
    let mut entry = IndexEntry::new_from_blob(
        path_to_index_key(path)?.to_string(),
        item.hash,
        blob.data.len() as u32,
    );
    entry.mode = tree_item_mode_to_index_mode(item.mode)?;
    entry.flags.stage = stage;
    index.add(entry);
    Ok(())
}

fn ensure_no_untracked_conflicts(
    current_index: &Index,
    paths: &[PathBuf],
) -> Result<(), PullMergeError> {
    let untracked_paths =
        worktree::untracked_workdir_paths(current_index).map_err(PullMergeError::IndexLoad)?;
    for untracked in &untracked_paths {
        for path in paths {
            if worktree::paths_conflict(untracked, path) {
                return Err(PullMergeError::UntrackedOverwrite {
                    path: untracked.display().to_string(),
                });
            }
        }
    }
    Ok(())
}

fn ensure_no_directory_file_conflicts<'a>(
    paths: impl IntoIterator<Item = &'a PathBuf>,
) -> Result<(), PullMergeError> {
    let mut sorted: Vec<&PathBuf> = paths.into_iter().collect();
    sorted.sort();
    for (index, path) in sorted.iter().enumerate() {
        for other in sorted.iter().skip(index + 1) {
            if other.starts_with(path) {
                return Err(PullMergeError::DirectoryFileConflict {
                    path: path.display().to_string(),
                });
            }
        }
    }
    Ok(())
}

fn write_workdir_file(workdir: &Path, relative: &Path, content: &[u8]) -> Result<(), String> {
    let file_path = workdir.join(relative);
    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    if fs::symlink_metadata(&file_path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fs::remove_file(&file_path)
            .map_err(|error| format!("failed to replace {}: {error}", file_path.display()))?;
    }
    fs::write(&file_path, content)
        .map_err(|error| format!("failed to write {}: {error}", file_path.display()))
}

fn write_workdir_blob(
    workdir: &Path,
    path: &Path,
    mode: TreeItemMode,
    content: &[u8],
) -> Result<(), String> {
    match mode {
        TreeItemMode::Blob => write_workdir_file(workdir, path, content),
        TreeItemMode::BlobExecutable => {
            write_workdir_file(workdir, path, content)?;
            set_executable_workdir_mode(&workdir.join(path))
        }
        TreeItemMode::Link => write_workdir_symlink(workdir, path, content),
        TreeItemMode::Tree => Err(format!(
            "tree entry cannot be written as a file: {}",
            path.display()
        )),
        TreeItemMode::Commit => Err(format!(
            "gitlink entries are not supported by merge: {}",
            path.display()
        )),
    }
}

#[cfg(unix)]
fn set_executable_workdir_mode(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).map_err(|error| {
        format!(
            "failed to set executable mode on {}: {error}",
            path.display()
        )
    })
}

#[cfg(not(unix))]
fn set_executable_workdir_mode(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn write_workdir_symlink(workdir: &Path, path: &Path, target: &[u8]) -> Result<(), String> {
    let file_path = workdir.join(path);
    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    if fs::symlink_metadata(&file_path).is_ok() {
        fs::remove_file(&file_path)
            .map_err(|error| format!("failed to replace {}: {error}", file_path.display()))?;
    }
    std::os::unix::fs::symlink(
        PathBuf::from(OsString::from_vec(target.to_vec())),
        &file_path,
    )
    .map_err(|error| format!("failed to create symlink {}: {error}", file_path.display()))
}

#[cfg(not(unix))]
fn write_workdir_symlink(workdir: &Path, path: &Path, target: &[u8]) -> Result<(), String> {
    write_workdir_file(workdir, path, target)
}

fn conflict_marker_eol() -> &'static str {
    if cfg!(windows) { "\r\n" } else { "\n" }
}

fn conflict_payload(content: &[u8]) -> Cow<'_, str> {
    if is_binary_blob(content) {
        return Cow::Owned(format!("[binary content, {} bytes]", content.len()));
    }
    match std::str::from_utf8(content) {
        Ok(text) => Cow::Borrowed(text),
        Err(_) => Cow::Owned(format!("[binary content, {} bytes]", content.len())),
    }
}

fn write_conflict_markers(
    workdir: &Path,
    path: &Path,
    marker_eol: &str,
    commit_abbrev: &str,
    kind: ConflictKind,
    base: Option<ObjectHash>,
    conflict_style: MergeConflictStyle,
) -> Result<(), String> {
    let content = match kind {
        ConflictKind::BothChanged { ours, theirs } => {
            let ours_blob: Blob = load_object(&ours).map_err(|error| error.to_string())?;
            let theirs_blob: Blob = load_object(&theirs).map_err(|error| error.to_string())?;
            let base_section = if conflict_style == MergeConflictStyle::Diff3 {
                base.map(load_conflict_base_payload)
                    .transpose()?
                    .map(|payload| format!("||||||| base{marker_eol}{payload}{marker_eol}"))
                    .unwrap_or_default()
            } else {
                String::new()
            };
            format!(
                "<<<<<<< HEAD{marker_eol}{}{marker_eol}{base_section}======={marker_eol}{}{marker_eol}>>>>>>> {}{marker_eol}",
                conflict_payload(&ours_blob.data),
                conflict_payload(&theirs_blob.data),
                commit_abbrev
            )
        }
        ConflictKind::OursModifiedTheirsDeleted { ours } => {
            let ours_blob: Blob = load_object(&ours).map_err(|error| error.to_string())?;
            format!(
                "<<<<<<< HEAD{marker_eol}{}{marker_eol}======={marker_eol}>>>>>>> {} (deleted){marker_eol}",
                conflict_payload(&ours_blob.data),
                commit_abbrev
            )
        }
        ConflictKind::TheirsModifiedOursDeleted { theirs } => {
            let theirs_blob: Blob = load_object(&theirs).map_err(|error| error.to_string())?;
            format!(
                "<<<<<<< HEAD (deleted){marker_eol}======={marker_eol}{}{marker_eol}>>>>>>> {}{marker_eol}",
                conflict_payload(&theirs_blob.data),
                commit_abbrev
            )
        }
    };
    write_workdir_file(workdir, path, content.as_bytes())
}

fn load_conflict_base_payload(base: ObjectHash) -> Result<Cow<'static, str>, String> {
    let base_blob: Blob = load_object(&base).map_err(|error| error.to_string())?;
    Ok(Cow::Owned(conflict_payload(&base_blob.data).into_owned()))
}

fn index_tree_items(index: &Index) -> Result<HashMap<PathBuf, MergeTreeEntry>, PullMergeError> {
    let mut items = HashMap::new();
    for path in index.tracked_files() {
        if let Some(entry) = index.get(path_to_index_key(&path)?, 0) {
            items.insert(
                path,
                MergeTreeEntry {
                    hash: entry.hash,
                    mode: index_mode_to_tree_item_mode(entry.mode)?,
                },
            );
        }
    }
    Ok(items)
}

fn create_tree_from_items_map(
    items: &HashMap<PathBuf, MergeTreeEntry>,
) -> Result<ObjectHash, String> {
    let mut entries_map = tree_entries_map_from_items(items)?;
    build_tree_recursively(Path::new(""), &mut entries_map)
}

fn tree_entries_map_from_items(
    items: &HashMap<PathBuf, MergeTreeEntry>,
) -> Result<HashMap<PathBuf, Vec<TreeItem>>, String> {
    let mut entries_map: HashMap<PathBuf, Vec<TreeItem>> = HashMap::new();
    for (path, entry) in items {
        let parent_dir = path.parent().unwrap_or_else(|| Path::new("")).to_path_buf();
        ensure_tree_parent_dirs(&mut entries_map, &parent_dir);
        entries_map.entry(parent_dir).or_default().push(TreeItem {
            mode: entry.mode,
            name: tree_item_name(path)?,
            id: entry.hash,
        });
    }
    Ok(entries_map)
}

fn ensure_tree_parent_dirs(entries_map: &mut HashMap<PathBuf, Vec<TreeItem>>, dir: &Path) {
    let mut current = Some(dir);
    while let Some(path) = current {
        if path.as_os_str().is_empty() {
            break;
        }
        entries_map.entry(path.to_path_buf()).or_default();
        current = path.parent();
    }
}

fn build_tree_recursively(
    current_path: &Path,
    entries_map: &mut HashMap<PathBuf, Vec<TreeItem>>,
) -> Result<ObjectHash, String> {
    let mut current_items = entries_map.remove(current_path).unwrap_or_default();
    let subdirs: Vec<_> = entries_map
        .keys()
        .filter(|path| path.parent() == Some(current_path))
        .cloned()
        .collect();

    for subdir in subdirs {
        let subtree_id = build_tree_recursively(&subdir, entries_map)?;
        current_items.push(TreeItem {
            mode: TreeItemMode::Tree,
            name: tree_item_name(&subdir)?,
            id: subtree_id,
        });
    }

    crate::utils::tree::sort_tree_items_for_git(&mut current_items);
    let tree = Tree::from_tree_items(current_items).map_err(|error| error.to_string())?;
    save_object(&tree, &tree.id).map_err(|error| error.to_string())?;
    Ok(tree.id)
}

fn reset_index_and_workdir_to_tree(tree_id: &ObjectHash) -> Result<(), PullMergeError> {
    let tree: Tree = load_object(tree_id).map_err(|error| PullMergeError::TreeLoad {
        tree_id: tree_id.to_string(),
        detail: error.to_string(),
    })?;
    let current_index =
        Index::load(path::index()).map_err(|error| PullMergeError::IndexLoad(error.to_string()))?;
    let mut new_index = Index::new();
    reset::rebuild_index_from_tree(&tree, &mut new_index, "")
        .map_err(PullMergeError::TreeCreate)?;
    reset_workdir_tracked_only(&current_index, &new_index)?;
    new_index
        .save(path::index())
        .map_err(|error| PullMergeError::IndexSave(error.to_string()))
}

fn reset_workdir_tracked_only(
    current_index: &Index,
    new_index: &Index,
) -> Result<(), PullMergeError> {
    let workdir = util::working_dir();
    let untracked_paths =
        worktree::untracked_workdir_paths(current_index).map_err(PullMergeError::IndexLoad)?;
    if let Some(conflict) = worktree::untracked_overwrite_path(&untracked_paths, new_index) {
        return Err(PullMergeError::UntrackedOverwrite {
            path: conflict.display().to_string(),
        });
    }

    let new_tracked_paths: HashSet<_> = new_index.tracked_files().into_iter().collect();
    for path_buf in current_index.tracked_files() {
        if !new_tracked_paths.contains(&path_buf) {
            let full_path = workdir.join(path_buf);
            if full_path.exists() {
                fs::remove_file(&full_path).map_err(|error| {
                    PullMergeError::WorkdirReset(format!("failed to remove file: {error}"))
                })?;
            }
        }
    }

    for path_buf in new_index.tracked_files() {
        if let Some(entry) = new_index.get(path_to_index_key(&path_buf)?, 0) {
            let blob: Blob = load_object(&entry.hash).map_err(|error| {
                PullMergeError::WorkdirReset(format!(
                    "failed to load blob {} for '{}': {error}",
                    entry.hash,
                    path_buf.display()
                ))
            })?;
            write_workdir_blob(
                &workdir,
                &path_buf,
                index_mode_to_tree_item_mode(entry.mode)?,
                &blob.data,
            )
            .map_err(PullMergeError::WorkdirReset)?;
        }
    }
    Ok(())
}

fn has_unmerged_entries(index: &Index) -> bool {
    !unresolved_conflicted_paths(index, &[]).is_empty()
}

pub(crate) fn unresolved_conflicted_paths(
    index: &Index,
    conflicted_paths: &[String],
) -> Vec<String> {
    let resolved: HashSet<String> = index
        .tracked_entries(0)
        .into_iter()
        .map(|entry| entry.name.clone())
        .collect();
    let staged_conflicts = staged_conflict_paths(index);
    let mut paths: Vec<String> = if conflicted_paths.is_empty() {
        staged_conflicts.into_iter().collect()
    } else {
        conflicted_paths
            .iter()
            .filter(|path| staged_conflicts.contains(path.as_str()))
            .cloned()
            .collect()
    };
    paths.retain(|path| !resolved.contains(path.as_str()));
    paths.sort();
    paths
}

fn staged_conflict_paths(index: &Index) -> HashSet<String> {
    (1..=3)
        .flat_map(|stage| index.tracked_entries(stage))
        .map(|entry| entry.name.clone())
        .collect()
}

fn tree_item_name(path: &Path) -> Result<String, String> {
    let name = path
        .file_name()
        .ok_or_else(|| format!("path has no file name: {}", path.display()))?;
    name.to_str()
        .map(str::to_string)
        .ok_or_else(|| format!("path is not valid UTF-8: {}", path.display()))
}

fn path_to_index_key(path: &Path) -> Result<&str, PullMergeError> {
    path.to_str().ok_or_else(|| {
        PullMergeError::IndexSave(format!("path is not valid UTF-8: {}", path.display()))
    })
}

fn tree_item_mode_to_index_mode(mode: TreeItemMode) -> Result<u32, PullMergeError> {
    match mode {
        TreeItemMode::Blob => Ok(0o100644),
        TreeItemMode::BlobExecutable => Ok(0o100755),
        TreeItemMode::Link => Ok(0o120000),
        TreeItemMode::Tree => Err(PullMergeError::IndexSave(
            "tree entry cannot be represented as a file index entry".to_string(),
        )),
        TreeItemMode::Commit => Err(PullMergeError::IndexSave(
            "gitlink entries are not supported by merge".to_string(),
        )),
    }
}

fn index_mode_to_tree_item_mode(mode: u32) -> Result<TreeItemMode, PullMergeError> {
    match mode {
        0o100644 => Ok(TreeItemMode::Blob),
        0o100755 => Ok(TreeItemMode::BlobExecutable),
        0o120000 => Ok(TreeItemMode::Link),
        other => Err(PullMergeError::TreeCreate(format!(
            "unsupported index mode {other:o} while creating merge tree"
        ))),
    }
}

fn short_object_id(object_id: &ObjectHash) -> String {
    let object_id = object_id.to_string();
    object_id.chars().take(7).collect()
}

async fn merge_commit_message(
    upstream: &str,
    head_name: &str,
    commits: Option<(&Commit, &Commit)>,
    options: &PullMergeOptions,
) -> Result<String, PullMergeError> {
    let into_label = options.into_name.as_deref().unwrap_or(head_name);
    let mut base_message = options
        .message
        .clone()
        .unwrap_or_else(|| format!("Merge {upstream} into {into_label}"));
    if let Some(limit) = options.log
        && limit > 0
        && let Some((current, target)) = commits
    {
        append_merge_shortlog(&mut base_message, current, target, limit).await?;
    }
    if !options.signoff {
        return Ok(base_message);
    }
    let (name, email) = resolve_signoff_identity().await?;
    Ok(format!("{base_message}\n\nSigned-off-by: {name} <{email}>"))
}

async fn append_merge_shortlog(
    message: &mut String,
    current: &Commit,
    target: &Commit,
    limit: usize,
) -> Result<(), PullMergeError> {
    let current_ids: HashSet<ObjectHash> = log::get_reachable_commits(current.id.to_string(), None)
        .await
        .map_err(|error| PullMergeError::History(error.to_string()))?
        .into_iter()
        .map(|commit| commit.id)
        .collect();
    let subjects: Vec<String> = log::get_reachable_commits(target.id.to_string(), None)
        .await
        .map_err(|error| PullMergeError::History(error.to_string()))?
        .into_iter()
        .filter(|commit| !current_ids.contains(&commit.id))
        .filter_map(|commit| first_non_empty_line(&commit.message))
        .take(limit)
        .collect();
    if subjects.is_empty() {
        return Ok(());
    }
    message.push_str("\n\n");
    for subject in subjects {
        message.push_str("* ");
        message.push_str(&subject);
        message.push('\n');
    }
    Ok(())
}

fn first_non_empty_line(message: &str) -> Option<String> {
    message
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .rev()
        .find(|line| {
            !line.starts_with("gpgsig")
                && !line.starts_with("-----")
                && !line.starts_with("Version:")
        })
        .map(str::to_string)
}

async fn resolve_signoff_identity() -> Result<(String, String), PullMergeError> {
    let config_name = read_cascaded_config_value(LocalIdentityTarget::CurrentRepo, "user.name")
        .await
        .ok()
        .flatten();
    let config_email = read_cascaded_config_value(LocalIdentityTarget::CurrentRepo, "user.email")
        .await
        .ok()
        .flatten();
    let env_name = env_first_non_empty(&[
        "GIT_COMMITTER_NAME",
        "GIT_AUTHOR_NAME",
        "LIBRA_COMMITTER_NAME",
    ]);
    let env_email = env_first_non_empty(&[
        "GIT_COMMITTER_EMAIL",
        "GIT_AUTHOR_EMAIL",
        "EMAIL",
        "LIBRA_COMMITTER_EMAIL",
    ]);
    match (config_name.or(env_name), config_email.or(env_email)) {
        (Some(name), Some(email)) => Ok((name, email)),
        _ => Err(PullMergeError::SignoffIdentity),
    }
}

fn zero_object_hash() -> ObjectHash {
    ObjectHash::from_str(&ObjectHash::zero_str(get_hash_kind()))
        .unwrap_or_else(|_| ObjectHash::new(&vec![0; get_hash_kind().size()]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn merge_entry(byte: u8, mode: TreeItemMode) -> MergeTreeEntry {
        MergeTreeEntry {
            hash: ObjectHash::new(&[byte; 20]),
            mode,
        }
    }

    /// Pin the `Display` format for every variant of [`PullMergeError`]
    /// (also exposed as `MergeError`). These strings are used as the
    /// CliError message via `From<PullMergeError> for CliError` and
    /// surface in both human and `--json` envelopes for `merge` and
    /// the merge phase of `pull`.
    #[test]
    fn pull_merge_error_display_pins_each_variant() {
        assert_eq!(
            PullMergeError::InvalidTarget("a/b".to_string()).to_string(),
            "a/b - not something we can merge",
        );
        assert_eq!(
            PullMergeError::TargetLoad {
                commit_id: "deadbeef".to_string(),
                detail: "object not found".to_string(),
            }
            .to_string(),
            "failed to load merge target 'deadbeef': object not found",
        );
        assert_eq!(
            PullMergeError::CurrentLoad {
                commit_id: "feedface".to_string(),
                detail: "io error".to_string(),
            }
            .to_string(),
            "failed to load current commit 'feedface': io error",
        );
        assert_eq!(
            PullMergeError::History("walk failed".to_string()).to_string(),
            "failed to inspect merge history: walk failed",
        );
        assert_eq!(
            PullMergeError::UnrelatedHistories.to_string(),
            "refusing to merge unrelated histories",
        );
        assert_eq!(
            PullMergeError::NonFastForward {
                current: "1111111".to_string(),
                target: "2222222".to_string(),
            }
            .to_string(),
            "non-fast-forward merge refused (current 1111111, target 2222222)",
        );
        assert_eq!(
            PullMergeError::TreeLoad {
                tree_id: "abc123".to_string(),
                detail: "decode failed".to_string(),
            }
            .to_string(),
            "failed to load tree 'abc123': decode failed",
        );
        assert_eq!(
            PullMergeError::ObjectLoad {
                object_id: "def456".to_string(),
                detail: "blob missing".to_string(),
            }
            .to_string(),
            "failed to load object 'def456': blob missing",
        );
        assert_eq!(
            PullMergeError::HeadResolve("db locked".to_string()).to_string(),
            "failed to resolve HEAD state: db locked",
        );
        assert_eq!(
            PullMergeError::HeadUpdate("write failed".to_string()).to_string(),
            "failed to update HEAD during merge: write failed",
        );
        assert_eq!(
            PullMergeError::Restore("checkout failed".to_string()).to_string(),
            "failed to restore working tree after merge: checkout failed",
        );
        assert_eq!(
            PullMergeError::InvalidDiffAlgorithm {
                value: "bogus".to_string(),
            }
            .to_string(),
            "unknown diff algorithm 'bogus' (expected myers, histogram, patience, or minimal)",
        );
        assert_eq!(
            PullMergeError::InvalidCleanupMode {
                value: "bogus".to_string(),
            }
            .to_string(),
            "unknown cleanup mode 'bogus' (expected strip, whitespace, verbatim, scissors, or default)",
        );
        assert_eq!(
            PullMergeError::Autostash("stash push failed".to_string()).to_string(),
            "autostash failed: stash push failed",
        );
    }

    #[test]
    fn merge_tree_items_preserves_mode_from_changed_side() {
        let path = PathBuf::from("script.sh");
        let base = merge_entry(1, TreeItemMode::Blob);
        let theirs = merge_entry(2, TreeItemMode::BlobExecutable);
        let mut base_items = HashMap::new();
        base_items.insert(path.clone(), base);
        let mut our_items = HashMap::new();
        our_items.insert(path.clone(), base);
        let mut their_items = HashMap::new();
        their_items.insert(path.clone(), theirs);

        let result =
            merge_tree_items(&base_items, &our_items, &their_items).expect("merge tree items");

        assert!(result.conflicts.is_empty());
        assert_eq!(result.merged_items.get(&path), Some(&theirs));
    }

    #[test]
    fn tree_entries_map_from_items_materializes_nested_parent_dirs() {
        let mut items = HashMap::new();
        items.insert(
            PathBuf::from("dir/sub/file.txt"),
            merge_entry(1, TreeItemMode::Blob),
        );

        let entries = tree_entries_map_from_items(&items).expect("build tree entries");

        assert!(
            entries.contains_key(Path::new("dir")),
            "parent directory should be present so recursive tree building can attach it to root"
        );
        assert!(
            entries.contains_key(Path::new("dir/sub")),
            "leaf directory should contain the nested file entry"
        );
        assert_eq!(entries[Path::new("dir")].len(), 0);
        assert_eq!(entries[Path::new("dir/sub")].len(), 1);
    }
}
