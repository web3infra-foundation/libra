//! Switch command to change branches safely, validating clean state, handling creation, and delegating checkout behavior to restore logic.

use clap::Parser;
use git_internal::hash::{ObjectHash, get_hash_kind};

use super::{
    restore::{self, RestoreArgs},
    status,
};
use crate::{
    command::{branch, status::StatusArgs},
    internal::{
        branch::{Branch, INTENT_BRANCH},
        db::get_db_conn_instance,
        head::Head,
        reflog::{ReflogAction, ReflogContext, with_reflog},
    },
    utils::{
        error::{CliError, CliResult},
        util::{self, get_commit_base},
    },
};

#[derive(Parser, Debug)]
pub struct SwitchArgs {
    /// branch name
    #[clap(required_unless_present("create"), required_unless_present("detach"))]
    pub branch: Option<String>,

    /// Create a new branch based on the given branch or current HEAD, and switch to it
    #[clap(long, short, group = "sub")]
    pub create: Option<String>,

    /// Switch to a commit
    #[clap(long, short, action, default_value = "false", group = "sub")]
    pub detach: bool,

    #[clap(
        long,
        conflicts_with_all = ["create", "detach"],
        help = "Set upstream tracking when switching to remote branch"
    )]
    pub track: bool,
}

pub async fn execute(args: SwitchArgs) {
    if let Err(err) = execute_safe(args).await {
        eprintln!("{}", err.render());
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting. Validates clean working-tree state, then switches,
/// creates, or detaches HEAD to the requested branch.
pub async fn execute_safe(args: SwitchArgs) -> CliResult<()> {
    ensure_clean_status().await?;
    let SwitchArgs {
        branch,
        create,
        detach,
        track,
    } = args;

    if track {
        let target = match branch {
            Some(branch) => branch,
            None => {
                return Err(CliError::fatal("missing remote branch name"));
            }
        };
        return switch_to_tracked_remote_branch(target).await;
    }

    match create {
        Some(new_branch_name) => {
            if new_branch_name == INTENT_BRANCH {
                return Err(CliError::fatal(format!(
                    "creating/switching to '{}' branch is not allowed",
                    INTENT_BRANCH
                )));
            }
            branch::create_branch(new_branch_name.clone(), branch).await;
            switch_to_branch(new_branch_name).await
        }
        None => match detach {
            true => {
                let branch = branch.ok_or_else(|| {
                    CliError::command_usage("branch name is required when using --detach")
                })?;
                let commit_base = get_commit_base(&branch)
                    .await
                    .map_err(|e| CliError::fatal(e.to_string()))?;
                switch_to_commit(commit_base).await
            }
            false => {
                let branch =
                    branch.ok_or_else(|| CliError::command_usage("branch name is required"))?;
                switch_to_branch(branch).await
            }
        },
    }
}

/// Check status before changing branches and return a user-facing error on failure.
///
/// When uncommitted or unstaged changes are detected, this prints the current
/// status summary (via `status::execute`) and returns a descriptive
/// [`CliError`] so callers can decide how to surface the problem.
pub async fn ensure_clean_status() -> CliResult<()> {
    let unstaged = match status::changes_to_be_staged() {
        Ok(c) => c,
        Err(err) => {
            return Err(CliError::fatal(format!(
                "failed to determine working tree status: {err}"
            )));
        }
    };
    if !unstaged.deleted.is_empty() || !unstaged.modified.is_empty() {
        status::execute(StatusArgs::default()).await;
        Err(CliError::fatal("unstaged changes, can't switch branch"))
    } else if !status::changes_to_be_committed().await.is_empty() {
        status::execute(StatusArgs::default()).await;
        Err(CliError::fatal("uncommitted changes, can't switch branch"))
    } else {
        Ok(())
    }
}

async fn switch_to_tracked_remote_branch(target: String) -> CliResult<()> {
    let (remote_name, remote_branch_name) = if let Some(rest) = target.strip_prefix("refs/remotes/")
    {
        match rest.split_once('/') {
            Some((remote_name, remote_branch_name)) => {
                (remote_name.to_string(), remote_branch_name.to_string())
            }
            None => {
                return Err(CliError::fatal(format!("invalid remote branch '{target}'")));
            }
        }
    } else if let Some((remote_name, remote_branch_name)) = target.split_once('/') {
        (remote_name.to_string(), remote_branch_name.to_string())
    } else {
        ("origin".to_string(), target)
    };

    if remote_branch_name == "intent" {
        return Err(CliError::fatal(
            "switching to 'intent' branch is not allowed",
        ));
    }

    let remote_tracking_ref = format!("refs/remotes/{remote_name}/{remote_branch_name}");

    let remote_tracking_branch = match Branch::find_branch(&remote_tracking_ref, None).await {
        Some(branch) => branch,
        None => {
            return Err(CliError::fatal(format!(
                "remote branch '{remote_name}/{remote_branch_name}' not found"
            ))
            .with_hint(format!(
                "Run 'libra fetch {remote_name}' to update remote-tracking branches."
            )));
        }
    };

    if Branch::find_branch(&remote_branch_name, None)
        .await
        .is_some()
    {
        return Err(CliError::fatal(format!(
            "a branch named '{remote_branch_name}' already exists"
        )));
    }

    Branch::update_branch(
        &remote_branch_name,
        &remote_tracking_branch.commit.to_string(),
        None,
    )
    .await;
    branch::set_upstream(
        &remote_branch_name,
        &format!("{remote_name}/{remote_branch_name}"),
    )
    .await;
    switch_to_branch(remote_branch_name).await
}

/// change the working directory to the version of commit_hash
async fn switch_to_commit(commit_hash: ObjectHash) -> CliResult<()> {
    let db = get_db_conn_instance().await;

    let old_oid = Head::current_commit_with_conn(db)
        .await
        .map(|oid| oid.to_string())
        .unwrap_or_else(|| ObjectHash::zero_str(get_hash_kind()).to_string());

    let from_ref_name = match Head::current_with_conn(db).await {
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
    };

    if let Err(e) = with_reflog(
        context,
        move |txn: &sea_orm::DatabaseTransaction| {
            Box::pin(async move {
                let new_head = Head::Detached(commit_hash);
                Head::update_with_conn(txn, new_head, None).await;
                Ok(())
            })
        },
        false,
    )
    .await
    {
        return Err(CliError::fatal(e.to_string()));
    };

    // Only restore the working directory *after* HEAD has been successfully updated.
    restore_to_commit(commit_hash).await?;
    println!("HEAD is now at {}", &commit_hash.to_string()[..7]);
    Ok(())
}

async fn switch_to_branch(branch_name: String) -> CliResult<()> {
    if branch_name == "intent" {
        return Err(CliError::fatal(
            "switching to 'intent' branch is not allowed",
        ));
    }
    let db = get_db_conn_instance().await;

    let target_branch = match Branch::find_branch_with_conn(db, &branch_name, None).await {
        Some(b) => b,
        None => {
            if !Branch::search_branch(&branch_name).await.is_empty() {
                return Err(CliError::fatal(format!(
                    "a branch is expected, got remote branch {branch_name}"
                )));
            } else {
                return Err(
                    CliError::fatal(format!("invalid reference: {}", &branch_name))
                        .with_hint(format!("create it with 'libra switch -c {}'.", branch_name)),
                );
            }
        }
    };
    let target_commit_id = target_branch.commit;

    let old_oid = Head::current_commit_with_conn(db)
        .await
        .map(|oid| oid.to_string())
        .unwrap_or_else(|| ObjectHash::zero_str(get_hash_kind()).to_string());

    let from_ref_name = match Head::current_with_conn(db).await {
        Head::Branch(name) => name,
        Head::Detached(hash) => hash.to_string()[..7].to_string(),
    };

    if from_ref_name == branch_name {
        println!("Already on '{branch_name}'");
        return Ok(());
    }

    let action = ReflogAction::Switch {
        from: from_ref_name,
        to: branch_name.clone(),
    };
    let context = ReflogContext {
        old_oid,
        new_oid: target_commit_id.to_string(),
        action,
    };

    // `log_for_branch` is `false`. This is the key insight for `switch`/`checkout`.
    if let Err(e) = with_reflog(
        context,
        move |txn: &sea_orm::DatabaseTransaction| {
            Box::pin(async move {
                let new_head = Head::Branch(branch_name.clone());
                Head::update_with_conn(txn, new_head, None).await;
                Ok(())
            })
        },
        false,
    )
    .await
    {
        return Err(CliError::fatal(e.to_string()));
    }

    restore_to_commit(target_commit_id).await?;
    println!("Switched to branch '{}'", target_branch.name);
    Ok(())
}

async fn restore_to_commit(commit_id: ObjectHash) -> CliResult<()> {
    let restore_args = RestoreArgs {
        worktree: true,
        staged: true,
        source: Some(commit_id.to_string()),
        pathspec: vec![util::working_dir_string()],
    };
    restore::execute_safe(restore_args).await
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::command::restore::RestoreArgs;
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
}
