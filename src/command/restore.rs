//! Implements restore flows to reset files or entire trees from commits or the index, respecting pathspecs and staged vs worktree targets.

use std::{
    collections::{HashMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
};

use clap::Parser;
use git_internal::{
    hash::ObjectHash,
    internal::{
        index::{Index, IndexEntry},
        object::{blob::Blob, commit::Commit, tree::Tree, types::ObjectType},
    },
};

use crate::{
    command::calc_file_blob_hash,
    internal::{branch::Branch, head::Head, protocol::lfs_client::LFSClient},
    utils::{
        lfs,
        object_ext::{BlobExt, CommitExt, TreeExt},
        path,
        path_ext::PathExt,
        util,
    },
};

#[derive(Parser, Debug)]
pub struct RestoreArgs {
    /// files or dir to restore
    #[clap(required = true)]
    pub pathspec: Vec<String>,
    /// source
    #[clap(long, short)]
    pub source: Option<String>,
    /// worktree
    #[clap(long, short = 'W')]
    pub worktree: bool,
    /// staged
    #[clap(long, short = 'S')]
    pub staged: bool,
}

pub async fn execute(args: RestoreArgs) {
    if let Err(e) = execute_checked(args).await {
        eprintln!("fatal: {e}");
    }
}

pub async fn execute_checked(args: RestoreArgs) -> io::Result<()> {
    if !util::check_repo_exist() {
        return Ok(());
    }
    let staged = args.staged;
    let mut worktree = args.worktree;
    // If neither option is specified, by default the `working tree` is restored.
    // Specifying `--staged` will only restore the `index`. Specifying both restores both.
    if !staged {
        worktree = true;
    }

    const HEAD: &str = "HEAD"; // prevent misspelling
    let mut source = args.source;
    if source.is_none() && staged {
        // If `--source` not specified, the contents are restored from `HEAD` if `--staged` is given,
        // otherwise from the [index].
        source = Some(HEAD.to_string());
    }

    let storage = util::objects_storage();
    let target_commit: Option<ObjectHash> = match source {
        None => {
            assert!(!staged); // pre-processed ↑
            None // Index
        }
        Some(ref src) => {
            // ref: prevent moving `source`
            if src == HEAD {
                // Default Source
                Head::current_commit().await
            } else if Branch::exists(src).await {
                // Branch Name, e.g. master
                let branch = Branch::find_branch(src, None)
                    .await
                    .ok_or_else(|| io::Error::other(format!("could not resolve {src}")))?;
                Some(branch.commit)
            } else {
                // [Commit Hash, e.g. a1b2c3d4] || [Wrong Branch Name]
                let objs = storage.search(src).await;
                // TODO hash can be `commit` or `tree`
                if objs.len() != 1 || !storage.is_object_type(&objs[0], ObjectType::Commit) {
                    None // Wrong Commit Hash
                } else {
                    Some(objs[0])
                }
            }
        }
    };

    // to workdir path
    let target_blobs: Vec<(PathBuf, ObjectHash)> = {
        match (source.as_ref(), target_commit) {
            (None, _) => {
                // only this situation, restore from [Index]
                assert!(!staged);
                let index =
                    Index::load(path::index()).map_err(|e| io::Error::other(e.to_string()))?;
                index
                    .tracked_entries(0)
                    .into_iter()
                    .map(|entry| (PathBuf::from(&entry.name), entry.hash))
                    .collect()
            }
            (Some(_), Some(commit)) => {
                // restore from commit hash
                let tree_id = Commit::load(&commit).tree_id;
                let tree = Tree::load(&tree_id);
                tree.get_plain_items()
            }
            (Some(src), None) => {
                if storage.search(src).await.len() != 1 {
                    return Err(io::Error::other(format!("could not resolve {src}")));
                } else {
                    return Err(io::Error::other(format!(
                        "reference is not a commit: {src}"
                    )));
                }
            }
        }
    };

    // String to PathBuf
    let paths = args
        .pathspec
        .iter()
        .map(PathBuf::from)
        .collect::<Vec<PathBuf>>();
    // restore worktree and staged respectively
    // The order is very important
    // `restore_worktree` will decide whether to delete the file based on whether it is tracked in the index.
    if worktree {
        restore_worktree(&paths, &target_blobs).await?;
    }
    if staged {
        restore_index(&paths, &target_blobs)?;
    }
    Ok(())
}

/// to HashMap
/// - `blobs`: to workdir
fn preprocess_blobs(blobs: &[(PathBuf, ObjectHash)]) -> HashMap<PathBuf, ObjectHash> {
    // TODO maybe can be HashMap<&PathBuf, &ObjectHash>
    blobs
        .iter()
        .map(|(path, hash)| (path.clone(), *hash))
        .collect()
}

fn path_to_utf8(path: &Path) -> io::Result<&str> {
    path.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("non-UTF8 path: {}", path.display()),
        )
    })
}

/// Restore a blob to file.
/// If blob is an LFS pointer, download the actual file from LFS server.
/// - `path` : to workdir
async fn restore_to_file(hash: &ObjectHash, path: &PathBuf) -> io::Result<()> {
    let blob = Blob::load(hash);
    let path_abs = util::workdir_to_absolute(path);
    if let Some(parent) = path_abs.parent() {
        fs::create_dir_all(parent)?;
    }
    match lfs::parse_pointer_data(&blob.data) {
        Some((oid, size)) => {
            // LFS file
            let lfs_obj_path = lfs::lfs_object_path(&oid);
            if lfs_obj_path.exists() {
                // found in local cache
                fs::copy(&lfs_obj_path, &path_abs)?;
            } else {
                // not exist, download from server
                if let Err(e) = LFSClient::get()
                    .await
                    .download_object(&oid, size, &path_abs, None)
                    .await
                {
                    return Err(io::Error::other(e.to_string()));
                }
            }
        }
        None => {
            // normal file
            util::write_file(&blob.data, &path_abs)?;
        }
    }
    Ok(())
}

/// Get the deleted files in the worktree(vs Index), filtered by `filters`
/// - filters: absolute path or relative path to current dir
/// - target_blobs: to workdir path
fn get_worktree_deleted_files_in_filters(
    filters: &Vec<PathBuf>,
    target_blobs: &HashMap<PathBuf, ObjectHash>,
) -> HashSet<PathBuf> {
    target_blobs // to workdir
        .iter()
        .filter(|(path, _)| {
            let path = util::workdir_to_absolute(path); // to absolute path
            !path.exists() && path.sub_of_paths(filters) // in filters & target but not in workdir
        })
        .map(|(path, _)| path.clone())
        .collect() // HashSet auto deduplication
}

/// Restore the worktree
/// - `filter`: abs or relative to current (user input)
/// - `target_blobs`: to workdir path
pub async fn restore_worktree(
    filter: &Vec<PathBuf>,
    target_blobs: &[(PathBuf, ObjectHash)],
) -> io::Result<()> {
    let target_blobs = preprocess_blobs(target_blobs);
    let deleted_files = get_worktree_deleted_files_in_filters(filter, &target_blobs);

    {
        // validate input pathspec(filter)
        for path in filter {
            // abs or relative to cur
            if !path.exists() {
                //TODO bug problem: 路径设计大问题，全部统一为to workdir
                if !target_blobs
                    .iter()
                    .any(|(p, _)| util::is_sub_path(p.workdir_to_absolute(), path))
                {
                    // not in target_blobs & worktree, illegal path
                    return Err(io::Error::other(format!(
                        "pathspec '{}' did not match any files",
                        path.display()
                    )));
                }
            }
        }
    }

    // to workdir path
    let mut file_paths = util::integrate_pathspec(filter);
    file_paths.extend(deleted_files);

    let index = Index::load(path::index()).map_err(|e| io::Error::other(e.to_string()))?;
    for path_wd in &file_paths {
        let path_abs = util::workdir_to_absolute(path_wd);
        if !path_abs.exists() {
            // file not exist, deleted or illegal
            if target_blobs.contains_key(path_wd) {
                // file in target_blobs (deleted), need to restore
                restore_to_file(&target_blobs[path_wd], path_wd).await?;
            } else {
                // not in target_commit and workdir (illegal path), user input
                unreachable!("It should be checked before");
            }
        } else {
            // file exists
            let path_wd_str = path_to_utf8(path_wd)?;
            let hash =
                calc_file_blob_hash(&path_abs).map_err(|e| io::Error::other(e.to_string()))?;
            if target_blobs.contains_key(path_wd) {
                // both in target & worktree: 1. modified 2. same
                if hash != target_blobs[path_wd] {
                    // modified
                    restore_to_file(&target_blobs[path_wd], path_wd).await?;
                } // else: same, keep
            } else {
                // not in target but in worktree: New file
                if index.tracked(path_wd_str, 0) {
                    // tracked, need to delete
                    fs::remove_file(&path_abs)?;
                    util::clear_empty_dir(&path_abs); // clean empty dir in cascade
                } // else: untracked, keep
            }
        }
    }
    Ok(())
}

/// Get the deleted files in the `index`(vs target_blobs), filtered by `filters`
fn get_index_deleted_files_in_filters(
    index: &Index,
    filters: &Vec<PathBuf>,
    target_blobs: &HashMap<PathBuf, ObjectHash>,
) -> io::Result<HashSet<PathBuf>> {
    let mut deleted = HashSet::new();
    for path_wd in target_blobs.keys() {
        let path_wd_str = path_to_utf8(path_wd)?;
        let path_abs = util::workdir_to_absolute(path_wd); // to absolute path
        if !index.tracked(path_wd_str, 0) && util::is_sub_of_paths(path_abs, filters) {
            deleted.insert(path_wd.clone());
        }
    }
    Ok(deleted)
}

pub fn restore_index(
    filter: &Vec<PathBuf>,
    target_blobs: &[(PathBuf, ObjectHash)],
) -> io::Result<()> {
    let target_blobs = preprocess_blobs(target_blobs);

    let idx_file = path::index();
    let mut index = Index::load(&idx_file).map_err(|e| io::Error::other(e.to_string()))?;
    let deleted_files_index = get_index_deleted_files_in_filters(&index, filter, &target_blobs)?;

    let mut file_paths = util::filter_to_fit_paths(&index.tracked_files(), filter);
    file_paths.extend(deleted_files_index); // maybe we should not integrate them rater than deal separately

    for path in &file_paths {
        // to workdir
        let path_str = path_to_utf8(path)?;
        if !index.tracked(path_str, 0) {
            // file not exist in index
            if target_blobs.contains_key(path) {
                // file in target_blobs (deleted), need to restore
                let hash = target_blobs[path];
                let blob = Blob::load(&hash);
                index.add(IndexEntry::new_from_blob(
                    path_str.to_string(),
                    hash,
                    blob.data.len() as u32,
                ));
            } else {
                return Err(io::Error::other(format!(
                    "pathspec '{}' did not match any files",
                    path.display()
                )));
            }
        } else {
            // file exists in index: 1. modified 2. same 3. need to deleted
            if target_blobs.contains_key(path) {
                let hash = target_blobs[path];
                if !index.verify_hash(path_str, 0, &hash) {
                    // modified
                    let blob = Blob::load(&hash);
                    index.update(IndexEntry::new_from_blob(
                        path_str.to_string(),
                        hash,
                        blob.data.len() as u32,
                    ));
                } // else: same, keep
            } else {
                // not in target but in index: need to delete
                index.remove(path_str, 0); // TODO all stages
            }
        }
    }
    index
        .save(&idx_file)
        .map_err(|e| io::Error::other(e.to_string()))?; // DO NOT forget to save
    Ok(())
}
