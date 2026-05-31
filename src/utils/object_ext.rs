//! Extension traits for Tree/Commit/Blob to load from storage, expand items recursively with modes, save blobs, and support LFS-backed files.
//!
//! 树/提交/blob 的扩展特性，用于从存储加载、递归扩展项带有模式、保存 blob 并支持 LFS 支持的文件。

use std::{
    fs,
    io::{BufReader, Read},
    path::{Path, PathBuf},
};

use colored::Colorize;
use git_internal::{
    hash::ObjectHash,
    internal::object::{
        ObjectTrait,
        blob::Blob,
        commit::Commit,
        tree::{Tree, TreeItemMode},
    },
};

use crate::utils::{lfs, util};

pub trait TreeExt {
    fn load(hash: &ObjectHash) -> Tree;
    fn try_load(hash: &ObjectHash) -> Option<Tree>;
    fn get_plain_items(&self) -> Vec<(PathBuf, ObjectHash)>;
    /// Get all the items in the tree recursively with mode information
    /// Returns (path, hash, mode) tuples
    fn get_plain_items_with_mode(&self) -> Vec<(PathBuf, ObjectHash, TreeItemMode)>;
    fn get_items_with_mode(&self) -> Vec<(PathBuf, ObjectHash, TreeItemMode)>;
}

pub trait CommitExt {
    fn load(hash: &ObjectHash) -> Commit;
    fn try_load(hash: &ObjectHash) -> Option<Commit>;
}

pub trait BlobExt {
    fn load(hash: &ObjectHash) -> Blob;
    fn from_file(path: impl AsRef<Path>) -> Blob;
    fn from_lfs_file(path: impl AsRef<Path>) -> Blob;
    fn save(&self) -> ObjectHash;
}

impl TreeExt for Tree {
    /// Load a tree object by hash, panicking on missing or corrupt data.
    ///
    /// Callers that want to handle the "not found" / corruption case must use
    /// [`TreeExt::try_load`]. The `.expect()` messages below intentionally name
    /// the hash and operation so a panic produced through this fast-path API
    /// stays actionable in logs / stack traces.
    fn load(hash: &ObjectHash) -> Tree {
        let storage = util::objects_storage();
        let tree_data = storage.get(hash).unwrap_or_else(|err| {
            panic!("Tree::load({hash}): failed to read object from storage: {err}")
        });
        Tree::from_bytes(&tree_data, *hash).unwrap_or_else(|err| {
            panic!("Tree::load({hash}): failed to decode tree bytes: {err:?}")
        })
    }

    fn try_load(hash: &ObjectHash) -> Option<Tree> {
        let storage = util::objects_storage();
        storage
            .get(hash)
            .ok()
            .and_then(|tree_data| Tree::from_bytes(&tree_data, *hash).ok())
    }

    /// Get all the items in the tree recursively (to workdir path)
    fn get_plain_items(&self) -> Vec<(PathBuf, ObjectHash)> {
        let mut items = Vec::new();
        for item in self.tree_items.iter() {
            if item.mode != TreeItemMode::Tree {
                // `160000` gitlink entries (submodules) reference commits that are
                // not guaranteed to exist in this repository's object database.
                // Treat them as unsupported for plain-file expansion to avoid
                // panicking later when callers assume Blob objects.
                if item.mode == TreeItemMode::Commit {
                    eprintln!(
                        "{}",
                        format!(
                            "Warning: Submodule '{}' is not supported yet; skipping checkout entry",
                            item.name
                        )
                        .red()
                    );
                    continue;
                }
                // Not Tree, maybe Blob, link, etc.
                items.push((PathBuf::from(item.name.clone()), item.id));
            } else {
                let sub_tree = Tree::load(&item.id);
                let sub_entries = sub_tree.get_plain_items();

                items.append(
                    sub_entries
                        .iter()
                        .map(|(path, hash)| (PathBuf::from(item.name.clone()).join(path), *hash))
                        .collect::<Vec<(PathBuf, ObjectHash)>>()
                        .as_mut(),
                );
            }
        }
        items
    }

    /// Get all the items in the tree recursively with mode information
    fn get_plain_items_with_mode(&self) -> Vec<(PathBuf, ObjectHash, TreeItemMode)> {
        let mut items = Vec::new();
        for item in self.tree_items.iter() {
            if item.mode != TreeItemMode::Tree {
                // Not Tree, maybe Blob, link, etc.
                items.push((PathBuf::from(item.name.clone()), item.id, item.mode));
            } else {
                let sub_tree = Tree::load(&item.id);
                let sub_entries = sub_tree.get_plain_items_with_mode();

                // Use extend() instead of append() to avoid intermediate allocation
                items.extend(sub_entries.into_iter().map(|(path, hash, mode)| {
                    (PathBuf::from(item.name.clone()).join(path), hash, mode)
                }));
            }
        }
        items
    }

    /// Get all the items in the tree recursively with mode information
    fn get_items_with_mode(&self) -> Vec<(PathBuf, ObjectHash, TreeItemMode)> {
        let mut items = Vec::new();
        items.push((PathBuf::from("/"), self.id, TreeItemMode::Tree));
        for item in self.tree_items.iter() {
            if item.mode != TreeItemMode::Tree {
                // Not Tree, maybe Blob, link, etc.
                items.push((PathBuf::from(item.name.clone()), item.id, item.mode));
            } else {
                let sub_tree = Tree::load(&item.id);
                let sub_entries = sub_tree.get_items_with_mode();

                // Use extend() instead of append() to avoid intermediate allocation
                items.extend(sub_entries.into_iter().map(|(path, hash, mode)| {
                    (PathBuf::from(item.name.clone()).join(path), hash, mode)
                }));
            }
        }
        items
    }
}

impl CommitExt for Commit {
    /// Load a commit object by hash, panicking on missing or corrupt data.
    /// Callers that need to handle the "not found" / corruption case must use
    /// [`CommitExt::try_load`].
    fn load(hash: &ObjectHash) -> Commit {
        let storage = util::objects_storage();
        let commit_data = storage.get(hash).unwrap_or_else(|err| {
            panic!("Commit::load({hash}): failed to read object from storage: {err}")
        });
        Commit::from_bytes(&commit_data, *hash).unwrap_or_else(|err| {
            panic!("Commit::load({hash}): failed to decode commit bytes: {err:?}")
        })
    }

    fn try_load(hash: &ObjectHash) -> Option<Commit> {
        let storage = util::objects_storage();
        storage
            .get(hash)
            .ok()
            .and_then(|commit_data| Commit::from_bytes(&commit_data, *hash).ok())
    }
}

impl BlobExt for Blob {
    /// Load a blob object by hash, panicking on missing or corrupt data.
    fn load(hash: &ObjectHash) -> Blob {
        let storage = util::objects_storage();
        let blob_data = storage.get(hash).unwrap_or_else(|err| {
            panic!("Blob::load({hash}): failed to read object from storage: {err}")
        });
        Blob::from_bytes(&blob_data, *hash).unwrap_or_else(|err| {
            panic!("Blob::load({hash}): failed to decode blob bytes: {err:?}")
        })
    }

    /// Create a blob from a file
    /// - `path`: absolute  or relative path to current dir
    fn from_file(path: impl AsRef<Path>) -> Blob {
        let path = path.as_ref();
        let mut data = Vec::new();
        let file = fs::File::open(path).unwrap_or_else(|err| {
            panic!("Blob::from_file({}): open failed: {err}", path.display())
        });
        let mut reader = BufReader::new(file);
        reader.read_to_end(&mut data).unwrap_or_else(|err| {
            panic!("Blob::from_file({}): read failed: {err}", path.display())
        });
        Blob::from_content_bytes(data)
    }

    /// Create a blob from an LFS file
    /// - include: create a pointer file & copy the file to `.libra/lfs/objects`
    /// - `path`: absolute  or relative path to current dir
    fn from_lfs_file(path: impl AsRef<Path>) -> Blob {
        let path = path.as_ref();
        let (pointer, oid) = lfs::generate_pointer_file(path);
        tracing::debug!("\n{}", pointer);
        lfs::backup_lfs_file(path, &oid).unwrap_or_else(|err| {
            panic!(
                "Blob::from_lfs_file({}): backup to .libra/lfs/objects failed: {err}",
                path.display()
            )
        });
        Blob::from_content(&pointer)
    }

    fn save(&self) -> ObjectHash {
        let storage = util::objects_storage();
        let id = self.id;
        if !storage.exist(&id) {
            storage
                .put(&id, &self.data, self.get_type())
                .unwrap_or_else(|err| panic!("Blob::save({id}): storage.put failed: {err}"));
        }
        self.id
    }
}
