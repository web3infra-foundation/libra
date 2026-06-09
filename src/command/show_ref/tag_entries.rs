use std::collections::HashSet;

use git_internal::{errors::GitError, hash::ObjectHash};

use super::ShowRefEntry;
use crate::{
    internal::tag,
    utils::error::{CliError, CliResult, StableErrorCode},
};

pub(super) async fn entries_for_loaded_tag(
    refname: &str,
    object: &tag::TagObject,
    dereference: bool,
) -> CliResult<Vec<ShowRefEntry>> {
    let mut entries = vec![ShowRefEntry {
        hash: tag_object_hash(object),
        refname: refname.to_string(),
    }];

    if dereference && let tag::TagObject::Tag(tag_object) = object {
        entries.push(ShowRefEntry {
            hash: peel_tag_object_hash(tag_object.object_hash, refname).await?,
            refname: format!("{refname}^{{}}"),
        });
    }

    Ok(entries)
}

pub(super) async fn entries_for_tag_target(
    refname: &str,
    target: &ObjectHash,
    dereference: bool,
) -> CliResult<Vec<ShowRefEntry>> {
    if !dereference {
        return Ok(vec![ShowRefEntry {
            hash: target.to_string(),
            refname: refname.to_string(),
        }]);
    }

    let object = tag::load_object_trait(target)
        .await
        .map_err(|error| tag_object_error(refname, error))?;
    entries_for_loaded_tag(refname, &object, true).await
}

fn tag_object_hash(object: &tag::TagObject) -> String {
    match object {
        tag::TagObject::Commit(commit) => commit.id.to_string(),
        tag::TagObject::Tag(tag) => tag.id.to_string(),
        tag::TagObject::Tree(tree) => tree.id.to_string(),
        tag::TagObject::Blob(blob) => blob.id.to_string(),
    }
}

async fn peel_tag_object_hash(mut object_hash: ObjectHash, refname: &str) -> CliResult<String> {
    let mut seen = HashSet::new();
    loop {
        if !seen.insert(object_hash) {
            return Err(CliError::fatal(format!(
                "detected cycle while dereferencing tag reference '{refname}'"
            ))
            .with_stable_code(StableErrorCode::RepoCorrupt));
        }

        match tag::load_object_trait(&object_hash)
            .await
            .map_err(|error| tag_object_error(refname, error))?
        {
            tag::TagObject::Commit(commit) => return Ok(commit.id.to_string()),
            tag::TagObject::Tree(tree) => return Ok(tree.id.to_string()),
            tag::TagObject::Blob(blob) => return Ok(blob.id.to_string()),
            tag::TagObject::Tag(tag) => object_hash = tag.object_hash,
        }
    }
}

fn tag_object_error(refname: &str, error: GitError) -> CliError {
    CliError::fatal(format!(
        "failed to dereference tag reference '{refname}': {error}"
    ))
    .with_stable_code(StableErrorCode::RepoCorrupt)
}
