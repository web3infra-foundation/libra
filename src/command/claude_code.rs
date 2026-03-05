//! Entry point for ingesting Claude Code hook events and persisting them as AI history.

use std::{
    collections::{BTreeMap, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
    io::Read,
    sync::Arc,
};

use chrono::Utc;
use clap::{Parser, Subcommand};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::{
    internal::{
        ai::{history::HistoryManager, session::SessionState},
        db,
    },
    utils::{object::write_git_object, storage::local::LocalStorage, util},
};

const PROCESSED_EVENT_KEYS: &str = "processed_event_keys";
const PROCESSED_EVENT_KEYS_MAX: usize = 200;
const CLAUDE_SESSION_TYPE: &str = "claude_session";

#[derive(Subcommand, Debug)]
pub enum ClaudeCodeCommand {
    #[command(about = "Handle SessionStart hook event")]
    SessionStart(ClaudeCodeArgs),
    #[command(about = "Handle UserPromptSubmit hook event")]
    Prompt(ClaudeCodeArgs),
    #[command(about = "Handle PostToolUse hook event")]
    ToolUse(ClaudeCodeArgs),
    #[command(about = "Handle Stop hook event")]
    Stop(ClaudeCodeArgs),
    #[command(about = "Handle SessionEnd hook event")]
    SessionEnd(ClaudeCodeArgs),
}

#[derive(Parser, Debug, Clone)]
pub struct ClaudeCodeArgs {}

#[derive(Debug, Deserialize)]
struct HookEnvelope {
    hook_event_name: String,
    session_id: String,
    cwd: String,
    #[serde(default)]
    transcript_path: Option<String>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClaudeSessionPhase {
    Active,
    Stopped,
    Ended,
}

#[derive(Debug)]
struct PersistOutcome {
    object_hash: String,
    already_exists: bool,
}

pub async fn execute(cmd: ClaudeCodeCommand) -> Result<(), String> {
    let mut stdin = String::new();
    std::io::stdin()
        .read_to_string(&mut stdin)
        .map_err(|e| format!("failed to read stdin: {e}"))?;

    if stdin.trim().is_empty() {
        return Err("hook input is empty".to_string());
    }

    let envelope: HookEnvelope =
        serde_json::from_str(&stdin).map_err(|e| format!("invalid hook JSON payload: {e}"))?;

    validate_core_fields(&envelope)?;

    let expected = expected_event_names(&cmd);
    if !expected.contains(&envelope.hook_event_name.as_str()) {
        return Err(format!(
            "hook_event_name mismatch: expected one of {:?}, got '{}'",
            expected, envelope.hook_event_name
        ));
    }

    // Use process cwd as the trust boundary for local writes.
    let working_dir =
        std::env::current_dir().map_err(|e| format!("failed to read current directory: {e}"))?;
    let working_dir_str = working_dir.to_string_lossy().to_string();
    let session_store = crate::internal::ai::session::SessionStore::new(&working_dir);

    let mut session = match session_store.load(&envelope.session_id) {
        Ok(s) => s,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let mut recovered = SessionState::new(&working_dir_str);
            recovered.id = envelope.session_id.clone();
            recovered.working_dir = working_dir_str.clone();
            recovered
                .metadata
                .insert("recovered_from_out_of_order".to_string(), json!(true));
            recovered
        }
        Err(err) => return Err(format!("failed to load session: {err}")),
    };
    session.working_dir = working_dir_str.clone();
    if envelope.cwd != working_dir_str {
        session
            .metadata
            .insert("hook_reported_cwd".to_string(), json!(envelope.cwd.clone()));
        session
            .metadata
            .insert("hook_cwd_mismatch".to_string(), json!(true));
    } else {
        session.metadata.remove("hook_cwd_mismatch");
        session.metadata.remove("hook_reported_cwd");
    }

    if dedup_hit(&session, &envelope) {
        if !is_session_end(&cmd) {
            return Ok(());
        }
        // For SessionEnd, only skip when persistence is already confirmed.
        // This allows retried end events to recover after a previous failure.
        if session_persisted(&session) {
            return Ok(());
        }
    }

    apply_event(&mut session, &envelope, &cmd)?;
    append_processed_event_key(&mut session, make_event_key(&envelope));

    if is_session_end(&cmd) {
        match persist_session_history(&session).await {
            Ok(outcome) => {
                session
                    .metadata
                    .insert("persisted".to_string(), json!(true));
                session
                    .metadata
                    .insert("persisted_at".to_string(), json!(Utc::now().to_rfc3339()));
                session
                    .metadata
                    .insert("history_ref".to_string(), json!("libra/intent"));
                session
                    .metadata
                    .insert("object_hash".to_string(), json!(outcome.object_hash));
                session.metadata.insert(
                    "persisted_from_history".to_string(),
                    json!(outcome.already_exists),
                );
                session.metadata.remove("persist_failed");
                session.metadata.remove("cleanup_failed");
                session.metadata.remove("last_error");

                // Delete local session after successful persistence.
                // Keep metadata on cleanup failure so the operator can inspect/retry.
                match session_store.delete(&session.id) {
                    Ok(_) => return Ok(()),
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                    Err(err) => {
                        session
                            .metadata
                            .insert("cleanup_failed".to_string(), json!(true));
                        session
                            .metadata
                            .insert("last_error".to_string(), json!(err.to_string()));
                    }
                }
            }
            Err(err) => {
                session
                    .metadata
                    .insert("persist_failed".to_string(), json!(true));
                session
                    .metadata
                    .insert("last_error".to_string(), json!(err.to_string()));
                session
                    .metadata
                    .insert("persisted".to_string(), json!(false));
            }
        }
    }

    session_store
        .save(&session)
        .map_err(|e| format!("failed to save session: {e}"))?;

    Ok(())
}

fn validate_core_fields(envelope: &HookEnvelope) -> Result<(), String> {
    if envelope.hook_event_name.trim().is_empty() {
        return Err("missing required field: hook_event_name".to_string());
    }
    if envelope.session_id.trim().is_empty() {
        return Err("missing required field: session_id".to_string());
    }
    validate_session_id(&envelope.session_id)?;
    if envelope.cwd.trim().is_empty() {
        return Err("missing required field: cwd".to_string());
    }
    Ok(())
}

fn validate_session_id(session_id: &str) -> Result<(), String> {
    if session_id.len() > 128 {
        return Err("invalid session_id: length exceeds 128 characters".to_string());
    }
    if session_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Ok(());
    }
    Err("invalid session_id: only [A-Za-z0-9._-] are allowed".to_string())
}

fn expected_event_names(cmd: &ClaudeCodeCommand) -> &'static [&'static str] {
    match cmd {
        ClaudeCodeCommand::SessionStart(_) => &["SessionStart"],
        ClaudeCodeCommand::Prompt(_) => &["UserPromptSubmit"],
        ClaudeCodeCommand::ToolUse(_) => &["PostToolUse"],
        ClaudeCodeCommand::Stop(_) => &["Stop", "SessionStop"],
        ClaudeCodeCommand::SessionEnd(_) => &["SessionEnd"],
    }
}

fn is_session_end(cmd: &ClaudeCodeCommand) -> bool {
    matches!(cmd, ClaudeCodeCommand::SessionEnd(_))
}

fn apply_event(
    session: &mut SessionState,
    envelope: &HookEnvelope,
    cmd: &ClaudeCodeCommand,
) -> Result<(), String> {
    session.updated_at = Utc::now();

    if let Some(transcript_path) = &envelope.transcript_path {
        session.metadata.insert(
            "transcript_path".to_string(),
            Value::String(transcript_path.clone()),
        );
    }

    let raw_events = session
        .metadata
        .entry("raw_hook_events".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Value::Array(events) = raw_events {
        // Keep normalized raw fragments for audit/debug and deterministic hashing.
        events.push(json!({
            "hook_event_name": envelope.hook_event_name,
            "payload": normalize_value(Value::Object(envelope.extra.clone())),
        }));
        if events.len() > PROCESSED_EVENT_KEYS_MAX {
            let drop_n = events.len() - PROCESSED_EVENT_KEYS_MAX;
            events.drain(0..drop_n);
        }
    }

    match cmd {
        ClaudeCodeCommand::SessionStart(_) => {
            set_phase(session, ClaudeSessionPhase::Active);
            if let Some(v) = envelope.extra.get("model") {
                session.metadata.insert("model".to_string(), v.clone());
            }
            if let Some(v) = envelope.extra.get("source") {
                session.metadata.insert("source".to_string(), v.clone());
            }
        }
        ClaudeCodeCommand::Prompt(_) => {
            set_phase(session, ClaudeSessionPhase::Active);
            if let Some(prompt) = find_string(envelope, &["prompt", "message", "user_prompt"]) {
                session.add_user_message(&prompt);
            }
        }
        ClaudeCodeCommand::ToolUse(_) => {
            set_phase(session, ClaudeSessionPhase::Active);
            let tools_entry = session
                .metadata
                .entry("tool_events".to_string())
                .or_insert_with(|| Value::Array(Vec::new()));
            if let Value::Array(items) = tools_entry {
                items.push(json!({
                    "tool_name": find_string(envelope, &["tool_name"]).unwrap_or_default(),
                    "input": envelope.extra.get("tool_input").cloned().unwrap_or(Value::Null),
                    "response": envelope.extra.get("tool_response").cloned().unwrap_or(Value::Null),
                    "timestamp": Utc::now().to_rfc3339(),
                }));
                if items.len() > PROCESSED_EVENT_KEYS_MAX {
                    let drop_n = items.len() - PROCESSED_EVENT_KEYS_MAX;
                    items.drain(0..drop_n);
                }
            }
        }
        ClaudeCodeCommand::Stop(_) => {
            set_phase(session, ClaudeSessionPhase::Stopped);
            if let Some(msg) = find_string(
                envelope,
                &["last_assistant_message", "assistant_message", "message"],
            ) {
                session
                    .metadata
                    .insert("last_assistant_message".to_string(), json!(msg.clone()));
                let should_append = session
                    .messages
                    .last()
                    .map(|m| m.role != "assistant" || m.content != msg)
                    .unwrap_or(true);
                if should_append {
                    session.add_assistant_message(&msg);
                }
            }
        }
        ClaudeCodeCommand::SessionEnd(_) => {
            set_phase(session, ClaudeSessionPhase::Ended);
        }
    }

    Ok(())
}

fn set_phase(session: &mut SessionState, phase: ClaudeSessionPhase) {
    let value = match phase {
        ClaudeSessionPhase::Active => "Active",
        ClaudeSessionPhase::Stopped => "Stopped",
        ClaudeSessionPhase::Ended => "Ended",
    };
    session
        .metadata
        .insert("claude_session_phase".to_string(), json!(value));
}

fn find_string(envelope: &HookEnvelope, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(Value::String(v)) = envelope.extra.get(*key) {
            return Some(v.clone());
        }
    }
    None
}

fn make_event_key(envelope: &HookEnvelope) -> String {
    let mut hasher = DefaultHasher::new();
    envelope.hook_event_name.hash(&mut hasher);
    envelope.session_id.hash(&mut hasher);
    // Hash canonicalized payload to keep dedup stable across key order differences.
    let normalized = normalize_value(Value::Object(envelope.extra.clone()));
    if let Ok(canonical_extra) = serde_json::to_string(&normalized) {
        canonical_extra.hash(&mut hasher);
    }

    format!(
        "{}:{}:{:x}",
        envelope.hook_event_name,
        envelope.session_id,
        hasher.finish()
    )
}

fn dedup_hit(session: &SessionState, envelope: &HookEnvelope) -> bool {
    let key = make_event_key(envelope);
    session
        .metadata
        .get(PROCESSED_EVENT_KEYS)
        .and_then(Value::as_array)
        .map(|items| items.iter().any(|v| v.as_str() == Some(key.as_str())))
        .unwrap_or(false)
}

fn append_processed_event_key(session: &mut SessionState, key: String) {
    let entry = session
        .metadata
        .entry(PROCESSED_EVENT_KEYS.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));

    let Value::Array(items) = entry else {
        session.metadata.insert(
            PROCESSED_EVENT_KEYS.to_string(),
            Value::Array(vec![json!(key)]),
        );
        return;
    };

    items.push(Value::String(key));
    if items.len() > PROCESSED_EVENT_KEYS_MAX {
        let drop_n = items.len() - PROCESSED_EVENT_KEYS_MAX;
        items.drain(0..drop_n);
    }
}

fn session_persisted(session: &SessionState) -> bool {
    session
        .metadata
        .get("persisted")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

async fn persist_session_history(session: &SessionState) -> anyhow::Result<PersistOutcome> {
    let storage_path =
        util::try_get_storage_path(Some(std::path::PathBuf::from(&session.working_dir)))?;
    let objects_dir = storage_path.join("objects");
    std::fs::create_dir_all(&objects_dir)?;

    let storage = Arc::new(LocalStorage::new(objects_dir));
    let db_conn = Arc::new(db::get_db_conn_instance().await.clone());
    let history_manager = HistoryManager::new(storage, storage_path.clone(), db_conn);
    // Idempotency fast path: skip append when object already exists.
    if let Some(existing) = history_manager
        .get_object_hash(CLAUDE_SESSION_TYPE, &session.id)
        .await?
    {
        return Ok(PersistOutcome {
            object_hash: existing.to_string(),
            already_exists: true,
        });
    }

    let payload = json!({
        "schema": "libra.claude_session.v1",
        "session": session,
        "raw_hook_events": session.metadata.get("raw_hook_events").cloned().unwrap_or(Value::Array(vec![])),
        "ingest_meta": {
            "source": "claude_code_hook",
            "ingested_at": Utc::now().to_rfc3339(),
        }
    });

    // Canonical JSON keeps blob content deterministic for the same semantic payload.
    let blob_data = to_canonical_json_bytes(&payload)?;
    let blob_hash = write_git_object(&storage_path, "blob", &blob_data)?;
    history_manager
        .append(CLAUDE_SESSION_TYPE, &session.id, blob_hash)
        .await?;

    Ok(PersistOutcome {
        object_hash: blob_hash.to_string(),
        already_exists: false,
    })
}

fn to_canonical_json_bytes(value: &Value) -> Result<Vec<u8>, serde_json::Error> {
    let normalized = normalize_value(value.clone());
    serde_json::to_vec(&normalized)
}

fn normalize_value(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted: BTreeMap<String, Value> = map
                .into_iter()
                .map(|(k, v)| (k, normalize_value(v)))
                .collect();
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(normalize_value).collect()),
        scalar => scalar,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_key_is_stable_for_same_payload() {
        let env = HookEnvelope {
            hook_event_name: "UserPromptSubmit".to_string(),
            session_id: "s1".to_string(),
            cwd: "/tmp".to_string(),
            transcript_path: None,
            extra: {
                let mut m = Map::new();
                m.insert("prompt".to_string(), Value::String("hello".to_string()));
                m
            },
        };

        let k1 = make_event_key(&env);
        let k2 = make_event_key(&env);
        assert_eq!(k1, k2);
    }

    #[test]
    fn processed_event_keys_capped() {
        let mut s = SessionState::new("/tmp");
        for i in 0..(PROCESSED_EVENT_KEYS_MAX + 50) {
            append_processed_event_key(&mut s, format!("k{i}"));
        }

        let len = s
            .metadata
            .get(PROCESSED_EVENT_KEYS)
            .and_then(Value::as_array)
            .map(std::vec::Vec::len)
            .unwrap_or(0);
        assert_eq!(len, PROCESSED_EVENT_KEYS_MAX);
    }

    #[test]
    fn validate_core_fields_rejects_missing() {
        let env = HookEnvelope {
            hook_event_name: "".to_string(),
            session_id: "a".to_string(),
            cwd: "/tmp".to_string(),
            transcript_path: None,
            extra: Map::new(),
        };
        assert!(validate_core_fields(&env).is_err());
    }

    #[test]
    fn validate_core_fields_rejects_invalid_session_id() {
        let env = HookEnvelope {
            hook_event_name: "SessionStart".to_string(),
            session_id: "../unsafe".to_string(),
            cwd: "/tmp".to_string(),
            transcript_path: None,
            extra: Map::new(),
        };
        assert!(validate_core_fields(&env).is_err());
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

        let canonical = serde_json::to_string(&normalize_value(value)).unwrap();
        assert_eq!(canonical, r#"{"a":{"k1":1,"k2":2},"z":1}"#);
    }
}
