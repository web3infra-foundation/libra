//! R2 upload helpers for publish snapshot artefacts.
//!
//! The snapshot builder produces pure data plans. This module is the
//! small I/O boundary that writes those plans into
//! [`PublishStorage`]: text file previews as raw bytes and the
//! revision `code-manifest.json` as JSON.

use crate::{
    internal::publish::snapshot::RevisionArtifactPlan,
    utils::storage::publish_storage::{PublishStorage, PublishStorageError},
};

/// Summary of R2 objects written for one revision artefact plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RevisionArtifactUploadSummary {
    pub code_manifest_key: String,
    pub text_blob_count: usize,
    pub text_blob_keys: Vec<String>,
}

/// Write a revision code snapshot to publish storage.
///
/// Binary, too-large and ignored files are intentionally not written
/// as R2 blobs; they are represented only in `code-manifest.json` and
/// later D1 file metadata rows.
pub async fn upload_revision_artifacts(
    storage: &PublishStorage,
    plan: &RevisionArtifactPlan,
) -> Result<RevisionArtifactUploadSummary, PublishStorageError> {
    let mut text_blob_keys = Vec::with_capacity(plan.text_blobs.len());
    for blob in &plan.text_blobs {
        storage.put_bytes(&blob.relative_key, &blob.bytes).await?;
        text_blob_keys.push(blob.object_key.clone());
    }
    storage
        .put_json(&plan.code_manifest_relative_key, &plan.code_manifest)
        .await?;

    Ok(RevisionArtifactUploadSummary {
        code_manifest_key: plan.code_manifest_key.clone(),
        text_blob_count: text_blob_keys.len(),
        text_blob_keys,
    })
}
