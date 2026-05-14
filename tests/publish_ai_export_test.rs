use chrono::{TimeZone, Utc};
use libra::internal::publish::{
    ai_export::{
        AiExportError, AiExportRequest, ai_object_model_type_specs, build_ai_export_plan,
        publish_ai_bundle_key, publish_ai_graph_key, publish_ai_index_key, publish_ai_object_key,
    },
    contract::{
        AiBundleAssociatedIds, AiBundleIndexes, AiObjectLayer, AiObjectRedaction,
        AiObjectRelationship, PUBLISH_SCHEMA_VERSION, PublishAiObject, RedactionMode,
    },
    snapshot::sha256_hex,
};

const REPO_ID: &str = "11111111-2222-3333-4444-555555555555";
const SITE_ID: &str = "00000000-0000-0000-0000-0000publish01";
const REVISION_OID: &str = "abcdef0123456789abcdef0123456789abcdef01";
const AI_VERSION_ID: &str = "ai-version-2026-05-13-001";
const RULES_VERSION: &str = "2026.05.13-1";

#[test]
fn publish_ai_export_test_builds_index_graph_bundle_and_storage_keys() {
    let generated_at = timestamp();
    let plan = build_ai_export_plan(AiExportRequest {
        repo_id: REPO_ID.to_string(),
        site_id: SITE_ID.to_string(),
        revision_oid: REVISION_OID.to_string(),
        ai_version_id: AI_VERSION_ID.to_string(),
        generated_at,
        ai_object_model_reference: "docs/agent/ai-object-model-reference.md".to_string(),
        redaction_mode: RedactionMode::Default,
        redaction_rules_version: RULES_VERSION.to_string(),
        associated_ids: AiBundleAssociatedIds {
            thread_id: Some("thread-1".to_string()),
            ..AiBundleAssociatedIds::default()
        },
        objects: vec![intent_object(), plan_object(), run_object()],
    })
    .expect("fixture objects should export");

    assert_eq!(
        plan.index_key,
        publish_ai_index_key(REPO_ID, SITE_ID, REVISION_OID)
    );
    assert_eq!(
        plan.graph_key,
        publish_ai_graph_key(REPO_ID, SITE_ID, REVISION_OID)
    );
    assert_eq!(
        plan.bundle_key,
        publish_ai_bundle_key(REPO_ID, SITE_ID, REVISION_OID, AI_VERSION_ID)
    );
    assert_eq!(plan.objects.len(), 3);
    assert_eq!(plan.index.schema_version, PUBLISH_SCHEMA_VERSION);
    assert_eq!(plan.index.objects.len(), 3);
    assert_eq!(plan.index.bundles.len(), 1);
    assert_eq!(plan.index.bundles[0].object_count, 3);
    assert_eq!(plan.index.bundles[0].bundle_key, plan.bundle_key);
    assert_eq!(
        plan.index.bundles[0].bundle_sha256,
        sha256_hex(&serde_json::to_vec(&plan.bundle).expect("bundle must serialize"))
    );

    let run_export = plan
        .objects
        .iter()
        .find(|entry| entry.object.object_type == "Run")
        .expect("run object must be exported");
    assert_eq!(
        run_export.r2_key,
        publish_ai_object_key(
            REPO_ID,
            SITE_ID,
            REVISION_OID,
            AiObjectLayer::Snapshot,
            "Run",
            "run-1"
        )
    );
    assert_eq!(
        run_export.payload_sha256,
        sha256_hex(&serde_json::to_vec(&run_export.object).expect("object must serialize"))
    );

    assert_eq!(plan.graph.nodes.len(), 3);
    assert!(
        plan.graph
            .edges
            .iter()
            .any(|edge| edge.from_object_type == "Run" && edge.to_object_type == "Plan")
    );
    assert_eq!(
        plan.bundle.indexes.by_plan.get("plan-1"),
        Some(&vec![
            "Intent/intent-1".to_string(),
            "Run/run-1".to_string()
        ])
    );
    assert_eq!(
        plan.bundle.indexes.by_run.get("run-1"),
        Some(&vec!["Plan/plan-1".to_string()])
    );
    assert_eq!(plan.bundle.redaction.removed_field_count, 2);
    assert_eq!(
        plan.bundle.redaction.removed_fields_by_type.get("Run"),
        Some(&vec![
            "payload.absoluteWorkspacePath".to_string(),
            "payload.providerRawResponse".to_string()
        ])
    );
    assert_eq!(
        plan.bundle.redaction.object_counts_by_type.get("Run"),
        Some(&1)
    );
}

#[test]
fn publish_ai_export_test_accepts_every_reference_object_type() {
    let generated_at = timestamp();
    let objects = ai_object_model_type_specs()
        .iter()
        .map(|spec| {
            let object_id = format!("{}-1", spec.object_type);
            object(
                spec.object_type,
                &object_id,
                spec.layer,
                serde_json::json!({
                    "objectType": spec.object_type,
                    "objectId": object_id,
                    "layer": spec.layer,
                }),
                vec![],
                vec![edge(
                    spec.object_type,
                    &object_id,
                    "self",
                    spec.object_type,
                    &object_id,
                )],
            )
        })
        .collect::<Vec<_>>();

    let plan = build_ai_export_plan(AiExportRequest {
        repo_id: REPO_ID.to_string(),
        site_id: SITE_ID.to_string(),
        revision_oid: REVISION_OID.to_string(),
        ai_version_id: AI_VERSION_ID.to_string(),
        generated_at,
        ai_object_model_reference: "docs/agent/ai-object-model-reference.md".to_string(),
        redaction_mode: RedactionMode::Default,
        redaction_rules_version: RULES_VERSION.to_string(),
        associated_ids: AiBundleAssociatedIds::default(),
        objects,
    })
    .expect("all reference AI object types should export");

    assert_eq!(plan.index.objects.len(), ai_object_model_type_specs().len());
    for spec in ai_object_model_type_specs() {
        let object_id = format!("{}-1", spec.object_type);
        assert_eq!(
            plan.bundle
                .redaction
                .object_counts_by_type
                .get(spec.object_type),
            Some(&1),
            "{} should be counted in redaction summary",
            spec.object_type
        );
        let indexed = plan
            .index
            .objects
            .iter()
            .find(|entry| entry.object_type == spec.object_type)
            .unwrap_or_else(|| panic!("{} should be listed in ai/index", spec.object_type));
        assert_eq!(
            indexed.layer, spec.layer,
            "{} should use the reference layer",
            spec.object_type
        );
        assert!(
            any_index_bucket_contains(&plan.bundle.indexes, &object_id),
            "{} should have a relationship index bucket",
            spec.object_type
        );
    }
}

#[test]
fn publish_ai_export_test_rejects_relationships_with_missing_endpoint() {
    let err = build_ai_export_plan(AiExportRequest {
        repo_id: REPO_ID.to_string(),
        site_id: SITE_ID.to_string(),
        revision_oid: REVISION_OID.to_string(),
        ai_version_id: AI_VERSION_ID.to_string(),
        generated_at: timestamp(),
        ai_object_model_reference: "docs/agent/ai-object-model-reference.md".to_string(),
        redaction_mode: RedactionMode::Default,
        redaction_rules_version: RULES_VERSION.to_string(),
        associated_ids: AiBundleAssociatedIds::default(),
        objects: vec![run_object()],
    })
    .expect_err("missing Plan endpoint must fail");

    assert!(matches!(
        err,
        AiExportError::MissingRelationshipEndpoint {
            from_object_type,
            to_object_type,
            ..
        } if from_object_type == "Run" && to_object_type == "Plan"
    ));
}

fn any_index_bucket_contains(indexes: &AiBundleIndexes, object_id: &str) -> bool {
    [
        &indexes.by_thread,
        &indexes.by_intent,
        &indexes.by_plan,
        &indexes.by_task,
        &indexes.by_run,
        &indexes.by_patchset,
        &indexes.by_event,
        &indexes.by_context,
    ]
    .into_iter()
    .any(|bucket| bucket.contains_key(object_id))
}

#[test]
fn publish_ai_export_test_rejects_redaction_policy_mismatch() {
    let mut object = run_object();
    object.redaction.rules_version = "older-rules".to_string();

    let err = build_ai_export_plan(AiExportRequest {
        repo_id: REPO_ID.to_string(),
        site_id: SITE_ID.to_string(),
        revision_oid: REVISION_OID.to_string(),
        ai_version_id: AI_VERSION_ID.to_string(),
        generated_at: timestamp(),
        ai_object_model_reference: "docs/agent/ai-object-model-reference.md".to_string(),
        redaction_mode: RedactionMode::Default,
        redaction_rules_version: RULES_VERSION.to_string(),
        associated_ids: AiBundleAssociatedIds::default(),
        objects: vec![object],
    })
    .expect_err("redaction rules must match the export policy");

    assert!(matches!(
        err,
        AiExportError::RedactionMismatch {
            object_type,
            actual_rules,
            expected_rules,
            ..
        } if object_type == "Run" && actual_rules == "older-rules" && expected_rules == RULES_VERSION
    ));
}

fn timestamp() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 5, 13, 12, 0, 0)
        .single()
        .expect("test timestamp must be valid")
}

fn intent_object() -> PublishAiObject {
    object(
        "Intent",
        "intent-1",
        AiObjectLayer::Snapshot,
        serde_json::json!({ "intentId": "intent-1", "title": "Publish" }),
        vec![],
        vec![edge("Plan", "plan-1", "isPartOf", "Intent", "intent-1")],
    )
}

fn plan_object() -> PublishAiObject {
    object(
        "Plan",
        "plan-1",
        AiObjectLayer::Snapshot,
        serde_json::json!({ "planId": "plan-1", "intentId": "intent-1" }),
        vec![],
        vec![],
    )
}

fn run_object() -> PublishAiObject {
    object(
        "Run",
        "run-1",
        AiObjectLayer::Snapshot,
        serde_json::json!({ "runId": "run-1", "planId": "plan-1" }),
        vec![
            "payload.providerRawResponse".to_string(),
            "payload.absoluteWorkspacePath".to_string(),
        ],
        vec![edge("Run", "run-1", "appliesTo", "Plan", "plan-1")],
    )
}

fn object(
    object_type: &str,
    object_id: &str,
    layer: AiObjectLayer,
    payload: serde_json::Value,
    removed_fields: Vec<String>,
    relationships: Vec<AiObjectRelationship>,
) -> PublishAiObject {
    PublishAiObject {
        schema_version: PUBLISH_SCHEMA_VERSION,
        site_id: SITE_ID.to_string(),
        revision_oid: REVISION_OID.to_string(),
        object_type: object_type.to_string(),
        object_id: object_id.to_string(),
        layer,
        source_refs: vec![format!("test/{object_id}")],
        relationships,
        payload,
        redaction: AiObjectRedaction {
            mode: RedactionMode::Default,
            rules_version: RULES_VERSION.to_string(),
        },
        removed_fields,
    }
}

fn edge(
    from_object_type: &str,
    from_object_id: &str,
    kind: &str,
    to_object_type: &str,
    to_object_id: &str,
) -> AiObjectRelationship {
    AiObjectRelationship {
        kind: kind.to_string(),
        from_object_type: from_object_type.to_string(),
        from_object_id: from_object_id.to_string(),
        to_object_type: to_object_type.to_string(),
        to_object_id: to_object_id.to_string(),
    }
}
