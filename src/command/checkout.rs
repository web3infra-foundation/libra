//! Handles checkout-style flows to show the current branch, switch to existing branches, or create and switch to a new one using restore utilities.

use clap::Parser;
use git_internal::hash::ObjectHash;

use crate::{
    command::{
        branch, pull,
        restore::{self, RestoreArgs},
        switch,
    },
    info_println,
    internal::{
        branch::{Branch, BranchStoreError, INTENT_BRANCH},
        head::Head,
    },
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::OutputConfig,
        util,
    },
};

const CHECKOUT_EXAMPLES: &str = "\
EXAMPLES:
    libra checkout                         Show the current branch
    libra checkout main                    Switch to an existing local branch
    libra checkout feature-x               Switch to another branch
    libra checkout -b feature-x            Create and switch to a new branch
    libra checkout --quiet main            Switch without informational stdout";

#[derive(Parser, Debug)]
#[command(after_help = CHECKOUT_EXAMPLES)]
pub struct CheckoutArgs {
    /// Target branch name
    branch: Option<String>,

    /// Create and switch to a new branch with the same content as the current branch
    #[clap(short = 'b', group = "sub")]
    new_branch: Option<String>,
}

pub async fn execute(args: CheckoutArgs) {
    if let Err(e) = execute_safe(args, &OutputConfig::default()).await {
        e.print_stderr();
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting. Validates arguments, checks for local changes, then
/// delegates to branch switching or creation via restore utilities.
pub async fn execute_safe(args: CheckoutArgs, output: &OutputConfig) -> CliResult<()> {
    if let Some(ref branch_name) = args.branch
        && branch_name == INTENT_BRANCH
    {
        return Err(CliError::fatal(format!(
            "checking out '{}' branch is not allowed",
            INTENT_BRANCH
        )));
    }
    if let Some(ref new_branch_name) = args.new_branch
        && new_branch_name == INTENT_BRANCH
    {
        return Err(CliError::fatal(format!(
            "creating/switching to '{}' branch is not allowed",
            INTENT_BRANCH
        )));
    }

    // Match Git behavior: checking out the current branch is a no-op and should
    // not be blocked by unrelated local changes.
    if let Some(ref target_branch) = args.branch
        && get_current_branch().await == Some(target_branch.clone())
    {
        info_println!(output, "Already on {target_branch}");
        return Ok(());
    }

    let target_commit = if let Some(ref branch_name) = args.branch {
        Branch::find_branch_result(branch_name, None)
            .await
            .map_err(|error| checkout_branch_store_error("resolve checkout target", error))?
            .map(|branch| branch.commit)
    } else {
        None
    };

    let clean_status = match target_commit {
        Some(target_commit) => switch::ensure_clean_status_for_commit(target_commit, output).await,
        None => switch::ensure_clean_status(output).await,
    };

    match clean_status {
        Ok(()) => {}
        Err(
            switch::SwitchError::DirtyUnstaged
            | switch::SwitchError::DirtyUncommitted
            | switch::SwitchError::UntrackedOverwrite(..),
        ) => {
            return Err(CliError::failure(
                "local changes would be overwritten by checkout",
            ));
        }
        Err(err) => return Err(CliError::from(err)),
    }

    match (args.branch, args.new_branch) {
        (Some(target_branch), _) => check_and_switch_branch(&target_branch, output).await?,
        (None, Some(new_branch)) => create_and_switch_new_branch(&new_branch, output).await?,
        (None, None) => show_current_branch(output).await,
    }
    Ok(())
}

fn checkout_branch_store_error(context: &str, error: BranchStoreError) -> CliError {
    match error {
        BranchStoreError::Query(detail) => {
            CliError::fatal(format!("failed to {context}: {detail}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        }
        other => CliError::fatal(format!("failed to {context}: {other}"))
            .with_stable_code(StableErrorCode::RepoCorrupt),
    }
}

pub async fn get_current_branch() -> Option<String> {
    match Head::current().await {
        Head::Detached(_) => None,
        Head::Branch(name) => Some(name),
    }
}

async fn show_current_branch(output: &OutputConfig) {
    match Head::current().await {
        Head::Detached(commit_hash) => {
            info_println!(output, "HEAD detached at {}", &commit_hash.to_string()[..8]);
        }
        Head::Branch(current_branch) => {
            info_println!(output, "Current branch is {current_branch}.");
        }
    }
}

pub async fn switch_branch(branch_name: &str) -> CliResult<()> {
    switch_branch_with_output(branch_name, &OutputConfig::default()).await
}

async fn switch_branch_with_output(branch_name: &str, output: &OutputConfig) -> CliResult<()> {
    if branch_name == INTENT_BRANCH {
        return Err(CliError::fatal(format!(
            "switching to '{}' branch is not allowed",
            INTENT_BRANCH
        )));
    }
    let target_branch = Branch::find_branch_result(branch_name, None)
        .await
        .map_err(|error| checkout_branch_store_error("resolve branch", error))?
        .ok_or_else(|| CliError::fatal(format!("branch '{}' not found", branch_name)))?;
    restore_to_commit(target_branch.commit, output).await?;
    let head = Head::Branch(branch_name.to_string());
    Head::update(head, None).await;
    Ok(())
}

async fn create_and_switch_new_branch(new_branch: &str, output: &OutputConfig) -> CliResult<()> {
    branch::create_branch_safe(new_branch.to_string(), get_current_branch().await).await?;
    switch_branch_with_output(new_branch, output).await?;
    info_println!(output, "Switched to a new branch '{new_branch}'");
    Ok(())
}

async fn get_remote(branch_name: &str, output: &OutputConfig) -> CliResult<()> {
    let remote_branch_name: String = format!("origin/{branch_name}");

    create_and_switch_new_branch(branch_name, output).await?;
    // Set branch upstream
    branch::set_upstream_safe_with_output(branch_name, &remote_branch_name, output).await?;
    // Synchronous branches
    // Use the pull command to update the local branch with the latest changes from the remote branch
    pull::execute_safe(pull::PullArgs::make(None, None), output).await?;
    Ok(())
}

/// Returns `Ok(Some(true))` if remote branch found, `Ok(Some(false))` if local branch found,
/// `Ok(None)` if already on the branch.
pub async fn check_branch(branch_name: &str) -> CliResult<Option<bool>> {
    check_branch_with_output(branch_name, &OutputConfig::default()).await
}

async fn check_branch_with_output(
    branch_name: &str,
    output: &OutputConfig,
) -> CliResult<Option<bool>> {
    if get_current_branch().await == Some(branch_name.to_string()) {
        info_println!(output, "Already on {branch_name}");
        return Ok(None);
    }

    let target_branch: Option<Branch> = Branch::find_branch_result(branch_name, None)
        .await
        .map_err(|error| checkout_branch_store_error("resolve branch", error))?;
    if target_branch.is_none() {
        let remote_branch_name: String = format!("origin/{branch_name}");
        if !Branch::search_branch_result(&remote_branch_name)
            .await
            .map_err(|error| checkout_branch_store_error("search remote tracking branches", error))?
            .is_empty()
        {
            info_println!(
                output,
                "branch '{branch_name}' set up to track '{remote_branch_name}'."
            );
            Ok(Some(true))
        } else {
            Err(CliError::fatal(format!(
                "path specification '{}' did not match any files known to libra",
                branch_name
            )))
        }
    } else {
        info_println!(output, "Switched to branch '{branch_name}'");
        Ok(Some(false))
    }
}

async fn check_and_switch_branch(branch_name: &str, output: &OutputConfig) -> CliResult<()> {
    match check_branch_with_output(branch_name, output).await? {
        Some(true) => get_remote(branch_name, output).await?,
        Some(false) => switch_branch_with_output(branch_name, output).await?,
        None => (),
    }
    Ok(())
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

/// Unit tests for the checkout module
#[cfg(test)]
mod tests {}
