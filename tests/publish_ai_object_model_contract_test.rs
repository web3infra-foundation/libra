use std::collections::BTreeSet;

use libra::internal::publish::contract::{
    AiObjectLayer, PublishAiBundle, PublishAiGraph, PublishAiIndex, PublishAiObject,
};

fn fixture(name: &str) -> serde_json::Value {
    let path = format!("{}/tests/data/publish/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_slice(&std::fs::read(&path).expect("publish fixture must be readable"))
        .expect("publish fixture must be valid JSON")
}

#[test]
fn publish_ai_object_model_contract_test_fixtures_cover_reference_layers() {
    let reference = include_str!("../docs/ai/object-model-reference.md");
    let snapshot: PublishAiObject = serde_json::from_value(fixture("ai-object.json")).unwrap();
    let event: PublishAiObject = serde_json::from_value(fixture("ai-object-event.json")).unwrap();
    let projection: PublishAiObject =
        serde_json::from_value(fixture("ai-object-projection.json")).unwrap();

    assert_eq!(snapshot.layer, AiObjectLayer::Snapshot);
    assert_eq!(snapshot.object_type, "Run");
    assert!(reference.contains("- `Run`"));
    assert!(!snapshot.source_refs.is_empty());
    assert!(
        snapshot
            .relationships
            .iter()
            .any(|edge| edge.from_object_id == snapshot.object_id),
        "snapshot fixture must expose graph edges from the object envelope"
    );

    assert_eq!(event.layer, AiObjectLayer::Event);
    assert_eq!(event.object_type, "RunEvent");
    assert!(reference.contains("- `RunEvent`"));
    assert!(!event.source_refs.is_empty());

    assert_eq!(projection.layer, AiObjectLayer::Projection);
    assert_eq!(projection.object_type, "Thread");
    assert!(reference.contains("- `Thread`"));
    assert!(!projection.source_refs.is_empty());
}

#[test]
fn publish_ai_object_model_contract_test_bundle_index_and_graph_cross_link_objects() {
    let bundle: PublishAiBundle = serde_json::from_value(fixture("ai-bundle.json")).unwrap();
    let index: PublishAiIndex = serde_json::from_value(fixture("ai-index.json")).unwrap();
    let graph: PublishAiGraph = serde_json::from_value(fixture("ai-graph.json")).unwrap();

    assert_eq!(bundle.site_id, index.site_id);
    assert_eq!(bundle.revision_oid, index.revision_oid);
    assert_eq!(bundle.site_id, graph.site_id);
    assert_eq!(bundle.revision_oid, graph.revision_oid);
    assert_eq!(bundle.ai_version_id, graph.ai_version_id);

    let bundle_objects = bundle
        .objects
        .iter()
        .map(|entry| (entry.object_type.as_str(), entry.object_id.as_str()))
        .collect::<BTreeSet<_>>();
    for entry in &index.objects {
        assert!(
            bundle_objects.contains(&(entry.object_type.as_str(), entry.object_id.as_str())),
            "ai-index object must also be listed in the bundle: {entry:?}"
        );
        let layer_path = match entry.layer {
            AiObjectLayer::Snapshot => "snapshot",
            AiObjectLayer::Event => "event",
            AiObjectLayer::Projection => "projection",
        };
        assert!(
            entry.r2_key.contains(&format!(
                "/ai/objects/{layer_path}/{}/{}.json",
                entry.object_type, entry.object_id
            )),
            "ai-index object key must include layer/type/id: {}",
            entry.r2_key
        );
    }

    let graph_nodes = graph
        .nodes
        .iter()
        .map(|node| (node.object_type.as_str(), node.object_id.as_str()))
        .collect::<BTreeSet<_>>();
    for edge in &graph.edges {
        assert!(
            graph_nodes.contains(&(edge.from_object_type.as_str(), edge.from_object_id.as_str())),
            "graph edge source must have a node: {edge:?}"
        );
        assert!(
            graph_nodes.contains(&(edge.to_object_type.as_str(), edge.to_object_id.as_str())),
            "graph edge target must have a node: {edge:?}"
        );
    }
}

#[test]
fn publish_ai_object_model_contract_test_redaction_summary_is_structured() {
    let bundle: PublishAiBundle = serde_json::from_value(fixture("ai-bundle.json")).unwrap();

    assert!(
        bundle.redaction.removed_field_count > 0,
        "bundle redaction summary must count removed fields"
    );
    assert!(
        bundle.redaction.removed_fields_by_type.contains_key("Run"),
        "bundle redaction summary must group removed fields by object type"
    );
    assert_eq!(
        bundle.redaction.object_counts_by_type.get("Run"),
        Some(&1),
        "bundle redaction summary must count emitted objects by type"
    );
}
