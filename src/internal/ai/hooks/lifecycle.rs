//! Canonical lifecycle event types and shared hook-ingestion helpers.
//!
//! Provider-specific hook adapters (Claude, Gemini, etc.) parse their own JSON wire
//! formats and lower them into the agent-agnostic [`LifecycleEvent`] structure
//! defined here. Downstream code (session storage, projection, audit logs) consumes
//! only this normalised form, so adding a new provider does not require changes to
//! the rest of the agent pipeline.
//!
//! Helpers in this module also enforce envelope validation (preventing path
//! traversal in `session_id`, capping `transcript_path` length) and produce stable
//! dedup keys so duplicate hook deliveries can be filtered out at ingestion time.

use std::{
    collections::{BTreeMap, hash_map::DefaultHasher},
    fmt,
    hash::{Hash, Hasher},
};

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::internal::ai::{runtime::event::Event, session::SessionState};

/// Agent-agnostic lifecycle event kinds.
///
/// These map roughly onto the union of every provider's hook taxonomy. Variants are
/// intentionally finer-grained than the public `HookEvent` enum so that internal
/// projection logic can distinguish, for example, `TurnStart` (a new user prompt
/// arrived) from `Compaction` (the model summarised history to fit its context
/// window).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LifecycleEventKind {
    SessionStart,
    TurnStart,
    ToolUse,
    ModelUpdate,
    Compaction,
    CompactionCompleted,
    PermissionRequest,
    SourceEnabled,
    SourceDisabled,
    TurnEnd,
    SessionEnd,
}

impl fmt::Display for LifecycleEventKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            LifecycleEventKind::SessionStart => "session_start",
            LifecycleEventKind::TurnStart => "turn_start",
            LifecycleEventKind::ToolUse => "tool_use",
            LifecycleEventKind::ModelUpdate => "model_update",
            LifecycleEventKind::Compaction => "compaction",
            LifecycleEventKind::CompactionCompleted => "compaction_completed",
            LifecycleEventKind::PermissionRequest => "permission_request",
            LifecycleEventKind::SourceEnabled => "source_enabled",
            LifecycleEventKind::SourceDisabled => "source_disabled",
            LifecycleEventKind::TurnEnd => "turn_end",
            LifecycleEventKind::SessionEnd => "session_end",
        };
        write!(f, "{value}")
    }
}

/// A normalized lifecycle event produced by a provider hook adapter.
///
/// All optional fields are populated only when the source envelope supplies them.
/// `timestamp` is recorded at parse time, not from the envelope, to keep ordering
/// authoritative even when providers omit it.
#[derive(Debug, Clone, PartialEq)]
pub struct LifecycleEvent {
    pub kind: LifecycleEventKind,
    pub session_id: String,
    pub session_ref: Option<String>,
    pub prompt: Option<String>,
    pub model: Option<Value>,
    pub source: Option<Value>,
    pub tool_name: Option<String>,
    pub tool_input: Option<Value>,
    pub tool_response: Option<Value>,
    pub assistant_message: Option<String>,
    pub timestamp: DateTime<Utc>,
}

/// Fixed v4 UUID acting as the namespace for `Uuid::new_v5` derivation of
/// `LifecycleEvent::event_id`.
///
/// **Stability contract**: this constant **must not change**. Any change is
/// equivalent to renumbering every previously-emitted lifecycle event id and
/// would break audit-log dedupe / correlation. If a future migration ever
/// needs a different namespace, ship it as `LIFECYCLE_EVENT_NAMESPACE_V2` and
/// version the `event_id` derivation explicitly.
const LIFECYCLE_EVENT_NAMESPACE: Uuid = Uuid::from_bytes([
    0x4d, 0xe6, 0x3a, 0x6b, // libra
    0x8a, 0x12, // lifecycle
    0x4f, 0x77, // namespace v1
    0x9c, 0x4b, // (random — frozen)
    0x05, 0x02, 0x06, 0x01, 0x10, 0x05, // 2026-05-02 round 2
]);

impl Event for LifecycleEvent {
    fn event_kind(&self) -> &'static str {
        // Stable wire kinds — keep this match in sync with
        // `LifecycleEventKind::Display` so audit / projection consumers see
        // a single canonical form.
        match self.kind {
            LifecycleEventKind::SessionStart => "session_start",
            LifecycleEventKind::TurnStart => "turn_start",
            LifecycleEventKind::ToolUse => "tool_use",
            LifecycleEventKind::ModelUpdate => "model_update",
            LifecycleEventKind::Compaction => "compaction",
            LifecycleEventKind::CompactionCompleted => "compaction_completed",
            LifecycleEventKind::PermissionRequest => "permission_request",
            LifecycleEventKind::SourceEnabled => "source_enabled",
            LifecycleEventKind::SourceDisabled => "source_disabled",
            LifecycleEventKind::TurnEnd => "turn_end",
            LifecycleEventKind::SessionEnd => "session_end",
        }
    }

    fn event_id(&self) -> Uuid {
        // Lifecycle events do not have a provider-assigned UUID, but generic
        // dedupe / indexing code expects `event_id()` to be stable per
        // occurrence. Derive a deterministic UUID v5 from the event's
        // natural identity tuple `(session_id, timestamp_nanos, kind)` so
        // two structurally identical events hash to the same id and two
        // distinct events do not collide. (CEX-00.5 Codex review P2 fix,
        // round 2: switched from `DefaultHasher` to `Uuid::new_v5` because
        // `DefaultHasher` is documented as not stable across Rust releases
        // and `event_id` may end up persisted in audit logs.)
        //
        // `LIFECYCLE_EVENT_NAMESPACE` is a fixed v4 UUID generated for the
        // libra runtime; it acts as the SHA-1 namespace for v5 derivation
        // and **must not change** without a coordinated audit-log migration.
        let mut name = Vec::with_capacity(self.session_id.len() + 32);
        name.extend_from_slice(self.session_id.as_bytes());
        name.push(0u8);
        name.extend_from_slice(
            &self
                .timestamp
                .timestamp_nanos_opt()
                .unwrap_or(0)
                .to_be_bytes(),
        );
        name.push(0u8);
        name.push(self.kind as u8);
        Uuid::new_v5(&LIFECYCLE_EVENT_NAMESPACE, &name)
    }

    fn event_summary(&self) -> String {
        let mut parts: Vec<String> = vec![
            format!("kind={}", self.kind),
            format!("session={}", self.session_id),
        ];
        if let Some(tool) = self.tool_name.as_deref() {
            parts.push(format!("tool={tool}"));
        }
        if let Some(model) = self.model.as_ref().and_then(|v| v.as_str()) {
            parts.push(format!("model={model}"));
        }
        parts.join(" ")
    }
}

/// Common hook payload envelope shared by provider-specific parsers.
///
/// The `#[serde(flatten)]` `extra` map captures any provider-specific keys that the
/// canonical envelope does not name explicitly, allowing downstream parsers to look
/// up `tool_name`, `prompt`, `model`, etc. without forcing every provider to share
/// an identical wire schema.
#[derive(Debug, Deserialize, Clone)]
pub struct SessionHookEnvelope {
    pub hook_event_name: String,
    pub session_id: String,
    pub cwd: String,
    #[serde(default)]
    pub transcript_path: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Core envelope validation shared by all providers.
///
/// Functional scope:
/// - Verifies the four required fields (`hook_event_name`, `session_id`, `cwd`,
///   and optionally a non-empty `transcript_path`) are populated.
/// - Restricts `session_id` to a safe ASCII alphabet so it can be used as part of
///   on-disk paths without sanitisation.
/// - Caps `transcript_path` length and rejects embedded NUL bytes to defend
///   downstream FS code from unsafe paths.
///
/// Boundary conditions:
/// - Returns `Err` (via `bail!`) on the first failed check; callers see one
///   actionable message rather than a list.
/// - Whitespace-only fields are treated as empty.
pub fn validate_session_hook_envelope(
    envelope: &SessionHookEnvelope,
    max_transcript_path_bytes: usize,
) -> Result<()> {
    if envelope.hook_event_name.trim().is_empty() {
        bail!("missing required field: hook_event_name");
    }
    if envelope.session_id.trim().is_empty() {
        bail!("missing required field: session_id");
    }
    validate_session_id(&envelope.session_id)?;
    if envelope.cwd.trim().is_empty() {
        bail!("missing required field: cwd");
    }
    if let Some(transcript_path) = envelope.transcript_path.as_deref() {
        validate_transcript_path(transcript_path, max_transcript_path_bytes)?;
    }
    Ok(())
}

/// Append normalized raw event fragments for audit/debug.
///
/// Functional scope:
/// - Stashes a JSON snapshot of the inbound envelope into
///   `session.metadata["raw_hook_events"]`, preserving the on-the-wire shape for
///   later inspection.
/// - Bounds the array to `max_raw_hook_events`, dropping the oldest entries once
///   the cap is reached.
///
/// Boundary conditions:
/// - If the metadata slot exists but is not a JSON array (schema drift from an
///   older session), it is overwritten with a fresh single-element array rather
///   than panicking.
pub fn append_raw_hook_event(
    session: &mut SessionState,
    envelope: &SessionHookEnvelope,
    max_raw_hook_events: usize,
) {
    let entry = session
        .metadata
        .entry("raw_hook_events".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));

    let raw = json!({
        "hook_event_name": envelope.hook_event_name,
        "session_id": envelope.session_id,
        "cwd": envelope.cwd,
        "transcript_path": envelope.transcript_path,
        "extra": envelope.extra,
        "timestamp": Utc::now().to_rfc3339(),
    });

    let Value::Array(items) = entry else {
        session
            .metadata
            .insert("raw_hook_events".to_string(), Value::Array(vec![raw]));
        return;
    };

    items.push(raw);
    if items.len() > max_raw_hook_events {
        let drop_n = items.len() - max_raw_hook_events;
        items.drain(0..drop_n);
    }
}

/// Apply a normalized lifecycle event to the in-memory session state.
///
/// Functional scope:
/// - Mutates `session` in place to reflect the event:
///   - `SessionStart` / `ModelUpdate` write `model` and `source` metadata.
///   - `TurnStart` appends a user message when a prompt is present.
///   - `ToolUse` appends to a bounded `tool_events` array.
///   - `Compaction` increments a counter so the UI can flag context compaction.
///   - `TurnEnd` records the final assistant message.
///   - `SessionEnd` is a no-op marker (state is flushed by the caller).
///
/// Boundary conditions:
/// - The `tool_events` array uses the same defensive overwrite path as
///   [`append_raw_hook_event`] when the slot is the wrong JSON shape.
/// - `tool_events` is capped at `max_tool_events`; oldest entries are dropped to
///   keep memory and persistence size bounded.
pub fn apply_lifecycle_event(
    session: &mut SessionState,
    event: &LifecycleEvent,
    max_tool_events: usize,
) {
    match event.kind {
        LifecycleEventKind::SessionStart => {
            if let Some(model) = &event.model {
                session
                    .metadata
                    .insert("model".to_string(), normalize_json_value(model.clone()));
            }
            if let Some(source) = &event.source {
                session
                    .metadata
                    .insert("source".to_string(), normalize_json_value(source.clone()));
            }
        }
        LifecycleEventKind::TurnStart => {
            if let Some(prompt) = &event.prompt {
                session.add_user_message(prompt);
            }
        }
        LifecycleEventKind::ToolUse => {
            let tool_event = json!({
                "name": event.tool_name,
                "input": event.tool_input,
                "response": event.tool_response,
                "timestamp": event.timestamp.to_rfc3339(),
            });

            let entry = session
                .metadata
                .entry("tool_events".to_string())
                .or_insert_with(|| Value::Array(Vec::new()));
            let Value::Array(items) = entry else {
                session
                    .metadata
                    .insert("tool_events".to_string(), Value::Array(vec![tool_event]));
                return;
            };
            items.push(tool_event);
            if items.len() > max_tool_events {
                let drop_n = items.len() - max_tool_events;
                items.drain(0..drop_n);
            }
        }
        LifecycleEventKind::ModelUpdate => {
            if let Some(model) = &event.model {
                session
                    .metadata
                    .insert("model".to_string(), normalize_json_value(model.clone()));
            }
        }
        LifecycleEventKind::Compaction => {
            let current = session
                .metadata
                .get("compaction_count")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            session
                .metadata
                .insert("compaction_count".to_string(), json!(current + 1));
        }
        LifecycleEventKind::CompactionCompleted => {
            session.metadata.insert(
                "last_compaction_completed_at".to_string(),
                json!(event.timestamp),
            );
        }
        LifecycleEventKind::PermissionRequest
        | LifecycleEventKind::SourceEnabled
        | LifecycleEventKind::SourceDisabled => {
            let entry = json!({
                "kind": event.kind.to_string(),
                "source": event.source,
                "tool": event.tool_name,
                "timestamp": event.timestamp.to_rfc3339(),
            });
            let slot = session
                .metadata
                .entry("automation_events".to_string())
                .or_insert_with(|| Value::Array(Vec::new()));
            let Value::Array(items) = slot else {
                session
                    .metadata
                    .insert("automation_events".to_string(), Value::Array(vec![entry]));
                return;
            };
            items.push(entry);
        }
        LifecycleEventKind::TurnEnd => {
            if let Some(message) = &event.assistant_message {
                session.add_assistant_message(message);
                session
                    .metadata
                    .insert("last_assistant_message".to_string(), json!(message));
            }
        }
        LifecycleEventKind::SessionEnd => {}
    }
}

/// Build a dedup key using provider-configured identity fields and lifecycle fallbacks.
///
/// Functional scope:
/// - Walks `identity_keys` in order; the first non-null match becomes the
///   primary identity for the event and is hashed together with the envelope.
/// - When no identity field is found but the event name is in
///   `lifecycle_fallback_events`, falls back to `session_id` so events that
///   genuinely repeat per-session (e.g. SessionStart) still produce a stable key.
///
/// Boundary conditions:
/// - Returns `None` when no identity key matches and the event is not in the
///   fallback list — callers may then choose to forward the event without dedup.
/// - The hash mixes in `session_id`, `cwd`, `transcript_path`, and the full
///   `extra` map so that semantically distinct payloads never collide.
pub fn make_dedup_key(
    identity_keys: &[&str],
    lifecycle_fallback_events: &[&str],
    envelope: &SessionHookEnvelope,
) -> Option<String> {
    for key in identity_keys {
        if let Some(value) = envelope.extra.get(*key)
            && !value.is_null()
        {
            return Some(make_event_key(
                &envelope.hook_event_name,
                key,
                value,
                envelope,
            ));
        }
    }

    if lifecycle_fallback_events.contains(&envelope.hook_event_name.as_str()) {
        return Some(make_event_key(
            &envelope.hook_event_name,
            "session_id",
            &Value::String(envelope.session_id.clone()),
            envelope,
        ));
    }

    None
}

/// Canonicalize JSON for deterministic blob generation.
///
/// Functional scope:
/// - Recursively sorts object keys alphabetically using a `BTreeMap` so that two
///   semantically equal payloads always serialise to the same byte sequence.
/// - Arrays preserve order; only object key order is normalised.
///
/// Boundary conditions:
/// - Numbers, strings, booleans, and nulls are returned untouched.
/// - The function is `O(n log n)` in total node count due to the per-object sort.
pub fn normalize_json_value(value: Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.into_iter().map(normalize_json_value).collect()),
        Value::Object(map) => {
            let normalized = map
                .into_iter()
                .map(|(key, value)| (key, normalize_json_value(value)))
                .collect::<BTreeMap<_, _>>();
            Value::Object(normalized.into_iter().collect())
        }
        other => other,
    }
}

/// Build a [`LifecycleEvent`] from a parsed envelope of the given kind.
///
/// Functional scope: probes a small set of well-known field names in `envelope.extra`
/// (`prompt`/`message`/`user_prompt`, `tool_input`/`tool_request`,
/// `tool_response`/`tool_result`, etc.) so that providers using slightly different
/// vocabulary still flow through the same downstream pipeline.
///
/// Boundary conditions: any missing field is left as `None` so callers can detect
/// and ignore events that don't carry the expected payload.
pub(crate) fn build_lifecycle_event(
    kind: LifecycleEventKind,
    envelope: &SessionHookEnvelope,
) -> LifecycleEvent {
    LifecycleEvent {
        kind,
        session_id: envelope.session_id.clone(),
        session_ref: envelope.transcript_path.clone(),
        prompt: find_string(&envelope.extra, &["prompt", "message", "user_prompt"]),
        model: extract_model(&envelope.extra),
        source: envelope.extra.get("source").cloned(),
        tool_name: find_string(&envelope.extra, &["tool_name", "tool"]),
        tool_input: envelope
            .extra
            .get("tool_input")
            .cloned()
            .or_else(|| envelope.extra.get("tool_request").cloned()),
        tool_response: envelope
            .extra
            .get("tool_response")
            .cloned()
            .or_else(|| envelope.extra.get("tool_result").cloned()),
        assistant_message: find_string(
            &envelope.extra,
            &["last_assistant_message", "assistant_message", "message"],
        ),
        timestamp: Utc::now(),
    }
}

/// Hash an event into a stable dedup key.
///
/// The textual prefix `event:key:` is preserved so logs remain human-readable;
/// the trailing hex digest is the deterministic `DefaultHasher` digest of the
/// canonicalised payload.
fn make_event_key(
    event_name: &str,
    key_name: &str,
    value: &Value,
    envelope: &SessionHookEnvelope,
) -> String {
    let mut hasher = DefaultHasher::new();
    event_name.hash(&mut hasher);
    key_name.hash(&mut hasher);
    normalize_json_value(value.clone())
        .to_string()
        .hash(&mut hasher);
    envelope.session_id.hash(&mut hasher);
    envelope.cwd.hash(&mut hasher);
    envelope.transcript_path.hash(&mut hasher);
    normalize_json_value(Value::Object(envelope.extra.clone()))
        .to_string()
        .hash(&mut hasher);
    format!("{event_name}:{key_name}:{:x}", hasher.finish())
}

/// Reject session IDs that are too long or contain unsafe characters.
///
/// Functional scope: limits the alphabet to `[A-Za-z0-9._-]` and the length to 128
/// characters. The conservative alphabet keeps session IDs usable as filename
/// segments without per-platform escaping.
fn validate_session_id(session_id: &str) -> Result<()> {
    if session_id.len() > 128 {
        bail!("invalid session_id: exceeds 128 characters");
    }
    if !session_id
        .chars()
        .all(|char| char.is_ascii_alphanumeric() || matches!(char, '.' | '_' | '-'))
    {
        bail!("invalid session_id: only [A-Za-z0-9._-] is allowed");
    }
    Ok(())
}

/// Verify the transcript path is plausible: non-empty, no NUL byte, bounded length.
///
/// Boundary conditions: the upper bound is configurable so embedding contexts that
/// know they only see short transcript paths can tighten the limit.
fn validate_transcript_path(transcript_path: &str, max_transcript_path_bytes: usize) -> Result<()> {
    if transcript_path.trim().is_empty() {
        bail!("invalid transcript_path: value cannot be empty");
    }
    if transcript_path.len() > max_transcript_path_bytes {
        bail!(
            "invalid transcript_path: exceeds {} bytes",
            max_transcript_path_bytes
        );
    }
    if transcript_path.contains('\0') {
        bail!("invalid transcript_path: contains NUL byte");
    }
    Ok(())
}

/// Return the first string value found among the listed keys, if any.
///
/// Used to fall back across renamed fields between provider versions without forcing
/// the rest of the parser to know each provider's vocabulary.
fn find_string(payload: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(Value::String(value)) = payload.get(*key) {
            return Some(value.clone());
        }
    }
    None
}

/// Locate the model identifier in either the top-level `model` field or inside the
/// nested `llm_request.model` object emitted by some providers.
fn extract_model(payload: &Map<String, Value>) -> Option<Value> {
    if let Some(model) = payload.get("model") {
        return Some(model.clone());
    }

    payload
        .get("llm_request")
        .and_then(Value::as_object)
        .and_then(|request| request.get("model"))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Scenario: each lifecycle kind formats as the documented snake_case string.
    #[test]
    fn lifecycle_event_kind_display() {
        assert_eq!(
            LifecycleEventKind::SessionStart.to_string(),
            "session_start"
        );
        assert_eq!(LifecycleEventKind::TurnStart.to_string(), "turn_start");
        assert_eq!(LifecycleEventKind::ToolUse.to_string(), "tool_use");
        assert_eq!(LifecycleEventKind::ModelUpdate.to_string(), "model_update");
        assert_eq!(LifecycleEventKind::Compaction.to_string(), "compaction");
        assert_eq!(
            LifecycleEventKind::CompactionCompleted.to_string(),
            "compaction_completed"
        );
        assert_eq!(
            LifecycleEventKind::PermissionRequest.to_string(),
            "permission_request"
        );
        assert_eq!(
            LifecycleEventKind::SourceEnabled.to_string(),
            "source_enabled"
        );
        assert_eq!(
            LifecycleEventKind::SourceDisabled.to_string(),
            "source_disabled"
        );
        assert_eq!(LifecycleEventKind::TurnEnd.to_string(), "turn_end");
        assert_eq!(LifecycleEventKind::SessionEnd.to_string(), "session_end");
    }

    // Scenario: a path-traversal style session ID is rejected by the validator.
    #[test]
    fn validate_envelope_rejects_bad_session_id() {
        let envelope = SessionHookEnvelope {
            hook_event_name: "SessionStart".to_string(),
            session_id: "../bad".to_string(),
            cwd: "/tmp".to_string(),
            transcript_path: None,
            extra: Map::new(),
        };
        assert!(validate_session_hook_envelope(&envelope, 4096).is_err());
    }

    // Scenario: when an identity field is present it wins; otherwise the lifecycle
    // fallback kicks in for events listed as fallback-eligible.
    #[test]
    fn make_dedup_key_identity_then_lifecycle_fallback() {
        let with_identity = SessionHookEnvelope {
            hook_event_name: "UserPromptSubmit".to_string(),
            session_id: "s1".to_string(),
            cwd: "/tmp".to_string(),
            transcript_path: None,
            extra: {
                let mut map = Map::new();
                map.insert("event_id".to_string(), Value::String("evt-1".to_string()));
                map
            },
        };
        assert!(make_dedup_key(&["event_id"], &["SessionStart"], &with_identity).is_some());

        let lifecycle_no_identity = SessionHookEnvelope {
            hook_event_name: "SessionStart".to_string(),
            session_id: "s1".to_string(),
            cwd: "/tmp".to_string(),
            transcript_path: None,
            extra: Map::new(),
        };
        assert!(make_dedup_key(&["event_id"], &["SessionStart"], &lifecycle_no_identity).is_some());
    }

    // Scenario: payload differences must produce different dedup keys, otherwise
    // distinct events would be silently merged.
    #[test]
    fn make_dedup_key_changes_when_payload_changes() {
        let first = SessionHookEnvelope {
            hook_event_name: "Compaction".to_string(),
            session_id: "s1".to_string(),
            cwd: "/tmp".to_string(),
            transcript_path: None,
            extra: {
                let mut map = Map::new();
                map.insert("message".to_string(), Value::String("one".to_string()));
                map
            },
        };
        let second = SessionHookEnvelope {
            hook_event_name: "Compaction".to_string(),
            session_id: "s1".to_string(),
            cwd: "/tmp".to_string(),
            transcript_path: None,
            extra: {
                let mut map = Map::new();
                map.insert("message".to_string(), Value::String("two".to_string()));
                map
            },
        };

        assert_ne!(
            make_dedup_key(&["event_id"], &["Compaction"], &first),
            make_dedup_key(&["event_id"], &["Compaction"], &second)
        );
    }

    // Scenario: object keys are sorted recursively so canonical JSON is stable.
    #[test]
    fn normalize_value_sorts_object_keys() {
        let value = json!({
            "z": 1,
            "a": {
                "k2": 2,
                "k1": 1
            }
        });

        let canonical = serde_json::to_string(&normalize_json_value(value)).unwrap();
        assert_eq!(canonical, r#"{"a":{"k1":1,"k2":2},"z":1}"#);
    }

    // Scenario: a transcript path containing a NUL byte is rejected as unsafe.
    #[test]
    fn validate_envelope_rejects_invalid_transcript_path() {
        let envelope = SessionHookEnvelope {
            hook_event_name: "SessionStart".to_string(),
            session_id: "s1".to_string(),
            cwd: "/tmp".to_string(),
            transcript_path: Some("\0bad".to_string()),
            extra: Map::new(),
        };
        assert!(validate_session_hook_envelope(&envelope, 4096).is_err());
    }

    // Scenario: a transcript path that's only whitespace is treated as empty.
    #[test]
    fn validate_envelope_rejects_empty_transcript_path() {
        let envelope = SessionHookEnvelope {
            hook_event_name: "SessionStart".to_string(),
            session_id: "s1".to_string(),
            cwd: "/tmp".to_string(),
            transcript_path: Some("   ".to_string()),
            extra: Map::new(),
        };
        assert!(validate_session_hook_envelope(&envelope, 4096).is_err());
    }
}
