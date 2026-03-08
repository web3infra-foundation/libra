//! Hook payload parsers that convert provider-specific hook data into
//! normalized lifecycle events.

use std::{
    collections::{BTreeMap, hash_map::DefaultHasher},
    fmt,
    hash::{Hash, Hasher},
};

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::state::SessionState;

const CLAUDE_DEDUP_IDENTITY_KEYS: &[&str] = &[
    "event_id",
    "request_id",
    "turn_id",
    "message_id",
    "tool_use_id",
    "sequence",
    "timestamp",
];

const CLAUDE_LIFECYCLE_EVENTS: &[&str] = &["SessionStart", "Stop", "SessionStop", "SessionEnd"];

/// Agent-agnostic lifecycle event kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LifecycleEventKind {
    SessionStart,
    TurnStart,
    ToolUse,
    TurnEnd,
    SessionEnd,
}

impl fmt::Display for LifecycleEventKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            LifecycleEventKind::SessionStart => "session_start",
            LifecycleEventKind::TurnStart => "turn_start",
            LifecycleEventKind::ToolUse => "tool_use",
            LifecycleEventKind::TurnEnd => "turn_end",
            LifecycleEventKind::SessionEnd => "session_end",
        };
        write!(f, "{value}")
    }
}

/// A normalized lifecycle event produced by an agent hook parser.
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

/// Each agent implements this to convert raw hook payloads into [`LifecycleEvent`].
pub trait AgentHookParser {
    fn source_name(&self) -> &'static str;

    fn parse_hook_event(
        &self,
        hook_event_name: &str,
        envelope: &SessionHookEnvelope,
    ) -> Result<LifecycleEvent>;

    fn dedup_identity_keys(&self) -> &'static [&'static str];
    fn lifecycle_fallback_events(&self) -> &'static [&'static str];
}

/// Claude Code parser implementation.
#[derive(Debug, Default, Clone, Copy)]
pub struct ClaudeCodeAgentParser;

impl AgentHookParser for ClaudeCodeAgentParser {
    fn source_name(&self) -> &'static str {
        "claude_code_hook"
    }

    fn parse_hook_event(
        &self,
        hook_event_name: &str,
        envelope: &SessionHookEnvelope,
    ) -> Result<LifecycleEvent> {
        let kind = match hook_event_name {
            "SessionStart" => LifecycleEventKind::SessionStart,
            "UserPromptSubmit" => LifecycleEventKind::TurnStart,
            "PostToolUse" => LifecycleEventKind::ToolUse,
            "Stop" | "SessionStop" => LifecycleEventKind::TurnEnd,
            "SessionEnd" => LifecycleEventKind::SessionEnd,
            other => bail!("unknown Claude Code hook event: '{other}'"),
        };

        Ok(LifecycleEvent {
            kind,
            session_id: envelope.session_id.clone(),
            session_ref: envelope.transcript_path.clone(),
            prompt: find_string(&envelope.extra, &["prompt", "message", "user_prompt"]),
            model: envelope.extra.get("model").cloned(),
            source: envelope.extra.get("source").cloned(),
            tool_name: find_string(&envelope.extra, &["tool_name"]),
            tool_input: envelope.extra.get("tool_input").cloned(),
            tool_response: envelope.extra.get("tool_response").cloned(),
            assistant_message: find_string(
                &envelope.extra,
                &["last_assistant_message", "assistant_message", "message"],
            ),
            timestamp: Utc::now(),
        })
    }

    fn dedup_identity_keys(&self) -> &'static [&'static str] {
        CLAUDE_DEDUP_IDENTITY_KEYS
    }

    fn lifecycle_fallback_events(&self) -> &'static [&'static str] {
        CLAUDE_LIFECYCLE_EVENTS
    }
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
    let raw_events = session
        .metadata
        .entry("raw_hook_events".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Value::Array(events) = raw_events {
        events.push(json!({
            "hook_event_name": envelope.hook_event_name,
            "payload": normalize_json_value(Value::Object(envelope.extra.clone())),
        }));
        if events.len() > max_raw_hook_events {
            let drop_n = events.len() - max_raw_hook_events;
            events.drain(0..drop_n);
        }
    }
}

/// Apply a normalized lifecycle event to in-memory session state.
pub fn apply_lifecycle_event(
    session: &mut SessionState,
    event: &LifecycleEvent,
    max_tool_events: usize,
) {
    match event.kind {
        LifecycleEventKind::SessionStart => {
            if let Some(v) = &event.model {
                session.metadata.insert("model".to_string(), v.clone());
            }
            if let Some(v) = &event.source {
                session.metadata.insert("source".to_string(), v.clone());
            }
        }
        LifecycleEventKind::TurnStart => {
            if let Some(prompt) = &event.prompt {
                session.add_user_message(prompt);
            }
        }
        LifecycleEventKind::ToolUse => {
            let tools_entry = session
                .metadata
                .entry("tool_events".to_string())
                .or_insert_with(|| Value::Array(Vec::new()));
            if let Value::Array(items) = tools_entry {
                items.push(json!({
                    "tool_name": event.tool_name.clone().unwrap_or_default(),
                    "input": event.tool_input.clone().unwrap_or(Value::Null),
                    "response": event.tool_response.clone().unwrap_or(Value::Null),
                    "timestamp": event.timestamp.to_rfc3339(),
                }));
                if items.len() > max_tool_events {
                    let drop_n = items.len() - max_tool_events;
                    items.drain(0..drop_n);
                }
            }
        }
        LifecycleEventKind::TurnEnd => {
            if let Some(msg) = &event.assistant_message {
                session
                    .metadata
                    .insert("last_assistant_message".to_string(), json!(msg));
                let should_append = session
                    .messages
                    .last()
                    .map(|m| m.role != "assistant" || m.content != *msg)
                    .unwrap_or(true);
                if should_append {
                    session.add_assistant_message(msg);
                }
            }
        }
        LifecycleEventKind::SessionEnd => {}
    }
}

/// Build dedup key with parser-specific identity and lifecycle rules.
pub fn make_dedup_key(
    parser: &impl AgentHookParser,
    envelope: &SessionHookEnvelope,
) -> Option<String> {
    make_event_key(parser, envelope).or_else(|| make_lifecycle_fallback_key(parser, envelope))
}

/// Canonicalize JSON values for deterministic hashing/serialization.
pub fn normalize_json_value(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted: BTreeMap<String, Value> = map
                .into_iter()
                .map(|(k, v)| (k, normalize_json_value(v)))
                .collect();
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(normalize_json_value).collect()),
        scalar => scalar,
    }
}

fn validate_session_id(session_id: &str) -> Result<()> {
    if session_id.len() > 128 {
        bail!("invalid session_id: length exceeds 128 characters");
    }
    if session_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Ok(());
    }
    bail!("invalid session_id: only [A-Za-z0-9._-] are allowed")
}

fn validate_transcript_path(path: &str, max_transcript_path_bytes: usize) -> Result<()> {
    if path.trim().is_empty() {
        bail!("invalid transcript_path: empty value");
    }
    if path.len() > max_transcript_path_bytes {
        bail!("invalid transcript_path: length exceeds {max_transcript_path_bytes} characters");
    }
    if path.contains('\0') {
        bail!("invalid transcript_path: contains NUL byte");
    }
    Ok(())
}

fn dedup_identity_value(value: &Value) -> Option<String> {
    match value {
        Value::String(v) => Some(v.clone()),
        Value::Number(v) => Some(v.to_string()),
        Value::Bool(v) => Some(v.to_string()),
        _ => None,
    }
}

fn make_event_key(parser: &impl AgentHookParser, envelope: &SessionHookEnvelope) -> Option<String> {
    let identity = parser.dedup_identity_keys().iter().find_map(|key| {
        envelope
            .extra
            .get(*key)
            .and_then(dedup_identity_value)
            .map(|value| format!("{key}:{value}"))
    })?;

    // DefaultHasher output is only used for short-lived in-session dedup keys.
    let mut hasher = DefaultHasher::new();
    envelope.hook_event_name.hash(&mut hasher);
    envelope.session_id.hash(&mut hasher);
    identity.hash(&mut hasher);
    let normalized = normalize_json_value(Value::Object(envelope.extra.clone()));
    if let Ok(canonical_extra) = serde_json::to_string(&normalized) {
        canonical_extra.hash(&mut hasher);
    }

    Some(format!(
        "{}:{}:{:x}",
        envelope.hook_event_name,
        envelope.session_id,
        hasher.finish()
    ))
}

fn make_lifecycle_fallback_key(
    parser: &impl AgentHookParser,
    envelope: &SessionHookEnvelope,
) -> Option<String> {
    if !parser
        .lifecycle_fallback_events()
        .contains(&envelope.hook_event_name.as_str())
    {
        return None;
    }

    let mut hasher = DefaultHasher::new();
    envelope.hook_event_name.hash(&mut hasher);
    envelope.session_id.hash(&mut hasher);
    let normalized = normalize_json_value(Value::Object(envelope.extra.clone()));
    if let Ok(canonical_extra) = serde_json::to_string(&normalized) {
        canonical_extra.hash(&mut hasher);
    }

    Some(format!(
        "lifecycle:{}:{}:{:x}",
        envelope.hook_event_name,
        envelope.session_id,
        hasher.finish()
    ))
}

fn find_string(payload: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(Value::String(v)) = payload.get(*key) {
            return Some(v.clone());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_parser_maps_all_hooks() {
        let parser = ClaudeCodeAgentParser;
        let envelope = SessionHookEnvelope {
            hook_event_name: "SessionStart".to_string(),
            session_id: "s1".to_string(),
            cwd: "/tmp".to_string(),
            transcript_path: Some("/tmp/transcript.jsonl".to_string()),
            extra: {
                let mut map = Map::new();
                map.insert("prompt".to_string(), Value::String("hello".to_string()));
                map.insert(
                    "tool_name".to_string(),
                    Value::String("read_file".to_string()),
                );
                map.insert("tool_input".to_string(), json!({"path": "a.txt"}));
                map.insert("tool_response".to_string(), json!({"ok": true}));
                map.insert(
                    "last_assistant_message".to_string(),
                    Value::String("done".to_string()),
                );
                map
            },
        };

        let cases = vec![
            ("SessionStart", LifecycleEventKind::SessionStart),
            ("UserPromptSubmit", LifecycleEventKind::TurnStart),
            ("PostToolUse", LifecycleEventKind::ToolUse),
            ("Stop", LifecycleEventKind::TurnEnd),
            ("SessionEnd", LifecycleEventKind::SessionEnd),
        ];

        for (name, kind) in cases {
            let event = parser
                .parse_hook_event(name, &envelope)
                .expect("parse should succeed");
            assert_eq!(event.kind, kind);
        }
    }

    #[test]
    fn claude_parser_rejects_unknown_hook() {
        let parser = ClaudeCodeAgentParser;
        let envelope = SessionHookEnvelope {
            hook_event_name: "UnknownHook".to_string(),
            session_id: "s1".to_string(),
            cwd: "/tmp".to_string(),
            transcript_path: None,
            extra: Map::new(),
        };

        assert!(parser.parse_hook_event("UnknownHook", &envelope).is_err());
    }

    #[test]
    fn lifecycle_event_kind_display() {
        assert_eq!(
            LifecycleEventKind::SessionStart.to_string(),
            "session_start"
        );
        assert_eq!(LifecycleEventKind::TurnStart.to_string(), "turn_start");
        assert_eq!(LifecycleEventKind::ToolUse.to_string(), "tool_use");
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
        let parser = ClaudeCodeAgentParser;
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
        assert!(make_dedup_key(&parser, &with_identity).is_some());

        let lifecycle_no_identity = SessionHookEnvelope {
            hook_event_name: "SessionStart".to_string(),
            session_id: "s1".to_string(),
            cwd: "/tmp".to_string(),
            transcript_path: None,
            extra: Map::new(),
        };
        assert!(make_dedup_key(&parser, &lifecycle_no_identity).is_some());
    }
}
