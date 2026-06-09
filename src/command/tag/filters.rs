use std::{
    cmp::Ordering,
    collections::{HashSet, VecDeque},
};

use git_internal::{hash::ObjectHash, internal::object::commit::Commit};

use super::{TagError, TagListEntry};
use crate::command::{get_target_commit, load_object};

const MAX_GRAPH_VISITS: usize = 100_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TagSortKey {
    RefName,
    RefNameDesc,
    CreatorDate,
    CreatorDateDesc,
}

pub(super) fn parse_sort_key(key: &str) -> Result<TagSortKey, TagError> {
    match key {
        "refname" => Ok(TagSortKey::RefName),
        "-refname" => Ok(TagSortKey::RefNameDesc),
        "creatordate" | "taggerdate" => Ok(TagSortKey::CreatorDate),
        "-creatordate" | "-taggerdate" => Ok(TagSortKey::CreatorDateDesc),
        other => Err(TagError::InvalidSortKey(other.to_string())),
    }
}

pub(super) async fn resolve_commit_set(
    commits: &[String],
) -> Result<HashSet<ObjectHash>, TagError> {
    let mut set = HashSet::new();
    for commit in commits {
        let target = resolve_commitish(commit).await?;
        set.insert(target);
    }
    Ok(set)
}

pub(super) async fn resolve_reachable_set(baseline: &str) -> Result<HashSet<ObjectHash>, TagError> {
    collect_reachable(resolve_commitish(baseline).await?)
}

pub(super) fn commit_contains(
    tip: ObjectHash,
    targets: &HashSet<ObjectHash>,
) -> Result<bool, TagError> {
    if targets.is_empty() {
        return Ok(false);
    }
    let mut queue = VecDeque::from([tip]);
    let mut seen = HashSet::from([tip]);
    while let Some(current) = queue.pop_front() {
        if targets.contains(&current) {
            return Ok(true);
        }
        if seen.len() > MAX_GRAPH_VISITS {
            return Err(TagError::CommitLoadFailed {
                commit: tip.to_string(),
                detail: format!("history walk exceeded {MAX_GRAPH_VISITS} commits"),
            });
        }
        let commit = load_commit(current)?;
        for parent in commit.parent_commit_ids {
            if seen.insert(parent) {
                queue.push_back(parent);
            }
        }
    }
    Ok(false)
}

pub(super) fn sort_entries(entries: &mut [TagListEntry], key: TagSortKey) {
    match key {
        TagSortKey::RefName => entries.sort_by(|a, b| a.name.cmp(&b.name)),
        TagSortKey::RefNameDesc => entries.sort_by(|a, b| b.name.cmp(&a.name)),
        TagSortKey::CreatorDate => entries.sort_by(compare_creator_date),
        TagSortKey::CreatorDateDesc => entries.sort_by(|a, b| compare_creator_date(b, a)),
    }
}

async fn resolve_commitish(spec: &str) -> Result<ObjectHash, TagError> {
    get_target_commit(spec)
        .await
        .map_err(|_| TagError::InvalidFilterObject(spec.to_string()))
}

fn collect_reachable(tip: ObjectHash) -> Result<HashSet<ObjectHash>, TagError> {
    let mut queue = VecDeque::from([tip]);
    let mut seen = HashSet::from([tip]);
    while let Some(current) = queue.pop_front() {
        if seen.len() > MAX_GRAPH_VISITS {
            return Err(TagError::CommitLoadFailed {
                commit: tip.to_string(),
                detail: format!("history walk exceeded {MAX_GRAPH_VISITS} commits"),
            });
        }
        let commit = load_commit(current)?;
        for parent in commit.parent_commit_ids {
            if seen.insert(parent) {
                queue.push_back(parent);
            }
        }
    }
    Ok(seen)
}

fn load_commit(id: ObjectHash) -> Result<Commit, TagError> {
    load_object::<Commit>(&id).map_err(|error| TagError::CommitLoadFailed {
        commit: id.to_string(),
        detail: error.to_string(),
    })
}

fn compare_creator_date(a: &TagListEntry, b: &TagListEntry) -> Ordering {
    a.sort_time
        .cmp(&b.sort_time)
        .then_with(|| a.name.cmp(&b.name))
}
