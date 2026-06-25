//! Merge command orchestration that resolves base/target commits, performs recursive merge, stages results, and updates refs or surfaces conflicts.

use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

use clap::Parser;
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
    save_object, status, switch,
};
use crate::{
    common_utils::format_commit_msg,
    info_println,
    internal::{
        branch::{Branch, BranchStoreError},
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
    libra merge feature-x --no-edit  Accept the default merge message (no editor)
    libra merge --verify-signatures feature-x  Require a valid PGP signature on the merged tip
    libra merge --continue         Finish an in-progress merge after resolving conflicts
    libra merge --abort            Restore the pre-merge HEAD, index, and worktree
    libra merge --json feature-x   Structured JSON output for agents

NOTES:
    Divergent single-head merges create a merge commit when paths do not
    conflict. Conflicts write markers and can be finished with --continue
    or restored with --abort.";

#[derive(Parser, Debug)]
#[command(after_help = MERGE_EXAMPLES)]
pub struct MergeArgs {
    /// The branch to merge into the current branch, could be remote branch
    pub branch: Option<String>,

    /// Continue an in-progress merge after resolving conflicts
    #[arg(long = "continue", conflicts_with = "abort")]
    pub continue_merge: bool,

    /// Abort an in-progress merge and restore the pre-merge state
    #[arg(long, conflicts_with = "continue_merge")]
    pub abort: bool,

    /// Refuse to merge unless the current branch can fast-forward to the target.
    #[arg(long = "ff-only", conflicts_with_all = ["no_ff", "continue_merge", "abort"])]
    pub ff_only: bool,

    /// Always create a merge commit, even when a fast-forward would be possible.
    #[arg(long = "no-ff", conflicts_with_all = ["ff_only", "continue_merge", "abort"])]
    pub no_ff: bool,

    /// Use the given message for the merge commit instead of the default.
    #[arg(short = 'm', long = "message", value_name = "MSG", conflicts_with_all = ["continue_merge", "abort"])]
    pub message: Option<String>,

    /// Merge changes but stage the result without committing or moving HEAD
    /// (no merge info recorded); finalize with a normal `commit`.
    #[arg(long, conflicts_with_all = ["ff_only", "continue_merge", "abort"])]
    pub squash: bool,

    /// Perform the merge and stage the result but stop before committing,
    /// recording merge state; finalize with `libra merge --continue`.
    #[arg(long = "no-commit", conflicts_with_all = ["squash", "ff_only", "continue_merge", "abort"])]
    pub no_commit: bool,

    /// Accept the auto-generated merge message without launching an editor.
    /// Libra never opens an editor for merge (it uses `-m` or the default
    /// message), so this is accepted for Git parity and is a no-op.
    #[arg(long = "no-edit")]
    pub no_edit: bool,

    /// Show a diffstat of the merge result at the end (what the merge changed,
    /// pre-merge HEAD vs the new commit). Git shows this by default; Libra
    /// defaults to no diffstat, so `--stat` opts in. Toggle pair with
    /// `--no-stat`/`-n`; the last one wins.
    #[arg(long = "stat", overrides_with = "no_stat")]
    pub stat: bool,

    /// Do not show a diffstat at the end of the merge (Libra's default).
    /// Accepted for Git parity. Toggle pair with `--stat`; the last one wins.
    #[arg(short = 'n', long = "no-stat", overrides_with = "stat")]
    pub no_stat: bool,

    /// Do not show a progress meter. Accepted for Git parity and is a no-op:
    /// Libra's merge never renders a progress meter, so there is nothing to
    /// suppress.
    #[arg(long = "no-progress")]
    pub no_progress: bool,

    /// Verify that the tip commit of the branch being merged carries a valid PGP
    /// signature, aborting the merge if it is unsigned or the signature is bad.
    /// Like `tag -v`, only signatures made by this repository's vault PGP key can
    /// be validated (Libra has no external GPG keyring), so a commit signed
    /// elsewhere — or with an SSH signature — is treated as not verifiable.
    #[arg(long = "verify-signatures", overrides_with = "no_verify_signatures", conflicts_with_all = ["continue_merge", "abort"])]
    pub verify_signatures: bool,

    /// Do not verify that the merged commits carry a valid GPG signature (the
    /// default). The inverse of `--verify-signatures`; last one wins.
    #[arg(long = "no-verify-signatures", overrides_with = "verify_signatures")]
    pub no_verify_signatures: bool,

    /// Do not update the rerere (reuse recorded resolution) index after the
    /// merge. Accepted for Git parity and is a no-op: Libra has no rerere, so
    /// there is nothing to update. (Git's opposite `--rerere-autoupdate` is not
    /// exposed.)
    #[arg(long = "no-rerere-autoupdate")]
    pub no_rerere_autoupdate: bool,
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
    /// Force a real merge commit even when the integration could fast-forward
    /// (`libra pull --no-ff`). When set, the fast-forward short-circuit is
    /// skipped and a two-parent merge commit is recorded instead.
    pub no_ff: bool,
    /// Override the merge-commit message (`libra merge -m <msg>`). `None` uses
    /// the default `Merge <upstream> into <head>` message.
    pub message: Option<String>,
    /// `libra merge --squash`: produce the merged index/worktree but do NOT
    /// create a commit or move HEAD (and never fast-forward), leaving the result
    /// staged for a subsequent normal `commit`.
    pub squash: bool,
    /// `libra merge --no-commit`: perform the merge and stage the result (never
    /// fast-forward) but stop before committing, recording a MergeState so
    /// `libra merge --continue` can finalize the two-parent commit.
    pub no_commit: bool,
    /// `libra merge --verify-signatures`: verify the resolved tip commit's PGP
    /// signature before mutating any state and abort if it is unsigned or invalid.
    /// Checked on the SAME loaded commit that is merged (no re-resolution), so the
    /// verified object is exactly the merged object. Always `false` for `pull`.
    pub verify_signatures: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct MergeState {
    pub head_name: String,
    pub orig_head: String,
    pub target: String,
    pub target_ref: String,
    pub base: String,
    pub conflicted_paths: Vec<String>,
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
    #[error("commit {commit} does not have a GPG signature")]
    UnsignedMergeCommit { commit: String },
    #[error("commit {commit} has a bad GPG signature")]
    BadMergeSignature { commit: String },
    #[error("failed to verify the signature of the merged commit: {0}")]
    SignatureCheck(String),
}

pub(crate) type MergeError = PullMergeError;

impl From<PullMergeError> for CliError {
    fn from(error: PullMergeError) -> Self {
        match &error {
            PullMergeError::MissingAction | PullMergeError::ConflictingAction => {
                CliError::command_usage(error.to_string())
                    .with_stable_code(StableErrorCode::CliInvalidArguments)
            }
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
            PullMergeError::UnsignedMergeCommit { .. }
            | PullMergeError::BadMergeSignature { .. } => CliError::failure(error.to_string())
                .with_stable_code(StableErrorCode::RepoStateInvalid)
                .with_hint("the tip commit could not be verified against the vault PGP key")
                .with_hint("re-run without --verify-signatures to merge without verification"),
            PullMergeError::SignatureCheck(..) => CliError::failure(error.to_string())
                .with_stable_code(StableErrorCode::RepoStateInvalid)
                .with_hint(
                    "ensure the repository vault is initialized and unsealed for signature verification",
                )
                .with_hint("re-run without --verify-signatures to merge without verification"),
            PullMergeError::NonFastForward { .. } => CliError::failure(error.to_string())
                .with_stable_code(StableErrorCode::ConflictOperationBlocked)
                .with_hint("run 'libra pull' without --ff-only to allow a merge commit")
                .with_hint("or run 'libra pull --rebase' to replay local commits"),
            PullMergeError::Conflicts { .. }
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
    // Refuse to start a merge while a cherry-pick sequence is in progress.
    crate::command::cherry_pick::ensure_no_cherry_pick_in_progress().await?;
    // `args` is moved into `run_merge`; capture the diffstat opt-in first.
    let show_stat = args.stat;
    let result = run_merge(args, output).await.map_err(merge_error_to_cli)?;
    render_merge_output(&result, output)?;
    maybe_print_merge_stat(show_stat, &result, output).await;
    Ok(())
}

/// `--stat`: print a Git-style diffstat of what the merge changed (pre-merge
/// HEAD vs the new commit). Human output only — `--json` already exposes
/// `files_changed`. Skipped when there is no completed new commit (up-to-date,
/// aborted, conflicted, or squash/no-commit that did not move HEAD). A failure
/// to compute the stat is non-fatal: the merge already succeeded.
async fn maybe_print_merge_stat(show_stat: bool, result: &MergeOutput, output: &OutputConfig) {
    if !show_stat || output.is_json() || output.quiet || !result.conflicted_paths.is_empty() {
        return;
    }
    let (Some(old), Some(new)) = (result.old_commit.as_deref(), result.commit.as_deref()) else {
        return;
    };
    let (Ok(old_hash), Ok(new_hash)) = (ObjectHash::from_str(old), ObjectHash::from_str(new))
    else {
        return;
    };
    match crate::command::diff::diff_stat_between_commits(&old_hash, &new_hash).await {
        Ok(stat) if !stat.trim().is_empty() => print!("{stat}"),
        Ok(_) => {}
        Err(err) => tracing::warn!(error = %err, "failed to compute merge diffstat"),
    }
}

async fn run_merge(args: MergeArgs, output: &OutputConfig) -> Result<MergeOutput, MergeError> {
    match (args.branch.as_deref(), args.continue_merge, args.abort) {
        (Some(branch), false, false) => {
            let options = PullMergeOptions {
                ff_only: args.ff_only,
                no_ff: args.no_ff,
                message: args.message.clone(),
                squash: args.squash,
                no_commit: args.no_commit,
                // `--verify-signatures` is enforced inside the merge on the loaded
                // tip commit, so the verified object is exactly the merged object.
                verify_signatures: args.verify_signatures,
            };
            run_merge_for_pull_with_options(branch, branch, output, options).await
        }
        (None, true, false) => run_merge_continue(output).await,
        (None, false, true) => run_merge_abort(output).await,
        (None, false, false) => Err(MergeError::MissingAction),
        _ => Err(MergeError::ConflictingAction),
    }
}

/// Verify `commit`'s PGP signature for a `--verify-signatures` merge, returning
/// a typed abort error when it is unsigned or the signature does not validate
/// against the vault PGP key. Run on the already-loaded tip commit (before any
/// state mutation) so the verified object is exactly the one being merged.
async fn verify_merge_commit_signature(commit: &Commit) -> Result<(), MergeError> {
    use crate::command::commit::{CommitSignatureStatus, verify_commit_signature};

    match verify_commit_signature(commit).await {
        Ok(CommitSignatureStatus::Good) => Ok(()),
        Ok(CommitSignatureStatus::Unsigned) => Err(MergeError::UnsignedMergeCommit {
            commit: commit.id.to_string(),
        }),
        Ok(CommitSignatureStatus::Bad) => Err(MergeError::BadMergeSignature {
            commit: commit.id.to_string(),
        }),
        Err(error) => Err(MergeError::SignatureCheck(error.to_string())),
    }
}

fn render_merge_output(result: &MergeOutput, output: &OutputConfig) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("merge", result, output);
    }
    if output.quiet {
        return Ok(());
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
            "squash" => info_println!(output, "Squash commit -- not updating HEAD"),
            "no-commit" => info_println!(
                output,
                "Automatic merge went well; stopped before committing as requested\n\
                 finalize with 'libra merge --continue'"
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
    options: PullMergeOptions,
) -> Result<PullMergeSummary, PullMergeError> {
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

    // `--verify-signatures`: validate the resolved tip's PGP signature on the
    // SAME loaded commit, before any state mutation — so the verified object is
    // exactly the merged object (no time-of-check/time-of-use re-resolution gap).
    if options.verify_signatures {
        verify_merge_commit_signature(&target_commit).await?;
    }

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

    let lca = lca.ok_or(PullMergeError::UnrelatedHistories)?;

    if lca.id == target_commit.id {
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

    if lca.id == current_commit.id && !options.no_ff && !options.squash && !options.no_commit {
        let files_changed = count_changed_files(Some(&current_commit), &target_commit)?;
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

    // `--no-ff` cannot be combined with `--ff-only` (clap rejects the pair on
    // the pull surface), so reaching `ff_only` here means a genuine
    // non-fast-forward history.
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
        ThreeWayMergeOptions {
            message_override: options.message.clone(),
            squash: options.squash,
            no_commit: options.no_commit,
            output,
        },
    )
    .await
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

struct ThreeWayMergeOptions<'a> {
    message_override: Option<String>,
    squash: bool,
    no_commit: bool,
    output: &'a OutputConfig,
}

async fn perform_three_way_merge(
    current_commit: Commit,
    target_commit: Commit,
    base_commit: Commit,
    upstream: &str,
    options: ThreeWayMergeOptions<'_>,
) -> Result<PullMergeSummary, PullMergeError> {
    switch::ensure_clean_status(options.output)
        .await
        .map_err(|_| PullMergeError::DirtyWorktree)?;

    let head_name = current_head_name().await?;
    let base_items = commit_tree_items(&base_commit)?;
    let our_items = commit_tree_items(&current_commit)?;
    let their_items = commit_tree_items(&target_commit)?;
    let merge_result = merge_tree_items(&base_items, &our_items, &their_items)?;
    let files_changed = count_item_map_changes(&our_items, &merge_result.merged_items);

    if !merge_result.conflicts.is_empty() {
        write_conflicted_merge_state(MergeConflictInput {
            head_name,
            upstream: upstream.to_string(),
            base: base_commit.id,
            ours: current_commit.id,
            theirs: target_commit.id,
            merged_items: merge_result.merged_items,
            conflicts: merge_result.conflicts,
            base_items,
            our_items,
            their_items,
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
        // `--squash`: update the index/worktree to the merged tree but do not
        // create a commit or move HEAD, leaving the result staged for a normal
        // `commit`. No MERGE_HEAD/merge info is recorded (matches Git).
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

    if options.no_commit {
        // `--no-commit`: stage the (conflict-free) merged tree but stop before
        // committing, recording a MergeState with no conflicted paths so
        // `libra merge --continue` finalizes the two-parent commit. (Unlike Git,
        // a plain `commit` would record only one parent, so the result must be
        // finalized via `merge --continue`.)
        reset_index_and_workdir_to_tree(&tree_id)?;
        MergeState {
            head_name: head_name.clone(),
            orig_head: current_commit.id.to_string(),
            target: target_commit.id.to_string(),
            target_ref: upstream.to_string(),
            base: base_commit.id.to_string(),
            conflicted_paths: Vec::new(),
        }
        .save()?;
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

    let message = options
        .message_override
        .unwrap_or_else(|| format!("Merge {upstream} into {head_name}"));
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
        write_workdir_file(&workdir, path, &blob.data).map_err(PullMergeError::WorkdirReset)?;
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
        write_conflict_markers(&workdir, path, marker_eol, &theirs_abbrev, *kind)
            .map_err(PullMergeError::WorkdirReset)?;
    }

    Ok(())
}

async fn run_merge_continue(_output: &OutputConfig) -> Result<MergeOutput, MergeError> {
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
    let message = format!("Merge {} into {}", state.target_ref, state.head_name);
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

    // Commit `eq` is based on tree_id, so we shouldn't use it here

    for commit in lhs_reachable.iter() {
        if commit.id == rhs.id {
            return Ok(Some(commit.to_owned()));
        }
    }

    for commit in rhs_reachable.iter() {
        if commit.id == lhs.id {
            return Ok(Some(commit.to_owned()));
        }
    }

    for lhs_parent in lhs_reachable.iter() {
        for rhs_parent in rhs_reachable.iter() {
            if lhs_parent.id == rhs_parent.id {
                return Ok(Some(lhs_parent.to_owned()));
            }
        }
    }
    Ok(None)
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
            overlay: false,
            no_overlay: false,
            ours: false,
            theirs: false,
            ignore_unmerged: false,
            merge: false,
            conflict: None,
            worktree: true,
            staged: true,
            source: None, // `restore` without source defaults to HEAD, which is now correct.
            pathspec: vec![util::working_dir_string()],
            pathspec_from_file: None,
            pathspec_file_nul: false,
            no_progress: false,
        },
        &output.child_output_config(),
    )
    .await
    .map_err(|error| PullMergeError::Restore(error.to_string()))?;
    Ok(())
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

fn resolve_three_way(
    base: Option<&MergeTreeEntry>,
    ours: Option<&MergeTreeEntry>,
    theirs: Option<&MergeTreeEntry>,
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
                MergeResolution::Conflict(ConflictKind::BothChanged {
                    ours: ours.hash,
                    theirs: theirs.hash,
                })
            }
        }
        (true, RelativeState::Same(ours), RelativeState::Same(_)) => MergeResolution::Use(ours),
        (true, RelativeState::Same(_), RelativeState::Modified(theirs)) => {
            MergeResolution::Use(theirs)
        }
        (true, RelativeState::Modified(ours), RelativeState::Same(_)) => MergeResolution::Use(ours),
        (true, RelativeState::Modified(ours), RelativeState::Modified(theirs)) => {
            if ours == theirs {
                MergeResolution::Use(theirs)
            } else if let Some(base) = base
                && let Some(merged) = try_merge_blob_contents(base, ours, theirs)?
            {
                MergeResolution::Use(merged)
            } else {
                MergeResolution::Conflict(ConflictKind::BothChanged {
                    ours: ours.hash,
                    theirs: theirs.hash,
                })
            }
        }
        (true, RelativeState::Deleted, RelativeState::Same(_)) => MergeResolution::Delete,
        (true, RelativeState::Same(_), RelativeState::Deleted) => MergeResolution::Delete,
        (true, RelativeState::Deleted, RelativeState::Deleted) => MergeResolution::Delete,
        (true, RelativeState::Deleted, RelativeState::Modified(theirs)) => {
            MergeResolution::Conflict(ConflictKind::TheirsModifiedOursDeleted {
                theirs: theirs.hash,
            })
        }
        (true, RelativeState::Modified(ours), RelativeState::Deleted) => {
            MergeResolution::Conflict(ConflictKind::OursModifiedTheirsDeleted { ours: ours.hash })
        }
        _ => MergeResolution::Delete,
    })
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

fn load_merge_blob(hash: ObjectHash) -> Result<Blob, PullMergeError> {
    load_object(&hash).map_err(|error| PullMergeError::ObjectLoad {
        object_id: hash.to_string(),
        detail: error.to_string(),
    })
}

fn merge_tree_items(
    base_items: &HashMap<PathBuf, MergeTreeEntry>,
    our_items: &HashMap<PathBuf, MergeTreeEntry>,
    their_items: &HashMap<PathBuf, MergeTreeEntry>,
) -> Result<ThreeWayMergeResult, PullMergeError> {
    let mut all_paths: HashSet<PathBuf> = base_items.keys().cloned().collect();
    all_paths.extend(our_items.keys().cloned());
    all_paths.extend(their_items.keys().cloned());

    let mut merged_items = HashMap::new();
    let mut conflicts = Vec::new();
    for path in all_paths {
        match resolve_three_way(
            base_items.get(&path),
            our_items.get(&path),
            their_items.get(&path),
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

fn write_workdir_file(workdir: &Path, relative: &Path, content: &[u8]) -> Result<(), String> {
    let file_path = workdir.join(relative);
    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    fs::write(&file_path, content)
        .map_err(|error| format!("failed to write {}: {error}", file_path.display()))
}

fn conflict_marker_eol() -> &'static str {
    if cfg!(windows) { "\r\n" } else { "\n" }
}

fn conflict_payload(content: &[u8]) -> Cow<'_, str> {
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
) -> Result<(), String> {
    let content = match kind {
        ConflictKind::BothChanged { ours, theirs } => {
            let ours_blob: Blob = load_object(&ours).map_err(|error| error.to_string())?;
            let theirs_blob: Blob = load_object(&theirs).map_err(|error| error.to_string())?;
            format!(
                "<<<<<<< HEAD{marker_eol}{}{marker_eol}======={marker_eol}{}{marker_eol}>>>>>>> {}{marker_eol}",
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
            write_workdir_file(&workdir, &path_buf, &blob.data)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn merge_entry(byte: u8, mode: TreeItemMode) -> MergeTreeEntry {
        MergeTreeEntry {
            hash: ObjectHash::new(&[byte; 20]),
            mode,
        }
    }

    #[test]
    fn merge_args_parse_ff_flags() {
        let no_ff = MergeArgs::try_parse_from(["merge", "--no-ff", "feature"]).unwrap();
        assert!(no_ff.no_ff);
        assert!(!no_ff.ff_only);
        assert_eq!(no_ff.branch.as_deref(), Some("feature"));

        let ff_only = MergeArgs::try_parse_from(["merge", "--ff-only", "feature"]).unwrap();
        assert!(ff_only.ff_only);
        assert!(!ff_only.no_ff);

        let with_msg = MergeArgs::try_parse_from(["merge", "-m", "custom", "feature"]).unwrap();
        assert_eq!(with_msg.message.as_deref(), Some("custom"));

        let squash = MergeArgs::try_parse_from(["merge", "--squash", "feature"]).unwrap();
        assert!(squash.squash);
        let no_commit = MergeArgs::try_parse_from(["merge", "--no-commit", "feature"]).unwrap();
        assert!(no_commit.no_commit);
        // --squash and --no-commit are mutually exclusive.
        assert!(
            MergeArgs::try_parse_from(["merge", "--squash", "--no-commit", "feature"]).is_err()
        );
    }

    #[test]
    fn merge_args_ff_only_conflicts_with_no_ff() {
        let err = MergeArgs::try_parse_from(["merge", "--ff-only", "--no-ff", "feature"])
            .expect_err("--ff-only and --no-ff are mutually exclusive");
        assert!(err.to_string().contains("cannot be used with"));
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
            PullMergeError::UnsignedMergeCommit {
                commit: "abc1234".to_string(),
            }
            .to_string(),
            "commit abc1234 does not have a GPG signature",
        );
        assert_eq!(
            PullMergeError::BadMergeSignature {
                commit: "def5678".to_string(),
            }
            .to_string(),
            "commit def5678 has a bad GPG signature",
        );
        assert_eq!(
            PullMergeError::SignatureCheck("vault sealed".to_string()).to_string(),
            "failed to verify the signature of the merged commit: vault sealed",
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
