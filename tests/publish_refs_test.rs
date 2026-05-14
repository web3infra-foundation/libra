use chrono::Utc;
use libra::internal::publish::{
    contract::{PUBLISH_SCHEMA_VERSION, RefType},
    snapshot::{FileSnapshot, RefInput, RevisionPlan, build_snapshot_plan, publish_refs_index_key},
};

fn revision(revision_oid: &str, tree_oid: &str) -> RevisionPlan {
    RevisionPlan {
        revision_oid: revision_oid.to_string(),
        commit_oid: revision_oid.to_string(),
        tree_oid: tree_oid.to_string(),
        files: vec![FileSnapshot::Text {
            path: "README.md".to_string(),
            size_bytes: 7,
            content_sha256: "f".repeat(64),
            language: Some("markdown".to_string()),
        }],
        generated_at: Utc::now(),
    }
}

fn publish_ref(ref_name: &str, target_oid: &str, revision_oid: &str) -> RefInput {
    RefInput {
        ref_name: ref_name.to_string(),
        target_oid: target_oid.to_string(),
        revision_oid: revision_oid.to_string(),
    }
}

#[test]
fn publish_refs_test_all_refs_share_unique_revision_snapshots() {
    let main_oid = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let dev_oid = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let main_tree = "cccccccccccccccccccccccccccccccccccccccc";
    let dev_tree = "dddddddddddddddddddddddddddddddddddddddd";
    let refs = vec![
        publish_ref("refs/heads/main", main_oid, main_oid),
        publish_ref("refs/tags/v1.0.0", main_oid, main_oid),
        publish_ref("refs/heads/dev", dev_oid, dev_oid),
    ];

    let plan = build_snapshot_plan(
        &refs,
        vec![revision(main_oid, main_tree), revision(dev_oid, dev_tree)],
        Some("refs/heads/main"),
    )
    .expect("refs plan should build");

    assert_eq!(plan.refs.len(), 3);
    assert_eq!(plan.revisions.len(), 2);
    assert_eq!(plan.revisions[0].revision_oid, main_oid);
    assert_eq!(plan.revisions[1].revision_oid, dev_oid);
    assert_eq!(plan.default_ref.as_deref(), Some("refs/heads/main"));
    assert_eq!(plan.default_latest_revision_oid(), Some(main_oid));

    let tag_entry = plan.refs[1].to_publish_ref_entry(Utc::now());
    assert_eq!(tag_entry.ref_type, RefType::Tag);
    assert_eq!(tag_entry.short_name, "v1.0.0");
    assert_eq!(tag_entry.revision_oid, main_oid);
    assert!(!tag_entry.is_default);

    let default_refs = plan
        .refs
        .iter()
        .filter(|publish_ref| publish_ref.is_default)
        .collect::<Vec<_>>();
    assert_eq!(default_refs.len(), 1);
    assert_eq!(default_refs[0].ref_name, "refs/heads/main");
}

#[test]
fn publish_refs_test_latest_uses_default_ref_not_first_ref() {
    let feature_oid = "1111111111111111111111111111111111111111";
    let main_oid = "2222222222222222222222222222222222222222";
    let feature_tree = "3333333333333333333333333333333333333333";
    let main_tree = "4444444444444444444444444444444444444444";
    let refs = vec![
        publish_ref("refs/heads/feature", feature_oid, feature_oid),
        publish_ref("refs/heads/main", main_oid, main_oid),
    ];

    let plan = build_snapshot_plan(
        &refs,
        vec![
            revision(feature_oid, feature_tree),
            revision(main_oid, main_tree),
        ],
        Some("refs/heads/main"),
    )
    .expect("refs plan should build");

    assert_eq!(plan.default_latest_revision_oid(), Some(main_oid));
    assert_eq!(plan.refs[0].revision_oid, feature_oid);
    assert!(!plan.refs[0].is_default);
    assert!(plan.refs[1].is_default);
}

#[test]
fn publish_refs_test_refs_index_payload_matches_plan() {
    let generated_at = Utc::now();
    let main_oid = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let tag_oid = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let tree_oid = "cccccccccccccccccccccccccccccccccccccccc";
    let refs = vec![
        publish_ref("refs/heads/main", main_oid, main_oid),
        publish_ref("refs/tags/v1.0.0", tag_oid, main_oid),
    ];
    let plan = build_snapshot_plan(
        &refs,
        vec![revision(main_oid, tree_oid)],
        Some("refs/heads/main"),
    )
    .expect("refs plan should build");

    let index = plan
        .to_refs_index("site-1", 42, generated_at)
        .expect("refs index should build");

    assert_eq!(
        publish_refs_index_key("repo-1", "site-1"),
        "repo-1/publish/sites/site-1/refs.json"
    );
    assert_eq!(index.schema_version, PUBLISH_SCHEMA_VERSION);
    assert_eq!(index.site_id, "site-1");
    assert_eq!(index.refs_generation, 42);
    assert_eq!(index.default_ref, "refs/heads/main");
    assert_eq!(index.generated_at, generated_at);
    assert_eq!(index.refs.len(), 2);
    assert_eq!(index.refs[0].ref_type, RefType::Branch);
    assert_eq!(index.refs[0].short_name, "main");
    assert!(index.refs[0].is_default);
    assert_eq!(index.refs[1].ref_type, RefType::Tag);
    assert_eq!(index.refs[1].short_name, "v1.0.0");
    assert_eq!(index.refs[1].revision_oid, main_oid);
}

#[test]
fn publish_refs_test_missing_revision_is_rejected() {
    let ref_oid = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let other_oid = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let tree_oid = "cccccccccccccccccccccccccccccccccccccccc";
    let refs = vec![publish_ref("refs/heads/main", ref_oid, ref_oid)];

    let err = build_snapshot_plan(
        &refs,
        vec![revision(other_oid, tree_oid)],
        Some("refs/heads/main"),
    )
    .expect_err("missing revision should fail");

    assert!(
        err.to_string().contains(ref_oid),
        "error should name the missing revision: {err}"
    );
}
