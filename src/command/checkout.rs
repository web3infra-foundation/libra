//! Handles checkout-style flows to show the current branch, switch to existing branches, or create and switch to a new one using restore utilities.

use clap::Parser;
use git_internal::hash::ObjectHash;

use crate::{
    command::{
        branch, pull,
        restore::{self, RestoreArgs},
        switch,
    },
    internal::{
        branch::{Branch, INTENT_BRANCH},
        head::Head,
    },
    utils::{
        error::{CliError, CliResult},
        util,
    },
};

#[derive(Parser, Debug)]
pub struct CheckoutArgs {
    /// Target branche name
    branch: Option<String>,

    /// Create and switch to a new branch with the same content as the current branch
    #[clap(short = 'b', group = "sub")]
    new_branch: Option<String>,
}

pub async fn execute(args: CheckoutArgs) {
    if let Err(e) = execute_safe(args).await {
        eprintln!("{}", e.render());
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting. Validates arguments, checks for local changes, then
/// delegates to branch switching or creation via restore utilities.
pub async fn execute_safe(args: CheckoutArgs) -> CliResult<()> {
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
        println!("Already on {target_branch}");
        return Ok(());
    }

    switch::ensure_clean_status()
        .await
        .map_err(|_| CliError::failure("local changes would be overwritten by checkout"))?;

    match (args.branch, args.new_branch) {
        (Some(target_branch), _) => check_and_switch_branch(&target_branch).await?,
        (None, Some(new_branch)) => create_and_switch_new_branch(&new_branch).await?,
        (None, None) => show_current_branch().await,
    }
    Ok(())
}

pub async fn get_current_branch() -> Option<String> {
    let head = Head::current().await;
    match head {
        Head::Detached(commit_hash) => {
            println!("HEAD detached at {}", &commit_hash.to_string()[..8]);
            None
        }
        Head::Branch(name) => Some(name),
    }
}

async fn show_current_branch() {
    if let Some(current_branch) = get_current_branch().await {
        println!("Current branch is {current_branch}.");
    }
}

pub async fn switch_branch(branch_name: &str) -> CliResult<()> {
    if branch_name == INTENT_BRANCH {
        return Err(CliError::fatal(format!(
            "switching to '{}' branch is not allowed",
            INTENT_BRANCH
        )));
    }
    let target_branch = Branch::find_branch(branch_name, None)
        .await
        .ok_or_else(|| CliError::fatal(format!("branch '{}' not found", branch_name)))?;
    restore_to_commit(target_branch.commit).await?;
    let head = Head::Branch(branch_name.to_string());
    Head::update(head, None).await;
    Ok(())
}

async fn create_and_switch_new_branch(new_branch: &str) -> CliResult<()> {
    branch::create_branch_safe(new_branch.to_string(), get_current_branch().await).await?;
    switch_branch(new_branch).await?;
    println!("Switched to a new branch '{new_branch}'");
    Ok(())
}

async fn get_remote(branch_name: &str) -> CliResult<()> {
    let remote_branch_name: String = format!("origin/{branch_name}");

    create_and_switch_new_branch(branch_name).await?;
    // Set branch upstream
    branch::set_upstream_safe(branch_name, &remote_branch_name).await?;
    // Synchronous branches
    // Use the pull command to update the local branch with the latest changes from the remote branch
    pull::execute_safe(pull::PullArgs::make(None, None)).await?;
    Ok(())
}

/// Returns `Ok(Some(true))` if remote branch found, `Ok(Some(false))` if local branch found,
/// `Ok(None)` if already on the branch.
pub async fn check_branch(branch_name: &str) -> CliResult<Option<bool>> {
    if get_current_branch().await == Some(branch_name.to_string()) {
        println!("Already on {branch_name}");
        return Ok(None);
    }

    let target_branch: Option<Branch> = Branch::find_branch(branch_name, None).await;
    if target_branch.is_none() {
        let remote_branch_name: String = format!("origin/{branch_name}");
        if !Branch::search_branch(&remote_branch_name).await.is_empty() {
            println!("branch '{branch_name}' set up to track '{remote_branch_name}'.");
            Ok(Some(true))
        } else {
            Err(CliError::fatal(format!(
                "path specification '{}' did not match any files known to libra",
                branch_name
            )))
        }
    } else {
        println!("Switched to branch '{branch_name}'");
        Ok(Some(false))
    }
}

async fn check_and_switch_branch(branch_name: &str) -> CliResult<()> {
    match check_branch(branch_name).await? {
        Some(true) => get_remote(branch_name).await?,
        Some(false) => switch_branch(branch_name).await?,
        None => (),
    }
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

/// Unit tests for the checkout module
#[cfg(test)]
mod tests {}
