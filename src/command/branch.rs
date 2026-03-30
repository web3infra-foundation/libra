//! Branch management utilities for creating, deleting, listing, and switching branches while handling upstream metadata.

use std::collections::{HashSet, VecDeque};

use clap::{ArgGroup, Parser};
use colored::Colorize;
use git_internal::{hash::ObjectHash, internal::object::commit::Commit};
use serde::Serialize;

use crate::{
    command::{get_target_commit, load_object},
    internal::{
        branch::{self, Branch},
        config::ConfigKv,
        head::Head,
    },
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
    },
};

pub enum BranchListMode {
    Local,
    Remote,
    All,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "action")]
pub enum BranchOutput {
    #[serde(rename = "list")]
    List { branches: Vec<BranchListEntry> },
    #[serde(rename = "create")]
    Create { name: String, commit: String },
    #[serde(rename = "delete")]
    Delete {
        name: String,
        commit: String,
        force: bool,
    },
    #[serde(rename = "rename")]
    Rename { old_name: String, new_name: String },
    #[serde(rename = "set-upstream")]
    SetUpstream { branch: String, upstream: String },
    #[serde(rename = "show-current")]
    ShowCurrent {
        name: Option<String>,
        detached: bool,
        commit: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct BranchListEntry {
    pub name: String,
    pub current: bool,
    pub commit: String,
}

const BRANCH_AFTER_HELP: &str = "\
Compatibility Notes:
  Libra's global --quiet suppresses the branch listing itself.
  This differs from `git branch --quiet`, which still prints the primary list.
";

// action options are mutually exclusive with query options
// query options can be combined
#[derive(Parser, Debug)]
#[command(after_help = BRANCH_AFTER_HELP)]
#[command(group(
    ArgGroup::new("action")
        .multiple(false)
        .conflicts_with("query")
))]
#[command(group(
    ArgGroup::new("query")
        .required(false)
        .multiple(true)
        .conflicts_with("action")
))]
pub struct BranchArgs {
    /// new branch name
    #[clap(group = "action")]
    pub new_branch: Option<String>,

    /// base branch name or commit hash
    #[clap(requires = "new_branch")]
    pub commit_hash: Option<String>,

    /// list all branches, don't include remote branches
    #[clap(short, long, group = "query")]
    pub list: bool,

    /// force delete branch
    #[clap(short = 'D', long = "delete-force", group = "action")]
    pub delete: Option<String>,

    /// safe delete branch (checks if merged before deletion)
    #[clap(short = 'd', long = "delete", group = "action")]
    pub delete_safe: Option<String>,

    ///  Set up `branchname`>`'s tracking information so `<`upstream`>` is considered `<`branchname`>`'s upstream branch.
    #[clap(short = 'u', long, group = "action")]
    pub set_upstream_to: Option<String>,

    /// show current branch
    #[clap(long, group = "action")]
    pub show_current: bool,

    /// Rename a branch. With one argument, renames the current branch. With two arguments, renames OLD_BRANCH to NEW_BRANCH.
    #[clap(short = 'm', long = "move", group = "action", value_names = ["OLD_BRANCH", "NEW_BRANCH"], num_args = 1..=2)]
    pub rename: Vec<String>,

    /// show remote branches
    #[clap(short, long, group = "query")]
    // TODO limit to required `list` option, even in default
    pub remotes: bool,

    /// show all branches (includes local and remote)
    #[clap(short, long, group = "query")]
    pub all: bool,

    /// Only list branches which contain the specified commit (HEAD if not specified). Implies --list.
    #[clap(long, group = "query", alias = "with", value_name = "commit", num_args = 0..=1, default_missing_value = "HEAD", action = clap::ArgAction::Append)]
    pub contains: Vec<String>,

    /// Only list branches which don’t contain the specified commit (HEAD if not specified). Implies --list.
    #[clap(long, group = "query", alias = "without", value_name = "commit", num_args = 0..=1, default_missing_value = "HEAD", action = clap::ArgAction::Append)]
    pub no_contains: Vec<String>,
}
pub async fn execute(args: BranchArgs) {
    if let Err(err) = execute_safe(args, &OutputConfig::default()).await {
        err.print_stderr();
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting. Creates, deletes, renames, or lists branches depending
/// on the provided arguments.
pub async fn execute_safe(args: BranchArgs, output: &OutputConfig) -> CliResult<()> {
    if output.is_json() {
        let result = run_branch_json(args, output).await?;
        return emit_json_data("branch", &result, output);
    }

    if let Some(new_branch) = args.new_branch {
        create_branch_safe(new_branch, args.commit_hash).await
    } else if let Some(branch_to_delete) = args.delete {
        delete_branch(branch_to_delete).await
    } else if let Some(branch_to_delete) = args.delete_safe {
        delete_branch_safe(branch_to_delete, output).await
    } else if args.show_current {
        if !output.quiet {
            show_current_branch().await;
        }
        Ok(())
    } else if let Some(upstream) = args.set_upstream_to {
        match Head::current().await {
            Head::Branch(name) => set_upstream_safe_with_output(&name, &upstream, output).await,
            Head::Detached(_) => Err(CliError::fatal("HEAD is detached")),
        }
    } else if !args.rename.is_empty() {
        rename_branch(args.rename, output).await
    } else {
        // Default behavior: list branches
        let list_mode = if args.all {
            BranchListMode::All
        } else if args.remotes {
            BranchListMode::Remote
        } else {
            BranchListMode::Local
        };

        if output.quiet {
            // Quiet mode: suppress branch listing.
            Ok(())
        } else {
            list_branches(list_mode, &args.contains, &args.no_contains).await
        }
    }
}

pub async fn set_upstream(branch: &str, upstream: &str) {
    if let Err(err) = set_upstream_safe(branch, upstream).await {
        err.print_stderr();
    }
}

pub async fn set_upstream_safe(branch: &str, upstream: &str) -> CliResult<()> {
    set_upstream_safe_with_output(branch, upstream, &OutputConfig::default()).await
}

pub async fn set_upstream_safe_with_output(
    branch: &str,
    upstream: &str,
    output: &OutputConfig,
) -> CliResult<()> {
    let branch_config = ConfigKv::branch_config(branch).await.ok().flatten();
    if branch_config.is_none() {
        let (remote, remote_branch) = match upstream.split_once('/') {
            Some((remote, branch)) => (remote, branch),
            None => {
                return Err(CliError::fatal(format!("invalid upstream '{}'", upstream))
                    .with_stable_code(StableErrorCode::CliInvalidTarget)
                    .with_hint("expected format: 'remote/branch'"));
            }
        };
        let _ = ConfigKv::set(&format!("branch.{branch}.remote"), remote, false).await;
        // set upstream branch (tracking branch)
        let _ = ConfigKv::set(
            &format!("branch.{branch}.merge"),
            &format!("refs/heads/{remote_branch}"),
            false,
        )
        .await;
    }
    crate::info_println!(
        output,
        "Branch '{branch}' set up to track remote branch '{upstream}'"
    );
    Ok(())
}

pub async fn create_branch(new_branch: String, branch_or_commit: Option<String>) {
    if let Err(err) = create_branch_safe(new_branch, branch_or_commit).await {
        err.print_stderr();
    }
}

pub async fn create_branch_safe(
    new_branch: String,
    branch_or_commit: Option<String>,
) -> CliResult<()> {
    tracing::debug!("create branch: {} from {:?}", new_branch, branch_or_commit);

    if !is_valid_git_branch_name(&new_branch) {
        return Err(
            CliError::fatal(format!("'{}' is not a valid branch name", new_branch))
                .with_stable_code(StableErrorCode::CliInvalidArguments),
        );
    }
    if branch::is_locked_branch(&new_branch) {
        return Err(CliError::fatal(format!(
            "the '{}' branch is locked and cannot be created",
            new_branch
        ))
        .with_stable_code(StableErrorCode::ConflictOperationBlocked));
    }

    // check if branch exists
    let branch = Branch::find_branch(&new_branch, None).await;
    if branch.is_some() {
        return Err(
            CliError::fatal(format!("a branch named '{}' already exists", new_branch))
                .with_stable_code(StableErrorCode::ConflictOperationBlocked),
        );
    }

    let base_name = branch_or_commit.clone();
    let commit_id = match branch_or_commit {
        Some(branch_or_commit) => get_target_commit(&branch_or_commit).await.map_err(|_| {
            CliError::fatal(format!("not a valid object name: '{}'", branch_or_commit))
                .with_stable_code(StableErrorCode::CliInvalidTarget)
        })?,
        None => {
            if let Some(commit_id) = Head::current_commit().await {
                commit_id
            } else {
                let current = match Head::current().await {
                    Head::Branch(name) => name,
                    Head::Detached(commit_hash) => commit_hash.to_string(),
                };
                return Err(
                    CliError::fatal(format!("not a valid object name: '{}'", current))
                        .with_stable_code(StableErrorCode::CliInvalidTarget),
                );
            }
        }
    };
    tracing::debug!("base commit_id: {}", commit_id);

    // check if commit_hash exists
    let commit_id_display = commit_id.to_string();
    load_object::<Commit>(&commit_id).map_err(|_| {
        CliError::fatal(format!(
            "not a valid object name: '{}'",
            base_name.as_deref().unwrap_or(commit_id_display.as_str())
        ))
        .with_stable_code(StableErrorCode::CliInvalidTarget)
    })?;

    // create branch
    Branch::update_branch(&new_branch, &commit_id.to_string(), None)
        .await
        .map_err(|e| {
            CliError::fatal(format!("failed to create branch '{}': {e}", new_branch))
                .with_stable_code(StableErrorCode::IoWriteFailed)
        })?;
    Ok(())
}

async fn delete_branch(branch_name: String) -> CliResult<()> {
    if branch::is_locked_branch(&branch_name) {
        return Err(CliError::fatal(format!(
            "the '{}' branch is locked and cannot be deleted",
            branch_name
        ))
        .with_stable_code(StableErrorCode::ConflictOperationBlocked));
    }

    Branch::find_branch(&branch_name, None)
        .await
        .ok_or_else(|| {
            CliError::fatal(format!("branch '{}' not found", branch_name))
                .with_stable_code(StableErrorCode::CliInvalidTarget)
                .with_hint("use 'libra branch -l' to list branches")
        })?;
    let head = Head::current().await;

    if let Head::Branch(name) = head
        && name == branch_name
    {
        return Err(CliError::fatal(format!(
            "Cannot delete the branch '{}' which you are currently on",
            branch_name
        ))
        .with_stable_code(StableErrorCode::RepoStateInvalid));
    }

    Branch::delete_branch(&branch_name, None).await;
    Ok(())
}

/// Safely delete a branch, refusing if it contains unmerged commits.
///
/// This performs a merge check to ensure the branch is fully merged into HEAD
/// before deletion. If the branch is not fully merged, prints an error and
/// suggests using `branch -D` for force deletion.
async fn delete_branch_safe(branch_name: String, output: &OutputConfig) -> CliResult<()> {
    if branch::is_locked_branch(&branch_name) {
        return Err(CliError::fatal(format!(
            "the '{}' branch is locked and cannot be deleted",
            branch_name
        ))
        .with_stable_code(StableErrorCode::ConflictOperationBlocked));
    }

    // 1. Check if branch exists
    let branch = Branch::find_branch(&branch_name, None)
        .await
        .ok_or_else(|| {
            CliError::fatal(format!("branch '{}' not found", branch_name))
                .with_stable_code(StableErrorCode::CliInvalidTarget)
                .with_hint("use 'libra branch -l' to list branches")
        })?;

    // 2. Check if trying to delete current branch
    let head = Head::current().await;
    if let Head::Branch(name) = &head
        && name == &branch_name
    {
        return Err(CliError::fatal(format!(
            "Cannot delete the branch '{}' which you are currently on",
            branch_name
        ))
        .with_stable_code(StableErrorCode::RepoStateInvalid));
    }

    // 3. Check if the branch is fully merged into HEAD
    // Get current HEAD commit
    let head_commit = match head {
        Head::Branch(_) => Head::current_commit()
            .await
            .ok_or_else(|| CliError::fatal("cannot get HEAD commit"))?,
        Head::Detached(commit_hash) => commit_hash,
    };

    // Get all commits reachable from HEAD
    let head_reachable =
        crate::command::log::get_reachable_commits(head_commit.to_string(), None).await?;

    // Build HashSet for efficient lookup using ObjectHash directly (avoid string allocations)
    let head_commit_ids: std::collections::HashSet<_> =
        head_reachable.iter().map(|c| c.id).collect();

    // Check if the branch's HEAD commit is reachable from current HEAD
    // If the branch commit is in HEAD's history, the branch is fully merged
    if !head_commit_ids.contains(&branch.commit) {
        // Branch is not fully merged
        return Err(CliError::failure(format!(
            "The branch '{}' is not fully merged.",
            branch_name
        ))
        .with_stable_code(StableErrorCode::RepoStateInvalid)
        .with_hint(format!(
            "If you are sure you want to delete it, run 'libra branch -D {}'.",
            branch_name
        )));
    }

    // All checks passed, safe to delete
    let commit = branch.commit.to_string();
    Branch::delete_branch(&branch_name, None).await;
    if !output.quiet && !output.is_json() {
        println!("Deleted branch {} (was {}).", branch_name, commit);
    }
    Ok(())
}

async fn rename_branch(args: Vec<String>, output: &OutputConfig) -> CliResult<()> {
    let (old_name, new_name) = match args.len() {
        1 => {
            // rename current branch
            let head = Head::current().await;
            match head {
                Head::Branch(name) => (name, args[0].clone()),
                Head::Detached(_) => {
                    return Err(CliError::fatal("HEAD is detached")
                        .with_stable_code(StableErrorCode::RepoStateInvalid));
                }
            }
        }
        2 => (args[0].clone(), args[1].clone()),
        _ => {
            return Err(CliError::command_usage("too many arguments")
                .with_stable_code(StableErrorCode::CliInvalidArguments)
                .with_hint("usage: libra branch -m [old-name] new-name"));
        }
    };

    if !is_valid_git_branch_name(&new_name) {
        return Err(
            CliError::fatal(format!("invalid branch name: {}", new_name))
                .with_stable_code(StableErrorCode::CliInvalidArguments),
        );
    }

    if branch::is_locked_branch(&new_name) {
        return Err(CliError::fatal(format!(
            "the '{}' branch is locked and cannot be overwritten",
            new_name
        ))
        .with_stable_code(StableErrorCode::ConflictOperationBlocked));
    }

    if branch::is_locked_branch(&old_name) {
        return Err(CliError::fatal(format!(
            "the '{}' branch is locked and cannot be renamed",
            old_name
        ))
        .with_stable_code(StableErrorCode::ConflictOperationBlocked));
    }

    // check if old branch exists
    let old_branch = Branch::find_branch(&old_name, None).await.ok_or_else(|| {
        CliError::fatal(format!("branch '{}' not found", old_name))
            .with_stable_code(StableErrorCode::CliInvalidTarget)
    })?;

    // check if new branch name already exists
    let new_branch_exists = Branch::find_branch(&new_name, None).await;
    if new_branch_exists.is_some() {
        return Err(
            CliError::fatal(format!("A branch named '{}' already exists.", new_name))
                .with_stable_code(StableErrorCode::ConflictOperationBlocked),
        );
    }

    let commit_hash = old_branch.commit.to_string();

    // create new branch with the same commit
    Branch::update_branch(&new_name, &commit_hash, None)
        .await
        .map_err(|e| {
            CliError::fatal(format!("failed to create branch '{}': {e}", new_name))
                .with_stable_code(StableErrorCode::IoWriteFailed)
        })?;

    // update HEAD if renaming current branch
    let head = Head::current().await;
    if let Head::Branch(name) = head
        && name == old_name
    {
        let new_head = Head::Branch(new_name.clone());
        Head::update(new_head, None).await;
    }

    // delete old branch
    Branch::delete_branch(&old_name, None).await;

    if !output.quiet && !output.is_json() {
        println!("Renamed branch '{old_name}' to '{new_name}'");
    }
    Ok(())
}

async fn show_current_branch() {
    // let head = reference::Model::current_head(&db).await.unwrap();
    let head = Head::current().await;
    match head {
        Head::Detached(commit_hash) => {
            println!("HEAD detached at {}", &commit_hash.to_string()[..8]);
        }
        Head::Branch(name) => {
            println!("{name}");
        }
    }
}
/// Return the current HEAD name and optionally print detached-HEAD info.
///
/// When `print` is `true`, a "HEAD detached at ..." line is written to stdout
/// (the traditional human-visible behavior). Pass `false` for machine-readable
/// paths that must not leak human text.
async fn head_branch_name(print: bool) -> String {
    let head = Head::current().await;
    if print && let Head::Detached(commit) = head {
        let s = "HEAD detached at  ".to_string() + &commit.to_string()[..8];
        let s = s.green();
        println!("{s}");
    }
    match head {
        Head::Branch(name) => name,
        Head::Detached(_) => "".to_string(),
    }
}

async fn display_head_state() -> String {
    head_branch_name(true).await
}

fn format_branch_name(branch: &Branch) -> String {
    let display_name = if let Some(stripped) = branch.name.strip_prefix("refs/remotes/") {
        stripped.to_string()
    } else {
        branch
            .remote
            .as_ref()
            .map(|remote| format!("{remote}/{}", branch.name))
            .unwrap_or_else(|| branch.name.clone())
    };
    display_name.red().to_string()
}

fn display_branches(branches: Vec<Branch>, head_name: &str, is_remote: bool) {
    let branches_sorted = {
        let mut sorted_branches = branches;
        sorted_branches.sort_by(|a, b| {
            if a.name == head_name {
                std::cmp::Ordering::Less
            } else if b.name == head_name {
                std::cmp::Ordering::Greater
            } else {
                a.name.cmp(&b.name)
            }
        });
        sorted_branches
    };

    for branch in branches_sorted {
        let name = if is_remote {
            format_branch_name(&branch)
        } else {
            branch.name.clone()
        };

        if head_name == branch.name {
            println!("* {}", name.green());
        } else {
            println!("  {}", name);
        }
    }
}

/// Collect branch names as strings for JSON output.
async fn collect_branch_names(
    list_mode: BranchListMode,
    commits_contains: &[String],
    commits_no_contains: &[String],
) -> CliResult<Vec<BranchListEntry>> {
    // Use the quiet variant: do NOT print "HEAD detached at ..." to stdout.
    let head_name = head_branch_name(false).await;

    let mut local_branches = match &list_mode {
        BranchListMode::Local | BranchListMode::All => Branch::list_branches(None).await,
        _ => vec![],
    };
    let mut remote_branches = vec![];
    match list_mode {
        BranchListMode::Remote | BranchListMode::All => {
            let remote_configs = ConfigKv::all_remote_configs().await.unwrap_or_default();
            for remote in remote_configs {
                remote_branches.extend(Branch::list_branches(Some(&remote.name)).await);
            }
        }
        _ => {}
    };

    let contains_set = resolve_commits(commits_contains).await?;
    let no_contains_set = resolve_commits(commits_no_contains).await?;
    for branches in [&mut local_branches, &mut remote_branches] {
        filter_branches(branches, &contains_set, &no_contains_set)?;
    }

    let mut result = Vec::new();
    for branch in local_branches.iter().chain(remote_branches.iter()) {
        result.push(BranchListEntry {
            name: branch.name.clone(),
            current: branch.name == head_name,
            commit: branch.commit.to_string(),
        });
    }
    Ok(result)
}

pub async fn list_branches(
    list_mode: BranchListMode,
    commits_contains: &[String],
    commits_no_contains: &[String],
) -> CliResult<()> {
    let head_name = display_head_state().await;
    let has_commit_filters = !commits_contains.is_empty() || !commits_no_contains.is_empty();

    // filter branches by `list_mode`
    let mut local_branches = match &list_mode {
        BranchListMode::Local | BranchListMode::All => Branch::list_branches(None).await,
        _ => vec![],
    };
    let mut remote_branches = vec![];
    match list_mode {
        BranchListMode::Remote | BranchListMode::All => {
            let remote_configs = ConfigKv::all_remote_configs().await.unwrap_or_default();
            for remote in remote_configs {
                remote_branches.extend(Branch::list_branches(Some(&remote.name)).await);
            }
        }
        _ => {}
    };

    // apply the filter to `local_branches` and `remote_branches`
    // When a list is empty the corresponding constraint is vacuously satisfied:
    //   - empty `commits_contains`    → every branch passes the "contains" check
    //   - empty `commits_no_contains` → every branch passes the "no-contains" check
    // Pre-resolve target commits once to avoid repeated string parsing
    let contains_set = resolve_commits(commits_contains).await?;
    let no_contains_set = resolve_commits(commits_no_contains).await?;
    for branches in [&mut local_branches, &mut remote_branches] {
        filter_branches(branches, &contains_set, &no_contains_set)?;
    }

    // display `local_branches` and `remote_branches` if not empty
    if !local_branches.is_empty() {
        display_branches(local_branches, &head_name, false);
    } else if matches!(list_mode, BranchListMode::Local | BranchListMode::All)
        && !has_commit_filters
    {
        // Fix: If there are no branches but we are on a valid HEAD (unborn branch), show it.
        // This happens on fresh init where HEAD points to 'main' but 'main' record doesn't exist yet.
        if !head_name.is_empty() {
            println!("* {}", head_name.green());
        }
    }
    if !remote_branches.is_empty() {
        display_branches(remote_branches, &head_name, true);
    }
    Ok(())
}

/// Filter given branches by whether they contain or don't contain certain commits.
///
/// Internal test helper — not part of the stable public API.
#[doc(hidden)]
pub fn filter_branches(
    branches: &mut Vec<Branch>,
    contains_set: &HashSet<ObjectHash>,
    no_contains_set: &HashSet<ObjectHash>,
) -> CliResult<()> {
    // Filter branches, propagating errors.
    // `retain` doesn't support fallible predicates, so we capture the first
    // error and short-circuit the remaining iterations.
    let mut error: Option<CliError> = None;
    branches.retain(|branch| {
        if error.is_some() {
            return false;
        }
        let contains_ok = contains_set.is_empty()
            || match commit_contains(branch, contains_set) {
                Ok(v) => v,
                Err(e) => {
                    error = Some(e);
                    return false;
                }
            };
        let no_contains_ok = no_contains_set.is_empty()
            || match commit_contains(branch, no_contains_set) {
                Ok(v) => !v,
                Err(e) => {
                    error = Some(e);
                    return false;
                }
            };
        contains_ok && no_contains_ok
    });
    if let Some(e) = error {
        return Err(e);
    }
    Ok(())
}

/// Resolve commit references to ObjectHash set.
async fn resolve_commits(commits: &[String]) -> CliResult<HashSet<ObjectHash>> {
    let mut set = HashSet::new();
    for commit in commits {
        let target_commit = get_target_commit(commit).await.map_err(|e| {
            CliError::fatal(format!("{}", e)).with_stable_code(StableErrorCode::CliInvalidTarget)
        })?;
        set.insert(target_commit);
    }
    Ok(set)
}

/// check if a branch contains at least one of the commits
///
/// NOTE: returns `false` if `commits` is empty
fn commit_contains(
    branch: &Branch,
    target_commits: &HashSet<ObjectHash>,
) -> Result<bool, CliError> {
    // do BFS to find out whether `branch` contains `target_commit` or not
    let mut q = VecDeque::new();
    let mut visited = HashSet::new();

    q.push_back(branch.commit);
    visited.insert(branch.commit);

    while let Some(current_commit) = q.pop_front() {
        // found target commit
        if target_commits.contains(&current_commit) {
            return Ok(true);
        }

        // enqueue all parent commits of `current_commit`
        let current_commit_object: Commit = load_object(&current_commit).map_err(|e| {
            CliError::fatal(format!("failed to load commit {}: {}", current_commit, e))
                .with_stable_code(StableErrorCode::RepoCorrupt)
        })?;
        for parent_commit in current_commit_object.parent_commit_ids {
            if !visited.contains(&parent_commit) {
                visited.insert(parent_commit);
                q.push_back(parent_commit);
            }
        }
    }

    // contains no commits
    Ok(false)
}

async fn run_branch_json(args: BranchArgs, output: &OutputConfig) -> CliResult<BranchOutput> {
    if let Some(new_branch) = args.new_branch {
        create_branch_safe(new_branch.clone(), args.commit_hash).await?;
        let branch = Branch::find_branch(&new_branch, None)
            .await
            .ok_or_else(|| {
                CliError::fatal(format!("branch '{}' not found", new_branch))
                    .with_stable_code(StableErrorCode::InternalInvariant)
            })?;
        Ok(BranchOutput::Create {
            name: new_branch,
            commit: branch.commit.to_string(),
        })
    } else if let Some(branch_to_delete) = args.delete {
        let branch = Branch::find_branch(&branch_to_delete, None)
            .await
            .ok_or_else(|| {
                CliError::fatal(format!("branch '{}' not found", branch_to_delete))
                    .with_stable_code(StableErrorCode::CliInvalidTarget)
                    .with_hint("use 'libra branch -l' to list branches")
            })?;
        delete_branch(branch_to_delete.clone()).await?;
        Ok(BranchOutput::Delete {
            name: branch_to_delete,
            commit: branch.commit.to_string(),
            force: true,
        })
    } else if let Some(branch_to_delete) = args.delete_safe {
        let branch = Branch::find_branch(&branch_to_delete, None)
            .await
            .ok_or_else(|| {
                CliError::fatal(format!("branch '{}' not found", branch_to_delete))
                    .with_stable_code(StableErrorCode::CliInvalidTarget)
                    .with_hint("use 'libra branch -l' to list branches")
            })?;
        delete_branch_safe(branch_to_delete.clone(), output).await?;
        Ok(BranchOutput::Delete {
            name: branch_to_delete,
            commit: branch.commit.to_string(),
            force: false,
        })
    } else if args.show_current {
        Ok(match Head::current().await {
            Head::Branch(name) => BranchOutput::ShowCurrent {
                name: Some(name),
                detached: false,
                commit: Head::current_commit().await.map(|hash| hash.to_string()),
            },
            Head::Detached(hash) => BranchOutput::ShowCurrent {
                name: None,
                detached: true,
                commit: Some(hash.to_string()),
            },
        })
    } else if let Some(upstream) = args.set_upstream_to {
        let branch = match Head::current().await {
            Head::Branch(name) => name,
            Head::Detached(_) => {
                return Err(CliError::fatal("HEAD is detached")
                    .with_stable_code(StableErrorCode::RepoStateInvalid));
            }
        };
        let mut quiet_output = output.clone();
        quiet_output.quiet = true;
        set_upstream_safe_with_output(&branch, &upstream, &quiet_output).await?;
        Ok(BranchOutput::SetUpstream { branch, upstream })
    } else if !args.rename.is_empty() {
        let old_name = if args.rename.len() == 1 {
            match Head::current().await {
                Head::Branch(name) => name,
                Head::Detached(_) => {
                    return Err(CliError::fatal("HEAD is detached")
                        .with_stable_code(StableErrorCode::RepoStateInvalid));
                }
            }
        } else {
            args.rename[0].clone()
        };
        let new_name = args.rename.last().cloned().unwrap_or_default();
        rename_branch(args.rename, output).await?;
        Ok(BranchOutput::Rename { old_name, new_name })
    } else {
        let list_mode = if args.all {
            BranchListMode::All
        } else if args.remotes {
            BranchListMode::Remote
        } else {
            BranchListMode::Local
        };
        let branches = collect_branch_names(list_mode, &args.contains, &args.no_contains).await?;
        Ok(BranchOutput::List { branches })
    }
}

pub fn is_valid_git_branch_name(name: &str) -> bool {
    // Validate branch name
    // Not contain spaces, control characters or special characters
    if name.contains(&[' ', '\t', '\\', ':', '"', '?', '*', '['][..])
        || name.chars().any(|c| c.is_ascii_control())
    {
        return false;
    }

    // Not start or end with a slash ('/'), or end with a dot ('.')
    // Not contain consecutive slashes ('//') or dots ('..')
    if name.starts_with('/')
        || name.ends_with('/')
        || name.ends_with('.')
        || name.contains("//")
        || name.contains("..")
    {
        return false;
    }

    // Not be reserved names like 'HEAD' or contain '@{'
    if name == "HEAD" || name.contains("@{") {
        return false;
    }

    // Not be empty or just a dot ('.')
    if name.trim().is_empty() || name.trim() == "." {
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use git_internal::hash::{ObjectHash, get_hash_kind};

    use super::{Branch, format_branch_name};

    fn any_hash() -> ObjectHash {
        ObjectHash::from_str(&ObjectHash::zero_str(get_hash_kind())).unwrap()
    }

    #[test]
    fn test_format_branch_name_with_full_remote_ref() {
        colored::control::set_override(false);
        let branch = Branch {
            name: "refs/remotes/origin/main".to_string(),
            commit: any_hash(),
            remote: Some("origin".to_string()),
        };

        assert_eq!(format_branch_name(&branch), "origin/main");
    }

    #[test]
    fn test_format_branch_name_with_short_remote_ref() {
        colored::control::set_override(false);
        let branch = Branch {
            name: "main".to_string(),
            commit: any_hash(),
            remote: Some("origin".to_string()),
        };

        assert_eq!(format_branch_name(&branch), "origin/main");
    }
}
