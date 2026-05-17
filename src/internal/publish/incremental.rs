//! Incremental publish upload planning.
//!
//! Full `publish sync` writes are still staged elsewhere, but this
//! module owns the idempotency decision for publish artefacts: upload
//! when a key is new or its digest changed; skip when the stored digest
//! already matches; upload everything under `--force`.

use std::collections::{BTreeMap, BTreeSet};

/// One desired publish artefact identified by its R2 key and
/// canonical lowercase sha256 digest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishArtifact {
    pub kind: PublishArtifactKind,
    pub key: String,
    pub sha256: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum PublishArtifactKind {
    TextFile,
    AiObject,
    AiBundle,
    Manifest,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IncrementalUploadPlan {
    pub uploads: Vec<PublishArtifact>,
    pub skipped: Vec<PublishArtifact>,
}

#[derive(Debug, thiserror::Error)]
pub enum IncrementalPlanError {
    #[error("publish artefact key {key:?} is planned more than once")]
    DuplicateKey { key: String },
}

/// Decide which artefacts must be uploaded for an idempotent sync.
pub fn plan_incremental_uploads(
    desired: Vec<PublishArtifact>,
    existing_sha256_by_key: &BTreeMap<String, String>,
    force: bool,
) -> Result<IncrementalUploadPlan, IncrementalPlanError> {
    let mut seen = BTreeSet::new();
    let mut uploads = Vec::new();
    let mut skipped = Vec::new();

    for artifact in desired {
        if !seen.insert(artifact.key.clone()) {
            return Err(IncrementalPlanError::DuplicateKey { key: artifact.key });
        }

        if !force
            && existing_sha256_by_key
                .get(&artifact.key)
                .is_some_and(|stored| stored == &artifact.sha256)
        {
            skipped.push(artifact);
        } else {
            uploads.push(artifact);
        }
    }

    uploads.sort_by_key(artifact_sort_key);
    skipped.sort_by_key(artifact_sort_key);

    Ok(IncrementalUploadPlan { uploads, skipped })
}

fn artifact_sort_key(artifact: &PublishArtifact) -> (PublishArtifactKind, String) {
    (artifact.kind, artifact.key.clone())
}

#[cfg(test)]
mod tests {
    use super::IncrementalPlanError;

    #[test]
    fn incremental_plan_error_display_pins_duplicate_key_template() {
        assert_eq!(
            IncrementalPlanError::DuplicateKey {
                key: "publish/snapshot.json".to_string(),
            }
            .to_string(),
            "publish artefact key \"publish/snapshot.json\" is planned more than once",
        );
    }
}
