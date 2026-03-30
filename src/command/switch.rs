//! Switch command to change branches safely, validating clean state, handling creation, and delegating checkout behavior to restore logic.

use clap::Parser;
use git_internal::hash::{ObjectHash, get_hash_kind};
use serde::Serialize;

use super::{
    restore::{self, RestoreArgs},
    status,
};
use crate::{
    command::{branch, status::StatusArgs},
    internal::{
        branch::{self as repo_branch, Branch},
        db::get_db_conn_instance,
        head::Head,
        reflog::{ReflogAction, ReflogContext, with_reflog},
    },
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
        util::{self, get_commit_base},
    },
};

fn is_internal_switch_target(name: &str) -> bool {
    name == repo_branch::INTENT_BRANCH
}

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

pub async fn execute(args: SwitchArgs) {
    if let Err(err) = execute_safe(args, &OutputConfig::default()).await {
        err.print_stderr();
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting. Validates clean working-tree state, then switches,
/// creates, or detaches HEAD to the requested branch.
pub async fn execute_safe(args: SwitchArgs, output: &OutputConfig) -> CliResult<()> {
    let result = run_switch(args, output).await?;
    render_switch_output(&result, output)
}

async fn run_switch(args: SwitchArgs, output: &OutputConfig) -> CliResult<SwitchOutput> {
    ensure_clean_status(output).await?;
    let SwitchArgs {
        branch,
        create,
        detach,
        track,
    } = args;
    let (previous_branch, previous_commit) = current_switch_state().await;

    if track {
        let target = match branch {
            Some(branch) => branch,
            None => {
                return Err(CliError::command_usage("remote branch name is required")
                    .with_stable_code(StableErrorCode::CliInvalidArguments)
                    .with_hint("provide a remote branch name, for example 'origin/main'."));
            }
        };
        let tracked = switch_to_tracked_remote_branch(target, output).await?;
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

    match create {
        Some(new_branch_name) => {
            if repo_branch::is_locked_branch(&new_branch_name) {
                return Err(CliError::fatal(format!(
                    "creating/switching to '{}' branch is not allowed",
                    new_branch_name
                ))
                .with_stable_code(StableErrorCode::CliInvalidTarget));
            }
            branch::create_branch_safe(new_branch_name.clone(), branch).await?;
            let commit = switch_to_branch(new_branch_name.clone(), output).await?;
            Ok(SwitchOutput {
                previous_branch,
                previous_commit,
                branch: Some(new_branch_name),
                commit: commit.to_string(),
                created: true,
                detached: false,
                already_on: false,
                tracking: None,
            })
        }
        None => match detach {
            true => {
                let branch = branch.ok_or_else(|| {
                    CliError::command_usage("branch name is required when using --detach")
                        .with_stable_code(StableErrorCode::CliInvalidArguments)
                })?;
                let commit_base = get_commit_base(&branch).await.map_err(|e| {
                    CliError::fatal(e.to_string())
                        .with_stable_code(StableErrorCode::CliInvalidTarget)
                })?;
                let commit = switch_to_commit(commit_base, output).await?;
                Ok(SwitchOutput {
                    previous_branch,
                    previous_commit,
                    branch: None,
                    commit: commit.to_string(),
                    created: false,
                    detached: true,
                    already_on: false,
                    tracking: None,
                })
            }
            false => {
                let branch = branch.ok_or_else(|| {
                    CliError::command_usage("branch name is required")
                        .with_stable_code(StableErrorCode::CliInvalidArguments)
                })?;
                let commit = switch_to_branch(branch.clone(), output).await?;
                let already_on = previous_branch.as_deref() == Some(&branch);
                Ok(SwitchOutput {
                    previous_branch,
                    previous_commit,
                    branch: Some(branch),
                    commit: commit.to_string(),
                    created: false,
                    detached: false,
                    already_on,
                    tracking: None,
                })
            }
        },
    }
}

/// Check status before changing branches and return a user-facing error on failure.
///
/// When uncommitted or unstaged changes are detected, this prints the current
/// status summary (via `status::execute`) and returns a descriptive
/// [`CliError`] so callers can decide how to surface the problem.
pub async fn ensure_clean_status(output: &OutputConfig) -> CliResult<()> {
    let unstaged = match status::changes_to_be_staged() {
        Ok(c) => c,
        Err(err) => {
            return Err(
                CliError::fatal(format!("failed to determine working tree status: {err}"))
                    .with_stable_code(StableErrorCode::IoReadFailed),
            );
        }
    };
    if !unstaged.deleted.is_empty() || !unstaged.modified.is_empty() {
        if !output.quiet && !output.is_json() {
            status::execute(StatusArgs::default()).await;
        }
        Err(CliError::fatal("unstaged changes, can't switch branch")
            .with_stable_code(StableErrorCode::RepoStateInvalid)
            .with_hint("commit or stash your changes before switching."))
    } else {
        let staged = match status::changes_to_be_committed_safe().await {
            Ok(c) => c,
            Err(err) => {
                return Err(CliError::fatal(format!(
                    "failed to determine working tree status: {err}"
                ))
                .with_stable_code(StableErrorCode::IoReadFailed));
            }
        };
        if !staged.is_empty() {
            if !output.quiet && !output.is_json() {
                status::execute(StatusArgs::default()).await;
            }
            Err(CliError::fatal("uncommitted changes, can't switch branch")
                .with_stable_code(StableErrorCode::RepoStateInvalid)
                .with_hint("commit or stash your changes before switching."))
        } else {
            Ok(())
        }
    }
}

struct TrackedSwitchResult {
    remote: String,
    remote_branch: String,
    local_branch: String,
    commit: ObjectHash,
}

async fn switch_to_tracked_remote_branch(
    target: String,
    output: &OutputConfig,
) -> CliResult<TrackedSwitchResult> {
    let (remote_name, remote_branch_name) = if let Some(rest) = target.strip_prefix("refs/remotes/")
    {
        match rest.split_once('/') {
            Some((remote_name, remote_branch_name)) => {
                (remote_name.to_string(), remote_branch_name.to_string())
            }
            None => {
                return Err(CliError::fatal(format!("invalid remote branch '{target}'"))
                    .with_stable_code(StableErrorCode::CliInvalidTarget)
                    .with_hint("expected format: 'remote/branch'."));
            }
        }
    } else if let Some((remote_name, remote_branch_name)) = target.split_once('/') {
        (remote_name.to_string(), remote_branch_name.to_string())
    } else {
        ("origin".to_string(), target)
    };

    if is_internal_switch_target(&remote_branch_name) {
        return Err(CliError::fatal(format!(
            "switching to '{}' branch is not allowed",
            remote_branch_name
        ))
        .with_stable_code(StableErrorCode::CliInvalidTarget));
    }

    let remote_tracking_ref = format!("refs/remotes/{remote_name}/{remote_branch_name}");

    let remote_tracking_branch = match Branch::find_branch(&remote_tracking_ref, None).await {
        Some(branch) => branch,
        None => {
            return Err(CliError::fatal(format!(
                "remote branch '{remote_name}/{remote_branch_name}' not found"
            ))
            .with_stable_code(StableErrorCode::CliInvalidTarget)
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
        ))
        .with_stable_code(StableErrorCode::RepoStateInvalid));
    }

    Branch::update_branch(
        &remote_branch_name,
        &remote_tracking_branch.commit.to_string(),
        None,
    )
    .await
    .map_err(|e| {
        CliError::fatal(format!(
            "failed to create branch '{remote_branch_name}': {e}"
        ))
        .with_stable_code(StableErrorCode::IoWriteFailed)
    })?;
    branch::set_upstream_safe_with_output(
        &remote_branch_name,
        &format!("{remote_name}/{remote_branch_name}"),
        output,
    )
    .await?;
    let commit = switch_to_branch(remote_branch_name.clone(), output).await?;
    Ok(TrackedSwitchResult {
        remote: remote_name,
        remote_branch: remote_branch_name.clone(),
        local_branch: remote_branch_name,
        commit,
    })
}

/// change the working directory to the version of commit_hash
async fn switch_to_commit(commit_hash: ObjectHash, output: &OutputConfig) -> CliResult<ObjectHash> {
    let db = get_db_conn_instance().await;

    let old_oid = Head::current_commit_with_conn(&db)
        .await
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
        return Err(CliError::fatal(e.to_string()).with_stable_code(StableErrorCode::IoWriteFailed));
    };

    // Only restore the working directory *after* HEAD has been successfully updated.
    restore_to_commit(commit_hash, output).await?;
    Ok(commit_hash)
}

async fn switch_to_branch(branch_name: String, output: &OutputConfig) -> CliResult<ObjectHash> {
    if is_internal_switch_target(&branch_name) {
        return Err(CliError::fatal(format!(
            "switching to '{}' branch is not allowed",
            branch_name
        ))
        .with_stable_code(StableErrorCode::CliInvalidTarget));
    }
    let db = get_db_conn_instance().await;

    let target_branch = match Branch::find_branch_with_conn(&db, &branch_name, None).await {
        Some(b) => b,
        None => {
            if !Branch::search_branch(&branch_name).await.is_empty() {
                return Err(CliError::fatal(format!(
                    "a branch is expected, got remote branch {branch_name}"
                ))
                .with_stable_code(StableErrorCode::CliInvalidTarget));
            } else {
                return Err(
                    CliError::fatal(format!("invalid reference: {}", &branch_name))
                        .with_stable_code(StableErrorCode::CliInvalidTarget)
                        .with_hint(format!("create it with 'libra switch -c {}'.", branch_name)),
                );
            }
        }
    };
    let target_commit_id = target_branch.commit;

    let old_oid = Head::current_commit_with_conn(&db)
        .await
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
        return Err(CliError::fatal(e.to_string()).with_stable_code(StableErrorCode::IoWriteFailed));
    }

    restore_to_commit(target_commit_id, output).await?;
    Ok(target_commit_id)
}

async fn restore_to_commit(commit_id: ObjectHash, output: &OutputConfig) -> CliResult<()> {
    let restore_args = RestoreArgs {
        worktree: true,
        staged: true,
        source: Some(commit_id.to_string()),
        pathspec: vec![util::working_dir_string()],
    };
    restore::execute_safe(restore_args, output).await
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

    Ok(())
}

async fn current_switch_state() -> (Option<String>, Option<String>) {
    let branch = match Head::current().await {
        Head::Branch(name) => Some(name),
        Head::Detached(_) => None,
    };
    let commit = Head::current_commit().await.map(|hash| hash.to_string());
    (branch, commit)
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
