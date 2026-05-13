use std::sync::Arc;

use chrono::{TimeZone, Utc};
use libra::{
    internal::publish::{
        contract::{PublishCodeManifest, PublishRefsIndex, PublishSiteLatest, RedactionMode},
        snapshot::{
            FileSnapshot, RefInput, RevisionFileInput, RevisionPlan, SnapshotConfig,
            build_revision_artifact_plan, build_snapshot_plan, publish_code_manifest_relative_key,
            publish_text_file_relative_key, sha256_hex,
        },
        upload::{
            PUBLISH_REFS_INDEX_RELATIVE_KEY, PUBLISH_SITE_LATEST_RELATIVE_KEY,
            build_revision_d1_rows, build_site_index_artifacts, upload_revision_artifacts,
            upload_site_index_artifacts,
        },
    },
    utils::storage::publish_storage::{PublishStorage, PublishStorageError},
};
use object_store::memory::InMemory;

#[tokio::test]
async fn publish_upload_test_writes_manifest_and_text_blobs_only() {
    let repo_id = "11111111-2222-3333-4444-555555555555";
    let site_id = "00000000-0000-0000-0000-0000publish01";
    let revision_oid = "abcdef0123456789abcdef0123456789abcdef01";
    let tree_oid = "1234567812345678123456781234567812345678";
    let generated_at = Utc
        .with_ymd_and_hms(2026, 5, 13, 12, 0, 0)
        .single()
        .expect("test timestamp must be valid");
    let mut config = SnapshotConfig {
        max_preview_bytes: 8,
        ..SnapshotConfig::default()
    };
    config.preflight.extend_with_ignore_text("ignored.txt\n");
    let text_body = b"fn x()\n";
    let text_sha = sha256_hex(text_body);
    let artifact = build_revision_artifact_plan(
        repo_id,
        site_id,
        revision_oid,
        revision_oid,
        tree_oid,
        generated_at,
        vec![
            RevisionFileInput {
                path: "src/lib.rs".to_string(),
                bytes: text_body.to_vec(),
            },
            RevisionFileInput {
                path: "assets/logo.bin".to_string(),
                bytes: vec![0, 1, 2],
            },
            RevisionFileInput {
                path: "docs/large.md".to_string(),
                bytes: b"123456789".to_vec(),
            },
            RevisionFileInput {
                path: "ignored.txt".to_string(),
                bytes: b"secret".to_vec(),
            },
        ],
        &config,
    )
    .expect("artifact plan should build");
    let storage = PublishStorage::new(Arc::new(InMemory::new()), repo_id, site_id)
        .expect("mock R2 storage should be constructed");

    let summary = upload_revision_artifacts(&storage, &artifact)
        .await
        .expect("artifact upload should succeed");

    assert_eq!(summary.code_manifest_key, artifact.code_manifest_key);
    assert_eq!(summary.text_blob_count, 1);
    assert_eq!(
        summary.text_blob_keys,
        vec![artifact.text_blobs[0].object_key.clone()]
    );

    let manifest_relative = publish_code_manifest_relative_key(revision_oid);
    let loaded: PublishCodeManifest = storage
        .get_json(&manifest_relative)
        .await
        .expect("manifest should be written to mock R2");
    assert_eq!(loaded, artifact.code_manifest);

    let text_relative = publish_text_file_relative_key(revision_oid, &text_sha);
    assert_eq!(
        storage
            .get_bytes(&text_relative)
            .await
            .expect("text preview should be written to mock R2"),
        text_body
    );

    for metadata_only in [
        "revisions/abcdef0123456789abcdef0123456789abcdef01/files/assets-logo.bin",
        "revisions/abcdef0123456789abcdef0123456789abcdef01/files/docs-large.md",
        "revisions/abcdef0123456789abcdef0123456789abcdef01/files/ignored.txt",
    ] {
        assert!(
            matches!(
                storage.get_bytes(metadata_only).await,
                Err(PublishStorageError::NotFound { .. })
            ),
            "metadata-only path must not create an R2 blob: {metadata_only}"
        );
    }
}

#[tokio::test]
async fn publish_upload_test_builds_and_uploads_site_index_artifacts() {
    let site_id = "00000000-0000-0000-0000-0000publish01";
    let repo_id = "11111111-2222-3333-4444-555555555555";
    let main_revision = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let tag_revision = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let generated_at = Utc
        .with_ymd_and_hms(2026, 5, 13, 12, 0, 0)
        .single()
        .expect("test timestamp must be valid");
    let plan = build_snapshot_plan(
        &[
            RefInput {
                ref_name: "refs/heads/main".to_string(),
                target_oid: main_revision.to_string(),
                revision_oid: main_revision.to_string(),
            },
            RefInput {
                ref_name: "refs/tags/v1.0.0".to_string(),
                target_oid: tag_revision.to_string(),
                revision_oid: tag_revision.to_string(),
            },
        ],
        vec![
            revision_plan(
                main_revision,
                "1111111111111111111111111111111111111111",
                generated_at,
            ),
            revision_plan(
                tag_revision,
                "2222222222222222222222222222222222222222",
                generated_at,
            ),
        ],
        Some("refs/heads/main"),
    )
    .expect("snapshot plan should build");

    let artifacts = build_site_index_artifacts(&plan, site_id, "sync-run-1", 7, generated_at)
        .expect("site index artifacts should build");

    assert_eq!(artifacts.latest.site_id, site_id);
    assert_eq!(artifacts.latest.default_ref, "refs/heads/main");
    assert_eq!(artifacts.latest.latest_revision_oid, main_revision);
    assert_eq!(artifacts.latest.refs_generation, 7);
    assert_eq!(artifacts.refs_index.refs.len(), 2);
    assert_eq!(artifacts.refs_index.default_ref, "refs/heads/main");
    assert_eq!(artifacts.ref_rows.len(), 2);

    let main_row = artifacts
        .ref_rows
        .iter()
        .find(|row| row.ref_name == "refs/heads/main")
        .expect("main ref row should exist");
    assert_eq!(main_row.ref_type, "branch");
    assert_eq!(main_row.short_name, "main");
    assert_eq!(main_row.revision_oid, main_revision);
    assert_eq!(main_row.is_default, 1);
    assert_eq!(main_row.sync_run_id, "sync-run-1");

    let tag_row = artifacts
        .ref_rows
        .iter()
        .find(|row| row.ref_name == "refs/tags/v1.0.0")
        .expect("tag ref row should exist");
    assert_eq!(tag_row.ref_type, "tag");
    assert_eq!(tag_row.short_name, "v1.0.0");
    assert_eq!(tag_row.is_default, 0);

    let storage = PublishStorage::new(Arc::new(InMemory::new()), repo_id, site_id)
        .expect("mock R2 storage should be constructed");
    upload_site_index_artifacts(&storage, &artifacts)
        .await
        .expect("site indexes should upload to mock R2");
    let refs_index: PublishRefsIndex = storage
        .get_json(PUBLISH_REFS_INDEX_RELATIVE_KEY)
        .await
        .expect("refs.json should be written");
    assert_eq!(refs_index, artifacts.refs_index);
    let latest: PublishSiteLatest = storage
        .get_json(PUBLISH_SITE_LATEST_RELATIVE_KEY)
        .await
        .expect("latest.json should be written");
    assert_eq!(latest, artifacts.latest);
}

fn revision_plan(
    revision_oid: &str,
    tree_oid: &str,
    generated_at: chrono::DateTime<chrono::Utc>,
) -> RevisionPlan {
    RevisionPlan {
        revision_oid: revision_oid.to_string(),
        commit_oid: revision_oid.to_string(),
        tree_oid: tree_oid.to_string(),
        files: Vec::<FileSnapshot>::new(),
        generated_at,
    }
}

#[test]
fn publish_upload_test_builds_d1_revision_and_file_rows() {
    let repo_id = "11111111-2222-3333-4444-555555555555";
    let site_id = "00000000-0000-0000-0000-0000publish01";
    let revision_oid = "abcdef0123456789abcdef0123456789abcdef01";
    let tree_oid = "1234567812345678123456781234567812345678";
    let generated_at = Utc
        .with_ymd_and_hms(2026, 5, 13, 12, 0, 0)
        .single()
        .expect("test timestamp must be valid");
    let mut config = SnapshotConfig {
        max_preview_bytes: 8,
        ..SnapshotConfig::default()
    };
    config.preflight.extend_with_ignore_text("ignored.txt\n");
    let text_body = b"fn x()\n";
    let text_sha = sha256_hex(text_body);
    let artifact = build_revision_artifact_plan(
        repo_id,
        site_id,
        revision_oid,
        revision_oid,
        tree_oid,
        generated_at,
        vec![
            RevisionFileInput {
                path: "src/lib.rs".to_string(),
                bytes: text_body.to_vec(),
            },
            RevisionFileInput {
                path: "assets/logo.bin".to_string(),
                bytes: vec![0, 1, 2],
            },
            RevisionFileInput {
                path: "docs/large.md".to_string(),
                bytes: b"123456789".to_vec(),
            },
            RevisionFileInput {
                path: "ignored.txt".to_string(),
                bytes: b"secret".to_vec(),
            },
        ],
        &config,
    )
    .expect("artifact plan should build");

    let rows = build_revision_d1_rows(
        &artifact,
        "sync-run-1",
        RedactionMode::Default,
        "2026.05.13-1",
    )
    .expect("D1 rows should build");

    assert_eq!(rows.revision.site_id, site_id);
    assert_eq!(rows.revision.revision_oid, revision_oid);
    assert_eq!(rows.revision.status, "published");
    assert_eq!(
        rows.revision.code_manifest_key.as_deref(),
        Some(artifact.code_manifest_key.as_str())
    );
    assert_eq!(rows.revision.ai_index_key, None);
    assert_eq!(rows.revision.file_count, 4);
    assert_eq!(rows.revision.ai_object_count, 0);
    assert_eq!(rows.revision.ai_bundle_count, 0);
    assert_eq!(rows.revision.redaction_mode, "default");
    assert_eq!(rows.revision.redaction_rules_version, "2026.05.13-1");
    assert_eq!(rows.revision.sync_run_id, "sync-run-1");
    assert_eq!(rows.revision.created_at, "2026-05-13T12:00:00+00:00");
    assert_eq!(rows.revision.updated_at, rows.revision.created_at);

    let text_row = rows
        .files
        .iter()
        .find(|row| row.path == "src/lib.rs")
        .expect("text row should exist");
    assert_eq!(text_row.display_mode, "text");
    assert_eq!(text_row.content_sha256.as_deref(), Some(text_sha.as_str()));
    assert_eq!(
        text_row.r2_key.as_deref(),
        Some(artifact.text_blobs[0].object_key.as_str())
    );
    assert_eq!(text_row.language.as_deref(), Some("rust"));

    for metadata_only in rows.files.iter().filter(|row| row.path != "src/lib.rs") {
        assert!(
            ["binary", "too_large", "ignored"].contains(&metadata_only.display_mode.as_str()),
            "unexpected metadata-only display mode: {metadata_only:?}"
        );
        assert_eq!(metadata_only.content_sha256, None);
        assert_eq!(metadata_only.r2_key, None);
    }
}
