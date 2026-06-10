#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{
    cmp::Reverse,
    collections::HashMap,
    fs,
    path::{Component, Path, PathBuf},
};

use git_internal::{
    hash::ObjectHash,
    internal::{
        index::Index,
        object::{
            ObjectTrait,
            commit::Commit,
            signature::Signature,
            tree::{Tree, TreeItem, TreeItemMode},
        },
    },
};

use super::{StashError, StashPushOptions, build_tree_from_flat_items};
use crate::{
    command::{
        load_object,
        reset::{remove_empty_directories, restore_working_directory_from_tree},
        status,
    },
    utils::{object, tree, util},
};

pub(super) fn collect_included_untracked_paths(
    options: &StashPushOptions,
) -> Result<Vec<PathBuf>, StashError> {
    if !options.include_untracked {
        return Ok(Vec::new());
    }

    let (mut visible, ignored) = if options.include_ignored {
        status::changes_to_be_staged_split_force()
    } else {
        status::changes_to_be_staged_split_safe()
    }
    .map_err(|error| {
        StashError::ReadObject(format!(
            "failed to inspect working tree for untracked files: {error}"
        ))
    })?;

    if options.include_ignored {
        visible.new.extend(ignored.new);
    }
    visible.new.retain(|path| !is_internal_untracked_path(path));
    visible.new.sort();
    visible.new.dedup();
    Ok(visible.new)
}

fn is_internal_untracked_path(path: &Path) -> bool {
    let Some(Component::Normal(first)) = path.components().next() else {
        return false;
    };
    let Some(first) = first.to_str() else {
        return false;
    };

    first == util::ROOT_DIR || first == ".git" || first == ".libra-test-home"
}

pub(super) fn create_untracked_parent_commit(
    workdir: &Path,
    git_dir: &Path,
    paths: &[PathBuf],
    author: &Signature,
    committer: &Signature,
    message: &str,
) -> Result<ObjectHash, StashError> {
    let untracked_tree =
        create_tree_from_paths(workdir, git_dir, paths).map_err(StashError::WriteObject)?;
    let untracked_tree_data = untracked_tree
        .to_data()
        .map_err(|error| StashError::WriteObject(error.to_string()))?;
    let untracked_tree_hash = object::write_git_object(git_dir, "tree", &untracked_tree_data)
        .map_err(|error| StashError::WriteObject(error.to_string()))?;
    let untracked_commit = Commit::new(
        author.clone(),
        committer.clone(),
        untracked_tree_hash,
        Vec::new(),
        message,
    );
    let untracked_commit_data = untracked_commit
        .to_data()
        .map_err(|error| StashError::WriteObject(error.to_string()))?;
    object::write_git_object(git_dir, "commit", &untracked_commit_data)
        .map_err(|error| StashError::WriteObject(error.to_string()))
}

fn create_tree_from_paths(
    workdir: &Path,
    git_dir: &Path,
    paths: &[PathBuf],
) -> Result<Tree, String> {
    let mut files = HashMap::new();
    for relative_path in paths {
        let full_path = workdir.join(relative_path);
        if !full_path.is_file() {
            return Err(format!(
                "included untracked path is not a file: {}",
                relative_path.display()
            ));
        }
        let path_str = worktree_relative_path_to_string(relative_path)?;
        let metadata = fs::metadata(&full_path).map_err(|error| error.to_string())?;
        let content = fs::read(&full_path).map_err(|error| error.to_string())?;
        let blob_hash = object::write_git_object(git_dir, "blob", &content)
            .map_err(|error| error.to_string())?;
        let mode = tree_item_mode_from_metadata(&metadata);
        files.insert(path_str.clone(), TreeItem::new(mode, blob_hash, path_str));
    }

    build_tree_from_flat_items(&files, git_dir)
}

fn worktree_relative_path_to_string(path: &Path) -> Result<String, String> {
    path.to_str()
        .map(ToString::to_string)
        .ok_or_else(|| format!("invalid path encoding: {}", path.display()))
}

pub(super) fn tree_item_mode_from_metadata(metadata: &fs::Metadata) -> TreeItemMode {
    #[cfg(unix)]
    {
        if metadata.permissions().mode() & 0o111 != 0 {
            TreeItemMode::BlobExecutable
        } else {
            TreeItemMode::Blob
        }
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        TreeItemMode::Blob
    }
}

pub(super) fn restore_worktree_to_index(
    index: &Index,
    head_commit_hash: &ObjectHash,
    workdir: &Path,
    git_dir: &Path,
) -> Result<(), String> {
    let target_commit: Commit = load_object(head_commit_hash)
        .map_err(|error| format!("failed to load target commit: {error}"))?;
    let target_tree: Tree = load_object(&target_commit.tree_id)
        .map_err(|error| format!("failed to load target tree: {error}"))?;
    let head_files = tree::get_tree_files_recursive(&target_tree, git_dir, &PathBuf::new())?;

    for path in head_files.keys() {
        if index.get(path, 0).is_none() {
            let full_path = workdir.join(path);
            if full_path.exists() {
                fs::remove_file(&full_path).map_err(|error| {
                    format!("failed to remove file {}: {error}", full_path.display())
                })?;
            }
        }
    }

    let index_tree = tree::create_tree_from_index(index).map_err(|error| error.to_string())?;
    restore_working_directory_from_tree(&index_tree, workdir, "")?;
    remove_empty_directories(workdir)?;
    Ok(())
}

pub(super) fn remove_included_untracked_paths(
    workdir: &Path,
    paths: &[PathBuf],
) -> Result<(), String> {
    let mut sorted_paths = paths.to_vec();
    sorted_paths.sort_by_key(|path| Reverse(path.components().count()));

    for relative_path in &sorted_paths {
        let full_path = workdir.join(relative_path);
        if full_path.is_dir() {
            fs::remove_dir_all(&full_path).map_err(|error| {
                format!(
                    "failed to remove directory {}: {error}",
                    full_path.display()
                )
            })?;
        } else if full_path.exists() {
            fs::remove_file(&full_path).map_err(|error| {
                format!("failed to remove file {}: {error}", full_path.display())
            })?;
        }
        remove_empty_parent_dirs(workdir, relative_path)?;
    }

    Ok(())
}

fn remove_empty_parent_dirs(workdir: &Path, relative_path: &Path) -> Result<(), String> {
    let Some(parent) = relative_path.parent() else {
        return Ok(());
    };
    let mut current = workdir.join(parent);
    while current != workdir && current.starts_with(workdir) {
        if current.file_name().and_then(|name| name.to_str()) == Some(util::ROOT_DIR) {
            break;
        }
        match fs::remove_dir(&current) {
            Ok(()) => {
                let Some(next) = current.parent() else {
                    break;
                };
                current = next.to_path_buf();
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(next) = current.parent() else {
                    break;
                };
                current = next.to_path_buf();
            }
            Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => break,
            Err(error) => {
                return Err(format!(
                    "failed to remove empty directory {}: {error}",
                    current.display()
                ));
            }
        }
    }
    Ok(())
}
