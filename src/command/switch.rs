//! Switch command to change branches safely, validating clean state, handling creation, and delegating checkout behavior to restore logic.

use std::{fs, path::PathBuf};

use clap::Parser;
use git_internal::{
    hash::{ObjectHash, get_hash_kind},
    internal::index::Index,
};
use serde::Serialize;

use super::{
    reset,
    restore::{self, RestoreArgs},
    status,
};
use crate::{
    command::{branch, load_object, status::StatusArgs},
    internal::{
        ai::automation::{VCS_EVENT_POST_SWITCH, dispatch_current_repo_vcs_event_to_history},
        branch::{self as repo_branch, Branch},
        config::{ConfigKv, parse_config_bool},
        db::get_db_conn_instance,
        head::Head,
        reflog::{ReflogAction, ReflogContext, with_reflog},
    },
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
        path,
        text::levenshtein,
        util,
        util::get_commit_base,
        worktree,
    },
};

fn is_internal_switch_target(name: &str) -> bool {
    repo_branch::is_ai_managed_branch(name)
}

const SWITCH_EXAMPLES: &str = "\
EXAMPLES:
    libra switch main                      Switch to an existing branch
    libra switch -c feature-x              Create and switch to a new branch
    libra switch -c fix-123 abc1234        Create branch from specific commit
    libra switch -C feature-x main         Create or reset feature-x to main, then switch
    libra switch -f main                   Discard tracked changes while switching
    libra switch --discard-changes main    Same as --force
    libra switch --orphan fresh-start      Create an unborn branch with no history
    libra switch --guess feature           Create local branch from a unique remote match
    libra switch --detach v1.0             Detach HEAD at a tag
    libra switch --track origin/main       Track and switch to remote branch
    libra switch --json main               Structured JSON output for agents";

#[derive(Parser, Debug, Clone, Default)]
#[command(after_help = SWITCH_EXAMPLES)]
pub struct SwitchArgs {
    /// Target branch, commit, or remote-tracking ref to switch to (e.g. `main`, `abc1234`, `origin/main`)
    pub branch: Option<String>,

    /// Create a new branch based on the given branch or current HEAD, and switch to it
    #[clap(long, short, group = "sub")]
    pub create: Option<String>,

    /// Force-create a branch and switch to it: create it, or reset it to the
    /// start point if it already exists (Git's `-C`).
    #[clap(long = "force-create", short = 'C', group = "sub")]
    pub force_create: Option<String>,

    /// Create a new unborn branch whose first commit will have no parents.
    #[clap(long = "orphan", value_name = "branch", group = "sub")]
    pub orphan: Option<String>,

    /// Switch to a commit
    #[clap(long, short, action, default_value = "false", group = "sub")]
    pub detach: bool,

    /// Discard tracked working tree changes while switching.
    #[clap(short = 'f', long = "force", visible_alias = "discard-changes", action)]
    pub force: bool,

    /// Guess a local branch from a unique remote-tracking branch.
    #[clap(long, action, conflicts_with = "no_guess")]
    pub guess: bool,

    /// Disable remote-tracking branch guessing.
    #[clap(long = "no-guess", action, conflicts_with = "guess")]
    pub no_guess: bool,

    #[clap(
        long,
        conflicts_with_all = ["create", "force_create", "detach", "orphan"],
        help = "Set upstream tracking when switching to remote branch"
    )]
    pub track: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SwitchTrackingInfo {
    pub remote: String,
    pub remote_branch: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SwitchOutput {
    pub previous_branch: Option<String>,
    pub previous_commit: Option<String>,
    pub branch: Option<String>,
    pub commit: String,
    pub created: bool,
    pub detached: bool,
    /// True when the target branch equals the current branch (no-op switch).
    pub already_on: bool,
    pub tracking: Option<SwitchTrackingInfo>,
}

// ---------------------------------------------------------------------------
// Structured error types
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum SwitchError {
    #[error("remote branch name is required")]
    MissingTrackTarget,

    #[error("branch name is required when using --detach")]
    MissingDetachTarget,

    #[error("branch name is required")]
    MissingBranchName,

    #[error("branch '{name}' not found")]
    BranchNotFound { name: String, similar: Vec<String> },

    #[error("a branch is expected, got remote branch '{0}'")]
    GotRemoteBranch(String),

    #[error("remote branch '{remote}/{branch}' not found")]
    RemoteBranchNotFound { remote: String, branch: String },

    #[error("invalid remote branch '{0}'")]
    InvalidRemoteBranch(String),

    #[error("a branch named '{0}' already exists")]
    BranchAlreadyExists(String),

    #[error("branch '{branch}' matched multiple remote branches: {candidates}")]
    GuessAmbiguous { branch: String, candidates: String },

    #[error("'{0}' is a reserved branch name")]
    InternalBranchBlocked(String),

    #[error("unstaged changes, can't switch branch")]
    DirtyUnstaged,

    #[error("uncommitted changes, can't switch branch")]
    DirtyUncommitted,

    #[error("untracked working tree file would be overwritten by switch: {0}")]
    UntrackedOverwrite(String),

    #[error("failed to determine working tree status: {0}")]
    StatusCheck(String),

    #[error("failed to resolve commit: {0}")]
    CommitResolve(String),

    #[error("failed to create branch '{branch}': {detail}")]
    BranchCreate { branch: String, detail: String },

    #[error("failed to update HEAD: {0}")]
    HeadUpdate(String),

    #[error("failed to create orphan branch '{branch}': {detail}")]
    OrphanFailed { branch: String, detail: String },

    #[error(transparent)]
    DelegatedCli(#[from] CliError),
}

impl From<SwitchError> for CliError {
    fn from(error: SwitchError) -> Self {
        match error {
            SwitchError::MissingTrackTarget => CliError::command_usage(error.to_string())
                .with_stable_code(StableErrorCode::CliInvalidArguments)
                .with_hint("provide a remote branch name, for example 'origin/main'."),
            SwitchError::MissingDetachTarget => CliError::command_usage(error.to_string())
                .with_stable_code(StableErrorCode::CliInvalidArguments)
                .with_hint("provide a commit, tag, or branch to detach at."),
            SwitchError::MissingBranchName => CliError::command_usage(error.to_string())
                .with_stable_code(StableErrorCode::CliInvalidArguments)
                .with_hint("provide a branch name."),
            SwitchError::BranchNotFound {
                ref name,
                ref similar,
            } => {
                let mut err = CliError::fatal(error.to_string())
                    .with_stable_code(StableErrorCode::CliInvalidTarget)
                    .with_hint(format!("create it with 'libra switch -c {}'.", name));
                for s in similar {
                    err = err.with_hint(format!("did you mean '{}'?", s));
                }
                err
            }
            SwitchError::GotRemoteBranch(ref name) => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::CliInvalidTarget)
                .with_hint(format!(
                    "use 'libra switch --track {}' to create a local tracking branch.",
                    name
                )),
            SwitchError::RemoteBranchNotFound { ref remote, .. } => {
                CliError::fatal(error.to_string())
                    .with_stable_code(StableErrorCode::CliInvalidTarget)
                    .with_hint(format!(
                        "Run 'libra fetch {}' to update remote-tracking branches.",
                        remote
                    ))
            }
            SwitchError::InvalidRemoteBranch(..) => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::CliInvalidTarget)
                .with_hint("expected format: 'remote/branch'."),
            SwitchError::BranchAlreadyExists(ref name) => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::ConflictOperationBlocked)
                .with_hint(format!(
                    "use 'libra switch {}' if you meant the existing local branch.",
                    name
                )),
            SwitchError::GuessAmbiguous { .. } => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::CliInvalidTarget)
                .with_exit_code(128)
                .with_hint(
                    "set checkout.defaultRemote or pass --no-guess and use --track explicitly.",
                ),
            SwitchError::InternalBranchBlocked(..) => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::CliInvalidTarget),
            SwitchError::DirtyUnstaged => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::RepoStateInvalid)
                .with_hint("commit or stash your changes before switching."),
            SwitchError::DirtyUncommitted => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::RepoStateInvalid)
                .with_hint("commit or stash your changes before switching."),
            SwitchError::UntrackedOverwrite(..) => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::ConflictOperationBlocked)
                .with_hint("move or remove it before switching."),
            SwitchError::StatusCheck(..) => {
                CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::IoReadFailed)
            }
            SwitchError::CommitResolve(..) => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::CliInvalidTarget)
                .with_hint("check the revision name and try again."),
            SwitchError::BranchCreate { .. } => {
                CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::IoWriteFailed)
            }
            SwitchError::HeadUpdate(..) => {
                CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::IoWriteFailed)
            }
            SwitchError::OrphanFailed { .. } => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::IoWriteFailed)
                .with_hint("inspect the worktree, then retry or restore files manually."),
            SwitchError::DelegatedCli(cli_err) => cli_err,
        }
    }
}

fn map_branch_store_error(error: repo_branch::BranchStoreError) -> SwitchError {
    match error {
        repo_branch::BranchStoreError::Query(detail) => SwitchError::DelegatedCli(
            CliError::fatal(format!("failed to read branch storage: {detail}"))
                .with_stable_code(StableErrorCode::IoReadFailed),
        ),
        repo_branch::BranchStoreError::Corrupt { .. } => {
            repo_corrupt_switch_error(error.to_string())
        }
        repo_branch::BranchStoreError::NotFound(name) => SwitchError::DelegatedCli(
            CliError::fatal(format!("branch '{name}' not found"))
                .with_stable_code(StableErrorCode::CliInvalidTarget),
        ),
        repo_branch::BranchStoreError::Delete { name, detail } => SwitchError::DelegatedCli(
            CliError::fatal(format!("failed to delete branch '{name}': {detail}"))
                .with_stable_code(StableErrorCode::IoWriteFailed),
        ),
    }
}

fn invalid_branch_name_error(branch_name: &str) -> CliError {
    CliError::fatal(format!("'{}' is not a valid branch name", branch_name))
        .with_stable_code(StableErrorCode::CliInvalidArguments)
}

fn existing_branch_conflict_error(branch_name: &str) -> CliError {
    CliError::fatal(format!("a branch named '{}' already exists", branch_name))
        .with_stable_code(StableErrorCode::ConflictOperationBlocked)
}

fn invalid_branch_base_error(target: &str) -> CliError {
    CliError::fatal(format!("not a valid object name: '{}'", target))
        .with_stable_code(StableErrorCode::CliInvalidTarget)
}

fn switch_config_read_error(scope: &str, error: impl ToString) -> SwitchError {
    SwitchError::DelegatedCli(
        CliError::fatal(format!("failed to {scope}: {}", error.to_string()))
            .with_stable_code(StableErrorCode::IoReadFailed),
    )
}

async fn read_checkout_guess_config() -> Result<bool, SwitchError> {
    let Some(entry) = ConfigKv::get("checkout.guess")
        .await
        .map_err(|error| switch_config_read_error("read checkout.guess", error))?
    else {
        return Ok(true);
    };

    parse_config_bool(&entry.value).ok_or_else(|| {
        SwitchError::DelegatedCli(
            CliError::fatal(format!(
                "invalid checkout.guess '{}': expected boolean",
                entry.value
            ))
            .with_stable_code(StableErrorCode::CliInvalidArguments)
            .with_hint("use true, false, yes, no, on, off, 1, or 0."),
        )
    })
}

async fn effective_guess(guess: bool, no_guess: bool) -> Result<bool, SwitchError> {
    if no_guess {
        return Ok(false);
    }
    if guess {
        return Ok(true);
    }
    read_checkout_guess_config().await
}

async fn validate_new_branch_request(
    new_branch_name: &str,
    branch_or_commit: Option<&str>,
) -> Result<(), SwitchError> {
    if !branch::is_valid_git_branch_name(new_branch_name) {
        return Err(SwitchError::DelegatedCli(invalid_branch_name_error(
            new_branch_name,
        )));
    }
    if repo_branch::is_locked_branch(new_branch_name) {
        return Err(SwitchError::InternalBranchBlocked(
            new_branch_name.to_string(),
        ));
    }
    if Branch::find_branch_result(new_branch_name, None)
        .await
        .map_err(map_branch_store_error)?
        .is_some()
    {
        return Err(SwitchError::DelegatedCli(existing_branch_conflict_error(
            new_branch_name,
        )));
    }
    if let Some(target) = branch_or_commit {
        get_commit_base(target)
            .await
            .map_err(|_| SwitchError::DelegatedCli(invalid_branch_base_error(target)))?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct ResolvedSwitchBranch {
    name: String,
    commit: ObjectHash,
}

#[derive(Debug, Clone)]
struct ResolvedTrackedRemoteTarget {
    remote: String,
    remote_branch: String,
    commit: ObjectHash,
}

#[derive(Debug, Clone)]
struct GuessedRemoteCandidate {
    remote: String,
    remote_branch: String,
    commit: ObjectHash,
}

enum ResolvedSwitchTarget {
    Local(ResolvedSwitchBranch),
    Guessed(ResolvedTrackedRemoteTarget),
}

fn find_similar_branch_names(branch_name: &str, branches: &[Branch]) -> Vec<String> {
    let target_len = branch_name.chars().count();
    let mut best: Option<(usize, String)> = None;

    for branch in branches {
        if branch.name.chars().count().abs_diff(target_len) > 2 {
            continue;
        }

        let distance = levenshtein(&branch.name, branch_name);
        if distance > 2 {
            continue;
        }

        match &mut best {
            Some((best_distance, best_name))
                if distance < *best_distance
                    || (distance == *best_distance && branch.name < *best_name) =>
            {
                *best_distance = distance;
                *best_name = branch.name.clone();
            }
            None => best = Some((distance, branch.name.clone())),
            _ => {}
        }
    }

    best.into_iter().map(|(_, name)| name).collect()
}

async fn resolve_switch_branch_target(
    branch_name: &str,
    guess_enabled: bool,
) -> Result<ResolvedSwitchTarget, SwitchError> {
    if is_internal_switch_target(branch_name) {
        return Err(SwitchError::InternalBranchBlocked(branch_name.to_string()));
    }
    if let Some(branch) = Branch::find_branch_result(branch_name, None)
        .await
        .map_err(map_branch_store_error)?
    {
        return Ok(ResolvedSwitchTarget::Local(ResolvedSwitchBranch {
            name: branch.name,
            commit: branch.commit,
        }));
    }
    if !Branch::search_branch_result(branch_name)
        .await
        .map_err(map_branch_store_error)?
        .is_empty()
    {
        return Err(SwitchError::GotRemoteBranch(branch_name.to_string()));
    }

    if guess_enabled
        && !branch_name.contains('/')
        && let Some(guessed) = resolve_guessed_remote_target(branch_name).await?
    {
        return Ok(ResolvedSwitchTarget::Guessed(guessed));
    }

    let all_branches = Branch::list_branches_result(None)
        .await
        .map_err(map_branch_store_error)?;
    let similar = find_similar_branch_names(branch_name, &all_branches);
    Err(SwitchError::BranchNotFound {
        name: branch_name.to_string(),
        similar,
    })
}

fn remote_branch_short_name(remote: &str, branch_name: &str) -> String {
    branch_name
        .strip_prefix(&format!("refs/remotes/{remote}/"))
        .unwrap_or(branch_name)
        .to_string()
}

fn dedupe_remote_candidates(
    candidates: Vec<GuessedRemoteCandidate>,
) -> Vec<GuessedRemoteCandidate> {
    let mut deduped = Vec::new();
    for candidate in candidates {
        let exists = deduped.iter().any(|existing: &GuessedRemoteCandidate| {
            existing.remote == candidate.remote
                && existing.remote_branch == candidate.remote_branch
                && existing.commit == candidate.commit
        });
        if !exists {
            deduped.push(candidate);
        }
    }
    deduped.sort_by(|left, right| {
        left.remote
            .cmp(&right.remote)
            .then(left.remote_branch.cmp(&right.remote_branch))
    });
    deduped
}

async fn remote_guess_candidates(
    branch_name: &str,
) -> Result<Vec<GuessedRemoteCandidate>, SwitchError> {
    let remotes = ConfigKv::all_remote_configs()
        .await
        .map_err(|error| switch_config_read_error("read remote config", error))?;
    let mut candidates = Vec::new();
    for remote in remotes {
        let branches = Branch::list_branches_result(Some(&remote.name))
            .await
            .map_err(map_branch_store_error)?;
        for branch in branches {
            let short_name = remote_branch_short_name(&remote.name, &branch.name);
            if short_name == branch_name {
                candidates.push(GuessedRemoteCandidate {
                    remote: remote.name.clone(),
                    remote_branch: short_name,
                    commit: branch.commit,
                });
            }
        }
    }

    let local_full_refs = Branch::list_branches_result(None)
        .await
        .map_err(map_branch_store_error)?;
    for branch in local_full_refs {
        let Some(rest) = branch.name.strip_prefix("refs/remotes/") else {
            continue;
        };
        let Some((remote, remote_branch)) = rest.split_once('/') else {
            continue;
        };
        if remote_branch == branch_name {
            candidates.push(GuessedRemoteCandidate {
                remote: remote.to_string(),
                remote_branch: remote_branch.to_string(),
                commit: branch.commit,
            });
        }
    }

    Ok(dedupe_remote_candidates(candidates))
}

async fn read_checkout_default_remote() -> Result<Option<String>, SwitchError> {
    ConfigKv::get("checkout.defaultRemote")
        .await
        .map(|entry| {
            entry
                .map(|entry| entry.value)
                .filter(|value| !value.is_empty())
        })
        .map_err(|error| switch_config_read_error("read checkout.defaultRemote", error))
}

async fn resolve_guessed_remote_target(
    branch_name: &str,
) -> Result<Option<ResolvedTrackedRemoteTarget>, SwitchError> {
    let candidates = remote_guess_candidates(branch_name).await?;
    if candidates.is_empty() {
        return Ok(None);
    }

    let selected = if let Some(default_remote) = read_checkout_default_remote().await? {
        candidates
            .iter()
            .find(|candidate| candidate.remote == default_remote)
            .cloned()
    } else if candidates.len() == 1 {
        candidates.first().cloned()
    } else {
        None
    };

    if let Some(candidate) = selected {
        return Ok(Some(ResolvedTrackedRemoteTarget {
            remote: candidate.remote,
            remote_branch: candidate.remote_branch,
            commit: candidate.commit,
        }));
    }

    let candidate_names = candidates
        .iter()
        .map(|candidate| format!("{}/{}", candidate.remote, candidate.remote_branch))
        .collect::<Vec<_>>()
        .join(", ");
    Err(SwitchError::GuessAmbiguous {
        branch: branch_name.to_string(),
        candidates: candidate_names,
    })
}

fn parse_remote_switch_target(target: &str) -> Result<(String, String), SwitchError> {
    if let Some(rest) = target.strip_prefix("refs/remotes/") {
        return match rest.split_once('/') {
            Some((remote_name, remote_branch_name)) => {
                Ok((remote_name.to_string(), remote_branch_name.to_string()))
            }
            None => Err(SwitchError::InvalidRemoteBranch(target.to_string())),
        };
    }
    if let Some((remote_name, remote_branch_name)) = target.split_once('/') {
        return Ok((remote_name.to_string(), remote_branch_name.to_string()));
    }
    Ok(("origin".to_string(), target.to_string()))
}

async fn resolve_tracked_remote_target(
    target: &str,
) -> Result<ResolvedTrackedRemoteTarget, SwitchError> {
    let (remote_name, remote_branch_name) = parse_remote_switch_target(target)?;

    if is_internal_switch_target(&remote_branch_name) {
        return Err(SwitchError::InternalBranchBlocked(remote_branch_name));
    }

    let remote_tracking_ref = format!("refs/remotes/{remote_name}/{remote_branch_name}");
    let remote_tracking_branch = if let Some(branch) =
        Branch::find_branch_result(&remote_tracking_ref, Some(&remote_name))
            .await
            .map_err(map_branch_store_error)?
    {
        Some(branch)
    } else if let Some(branch) = Branch::find_branch_result(&remote_tracking_ref, None)
        .await
        .map_err(map_branch_store_error)?
    {
        Some(branch)
    } else {
        Branch::find_branch_result(&remote_branch_name, Some(&remote_name))
            .await
            .map_err(map_branch_store_error)?
    }
    .ok_or_else(|| SwitchError::RemoteBranchNotFound {
        remote: remote_name.clone(),
        branch: remote_branch_name.clone(),
    })?;
    if Branch::find_branch_result(&remote_branch_name, None)
        .await
        .map_err(map_branch_store_error)?
        .is_some()
    {
        return Err(SwitchError::BranchAlreadyExists(remote_branch_name));
    }
    Ok(ResolvedTrackedRemoteTarget {
        remote: remote_name,
        remote_branch: remote_branch_name,
        commit: remote_tracking_branch.commit,
    })
}

fn internal_switch_invariant(message: impl Into<String>) -> SwitchError {
    SwitchError::DelegatedCli(
        CliError::fatal(message.into()).with_stable_code(StableErrorCode::InternalInvariant),
    )
}

async fn resolve_created_branch(branch_name: &str) -> Result<ResolvedSwitchBranch, SwitchError> {
    let branch = Branch::find_branch_result(branch_name, None)
        .await
        .map_err(map_branch_store_error)?
        .ok_or_else(|| {
            internal_switch_invariant(format!(
                "failed to resolve newly created branch '{}'",
                branch_name
            ))
        })?;

    Ok(ResolvedSwitchBranch {
        name: branch.name,
        commit: branch.commit,
    })
}

async fn resolve_create_switch_target(
    branch_or_commit: Option<&str>,
) -> Result<Option<ObjectHash>, SwitchError> {
    match branch_or_commit {
        Some(target) => get_commit_base(target)
            .await
            .map(Some)
            .map_err(|_| SwitchError::DelegatedCli(invalid_branch_base_error(target))),
        None => Head::current_commit_result()
            .await
            .map_err(map_branch_store_error),
    }
}

fn repo_corrupt_switch_error(message: impl Into<String>) -> SwitchError {
    SwitchError::DelegatedCli(
        CliError::fatal(message.into()).with_stable_code(StableErrorCode::RepoCorrupt),
    )
}

fn target_index_for_commit(commit_id: &ObjectHash) -> Result<Index, SwitchError> {
    let commit: git_internal::internal::object::commit::Commit =
        load_object(commit_id).map_err(|e| {
            repo_corrupt_switch_error(format!("failed to inspect target commit {commit_id}: {e}"))
        })?;
    let tree: git_internal::internal::object::tree::Tree =
        load_object(&commit.tree_id).map_err(|e| {
            repo_corrupt_switch_error(format!(
                "failed to inspect target tree {}: {e}",
                commit.tree_id
            ))
        })?;

    let mut index = Index::new();
    reset::rebuild_index_from_tree(&tree, &mut index, "").map_err(|e| {
        repo_corrupt_switch_error(format!("failed to inspect target tree state: {e}"))
    })?;
    Ok(index)
}

fn ensure_no_untracked_overwrite(target_commit: ObjectHash) -> Result<(), SwitchError> {
    let current_index =
        Index::load(path::index()).map_err(|err| SwitchError::StatusCheck(err.to_string()))?;
    let untracked_paths =
        worktree::untracked_workdir_paths(&current_index).map_err(SwitchError::StatusCheck)?;
    let target_index = target_index_for_commit(&target_commit)?;

    if let Some(conflict) = worktree::untracked_overwrite_path(&untracked_paths, &target_index) {
        return Err(SwitchError::UntrackedOverwrite(
            conflict.display().to_string(),
        ));
    }

    Ok(())
}

async fn validate_orphan_branch_request(branch_name: &str) -> Result<(), SwitchError> {
    if !branch::is_valid_git_branch_name(branch_name) {
        return Err(SwitchError::DelegatedCli(invalid_branch_name_error(
            branch_name,
        )));
    }
    if Branch::exists_result(branch_name, None)
        .await
        .map_err(map_branch_store_error)?
    {
        return Err(SwitchError::BranchAlreadyExists(branch_name.to_string()));
    }
    if repo_branch::is_locked_branch(branch_name) {
        return Err(SwitchError::InternalBranchBlocked(branch_name.to_string()));
    }
    Ok(())
}

fn current_tracked_worktree_paths(branch_name: &str) -> Result<Vec<PathBuf>, SwitchError> {
    let index_path = path::index();
    let index = Index::load(&index_path).map_err(|error| SwitchError::OrphanFailed {
        branch: branch_name.to_string(),
        detail: format!("failed to read index: {error}"),
    })?;
    Ok(index.tracked_files())
}

fn save_empty_index(branch_name: &str) -> Result<(), SwitchError> {
    Index::new()
        .save(path::index())
        .map_err(|error| SwitchError::OrphanFailed {
            branch: branch_name.to_string(),
            detail: format!("failed to clear index: {error}"),
        })
}

fn remove_tracked_worktree_files(
    branch_name: &str,
    tracked_paths: &[PathBuf],
) -> Result<(), SwitchError> {
    let workdir = util::try_working_dir().map_err(|error| SwitchError::OrphanFailed {
        branch: branch_name.to_string(),
        detail: format!("failed to resolve worktree: {error}"),
    })?;
    for path in tracked_paths {
        let absolute = workdir.join(path);
        let Ok(metadata) = fs::symlink_metadata(&absolute) else {
            continue;
        };
        let result = if metadata.is_dir() {
            fs::remove_dir_all(&absolute)
        } else {
            fs::remove_file(&absolute)
        };
        result.map_err(|error| SwitchError::OrphanFailed {
            branch: branch_name.to_string(),
            detail: format!("failed to remove '{}': {error}", path.display()),
        })?;
    }
    Ok(())
}

async fn switch_to_orphan_branch(
    branch_name: &str,
    previous_branch: Option<String>,
    previous_commit: Option<String>,
) -> Result<SwitchOutput, SwitchError> {
    let tracked_paths = current_tracked_worktree_paths(branch_name)?;
    Head::update_result(Head::Branch(branch_name.to_string()), None)
        .await
        .map_err(|error| SwitchError::HeadUpdate(error.to_string()))?;
    save_empty_index(branch_name)?;
    remove_tracked_worktree_files(branch_name, &tracked_paths)?;

    Ok(SwitchOutput {
        previous_branch,
        previous_commit,
        branch: Some(branch_name.to_string()),
        commit: String::new(),
        created: true,
        detached: false,
        already_on: false,
        tracking: None,
    })
}

pub async fn execute(args: SwitchArgs) {
    if let Err(err) = execute_safe(args, &OutputConfig::default()).await {
        err.print_stderr();
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting. When a branch or commit change will occur, validates a
/// clean working-tree state before switching, creating, or detaching HEAD.
/// No-op "already on" cases return before the cleanliness check.
pub async fn execute_safe(args: SwitchArgs, output: &OutputConfig) -> CliResult<()> {
    let result = run_switch(args, output).await.map_err(CliError::from)?;
    render_switch_output(&result, output)?;
    if !result.already_on {
        dispatch_current_repo_vcs_event_to_history(VCS_EVENT_POST_SWITCH).await;
    }
    Ok(())
}

async fn run_switch(args: SwitchArgs, output: &OutputConfig) -> Result<SwitchOutput, SwitchError> {
    let SwitchArgs {
        branch,
        create,
        force_create,
        orphan,
        detach,
        force,
        guess,
        no_guess,
        track,
    } = args;
    let (previous_branch, previous_commit) = current_switch_state().await;

    if track {
        let target = branch.ok_or(SwitchError::MissingTrackTarget)?;
        let tracked_target = resolve_tracked_remote_target(&target).await?;
        ensure_clean_status_for_commit_with_force(tracked_target.commit, output, force).await?;

        let tracked = switch_to_tracked_remote_branch(tracked_target, output).await?;
        return Ok(SwitchOutput {
            previous_branch,
            previous_commit,
            branch: Some(tracked.local_branch),
            commit: tracked.commit.to_string(),
            created: true,
            detached: false,
            already_on: false,
            tracking: Some(SwitchTrackingInfo {
                remote: tracked.remote,
                remote_branch: tracked.remote_branch,
            }),
        });
    }

    if let Some(new_branch_name) = create {
        validate_new_branch_request(&new_branch_name, branch.as_deref()).await?;
        match resolve_create_switch_target(branch.as_deref()).await? {
            Some(target_commit) => {
                ensure_clean_status_for_commit_with_force(target_commit, output, force).await?
            }
            None => ensure_clean_status_with_force(output, force).await?,
        }

        branch::create_branch_safe(new_branch_name.clone(), branch).await?;
        let created_branch = resolve_created_branch(&new_branch_name).await?;
        let commit = switch_to_resolved_branch(created_branch, output).await?;
        return Ok(SwitchOutput {
            previous_branch,
            previous_commit,
            branch: Some(new_branch_name),
            commit: commit.to_string(),
            created: true,
            detached: false,
            already_on: false,
            tracking: None,
        });
    }

    // `-C`/`--force-create`: like `-c` but resets the branch to the start point
    // if it already exists, rather than refusing. Locked branches stay refused.
    if let Some(new_branch_name) = force_create {
        if !branch::is_valid_git_branch_name(&new_branch_name) {
            return Err(SwitchError::DelegatedCli(invalid_branch_name_error(
                &new_branch_name,
            )));
        }
        if repo_branch::is_locked_branch(&new_branch_name) {
            return Err(SwitchError::InternalBranchBlocked(new_branch_name.clone()));
        }
        if let Some(target) = branch.as_deref() {
            get_commit_base(target)
                .await
                .map_err(|_| SwitchError::DelegatedCli(invalid_branch_base_error(target)))?;
        }
        match resolve_create_switch_target(branch.as_deref()).await? {
            Some(target_commit) => {
                ensure_clean_status_for_commit_with_force(target_commit, output, force).await?
            }
            None => ensure_clean_status_with_force(output, force).await?,
        }

        branch::create_branch_force(new_branch_name.clone(), branch).await?;
        let created_branch = resolve_created_branch(&new_branch_name).await?;
        let commit = switch_to_resolved_branch(created_branch, output).await?;
        return Ok(SwitchOutput {
            previous_branch,
            previous_commit,
            branch: Some(new_branch_name),
            commit: commit.to_string(),
            created: true,
            detached: false,
            already_on: false,
            tracking: None,
        });
    }

    if let Some(orphan_branch) = orphan {
        validate_orphan_branch_request(&orphan_branch).await?;
        if !force {
            ensure_clean_status(output).await?;
        }
        let output =
            switch_to_orphan_branch(&orphan_branch, previous_branch, previous_commit).await?;
        return Ok(output);
    }

    if detach {
        let target = branch.ok_or(SwitchError::MissingDetachTarget)?;
        let commit_base = get_commit_base(&target)
            .await
            .map_err(|e| SwitchError::CommitResolve(e.to_string()))?;
        ensure_clean_status_for_commit_with_force(commit_base, output, force).await?;

        let commit = switch_to_commit(commit_base, output).await?;
        return Ok(SwitchOutput {
            previous_branch,
            previous_commit,
            branch: None,
            commit: commit.to_string(),
            created: false,
            detached: true,
            already_on: false,
            tracking: None,
        });
    }

    let branch = branch.ok_or(SwitchError::MissingBranchName)?;
    let guess_enabled = effective_guess(guess, no_guess).await?;
    let target_branch = resolve_switch_branch_target(&branch, guess_enabled).await?;
    let target_branch = match target_branch {
        ResolvedSwitchTarget::Local(target_branch) => target_branch,
        ResolvedSwitchTarget::Guessed(tracked_target) => {
            ensure_clean_status_for_commit_with_force(tracked_target.commit, output, force).await?;
            let tracked = switch_to_tracked_remote_branch(tracked_target, output).await?;
            return Ok(SwitchOutput {
                previous_branch,
                previous_commit,
                branch: Some(tracked.local_branch),
                commit: tracked.commit.to_string(),
                created: true,
                detached: false,
                already_on: false,
                tracking: Some(SwitchTrackingInfo {
                    remote: tracked.remote,
                    remote_branch: tracked.remote_branch,
                }),
            });
        }
    };
    if previous_branch.as_deref() == Some(&branch) {
        return Ok(SwitchOutput {
            previous_branch,
            previous_commit: previous_commit.clone(),
            branch: Some(branch),
            commit: target_branch.commit.to_string(),
            created: false,
            detached: false,
            already_on: true,
            tracking: None,
        });
    }

    ensure_clean_status_for_commit_with_force(target_branch.commit, output, force).await?;

    let commit = switch_to_resolved_branch(target_branch, output).await?;
    Ok(SwitchOutput {
        previous_branch,
        previous_commit,
        branch: Some(branch),
        commit: commit.to_string(),
        created: false,
        detached: false,
        already_on: false,
        tracking: None,
    })
}

/// Check status before changing branches and return a typed error on failure.
///
/// When uncommitted or unstaged changes are detected, this prints the current
/// status summary (via `status::execute`) unless the caller requested quiet or
/// structured output, then returns the corresponding [`SwitchError`] variant.
pub async fn ensure_clean_status(output: &OutputConfig) -> Result<(), SwitchError> {
    ensure_clean_status_internal(None, output).await
}

async fn ensure_clean_status_with_force(
    output: &OutputConfig,
    force: bool,
) -> Result<(), SwitchError> {
    ensure_clean_status_internal_with_force(None, output, force).await
}

/// Like [`ensure_clean_status`], but also rejects untracked files that the
/// target commit would overwrite during the branch/commit change.
pub async fn ensure_clean_status_for_commit(
    target_commit: ObjectHash,
    output: &OutputConfig,
) -> Result<(), SwitchError> {
    ensure_clean_status_internal(Some(target_commit), output).await
}

async fn ensure_clean_status_for_commit_with_force(
    target_commit: ObjectHash,
    output: &OutputConfig,
    force: bool,
) -> Result<(), SwitchError> {
    ensure_clean_status_internal_with_force(Some(target_commit), output, force).await
}

async fn ensure_clean_status_internal(
    target_commit: Option<ObjectHash>,
    output: &OutputConfig,
) -> Result<(), SwitchError> {
    ensure_clean_status_internal_with_force(target_commit, output, false).await
}

async fn ensure_clean_status_internal_with_force(
    target_commit: Option<ObjectHash>,
    output: &OutputConfig,
    force: bool,
) -> Result<(), SwitchError> {
    let unstaged = match status::changes_to_be_staged() {
        Ok(c) => c,
        Err(err) => {
            return Err(SwitchError::StatusCheck(err.to_string()));
        }
    };
    if !force && (!unstaged.deleted.is_empty() || !unstaged.modified.is_empty()) {
        if !output.quiet && !output.is_json() {
            status::execute(StatusArgs::default()).await;
        }
        return Err(SwitchError::DirtyUnstaged);
    }

    let staged = match status::changes_to_be_committed_safe().await {
        Ok(c) => c,
        Err(err) => {
            return Err(SwitchError::StatusCheck(err.to_string()));
        }
    };
    if !force && !staged.is_empty() {
        if !output.quiet && !output.is_json() {
            status::execute(StatusArgs::default()).await;
        }
        return Err(SwitchError::DirtyUncommitted);
    }

    if let Some(target_commit) = target_commit {
        ensure_no_untracked_overwrite(target_commit)?;
    }

    Ok(())
}

struct TrackedSwitchResult {
    remote: String,
    remote_branch: String,
    local_branch: String,
    commit: ObjectHash,
}

async fn switch_to_tracked_remote_branch(
    target: ResolvedTrackedRemoteTarget,
    output: &OutputConfig,
) -> Result<TrackedSwitchResult, SwitchError> {
    let local_branch = target.remote_branch.clone();
    let db = get_db_conn_instance().await;
    let old_oid = Head::current_commit_result_with_conn(&db)
        .await
        .map_err(map_branch_store_error)?
        .map(|oid| oid.to_string())
        .unwrap_or_else(|| ObjectHash::zero_str(get_hash_kind()).to_string());
    let from_ref_name = match Head::current_with_conn(&db).await {
        Head::Branch(name) => name,
        Head::Detached(hash) => hash.to_string()[..7].to_string(),
    };
    let upstream = format!("{}/{}", target.remote, local_branch);
    let context = ReflogContext {
        old_oid,
        new_oid: target.commit.to_string(),
        action: ReflogAction::Switch {
            from: from_ref_name,
            to: local_branch.clone(),
        },
        message: None,
    };

    if let Err(e) = with_reflog(
        context,
        {
            let local_branch = local_branch.clone();
            let upstream = upstream.clone();
            move |txn: &sea_orm::DatabaseTransaction| {
                Box::pin(async move {
                    let commit = target.commit.to_string();
                    Branch::update_branch_with_conn(txn, &local_branch, &commit, None).await?;
                    branch::set_upstream_with_conn(txn, &local_branch, &upstream)
                        .await
                        .map_err(|error| sea_orm::DbErr::Custom(error.to_string()))?;
                    Head::update_result_with_conn(txn, Head::Branch(local_branch), None)
                        .await
                        .map_err(|error| sea_orm::DbErr::Custom(error.to_string()))?;
                    Ok(())
                })
            }
        },
        false,
    )
    .await
    {
        return Err(SwitchError::BranchCreate {
            branch: local_branch.clone(),
            detail: e.to_string(),
        });
    }

    restore_to_commit(target.commit, output).await?;
    Ok(TrackedSwitchResult {
        remote: target.remote,
        remote_branch: target.remote_branch,
        local_branch,
        commit: target.commit,
    })
}

/// change the working directory to the version of commit_hash
async fn switch_to_commit(
    commit_hash: ObjectHash,
    output: &OutputConfig,
) -> Result<ObjectHash, SwitchError> {
    let db = get_db_conn_instance().await;

    let old_oid = Head::current_commit_result_with_conn(&db)
        .await
        .map_err(map_branch_store_error)?
        .map(|oid| oid.to_string())
        .unwrap_or_else(|| ObjectHash::zero_str(get_hash_kind()).to_string());

    let from_ref_name = match Head::current_with_conn(&db).await {
        Head::Branch(name) => name,
        Head::Detached(hash) => hash.to_string()[..7].to_string(), // Use short hash for detached HEAD
    };

    let action = ReflogAction::Switch {
        from: from_ref_name,
        to: commit_hash.to_string()[..7].to_string(), // Use short hash for target commit
    };
    let context = ReflogContext {
        old_oid,
        new_oid: commit_hash.to_string(),
        action,
        message: None,
    };

    if let Err(e) = with_reflog(
        context,
        move |txn: &sea_orm::DatabaseTransaction| {
            Box::pin(async move {
                let new_head = Head::Detached(commit_hash);
                Head::update_result_with_conn(txn, new_head, None)
                    .await
                    .map_err(|error| sea_orm::DbErr::Custom(error.to_string()))?;
                Ok(())
            })
        },
        false,
    )
    .await
    {
        return Err(SwitchError::HeadUpdate(e.to_string()));
    };

    // Only restore the working directory *after* HEAD has been successfully updated.
    restore_to_commit(commit_hash, output).await?;
    Ok(commit_hash)
}

async fn switch_to_resolved_branch(
    target_branch: ResolvedSwitchBranch,
    output: &OutputConfig,
) -> Result<ObjectHash, SwitchError> {
    let ResolvedSwitchBranch {
        name: branch_name,
        commit: target_commit_id,
    } = target_branch;
    let db = get_db_conn_instance().await;

    let old_oid = Head::current_commit_result_with_conn(&db)
        .await
        .map_err(map_branch_store_error)?
        .map(|oid| oid.to_string())
        .unwrap_or_else(|| ObjectHash::zero_str(get_hash_kind()).to_string());

    let from_ref_name = match Head::current_with_conn(&db).await {
        Head::Branch(name) => name,
        Head::Detached(hash) => hash.to_string()[..7].to_string(),
    };

    if from_ref_name == branch_name {
        // No-op: already on the target branch. The "Already on" message is
        // rendered by render_switch_output() based on the `already_on` flag,
        // so we must not emit anything here (it would corrupt --json stdout).
        return Ok(target_commit_id);
    }

    let action = ReflogAction::Switch {
        from: from_ref_name,
        to: branch_name.clone(),
    };
    let context = ReflogContext {
        old_oid,
        new_oid: target_commit_id.to_string(),
        action,
        message: None,
    };

    // `log_for_branch` is `false`. This is the key insight for `switch`/`checkout`.
    if let Err(e) = with_reflog(
        context,
        move |txn: &sea_orm::DatabaseTransaction| {
            Box::pin(async move {
                let new_head = Head::Branch(branch_name.clone());
                Head::update_result_with_conn(txn, new_head, None)
                    .await
                    .map_err(|error| sea_orm::DbErr::Custom(error.to_string()))?;
                Ok(())
            })
        },
        false,
    )
    .await
    {
        return Err(SwitchError::HeadUpdate(e.to_string()));
    }

    restore_to_commit(target_commit_id, output).await?;
    Ok(target_commit_id)
}

async fn restore_to_commit(
    commit_id: ObjectHash,
    output: &OutputConfig,
) -> Result<(), SwitchError> {
    let restore_args = RestoreArgs {
        worktree: true,
        staged: true,
        source: Some(commit_id.to_string()),
        pathspec: vec![util::working_dir_string()],
        ..Default::default()
    };
    restore::execute_safe(restore_args, &output.child_output_config()).await?;
    Ok(())
}

fn render_switch_output(result: &SwitchOutput, output: &OutputConfig) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("switch", result, output);
    }

    if output.quiet {
        return Ok(());
    }

    if result.already_on {
        if let Some(branch) = &result.branch {
            println!("Already on '{}'", branch);
        }
    } else if result.detached {
        println!("HEAD is now at {}", &result.commit[..7]);
    } else if result.created {
        println!(
            "Switched to a new branch '{}'",
            result.branch.as_deref().unwrap_or_default()
        );
    } else if let Some(branch) = &result.branch {
        println!("Switched to branch '{}'", branch);
    }

    if let (Some(branch), Some(tracking)) = (&result.branch, &result.tracking) {
        println!(
            "Branch '{}' set up to track remote branch '{}/{}'",
            branch, tracking.remote, tracking.remote_branch
        );
    }

    Ok(())
}

async fn current_switch_state() -> (Option<String>, Option<String>) {
    let branch = match Head::current().await {
        Head::Branch(name) => Some(name),
        Head::Detached(_) => None,
    };
    let commit = match Head::current_commit_result().await {
        Ok(commit) => commit.map(|hash| hash.to_string()),
        Err(error) => {
            tracing::error!("failed to resolve current switch state: {error}");
            None
        }
    };
    (branch, commit)
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::command::restore::RestoreArgs;

    /// Pin the `Display` format for the static-message and direct-message
    /// variants of [`SwitchError`]. These strings are used directly as
    /// the `CliError` message via `From<SwitchError> for CliError` and
    /// surface in both human and `--json` envelopes.
    ///
    /// Source-chained variants (StatusCheck, CommitResolve, BranchCreate,
    /// HeadUpdate, DelegatedCli) wrap upstream error messages and are
    /// intentionally skipped — their `{0}` slot is owned by the wrapped
    /// type.
    #[test]
    fn switch_error_display_pins_static_message_variants() {
        assert_eq!(
            SwitchError::MissingTrackTarget.to_string(),
            "remote branch name is required",
        );
        assert_eq!(
            SwitchError::MissingDetachTarget.to_string(),
            "branch name is required when using --detach",
        );
        assert_eq!(
            SwitchError::MissingBranchName.to_string(),
            "branch name is required",
        );
        assert_eq!(
            SwitchError::BranchNotFound {
                name: "topic/x".to_string(),
                similar: vec![],
            }
            .to_string(),
            "branch 'topic/x' not found",
        );
        assert_eq!(
            SwitchError::GotRemoteBranch("origin/main".to_string()).to_string(),
            "a branch is expected, got remote branch 'origin/main'",
        );
        assert_eq!(
            SwitchError::RemoteBranchNotFound {
                remote: "origin".to_string(),
                branch: "feature".to_string(),
            }
            .to_string(),
            "remote branch 'origin/feature' not found",
        );
        assert_eq!(
            SwitchError::InvalidRemoteBranch("garbage".to_string()).to_string(),
            "invalid remote branch 'garbage'",
        );
        assert_eq!(
            SwitchError::BranchAlreadyExists("main".to_string()).to_string(),
            "a branch named 'main' already exists",
        );
        assert_eq!(
            SwitchError::InternalBranchBlocked("intent".to_string()).to_string(),
            "'intent' is a reserved branch name",
        );
        assert_eq!(
            SwitchError::DirtyUnstaged.to_string(),
            "unstaged changes, can't switch branch",
        );
        assert_eq!(
            SwitchError::DirtyUncommitted.to_string(),
            "uncommitted changes, can't switch branch",
        );
        assert_eq!(
            SwitchError::UntrackedOverwrite("scratch.txt".to_string()).to_string(),
            "untracked working tree file would be overwritten by switch: scratch.txt",
        );
    }

    #[test]
    /// Test parsing RestoreArgs from command-line style arguments
    fn test_parse_from() {
        let commit_id = ObjectHash::from_str("0cb5eb6281e1c0df48a70716869686c694706189").unwrap();
        let restore_args = RestoreArgs::parse_from([
            "restore", // important, the first will be ignored
            "--worktree",
            "--staged",
            "--source",
            &commit_id.to_string(),
            "./",
        ]);
        println!("{restore_args:?}");
    }

    #[test]
    fn levenshtein_handles_basic_edge_cases() {
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("abc", ""), 3);
        assert_eq!(levenshtein("main", "main"), 0);
        assert_eq!(levenshtein("main", "maim"), 1);
        assert_eq!(levenshtein("feature", "featur"), 1);
    }
}
