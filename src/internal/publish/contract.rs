//! Publish wire-format contract types.
//!
//! Per `docs/improvement/publish.md` Phase 0, every JSON object the
//! CLI produces and the Worker consumes round-trips through the
//! types declared here. The shape of the JSON is fixed to:
//!
//! - `schemaVersion` — bumped when a breaking change ships; readers
//!   refuse newer-than-known versions.
//! - the rest of the named fields per type in the design doc
//!   (`AI Object Model Reference` and `R2 object layout` sections).
//!
//! Field names are camelCase on the wire — both the Worker (TypeScript)
//! and the CLI agree; serde's `#[serde(rename_all = "camelCase")]`
//! handles the snake_case→camelCase mapping. The Worker side
//! re-declares matching TypeScript types, but the **single source of
//! truth** is this Rust module: the Phase 0 contract tests round-trip
//! every JSON fixture under `tests/data/publish/` through these types
//! and re-serialise byte-equal output, which guarantees that any
//! schema drift in either direction surfaces as a test failure.
//!
//! The fixtures double as Worker test inputs (Miniflare loads them
//! directly into D1/R2 in later phases), so introducing a new field
//! has a bounded blast radius: add the field here with `#[serde(default)]`
//! when reading legacy payloads, update one fixture, run the
//! contract tests, and ship.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Schema version the Phase 0 contract emits. Readers older than the
/// version they encounter MUST refuse the payload; newer readers
/// accept any version up to and including this constant.
pub const PUBLISH_SCHEMA_VERSION: u32 = 1;

/// Errors surfaced by [`parse_versioned`] when a payload cannot be
/// honoured because of its `schemaVersion`. Distinct from a serde
/// shape error (which is propagated as the wrapped `serde_json::Error`).
#[derive(Debug, thiserror::Error)]
pub enum PublishContractError {
    #[error(
        "publish payload schemaVersion {actual} is newer than this binary's known maximum {max}; \
         upgrade Libra or downgrade the publish source"
    )]
    UnsupportedNewerSchemaVersion { actual: u32, max: u32 },
    #[error("publish payload is missing a numeric `schemaVersion` top-level field")]
    MissingSchemaVersion,
    #[error("publish payload deserialization failed: {0}")]
    Deserialize(#[from] serde_json::Error),
}

/// Parse a JSON payload into one of the versioned contract types,
/// rejecting payloads that advertise `schemaVersion` newer than
/// [`PUBLISH_SCHEMA_VERSION`]. The check runs **before** full
/// deserialization so a future-only field shape does not surface as
/// a confusing serde error.
///
/// Codex pass-1 P2: the doc says "readers older than the version
/// they encounter MUST refuse the payload" but the schema-version
/// constant alone is informational; this helper enforces the gate
/// at every public CLI consumption seam.
pub fn parse_versioned<T>(raw: &serde_json::Value) -> Result<T, PublishContractError>
where
    T: serde::de::DeserializeOwned,
{
    let advertised = raw
        .as_object()
        .and_then(|m| m.get("schemaVersion"))
        .and_then(|v| v.as_u64())
        .ok_or(PublishContractError::MissingSchemaVersion)?;
    let advertised = u32::try_from(advertised).map_err(|_| {
        PublishContractError::UnsupportedNewerSchemaVersion {
            actual: u32::MAX,
            max: PUBLISH_SCHEMA_VERSION,
        }
    })?;
    if advertised > PUBLISH_SCHEMA_VERSION {
        return Err(PublishContractError::UnsupportedNewerSchemaVersion {
            actual: advertised,
            max: PUBLISH_SCHEMA_VERSION,
        });
    }
    let parsed = serde_json::from_value(raw.clone())?;
    Ok(parsed)
}

/// `publish_sites.status` enum on the wire. Mirrors the SQL `TEXT`
/// column — a typed enum here means a fixture with `"foo"` fails the
/// contract test instead of silently round-tripping.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SiteStatus {
    Active,
    Disabled,
}

/// Site visibility. `public` → no Access JWT required; `private` →
/// Worker enforces `Cf-Access-Jwt-Assertion`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SiteVisibility {
    Public,
    Private,
}

/// `publish_refs.ref_type`. Annotated tags peel to a commit oid in
/// `revision_oid`; lightweight tags share `target_oid` and
/// `revision_oid`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefType {
    Branch,
    Tag,
}

/// `publish_revisions.status`. A revision marked `syncing` is invisible
/// to the Worker API; only `published` rows respond to file/tree
/// queries. `failed` is a terminal error state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevisionStatus {
    Syncing,
    Published,
    Failed,
}

/// AI object layer per the AI object model reference.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiObjectLayer {
    Snapshot,
    Event,
    Projection,
}

/// Redaction mode applied at publish time. `default` follows the
/// site's visibility + built-in sensitive-field rules; `strict`
/// additionally drops prompt-like / tool-payload-like / path-like /
/// provider-detail-like fields while preserving every object envelope.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RedactionMode {
    Default,
    Strict,
}

/// Per-file display mode in `publish_files`. Mirrors the SQL `TEXT`
/// column.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileDisplayMode {
    /// UTF-8 text rendered inline; `r2_key` + `content_sha256` are
    /// always set.
    Text,
    /// Binary or non-UTF-8; only metadata is published, no R2
    /// contents.
    Binary,
    /// Larger than `publish.max_preview_bytes`; metadata only.
    TooLarge,
    /// Excluded by `.librapublishignore` or built-in deny rules.
    Ignored,
}

/// Top-level site row, mirrors `publish_sites`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublishSite {
    pub schema_version: u32,
    pub site_id: String,
    pub repo_id: String,
    pub clone_domain: String,
    pub slug: String,
    pub display_origin: String,
    pub name: String,
    pub visibility: SiteVisibility,
    pub status: SiteStatus,
    pub worker_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_revision_oid: Option<String>,
    pub refs_generation: u64,
    pub max_preview_bytes: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// One ref → revision binding, mirrors `publish_refs`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublishRef {
    pub schema_version: u32,
    pub site_id: String,
    pub ref_name: String,
    pub ref_type: RefType,
    pub short_name: String,
    pub target_oid: String,
    pub revision_oid: String,
    #[serde(default)]
    pub is_default: bool,
    pub sync_run_id: String,
    pub updated_at: DateTime<Utc>,
}

/// One published snapshot, mirrors `publish_revisions`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublishRevision {
    pub schema_version: u32,
    pub site_id: String,
    pub revision_oid: String,
    pub status: RevisionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_manifest_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_index_key: Option<String>,
    pub file_count: u64,
    pub ai_object_count: u64,
    pub ai_bundle_count: u64,
    pub redaction_mode: RedactionMode,
    pub redaction_rules_version: String,
    pub sync_run_id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Per-file metadata, mirrors `publish_files`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublishFile {
    pub site_id: String,
    pub revision_oid: String,
    pub path: String,
    pub display_mode: FileDisplayMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r2_key: Option<String>,
    pub size_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

/// Top-level code manifest per revision. Stored at
/// `{repo_id}/publish/sites/{site_id}/revisions/{revision_oid}/code-manifest.json`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublishCodeManifest {
    pub schema_version: u32,
    pub site_id: String,
    pub revision_oid: String,
    pub commit_oid: String,
    pub tree_oid: String,
    pub generated_at: DateTime<Utc>,
    pub files: Vec<PublishFile>,
}

/// One AI object envelope. The `payload` body is the redacted object
/// content; `removedFields` lets the Worker render a "this field was
/// removed by redaction" affordance without re-running the redactor.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublishAiObject {
    pub schema_version: u32,
    pub site_id: String,
    pub revision_oid: String,
    pub object_type: String,
    pub object_id: String,
    pub layer: AiObjectLayer,
    pub source_refs: Vec<String>,
    pub relationships: Vec<AiObjectRelationship>,
    pub payload: serde_json::Value,
    pub redaction: AiObjectRedaction,
    #[serde(default)]
    pub removed_fields: Vec<String>,
}

/// One typed edge in the AI object graph (e.g. `Run → patches → PatchSet`).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiObjectRelationship {
    pub kind: String,
    pub from_object_type: String,
    pub from_object_id: String,
    pub to_object_type: String,
    pub to_object_id: String,
}

/// Per-object redaction record.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiObjectRedaction {
    pub mode: RedactionMode,
    pub rules_version: String,
}

/// Aggregated AI bundle for one revision. Stored at
/// `{repo_id}/publish/sites/{site_id}/revisions/{revision_oid}/ai/bundles/{ai_version_id}.json`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublishAiBundle {
    pub schema_version: u32,
    pub ai_object_model_reference: String,
    pub site_id: String,
    pub revision_oid: String,
    pub ai_version_id: String,
    pub objects: Vec<AiBundleObjectEntry>,
    pub relationships: Vec<AiObjectRelationship>,
    pub indexes: AiBundleIndexes,
    pub redaction: AiBundleRedaction,
    pub associated_ids: AiBundleAssociatedIds,
}

/// One row in the bundle's object manifest. Indirects to the per-object
/// JSON the Worker fetches lazily.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiBundleObjectEntry {
    pub object_type: String,
    pub object_id: String,
    pub layer: AiObjectLayer,
    pub r2_key: String,
    pub payload_sha256: String,
}

/// Reverse / forward index buckets the bundle ships, keyed by domain
/// id (thread, intent, plan, ...). Each bucket lists object ids
/// participating in that index slot.
#[derive(Clone, Debug, Eq, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiBundleIndexes {
    #[serde(default)]
    pub by_thread: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub by_intent: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub by_plan: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub by_task: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub by_run: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub by_patchset: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub by_event: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub by_context: BTreeMap<String, Vec<String>>,
}

/// Bundle-level redaction summary. Per-object redaction lives on the
/// individual `PublishAiObject`s.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiBundleRedaction {
    pub mode: RedactionMode,
    pub rules_version: String,
    pub removed_field_count: u64,
    /// Per-object-type breakdown of removed fields, e.g.
    /// `{"Run": ["prompt", "raw_response"], ...}`.
    #[serde(default)]
    pub removed_fields_by_type: BTreeMap<String, Vec<String>>,
    /// Per-object-type emitted-object counts. The Worker renders a
    /// table from this without scanning every object payload.
    #[serde(default)]
    pub object_counts_by_type: BTreeMap<String, u64>,
}

/// IDs that link an AI bundle back to non-AI Libra entities (Git
/// trees, agent traces, etc.). All optional because some Goals do not
/// have one of every kind.
#[derive(Clone, Debug, Eq, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiBundleAssociatedIds {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree_oid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traces_commit: Option<String>,
}

/// Site-level "what is the latest published revision?" pointer
/// stored at `{repo_id}/publish/sites/{site_id}/latest.json`. The
/// Worker reads this once per request to resolve a request that
/// arrived without an explicit ref/revision.
///
/// Codex pass-1 P2: the design doc (publish.md:506) lists this
/// object under R2 layout but the Phase 0 contract didn't carry a
/// type for it. Adding it now keeps Phase 6 Worker reads from
/// inventing a new shape.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublishSiteLatest {
    pub schema_version: u32,
    pub site_id: String,
    pub default_ref: String,
    pub latest_revision_oid: String,
    pub refs_generation: u64,
    pub updated_at: DateTime<Utc>,
}

/// Full refs index for a site, stored at
/// `{repo_id}/publish/sites/{site_id}/refs.json`. The Worker reads
/// this to render the branch/tag picker without paginating
/// `publish_refs` rows.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublishRefsIndex {
    pub schema_version: u32,
    pub site_id: String,
    pub refs_generation: u64,
    pub default_ref: String,
    pub refs: Vec<PublishRefEntry>,
    pub generated_at: DateTime<Utc>,
}

/// One entry in [`PublishRefsIndex::refs`]. A trimmed-down view of
/// `PublishRef` — the Worker doesn't need `sync_run_id` or
/// `schema_version` per row to render the picker.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublishRefEntry {
    pub ref_name: String,
    pub ref_type: RefType,
    pub short_name: String,
    pub target_oid: String,
    pub revision_oid: String,
    #[serde(default)]
    pub is_default: bool,
    pub updated_at: DateTime<Utc>,
}

/// Per-revision AI object index stored at
/// `{repo_id}/publish/sites/{site_id}/revisions/{revision_oid}/ai/index.json`.
/// The Worker uses this to render the AI object explorer without
/// fetching every per-object JSON eagerly.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublishAiIndex {
    pub schema_version: u32,
    pub site_id: String,
    pub revision_oid: String,
    pub objects: Vec<AiBundleObjectEntry>,
    pub bundles: Vec<PublishAiIndexBundleEntry>,
    pub redaction: AiBundleRedaction,
    pub generated_at: DateTime<Utc>,
}

/// One bundle reference inside a [`PublishAiIndex`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublishAiIndexBundleEntry {
    pub ai_version_id: String,
    pub bundle_key: String,
    pub object_count: u64,
    pub created_at: DateTime<Utc>,
}

/// Standalone AI graph projection for visualisation. The shape is a
/// strict subset of [`PublishAiBundle`] — just nodes and edges —
/// because the renderer doesn't need redaction stats or index
/// buckets at request time.
///
/// Codex pass-1 P2 + publish.md:720: the Phase 0 acceptance list
/// names "AI graph" as a separate fixture; this type is the
/// canonical shape the renderer reads.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublishAiGraph {
    pub schema_version: u32,
    pub site_id: String,
    pub revision_oid: String,
    pub ai_version_id: String,
    pub nodes: Vec<AiGraphNode>,
    pub edges: Vec<AiObjectRelationship>,
    pub generated_at: DateTime<Utc>,
}

/// One node in [`PublishAiGraph`]. Carries the minimum the renderer
/// needs to draw the node + lazy-load the full object JSON via
/// `r2_key`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiGraphNode {
    pub object_type: String,
    pub object_id: String,
    pub layer: AiObjectLayer,
    pub r2_key: String,
}

/// Audit row for one sync invocation. Stored at
/// `{repo_id}/publish/sites/{site_id}/sync-runs/{sync_run_id}.json`
/// **and** as a row in `publish_sync_runs`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublishSyncRun {
    pub schema_version: u32,
    pub sync_run_id: String,
    pub site_id: String,
    pub status: SyncRunStatus,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
    pub refs_count: u64,
    pub revision_count: u64,
    pub file_count: u64,
    pub ai_object_count: u64,
    pub ai_bundle_count: u64,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    pub cli_version: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncRunStatus {
    Running,
    Succeeded,
    Failed,
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn fixture_path(name: &str) -> PathBuf {
        let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.push("tests/data/publish");
        p.push(name);
        p
    }

    fn load_fixture(name: &str) -> serde_json::Value {
        let path = fixture_path(name);
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|err| panic!("failed to read fixture {}: {err}", path.display()));
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|err| panic!("failed to parse fixture {}: {err}", path.display()))
    }

    fn round_trip<T>(name: &str)
    where
        T: serde::de::DeserializeOwned + Serialize + std::fmt::Debug + PartialEq,
    {
        let raw = load_fixture(name);
        let parsed: T = serde_json::from_value(raw.clone())
            .unwrap_or_else(|err| panic!("deserialize {name}: {err}"));
        let reserialised = serde_json::to_value(&parsed)
            .unwrap_or_else(|err| panic!("re-serialise {name}: {err}"));
        // Codex pass-1 P2: compare the freshly reserialised JSON
        // structurally against the source fixture. `serde_json::Value`'s
        // map is BTreeMap-backed so key ordering is deterministic, and
        // the structs all opt into `deny_unknown_fields`, so an extra
        // field on either side surfaces here.
        assert_eq!(
            raw, reserialised,
            "round-trip diverged structurally for fixture {name}",
        );
        // Belt-and-braces: re-parse the reserialised JSON to confirm
        // the type is happy with its own output (catches a Serialize
        // impl that emits a value Deserialize would refuse).
        let parsed_again: T = serde_json::from_value(reserialised)
            .unwrap_or_else(|err| panic!("deserialize re-serialised {name}: {err}"));
        assert_eq!(
            parsed, parsed_again,
            "type round-trip diverged for fixture {name}",
        );
    }

    #[test]
    fn publish_contract_round_trip_site() {
        round_trip::<PublishSite>("site.json");
    }

    #[test]
    fn publish_contract_round_trip_ref() {
        round_trip::<PublishRef>("ref.json");
    }

    #[test]
    fn publish_contract_round_trip_revision() {
        round_trip::<PublishRevision>("revision.json");
    }

    #[test]
    fn publish_contract_round_trip_code_manifest() {
        round_trip::<PublishCodeManifest>("code-manifest.json");
    }

    #[test]
    fn publish_contract_round_trip_file_metadata() {
        round_trip::<PublishFile>("file-metadata.json");
    }

    #[test]
    fn publish_contract_round_trip_ai_object() {
        round_trip::<PublishAiObject>("ai-object.json");
    }

    #[test]
    fn publish_contract_round_trip_ai_bundle() {
        round_trip::<PublishAiBundle>("ai-bundle.json");
    }

    #[test]
    fn publish_contract_round_trip_sync_run() {
        round_trip::<PublishSyncRun>("sync-run.json");
    }

    #[test]
    fn publish_contract_round_trip_site_latest() {
        round_trip::<PublishSiteLatest>("latest.json");
    }

    #[test]
    fn publish_contract_round_trip_refs_index() {
        round_trip::<PublishRefsIndex>("refs-index.json");
    }

    #[test]
    fn publish_contract_round_trip_ai_index() {
        round_trip::<PublishAiIndex>("ai-index.json");
    }

    #[test]
    fn publish_contract_round_trip_ai_graph() {
        round_trip::<PublishAiGraph>("ai-graph.json");
    }

    /// Codex pass-1 P2: cover lightweight tag (target_oid ==
    /// revision_oid) explicitly so a future refactor that conflates
    /// the two oids surfaces here.
    #[test]
    fn publish_contract_round_trip_ref_tag_lightweight() {
        let raw = load_fixture("ref-tag-lightweight.json");
        let parsed: PublishRef = serde_json::from_value(raw).unwrap();
        assert!(matches!(parsed.ref_type, RefType::Tag));
        assert_eq!(parsed.target_oid, parsed.revision_oid);
    }

    /// Codex pass-1 P2: annotated tag has `target_oid` (the tag
    /// object) distinct from `revision_oid` (the peeled commit).
    /// publish.md:477 mandates the schema preserve both.
    #[test]
    fn publish_contract_round_trip_ref_tag_annotated() {
        let raw = load_fixture("ref-tag-annotated.json");
        let parsed: PublishRef = serde_json::from_value(raw).unwrap();
        assert!(matches!(parsed.ref_type, RefType::Tag));
        assert_ne!(
            parsed.target_oid, parsed.revision_oid,
            "annotated tag fixture must keep target_oid and revision_oid distinct",
        );
    }

    #[test]
    fn publish_contract_round_trip_ai_object_strict() {
        let raw = load_fixture("ai-object-strict.json");
        let parsed: PublishAiObject = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(parsed.redaction.mode, RedactionMode::Strict);
        assert!(
            !parsed.removed_fields.is_empty(),
            "strict redaction fixture must list removed_fields",
        );
        round_trip::<PublishAiObject>("ai-object-strict.json");
    }

    #[test]
    fn publish_contract_round_trip_ai_object_event_layer() {
        let raw = load_fixture("ai-object-event.json");
        let parsed: PublishAiObject = serde_json::from_value(raw).unwrap();
        assert_eq!(parsed.layer, AiObjectLayer::Event);
        round_trip::<PublishAiObject>("ai-object-event.json");
    }

    #[test]
    fn publish_contract_round_trip_ai_object_projection_layer() {
        let raw = load_fixture("ai-object-projection.json");
        let parsed: PublishAiObject = serde_json::from_value(raw).unwrap();
        assert_eq!(parsed.layer, AiObjectLayer::Projection);
        round_trip::<PublishAiObject>("ai-object-projection.json");
    }

    /// Schema version is `1` and all top-level fixtures advertise it.
    #[test]
    fn publish_schema_version_pins_phase_zero_baseline() {
        assert_eq!(PUBLISH_SCHEMA_VERSION, 1);
        let site: PublishSite = serde_json::from_value(load_fixture("site.json")).unwrap();
        assert_eq!(site.schema_version, PUBLISH_SCHEMA_VERSION);
        let revision: PublishRevision =
            serde_json::from_value(load_fixture("revision.json")).unwrap();
        assert_eq!(revision.schema_version, PUBLISH_SCHEMA_VERSION);
        let bundle: PublishAiBundle =
            serde_json::from_value(load_fixture("ai-bundle.json")).unwrap();
        assert_eq!(bundle.schema_version, PUBLISH_SCHEMA_VERSION);
    }

    /// Codex pass-1 P2: `parse_versioned` rejects payloads whose
    /// advertised `schemaVersion` is newer than this binary knows.
    /// A doc-only constant is informational; this test pins that the
    /// guard fires at the consumption seam.
    #[test]
    fn parse_versioned_rejects_newer_than_known_schema_version() {
        let mut raw = load_fixture("site.json");
        raw["schemaVersion"] = serde_json::json!(99);
        let err = parse_versioned::<PublishSite>(&raw).unwrap_err();
        assert!(
            matches!(
                err,
                PublishContractError::UnsupportedNewerSchemaVersion {
                    actual: 99,
                    max: PUBLISH_SCHEMA_VERSION,
                },
            ),
            "expected UnsupportedNewerSchemaVersion(99), got {err:?}",
        );
    }

    #[test]
    fn parse_versioned_rejects_payload_missing_schema_version() {
        let raw = serde_json::json!({"siteId": "x"});
        let err = parse_versioned::<PublishSite>(&raw).unwrap_err();
        assert!(matches!(err, PublishContractError::MissingSchemaVersion));
    }

    #[test]
    fn parse_versioned_accepts_current_schema_version() {
        let raw = load_fixture("site.json");
        let site: PublishSite = parse_versioned(&raw).expect("current-version payload must parse");
        assert_eq!(site.schema_version, PUBLISH_SCHEMA_VERSION);
    }

    /// Codex pass-1 P2: `deny_unknown_fields` makes any forward-only
    /// field surface as a parse error rather than silently dropping
    /// it on round-trip. Pin that contract by feeding a fixture with
    /// an extra field and asserting parse fails.
    #[test]
    fn contract_types_deny_unknown_fields() {
        let mut raw = load_fixture("site.json");
        raw["totallyMadeUpField"] = serde_json::json!("future-only");
        let err = serde_json::from_value::<PublishSite>(raw)
            .expect_err("unknown field must surface as a parse error");
        let msg = err.to_string();
        assert!(
            msg.contains("totallyMadeUpField") && msg.contains("unknown field"),
            "expected unknown-field error, got: {msg}",
        );
    }

    /// Schema source-of-truth contract: the worker's mirror copy must
    /// be byte-equal to `sql/publish/0001_publish.sql`. The publish
    /// design doc names this the `publish_schema_contract` test.
    #[test]
    fn publish_schema_contract_worker_mirror_is_byte_equal() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let source = std::fs::read(manifest_dir.join("sql/publish/0001_publish.sql"))
            .expect("read sql/publish/0001_publish.sql");
        let worker = std::fs::read(manifest_dir.join("worker/migrations/0001_publish.sql"))
            .expect("read worker/migrations/0001_publish.sql");
        assert_eq!(
            source, worker,
            "worker/migrations/0001_publish.sql must be byte-equal to sql/publish/0001_publish.sql",
        );
    }
}
