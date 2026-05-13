//! R2 upload helpers for publish snapshot artefacts.
//!
//! The snapshot builder produces pure data plans. This module is the
//! small I/O boundary that writes those plans into
//! [`PublishStorage`]: text file previews as raw bytes and the
//! revision `code-manifest.json` as JSON.

use crate::{
    internal::publish::{
        contract::{FileDisplayMode, PUBLISH_SCHEMA_VERSION, RedactionMode},
        snapshot::RevisionArtifactPlan,
    },
    utils::{
        d1_client::{PublishFileRow, PublishRevisionRow},
        storage::publish_storage::{PublishStorage, PublishStorageError},
    },
};

/// Summary of R2 objects written for one revision artefact plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RevisionArtifactUploadSummary {
    pub code_manifest_key: String,
    pub text_blob_count: usize,
    pub text_blob_keys: Vec<String>,
}

/// D1 rows generated for one revision artefact plan.
#[derive(Clone, Debug)]
pub struct RevisionD1Rows {
    pub revision: PublishRevisionRow,
    pub files: Vec<PublishFileRow>,
}

/// Errors surfaced while converting artefact plans into D1 row values.
#[derive(Debug, thiserror::Error)]
pub enum RevisionD1RowsError {
    #[error("publish revision file count {count} exceeds D1 integer range")]
    FileCountTooLarge { count: usize },
    #[error("publish file {path:?} has size {size_bytes} which exceeds D1 integer range")]
    FileSizeTooLarge { path: String, size_bytes: u64 },
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

/// Build the D1 `publish_revisions` and `publish_files` rows for a
/// code-only revision snapshot.
///
/// The returned rows are ready for `D1Client::upsert_publish_revision`
/// and `D1Client::upsert_publish_file`. AI object/version rows are
/// deliberately not produced here; the AI exporter owns that surface.
pub fn build_revision_d1_rows(
    plan: &RevisionArtifactPlan,
    sync_run_id: &str,
    redaction_mode: RedactionMode,
    redaction_rules_version: &str,
) -> Result<RevisionD1Rows, RevisionD1RowsError> {
    let timestamp = plan.revision.generated_at.to_rfc3339();
    let file_count = i64::try_from(plan.code_manifest.files.len()).map_err(|_| {
        RevisionD1RowsError::FileCountTooLarge {
            count: plan.code_manifest.files.len(),
        }
    })?;
    let revision = PublishRevisionRow {
        site_id: plan.code_manifest.site_id.clone(),
        revision_oid: plan.revision.revision_oid.clone(),
        status: "published".to_string(),
        code_manifest_key: Some(plan.code_manifest_key.clone()),
        ai_index_key: None,
        file_count,
        ai_object_count: 0,
        ai_bundle_count: 0,
        redaction_mode: redaction_mode_label(redaction_mode).to_string(),
        redaction_rules_version: redaction_rules_version.to_string(),
        sync_run_id: sync_run_id.to_string(),
        schema_version: i64::from(PUBLISH_SCHEMA_VERSION),
        created_at: timestamp.clone(),
        updated_at: timestamp,
    };

    let mut files = Vec::with_capacity(plan.code_manifest.files.len());
    for file in &plan.code_manifest.files {
        let size_bytes =
            i64::try_from(file.size_bytes).map_err(|_| RevisionD1RowsError::FileSizeTooLarge {
                path: file.path.clone(),
                size_bytes: file.size_bytes,
            })?;
        files.push(PublishFileRow {
            site_id: file.site_id.clone(),
            revision_oid: file.revision_oid.clone(),
            path: file.path.clone(),
            display_mode: file_display_mode_label(file.display_mode).to_string(),
            content_sha256: file.content_sha256.clone(),
            r2_key: file.r2_key.clone(),
            size_bytes,
            language: file.language.clone(),
            schema_version: i64::from(PUBLISH_SCHEMA_VERSION),
        });
    }

    Ok(RevisionD1Rows { revision, files })
}

fn redaction_mode_label(mode: RedactionMode) -> &'static str {
    match mode {
        RedactionMode::Default => "default",
        RedactionMode::Strict => "strict",
    }
}

fn file_display_mode_label(mode: FileDisplayMode) -> &'static str {
    match mode {
        FileDisplayMode::Text => "text",
        FileDisplayMode::Binary => "binary",
        FileDisplayMode::TooLarge => "too_large",
        FileDisplayMode::Ignored => "ignored",
    }
}
