//! Canonical lifecycle event types and shared hook-ingestion helpers.

use std::{
    collections::{BTreeMap, hash_map::DefaultHasher},
    fmt,
    hash::{Hash, Hasher},
};

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::internal::ai::session::SessionState;

/// Agent-agnostic lifecycle event kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LifecycleEventKind {
    SessionStart,
    TurnStart,
    ToolUse,
    ModelUpdate,
    Compaction,
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
            LifecycleEventKind::TurnEnd => "turn_end",
            LifecycleEventKind::SessionEnd => "session_end",
        };
        write!(f, "{value}")
    }
}

/// A normalized lifecycle event produced by a provider hook adapter.
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

/// Common hook payload envelope shared by provider-specific parsers.
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

fn find_string(payload: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(Value::String(value)) = payload.get(*key) {
            return Some(value.clone());
        }
    }
    None
}

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
        assert_eq!(LifecycleEventKind::TurnEnd.to_string(), "turn_end");
        assert_eq!(LifecycleEventKind::SessionEnd.to_string(), "session_end");
    }

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
