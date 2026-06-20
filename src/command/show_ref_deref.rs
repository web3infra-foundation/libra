use std::collections::HashSet;

use git_internal::{errors::GitError, hash::ObjectHash};

use crate::{
    command::show_ref::ShowRefEntry,
    internal::tag,
    utils::error::{CliError, CliResult, StableErrorCode},
};

pub(crate) async fn tag_entries(tag: tag::Tag, dereference: bool) -> CliResult<Vec<ShowRefEntry>> {
    let refname = format!("refs/tags/{}", tag.name);
    let hash = tag_object_hash(&tag.object);
    let peeled_hash = if dereference {
        match &tag.object {
            tag::TagObject::Tag(tag_object) => {
                Some(peel_tag_object_hash(tag_object.object_hash, &refname).await?)
            }
            tag::TagObject::Commit(_) | tag::TagObject::Tree(_) | tag::TagObject::Blob(_) => None,
        }
    } else {
        None
    };

    let mut entries = vec![ShowRefEntry {
        hash,
        refname: refname.clone(),
    }];
    if let Some(hash) = peeled_hash {
        entries.push(ShowRefEntry {
            hash,
            refname: format!("{refname}^{{}}"),
        });
    }
    Ok(entries)
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
            return Err(
                CliError::fatal(format!("detected cycle while peeling tag '{refname}'"))
                    .with_stable_code(StableErrorCode::RepoCorrupt),
            );
        }

        match tag::load_object_trait(&object_hash)
            .await
            .map_err(|error| tag_peel_error(refname, error))?
        {
            tag::TagObject::Commit(commit) => return Ok(commit.id.to_string()),
            tag::TagObject::Tag(tag) => object_hash = tag.object_hash,
            tag::TagObject::Tree(tree) => return Ok(tree.id.to_string()),
            tag::TagObject::Blob(blob) => return Ok(blob.id.to_string()),
        }
    }
}

fn tag_peel_error(refname: &str, error: GitError) -> CliError {
    CliError::fatal(format!("failed to peel annotated tag '{refname}': {error}"))
        .with_stable_code(StableErrorCode::RepoCorrupt)
}
