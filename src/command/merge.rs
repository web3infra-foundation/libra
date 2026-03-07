//! Merge command orchestration that resolves base/target commits, performs recursive merge, stages results, and updates refs or surfaces conflicts.

use clap::Parser;
use git_internal::{
    hash::{ObjectHash, get_hash_kind},
    internal::object::commit::Commit,
};

use super::{
    get_target_commit, load_object, log,
    restore::{self, RestoreArgs},
};
use crate::{
    internal::{
        branch::Branch,
        db::get_db_conn_instance,
        head::Head,
        reflog::{ReflogAction, ReflogContext, with_reflog},
    },
    utils::{
        error::{CliError, CliResult},
        util,
    },
};

#[derive(Parser, Debug)]
pub struct MergeArgs {
    /// The branch to merge into the current branch, could be remote branch
    pub branch: String,
}

pub async fn execute(args: MergeArgs) {
    if let Err(err) = execute_safe(args).await {
        eprintln!("{}", err.render());
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting. Resolves the merge target, performs fast-forward or
/// recursive merge, stages results, and updates refs.
pub async fn execute_safe(args: MergeArgs) -> CliResult<()> {
    let commit_hash = get_target_commit(&args.branch)
        .await
        .map_err(|_| CliError::failure(format!("{} - not something we can merge", args.branch)))?;
    let target_commit: Commit = load_object(&commit_hash)
        .map_err(|_| CliError::fatal(format!("not a valid object name: '{}'", commit_hash)))?;

    // Handle the case where merging into an empty branch or merging with remote when no local commits exist
    // If the current HEAD doesn't point to any commit, perform a fast-forward merge directly
    let current_commit_id = Head::current_commit().await;
    if current_commit_id.is_none() {
        return merge_ff(target_commit, &args.branch).await;
    }

    // INVARIANT: `current_commit_id` is `Some` — the `None` case returns above.
    let current_commit_id = current_commit_id.unwrap();
    let current_commit: Commit = load_object(&current_commit_id).map_err(|_| {
        CliError::fatal(format!("not a valid object name: '{}'", current_commit_id))
    })?;

    let lca = lca_commit(&current_commit, &target_commit).await?;

    let lca = lca.ok_or_else(|| CliError::fatal("refusing to merge unrelated histories"))?;

    if lca.id == target_commit.id {
        // no need to merge
        println!("Already up to date.");
        Ok(())
    } else if lca.id == current_commit.id {
        println!(
            "Updating {}..{}",
            &current_commit.id.to_string()[..6],
            &target_commit.id.to_string()[..6]
        );
        // fast-forward merge
        merge_ff(target_commit, &args.branch).await
    } else {
        // didn't support yet
        Err(CliError::fatal(
            "Not possible to fast-forward merge, try merge manually",
        ))
    }
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

/// try merge in fast-forward mode, if it's not possible, do nothing
async fn merge_ff(target_commit: Commit, target_branch_name: &str) -> CliResult<()> {
    println!("Fast-forward");
    let db = get_db_conn_instance().await;

    let old_oid_opt = Head::current_commit_with_conn(db).await;
    let current_head_state = Head::current_with_conn(db).await;

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
                        .await;
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
        return Err(CliError::fatal(e.to_string()));
    }

    // Only restore the working directory *after* the pointers have been updated.
    restore::execute_safe(RestoreArgs {
        worktree: true,
        staged: true,
        source: None, // `restore` without source defaults to HEAD, which is now correct.
        pathspec: vec![util::working_dir_string()],
    })
    .await?;
    Ok(())
}
