//! Worktree helpers shared across commands.

use std::path::{Path, PathBuf};

use git_internal::internal::index::Index;

use crate::utils::{
    ignore::{self, IgnorePolicy},
    util,
};

pub fn untracked_workdir_paths(current_index: &Index) -> Result<Vec<PathBuf>, String> {
    let workdir_files = util::list_workdir_files().map_err(|e| e.to_string())?;
    let visible_files =
        ignore::filter_workdir_paths(workdir_files, IgnorePolicy::Respect, current_index);
    let mut untracked = Vec::new();
    for path in visible_files {
        let path_str = path
            .to_str()
            .ok_or_else(|| format!("path {:?} is not valid UTF-8", path))?;
        if !index_has_any_stage(current_index, path_str) {
            untracked.push(path);
        }
    }
    Ok(untracked)
}

pub fn index_has_any_stage(index: &Index, path: &str) -> bool {
    (0..=3).any(|stage| index.tracked(path, stage))
}

pub fn untracked_overwrite_path(untracked: &[PathBuf], new_index: &Index) -> Option<PathBuf> {
    let new_tracked = new_index.tracked_files();
    for untracked_path in untracked {
        for tracked_path in &new_tracked {
            if paths_conflict(untracked_path, tracked_path) {
                return Some(untracked_path.clone());
            }
        }
    }
    None
}

pub fn paths_conflict(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}
