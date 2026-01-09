//! Removes paths from the index and working tree according to pathspecs, supporting recursive deletion and cache-only modes.

use std::path::PathBuf;

use clap::Parser;
use colored::Colorize;
use git_internal::{errors::GitError, internal::index::Index};
use tokio::fs;

use crate::{
    command::status::{changes_to_be_committed, changes_to_be_staged},
    utils::{path, path_ext::PathExt, util},
};

#[derive(Parser, Debug, Clone)]
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
    /// Exit with a zero status even if no files matched.
    #[clap(long)]
    pub ignore_unmatch: bool,
}

//  ==============================================
//  Scenarios where --cached is recommended
//  ==============================================
//  1. Files have local modifications:
//     When the file in the working tree differs from the index,
//     the error message will prompt to use --cached to keep the local file.
//
//  2. Index has staged changes:
//     When the content in the index differs from HEAD,
//     the error message will also prompt to use --cached.

//  ==============================================
//  Scenarios where -f (force) is required
//  ==============================================
//  1. Both index and working tree have modifications:
//     The file's content in the index differs from the working tree,
//     AND the content in the index also differs from HEAD.
//
//  2. Has staged conflicting content:
//     When the staged content of the file differs from both the file itself (working tree) and HEAD,
//     the error message will prompt to use -f to force deletion.
/// Represents the status of files with uncommitted changes, used to determine
/// which files have local modifications, staged changes, or both, relative to the index and HEAD.
#[derive(Debug, Default)]
struct DiffStatus {
    /// Files with local modifications: working tree differs from index.
    index_workingtree: Option<Vec<String>>,
    /// Files with staged changes: index differs from HEAD (commit).
    index_commit: Option<Vec<String>>,
    /// Files with both staged and working tree changes that differ from HEAD.
    index_commit_workingtree: Vec<String>,
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

    let dirs = get_dirs(&args.pathspec);
    match validate_pathspec(&args.pathspec, &index, args.ignore_unmatch) {
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

    // Build the remove list from input paths, handling tracked files and optionally ignoring untracked paths based on the `ignore_unmatch` flag.
    for path_str in args.pathspec.iter() {
        let path = PathBuf::from(path_str);
        let relative_path = path.to_workdir().to_string_or_panic();
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
                }
            }
            // For recursive removal, add the directory itself to be removed from filesystem
            if args.recursive && !args.cached {
                remove_dir_list.push(path_str.clone());
            }
        } else {
            // file
            // - If tracked, would be removed from index
            if index.tracked(&relative_path, 0) {
                remove_list.push(path_str.clone());
            } else if !args.ignore_unmatch {
                // If ignore_unmatch is false, error if the pathspec does not match any tracked files (consistent with Git behavior).
                let error_msg = format!("pathspec '{path_str}' did not match any files");
                eprintln!("fatal: {error_msg}");
                return;
            }
        }
    }

    // Check all input paths for any uncommitted changes.
    let mut diff_status = DiffStatus::default();
    if !args.force {
        let mut error_msg = String::new();
        let changes_staged = changes_to_be_staged().polymerization();
        let changes_committed = changes_to_be_committed().await.polymerization();
        // Check for both
        let mut buf = Vec::new();
        for path_str in remove_list.iter() {
            if changes_staged.contains(&PathBuf::from(&path_str))
                && changes_committed.contains(&PathBuf::from(&path_str))
            {
                buf.push(path_str.clone());
            }
        }
        if !buf.is_empty() {
            diff_status.index_commit_workingtree = buf
        }
        if !args.cached {
            // Check for unstaged changes in workingtree files
            let mut buf = Vec::new();
            for path_str in remove_list.iter() {
                if changes_staged.contains(&PathBuf::from(path_str))
                    && !diff_status.index_commit_workingtree.contains(path_str)
                {
                    buf.push(path_str.clone());
                }
            }
            if !buf.is_empty() {
                diff_status.index_workingtree = Some(buf)
            }
            // Check for workingtree changes in committed files
            let mut buf = Vec::new();
            for path_str in remove_list.iter() {
                if changes_committed.contains(&PathBuf::from(path_str))
                    && !diff_status.index_commit_workingtree.contains(path_str)
                {
                    buf.push(path_str.clone());
                }
            }
            if !buf.is_empty() {
                diff_status.index_commit = Some(buf)
            }

            // Print error reason
            if let Some(files) = diff_status.index_commit.as_ref() {
                error_msg.push_str(&format!(
                    "error: the following {} changes staged in the index:\n",
                    if files.len() > 1 {
                        "files have"
                    } else {
                        "file has"
                    }
                ));
                for file in files {
                    error_msg.push_str(&format!("\t{}\n", file));
                }
                error_msg.push_str("(use --cached to keep the file, or -f to force removal)");
            }
            if let Some(files) = diff_status.index_workingtree.as_ref() {
                error_msg.push_str(&format!(
                    "error: the following {} local modifications:\n",
                    if files.len() > 1 {
                        "files have"
                    } else {
                        "file has"
                    }
                ));
                for file in files {
                    error_msg.push_str(&format!("\t{}\n", file));
                }
                error_msg.push_str("(use --cached to keep the file, or -f to force removal)");
            }
        }
        if !diff_status.index_commit_workingtree.is_empty() {
            error_msg.push_str(&format!("error: the following {} staged content different from both the\nfile and the HEAD:\n",
                if diff_status.index_commit_workingtree.len() > 1 {
                    "files have"
                } else {
                    "file has"
                }));
            for file in diff_status.index_commit_workingtree {
                error_msg.push_str(&format!("\t{}\n", file));
            }
            error_msg.push_str("(use -f to force removal)");
        }
        if !error_msg.is_empty() {
            eprintln!("{}", error_msg);
            return;
        }
    }

    for path_str in remove_list.iter() {
        println!("rm '{}'", path_str.bright_yellow());
        if !args.dry_run {
            let relative_path = PathBuf::from(&path_str).to_workdir().to_string_or_panic();
            index.remove(&relative_path, 0);
        }
    }
    if !args.cached && !args.dry_run {
        for path_str in remove_list {
            let path = PathBuf::from(&path_str);
            if let Err(e) = fs::remove_file(&path).await {
                eprintln!("Failed to remove file {}: {}", &path.display(), e);
            }
        }
        for path_str in remove_dir_list {
            let path = PathBuf::from(&path_str);
            if args.recursive {
                if let Err(e) = fs::remove_dir_all(&path).await {
                    eprintln!("Failed to remove directory {}: {}", &path.display(), e);
                }
            } else if let Err(e) = fs::remove_dir(&path).await {
                eprintln!("Failed to remove directory {}: {}", &path.display(), e);
            }
        }
    }

    if index.save(&idx_file).is_err() {
        eprintln!("Failed to save index.");
    }
}

/// check if pathspec is all valid(in index)
/// - if path is a dir, check if any file in the dir is in index
fn validate_pathspec(
    pathspec: &[String],
    index: &Index,
    ignore_unmatch: bool,
) -> Result<(), GitError> {
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
            if !index.contains_dir_file(&relative_path) && !ignore_unmatch {
                let error_msg = format!("pathspec '{path_str}' did not match any files");
                return Err(GitError::CustomError(error_msg));
            }
        }
    }
    Ok(())
}

/// run after `validate_pathspec`
fn get_dirs(pathspec: &[String]) -> Vec<String> {
    let mut dirs = Vec::new();
    for path_str in pathspec.iter() {
        let path = PathBuf::from(path_str);
        if path.exists() && path.is_dir() {
            dirs.push(path_str.clone());
        }
    }
    dirs
}
