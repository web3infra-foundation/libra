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
#[serde(rename_all = "camelCase")]
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
#[serde(rename_all = "camelCase")]
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
#[serde(rename_all = "camelCase")]
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
#[serde(rename_all = "camelCase")]
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
#[serde(rename_all = "camelCase")]
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
#[serde(rename_all = "camelCase")]
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
#[serde(rename_all = "camelCase")]
pub struct AiObjectRelationship {
    pub kind: String,
    pub from_object_type: String,
    pub from_object_id: String,
    pub to_object_type: String,
    pub to_object_id: String,
}

/// Per-object redaction record.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiObjectRedaction {
    pub mode: RedactionMode,
    pub rules_version: String,
}

/// Aggregated AI bundle for one revision. Stored at
/// `{repo_id}/publish/sites/{site_id}/revisions/{revision_oid}/ai/bundles/{ai_version_id}.json`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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
#[serde(rename_all = "camelCase")]
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
#[serde(rename_all = "camelCase")]
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
#[serde(rename_all = "camelCase")]
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
#[serde(rename_all = "camelCase")]
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

/// Audit row for one sync invocation. Stored at
/// `{repo_id}/publish/sites/{site_id}/sync-runs/{sync_run_id}.json`
/// **and** as a row in `publish_sync_runs`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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
        // Round-trip the freshly serialised JSON back into the type
        // and assert structural equality. Equality of `Value`s would
        // catch field reorderings too, but BTreeMap-backed indexes
        // already give us deterministic order on the way out.
        let parsed_again: T = serde_json::from_value(reserialised.clone())
            .unwrap_or_else(|err| panic!("deserialize re-serialised {name}: {err}"));
        assert_eq!(
            parsed, parsed_again,
            "round-trip diverged for fixture {name}",
        );
        // Also assert that the reserialised JSON re-parses into the
        // same `Value` shape (catches accidental new fields the type
        // doesn't carry).
        let reparsed: serde_json::Value = serde_json::from_value(reserialised.clone())
            .unwrap_or_else(|err| panic!("Value reparse {name}: {err}"));
        assert_eq!(
            reserialised, reparsed,
            "reserialised JSON for {name} must reparse byte-equal",
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
