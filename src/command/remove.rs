use std::fs;
use std::path::PathBuf;

use clap::Parser;
use colored::Colorize;

use git_internal::errors::GitError;
use tokio::fs::remove_dir_all;

use crate::command::status::{
    changes_to_be_committed, changes_to_be_staged, changes_to_be_staged_with_policy,
};
use crate::utils::path_ext::PathExt;
use crate::utils::{path, util};
use git_internal::internal::index::Index;

#[derive(Parser, Debug)]
pub struct RemoveArgs {
    /// file or dir to remove
    pub pathspec: Vec<String>,
    /// whether to remove from index
    #[clap(long)]
    pub cached: bool,
    /// indicate recursive remove dir
    #[clap(short, long)]
    pub recursive: bool,
    /// force removal, skip validation
    #[clap(short, long)]
    pub force: bool,
    /// show what would be removed without actually removing
    #[clap(long)]
    pub dry_run: bool,
}

pub async fn execute(args: RemoveArgs) {
    if !util::check_repo_exist() {
        return;
    }
    let idx_file = path::index();
    let mut remove_list = Vec::new();
    let mut remove_dir_list = Vec::new();
    let mut index = match Index::load(&idx_file) {
        Ok(index) => index,
        Err(err) => {
            eprintln!("fatal: {}", err);
            return;
        }
    };

    let dirs = get_dirs(&args.pathspec, &index, args.force);
    match validate_pathspec(&args.pathspec, &index) {
        Ok(_) => (),
        Err(err) => {
            eprintln!("fatal: {}", err);
            return;
        }
    }

    if !dirs.is_empty() && !args.recursive {
        let error_msg = format!("not removing '{}' recursively without -r", dirs[0]);
        eprintln!("fatal: {error_msg}");
        return;
    }

    // Check if all input paths are being traced.
    for path_str in args.pathspec.iter() {
        let path = PathBuf::from(path_str);
        let relative_path = path.to_workdir().to_string_or_panic();
        let mut empty_dir = true;
        if dirs.contains(path_str) {
            // dir - find all files in this directory that are tracked
            let entries = index.tracked_entries(0);
            // Create directory prefix with proper path separator for cross-platform compatibility
            let dir_prefix = if relative_path.is_empty() {
                String::new()
            } else if relative_path.ends_with(std::path::MAIN_SEPARATOR) {
                relative_path.clone()
            } else {
                format!("{}{}", relative_path, std::path::MAIN_SEPARATOR)
            };
            for entry in entries.iter() {
                if entry.name.starts_with(&dir_prefix) {
                    remove_list.push(entry.name.clone());
                    continue;
                }
                empty_dir = false
            }
            if empty_dir {
                remove_dir_list.push(path_str)
            }
        } else {
            // file
            // - If tracked, would be removed from index
            if index.tracked(&relative_path, 0) {
                remove_list.push(path_str.clone());
            } else {
                // In forced mode, untracked files are not processed, consistent with Git behavior.
                let error_msg = format!("pathspec '{path_str}' did not match any files");
                eprintln!("fatal: {error_msg}");
                return;
            }
        }
    }
    // Check all input paths for any uncommitted changes.
    if !args.force {
        let changes_staged = changes_to_be_staged().polymerization();
        let changes_commited = changes_to_be_committed().await.polymerization();
        // The output for HEAD inconsistency and temporary storage inconsistency is different, and it will output all the problems.
        // Do it tomorrow ！！
        for path_str in remove_list.iter() {
            if changes_staged.contains(&PathBuf::from(&path_str)) {
                let error_msg = format!(
                    "the following file has staged content differentfrom both thefile and the HEAD:\n\t\t{}\n\t(use -f to force removal)",
                    &path_str
                );
                eprintln!("error: {error_msg}");
                return;
            }
        }
    }
    for path_str in remove_list.iter() {
        let relative_path = PathBuf::from(&path_str).to_workdir().to_string_or_panic();
        index.remove(&relative_path, 0);
    }
    if !args.cached {
        for path_str in remove_list {
            let path = PathBuf::from(&path_str);
            println!("rm '{}'", path_str.bright_yellow());
            fs::remove_file(path).unwrap()
        }
        for path_str in remove_dir_list {
            let path = PathBuf::from(&path_str);
            fs::remove_dir(path).unwrap()
        }
    }

    // The unwrap function needs to be replaced subsequently to ensure consistency with git's behavior.
    index.save(&idx_file).unwrap();
}

/// check if pathspec is all valid(in index)
/// - if path is a dir, check if any file in the dir is in index
fn validate_pathspec(pathspec: &[String], index: &Index) -> Result<(), GitError> {
    if pathspec.is_empty() {
        let error_msg = "No pathspec was given. Which files should I remove?".to_string();
        return Err(GitError::CustomError(error_msg));
    }
    for path_str in pathspec.iter() {
        let path = PathBuf::from(path_str);
        let relative_path = path.to_workdir().to_string_or_panic();
        if !index.tracked(&relative_path, 0) {
            // not tracked, but path may be a directory
            // check if any tracked file in the directory
            if !index.contains_dir_file(&relative_path) {
                let error_msg = format!("pathspec '{path_str}' did not match any files");
                return Err(GitError::CustomError(error_msg));
            }
        }
    }
    Ok(())
}

/// run after `validate_pathspec`
fn get_dirs(pathspec: &[String], index: &Index, force: bool) -> Vec<String> {
    let mut dirs = Vec::new();
    for path_str in pathspec.iter() {
        let path = PathBuf::from(path_str);
        if path.exists() && path.is_dir() {
            dirs.push(path_str.clone());
        }
    }
    dirs
}
