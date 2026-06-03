//! Handles checkout-style flows to show the current branch, switch to existing branches, or create and switch to a new one using restore utilities.

use std::{fs, path::PathBuf};

use clap::Parser;
use git_internal::{
    hash::{ObjectHash, get_hash_kind},
    internal::{index::Index, object::blob::Blob},
};
use sea_orm::DbErr;
use serde::Serialize;

use crate::{
    command::{
        branch, get_target_commit, load_object, pull,
        restore::{self, RestoreArgs},
        switch,
    },
    info_println,
    internal::{
        branch::{
            AGENT_TRACES_BRANCH, Branch, BranchStoreError, INTENT_BRANCH, is_ai_managed_branch,
            is_ai_managed_revision,
        },
        head::Head,
        reflog::{ReflogAction, ReflogContext, with_reflog},
    },
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
        path, util,
    },
};

const CHECKOUT_EXAMPLES: &str = "\
NOTE:
    libra checkout is a branch compatibility surface. New code paths
    should prefer:
      - `libra switch <branch>` / `libra switch -c <branch>` for branch
        navigation and creation
      - `libra restore <path>` to restore files from the index or HEAD

EXAMPLES:
    libra checkout                         Show the current branch
    libra checkout main                    Switch to a branch (prefer: libra switch main)
    libra checkout feature-x               Switch to another branch (prefer: libra switch feature-x)
    libra checkout -b feature-x            Create + switch to a new branch (prefer: libra switch -c feature-x)
    libra checkout -B feature-x            Create or reset a branch to HEAD, then switch
    libra checkout --detach HEAD~1         Detach HEAD at a commit (no branch)
    libra checkout --orphan fresh          Start a new unborn branch with no history
    libra checkout -f main                 Force switch, discarding local changes
    libra checkout --ours -- file.txt      Take our side of a conflicted path
    libra checkout --theirs -- file.txt    Take their side of a conflicted path
    libra checkout -- file.txt             Restore a path from the index (prefer: libra restore file.txt)
    libra checkout HEAD -- file.txt        Restore a path from HEAD into index + worktree
    libra --json checkout main             Structured compatibility output
    libra checkout --quiet main            Switch without informational stdout";

#[derive(Parser, Debug)]
#[command(after_help = CHECKOUT_EXAMPLES)]
pub struct CheckoutArgs {
    /// Target branch, commit, or tag to check out (prefer `libra switch` for branches)
    branch: Option<String>,

    /// Create and switch to a new branch with the same content as the current branch
    #[clap(short = 'b', group = "sub")]
    new_branch: Option<String>,

    /// Create or reset a branch to the start point (or current HEAD) and switch to it
    #[clap(short = 'B', value_name = "branch", group = "sub")]
    force_new_branch: Option<String>,

    /// Detach HEAD at the given commit-ish (or current HEAD) instead of switching to a branch
    #[clap(long = "detach", conflicts_with = "sub")]
    detach: bool,

    /// Create a new unborn branch whose first commit will have no parents
    #[clap(long = "orphan", value_name = "branch", group = "sub")]
    orphan: Option<String>,

    /// On a conflicted path, check out our side of the merge (stage #2); requires `-- <path>`
    #[clap(long = "ours", conflicts_with = "theirs")]
    ours: bool,

    /// On a conflicted path, check out their side of the merge (stage #3); requires `-- <path>`
    #[clap(long = "theirs")]
    theirs: bool,

    /// Force checkout: proceed even when the working tree has changes that would be overwritten
    #[clap(short = 'f', long = "force")]
    force: bool,

    /// Paths to restore after an explicit `--` separator
    #[clap(last = true, value_name = "pathspec")]
    pathspec: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct CheckoutOutput {
    action: String,
    previous_branch: Option<String>,
    previous_commit: Option<String>,
    branch: Option<String>,
    commit: Option<String>,
    short_commit: Option<String>,
    switched: bool,
    created: bool,
    pulled: bool,
    already_on: bool,
    detached: bool,
    /// `true` when `-B` reset an already-existing branch (vs. creating it).
    #[serde(default)]
    reset: bool,
    /// `true` when the target is an unborn `--orphan` branch.
    #[serde(default)]
    orphan: bool,
    tracking: Option<CheckoutTrackingOutput>,
    restore: Option<restore::RestoreOutput>,
}

#[derive(Debug, Clone, Serialize)]
struct CheckoutTrackingOutput {
    remote: String,
    remote_branch: String,
}

#[derive(Debug, thiserror::Error)]
enum CheckoutError {
    #[error("checking out '{0}' branch is not allowed")]
    CheckingOutBranchBlocked(String),

    #[error("creating/switching to '{0}' branch is not allowed")]
    CreatingBranchBlocked(String),

    #[error("switching to '{0}' branch is not allowed")]
    SwitchingToBranchBlocked(String),

    #[error("branch '{0}' not found")]
    BranchNotFound(String),

    #[error("path specification '{0}' did not match any files known to libra")]
    PathSpecNotMatched(String),

    #[error("unstaged changes, can't switch branch")]
    DirtyUnstaged,

    #[error("uncommitted changes, can't switch branch")]
    DirtyUncommitted,

    #[error("untracked working tree file would be overwritten by checkout: {0}")]
    UntrackedOverwrite(String),

    #[error("checkout path mode cannot be combined with {0}")]
    InvalidPathMode(String),

    #[error("failed to {context}: {detail}")]
    BranchStoreRead { context: String, detail: String },

    #[error("failed to {context}: {detail}")]
    BranchStoreCorrupt { context: String, detail: String },

    #[error("failed to update HEAD/branch reference: {detail}")]
    HeadUpdateFailed { detail: String },

    #[error("'{flag}' requires a pathspec after '--' (e.g. 'libra checkout {flag} -- <path>')")]
    ConflictPathRequired { flag: &'static str },

    #[error("path '{path}' is not in a merge conflict state")]
    NotInMergeConflict { path: String },

    #[error("failed to read index/object for '{path}': {detail}")]
    IndexReadFailed { path: String, detail: String },

    #[error("failed to write '{path}': {detail}")]
    WorktreeWriteFailed { path: String, detail: String },

    #[error("checkout remote branch left HEAD without a commit")]
    RemoteHeadMissing,

    #[error("failed to {stage} during remote branch checkout: {}", source.message())]
    RemoteSyncFailed {
        stage: &'static str,
        #[source]
        source: Box<CliError>,
    },

    #[error(transparent)]
    DelegatedCli(#[from] CliError),
}

impl From<CheckoutError> for CliError {
    fn from(error: CheckoutError) -> Self {
        match error {
            CheckoutError::CheckingOutBranchBlocked(branch) => {
                CliError::fatal(format!("checking out '{}' branch is not allowed", branch))
                    .with_stable_code(StableErrorCode::CliInvalidTarget)
            }

            CheckoutError::CreatingBranchBlocked(branch) => CliError::fatal(format!(
                "creating/switching to '{}' branch is not allowed",
                branch
            ))
            .with_stable_code(StableErrorCode::CliInvalidTarget),

            CheckoutError::SwitchingToBranchBlocked(branch) => {
                CliError::fatal(format!("switching to '{}' branch is not allowed", branch))
                    .with_stable_code(StableErrorCode::CliInvalidTarget)
            }

            CheckoutError::BranchNotFound(branch) => {
                CliError::fatal(format!("branch '{}' not found", branch))
                    .with_stable_code(StableErrorCode::CliInvalidTarget)
            }

            CheckoutError::PathSpecNotMatched(spec) => CliError::fatal(format!(
                "path specification '{}' did not match any files known to libra",
                spec
            ))
            .with_stable_code(StableErrorCode::CliInvalidTarget),

            CheckoutError::DirtyUnstaged | CheckoutError::DirtyUncommitted => {
                CliError::failure("local changes would be overwritten by checkout")
                    .with_stable_code(StableErrorCode::RepoStateInvalid)
            }

            CheckoutError::UntrackedOverwrite(path) => CliError::failure(format!(
                "local changes would be overwritten by checkout: {path}"
            ))
            .with_stable_code(StableErrorCode::ConflictOperationBlocked),

            CheckoutError::InvalidPathMode(flag) => CliError::fatal(format!(
                "checkout path mode cannot be combined with {flag}"
            ))
            .with_stable_code(StableErrorCode::CliInvalidArguments)
            .with_hint(
                "use 'libra restore' for file restoration, or omit '--' for branch checkout",
            ),

            CheckoutError::BranchStoreRead { context, detail } => {
                CliError::fatal(format!("failed to {context}: {detail}"))
                    .with_stable_code(StableErrorCode::IoReadFailed)
            }
            CheckoutError::BranchStoreCorrupt { context, detail } => {
                CliError::fatal(format!("failed to {context}: {detail}"))
                    .with_stable_code(StableErrorCode::RepoCorrupt)
            }
            CheckoutError::HeadUpdateFailed { detail } => {
                CliError::fatal(format!("failed to update HEAD/branch reference: {detail}"))
                    .with_stable_code(StableErrorCode::IoWriteFailed)
                    .with_hint("check that the repository database is writable, then retry")
            }
            CheckoutError::ConflictPathRequired { flag } => CliError::fatal(format!(
                "'{flag}' requires a pathspec after '--' (e.g. 'libra checkout {flag} -- <path>')"
            ))
            .with_stable_code(StableErrorCode::CliInvalidArguments),
            CheckoutError::NotInMergeConflict { path } => {
                CliError::failure(format!("path '{path}' is not in a merge conflict state"))
                    .with_stable_code(StableErrorCode::ConflictOperationBlocked)
                    .with_hint("only conflicted paths accept '--ours'/'--theirs'")
            }
            CheckoutError::IndexReadFailed { path, detail } => CliError::fatal(format!(
                "failed to read index/object for '{path}': {detail}"
            ))
            .with_stable_code(StableErrorCode::IoReadFailed),
            CheckoutError::WorktreeWriteFailed { path, detail } => {
                CliError::fatal(format!("failed to write '{path}': {detail}"))
                    .with_stable_code(StableErrorCode::IoWriteFailed)
                    .with_hint("check that the path is writable, then retry")
            }
            CheckoutError::RemoteHeadMissing => {
                CliError::fatal("checkout remote branch left HEAD without a commit")
                    .with_stable_code(StableErrorCode::RepoStateInvalid)
            }
            CheckoutError::RemoteSyncFailed { stage, source } => {
                let inner = *source;
                let stable_code = inner.stable_code();
                let message = format!(
                    "failed to {stage} during remote branch checkout: {}",
                    inner.message()
                );
                let wrapped = match inner.kind() {
                    crate::utils::error::CliErrorKind::Fatal => CliError::fatal(message),
                    _ => CliError::failure(message),
                };
                wrapped.with_stable_code(stable_code)
            }
            CheckoutError::DelegatedCli(err) => err,
        }
    }
}

pub async fn execute(args: CheckoutArgs) {
    if let Err(e) = execute_safe(args, &OutputConfig::default()).await {
        e.print_stderr();
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting.
///
/// # Side Effects
/// - Validates target branch names and blocks the internal `intent` branch.
/// - May create a branch when `-b` is supplied.
/// - Switches HEAD/current branch and restores the working tree to the target.
/// - Emits status messages through [`OutputConfig`].
///
/// # Errors
/// Returns [`CliError`] when the target branch is invalid or missing, local
/// changes would be overwritten, branch creation fails, or checkout/restore
/// writes fail.
pub async fn execute_safe(args: CheckoutArgs, output: &OutputConfig) -> CliResult<()> {
    let result = run_checkout(args, output).await.map_err(CliError::from)?;
    render_checkout_output(&result, output)
}

async fn run_checkout(
    args: CheckoutArgs,
    output: &OutputConfig,
) -> Result<CheckoutOutput, CheckoutError> {
    // `--ours`/`--theirs` operate on conflicted paths only; without a pathspec
    // after `--` they are a usage error (mirrors Git's "no paths given").
    if (args.ours || args.theirs) && args.pathspec.is_empty() {
        let flag = if args.ours { "--ours" } else { "--theirs" };
        return Err(CheckoutError::ConflictPathRequired { flag });
    }
    if !args.pathspec.is_empty() {
        return restore_checkout_paths(args).await;
    }

    // ── AI-managed branch isolation (intent / agent-traces only; NOT main) ──
    // Branch NAMES created or reset by -b / -B / --orphan use the plain-name
    // check; the positional commit-ish / start_point (which may carry a
    // revision suffix like `agent-traces~1`) uses the revision-aware check.
    for new_name in [
        args.new_branch.as_deref(),
        args.force_new_branch.as_deref(),
        args.orphan.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if is_ai_managed_branch(new_name) {
            return Err(CheckoutError::CreatingBranchBlocked(new_name.to_string()));
        }
    }
    let positional_is_commitish =
        args.force_new_branch.is_some() || args.detach || args.orphan.is_some();
    if positional_is_commitish
        && let Some(rev) = args.branch.as_deref()
        && is_ai_managed_revision(rev)
    {
        return Err(CheckoutError::CheckingOutBranchBlocked(rev.to_string()));
    }
    if !positional_is_commitish
        && let Some(name) = args.branch.as_deref()
        && is_ai_managed_branch(name)
    {
        return Err(CheckoutError::CheckingOutBranchBlocked(name.to_string()));
    }

    // ── New branch-control modes (Batch 0) ──
    if let Some(force_branch) = args.force_new_branch.clone() {
        return run_force_branch_checkout(&force_branch, args.branch.clone(), args.force, output)
            .await;
    }
    if args.detach {
        return run_detach_checkout(args.branch.clone(), args.force, output).await;
    }
    if let Some(orphan_branch) = args.orphan.clone() {
        return run_orphan_checkout(&orphan_branch, args.branch.clone(), args.force, output).await;
    }

    let previous_branch = get_current_branch().await;
    let previous_commit = current_commit_string().await?;

    // Match Git behavior: checking out the current branch is a no-op and should
    // not be blocked by unrelated local changes.
    if let Some(ref target_branch) = args.branch
        && previous_branch.as_ref() == Some(target_branch)
    {
        return Ok(CheckoutOutput {
            action: "already-on".to_string(),
            previous_branch,
            previous_commit: previous_commit.clone(),
            branch: Some(target_branch.clone()),
            commit: previous_commit.clone(),
            short_commit: previous_commit.as_deref().map(short_oid),
            switched: false,
            created: false,
            pulled: false,
            already_on: true,
            detached: false,
            reset: false,
            orphan: false,
            tracking: None,
            restore: None,
        });
    }

    let target_commit = if let Some(ref branch_name) = args.branch {
        Branch::find_branch_result(branch_name, None)
            .await
            .map_err(|error| map_checkout_branch_store_error("resolve checkout target", error))?
            .map(|branch| branch.commit)
    } else {
        None
    };

    // `-f`/`--force` skips the dirty-working-tree guard, letting the target
    // commit overwrite uncommitted changes (matches Git's forced checkout).
    if !args.force {
        let clean_status = match target_commit {
            Some(target_commit) => {
                switch::ensure_clean_status_for_commit(target_commit, output).await
            }
            None => switch::ensure_clean_status(output).await,
        };
        map_clean_status(clean_status)?;
    }

    match (args.branch, args.new_branch) {
        (Some(target_branch), _) => {
            check_and_switch_branch(&target_branch, previous_branch, previous_commit, output).await
        }
        (None, Some(new_branch)) => {
            let child_output = silent_child_output(output);
            let commit = create_and_switch_new_branch(&new_branch, &child_output).await?;
            let commit = commit.to_string();
            Ok(CheckoutOutput {
                action: "create".to_string(),
                previous_branch,
                previous_commit,
                branch: Some(new_branch),
                short_commit: Some(short_oid(&commit)),
                commit: Some(commit),
                switched: true,
                created: true,
                pulled: false,
                already_on: false,
                detached: false,
                reset: false,
                orphan: false,
                tracking: None,
                restore: None,
            })
        }
        (None, None) => show_current_branch(previous_branch, previous_commit).await,
    }
}

async fn restore_checkout_paths(args: CheckoutArgs) -> Result<CheckoutOutput, CheckoutError> {
    if args.new_branch.is_some() {
        return Err(CheckoutError::InvalidPathMode("-b".to_string()));
    }

    // `--ours`/`--theirs` need a dedicated direct-write path: `RestoreArgs`
    // carries only `source`/`staged`/`pathspec` and cannot target a merge stage.
    if args.ours || args.theirs {
        return restore_conflict_stage_paths(&args.pathspec, args.ours).await;
    }

    let previous_branch = get_current_branch().await;
    let previous_commit = current_commit_string().await?;
    let source = args.branch;
    let restore_args = RestoreArgs {
        worktree: true,
        staged: source.is_some(),
        source,
        pathspec: args.pathspec,
    };
    let restore = restore::execute_to_output(restore_args)
        .await
        .map_err(CheckoutError::DelegatedCli)?;
    let was_detached = previous_branch.is_none();

    Ok(CheckoutOutput {
        action: "restore-paths".to_string(),
        previous_branch: previous_branch.clone(),
        previous_commit: previous_commit.clone(),
        branch: previous_branch,
        commit: previous_commit.clone(),
        short_commit: previous_commit.as_deref().map(short_oid),
        switched: false,
        created: false,
        pulled: false,
        already_on: false,
        detached: was_detached,
        reset: false,
        orphan: false,
        tracking: None,
        restore: Some(restore),
    })
}

/// `--ours` / `--theirs` path checkout: for each conflicted pathspec, restore the
/// requested merge stage (stage #2 = ours, stage #3 = theirs) into the working
/// tree and collapse the index to a clean stage #0 entry, dropping the remaining
/// conflict stages.
///
/// Uses a dedicated direct-write path because [`RestoreArgs`] can only select a
/// `source`/`staged` target, never a specific merge stage. Validation is a
/// pre-pass: every pathspec must carry the requested stage before *any* write
/// happens, so a non-conflicted path fails the whole operation cleanly (no
/// half-restored worktree). The promoted stage #0 entry is taken **owned** from
/// `Index::remove` (since `IndexEntry` is not `Clone`) and only has its stage
/// rewritten, preserving the original mode/metadata — `IndexEntry::new_from_blob`
/// is avoided because it hard-codes `0o100644` and would drop the executable bit.
async fn restore_conflict_stage_paths(
    pathspec: &[String],
    use_ours: bool,
) -> Result<CheckoutOutput, CheckoutError> {
    let stage: u8 = if use_ours { 2 } else { 3 };
    let other_stage: u8 = if use_ours { 3 } else { 2 };

    let index_path = path::index();
    // Use a stable "<index>" label rather than the absolute on-disk path so error
    // messages never leak internal filesystem locations.
    let mut index = Index::load(&index_path).map_err(|e| CheckoutError::IndexReadFailed {
        path: "<index>".to_string(),
        detail: e.to_string(),
    })?;

    // Pre-pass: every pathspec must be in a conflict state for the requested
    // side before we touch the worktree or index.
    let mut targets = Vec::with_capacity(pathspec.len());
    for spec in pathspec {
        let hash = index
            .get_hash(spec, stage)
            .ok_or_else(|| CheckoutError::NotInMergeConflict { path: spec.clone() })?;
        targets.push((spec.clone(), hash));
    }

    let mut restored_files = Vec::with_capacity(targets.len());
    for (spec, hash) in &targets {
        // Read the chosen-stage blob and write it to the working tree.
        let blob = load_object::<Blob>(hash).map_err(|e| CheckoutError::IndexReadFailed {
            path: spec.clone(),
            detail: e.to_string(),
        })?;
        let path_abs = util::workdir_to_absolute(PathBuf::from(spec));
        if let Some(parent) = path_abs.parent() {
            fs::create_dir_all(parent).map_err(|e| CheckoutError::WorktreeWriteFailed {
                path: spec.clone(),
                detail: e.to_string(),
            })?;
        }
        util::write_file(&blob.data, &path_abs).map_err(|e| {
            CheckoutError::WorktreeWriteFailed {
                path: spec.clone(),
                detail: e.to_string(),
            }
        })?;

        // Promote the chosen stage to stage #0 (owned move; preserves mode), then
        // drop the base (#1) and the opposite side so only stage #0 remains.
        let mut entry = index
            .remove(spec, stage)
            .ok_or_else(|| CheckoutError::NotInMergeConflict { path: spec.clone() })?;
        entry.flags.stage = 0;
        index.add(entry);
        index.remove(spec, 1);
        index.remove(spec, other_stage);

        restored_files.push(spec.clone());
    }

    index
        .to_file(&index_path)
        .map_err(|e| CheckoutError::WorktreeWriteFailed {
            path: "<index>".to_string(),
            detail: e.to_string(),
        })?;

    let previous_branch = get_current_branch().await;
    let previous_commit = current_commit_string().await?;
    let was_detached = previous_branch.is_none();
    Ok(CheckoutOutput {
        action: "restore-paths".to_string(),
        previous_branch: previous_branch.clone(),
        previous_commit: previous_commit.clone(),
        branch: previous_branch,
        commit: previous_commit.clone(),
        short_commit: previous_commit.as_deref().map(short_oid),
        switched: false,
        created: false,
        pulled: false,
        already_on: false,
        detached: was_detached,
        reset: false,
        orphan: false,
        tracking: None,
        restore: Some(restore::RestoreOutput {
            source: Some(
                if use_ours {
                    "stage2-ours"
                } else {
                    "stage3-theirs"
                }
                .to_string(),
            ),
            worktree: true,
            staged: true,
            restored_files,
            deleted_files: Vec::new(),
        }),
    })
}

fn map_checkout_branch_store_error(context: &str, error: BranchStoreError) -> CheckoutError {
    match error {
        BranchStoreError::Query(detail) => CheckoutError::BranchStoreRead {
            context: context.to_string(),
            detail,
        },
        other => CheckoutError::BranchStoreCorrupt {
            context: context.to_string(),
            detail: other.to_string(),
        },
    }
}

pub async fn get_current_branch() -> Option<String> {
    match Head::current().await {
        Head::Detached(_) => None,
        Head::Branch(name) => Some(name),
    }
}

async fn current_commit_string() -> Result<Option<String>, CheckoutError> {
    Head::current_commit_result()
        .await
        .map(|commit| commit.map(|hash| hash.to_string()))
        .map_err(|error| CheckoutError::BranchStoreCorrupt {
            context: "resolve HEAD commit".to_string(),
            detail: error.to_string(),
        })
}

pub async fn switch_branch(branch_name: &str) -> CliResult<()> {
    switch_branch_with_output(branch_name, &OutputConfig::default())
        .await
        .map(|_| ())
        .map_err(CliError::from)
}

async fn switch_branch_with_output(
    branch_name: &str,
    output: &OutputConfig,
) -> Result<ObjectHash, CheckoutError> {
    if branch_name == INTENT_BRANCH || branch_name == AGENT_TRACES_BRANCH {
        return Err(CheckoutError::SwitchingToBranchBlocked(
            branch_name.to_string(),
        ));
    }
    let target_branch = Branch::find_branch_result(branch_name, None)
        .await
        .map_err(|error| map_checkout_branch_store_error("resolve branch", error))?
        .ok_or_else(|| CheckoutError::BranchNotFound(branch_name.to_string()))?;
    let target_commit = target_branch.commit;
    restore_to_commit(target_branch.commit, output)
        .await
        .map_err(CheckoutError::DelegatedCli)?;
    let head = Head::Branch(branch_name.to_string());
    Head::update(head, None).await;
    Ok(target_commit)
}

async fn create_and_switch_new_branch(
    new_branch: &str,
    output: &OutputConfig,
) -> Result<ObjectHash, CheckoutError> {
    branch::create_branch_safe(new_branch.to_string(), get_current_branch().await)
        .await
        .map_err(CheckoutError::DelegatedCli)?;
    switch_branch_with_output(new_branch, output).await
}

/// Resolve an optional start-point / commit-ish to a commit hash.
///
/// `Some(rev)` is resolved through the usual rev parser (branch / `HEAD` / OID /
/// `~`/`^` suffixes); an unresolvable rev becomes [`CheckoutError::BranchNotFound`].
/// `None` falls back to the current `HEAD` commit, which is `Ok(None)` on an
/// unborn HEAD (no commit yet) — callers decide whether that is permitted.
async fn resolve_start_point(
    start_point: Option<&str>,
) -> Result<Option<ObjectHash>, CheckoutError> {
    match start_point {
        Some(rev) => get_target_commit(rev)
            .await
            .map(Some)
            .map_err(|_| CheckoutError::BranchNotFound(rev.to_string())),
        None => {
            Head::current_commit_result()
                .await
                .map_err(|error| CheckoutError::BranchStoreCorrupt {
                    context: "resolve HEAD commit".to_string(),
                    detail: error.to_string(),
                })
        }
    }
}

/// Map the dirty-status outcome of a `switch::ensure_clean_status*` call to the
/// checkout error domain.
fn map_clean_status(result: Result<(), switch::SwitchError>) -> Result<(), CheckoutError> {
    match result {
        Ok(()) => Ok(()),
        Err(switch::SwitchError::DirtyUnstaged) => Err(CheckoutError::DirtyUnstaged),
        Err(switch::SwitchError::DirtyUncommitted) => Err(CheckoutError::DirtyUncommitted),
        Err(switch::SwitchError::UntrackedOverwrite(path)) => {
            Err(CheckoutError::UntrackedOverwrite(path))
        }
        Err(err) => Err(CheckoutError::DelegatedCli(CliError::from(err))),
    }
}

/// Write a `checkout` reflog entry (action `checkout`, message
/// `moving from <from> to <to>`) while updating HEAD and, optionally, a branch
/// ref — atomically within one transaction.
///
/// Uses the **error-propagating** `Head::update_result_with_conn` so a failed
/// HEAD/ref write rolls back the entire transaction (no half-written ref or
/// reflog). `branch_ref = Some((name, commit))` also (re)creates/resets that
/// branch ref in the same transaction (used by `-B`); `--orphan` passes `None`
/// to leave the ref unborn. `insert_ref` mirrors Git: `true` for branch modes
/// (writes the extra `refs/heads/<branch>` reflog row), `false` for `--detach`.
async fn write_checkout_reflog(
    new_head: Head,
    branch_ref: Option<(String, ObjectHash)>,
    from_label: String,
    to_label: String,
    old_oid: String,
    new_oid: String,
    insert_ref: bool,
) -> Result<(), CheckoutError> {
    let context = ReflogContext {
        old_oid,
        new_oid,
        action: ReflogAction::Checkout {
            from: from_label,
            to: to_label,
        },
    };
    with_reflog(
        context,
        move |txn: &sea_orm::DatabaseTransaction| {
            Box::pin(async move {
                if let Some((branch_name, commit)) = branch_ref {
                    let commit_str = commit.to_string();
                    Branch::update_branch_with_conn(txn, &branch_name, &commit_str, None).await?;
                }
                Head::update_result_with_conn(txn, new_head, None)
                    .await
                    .map_err(|e| DbErr::Custom(e.to_string()))?;
                Ok(())
            })
        },
        insert_ref,
    )
    .await
    .map_err(|e| CheckoutError::HeadUpdateFailed {
        detail: e.to_string(),
    })
}

/// Label for the `from` side of a checkout reflog message: the current branch
/// name, or a short OID when HEAD is detached / unborn.
fn reflog_from_label(previous_branch: &Option<String>, old_oid: &str) -> String {
    previous_branch
        .clone()
        .unwrap_or_else(|| short_oid(old_oid))
}

/// `-B <branch> [<start_point>]`: create the branch (if absent) or reset it (if
/// present) to the resolved start point, restore the worktree, and switch.
async fn run_force_branch_checkout(
    branch_name: &str,
    start_point: Option<String>,
    force: bool,
    output: &OutputConfig,
) -> Result<CheckoutOutput, CheckoutError> {
    let previous_branch = get_current_branch().await;
    let previous_commit = current_commit_string().await?;

    let resolved_commit = resolve_start_point(start_point.as_deref())
        .await?
        .ok_or_else(|| {
            CheckoutError::BranchNotFound(start_point.clone().unwrap_or_else(|| "HEAD".to_string()))
        })?;

    // clean-status → restore happen BEFORE the ref is reset, so a failure here
    // leaves the target branch unchanged (no half-reset). `-f` skips the guard.
    if !force {
        map_clean_status(switch::ensure_clean_status_for_commit(resolved_commit, output).await)?;
    }
    restore_to_commit(resolved_commit, output)
        .await
        .map_err(CheckoutError::DelegatedCli)?;

    let existed = Branch::find_branch_result(branch_name, None)
        .await
        .map_err(|error| map_checkout_branch_store_error("resolve branch", error))?
        .is_some();

    let old_oid = previous_commit
        .clone()
        .unwrap_or_else(|| ObjectHash::zero_str(get_hash_kind()));
    let new_oid = resolved_commit.to_string();
    write_checkout_reflog(
        Head::Branch(branch_name.to_string()),
        Some((branch_name.to_string(), resolved_commit)),
        reflog_from_label(&previous_branch, &old_oid),
        branch_name.to_string(),
        old_oid,
        new_oid,
        true,
    )
    .await?;

    let commit = resolved_commit.to_string();
    Ok(CheckoutOutput {
        action: if existed { "reset" } else { "create" }.to_string(),
        previous_branch,
        previous_commit,
        branch: Some(branch_name.to_string()),
        short_commit: Some(short_oid(&commit)),
        commit: Some(commit),
        switched: true,
        created: !existed,
        pulled: false,
        already_on: false,
        detached: false,
        reset: existed,
        orphan: false,
        tracking: None,
        restore: None,
    })
}

/// `--detach [<commit-ish>]`: restore the worktree to the resolved commit and
/// move HEAD into the detached state.
async fn run_detach_checkout(
    commit_ish: Option<String>,
    force: bool,
    output: &OutputConfig,
) -> Result<CheckoutOutput, CheckoutError> {
    let previous_branch = get_current_branch().await;
    let previous_commit = current_commit_string().await?;

    let resolved_commit = resolve_start_point(commit_ish.as_deref())
        .await?
        .ok_or_else(|| {
            CheckoutError::BranchNotFound(commit_ish.clone().unwrap_or_else(|| "HEAD".to_string()))
        })?;

    if !force {
        map_clean_status(switch::ensure_clean_status_for_commit(resolved_commit, output).await)?;
    }
    restore_to_commit(resolved_commit, output)
        .await
        .map_err(CheckoutError::DelegatedCli)?;

    let old_oid = previous_commit
        .clone()
        .unwrap_or_else(|| ObjectHash::zero_str(get_hash_kind()));
    let new_oid = resolved_commit.to_string();
    write_checkout_reflog(
        Head::Detached(resolved_commit),
        None,
        reflog_from_label(&previous_branch, &old_oid),
        short_oid(&new_oid),
        old_oid,
        new_oid,
        false,
    )
    .await?;

    let commit = resolved_commit.to_string();
    Ok(CheckoutOutput {
        action: "detach".to_string(),
        previous_branch,
        previous_commit,
        branch: None,
        short_commit: Some(short_oid(&commit)),
        commit: Some(commit),
        switched: true,
        created: false,
        pulled: false,
        already_on: false,
        detached: true,
        reset: false,
        orphan: false,
        tracking: None,
        restore: None,
    })
}

/// `--orphan <branch> [<start_point>]`: point HEAD at a new unborn branch (no
/// `reference` row until the first commit). The index/worktree are aligned to
/// the start point (default current HEAD), matching Git's "as if you had run
/// checkout <start-point>"; an empty/unborn repo simply renames the unborn HEAD.
async fn run_orphan_checkout(
    branch_name: &str,
    start_point: Option<String>,
    force: bool,
    output: &OutputConfig,
) -> Result<CheckoutOutput, CheckoutError> {
    let previous_branch = get_current_branch().await;
    let previous_commit = current_commit_string().await?;

    // `None` here means: no explicit start point AND current HEAD is unborn.
    let start_commit = resolve_start_point(start_point.as_deref()).await?;

    if let Some(start_commit) = start_commit {
        if !force {
            map_clean_status(switch::ensure_clean_status_for_commit(start_commit, output).await)?;
        }
        restore_to_commit(start_commit, output)
            .await
            .map_err(CheckoutError::DelegatedCli)?;
    } else if !force {
        // Unborn HEAD with no start point: nothing to restore; only rename HEAD.
        map_clean_status(switch::ensure_clean_status(output).await)?;
    }

    // Git writes NO HEAD reflog entry for `checkout --orphan`: the target branch
    // is unborn, so there is no commit OID to record (verified against stock Git,
    // whose `.git/logs/HEAD` gains no entry). Point HEAD at the unborn branch
    // directly — no `reference` row, no reflog — using the error-propagating
    // update so a failed write surfaces as `HeadUpdateFailed` instead of panicking.
    Head::update_result(Head::Branch(branch_name.to_string()), None)
        .await
        .map_err(|error| CheckoutError::HeadUpdateFailed {
            detail: error.to_string(),
        })?;

    Ok(CheckoutOutput {
        action: "create".to_string(),
        previous_branch,
        previous_commit,
        branch: Some(branch_name.to_string()),
        short_commit: None,
        commit: None,
        switched: true,
        created: true,
        pulled: false,
        already_on: false,
        detached: false,
        reset: false,
        orphan: true,
        tracking: None,
        restore: None,
    })
}

async fn get_remote(branch_name: &str, output: &OutputConfig) -> Result<ObjectHash, CheckoutError> {
    let remote_branch_name: String = format!("origin/{branch_name}");
    let child_output = silent_child_output(output);

    create_and_switch_new_branch(branch_name, &child_output)
        .await
        .map_err(|err| wrap_remote_proxy_error("create local tracking branch", err))?;
    // Set branch upstream
    branch::set_upstream_safe_with_output(branch_name, &remote_branch_name, &child_output)
        .await
        .map_err(|err| CheckoutError::RemoteSyncFailed {
            stage: "set upstream",
            source: Box::new(err),
        })?;
    // Synchronous branches
    // Use the pull command to update the local branch with the latest changes from the remote branch
    pull::execute_safe(pull::PullArgs::make(None, None), &child_output)
        .await
        .map_err(|err| CheckoutError::RemoteSyncFailed {
            stage: "pull from remote",
            source: Box::new(err),
        })?;
    Head::current_commit_result()
        .await
        .map_err(|error| map_checkout_branch_store_error("resolve checkout result", error))?
        .ok_or(CheckoutError::RemoteHeadMissing)
}

/// Converts a [`CheckoutError`] surfaced by the local-creation step of remote tracking
/// into a [`CheckoutError::RemoteSyncFailed`] envelope so downstream callers see a single
/// proxy-error variant regardless of which sub-step failed.
fn wrap_remote_proxy_error(stage: &'static str, err: CheckoutError) -> CheckoutError {
    match err {
        already @ CheckoutError::RemoteSyncFailed { .. } => already,
        other => CheckoutError::RemoteSyncFailed {
            stage,
            source: Box::new(CliError::from(other)),
        },
    }
}

/// Returns `Ok(Some(true))` if remote branch found, `Ok(Some(false))` if local branch found,
/// `Ok(None)` if already on the branch.
pub async fn check_branch(branch_name: &str) -> CliResult<Option<bool>> {
    check_branch_with_output(branch_name, &OutputConfig::default())
        .await
        .map_err(CliError::from)
}

async fn check_branch_with_output(
    branch_name: &str,
    output: &OutputConfig,
) -> Result<Option<bool>, CheckoutError> {
    if get_current_branch().await == Some(branch_name.to_string()) {
        info_println!(output, "Already on {branch_name}");
        return Ok(None);
    }

    let target_branch: Option<Branch> = Branch::find_branch_result(branch_name, None)
        .await
        .map_err(|error| map_checkout_branch_store_error("resolve branch", error))?;
    if target_branch.is_none() {
        let remote_branch_name: String = format!("origin/{branch_name}");
        if !Branch::search_branch_result(&remote_branch_name)
            .await
            .map_err(|error| {
                map_checkout_branch_store_error("search remote tracking branches", error)
            })?
            .is_empty()
        {
            info_println!(
                output,
                "branch '{branch_name}' set up to track '{remote_branch_name}'."
            );
            Ok(Some(true))
        } else {
            Err(CheckoutError::PathSpecNotMatched(branch_name.to_string()))
        }
    } else {
        info_println!(output, "Switched to branch '{branch_name}'");
        Ok(Some(false))
    }
}

async fn check_and_switch_branch(
    branch_name: &str,
    previous_branch: Option<String>,
    previous_commit: Option<String>,
    output: &OutputConfig,
) -> Result<CheckoutOutput, CheckoutError> {
    let child_output = silent_child_output(output);
    match check_branch_with_output(branch_name, &child_output).await? {
        Some(true) => {
            let commit = get_remote(branch_name, output).await?.to_string();
            Ok(CheckoutOutput {
                action: "track".to_string(),
                previous_branch,
                previous_commit,
                branch: Some(branch_name.to_string()),
                commit: Some(commit.clone()),
                short_commit: Some(short_oid(&commit)),
                switched: true,
                created: true,
                pulled: true,
                already_on: false,
                detached: false,
                reset: false,
                orphan: false,
                tracking: Some(CheckoutTrackingOutput {
                    remote: "origin".to_string(),
                    remote_branch: format!("origin/{branch_name}"),
                }),
                restore: None,
            })
        }
        Some(false) => {
            let commit = switch_branch_with_output(branch_name, &child_output)
                .await?
                .to_string();
            Ok(CheckoutOutput {
                action: "switch".to_string(),
                previous_branch,
                previous_commit,
                branch: Some(branch_name.to_string()),
                commit: Some(commit.clone()),
                short_commit: Some(short_oid(&commit)),
                switched: true,
                created: false,
                pulled: false,
                already_on: false,
                detached: false,
                reset: false,
                orphan: false,
                tracking: None,
                restore: None,
            })
        }
        None => Ok(CheckoutOutput {
            action: "already-on".to_string(),
            previous_branch: previous_branch.clone(),
            previous_commit: previous_commit.clone(),
            branch: Some(branch_name.to_string()),
            commit: previous_commit.clone(),
            short_commit: previous_commit.as_deref().map(short_oid),
            switched: false,
            created: false,
            pulled: false,
            already_on: true,
            detached: false,
            reset: false,
            orphan: false,
            tracking: None,
            restore: None,
        }),
    }
}

async fn restore_to_commit(commit_id: ObjectHash, output: &OutputConfig) -> CliResult<()> {
    let restore_args = RestoreArgs {
        worktree: true,
        staged: true,
        source: Some(commit_id.to_string()),
        pathspec: vec![util::working_dir_string()],
    };
    restore::execute_safe(restore_args, &output.child_output_config()).await
}

async fn show_current_branch(
    current_branch: Option<String>,
    current_commit: Option<String>,
) -> Result<CheckoutOutput, CheckoutError> {
    match Head::current().await {
        Head::Detached(commit_hash) => {
            let commit = commit_hash.to_string();
            Ok(CheckoutOutput {
                action: "show-current".to_string(),
                previous_branch: current_branch,
                previous_commit: current_commit,
                branch: None,
                commit: Some(commit.clone()),
                short_commit: Some(short_oid(&commit)),
                switched: false,
                created: false,
                pulled: false,
                already_on: false,
                detached: true,
                reset: false,
                orphan: false,
                tracking: None,
                restore: None,
            })
        }
        Head::Branch(current_branch) => Ok(CheckoutOutput {
            action: "show-current".to_string(),
            previous_branch: Some(current_branch.clone()),
            previous_commit: current_commit.clone(),
            branch: Some(current_branch),
            commit: current_commit.clone(),
            short_commit: current_commit.as_deref().map(short_oid),
            switched: false,
            created: false,
            pulled: false,
            already_on: false,
            detached: false,
            reset: false,
            orphan: false,
            tracking: None,
            restore: None,
        }),
    }
}

fn render_checkout_output(result: &CheckoutOutput, output: &OutputConfig) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("checkout", result, output);
    }
    if output.quiet {
        return Ok(());
    }

    match result.action.as_str() {
        "show-current" if result.detached => {
            if let Some(short_commit) = &result.short_commit {
                println!("HEAD detached at {short_commit}");
            }
        }
        "show-current" => {
            if let Some(branch) = &result.branch {
                println!("Current branch is {branch}.");
            }
        }
        "already-on" => {
            if let Some(branch) = &result.branch {
                println!("Already on {branch}");
            }
        }
        "create" => {
            if let Some(branch) = &result.branch {
                println!("Switched to a new branch '{branch}'");
            }
        }
        "switch" => {
            if let Some(branch) = &result.branch {
                println!("Switched to branch '{branch}'");
            }
        }
        "reset" => {
            if let Some(branch) = &result.branch {
                println!("Reset branch '{branch}'");
                println!("Switched to branch '{branch}'");
            }
        }
        "detach" => {
            if let Some(short_commit) = &result.short_commit {
                println!("HEAD detached at {short_commit}");
            }
        }
        "track" => {
            if let (Some(branch), Some(tracking)) = (&result.branch, &result.tracking) {
                println!(
                    "branch '{branch}' set up to track '{}'.",
                    tracking.remote_branch
                );
                println!("Switched to a new branch '{branch}'");
            }
        }
        "restore-paths" => {
            if let Some(restore) = &result.restore {
                let total = restore.restored_files.len() + restore.deleted_files.len();
                if total > 0 {
                    let source_desc = restore.source.as_deref().unwrap_or("the index");
                    println!("Updated {total} path(s) from {source_desc}");
                }
            }
        }
        _ => {}
    }

    Ok(())
}

fn short_oid(oid: &str) -> String {
    oid.chars().take(8).collect()
}

fn silent_child_output(output: &OutputConfig) -> OutputConfig {
    let mut child = output.child_output_config();
    child.quiet = true;
    child
}

/// Unit tests for the checkout module
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkout_error_display_pins_owned_variants() {
        assert_eq!(
            CheckoutError::CheckingOutBranchBlocked("HEAD".to_string()).to_string(),
            "checking out 'HEAD' branch is not allowed",
        );
        assert_eq!(
            CheckoutError::CreatingBranchBlocked("HEAD".to_string()).to_string(),
            "creating/switching to 'HEAD' branch is not allowed",
        );
        assert_eq!(
            CheckoutError::SwitchingToBranchBlocked("intent".to_string()).to_string(),
            "switching to 'intent' branch is not allowed",
        );
        assert_eq!(
            CheckoutError::BranchNotFound("feature".to_string()).to_string(),
            "branch 'feature' not found",
        );
        assert_eq!(
            CheckoutError::PathSpecNotMatched("nonexistent".to_string()).to_string(),
            "path specification 'nonexistent' did not match any files known to libra",
        );
        assert_eq!(
            CheckoutError::DirtyUnstaged.to_string(),
            "unstaged changes, can't switch branch",
        );
        assert_eq!(
            CheckoutError::DirtyUncommitted.to_string(),
            "uncommitted changes, can't switch branch",
        );
        assert_eq!(
            CheckoutError::UntrackedOverwrite("src/new.rs".to_string()).to_string(),
            "untracked working tree file would be overwritten by checkout: src/new.rs",
        );
        assert_eq!(
            CheckoutError::BranchStoreRead {
                context: "load branch 'main'".to_string(),
                detail: "database is locked".to_string(),
            }
            .to_string(),
            "failed to load branch 'main': database is locked",
        );
        assert_eq!(
            CheckoutError::BranchStoreCorrupt {
                context: "resolve branch 'feature'".to_string(),
                detail: "ref points to non-commit object".to_string(),
            }
            .to_string(),
            "failed to resolve branch 'feature': ref points to non-commit object",
        );
        assert_eq!(
            CheckoutError::RemoteHeadMissing.to_string(),
            "checkout remote branch left HEAD without a commit",
        );
        assert_eq!(
            CheckoutError::ConflictPathRequired { flag: "--ours" }.to_string(),
            "'--ours' requires a pathspec after '--' (e.g. 'libra checkout --ours -- <path>')",
        );
        assert_eq!(
            CheckoutError::NotInMergeConflict {
                path: "a.txt".to_string(),
            }
            .to_string(),
            "path 'a.txt' is not in a merge conflict state",
        );
        assert_eq!(
            CheckoutError::IndexReadFailed {
                path: "a.txt".to_string(),
                detail: "object missing".to_string(),
            }
            .to_string(),
            "failed to read index/object for 'a.txt': object missing",
        );
        assert_eq!(
            CheckoutError::WorktreeWriteFailed {
                path: "a.txt".to_string(),
                detail: "disk full".to_string(),
            }
            .to_string(),
            "failed to write 'a.txt': disk full",
        );
        let proxy_err = CliError::failure("remote not configured")
            .with_stable_code(StableErrorCode::NetworkUnavailable);
        assert_eq!(
            CheckoutError::RemoteSyncFailed {
                stage: "pull from remote",
                source: Box::new(proxy_err),
            }
            .to_string(),
            "failed to pull from remote during remote branch checkout: remote not configured",
        );
    }

    #[test]
    fn checkout_error_maps_owned_variants_to_stable_codes() {
        let cases: Vec<(CheckoutError, StableErrorCode)> = vec![
            (
                CheckoutError::CheckingOutBranchBlocked("intent".to_string()),
                StableErrorCode::CliInvalidTarget,
            ),
            (
                CheckoutError::CreatingBranchBlocked("intent".to_string()),
                StableErrorCode::CliInvalidTarget,
            ),
            (
                CheckoutError::SwitchingToBranchBlocked("intent".to_string()),
                StableErrorCode::CliInvalidTarget,
            ),
            (
                CheckoutError::BranchNotFound("feature".to_string()),
                StableErrorCode::CliInvalidTarget,
            ),
            (
                CheckoutError::PathSpecNotMatched("nope".to_string()),
                StableErrorCode::CliInvalidTarget,
            ),
            (
                CheckoutError::DirtyUnstaged,
                StableErrorCode::RepoStateInvalid,
            ),
            (
                CheckoutError::DirtyUncommitted,
                StableErrorCode::RepoStateInvalid,
            ),
            (
                CheckoutError::UntrackedOverwrite("a.txt".to_string()),
                StableErrorCode::ConflictOperationBlocked,
            ),
            (
                CheckoutError::BranchStoreRead {
                    context: "resolve branch".to_string(),
                    detail: "database is locked".to_string(),
                },
                StableErrorCode::IoReadFailed,
            ),
            (
                CheckoutError::BranchStoreCorrupt {
                    context: "resolve branch".to_string(),
                    detail: "ref points to non-commit object".to_string(),
                },
                StableErrorCode::RepoCorrupt,
            ),
            (
                CheckoutError::RemoteHeadMissing,
                StableErrorCode::RepoStateInvalid,
            ),
            (
                CheckoutError::HeadUpdateFailed {
                    detail: "database is locked".to_string(),
                },
                StableErrorCode::IoWriteFailed,
            ),
            (
                CheckoutError::InvalidPathMode("-b".to_string()),
                StableErrorCode::CliInvalidArguments,
            ),
            (
                CheckoutError::ConflictPathRequired { flag: "--theirs" },
                StableErrorCode::CliInvalidArguments,
            ),
            (
                CheckoutError::NotInMergeConflict {
                    path: "a.txt".to_string(),
                },
                StableErrorCode::ConflictOperationBlocked,
            ),
            (
                CheckoutError::IndexReadFailed {
                    path: "a.txt".to_string(),
                    detail: "object missing".to_string(),
                },
                StableErrorCode::IoReadFailed,
            ),
            (
                CheckoutError::WorktreeWriteFailed {
                    path: "a.txt".to_string(),
                    detail: "disk full".to_string(),
                },
                StableErrorCode::IoWriteFailed,
            ),
        ];

        for (err, expected) in cases {
            let cli: CliError = err.into();
            assert_eq!(cli.stable_code(), expected);
        }
    }

    /// Pin the stable-code mapping for the path-mode / conflict / IO variants
    /// introduced for `--ours`/`--theirs`/`-f` (Batch 1) per the exit-code
    /// contract: usage → `CliInvalidArguments` (129), non-conflict path →
    /// `ConflictOperationBlocked` (128), read → `IoReadFailed` (128), write →
    /// `IoWriteFailed` (128).
    #[test]
    fn checkout_error_maps_new_variants_to_stable_codes() {
        let cases: Vec<(CheckoutError, StableErrorCode)> = vec![
            (
                CheckoutError::ConflictPathRequired { flag: "--ours" },
                StableErrorCode::CliInvalidArguments,
            ),
            (
                CheckoutError::InvalidPathMode("-b".to_string()),
                StableErrorCode::CliInvalidArguments,
            ),
            (
                CheckoutError::NotInMergeConflict {
                    path: "a.txt".to_string(),
                },
                StableErrorCode::ConflictOperationBlocked,
            ),
            (
                CheckoutError::IndexReadFailed {
                    path: "<index>".to_string(),
                    detail: "bad header".to_string(),
                },
                StableErrorCode::IoReadFailed,
            ),
            (
                CheckoutError::WorktreeWriteFailed {
                    path: "a.txt".to_string(),
                    detail: "permission denied".to_string(),
                },
                StableErrorCode::IoWriteFailed,
            ),
        ];
        for (err, expected) in cases {
            let cli: CliError = err.into();
            assert_eq!(cli.stable_code(), expected);
        }
    }

    #[test]
    fn checkout_remote_sync_failed_preserves_inner_stable_code() {
        let inner = CliError::fatal("upstream missing")
            .with_stable_code(StableErrorCode::NetworkUnavailable);
        let wrapped = CheckoutError::RemoteSyncFailed {
            stage: "set upstream",
            source: Box::new(inner),
        };
        let cli: CliError = wrapped.into();
        assert_eq!(cli.stable_code(), StableErrorCode::NetworkUnavailable);
        assert!(
            cli.message()
                .contains("failed to set upstream during remote branch checkout"),
            "got: {}",
            cli.message()
        );
        assert!(
            cli.message().contains("upstream missing"),
            "got: {}",
            cli.message()
        );
    }
}
