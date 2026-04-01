//! Merge command orchestration that resolves base/target commits, performs recursive merge, stages results, and updates refs or surfaces conflicts.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use clap::Parser;
use git_internal::{
    hash::{ObjectHash, get_hash_kind},
    internal::object::{commit::Commit, tree::Tree},
};

use super::{
    get_target_commit, load_object, log,
    restore::{self, RestoreArgs},
};
use crate::{
    internal::{
        branch::{Branch, BranchStoreError},
        db::get_db_conn_instance,
        head::Head,
        reflog::{ReflogAction, ReflogContext, with_reflog},
    },
    utils::{
        error::{CliError, CliResult},
        object_ext::TreeExt,
        output::OutputConfig,
        util,
    },
};

#[derive(Parser, Debug)]
pub struct MergeArgs {
    /// The branch to merge into the current branch, could be remote branch
    pub branch: String,
}

#[derive(Debug, Clone)]
pub(crate) struct PullMergeSummary {
    pub strategy: String,
    /// The previous HEAD commit before merge (None for root commits).
    pub old_commit: Option<String>,
    pub commit: Option<String>,
    pub files_changed: usize,
    pub up_to_date: bool,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum PullMergeError {
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
    #[error("non-fast-forward merge from '{upstream}' requires manual merge")]
    ManualMergeRequired { upstream: String },
    #[error("failed to load tree '{tree_id}': {detail}")]
    TreeLoad { tree_id: String, detail: String },
    #[error("failed to update HEAD during merge: {0}")]
    HeadUpdate(String),
    #[error("failed to restore working tree after merge: {0}")]
    Restore(String),
}

impl From<PullMergeError> for CliError {
    fn from(error: PullMergeError) -> Self {
        match &error {
            PullMergeError::InvalidTarget(..) => CliError::command_usage(error.to_string())
                .with_stable_code(crate::utils::error::StableErrorCode::CliInvalidTarget),
            PullMergeError::TargetLoad { .. }
            | PullMergeError::CurrentLoad { .. }
            | PullMergeError::History(..)
            | PullMergeError::TreeLoad { .. } => CliError::fatal(error.to_string())
                .with_stable_code(crate::utils::error::StableErrorCode::RepoCorrupt),
            PullMergeError::UnrelatedHistories => CliError::failure(error.to_string())
                .with_stable_code(crate::utils::error::StableErrorCode::RepoStateInvalid),
            PullMergeError::ManualMergeRequired { .. } => CliError::failure(error.to_string())
                .with_stable_code(crate::utils::error::StableErrorCode::ConflictOperationBlocked),
            PullMergeError::HeadUpdate(..) | PullMergeError::Restore(..) => {
                CliError::fatal(error.to_string())
                    .with_stable_code(crate::utils::error::StableErrorCode::IoWriteFailed)
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
/// errors and exiting. Resolves the merge target, performs fast-forward or
/// recursive merge, stages results, and updates refs.
pub async fn execute_safe(args: MergeArgs, output: &OutputConfig) -> CliResult<()> {
    let result =
        match run_merge_for_pull(&args.branch, &args.branch, output).await {
            Ok(result) => result,
            Err(PullMergeError::ManualMergeRequired { .. }) => {
                return Err(CliError::fatal(
                    "Not possible to fast-forward merge, try merge manually",
                )
                .with_stable_code(crate::utils::error::StableErrorCode::ConflictOperationBlocked));
            }
            Err(error) => return Err(CliError::from(error)),
        };
    if result.up_to_date {
        crate::info_println!(output, "Already up to date.");
    } else {
        crate::info_println!(output, "Fast-forward");
    }
    Ok(())
}

pub(crate) async fn run_merge_for_pull(
    target_ref: &str,
    upstream: &str,
    output: &OutputConfig,
) -> Result<PullMergeSummary, PullMergeError> {
    let commit_hash = resolve_merge_target(target_ref)
        .await
        .map_err(|_| PullMergeError::InvalidTarget(upstream.to_string()))?;
    let target_commit: Commit =
        load_object(&commit_hash).map_err(|error| PullMergeError::TargetLoad {
            commit_id: commit_hash.to_string(),
            detail: error.to_string(),
        })?;

    let current_commit_id = Head::current_commit().await;
    if current_commit_id.is_none() {
        let files_changed = count_changed_files(None, &target_commit)?;
        apply_fast_forward_merge(target_commit.clone(), upstream, output).await?;
        return Ok(PullMergeSummary {
            strategy: "fast-forward".to_string(),
            old_commit: None,
            commit: Some(target_commit.id.to_string()),
            files_changed,
            up_to_date: false,
        });
    }

    let current_commit_id = match current_commit_id {
        Some(commit_id) => commit_id,
        None => unreachable!("checked above"),
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
        });
    }

    if lca.id == current_commit.id {
        let files_changed = count_changed_files(Some(&current_commit), &target_commit)?;
        apply_fast_forward_merge(target_commit.clone(), upstream, output).await?;
        return Ok(PullMergeSummary {
            strategy: "fast-forward".to_string(),
            old_commit: Some(current_commit_id.to_string()),
            commit: Some(target_commit.id.to_string()),
            files_changed,
            up_to_date: false,
        });
    }

    Err(PullMergeError::ManualMergeRequired {
        upstream: upstream.to_string(),
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
    let db = get_db_conn_instance().await;

    let old_oid_opt = Head::current_commit_with_conn(&db).await;
    let current_head_state = Head::current_with_conn(&db).await;

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
        output,
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

fn commit_tree_items(commit: &Commit) -> Result<HashMap<PathBuf, ObjectHash>, PullMergeError> {
    let tree: Tree = load_object(&commit.tree_id).map_err(|error| PullMergeError::TreeLoad {
        tree_id: commit.tree_id.to_string(),
        detail: error.to_string(),
    })?;
    Ok(tree.get_plain_items().into_iter().collect())
}
