use crate::{
    command::get_target_commit,
    internal::{branch::Branch, config::Config, head::Head},
};
use clap::Parser;
use colored::Colorize;
use git_internal::internal::object::commit::Commit;

use crate::command::load_object;

pub enum BranchListMode {
    Local,
    Remote,
    All,
}

#[derive(Parser, Debug)]
pub struct BranchArgs {
    /// new branch name
    #[clap(group = "sub")]
    pub new_branch: Option<String>,

    /// base branch name or commit hash
    #[clap(requires = "new_branch")]
    pub commit_hash: Option<String>,

    /// list all branches, don't include remote branches
    #[clap(short, long, group = "sub", default_value = "true")]
    pub list: bool,

    /// force delete branch
    #[clap(short = 'D', long, group = "sub")]
    pub delete: Option<String>,

    ///  Set up `branchname`>`'s tracking information so `<`upstream`>` is considered `<`branchname`>`'s upstream branch.
    #[clap(short = 'u', long, group = "sub")]
    pub set_upstream_to: Option<String>,

    /// show current branch
    #[clap(long, group = "sub")]
    pub show_current: bool,

    /// Rename a branch. With one argument, renames the current branch. With two arguments, renames OLD_BRANCH to NEW_BRANCH.
    #[clap(short = 'm', long = "move", group = "sub", value_names = ["OLD_BRANCH", "NEW_BRANCH"], num_args = 1..=2)]
    pub rename: Vec<String>,

    /// show remote branches
    #[clap(short, long)] // TODO limit to required `list` option, even in default
    pub remotes: bool,

    /// show all branches (includes local and remote)
    #[clap(short, long, group = "sub")]
    pub all: bool,
}
pub async fn execute(args: BranchArgs) {
    if args.new_branch.is_some() {
        create_branch(args.new_branch.unwrap(), args.commit_hash).await;
    } else if args.delete.is_some() {
        delete_branch(args.delete.unwrap()).await;
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
    } else if args.list || args.all || args.remotes {
        // default behavior
        let mode = if args.all {
            BranchListMode::All
        } else if args.remotes {
            BranchListMode::Remote
        } else {
            BranchListMode::Local
        };
        list_branches(mode).await;
    } else {
        panic!("should not reach here")
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
pub async fn list_branches(mode: BranchListMode) {
    let head_name = display_head_state().await;

    match mode {
        BranchListMode::Local => {
            let branches = Branch::list_branches(None).await;
            display_branches(branches, &head_name, false);
        }

        BranchListMode::Remote => {
            let remote_configs = Config::all_remote_configs().await;
            let mut branches = vec![];
            for remote in remote_configs {
                let remote_branches = Branch::list_branches(Some(&remote.name)).await;
                branches.extend(remote_branches);
            }
            display_branches(branches, &head_name, true);
        }

        BranchListMode::All => {
            let branches = Branch::list_branches(None).await;
            display_branches(branches, &head_name, false);

            let remote_configs = Config::all_remote_configs().await;
            let mut remote_branches = vec![];
            for remote in remote_configs {
                let remote_branches_for_remote = Branch::list_branches(Some(&remote.name)).await;
                remote_branches.extend(remote_branches_for_remote);
            }
            display_branches(remote_branches, &head_name, true);
        }
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
mod tests {}
