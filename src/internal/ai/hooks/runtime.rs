//! Shared runtime for provider lifecycle hook ingestion.
//!
//! When an external provider invokes `libra hooks <command>`, control lands in
//! [`process_hook_event_from_stdin`]. This function:
//! 1. Reads, size-bounds, and JSON-parses the stdin envelope.
//! 2. Validates it against the canonical schema.
//! 3. Asks the provider adapter to lower it into a [`LifecycleEvent`].
//! 4. Loads (or recovers) the persistent [`SessionState`], deduplicates the event,
//!    applies it, and on `SessionEnd` writes a content-addressed `ai_session` blob
//!    plus a history reference so other tools can read the session later.
//!
//! All bounded constants below (`MAX_*`) protect the runtime from runaway providers
//! that emit pathologically large or repetitive payloads.

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
            session::{SessionState, SessionStore},
        },
        config::ConfigKv,
        db,
    },
    utils::{client_storage::ClientStorage, error::emit_warning, object::write_git_object, util},
};

// Metadata keys persisted on `SessionState`. Centralised here so that ingestion,
// projection, and tests all see the same names.
const PROCESSED_EVENT_KEYS: &str = "processed_event_keys";
const NORMALIZED_EVENTS_KEY: &str = "normalized_events";
const PROVIDER_METADATA_KEY: &str = "provider";
const PROVIDER_SESSION_ID_METADATA_KEY: &str = "provider_session_id";
const SESSION_PHASE_METADATA_KEY: &str = "session_phase";
/// Separator inserted between provider name and the provider's native session ID
/// when forming Libra's namespaced AI session ID.
const SESSION_ID_DELIMITER: &str = "__";

// Resource bounds. The values are deliberately small enough to stay in memory for
// the longest plausible session while large enough to capture the events the agent
// actually needs for projection.
const MAX_STDIN_BYTES: usize = 1_048_576;
const MAX_PROCESSED_EVENT_KEYS: usize = 200;
const MAX_NORMALIZED_EVENTS: usize = 400;
const MAX_RAW_HOOK_EVENTS: usize = 200;
const MAX_TOOL_EVENTS: usize = 200;
const MAX_TRANSCRIPT_PATH_BYTES: usize = 4096;

/// Object type tag stamped on persisted AI session blobs.
pub const AI_SESSION_TYPE: &str = "ai_session";
/// Schema version. Bump when the persisted shape changes incompatibly.
pub const AI_SESSION_SCHEMA: &str = "libra.ai_session.v2";

/// Coarse session lifecycle phase recorded as `session_phase` metadata.
///
/// Distinct from [`LifecycleEventKind`] — the latter is per-event, the former is
/// aggregated state suitable for UIs (a single status badge per session).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionPhase {
    Active,
    Stopped,
    Ended,
}

/// Outcome of attempting to persist a session at SessionEnd.
///
/// Carries the resulting blob's object hash so callers can advertise it on the
/// session's metadata, and `already_exists` to distinguish a fresh write from a
/// retry that reused a previous blob (idempotent SessionEnd handling).
#[derive(Debug)]
struct PersistOutcome {
    object_hash: String,
    already_exists: bool,
}

/// Combine a provider name with the provider's native session ID into Libra's
/// canonical ID.
///
/// Functional scope: the resulting string is used as a directory name and as a
/// metadata key, so it must round-trip without escaping. Both inputs are assumed
/// to come from validated envelopes (see [`validate_session_hook_envelope`]).
pub fn build_ai_session_id(provider: &str, provider_session_id: &str) -> String {
    format!("{provider}{SESSION_ID_DELIMITER}{provider_session_id}")
}

/// Strip session IDs down to a non-secret prefix for log output.
///
/// Functional scope: keeps the first eight characters and replaces the rest with
/// `***`. For very short IDs the entire value is masked. Used in `tracing` and
/// `eprintln!` calls to avoid leaking provider session identifiers into logs.
fn redact_session_id(session_id: &str) -> String {
    let mut chars = session_id.chars();
    let prefix: String = chars.by_ref().take(8).collect();
    if chars.next().is_some() {
        format!("{prefix}***")
    } else {
        "***".to_string()
    }
}

/// Top-level entry for `libra hooks <command>`.
///
/// Functional scope:
/// - Reads up to `MAX_STDIN_BYTES + 1` bytes from stdin and rejects oversize
///   payloads early.
/// - Parses the canonical [`SessionHookEnvelope`] and validates it.
/// - Asks `provider` to lower the envelope into a [`LifecycleEvent`] and confirms
///   the result matches the expected `expected_kind`.
/// - Loads the persistent session (creating a fresh one if missing, recovering
///   from corruption by archiving the bad cache file and starting clean).
/// - Updates session metadata, applies the lifecycle event, records dedup keys,
///   and on `SessionEnd` writes the final blob to the AI history ref.
///
/// Boundary conditions:
/// - Out-of-order delivery (e.g. the very first observed event is `ToolUse`)
///   creates a synthetic session marked with `recovered_from_out_of_order`.
/// - Corrupt session caches are archived for forensic inspection rather than
///   discarded silently — operators can still retrieve the original bytes from
///   the path in `corrupt_session_backup`.
/// - Errors during final persistence are surfaced; the partially-mutated session
///   is still saved so retries can converge.
///
/// See: `tests::v2_payload_contains_state_machine_and_summary`,
/// `tests::dedup_keys_remain_stable_across_providers`.
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
    let session_store = SessionStore::from_storage_path(&storage_path);

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
                emit_warning(format!("failed to persist session history: {err}"));
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

/// Load `core.objectformat` from the local repository and pin the global hash kind.
///
/// Mirrors `cli::set_local_hash_kind_for_storage` but reads via the already-open
/// connection that the hook runtime obtains. Defaults to `sha1` for repositories
/// initialised before SHA-256 support landed.
async fn set_hash_kind_from_repo() -> Result<()> {
    let object_format = ConfigKv::get("core.objectformat")
        .await
        .ok()
        .flatten()
        .map(|e| e.value)
        .unwrap_or_else(|| "sha1".to_string());

    let hash_kind = match object_format.as_str() {
        "sha1" => HashKind::Sha1,
        "sha256" => HashKind::Sha256,
        _ => bail!("unsupported object format: '{object_format}'"),
    };
    set_hash_kind(hash_kind);
    Ok(())
}

/// Apply the canonical event together with bookkeeping into `session`.
///
/// Functional scope: bumps `updated_at`, records the transcript path if any,
/// appends the raw envelope to the audit ring, applies the lifecycle delta, and
/// transitions the coarse phase. Finally appends a normalized projection-friendly
/// fragment to `normalized_events` so downstream consumers don't re-parse the raw
/// envelope.
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

/// Compute the new [`SessionPhase`] given the previous phase and the incoming
/// event kind, then record it back on the session.
///
/// Functional scope: `SessionEnd` always wins, transitioning to `Ended`; any
/// activity event resets to `Active`; `TurnEnd` parks at `Stopped`; `ModelUpdate`
/// is a no-op preserving the current phase. This produces a small, deterministic
/// state machine usable as a UI badge.
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
        | LifecycleEventKind::Compaction
        | LifecycleEventKind::CompactionCompleted
        | LifecycleEventKind::PermissionRequest
        | LifecycleEventKind::SourceEnabled
        | LifecycleEventKind::SourceDisabled => SessionPhase::Active,
        LifecycleEventKind::ModelUpdate => current_phase.unwrap_or(SessionPhase::Active),
    };

    session.metadata.insert(
        SESSION_PHASE_METADATA_KEY.to_string(),
        json!(next_phase.as_str()),
    );
}

/// Append a small projection-friendly summary of the event.
///
/// Functional scope: includes the kind, timestamp, prompt, tool name, assistant
/// message, and a few `has_*` flags so projections can render activity feeds
/// without paying the cost of streaming every raw envelope.
///
/// Boundary conditions: capped at `MAX_NORMALIZED_EVENTS`; oldest entries are
/// dropped first.
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

/// Return true when `key` is already in the processed-keys ring.
///
/// Boundary conditions: a `None` key always returns false because callers asked
/// for "no dedup".
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

/// Push `key` onto the processed-keys ring, evicting old entries past
/// `MAX_PROCESSED_EVENT_KEYS`. The same defensive overwrite pattern as
/// [`append_normalized_event`] applies when the slot is the wrong shape.
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

/// Whether the session has already been written to the AI history ref.
///
/// Used together with `dedup_hit` so a duplicate `SessionEnd` doesn't repeat the
/// blob write but still updates metadata fields that may have changed.
fn session_persisted(session: &SessionState) -> bool {
    session
        .metadata
        .get("persisted")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Materialise the final session as a Git blob and append it to the AI history.
///
/// Functional scope:
/// - If a blob already exists for this session ID under [`AI_SESSION_TYPE`], reuse
///   its hash without writing a new one (idempotent).
/// - Otherwise serialise [`build_ai_session_payload`], write a Git blob, and
///   append a `(type, id, hash)` triple to the AI history ref.
///
/// Boundary conditions: any I/O error short-circuits with context; the caller
/// catches and surfaces it via session metadata so the user sees an actionable
/// message and can retry.
async fn persist_session_history(
    storage_path: &Path,
    session: &SessionState,
    provider: &dyn HookProvider,
) -> Result<PersistOutcome> {
    let objects_dir = storage_path.join("objects");
    std::fs::create_dir_all(&objects_dir)?;

    let storage = Arc::new(ClientStorage::init(objects_dir));
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

/// Construct the canonical JSON payload persisted as an `ai_session` blob.
///
/// Functional scope: bundles a state-machine summary, a message-count summary, the
/// transcript pointer, the projected event stream, the raw event ring, and the
/// in-memory session itself. The whole document is keyed by the
/// [`AI_SESSION_SCHEMA`] string so future schema migrations can detect old blobs.
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

/// Translate a phase string into a UI-friendly status label.
///
/// Boundary conditions: an unknown phase falls back to `"running"` so a
/// schema-drift session never produces an empty status.
fn phase_status_label(phase: &str) -> &'static str {
    match phase {
        "active" => "running",
        "stopped" => "idle",
        "ended" => "ended",
        _ => "running",
    }
}

/// Count normalized events with the given `kind`. Used to populate per-session
/// summary counters (tool uses, compactions, etc.).
fn count_events(events: &[Value], kind: &str) -> usize {
    events
        .iter()
        .filter(|value| value.get("kind").and_then(Value::as_str) == Some(kind))
        .count()
}

/// Return the timestamp of the first matching event, or `None` if no event has the
/// requested kind. Used to populate `started_at`/`ended_at` on the persisted
/// state-machine summary.
fn first_event_timestamp(events: &[Value], kind: &str) -> Option<String> {
    events
        .iter()
        .find(|value| value.get("kind").and_then(Value::as_str) == Some(kind))
        .and_then(|value| value.get("timestamp"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

impl SessionPhase {
    /// Stable string form persisted in `session_phase` metadata.
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

    // Scenario: pushing many keys past the cap evicts the oldest, never exceeding
    // `MAX_PROCESSED_EVENT_KEYS`.
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

    // Scenario: a SessionStart event sets the session phase to "active".
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

    // Scenario: the same envelope yields identical dedup keys regardless of
    // which provider's identity-key list is supplied, because both lists pull
    // from `CANONICAL_DEDUP_IDENTITY_KEYS`.
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

    // Scenario: identical native session IDs from different providers do not
    // collide because the namespacing prefix differs.
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

    // Scenario: long IDs keep their first eight characters; short IDs are fully
    // masked.
    #[test]
    fn session_id_redaction_masks_suffix() {
        assert_eq!(redact_session_id("gemini__session-123"), "gemini__***");
        assert_eq!(redact_session_id("short"), "***");
    }

    // Scenario: a synthetic ended session includes the schema id, state machine
    // counters, message-count summary, and transcript path in the payload.
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
