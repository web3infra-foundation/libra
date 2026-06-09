//! Shared merge-base discovery for commands that need a Git-style best common ancestor.

use std::collections::{HashMap, HashSet, VecDeque};

use git_internal::{hash::ObjectHash, internal::object::commit::Commit};
use thiserror::Error;

use super::load_object;

#[derive(Debug, Error)]
pub(crate) enum MergeBaseError {
    #[error("failed to load commit '{commit_id}': {detail}")]
    Load { commit_id: String, detail: String },
    #[error("multiple best merge bases found ({bases}); criss-cross merge bases are unsupported")]
    Ambiguous { bases: String },
}

struct AncestorInfo {
    parents: Vec<ObjectHash>,
    distance: usize,
}

pub(crate) fn find_best_merge_base(
    lhs: ObjectHash,
    rhs: ObjectHash,
) -> Result<Option<ObjectHash>, MergeBaseError> {
    let best_ids = find_best_merge_bases(lhs, rhs)?;
    match best_ids.as_slice() {
        [] => Ok(None),
        [id] => Ok(Some(*id)),
        ids => Err(MergeBaseError::Ambiguous {
            bases: ids
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", "),
        }),
    }
}

pub(crate) fn find_best_merge_bases(
    lhs: ObjectHash,
    rhs: ObjectHash,
) -> Result<Vec<ObjectHash>, MergeBaseError> {
    let lhs_ancestors = collect_ancestors(lhs)?;
    let rhs_ancestors = collect_ancestors(rhs)?;
    let common: Vec<ObjectHash> = lhs_ancestors
        .keys()
        .filter(|id| rhs_ancestors.contains_key(id))
        .copied()
        .collect();

    if common.is_empty() {
        return Ok(Vec::new());
    }

    let parent_map: HashMap<ObjectHash, Vec<ObjectHash>> = lhs_ancestors
        .iter()
        .chain(rhs_ancestors.iter())
        .map(|(id, info)| (*id, info.parents.clone()))
        .collect();

    let mut best_ids: Vec<ObjectHash> = common
        .iter()
        .copied()
        .filter(|candidate| {
            !common
                .iter()
                .any(|other| other != candidate && commit_reaches(*other, *candidate, &parent_map))
        })
        .collect();

    best_ids.sort_by_key(|id| {
        let lhs = lhs_ancestors
            .get(id)
            .map(|info| info.distance)
            .unwrap_or(usize::MAX);
        let rhs = rhs_ancestors
            .get(id)
            .map(|info| info.distance)
            .unwrap_or(usize::MAX);
        (lhs.max(rhs), lhs + rhs, id.to_string())
    });

    Ok(best_ids)
}

fn collect_ancestors(
    start: ObjectHash,
) -> Result<HashMap<ObjectHash, AncestorInfo>, MergeBaseError> {
    let mut ancestors = HashMap::new();
    let mut queue = VecDeque::from([(start, 0usize)]);
    while let Some((id, distance)) = queue.pop_front() {
        if ancestors.contains_key(&id) {
            continue;
        }
        let commit: Commit = load_object(&id).map_err(|error| MergeBaseError::Load {
            commit_id: id.to_string(),
            detail: error.to_string(),
        })?;
        let parents = commit.parent_commit_ids;
        queue.extend(parents.iter().map(|parent| (*parent, distance + 1)));
        ancestors.insert(id, AncestorInfo { parents, distance });
    }
    Ok(ancestors)
}

fn commit_reaches(
    start: ObjectHash,
    target: ObjectHash,
    parent_map: &HashMap<ObjectHash, Vec<ObjectHash>>,
) -> bool {
    let mut seen = HashSet::new();
    let mut queue = VecDeque::from([start]);
    while let Some(id) = queue.pop_front() {
        if id == target {
            return true;
        }
        if !seen.insert(id) {
            continue;
        }
        if let Some(parents) = parent_map.get(&id) {
            queue.extend(parents.iter().copied());
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_reaches_follows_parent_edges() {
        let a = ObjectHash::new(&[1u8; 20]);
        let b = ObjectHash::new(&[2u8; 20]);
        let c = ObjectHash::new(&[3u8; 20]);
        let parent_map = HashMap::from([(c, vec![b]), (b, vec![a]), (a, vec![])]);

        assert!(commit_reaches(c, a, &parent_map));
        assert!(!commit_reaches(a, c, &parent_map));
    }
}
