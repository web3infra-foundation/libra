//! Branch management utilities for creating, deleting, listing, and switching branches while handling upstream metadata.

use std::collections::VecDeque;

use clap::{ArgGroup, Parser};
use colored::Colorize;
use git_internal::internal::object::commit::Commit;

use crate::{
    command::{get_target_commit, load_object},
    internal::{branch::Branch, config::Config, head::Head},
};

pub enum BranchListMode {
    Local,
    Remote,
    All,
}

// options which manipulate branches are mutually exclusive with options which show branches
// meanwhile, options which show branches can be combined
#[derive(Parser, Debug)]
#[command(group(
    ArgGroup::new("manipulate")
        .multiple(false)
        .conflicts_with("show")
))]
#[command(group(
    ArgGroup::new("show")
        .required(false)
        .multiple(true)
        .conflicts_with("manipulate")
))]
pub struct BranchArgs {
    /// new branch name
    #[clap(group = "manipulate")]
    pub new_branch: Option<String>,

    /// base branch name or commit hash
    #[clap(requires = "new_branch")]
    pub commit_hash: Option<String>,

    /// list all branches, don't include remote branches
    #[clap(short, long, group = "show")]
    pub list: bool,

    /// force delete branch
    #[clap(short = 'D', long = "delete-force", group = "manipulate")]
    pub delete: Option<String>,

    /// safe delete branch (checks if merged before deletion)
    #[clap(short = 'd', long = "delete", group = "manipulate")]
    pub delete_safe: Option<String>,

    ///  Set up `branchname`>`'s tracking information so `<`upstream`>` is considered `<`branchname`>`'s upstream branch.
    #[clap(short = 'u', long, group = "manipulate")]
    pub set_upstream_to: Option<String>,

    /// show current branch
    #[clap(long, group = "show")]
    pub show_current: bool,

    /// Rename a branch. With one argument, renames the current branch. With two arguments, renames OLD_BRANCH to NEW_BRANCH.
    #[clap(short = 'm', long = "move", group = "manipulate", value_names = ["OLD_BRANCH", "NEW_BRANCH"], num_args = 1..=2)]
    pub rename: Vec<String>,

    /// show remote branches
    #[clap(short, long, group = "show")]
    // TODO limit to required `list` option, even in default
    pub remotes: bool,

    /// show all branches (includes local and remote)
    #[clap(short, long, group = "show")]
    pub all: bool,

    /// Only list branches which contain the specified commit (HEAD if not specified). Implies --list.
    #[clap(long, group = "show", alias = "with", value_name = "commit", num_args = 0..=1, default_missing_value = "HEAD", action = clap::ArgAction::Append)]
    pub contains: Vec<String>,

    /// Only list branches which don’t contain the specified commit (HEAD if not specified). Implies --list.
    #[clap(long, group = "show", alias = "without", value_name = "commit", num_args = 0..=1, default_missing_value = "HEAD", action = clap::ArgAction::Append)]
    pub no_contains: Vec<String>,
}
pub async fn execute(args: BranchArgs) {
    if let Some(new_branch) = args.new_branch {
        create_branch(new_branch, args.commit_hash).await;
    } else if let Some(branch_to_delete) = args.delete {
        delete_branch(branch_to_delete).await;
    } else if let Some(branch_to_delete) = args.delete_safe {
        delete_branch_safe(branch_to_delete).await;
    } else if args.show_current {
        show_current_branch().await;
    } else if args.set_upstream_to.is_some() {
        match Head::current().await {
            Head::Branch(name) => set_upstream(&name, &args.set_upstream_to.unwrap()).await,
            Head::Detached(_) => {
                eprintln!("fatal: HEAD is detached");
            }
        };
    } else if !args.rename.is_empty() {
        rename_branch(args.rename).await;
    } else {
        // Default behavior: list branches
        // priority: `--all` > `--remote` > `--list` (default when no manipulate options given)
        let list_mode = if args.all {
            BranchListMode::All
        } else if args.remotes {
            BranchListMode::Remote
        } else {
            BranchListMode::Local
        };

        list_branches(list_mode, &args.contains, &args.no_contains).await;
    }
}

pub async fn set_upstream(branch: &str, upstream: &str) {
    let branch_config = Config::branch_config(branch).await;
    if branch_config.is_none() {
        let (remote, remote_branch) = match upstream.split_once('/') {
            Some((remote, branch)) => (remote, branch),
            None => {
                eprintln!("fatal: invalid upstream '{upstream}'");
                return;
            }
        };
        Config::insert("branch", Some(branch), "remote", remote).await;
        // set upstream branch (tracking branch)
        Config::insert(
            "branch",
            Some(branch),
            "merge",
            &format!("refs/heads/{remote_branch}"),
        )
        .await;
    }
    println!("Branch '{branch}' set up to track remote branch '{upstream}'");
}

pub async fn create_branch(new_branch: String, branch_or_commit: Option<String>) {
    tracing::debug!("create branch: {} from {:?}", new_branch, branch_or_commit);

    if !is_valid_git_branch_name(&new_branch) {
        eprintln!("fatal: invalid branch name: {new_branch}");
        return;
    }

    // check if branch exists
    let branch = Branch::find_branch(&new_branch, None).await;
    if branch.is_some() {
        panic!("fatal: A branch named '{new_branch}' already exists.");
    }

    let commit_id = match branch_or_commit {
        Some(branch_or_commit) => {
            let commit = get_target_commit(&branch_or_commit).await;
            match commit {
                Ok(commit) => commit,
                Err(e) => {
                    eprintln!("fatal: {e}");
                    return;
                }
            }
        }
        None => Head::current_commit().await.unwrap(),
    };
    tracing::debug!("base commit_id: {}", commit_id);

    // check if commit_hash exists
    let _ = load_object::<Commit>(&commit_id)
        .unwrap_or_else(|_| panic!("fatal: not a valid object name: '{commit_id}'"));

    // create branch
    Branch::update_branch(&new_branch, &commit_id.to_string(), None).await;
}

async fn delete_branch(branch_name: String) {
    let _ = Branch::find_branch(&branch_name, None)
        .await
        .unwrap_or_else(|| panic!("fatal: branch '{branch_name}' not found"));
    let head = Head::current().await;

    if let Head::Branch(name) = head
        && name == branch_name
    {
        panic!("fatal: Cannot delete the branch '{branch_name}' which you are currently on");
    }

    Branch::delete_branch(&branch_name, None).await;
}

/// Safely delete a branch, refusing if it contains unmerged commits.
///
/// This performs a merge check to ensure the branch is fully merged into HEAD
/// before deletion. If the branch is not fully merged, prints an error and
/// suggests using `branch -D` for force deletion.
async fn delete_branch_safe(branch_name: String) {
    // 1. Check if branch exists
    let branch = Branch::find_branch(&branch_name, None)
        .await
        .unwrap_or_else(|| panic!("fatal: branch '{branch_name}' not found"));

    // 2. Check if trying to delete current branch
    let head = Head::current().await;
    if let Head::Branch(name) = &head
        && name == &branch_name
    {
        panic!("fatal: Cannot delete the branch '{branch_name}' which you are currently on");
    }

    // 3. Check if the branch is fully merged into HEAD
    // Get current HEAD commit
    let head_commit = match head {
        Head::Branch(_) => Head::current_commit()
            .await
            .unwrap_or_else(|| panic!("fatal: cannot get HEAD commit")),
        Head::Detached(commit_hash) => commit_hash,
    };

    // Get all commits reachable from HEAD
    let head_reachable =
        crate::command::log::get_reachable_commits(head_commit.to_string(), None).await;

    // Build HashSet for efficient lookup using ObjectHash directly (avoid string allocations)
    let head_commit_ids: std::collections::HashSet<_> =
        head_reachable.iter().map(|c| c.id).collect();

    // Check if the branch's HEAD commit is reachable from current HEAD
    // If the branch commit is in HEAD's history, the branch is fully merged
    if !head_commit_ids.contains(&branch.commit) {
        // Branch is not fully merged
        eprintln!("error: The branch '{}' is not fully merged.", branch_name);
        eprintln!(
            "If you are sure you want to delete it, run 'libra branch -D {}'.",
            branch_name
        );
        return;
    }

    // All checks passed, safe to delete
    Branch::delete_branch(&branch_name, None).await;
    println!("Deleted branch {} (was {}).", branch_name, branch.commit);
}

async fn rename_branch(args: Vec<String>) {
    let (old_name, new_name) = match args.len() {
        1 => {
            // rename current branch
            let head = Head::current().await;
            match head {
                Head::Branch(name) => (name, args[0].clone()),
                Head::Detached(_) => {
                    eprintln!("fatal: HEAD is detached");
                    return;
                }
            }
        }
        2 => (args[0].clone(), args[1].clone()),
        _ => {
            eprintln!("fatal: too many arguments");
            return;
        }
    };

    if !is_valid_git_branch_name(&new_name) {
        eprintln!("fatal: invalid branch name: {new_name}");
        return;
    }

    // check if old branch exists
    let old_branch = Branch::find_branch(&old_name, None).await;
    if old_branch.is_none() {
        eprintln!("fatal: branch '{old_name}' not found");
        return;
    }

    // check if new branch name already exists
    let new_branch_exists = Branch::find_branch(&new_name, None).await;
    if new_branch_exists.is_some() {
        eprintln!("fatal: A branch named '{new_name}' already exists.");
        return;
    }

    let old_branch = old_branch.unwrap();
    let commit_hash = old_branch.commit.to_string();

    // create new branch with the same commit
    Branch::update_branch(&new_name, &commit_hash, None).await;

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

    println!("Renamed branch '{old_name}' to '{new_name}'");
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
async fn display_head_state() -> String {
    let head = Head::current().await;
    if let Head::Detached(commit) = head {
        let s = "HEAD detached at  ".to_string() + &commit.to_string()[..8];
        let s = s.green();
        println!("{s}");
    };
    match head {
        Head::Branch(name) => name,
        Head::Detached(_) => "".to_string(),
    }
}

fn format_branch_name(branch: &Branch) -> String {
    branch
        .remote
        .as_ref()
        .map(|remote| format!("{}/{}", remote, branch.name))
        .unwrap_or_else(|| branch.name.clone())
        .red()
        .to_string()
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

pub async fn list_branches(
    list_mode: BranchListMode,
    commits_contains: &[String],
    commits_no_contains: &[String],
) {
    let head_name = display_head_state().await;

    // filter branches by `list_mode`
    let mut local_branches = match &list_mode {
        BranchListMode::Local | BranchListMode::All => Branch::list_branches(None).await,
        _ => vec![],
    };
    let mut remote_branches = vec![];
    match list_mode {
        BranchListMode::Remote | BranchListMode::All => {
            let remote_configs = Config::all_remote_configs().await;
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
    for branches in [&mut local_branches, &mut remote_branches] {
        filter_branches(branches, commits_contains, commits_no_contains).await;
    }

    // display `local_branches` and `remote_branches` if not empty
    if !local_branches.is_empty() {
        display_branches(local_branches, &head_name, false);
    }
    if !remote_branches.is_empty() {
        display_branches(remote_branches, &head_name, true);
    }
}

/// Filter given branches by whether they contain or don't contain certain commits.
///
/// Internal test helper — not part of the stable public API.
#[doc(hidden)]
pub async fn filter_branches(
    branches: &mut Vec<Branch>,
    commits_contains: &[String],
    commits_no_contains: &[String],
) {
    let mut keep = vec![false; branches.len()];
    for (i, branch) in branches.iter().enumerate() {
        let contains_ok =
            commits_contains.is_empty() || commit_contains(branch, commits_contains).await;
        let no_contains_ok =
            commits_no_contains.is_empty() || !commit_contains(branch, commits_no_contains).await;
        keep[i] = contains_ok && no_contains_ok;
    }
    let mut keep_iter = keep.iter();
    branches.retain(|_| *keep_iter.next().unwrap());
}

/// check if a branch contains at least one of the commits
///
/// NOTE: returns `false` if `commits` is empty
async fn commit_contains(branch: &Branch, commits: &[String]) -> bool {
    for commit in commits {
        let target_commit = match get_target_commit(commit).await {
            Ok(commit) => commit,
            Err(e) => panic!("fatal: {e}"),
        };

        let mut q = VecDeque::new();
        q.push_back(branch.commit);

        while let Some(current_commit) = q.pop_front() {
            // found target commit
            if current_commit == target_commit {
                return true;
            }

            // enqueue all parent commits of `current_commit`
            let current_commit_object: Commit = match load_object(&current_commit) {
                Ok(commit) => commit,
                Err(e) => panic!("error: failed to load commit {current_commit}: {e}"),
            };
            for parent_commit in current_commit_object.parent_commit_ids {
                q.push_back(parent_commit);
            }
        }
    }

    // contains no commits
    false
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
mod tests {}
