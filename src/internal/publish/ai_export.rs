//! AI object model export planning.
//!
//! This module is the bridge between redacted AI object envelopes and
//! the publish artefacts written to R2/D1: per-object JSON keys,
//! `ai/index.json`, the graph projection, and the revision bundle.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};

use super::{
    contract::{
        AiBundleAssociatedIds, AiBundleIndexes, AiBundleObjectEntry, AiBundleRedaction,
        AiGraphNode, AiObjectLayer, AiObjectRelationship, PUBLISH_SCHEMA_VERSION, PublishAiBundle,
        PublishAiGraph, PublishAiIndex, PublishAiIndexBundleEntry, PublishAiObject, RedactionMode,
    },
    snapshot::sha256_hex,
};

/// Inputs needed to build the AI publish artefact set for one revision.
#[derive(Clone, Debug)]
pub struct AiExportRequest {
    pub repo_id: String,
    pub site_id: String,
    pub revision_oid: String,
    pub ai_version_id: String,
    pub generated_at: DateTime<Utc>,
    pub ai_object_model_reference: String,
    pub redaction_mode: RedactionMode,
    pub redaction_rules_version: String,
    pub associated_ids: AiBundleAssociatedIds,
    pub objects: Vec<PublishAiObject>,
}

/// Planned R2/D1 outputs for one revision's AI object model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AiExportPlan {
    pub index_key: String,
    pub graph_key: String,
    pub bundle_key: String,
    pub objects: Vec<AiExportObject>,
    pub index: PublishAiIndex,
    pub graph: PublishAiGraph,
    pub bundle: PublishAiBundle,
}

/// One per-object JSON body plus the storage metadata D1 needs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AiExportObject {
    pub object: PublishAiObject,
    pub r2_key: String,
    pub payload_sha256: String,
}

/// Errors surfaced while converting redacted objects into publish artefacts.
#[derive(Debug, thiserror::Error)]
pub enum AiExportError {
    #[error(
        "AI object {object_type}/{object_id} belongs to site {actual:?}, expected {expected:?}"
    )]
    SiteMismatch {
        object_type: String,
        object_id: String,
        actual: String,
        expected: String,
    },
    #[error(
        "AI object {object_type}/{object_id} belongs to revision {actual:?}, expected {expected:?}"
    )]
    RevisionMismatch {
        object_type: String,
        object_id: String,
        actual: String,
        expected: String,
    },
    #[error("duplicate AI object {object_type}/{object_id} in revision export")]
    DuplicateObject {
        object_type: String,
        object_id: String,
    },
    #[error(
        "AI object {object_type}/{object_id} redaction {actual:?}/{actual_rules:?} does not match export redaction {expected:?}/{expected_rules:?}"
    )]
    RedactionMismatch {
        object_type: String,
        object_id: String,
        actual: RedactionMode,
        actual_rules: String,
        expected: RedactionMode,
        expected_rules: String,
    },
    #[error(
        "AI relationship {kind} from {from_object_type}/{from_object_id} to {to_object_type}/{to_object_id} references an object that is missing from the export"
    )]
    MissingRelationshipEndpoint {
        kind: String,
        from_object_type: String,
        from_object_id: String,
        to_object_type: String,
        to_object_id: String,
    },
    #[error("failed to serialize AI publish artefact {artifact}: {source}")]
    Serialize {
        artifact: &'static str,
        #[source]
        source: serde_json::Error,
    },
}

/// Build index, graph and bundle artefacts from redacted AI objects.
pub fn build_ai_export_plan(request: AiExportRequest) -> Result<AiExportPlan, AiExportError> {
    let AiExportRequest {
        repo_id,
        site_id,
        revision_oid,
        ai_version_id,
        generated_at,
        ai_object_model_reference,
        redaction_mode,
        redaction_rules_version,
        associated_ids,
        objects,
    } = request;
    let mut seen = BTreeSet::new();
    let mut export_objects = Vec::with_capacity(objects.len());
    let mut relationships = Vec::new();
    let mut redaction = AiBundleRedaction {
        mode: redaction_mode,
        rules_version: redaction_rules_version.clone(),
        removed_field_count: 0,
        removed_fields_by_type: BTreeMap::new(),
        object_counts_by_type: BTreeMap::new(),
    };

    for object in objects {
        validate_object(
            &site_id,
            &revision_oid,
            redaction_mode,
            &redaction_rules_version,
            &object,
        )?;
        let key = (object.object_type.clone(), object.object_id.clone());
        if !seen.insert(key.clone()) {
            return Err(AiExportError::DuplicateObject {
                object_type: key.0,
                object_id: key.1,
            });
        }

        relationships.extend(object.relationships.clone());
        redaction.removed_field_count += object.removed_fields.len() as u64;
        if !object.removed_fields.is_empty() {
            redaction
                .removed_fields_by_type
                .entry(object.object_type.clone())
                .or_default()
                .extend(object.removed_fields.clone());
        }
        *redaction
            .object_counts_by_type
            .entry(object.object_type.clone())
            .or_insert(0) += 1;

        let bytes = serde_json::to_vec(&object).map_err(|source| AiExportError::Serialize {
            artifact: "ai object",
            source,
        })?;
        let r2_key = publish_ai_object_key(
            &repo_id,
            &site_id,
            &revision_oid,
            object.layer,
            &object.object_type,
            &object.object_id,
        );
        export_objects.push(AiExportObject {
            object,
            r2_key,
            payload_sha256: sha256_hex(&bytes),
        });
    }

    for fields in redaction.removed_fields_by_type.values_mut() {
        fields.sort();
        fields.dedup();
    }
    validate_relationships(&seen, &relationships)?;

    let objects = export_objects
        .iter()
        .map(|entry| AiBundleObjectEntry {
            object_type: entry.object.object_type.clone(),
            object_id: entry.object.object_id.clone(),
            layer: entry.object.layer,
            r2_key: entry.r2_key.clone(),
            payload_sha256: entry.payload_sha256.clone(),
        })
        .collect::<Vec<_>>();

    let bundle = PublishAiBundle {
        schema_version: PUBLISH_SCHEMA_VERSION,
        ai_object_model_reference,
        site_id: site_id.clone(),
        revision_oid: revision_oid.clone(),
        ai_version_id: ai_version_id.clone(),
        objects: objects.clone(),
        relationships: relationships.clone(),
        indexes: build_indexes(&relationships),
        redaction: redaction.clone(),
        associated_ids,
    };
    let bundle_bytes = serde_json::to_vec(&bundle).map_err(|source| AiExportError::Serialize {
        artifact: "ai bundle",
        source,
    })?;
    let bundle_sha256 = sha256_hex(&bundle_bytes);
    let bundle_key = publish_ai_bundle_key(&repo_id, &site_id, &revision_oid, &ai_version_id);

    let index = PublishAiIndex {
        schema_version: PUBLISH_SCHEMA_VERSION,
        site_id: site_id.clone(),
        revision_oid: revision_oid.clone(),
        objects: objects.clone(),
        bundles: vec![PublishAiIndexBundleEntry {
            ai_version_id: ai_version_id.clone(),
            bundle_key: bundle_key.clone(),
            bundle_sha256,
            object_count: objects.len() as u64,
            created_at: generated_at,
        }],
        redaction,
        generated_at,
    };

    let graph = PublishAiGraph {
        schema_version: PUBLISH_SCHEMA_VERSION,
        site_id: site_id.clone(),
        revision_oid: revision_oid.clone(),
        ai_version_id,
        nodes: objects
            .iter()
            .map(|entry| AiGraphNode {
                object_type: entry.object_type.clone(),
                object_id: entry.object_id.clone(),
                layer: entry.layer,
                r2_key: entry.r2_key.clone(),
            })
            .collect(),
        edges: relationships,
        generated_at,
    };

    Ok(AiExportPlan {
        index_key: publish_ai_index_key(&repo_id, &site_id, &revision_oid),
        graph_key: publish_ai_graph_key(&repo_id, &site_id, &revision_oid),
        bundle_key,
        objects: export_objects,
        index,
        graph,
        bundle,
    })
}

pub fn publish_ai_index_key(repo_id: &str, site_id: &str, revision_oid: &str) -> String {
    format!(
        "{repo_id}/publish/sites/{site_id}/{}",
        publish_ai_index_relative_key(revision_oid)
    )
}

pub fn publish_ai_graph_key(repo_id: &str, site_id: &str, revision_oid: &str) -> String {
    format!(
        "{repo_id}/publish/sites/{site_id}/{}",
        publish_ai_graph_relative_key(revision_oid)
    )
}

pub fn publish_ai_bundle_key(
    repo_id: &str,
    site_id: &str,
    revision_oid: &str,
    ai_version_id: &str,
) -> String {
    format!(
        "{repo_id}/publish/sites/{site_id}/{}",
        publish_ai_bundle_relative_key(revision_oid, ai_version_id)
    )
}

pub fn publish_ai_object_key(
    repo_id: &str,
    site_id: &str,
    revision_oid: &str,
    layer: AiObjectLayer,
    object_type: &str,
    object_id: &str,
) -> String {
    let layer = match layer {
        AiObjectLayer::Snapshot => "snapshot",
        AiObjectLayer::Event => "event",
        AiObjectLayer::Projection => "projection",
    };
    format!(
        "{repo_id}/publish/sites/{site_id}/{}",
        publish_ai_object_relative_key(revision_oid, layer, object_type, object_id)
    )
}

pub fn publish_ai_index_relative_key(revision_oid: &str) -> String {
    format!("revisions/{revision_oid}/ai/index.json")
}

pub fn publish_ai_graph_relative_key(revision_oid: &str) -> String {
    format!("revisions/{revision_oid}/ai/graph.json")
}

pub fn publish_ai_bundle_relative_key(revision_oid: &str, ai_version_id: &str) -> String {
    format!("revisions/{revision_oid}/ai/bundles/{ai_version_id}.json")
}

pub fn publish_ai_object_relative_key(
    revision_oid: &str,
    layer: &str,
    object_type: &str,
    object_id: &str,
) -> String {
    format!("revisions/{revision_oid}/ai/objects/{layer}/{object_type}/{object_id}.json")
}

fn validate_object(
    site_id: &str,
    revision_oid: &str,
    redaction_mode: RedactionMode,
    redaction_rules_version: &str,
    object: &PublishAiObject,
) -> Result<(), AiExportError> {
    if object.site_id != site_id {
        return Err(AiExportError::SiteMismatch {
            object_type: object.object_type.clone(),
            object_id: object.object_id.clone(),
            actual: object.site_id.clone(),
            expected: site_id.to_string(),
        });
    }
    if object.revision_oid != revision_oid {
        return Err(AiExportError::RevisionMismatch {
            object_type: object.object_type.clone(),
            object_id: object.object_id.clone(),
            actual: object.revision_oid.clone(),
            expected: revision_oid.to_string(),
        });
    }
    if object.redaction.mode != redaction_mode
        || object.redaction.rules_version != redaction_rules_version
    {
        return Err(AiExportError::RedactionMismatch {
            object_type: object.object_type.clone(),
            object_id: object.object_id.clone(),
            actual: object.redaction.mode,
            actual_rules: object.redaction.rules_version.clone(),
            expected: redaction_mode,
            expected_rules: redaction_rules_version.to_string(),
        });
    }
    Ok(())
}

fn validate_relationships(
    objects: &BTreeSet<(String, String)>,
    relationships: &[AiObjectRelationship],
) -> Result<(), AiExportError> {
    for edge in relationships {
        let from = (edge.from_object_type.clone(), edge.from_object_id.clone());
        let to = (edge.to_object_type.clone(), edge.to_object_id.clone());
        if !objects.contains(&from) || !objects.contains(&to) {
            return Err(AiExportError::MissingRelationshipEndpoint {
                kind: edge.kind.clone(),
                from_object_type: edge.from_object_type.clone(),
                from_object_id: edge.from_object_id.clone(),
                to_object_type: edge.to_object_type.clone(),
                to_object_id: edge.to_object_id.clone(),
            });
        }
    }
    Ok(())
}

fn build_indexes(relationships: &[AiObjectRelationship]) -> AiBundleIndexes {
    let mut indexes = AiBundleIndexes::default();
    for edge in relationships {
        add_index_entries(&mut indexes, edge);
    }
    sort_index_values(&mut indexes);
    indexes
}

fn add_index_entries(indexes: &mut AiBundleIndexes, edge: &AiObjectRelationship) {
    add_index_entry(
        bucket_for_type(indexes, &edge.from_object_type),
        &edge.from_object_id,
        &format!("{}/{}", edge.to_object_type, edge.to_object_id),
    );
    add_index_entry(
        bucket_for_type(indexes, &edge.to_object_type),
        &edge.to_object_id,
        &format!("{}/{}", edge.from_object_type, edge.from_object_id),
    );
}

fn bucket_for_type<'a>(
    indexes: &'a mut AiBundleIndexes,
    object_type: &str,
) -> Option<&'a mut BTreeMap<String, Vec<String>>> {
    match object_type {
        "Thread" => Some(&mut indexes.by_thread),
        "Intent" | "IntentEvent" => Some(&mut indexes.by_intent),
        "Plan" | "PlanStepEvent" => Some(&mut indexes.by_plan),
        "Task" | "TaskEvent" => Some(&mut indexes.by_task),
        "Run" | "RunEvent" | "RunUsage" => Some(&mut indexes.by_run),
        "PatchSet" => Some(&mut indexes.by_patchset),
        "ToolInvocation" | "Evidence" | "Decision" => Some(&mut indexes.by_event),
        "ContextSnapshot" | "ContextFrame" => Some(&mut indexes.by_context),
        _ => None,
    }
}

fn add_index_entry(bucket: Option<&mut BTreeMap<String, Vec<String>>>, key: &str, value: &str) {
    if let Some(bucket) = bucket {
        bucket
            .entry(key.to_string())
            .or_default()
            .push(value.to_string());
    }
}

fn sort_index_values(indexes: &mut AiBundleIndexes) {
    for bucket in [
        &mut indexes.by_thread,
        &mut indexes.by_intent,
        &mut indexes.by_plan,
        &mut indexes.by_task,
        &mut indexes.by_run,
        &mut indexes.by_patchset,
        &mut indexes.by_event,
        &mut indexes.by_context,
    ] {
        for values in bucket.values_mut() {
            values.sort();
            values.dedup();
        }
    }
}
