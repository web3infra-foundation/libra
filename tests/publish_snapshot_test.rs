use std::sync::Arc;

use chrono::{TimeZone, Utc};
use libra::{
    internal::publish::{
        contract::{FileDisplayMode, PUBLISH_SCHEMA_VERSION, PublishCodeManifest},
        snapshot::{
            FileSnapshot, IgnoredReason, RevisionPlan, publish_code_manifest_key,
            publish_text_file_key,
        },
    },
    utils::storage::publish_storage::PublishStorage,
};
use object_store::memory::InMemory;

#[test]
fn publish_snapshot_test_revision_plan_emits_code_manifest_contract() {
    let repo_id = "11111111-2222-3333-4444-555555555555";
    let site_id = "00000000-0000-0000-0000-0000publish01";
    let revision_oid = "abcdef0123456789abcdef0123456789abcdef01";
    let text_sha = "9a0364b9e99bb480dd25e1f0284c8555a30dca56ab59e10e8a95da4c6f97c5e8";
    let generated_at = Utc
        .with_ymd_and_hms(2026, 5, 13, 12, 0, 0)
        .single()
        .expect("test timestamp must be valid");
    let plan = RevisionPlan {
        revision_oid: revision_oid.to_string(),
        commit_oid: revision_oid.to_string(),
        tree_oid: "1234567812345678123456781234567812345678".to_string(),
        generated_at,
        files: vec![
            FileSnapshot::Text {
                path: "src/lib.rs".to_string(),
                size_bytes: 4096,
                content_sha256: text_sha.to_string(),
                language: Some("rust".to_string()),
            },
            FileSnapshot::Binary {
                path: "assets/logo.png".to_string(),
                size_bytes: 16384,
            },
            FileSnapshot::TooLarge {
                path: "data/big.bin".to_string(),
                size_bytes: 2 * 1024 * 1024,
            },
            FileSnapshot::Ignored {
                path: ".env.local".to_string(),
                size_bytes: 42,
                reason: IgnoredReason::BuiltinCredential,
            },
        ],
    };

    let manifest = plan.to_code_manifest(repo_id, site_id);

    assert_eq!(manifest.schema_version, PUBLISH_SCHEMA_VERSION);
    assert_eq!(manifest.site_id, site_id);
    assert_eq!(manifest.revision_oid, revision_oid);
    assert_eq!(manifest.commit_oid, revision_oid);
    assert_eq!(
        manifest.tree_oid,
        "1234567812345678123456781234567812345678"
    );
    assert_eq!(manifest.generated_at, generated_at);
    assert_eq!(manifest.files.len(), 4);
    assert_eq!(plan.total_file_count(), 4);
    assert_eq!(plan.r2_blob_count(), 1);

    let text_file = &manifest.files[0];
    assert_eq!(text_file.path, "src/lib.rs");
    assert_eq!(text_file.display_mode, FileDisplayMode::Text);
    assert_eq!(text_file.content_sha256.as_deref(), Some(text_sha));
    let text_key = publish_text_file_key(repo_id, site_id, revision_oid, text_sha);
    assert_eq!(text_file.r2_key.as_deref(), Some(text_key.as_str()));
    assert_eq!(text_file.language.as_deref(), Some("rust"));

    for metadata_only in &manifest.files[1..] {
        assert!(
            matches!(
                metadata_only.display_mode,
                FileDisplayMode::Binary | FileDisplayMode::TooLarge | FileDisplayMode::Ignored
            ),
            "metadata-only file must use a non-text display mode"
        );
        assert_eq!(metadata_only.content_sha256, None);
        assert_eq!(metadata_only.r2_key, None);
    }

    let raw = serde_json::to_value(&manifest).expect("manifest must serialize");
    let parsed: PublishCodeManifest =
        serde_json::from_value(raw).expect("manifest must satisfy the contract shape");
    assert_eq!(parsed, manifest);
    assert_eq!(
        publish_code_manifest_key(repo_id, site_id, revision_oid),
        format!("{repo_id}/publish/sites/{site_id}/revisions/{revision_oid}/code-manifest.json")
    );
}

#[tokio::test]
async fn publish_snapshot_test_mock_r2_d1_snapshot_round_trip() {
    let repo_id = "11111111-2222-3333-4444-555555555555";
    let site_id = "00000000-0000-0000-0000-0000publish01";
    let revision_oid = "abcdef0123456789abcdef0123456789abcdef01";
    let text_body = b"pub fn demo() {}\n";
    let text_sha = libra::internal::publish::snapshot::sha256_hex(text_body);
    let generated_at = Utc
        .with_ymd_and_hms(2026, 5, 13, 12, 0, 0)
        .single()
        .expect("test timestamp must be valid");
    let plan = RevisionPlan {
        revision_oid: revision_oid.to_string(),
        commit_oid: revision_oid.to_string(),
        tree_oid: "1234567812345678123456781234567812345678".to_string(),
        generated_at,
        files: vec![
            FileSnapshot::Text {
                path: "src/lib.rs".to_string(),
                size_bytes: text_body.len() as u64,
                content_sha256: text_sha.clone(),
                language: Some("rust".to_string()),
            },
            FileSnapshot::Binary {
                path: "assets/logo.png".to_string(),
                size_bytes: 128,
            },
            FileSnapshot::TooLarge {
                path: "target/big.log".to_string(),
                size_bytes: 2 * 1024 * 1024,
            },
            FileSnapshot::Ignored {
                path: ".env.local".to_string(),
                size_bytes: 64,
                reason: IgnoredReason::BuiltinCredential,
            },
        ],
    };
    let storage = PublishStorage::new(Arc::new(InMemory::new()), repo_id, site_id)
        .expect("mock R2 storage should be constructed");
    let manifest = plan.to_code_manifest(repo_id, site_id);
    let manifest_relative = format!("revisions/{revision_oid}/code-manifest.json");
    let text_relative = format!("revisions/{revision_oid}/files/{text_sha}.txt");

    storage
        .put_json(&manifest_relative, &manifest)
        .await
        .expect("manifest should write to mock R2");
    storage
        .put_bytes(&text_relative, text_body)
        .await
        .expect("text body should write to mock R2");

    assert_eq!(
        storage.key_for(&manifest_relative).unwrap(),
        publish_code_manifest_key(repo_id, site_id, revision_oid)
    );
    let loaded_manifest: PublishCodeManifest = storage
        .get_json(&manifest_relative)
        .await
        .expect("manifest should round-trip from mock R2");
    assert_eq!(loaded_manifest, manifest);

    let mock_d1_rows = loaded_manifest
        .files
        .iter()
        .map(|file| {
            (
                file.path.as_str(),
                file.display_mode,
                file.content_sha256.as_deref(),
                file.r2_key.as_deref(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(mock_d1_rows.len(), 4);

    let text_row = mock_d1_rows
        .iter()
        .find(|row| row.1 == FileDisplayMode::Text)
        .expect("text file row should exist");
    assert_eq!(text_row.2, Some(text_sha.as_str()));
    assert_eq!(
        text_row.3,
        Some(publish_text_file_key(repo_id, site_id, revision_oid, &text_sha).as_str())
    );
    assert_eq!(
        storage
            .get_bytes(&text_relative)
            .await
            .expect("text body should round-trip from mock R2"),
        text_body
    );

    for row in mock_d1_rows
        .iter()
        .filter(|row| row.1 != FileDisplayMode::Text)
    {
        assert_eq!(
            row.2, None,
            "metadata-only row has no content hash: {row:?}"
        );
        assert_eq!(row.3, None, "metadata-only row has no R2 key: {row:?}");
    }
}
