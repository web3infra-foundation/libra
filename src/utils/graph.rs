//! Commit and tree graph walkers for plumbing commands.

use std::{
    cmp::Ordering,
    collections::{BinaryHeap, HashSet},
};

use git_internal::{
    hash::ObjectHash,
    internal::object::{
        ObjectTrait,
        commit::Commit,
        tree::{Tree, TreeItem, TreeItemMode},
    },
};

use crate::utils::{
    error::{CliError, CliResult, StableErrorCode},
    util,
};

#[derive(Clone)]
struct CommitNode {
    commit: Commit,
    sequence: usize,
}

impl Eq for CommitNode {}

impl PartialEq for CommitNode {
    fn eq(&self, other: &Self) -> bool {
        self.commit.id == other.commit.id
    }
}

impl Ord for CommitNode {
    fn cmp(&self, other: &Self) -> Ordering {
        self.commit
            .committer
            .timestamp
            .cmp(&other.commit.committer.timestamp)
            .then_with(|| other.sequence.cmp(&self.sequence))
    }
}

impl PartialOrd for CommitNode {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Commit-date priority walker over one or more positive tips.
pub struct CommitWalker {
    frontier: BinaryHeap<CommitNode>,
    seen: HashSet<ObjectHash>,
    excluded: HashSet<ObjectHash>,
    next_sequence: usize,
}

impl CommitWalker {
    /// Create a walker from resolved commit tips and an optional excluded set.
    ///
    /// # Errors
    /// Returns [`CliError`] when any positive tip cannot be loaded as a commit.
    pub fn new(tips: &[ObjectHash], excluded: HashSet<ObjectHash>) -> CliResult<Self> {
        let mut walker = Self {
            frontier: BinaryHeap::new(),
            seen: HashSet::new(),
            excluded,
            next_sequence: 0,
        };
        for tip in tips {
            walker.push_commit(*tip)?;
        }
        Ok(walker)
    }

    fn push_commit(&mut self, id: ObjectHash) -> CliResult<()> {
        if self.excluded.contains(&id) || !self.seen.insert(id) {
            return Ok(());
        }
        let commit = load_graph_object::<Commit>(&id, "commit")?;
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        self.frontier.push(CommitNode { commit, sequence });
        Ok(())
    }

    /// Return the next reachable commit, newest first by committer timestamp.
    ///
    /// # Errors
    /// Returns [`CliError`] if a parent commit is missing or corrupt.
    pub fn next_commit(&mut self) -> CliResult<Option<Commit>> {
        let Some(node) = self.frontier.pop() else {
            return Ok(None);
        };
        for parent in &node.commit.parent_commit_ids {
            self.push_commit(*parent)?;
        }
        Ok(Some(node.commit))
    }

    /// Drain the walker into a commit vector.
    ///
    /// # Errors
    /// Returns [`CliError`] if any reachable commit is missing or corrupt.
    pub fn collect(mut self) -> CliResult<Vec<Commit>> {
        let mut commits = Vec::new();
        while let Some(commit) = self.next_commit()? {
            commits.push(commit);
        }
        Ok(commits)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeWalkObjectKind {
    Tree,
    Blob,
}

impl TreeWalkObjectKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tree => "tree",
            Self::Blob => "blob",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeWalkObject {
    pub id: ObjectHash,
    pub path: String,
    pub kind: TreeWalkObjectKind,
}

struct TreeStackEntry {
    id: ObjectHash,
    path: String,
    mode: TreeItemMode,
}

/// Stack-based tree walker used by `rev-list --objects`.
pub struct TreeWalker {
    stack: Vec<TreeStackEntry>,
    seen: HashSet<ObjectHash>,
    warnings: Vec<String>,
}

impl TreeWalker {
    pub fn new(root_tree: ObjectHash) -> Self {
        Self {
            stack: vec![TreeStackEntry {
                id: root_tree,
                path: String::new(),
                mode: TreeItemMode::Tree,
            }],
            seen: HashSet::new(),
            warnings: Vec::new(),
        }
    }

    pub fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }

    /// Return the next tree/blob object. Gitlink entries are skipped because
    /// their commits live in submodule object stores, not this repository.
    ///
    /// # Errors
    /// Returns [`CliError`] when a reachable tree/blob object is missing or corrupt.
    pub fn next_object(&mut self) -> CliResult<Option<TreeWalkObject>> {
        while let Some(entry) = self.stack.pop() {
            if !self.seen.insert(entry.id) {
                continue;
            }
            match entry.mode {
                TreeItemMode::Tree => {
                    let tree = load_graph_object::<Tree>(&entry.id, "tree")?;
                    self.push_children(&entry.path, &tree.tree_items);
                    return Ok(Some(TreeWalkObject {
                        id: entry.id,
                        path: entry.path,
                        kind: TreeWalkObjectKind::Tree,
                    }));
                }
                TreeItemMode::Blob | TreeItemMode::BlobExecutable | TreeItemMode::Link => {
                    verify_graph_object_exists(&entry.id, "blob")?;
                    return Ok(Some(TreeWalkObject {
                        id: entry.id,
                        path: entry.path,
                        kind: TreeWalkObjectKind::Blob,
                    }));
                }
                TreeItemMode::Commit => {}
            }
        }
        Ok(None)
    }

    fn push_children(&mut self, parent_path: &str, items: &[TreeItem]) {
        for item in items.iter().rev() {
            let Some(path) = child_path(parent_path, &item.name) else {
                self.warnings.push(format!(
                    "rev-list --objects skipped unsafe tree path component '{}'",
                    item.name
                ));
                continue;
            };
            self.stack.push(TreeStackEntry {
                id: item.id,
                path,
                mode: item.mode,
            });
        }
    }
}

fn child_path(parent: &str, name: &str) -> Option<String> {
    if name.is_empty()
        || name
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return None;
    }
    if parent.is_empty() {
        Some(name.to_string())
    } else {
        Some(format!("{parent}/{name}"))
    }
}

fn load_graph_object<T>(id: &ObjectHash, kind: &str) -> CliResult<T>
where
    T: ObjectTrait,
{
    let storage = util::try_objects_storage().map_err(|error| {
        CliError::fatal(format!("failed to open object storage: {error}"))
            .with_stable_code(StableErrorCode::IoReadFailed)
    })?;
    let data = storage.get(id).map_err(|error| {
        CliError::fatal(format!("failed to load {kind} object '{id}': {error}"))
            .with_stable_code(StableErrorCode::RepoCorrupt)
    })?;
    T::from_bytes(&data, *id).map_err(|error| {
        CliError::fatal(format!("failed to decode {kind} object '{id}': {error}"))
            .with_stable_code(StableErrorCode::RepoCorrupt)
    })
}

fn verify_graph_object_exists(id: &ObjectHash, kind: &str) -> CliResult<()> {
    let storage = util::try_objects_storage().map_err(|error| {
        CliError::fatal(format!("failed to open object storage: {error}"))
            .with_stable_code(StableErrorCode::IoReadFailed)
    })?;
    storage.get(id).map(|_| ()).map_err(|error| {
        CliError::fatal(format!("failed to load {kind} object '{id}': {error}"))
            .with_stable_code(StableErrorCode::RepoCorrupt)
    })
}

#[cfg(test)]
mod tests {
    use git_internal::{
        hash::{ObjectHash, get_hash_kind},
        internal::object::{
            commit::Commit,
            signature::{Signature, SignatureType},
        },
    };

    use super::{CommitNode, child_path};

    fn test_hash(byte: u8) -> ObjectHash {
        ObjectHash::from_bytes(&vec![byte; get_hash_kind().size()])
            .expect("test hash bytes should match active hash kind")
    }

    fn test_signature(timestamp: usize) -> Signature {
        Signature {
            signature_type: SignatureType::Committer,
            name: "tester".to_string(),
            email: "tester@example.com".to_string(),
            timestamp,
            timezone: "+0000".to_string(),
        }
    }

    fn test_node(byte: u8, timestamp: usize, sequence: usize) -> CommitNode {
        let id = test_hash(byte);
        CommitNode {
            commit: Commit {
                id,
                tree_id: id,
                parent_commit_ids: Vec::new(),
                author: test_signature(timestamp),
                committer: test_signature(timestamp),
                message: "test".to_string(),
            },
            sequence,
        }
    }

    #[test]
    fn commit_node_orders_newest_first_and_preserves_insert_order_ties() {
        let old = test_node(0x01, 1, 0);
        let new = test_node(0x02, 2, 1);
        assert!(new > old);

        let first = test_node(0x03, 2, 1);
        let second = test_node(0x04, 2, 2);
        assert!(first > second);
    }

    #[test]
    fn child_path_rejects_unsafe_components() {
        assert_eq!(child_path("", "file.txt").as_deref(), Some("file.txt"));
        assert_eq!(
            child_path("src", "file.txt").as_deref(),
            Some("src/file.txt")
        );
        assert!(child_path("", "").is_none());
        assert!(child_path("", ".").is_none());
        assert!(child_path("", "..").is_none());
        assert!(child_path("", "dir/../file").is_none());
    }
}
