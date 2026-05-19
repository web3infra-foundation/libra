use std::collections::BTreeMap;

use libra::internal::publish::incremental::{
    IncrementalPlanError, PublishArtifact, PublishArtifactKind, plan_incremental_uploads,
};

#[test]
fn publish_incremental_test_skips_unchanged_files_ai_objects_and_bundles() {
    let desired = vec![
        artifact(
            PublishArtifactKind::TextFile,
            "repo/publish/sites/site/revisions/rev/files/file.txt",
            "file-sha",
        ),
        artifact(
            PublishArtifactKind::AiObject,
            "repo/publish/sites/site/revisions/rev/ai/objects/snapshot/Run/run-1.json",
            "object-sha",
        ),
        artifact(
            PublishArtifactKind::AiBundle,
            "repo/publish/sites/site/revisions/rev/ai/bundles/ai-version-1.json",
            "bundle-sha",
        ),
    ];
    let existing = BTreeMap::from([
        (desired[0].key.clone(), desired[0].sha256.clone()),
        (desired[1].key.clone(), desired[1].sha256.clone()),
        (desired[2].key.clone(), desired[2].sha256.clone()),
    ]);

    let plan = plan_incremental_uploads(desired, &existing, false).unwrap();

    assert!(plan.uploads.is_empty());
    assert_eq!(plan.skipped.len(), 3);
    assert!(
        plan.skipped
            .iter()
            .any(|artifact| artifact.kind == PublishArtifactKind::TextFile)
    );
    assert!(
        plan.skipped
            .iter()
            .any(|artifact| artifact.kind == PublishArtifactKind::AiObject)
    );
    assert!(
        plan.skipped
            .iter()
            .any(|artifact| artifact.kind == PublishArtifactKind::AiBundle)
    );
}

#[test]
fn publish_incremental_test_uploads_changed_or_missing_artifacts() {
    let desired = vec![
        artifact(PublishArtifactKind::TextFile, "files/unchanged.txt", "same"),
        artifact(
            PublishArtifactKind::AiObject,
            "ai/objects/Run/run-1.json",
            "new",
        ),
        artifact(
            PublishArtifactKind::AiBundle,
            "ai/bundles/version.json",
            "missing",
        ),
    ];
    let existing = BTreeMap::from([
        ("files/unchanged.txt".to_string(), "same".to_string()),
        ("ai/objects/Run/run-1.json".to_string(), "old".to_string()),
    ]);

    let plan = plan_incremental_uploads(desired, &existing, false).unwrap();

    assert_eq!(
        plan.skipped
            .iter()
            .map(|artifact| artifact.key.as_str())
            .collect::<Vec<_>>(),
        vec!["files/unchanged.txt"]
    );
    assert_eq!(
        plan.uploads
            .iter()
            .map(|artifact| artifact.key.as_str())
            .collect::<Vec<_>>(),
        vec!["ai/objects/Run/run-1.json", "ai/bundles/version.json"]
    );
}

#[test]
fn publish_incremental_test_force_uploads_unchanged_artifacts() {
    let desired = vec![
        artifact(PublishArtifactKind::TextFile, "files/unchanged.txt", "same"),
        artifact(
            PublishArtifactKind::AiObject,
            "ai/objects/Run/run-1.json",
            "same",
        ),
    ];
    let existing = desired
        .iter()
        .map(|artifact| (artifact.key.clone(), artifact.sha256.clone()))
        .collect::<BTreeMap<_, _>>();

    let plan = plan_incremental_uploads(desired, &existing, true).unwrap();

    assert!(plan.skipped.is_empty());
    assert_eq!(plan.uploads.len(), 2);
}

#[test]
fn publish_incremental_test_rejects_duplicate_artifact_keys() {
    let desired = vec![
        artifact(PublishArtifactKind::TextFile, "files/dup.txt", "first"),
        artifact(PublishArtifactKind::TextFile, "files/dup.txt", "second"),
    ];

    let err = plan_incremental_uploads(desired, &BTreeMap::new(), false)
        .expect_err("duplicate keys must be rejected");

    assert!(matches!(
        err,
        IncrementalPlanError::DuplicateKey { key } if key == "files/dup.txt"
    ));
}

fn artifact(kind: PublishArtifactKind, key: &str, sha256: &str) -> PublishArtifact {
    PublishArtifact {
        kind,
        key: key.to_string(),
        sha256: sha256.to_string(),
    }
}
