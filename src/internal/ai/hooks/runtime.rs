//! Shared runtime for provider lifecycle hook ingestion.

use std::{io::Read, path::Path, sync::Arc};

use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use git_internal::hash::{HashKind, set_hash_kind};
use serde_json::{Value, json};

use super::{
    lifecycle::{
        LifecycleEvent, LifecycleEventKind, SessionHookEnvelope, append_raw_hook_event,
        apply_lifecycle_event, make_dedup_key, normalize_json_value,
        validate_session_hook_envelope,
    },
    provider::HookProvider,
};
use crate::{
    internal::{
        ai::{
            history::{AI_REF, HistoryManager},
            session::SessionState,
        },
        db,
    },
    utils::{object::write_git_object, storage::local::LocalStorage, util},
};

const PROCESSED_EVENT_KEYS: &str = "processed_event_keys";
const NORMALIZED_EVENTS_KEY: &str = "normalized_events";
const PROVIDER_METADATA_KEY: &str = "provider";
const PROVIDER_SESSION_ID_METADATA_KEY: &str = "provider_session_id";
const SESSION_PHASE_METADATA_KEY: &str = "session_phase";
const SESSION_ID_DELIMITER: &str = "__";

const MAX_STDIN_BYTES: usize = 1_048_576;
const MAX_PROCESSED_EVENT_KEYS: usize = 200;
const MAX_NORMALIZED_EVENTS: usize = 400;
const MAX_RAW_HOOK_EVENTS: usize = 200;
const MAX_TOOL_EVENTS: usize = 200;
const MAX_TRANSCRIPT_PATH_BYTES: usize = 4096;

pub const AI_SESSION_TYPE: &str = "ai_session";
pub const AI_SESSION_SCHEMA: &str = "libra.ai_session.v2";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionPhase {
    Active,
    Stopped,
    Ended,
}

#[derive(Debug)]
struct PersistOutcome {
    object_hash: String,
    already_exists: bool,
}

pub fn build_ai_session_id(provider: &str, provider_session_id: &str) -> String {
    format!("{provider}{SESSION_ID_DELIMITER}{provider_session_id}")
}

fn redact_session_id(session_id: &str) -> String {
    let mut chars = session_id.chars();
    let prefix: String = chars.by_ref().take(8).collect();
    if chars.next().is_some() {
        format!("{prefix}***")
    } else {
        "***".to_string()
    }
}

pub async fn process_hook_event_from_stdin(
    command: super::provider::ProviderHookCommand,
    expected_kind: LifecycleEventKind,
    provider: &dyn HookProvider,
) -> Result<()> {
    let mut stdin_bytes = Vec::new();
    std::io::stdin()
        .take((MAX_STDIN_BYTES + 1) as u64)
        .read_to_end(&mut stdin_bytes)
        .context("failed to read stdin")?;
    if stdin_bytes.len() > MAX_STDIN_BYTES {
        bail!("hook input exceeds {MAX_STDIN_BYTES} bytes");
    }
    let stdin = String::from_utf8(stdin_bytes).context("hook input is not valid UTF-8")?;

    if stdin.trim().is_empty() {
        bail!("hook input is empty");
    }

    let envelope: SessionHookEnvelope =
        serde_json::from_str(&stdin).map_err(|err| anyhow!("invalid hook JSON payload: {err}"))?;
    validate_session_hook_envelope(&envelope, MAX_TRANSCRIPT_PATH_BYTES)?;

    let event = provider.parse_hook_event(&envelope.hook_event_name, &envelope)?;
    if event.kind != expected_kind {
        bail!(
            "hook event kind mismatch: expected '{}', got '{}' from hook_event_name '{}'",
            expected_kind,
            event.kind,
            envelope.hook_event_name
        );
    }

    let process_cwd = std::env::current_dir().context("failed to read current directory")?;
    let storage_path = util::try_get_storage_path(Some(process_cwd.clone()))
        .context("failed to resolve libra storage path from current directory")?;
    set_hash_kind_from_repo()
        .await
        .context("failed to configure hash kind from repo config")?;

    let process_cwd_str = process_cwd.to_string_lossy().to_string();
    let session_store =
        crate::internal::ai::session::SessionStore::from_storage_path(&storage_path);

    let ai_session_id = build_ai_session_id(provider.provider_name(), &envelope.session_id);
    let recovered_from_out_of_order = event.kind != LifecycleEventKind::SessionStart;
    let _session_lock = session_store
        .lock_session(&ai_session_id)
        .with_context(|| {
            format!(
                "failed to acquire session lock for '{}'",
                redact_session_id(&ai_session_id)
            )
        })?;

    let mut session = match session_store.load(&ai_session_id) {
        Ok(session) => session,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let mut recovered = SessionState::new(&process_cwd_str);
            recovered.id = ai_session_id.clone();
            recovered.working_dir = process_cwd_str.clone();
            if recovered_from_out_of_order {
                recovered
                    .metadata
                    .insert("recovered_from_out_of_order".to_string(), json!(true));
            }
            recovered
        }
        Err(err) if err.kind() == std::io::ErrorKind::InvalidData => {
            let archived_path = match session_store.archive_corrupt_session(&ai_session_id) {
                Ok(path) => path,
                Err(archive_err) => {
                    eprintln!(
                        "warning: failed to archive malformed session '{}': {}",
                        redact_session_id(&ai_session_id),
                        archive_err
                    );
                    None
                }
            };
            eprintln!(
                "warning: malformed session cache detected for '{}', recovering with a new in-memory session",
                redact_session_id(&ai_session_id)
            );

            let mut recovered = SessionState::new(&process_cwd_str);
            recovered.id = ai_session_id.clone();
            recovered.working_dir = process_cwd_str.clone();
            recovered
                .metadata
                .insert("recovered_from_corrupt_session".to_string(), json!(true));
            recovered
                .metadata
                .insert("recovery_error".to_string(), json!(err.to_string()));
            if let Some(path) = archived_path {
                recovered.metadata.insert(
                    "corrupt_session_backup".to_string(),
                    json!(path.to_string_lossy().to_string()),
                );
            }
            recovered
        }
        Err(err) => return Err(anyhow!("failed to load session: {err}")),
    };

    session.id = ai_session_id;
    session.working_dir = process_cwd_str.clone();
    session.metadata.insert(
        PROVIDER_METADATA_KEY.to_string(),
        json!(provider.provider_name().to_string()),
    );
    session.metadata.insert(
        PROVIDER_SESSION_ID_METADATA_KEY.to_string(),
        json!(envelope.session_id.clone()),
    );

    if envelope.cwd != process_cwd_str {
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

    let dedup_key = make_dedup_key(
        provider.dedup_identity_keys(),
        provider.lifecycle_fallback_events(),
        &envelope,
    );
    if dedup_hit(&session, dedup_key.as_deref()) {
        if event.kind != LifecycleEventKind::SessionEnd {
            return Ok(());
        }
        if session_persisted(&session) {
            return Ok(());
        }
    }

    apply_hook_event(&mut session, &envelope, &event, provider.provider_name());
    provider
        .post_process_event(command, &storage_path, &mut session, &envelope, &event)
        .context("provider hook post-processing failed")?;
    if let Some(event_key) = dedup_key {
        append_processed_event_key(&mut session, event_key);
    }

    if event.kind == LifecycleEventKind::SessionEnd {
        match persist_session_history(&storage_path, &session, provider).await {
            Ok(outcome) => {
                session
                    .metadata
                    .insert("persisted".to_string(), json!(true));
                session
                    .metadata
                    .insert("persisted_at".to_string(), json!(Utc::now().to_rfc3339()));
                session
                    .metadata
                    .insert("history_ref".to_string(), json!(AI_REF));
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
                eprintln!("warning: failed to persist session history: {err}");
                session_store.save(&session).map_err(|save_err| {
                    anyhow!("failed to save session after persistence failure: {save_err}")
                })?;
                return Err(err.context("session history persistence failed"));
            }
        }
    }

    session_store
        .save(&session)
        .map_err(|err| anyhow!("failed to save session: {err}"))?;
    Ok(())
}

async fn set_hash_kind_from_repo() -> Result<()> {
    let object_format = crate::internal::config::Config::get("core", None, "objectformat")
        .await
        .unwrap_or_else(|| "sha1".to_string());

    let hash_kind = match object_format.as_str() {
        "sha1" => HashKind::Sha1,
        "sha256" => HashKind::Sha256,
        _ => bail!("unsupported object format: '{object_format}'"),
    };
    set_hash_kind(hash_kind);
    Ok(())
}

fn apply_hook_event(
    session: &mut SessionState,
    envelope: &SessionHookEnvelope,
    event: &LifecycleEvent,
    provider_name: &str,
) {
    session.updated_at = Utc::now();

    if let Some(session_ref) = &event.session_ref {
        session.metadata.insert(
            "transcript_path".to_string(),
            Value::String(session_ref.clone()),
        );
    }

    append_raw_hook_event(session, envelope, MAX_RAW_HOOK_EVENTS);
    apply_lifecycle_event(session, event, MAX_TOOL_EVENTS);
    transition_phase(session, event.kind);
    append_normalized_event(session, event, provider_name);
}

fn transition_phase(session: &mut SessionState, event_kind: LifecycleEventKind) {
    let current_phase = session
        .metadata
        .get(SESSION_PHASE_METADATA_KEY)
        .and_then(Value::as_str)
        .and_then(|phase| match phase {
            "active" => Some(SessionPhase::Active),
            "stopped" => Some(SessionPhase::Stopped),
            "ended" => Some(SessionPhase::Ended),
            _ => None,
        });

    let next_phase = match event_kind {
        LifecycleEventKind::SessionEnd => SessionPhase::Ended,
        LifecycleEventKind::TurnEnd => SessionPhase::Stopped,
        LifecycleEventKind::SessionStart
        | LifecycleEventKind::TurnStart
        | LifecycleEventKind::ToolUse
        | LifecycleEventKind::Compaction => SessionPhase::Active,
        LifecycleEventKind::ModelUpdate => current_phase.unwrap_or(SessionPhase::Active),
    };

    session.metadata.insert(
        SESSION_PHASE_METADATA_KEY.to_string(),
        json!(next_phase.as_str()),
    );
}

fn append_normalized_event(
    session: &mut SessionState,
    event: &LifecycleEvent,
    provider_name: &str,
) {
    let entry = session
        .metadata
        .entry(NORMALIZED_EVENTS_KEY.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));

    let normalized = json!({
        "provider": provider_name,
        "kind": event.kind.to_string(),
        "timestamp": event.timestamp.to_rfc3339(),
        "prompt": event.prompt,
        "tool_name": event.tool_name,
        "assistant_message": event.assistant_message,
        "has_model": event.model.is_some(),
        "has_tool_input": event.tool_input.is_some(),
        "has_tool_response": event.tool_response.is_some(),
    });

    let Value::Array(items) = entry else {
        session.metadata.insert(
            NORMALIZED_EVENTS_KEY.to_string(),
            Value::Array(vec![normalized]),
        );
        return;
    };

    items.push(normalized);
    if items.len() > MAX_NORMALIZED_EVENTS {
        let drop_n = items.len() - MAX_NORMALIZED_EVENTS;
        items.drain(0..drop_n);
    }
}

fn dedup_hit(session: &SessionState, key: Option<&str>) -> bool {
    let Some(key) = key else {
        return false;
    };
    session
        .metadata
        .get(PROCESSED_EVENT_KEYS)
        .and_then(Value::as_array)
        .map(|items| items.iter().any(|value| value.as_str() == Some(key)))
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
    if items.len() > MAX_PROCESSED_EVENT_KEYS {
        let drop_n = items.len() - MAX_PROCESSED_EVENT_KEYS;
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

async fn persist_session_history(
    storage_path: &Path,
    session: &SessionState,
    provider: &dyn HookProvider,
) -> Result<PersistOutcome> {
    let objects_dir = storage_path.join("objects");
    std::fs::create_dir_all(&objects_dir)?;

    let storage = Arc::new(LocalStorage::new(objects_dir));
    let db_conn = Arc::new(db::get_db_conn_instance().await.clone());
    let history_manager = HistoryManager::new(storage, storage_path.to_path_buf(), db_conn);

    if let Some(existing) = history_manager
        .get_object_hash(AI_SESSION_TYPE, &session.id)
        .await?
    {
        return Ok(PersistOutcome {
            object_hash: existing.to_string(),
            already_exists: true,
        });
    }

    let payload = build_ai_session_payload(session, provider);
    let blob_data = serde_json::to_vec(&normalize_json_value(payload))
        .context("failed to serialize ai_session payload")?;
    let blob_hash = write_git_object(storage_path, "blob", &blob_data)?;
    history_manager
        .append(AI_SESSION_TYPE, &session.id, blob_hash)
        .await?;

    Ok(PersistOutcome {
        object_hash: blob_hash.to_string(),
        already_exists: false,
    })
}

fn build_ai_session_payload(session: &SessionState, provider: &dyn HookProvider) -> Value {
    let events = session
        .metadata
        .get(NORMALIZED_EVENTS_KEY)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let raw_events = session
        .metadata
        .get("raw_hook_events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let phase = session
        .metadata
        .get(SESSION_PHASE_METADATA_KEY)
        .and_then(Value::as_str)
        .unwrap_or("active");
    let provider_session_id = session
        .metadata
        .get(PROVIDER_SESSION_ID_METADATA_KEY)
        .and_then(Value::as_str)
        .unwrap_or(&session.id);
    let transcript_path = session
        .metadata
        .get("transcript_path")
        .and_then(Value::as_str);
    let last_assistant_message = session
        .metadata
        .get("last_assistant_message")
        .and_then(Value::as_str);

    json!({
        "schema": AI_SESSION_SCHEMA,
        "object_type": AI_SESSION_TYPE,
        "provider": provider.provider_name(),
        "ai_session_id": session.id,
        "provider_session_id": provider_session_id,
        "state_machine": {
            "phase": phase,
            "status": phase_status_label(phase),
            "event_count": events.len(),
            "tool_use_count": count_events(&events, "tool_use"),
            "compaction_count": count_events(&events, "compaction"),
            "started_at": first_event_timestamp(&events, "session_start"),
            "ended_at": first_event_timestamp(&events, "session_end"),
            "updated_at": session.updated_at.to_rfc3339(),
        },
        "summary": {
            "message_count": session.messages.len(),
            "user_message_count": session.messages.iter().filter(|message| message.role == "user").count(),
            "assistant_message_count": session.messages.iter().filter(|message| message.role == "assistant").count(),
            "last_assistant_message": last_assistant_message,
        },
        "transcript": {
            "path": transcript_path,
            "raw_event_count": raw_events.len(),
        },
        "events": events,
        "raw_hook_events": raw_events,
        "session": session,
        "ingest_meta": {
            "source": provider.source_name(),
            "provider": provider.provider_name(),
            "history_ref": AI_REF,
            "ingested_at": Utc::now().to_rfc3339(),
        }
    })
}

fn phase_status_label(phase: &str) -> &'static str {
    match phase {
        "active" => "running",
        "stopped" => "idle",
        "ended" => "ended",
        _ => "running",
    }
}

fn count_events(events: &[Value], kind: &str) -> usize {
    events
        .iter()
        .filter(|value| value.get("kind").and_then(Value::as_str) == Some(kind))
        .count()
}

fn first_event_timestamp(events: &[Value], kind: &str) -> Option<String> {
    events
        .iter()
        .find(|value| value.get("kind").and_then(Value::as_str) == Some(kind))
        .and_then(|value| value.get("timestamp"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

impl SessionPhase {
    fn as_str(self) -> &'static str {
        match self {
            SessionPhase::Active => "active",
            SessionPhase::Stopped => "stopped",
            SessionPhase::Ended => "ended",
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Map;

    use super::*;
    use crate::internal::ai::hooks::providers::{claude_provider, gemini_provider};

    #[test]
    fn processed_event_keys_capped() {
        let mut session = SessionState::new("/tmp");
        for index in 0..(MAX_PROCESSED_EVENT_KEYS + 50) {
            append_processed_event_key(&mut session, format!("k{index}"));
        }

        let len = session
            .metadata
            .get(PROCESSED_EVENT_KEYS)
            .and_then(Value::as_array)
            .map(std::vec::Vec::len)
            .unwrap_or(0);
        assert_eq!(len, MAX_PROCESSED_EVENT_KEYS);
    }

    #[test]
    fn unified_phase_metadata_key_is_used() {
        let envelope = SessionHookEnvelope {
            hook_event_name: "SessionStart".to_string(),
            session_id: "s1".to_string(),
            cwd: "/tmp".to_string(),
            transcript_path: None,
            extra: Map::new(),
        };
        let event = gemini_provider()
            .parse_hook_event("SessionStart", &envelope)
            .expect("parse should succeed");
        let mut session = SessionState::new("/tmp");

        apply_hook_event(&mut session, &envelope, &event, "gemini");

        assert_eq!(
            session.metadata.get(SESSION_PHASE_METADATA_KEY),
            Some(&json!("active"))
        );
    }

    #[test]
    fn dedup_keys_remain_stable_across_providers() {
        let envelope = SessionHookEnvelope {
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

        let claude_key = make_dedup_key(
            claude_provider().dedup_identity_keys(),
            claude_provider().lifecycle_fallback_events(),
            &envelope,
        );
        let gemini_key = make_dedup_key(
            gemini_provider().dedup_identity_keys(),
            gemini_provider().lifecycle_fallback_events(),
            &envelope,
        );
        assert_eq!(claude_key, gemini_key);
    }

    #[test]
    fn session_id_is_namespaced_by_provider() {
        assert_eq!(
            build_ai_session_id("gemini", "session-123"),
            "gemini__session-123"
        );
        assert_eq!(
            build_ai_session_id("claude", "session-123"),
            "claude__session-123"
        );
    }

    #[test]
    fn session_id_redaction_masks_suffix() {
        assert_eq!(redact_session_id("gemini__session-123"), "gemini__***");
        assert_eq!(redact_session_id("short"), "***");
    }

    #[test]
    fn v2_payload_contains_state_machine_and_summary() {
        let mut session = SessionState::new("/tmp/repo");
        session.id = "gemini__s-1".to_string();
        session.metadata.insert(
            PROVIDER_SESSION_ID_METADATA_KEY.to_string(),
            json!("s-1".to_string()),
        );
        session
            .metadata
            .insert(SESSION_PHASE_METADATA_KEY.to_string(), json!("ended"));
        session.metadata.insert(
            NORMALIZED_EVENTS_KEY.to_string(),
            json!([
                {"kind":"session_start","timestamp":"2026-01-01T00:00:00Z"},
                {"kind":"turn_start","timestamp":"2026-01-01T00:00:01Z"},
                {"kind":"tool_use","timestamp":"2026-01-01T00:00:02Z"},
                {"kind":"session_end","timestamp":"2026-01-01T00:00:03Z"}
            ]),
        );
        session
            .metadata
            .insert("transcript_path".to_string(), json!("/tmp/t.jsonl"));
        session
            .metadata
            .insert("last_assistant_message".to_string(), json!("done"));
        session.add_user_message("hello");
        session.add_assistant_message("done");

        let payload = build_ai_session_payload(&session, gemini_provider());

        assert_eq!(payload["schema"], json!(AI_SESSION_SCHEMA));
        assert_eq!(payload["provider"], json!("gemini"));
        assert_eq!(payload["object_type"], json!(AI_SESSION_TYPE));
        assert_eq!(payload["state_machine"]["phase"], json!("ended"));
        assert_eq!(payload["state_machine"]["tool_use_count"], json!(1));
        assert_eq!(payload["summary"]["message_count"], json!(2));
        assert_eq!(payload["summary"]["user_message_count"], json!(1));
        assert_eq!(payload["transcript"]["path"], json!("/tmp/t.jsonl"));
    }
}
