//! Merge-base computation over the commit graph: the lowest common ancestors
//! (LCAs) of two commits.
//!
//! This is the single, correct implementation behind `libra merge-base` and the
//! `diff A...B` three-dot range. Unlike the older first-found walks in
//! `log.rs` / `rebase.rs`, it returns true LCAs: a common ancestor is a merge
//! base only when it is not a *strict* ancestor of another common ancestor, so
//! criss-cross histories yield every maximal common ancestor (with `--all`) and
//! a deterministic single base otherwise.
//!
//! Migrating `log.rs` / `rebase.rs` onto this module (with golden-output
//! regression and a legacy toggle) is tracked as a follow-up; this module is
//! self-contained and does not change those call sites yet.

use std::collections::{HashMap, HashSet, VecDeque};

use git_internal::{hash::ObjectHash, internal::object::commit::Commit};

use crate::utils::object_ext::CommitExt;

/// Error raised when a commit in the graph cannot be loaded.
#[derive(Debug, thiserror::Error)]
pub enum MergeBaseError {
    /// A commit object could not be loaded (missing, corrupt, or not a commit).
    #[error("failed to load commit {0}")]
    Load(String),
}

/// Lazily-loaded parent adjacency, so each commit is read at most once.
struct CommitGraph {
    parents: HashMap<ObjectHash, Vec<ObjectHash>>,
}

impl CommitGraph {
    fn new() -> Self {
        Self {
            parents: HashMap::new(),
        }
    }

    /// Parent ids of `id`, loading and caching the commit on first access.
    fn parents_of(&mut self, id: &ObjectHash) -> Result<Vec<ObjectHash>, MergeBaseError> {
        if let Some(parents) = self.parents.get(id) {
            return Ok(parents.clone());
        }
        let commit: Commit =
            Commit::try_load(id).ok_or_else(|| MergeBaseError::Load(id.to_string()))?;
        let parents = commit.parent_commit_ids.clone();
        self.parents.insert(*id, parents.clone());
        Ok(parents)
    }

    /// All ancestors of `start`, inclusive of `start` itself.
    fn ancestors(&mut self, start: &ObjectHash) -> Result<HashSet<ObjectHash>, MergeBaseError> {
        let mut seen = HashSet::new();
        let mut queue = VecDeque::new();
        queue.push_back(*start);
        while let Some(id) = queue.pop_front() {
            if !seen.insert(id) {
                continue;
            }
            for parent in self.parents_of(&id)? {
                queue.push_back(parent);
            }
        }
        Ok(seen)
    }
}

/// Every lowest common ancestor of `a` and `b`, sorted deterministically by hex
/// id. Empty when the two commits share no history.
pub fn merge_bases(a: &ObjectHash, b: &ObjectHash) -> Result<Vec<ObjectHash>, MergeBaseError> {
    let mut graph = CommitGraph::new();
    let ancestors_a = graph.ancestors(a)?;
    let ancestors_b = graph.ancestors(b)?;
    let common: HashSet<ObjectHash> = ancestors_a.intersection(&ancestors_b).copied().collect();
    if common.is_empty() {
        return Ok(Vec::new());
    }

    // A common ancestor is dominated (not an LCA) when it is a *strict*
    // ancestor of another common ancestor.
    let mut dominated: HashSet<ObjectHash> = HashSet::new();
    for start in &common {
        let mut local_seen = HashSet::new();
        let mut queue: VecDeque<ObjectHash> = graph.parents_of(start)?.into_iter().collect();
        while let Some(id) = queue.pop_front() {
            if !local_seen.insert(id) {
                continue;
            }
            if common.contains(&id) {
                dominated.insert(id);
            }
            for parent in graph.parents_of(&id)? {
                queue.push_back(parent);
            }
        }
    }

    let mut lcas: Vec<ObjectHash> = common
        .into_iter()
        .filter(|id| !dominated.contains(id))
        .collect();
    lcas.sort_by_key(|id| id.to_string());
    Ok(lcas)
}

/// A single "best" merge base of `a` and `b` (the lowest-hex LCA for
/// determinism), or `None` when there is no common ancestor.
pub fn merge_base(a: &ObjectHash, b: &ObjectHash) -> Result<Option<ObjectHash>, MergeBaseError> {
    Ok(merge_bases(a, b)?.into_iter().next())
}

/// Whether `ancestor` is an ancestor of `descendant`. Reflexive: a commit is its
/// own ancestor, matching `git merge-base --is-ancestor X X` (exit 0).
pub fn is_ancestor(ancestor: &ObjectHash, descendant: &ObjectHash) -> Result<bool, MergeBaseError> {
    let mut graph = CommitGraph::new();
    Ok(graph.ancestors(descendant)?.contains(ancestor))
}
