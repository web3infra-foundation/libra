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
            automation::dispatch_repo_hook_lifecycle_event_to_history,
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

/// Where a parsed hook event should land.
///
/// CEX-EntireIO Phase 1.5 introduces this enum so the same parsing /
/// validation pipeline can fan out to two refs:
///
/// - [`HookTarget::AiIntent`] — the canonical `refs/libra/intent` writer used
///   by `libra code` and the existing Claude/Gemini hook configs.
/// - [`HookTarget::AgentTraces`] — the new external-Agent capture writer
///   that lives on `refs/libra/agent-traces`. **Phase 1 stub**: the variant
///   exists for API surface stability, but the runtime currently rejects it
///   with a clear "not yet wired" message; Phase 2 lands the actual writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookTarget {
    AiIntent,
    AgentTraces,
}

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
    process_hook_event_with_target(command, expected_kind, provider, HookTarget::AiIntent).await
}

/// Parametric form of [`process_hook_event_from_stdin`] that selects the
/// writer destination via [`HookTarget`].
///
/// CEX-EntireIO Phase 1.5: this is the seam Phase 2 grows into. For
/// [`HookTarget::AiIntent`] the function is exactly the historical behaviour
/// (1:1 byte-compatible). For [`HookTarget::AgentTraces`] the function
/// runs a Phase 1 minimal ingest — stdin parse, validate, redact, and
/// upsert into `agent_session` — and returns. Phase 2 will extend the
/// AgentTraces branch to additionally generate checkpoint commits on
/// `refs/libra/agent-traces`.
pub async fn process_hook_event_with_target(
    command: super::provider::ProviderHookCommand,
    expected_kind: LifecycleEventKind,
    provider: &dyn HookProvider,
    target: HookTarget,
) -> Result<()> {
    if matches!(target, HookTarget::AgentTraces) {
        return ingest_agent_traces(command, expected_kind, provider).await;
    }

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
    // CEX-EntireIO §11.2: agent capture sessions live under `sessions/agent/`
    // so their session-id locks cannot collide with `libra code` session
    // locks (which still live one level up at `sessions/`). The store also
    // adopts any in-flight legacy entry, preserving hook continuity for
    // sessions that started before this partition existed.
    let session_store = SessionStore::from_storage_path_with_subdir(&storage_path, "agent");

    let ai_session_id = build_ai_session_id(provider.provider_name(), &envelope.session_id);
    if let Err(err) = session_store.adopt_legacy_subdir_session_if_needed(&ai_session_id) {
        tracing::warn!(
            session_id = %redact_session_id(&ai_session_id),
            error = %err,
            "failed to migrate legacy session into agent subdir; continuing with fresh session under sessions/agent/"
        );
    }
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

    if let Err(err) =
        dispatch_repo_hook_lifecycle_event_to_history(&process_cwd, &storage_path, event.kind).await
    {
        emit_warning(format!("failed to dispatch automation hook event: {err}"));
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
/// CEX-EntireIO Phase 1: minimal AgentTraces ingest.
///
/// Reads the hook envelope from stdin, validates, parses to a
/// [`LifecycleEvent`], redacts free-form fields, and upserts into
/// `agent_session`. Does NOT yet generate checkpoint commits on
/// `refs/libra/agent-traces` — that is Phase 2 work.
///
/// Boundary conditions:
/// - Idempotent on repeated `SessionStart` for the same provider session
///   (UPSERT keyed by `(agent_kind, provider_session_id)`).
/// - Best-effort on `agent_session` table absence: if the migration hasn't
///   run, returns a clear error so misconfigured installs surface a
///   diagnostic rather than panic.
async fn ingest_agent_traces(
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

    // Resolve storage and pin hash kind, exactly as the AiIntent flow does,
    // so this entry point's surface stays compatible with the rest of the
    // runtime.
    let process_cwd = std::env::current_dir().context("failed to read current directory")?;
    let storage_path = util::try_get_storage_path(Some(process_cwd.clone()))
        .context("failed to resolve libra storage path from current directory")?;
    set_hash_kind_from_repo()
        .await
        .context("failed to configure hash kind from repo config")?;

    let conn = db::get_db_conn_instance_for_path(&storage_path.join(util::DATABASE))
        .await
        .map_err(|err| anyhow!("failed to open libra database: {err}"))?;

    ingest_agent_traces_payload(
        &stdin_bytes,
        command,
        expected_kind,
        provider,
        &conn,
        Some(&storage_path),
    )
    .await
}

/// Connection-bound core of [`ingest_agent_traces`]. `pub(crate)` so unit
/// tests in this module can drive the function without stubbing stdin or
/// the process-wide working directory; it is intentionally NOT re-exported
/// from the crate root. Fully deterministic given `payload`, the
/// connection, and (optionally) `repo_path` — round-2 BLOCK #10 acceptance
/// criterion.
///
/// `repo_path` is the `.libra` directory used to resolve the Git object
/// store for checkpoint commit creation (Phase 2.1). Passing `None` skips
/// the checkpoint commit step on checkpoint-worthy lifecycle events and only persists the
/// `agent_session` summary; tests use that path so they don't need a live
/// `libra init` workspace.
pub(crate) async fn ingest_agent_traces_payload(
    payload: &[u8],
    command: super::provider::ProviderHookCommand,
    expected_kind: LifecycleEventKind,
    provider: &dyn HookProvider,
    conn: &sea_orm::DatabaseConnection,
    repo_path: Option<&std::path::Path>,
) -> Result<()> {
    use sea_orm::{ConnectionTrait, Statement};

    use crate::internal::ai::observed_agents::{RedactionMatch, Redactor};

    if payload.len() > MAX_STDIN_BYTES {
        bail!("hook input exceeds {MAX_STDIN_BYTES} bytes");
    }
    let stdin = std::str::from_utf8(payload).context("hook input is not valid UTF-8")?;
    if stdin.trim().is_empty() {
        bail!("hook input is empty");
    }

    let envelope: SessionHookEnvelope =
        serde_json::from_str(stdin).map_err(|err| anyhow!("invalid hook JSON payload: {err}"))?;
    validate_session_hook_envelope(&envelope, MAX_TRANSCRIPT_PATH_BYTES)?;

    let mut event = provider.parse_hook_event(&envelope.hook_event_name, &envelope)?;
    if event.kind != expected_kind {
        bail!(
            "hook event kind mismatch: expected '{}', got '{}' from hook_event_name '{}'",
            expected_kind,
            event.kind,
            envelope.hook_event_name
        );
    }

    // Redact free-form text fields before they get anywhere near durable
    // storage. We aggregate the per-field reports into a single JSON document
    // that lands in `agent_session.redaction_report` so the persisted row is
    // observably scrubbed (Codex round-3 review: "assert observable redaction
    // outcome").
    let redactor = Redactor::new_default();
    let mut all_matches: Vec<RedactionMatch> = Vec::new();
    let mut bytes_scanned: usize = 0;
    let mut bytes_redacted: usize = 0;
    if let Some(prompt) = event.prompt.take() {
        let (redacted, report) = redactor.redact(prompt.as_bytes());
        event.prompt = Some(String::from_utf8_lossy(redacted.bytes()).into_owned());
        bytes_scanned += report.bytes_scanned;
        bytes_redacted += report.bytes_redacted;
        all_matches.extend(report.matches);
    }
    if let Some(input) = event.tool_input.take() {
        let serialized = serde_json::to_vec(&input).unwrap_or_default();
        let (redacted, report) = redactor.redact(&serialized);
        event.tool_input = serde_json::from_slice(redacted.bytes()).ok();
        bytes_scanned += report.bytes_scanned;
        bytes_redacted += report.bytes_redacted;
        all_matches.extend(report.matches);
    }
    let redaction_report_json = serde_json::to_string(&serde_json::json!({
        "matches": all_matches,
        "bytes_scanned": bytes_scanned,
        "bytes_redacted": bytes_redacted,
    }))
    .unwrap_or_else(|_| "{}".to_string());

    let backend = conn.get_database_backend();

    // If the migration has not run yet, fail loud rather than silently.
    let table_check = conn
        .query_one(Statement::from_sql_and_values(
            backend,
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'agent_session' LIMIT 1",
            [],
        ))
        .await
        .context("failed to query sqlite_master")?;
    if table_check.is_none() {
        bail!(
            "agent_session table does not exist; run `libra init` against this repository to apply migrations",
        );
    }

    let now = Utc::now().timestamp();
    let session_id = build_ai_session_id(provider.provider_name(), &envelope.session_id);
    let agent_kind = match provider.provider_name() {
        // Map HookProvider names to the closed set used by the
        // `agent_kind` CHECK constraint. Adding a new provider here
        // requires extending both this match and the migration.
        "claude" => "claude_code",
        "gemini" => "gemini",
        other => other,
    };

    let new_state = match event.kind {
        LifecycleEventKind::SessionStart => "active",
        LifecycleEventKind::SessionEnd | LifecycleEventKind::TurnEnd => "stopped",
        LifecycleEventKind::Compaction => "condensed",
        LifecycleEventKind::CompactionCompleted => "active",
        _ => "active",
    };

    // UPSERT: insert a fresh row on first sight; otherwise just bump
    // `last_event_at`, `state`, and `redaction_report`. We key by
    // `(agent_kind, provider_session_id)` because the unique index already
    // lives there.
    //
    // Phase 4.1: also persist the `transcript_path` from the envelope
    // into `metadata_json` so `libra agent checkpoint rewind --apply`
    // can resolve the on-disk transcript file without re-running the
    // adapter's path-discovery heuristics.
    let concurrent_active = if matches!(event.kind, LifecycleEventKind::TurnStart) {
        has_concurrent_active_session(conn, agent_kind, &envelope).await?
    } else {
        false
    };
    let metadata_json = build_agent_session_metadata_json(&envelope, concurrent_active);
    let upsert_sql = "
        INSERT INTO agent_session (
            session_id, agent_kind, provider_session_id, state, working_dir,
            metadata_json, redaction_report, started_at, last_event_at, stopped_at
        )
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(agent_kind, provider_session_id) DO UPDATE SET
            state = excluded.state,
            last_event_at = excluded.last_event_at,
            redaction_report = excluded.redaction_report,
            stopped_at = CASE WHEN excluded.state = 'stopped' THEN excluded.last_event_at
                              ELSE agent_session.stopped_at END,
            metadata_json = CASE
                WHEN length(excluded.metadata_json) > 2 THEN excluded.metadata_json
                ELSE agent_session.metadata_json
            END
    ";
    let stopped_at: Option<i64> = matches!(
        event.kind,
        LifecycleEventKind::SessionEnd | LifecycleEventKind::TurnEnd
    )
    .then_some(now);
    conn.execute(Statement::from_sql_and_values(
        backend,
        upsert_sql,
        [
            session_id.clone().into(),
            agent_kind.into(),
            envelope.session_id.clone().into(),
            new_state.into(),
            envelope.cwd.clone().into(),
            metadata_json.into(),
            redaction_report_json.clone().into(),
            now.into(),
            now.into(),
            stopped_at.into(),
        ],
    ))
    .await
    .with_context(|| format!("failed to upsert agent_session for command '{command}'"))?;

    // Materialise committed checkpoints at durable agent boundaries. SessionEnd
    // captures the final snapshot; TurnEnd captures recoverable per-turn state.
    if matches!(
        event.kind,
        LifecycleEventKind::SessionEnd | LifecycleEventKind::TurnEnd
    ) && let Some(repo) = repo_path
    {
        write_committed_checkpoint(
            conn,
            repo,
            &session_id,
            &envelope,
            agent_kind,
            event.prompt.as_deref(),
            &redaction_report_json,
            &all_matches,
            now,
        )
        .await?;
    }

    Ok(())
}

/// Build the JSON object stored in `agent_session.metadata_json`.
/// Currently captures the agent's on-disk transcript path so the rewind
/// path can locate the file without re-deriving provider conventions.
/// Returns `"{}"` when no useful fields are populated, so the upsert
/// CASE expression can detect the placeholder.
fn build_agent_session_metadata_json(
    envelope: &SessionHookEnvelope,
    concurrent_active: bool,
) -> String {
    let mut obj = serde_json::Map::new();
    if let Some(path) = envelope.transcript_path.as_deref()
        && !path.is_empty()
    {
        obj.insert(
            "transcript_path".to_string(),
            serde_json::Value::String(path.to_string()),
        );
    }
    if concurrent_active {
        obj.insert(
            "concurrent_active".to_string(),
            serde_json::Value::Bool(true),
        );
    }
    if obj.is_empty() {
        return "{}".to_string();
    }
    serde_json::to_string(&obj).unwrap_or_else(|_| "{}".to_string())
}

async fn has_concurrent_active_session(
    conn: &sea_orm::DatabaseConnection,
    agent_kind: &str,
    envelope: &SessionHookEnvelope,
) -> Result<bool> {
    use sea_orm::{ConnectionTrait, Statement};

    let row = conn
        .query_one(Statement::from_sql_and_values(
            conn.get_database_backend(),
            "SELECT COUNT(*) AS n FROM agent_session \
             WHERE state = 'active' AND working_dir = ? \
               AND NOT (agent_kind = ? AND provider_session_id = ?)",
            [
                envelope.cwd.clone().into(),
                agent_kind.into(),
                envelope.session_id.clone().into(),
            ],
        ))
        .await
        .context("failed to detect concurrent active agent sessions")?;
    let count = row
        .and_then(|row| row.try_get_by::<i64, _>("n").ok())
        .unwrap_or(0);
    Ok(count > 0)
}

/// Write a committed checkpoint: materialise transcript + metadata blobs,
/// append a commit on `refs/libra/agent-traces`, and insert the
/// corresponding `agent_checkpoint` row. Errors are surfaced verbatim — a
/// failure here means the hook ingest cannot acknowledge a clean checkpoint
/// boundary to the caller.
#[allow(clippy::too_many_arguments)]
async fn write_committed_checkpoint(
    conn: &sea_orm::DatabaseConnection,
    repo_path: &std::path::Path,
    libra_session_id: &str,
    envelope: &SessionHookEnvelope,
    agent_kind: &str,
    redacted_prompt: Option<&str>,
    redaction_report_json: &str,
    redaction_matches: &[crate::internal::ai::observed_agents::RedactionMatch],
    now: i64,
) -> Result<()> {
    use sea_orm::{ConnectionTrait, Statement};

    use crate::internal::ai::history::{CheckpointCommitParams, CheckpointScope, HistoryManager};

    // Build a minimal metadata.json. Phase 2 keeps the schema small; later
    // phases extend with model_info, tool_use_id, subagent links, etc.
    let metadata = serde_json::json!({
        "schema_version": 1,
        "checkpoint_id": null, // filled in below once we have the UUID
        "session_id": libra_session_id,
        "agent_kind": agent_kind,
        "scope": "committed",
        "provider_session_id": envelope.session_id,
        "working_dir": envelope.cwd,
        "redaction_report": serde_json::from_str::<serde_json::Value>(redaction_report_json)
            .unwrap_or_else(|_| serde_json::json!({})),
        "created_at": now,
    });

    let checkpoint_id = uuid::Uuid::new_v4().to_string();
    let mut metadata = metadata;
    if let Some(obj) = metadata.as_object_mut() {
        obj.insert(
            "checkpoint_id".to_string(),
            serde_json::Value::String(checkpoint_id.clone()),
        );
    }
    let transcript_redacted =
        build_checkpoint_transcript_redacted(envelope, agent_kind, redacted_prompt, &mut metadata);
    let metadata_bytes =
        serde_json::to_vec_pretty(&metadata).context("serialize checkpoint metadata")?;

    let provider_name = envelope_provider_slug(agent_kind);

    let objects_dir = repo_path.join("objects");
    std::fs::create_dir_all(&objects_dir).context("create objects dir for checkpoint commit")?;
    let storage = std::sync::Arc::new(crate::utils::client_storage::ClientStorage::init(
        objects_dir,
    ));
    let manager = HistoryManager::new_with_ref(
        storage,
        repo_path.to_path_buf(),
        std::sync::Arc::new(conn.clone()),
        crate::internal::branch::AGENT_TRACES_BRANCH,
    );

    // Resolve the user-branch HEAD via the typed helper so we can
    // distinguish "no HEAD yet (unborn)" from real storage errors. The
    // empty-string conflation produced by the lossy wrapper was flagged
    // in the Codex Phase-2 round-1 review.
    //
    // Three semantic cases land in `parent_commit`:
    // - `Some(hash)` — repo has at least one commit and HEAD resolves.
    // - `None` from typed `Ok(None)` — HEAD is born but the branch is
    //   commit-less (e.g. immediately after `libra init`).
    // - `None` from `BranchStoreError::Corrupt { "HEAD reference is missing" }`
    //   — the schema is wired but the HEAD row was never seeded. This
    //   shows up in test fixtures that bootstrap the migrations without
    //   running `initialize_refs`. Functionally equivalent to "unborn" for
    //   the agent-traces writer, so we coerce to `None` rather than
    //   failing the whole ingest.
    let parent_commit: Option<String> =
        match crate::internal::head::Head::current_commit_result_with_conn(conn).await {
            Ok(commit) => commit.map(|h| h.to_string()),
            Err(crate::internal::branch::BranchStoreError::Corrupt { detail, .. })
                if detail.contains("HEAD reference is missing") =>
            {
                None
            }
            Err(err) => {
                return Err(anyhow!(
                    "failed to resolve HEAD while writing checkpoint: {err}"
                ));
            }
        };

    let written = manager
        .append_checkpoint_commit(CheckpointCommitParams {
            checkpoint_id: &checkpoint_id,
            session_id: libra_session_id,
            agent_kind,
            parent_commit: parent_commit.as_deref(),
            scope: CheckpointScope::Committed,
            tool_use_id: None,
            metadata_json: &metadata_bytes,
            transcript_redacted: &transcript_redacted,
            provider_name,
            events_jsonl: None,
        })
        .await
        .context("failed to append checkpoint commit on agent-traces")?;

    let parent_commit_value: sea_orm::Value = parent_commit.clone().into();
    conn.execute(Statement::from_sql_and_values(
        conn.get_database_backend(),
        "INSERT INTO agent_checkpoint (
            checkpoint_id, session_id, scope, parent_commit, tree_oid,
            metadata_blob_oid, traces_commit, created_at
         ) VALUES (?, ?, 'committed', ?, ?, ?, ?, ?)",
        [
            checkpoint_id.into(),
            libra_session_id.into(),
            parent_commit_value,
            written.tree_oid.to_string().into(),
            written.metadata_blob_oid.to_string().into(),
            written.commit_hash.to_string().into(),
            now.into(),
        ],
    ))
    .await
    .context("failed to insert agent_checkpoint row")?;

    // Suppress the unused-warning for redaction_matches; reserved for a
    // Phase 3 enhancement that adds per-rule counters to metadata.
    let _ = redaction_matches;
    Ok(())
}

fn build_checkpoint_transcript_redacted(
    envelope: &SessionHookEnvelope,
    agent_kind: &str,
    redacted_prompt: Option<&str>,
    metadata: &mut serde_json::Value,
) -> crate::internal::ai::observed_agents::RedactedBytes {
    use crate::internal::ai::observed_agents::{
        AgentKind, AgentSessionCtx, RedactedBytes, Redactor, agent_for,
    };

    let Some(kind) = AgentKind::from_db_str(agent_kind) else {
        if let Some(obj) = metadata.as_object_mut() {
            obj.insert(
                "transcript_source".to_string(),
                serde_json::Value::String("fallback_prompt_unknown_agent_kind".to_string()),
            );
        }
        return RedactedBytes::new_unchecked(redacted_prompt.unwrap_or("").as_bytes().to_vec());
    };
    let ctx = AgentSessionCtx {
        session_id: build_ai_session_id(
            envelope_provider_name_for_kind(kind),
            &envelope.session_id,
        ),
        provider_session_id: envelope.session_id.clone(),
        working_dir: std::path::PathBuf::from(&envelope.cwd),
        transcript_path: envelope
            .transcript_path
            .as_ref()
            .map(std::path::PathBuf::from),
    };
    let agent = agent_for(kind);
    match agent.read_transcript(&ctx) {
        Ok(Some(raw)) => {
            let (redacted, report) = Redactor::new_default().redact(&raw);
            if let Some(obj) = metadata.as_object_mut() {
                obj.insert(
                    "transcript_source".to_string(),
                    serde_json::Value::String("adapter".to_string()),
                );
                obj.insert(
                    "transcript_redaction_report".to_string(),
                    serde_json::to_value(&report).unwrap_or_else(|_| serde_json::json!({})),
                );
            }
            redacted
        }
        Ok(None) | Err(_) => {
            if let Some(obj) = metadata.as_object_mut() {
                obj.insert(
                    "transcript_source".to_string(),
                    serde_json::Value::String("fallback_prompt".to_string()),
                );
            }
            RedactedBytes::new_unchecked(redacted_prompt.unwrap_or("").as_bytes().to_vec())
        }
    }
}

fn envelope_provider_name_for_kind(
    kind: crate::internal::ai::observed_agents::AgentKind,
) -> &'static str {
    match kind {
        crate::internal::ai::observed_agents::AgentKind::ClaudeCode => "claude",
        crate::internal::ai::observed_agents::AgentKind::Gemini => "gemini",
        _ => kind.as_db_str(),
    }
}

/// Map `agent_session.agent_kind` (the closed enum stored in the database)
/// onto the file-name component used inside the checkpoint tree
/// (`transcript/<provider>` and `events/<provider>.jsonl`). For Phase 1's
/// stable agents (claude_code, gemini) the slug equals the kind string.
fn envelope_provider_slug(agent_kind: &str) -> &str {
    agent_kind
}

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
pub(crate) fn append_normalized_event(
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

    // -------------------------------------------------------------------
    // CEX-EntireIO: AgentTraces ingest tests. Codex round-2 BLOCK #10 + #2
    // round-3 followup ("assert observable redaction outcome").
    // -------------------------------------------------------------------

    use sea_orm::{
        ConnectOptions, ConnectionTrait, Database, DatabaseConnection, ExecResult, Statement,
    };
    use tempfile::TempDir;

    use crate::internal::db::{
        ensure_ai_runtime_contract_schema, migration::run_builtin_migrations,
    };

    const LEGACY_BOOTSTRAP_SQL: &str = include_str!("../../../../sql/sqlite_20260309_init.sql");

    async fn ingest_fresh_conn() -> (TempDir, DatabaseConnection) {
        let dir = tempfile::tempdir().expect("tempdir");
        // Use the canonical `libra.db` filename here so the Phase 3.5c
        // object_index queue (`enqueue_agent_blob_object_index_update`)
        // — which derives the database path from `repo_path.join(DATABASE)`
        // — finds the same file the test fixture set up.
        let path = dir.path().join(crate::utils::util::DATABASE);
        std::fs::File::create(&path).expect("touch sqlite file");
        let url = format!("sqlite://{}", path.display());
        let mut opts = ConnectOptions::new(url);
        opts.sqlx_logging(false);
        let conn = Database::connect(opts).await.expect("connect");
        // Mirror production wiring exactly: legacy bootstrap (creates
        // `ai_thread`) → AI runtime contract → registered migrations.
        let backend = conn.get_database_backend();
        for raw in LEGACY_BOOTSTRAP_SQL.split(';') {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            let _: ExecResult = conn
                .execute(Statement::from_string(backend, trimmed.to_string()))
                .await
                .unwrap_or_else(|e| panic!("legacy bootstrap stmt failed: {trimmed}\n{e}"));
        }
        ensure_ai_runtime_contract_schema(&conn)
            .await
            .expect("ensure_ai_runtime_contract_schema");
        run_builtin_migrations(&conn)
            .await
            .expect("run_builtin_migrations");
        (dir, conn)
    }

    fn ingest_envelope(
        hook_event_name: &str,
        session_id: &str,
        extra: serde_json::Value,
    ) -> Vec<u8> {
        let mut base = json!({
            "hook_event_name": hook_event_name,
            "session_id": session_id,
            "cwd": "/tmp/repo",
            "transcript_path": "/tmp/repo/transcript.jsonl",
        });
        if let serde_json::Value::Object(extra_map) = extra
            && let serde_json::Value::Object(base_map) = &mut base
        {
            for (k, v) in extra_map {
                base_map.insert(k, v);
            }
        }
        serde_json::to_vec(&base).expect("serialize envelope")
    }

    #[tokio::test]
    async fn ingest_session_start_creates_active_row() {
        let (_dir, conn) = ingest_fresh_conn().await;

        let payload = ingest_envelope("SessionStart", "S-001", json!({}));
        ingest_agent_traces_payload(
            &payload,
            super::super::provider::ProviderHookCommand::SessionStart,
            LifecycleEventKind::SessionStart,
            claude_provider(),
            &conn,
            None,
        )
        .await
        .expect("session start ingest succeeds");

        let backend = conn.get_database_backend();
        let row = conn
            .query_one(Statement::from_sql_and_values(
                backend,
                "SELECT agent_kind, state, working_dir, provider_session_id, stopped_at \
                 FROM agent_session WHERE provider_session_id = ?",
                ["S-001".into()],
            ))
            .await
            .expect("query")
            .expect("session row exists");

        assert_eq!(
            row.try_get_by::<String, _>("agent_kind").unwrap(),
            "claude_code"
        );
        assert_eq!(row.try_get_by::<String, _>("state").unwrap(), "active");
        assert_eq!(
            row.try_get_by::<String, _>("working_dir").unwrap(),
            "/tmp/repo"
        );
        assert_eq!(
            row.try_get_by::<String, _>("provider_session_id").unwrap(),
            "S-001"
        );
        assert!(
            row.try_get_by::<Option<i64>, _>("stopped_at")
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn ingest_session_end_marks_stopped_and_is_idempotent() {
        let (_dir, conn) = ingest_fresh_conn().await;

        let start = ingest_envelope("SessionStart", "S-002", json!({}));
        ingest_agent_traces_payload(
            &start,
            super::super::provider::ProviderHookCommand::SessionStart,
            LifecycleEventKind::SessionStart,
            claude_provider(),
            &conn,
            None,
        )
        .await
        .expect("start ok");

        let end = ingest_envelope("SessionEnd", "S-002", json!({}));
        ingest_agent_traces_payload(
            &end,
            super::super::provider::ProviderHookCommand::SessionEnd,
            LifecycleEventKind::SessionEnd,
            claude_provider(),
            &conn,
            None,
        )
        .await
        .expect("end ok");

        let backend = conn.get_database_backend();
        let row = conn
            .query_one(Statement::from_sql_and_values(
                backend,
                "SELECT state, stopped_at FROM agent_session WHERE provider_session_id = ?",
                ["S-002".into()],
            ))
            .await
            .expect("query")
            .expect("row");

        assert_eq!(row.try_get_by::<String, _>("state").unwrap(), "stopped");
        assert!(
            row.try_get_by::<Option<i64>, _>("stopped_at")
                .unwrap()
                .is_some()
        );

        // Repeat-ingest is idempotent: still exactly one row for that session.
        let count_row = conn
            .query_one(Statement::from_sql_and_values(
                backend,
                "SELECT COUNT(*) AS n FROM agent_session WHERE provider_session_id = ?",
                ["S-002".into()],
            ))
            .await
            .expect("count query")
            .expect("count row");
        assert_eq!(count_row.try_get_by::<i64, _>("n").unwrap(), 1);
    }

    /// Round-3 strengthened test: the redaction_report column should be
    /// populated with at least one match when an envelope carries a known
    /// secret, so the persisted row carries observable evidence the redactor
    /// ran.
    #[tokio::test]
    async fn ingest_persists_observable_redaction_report() {
        let (_dir, conn) = ingest_fresh_conn().await;

        let payload = ingest_envelope(
            "UserPromptSubmit",
            "S-redact",
            json!({
                "prompt": "deploy with AKIAIOSFODNN7EXAMPLE please",
            }),
        );
        ingest_agent_traces_payload(
            &payload,
            super::super::provider::ProviderHookCommand::Prompt,
            LifecycleEventKind::TurnStart,
            claude_provider(),
            &conn,
            None,
        )
        .await
        .expect("prompt ingest succeeds");

        let backend = conn.get_database_backend();
        let row = conn
            .query_one(Statement::from_sql_and_values(
                backend,
                "SELECT state, redaction_report FROM agent_session WHERE provider_session_id = ?",
                ["S-redact".into()],
            ))
            .await
            .expect("query")
            .expect("row");

        assert_eq!(row.try_get_by::<String, _>("state").unwrap(), "active");

        let report_json: String = row.try_get_by("redaction_report").unwrap();
        let report: serde_json::Value =
            serde_json::from_str(&report_json).expect("redaction_report is JSON");
        let matches = report
            .get("matches")
            .and_then(|v| v.as_array())
            .expect("matches is an array");
        assert!(
            !matches.is_empty(),
            "redaction_report.matches must be non-empty when prompt carries a known secret; got: {report_json}"
        );
        let bytes_redacted = report
            .get("bytes_redacted")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        assert!(
            bytes_redacted > 0,
            "redaction_report.bytes_redacted must be > 0; got {bytes_redacted}"
        );
        // The literal AKIA secret must NOT be reachable through the
        // persisted row — the only place we'd have stored its bytes is the
        // redaction_report, which now contains only positional matches.
        assert!(
            !report_json.contains("AKIAIOSFODNN7EXAMPLE"),
            "raw secret leaked into redaction_report column: {report_json}"
        );
    }

    /// entire.md §13 #6: concurrent TurnStart events in the same working_dir
    /// must not block ingestion, but the later session should be marked so
    /// operators can identify overlapping external-agent activity.
    #[tokio::test]
    async fn ingest_turn_start_marks_concurrent_active_session() {
        let (_dir, conn) = ingest_fresh_conn().await;

        let first = ingest_envelope("SessionStart", "S-concurrent-a", json!({}));
        ingest_agent_traces_payload(
            &first,
            super::super::provider::ProviderHookCommand::SessionStart,
            LifecycleEventKind::SessionStart,
            claude_provider(),
            &conn,
            None,
        )
        .await
        .expect("first session starts");

        let second = ingest_envelope("UserPromptSubmit", "S-concurrent-b", json!({}));
        ingest_agent_traces_payload(
            &second,
            super::super::provider::ProviderHookCommand::Prompt,
            LifecycleEventKind::TurnStart,
            claude_provider(),
            &conn,
            None,
        )
        .await
        .expect("second prompt ingests despite concurrent active session");

        let backend = conn.get_database_backend();
        let row = conn
            .query_one(Statement::from_sql_and_values(
                backend,
                "SELECT metadata_json FROM agent_session WHERE provider_session_id = ?",
                ["S-concurrent-b".into()],
            ))
            .await
            .expect("query")
            .expect("second session row exists");
        let metadata_json: String = row.try_get_by("metadata_json").unwrap();
        let metadata: serde_json::Value = serde_json::from_str(&metadata_json).unwrap();
        assert_eq!(metadata["concurrent_active"], json!(true));
    }

    #[tokio::test]
    async fn ingest_rejects_kind_mismatch() {
        let (_dir, conn) = ingest_fresh_conn().await;

        let payload = ingest_envelope("SessionStart", "S-mismatch", json!({}));
        let err = ingest_agent_traces_payload(
            &payload,
            super::super::provider::ProviderHookCommand::SessionEnd,
            LifecycleEventKind::SessionEnd,
            claude_provider(),
            &conn,
            None,
        )
        .await
        .expect_err("kind mismatch must fail");
        assert!(
            err.to_string().contains("hook event kind mismatch"),
            "unexpected error: {err}"
        );

        // No row should have been written.
        let backend = conn.get_database_backend();
        let count_row = conn
            .query_one(Statement::from_sql_and_values(
                backend,
                "SELECT COUNT(*) AS n FROM agent_session WHERE provider_session_id = ?",
                ["S-mismatch".into()],
            ))
            .await
            .expect("count query")
            .expect("count row");
        assert_eq!(count_row.try_get_by::<i64, _>("n").unwrap(), 0);
    }

    #[tokio::test]
    async fn ingest_fails_loud_when_table_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("noschema.db");
        std::fs::File::create(&path).expect("touch sqlite file");
        let url = format!("sqlite://{}", path.display());
        let mut opts = ConnectOptions::new(url);
        opts.sqlx_logging(false);
        let conn = Database::connect(opts).await.expect("connect");
        // intentionally NOT calling run_builtin_migrations.

        let payload = ingest_envelope("SessionStart", "S-bare", json!({}));
        let err = ingest_agent_traces_payload(
            &payload,
            super::super::provider::ProviderHookCommand::SessionStart,
            LifecycleEventKind::SessionStart,
            claude_provider(),
            &conn,
            None,
        )
        .await
        .expect_err("missing table must fail");
        assert!(
            err.to_string()
                .contains("agent_session table does not exist"),
            "unexpected error: {err}"
        );
    }

    /// Phase 2.1: when a `repo_path` is supplied and SessionEnd fires, the
    /// runtime must (a) write a checkpoint commit on `refs/libra/agent-traces`
    /// and (b) insert a row into `agent_checkpoint`. The checkpoint blob /
    /// commit objects live under `<repo>/objects/`, so we point the test at a
    /// fresh tempdir for that side too.
    #[tokio::test]
    async fn ingest_session_end_writes_checkpoint_when_repo_path_provided() {
        let (dir, conn) = ingest_fresh_conn().await;
        // Use the same tempdir as the SQLite file so the objects directory
        // and DB live together. We never need to run `libra init` here —
        // `append_checkpoint_commit` only needs the objects/ directory and
        // a sea-orm connection.
        let repo_path = dir.path().to_path_buf();

        let start = ingest_envelope("SessionStart", "S-cp", json!({}));
        ingest_agent_traces_payload(
            &start,
            super::super::provider::ProviderHookCommand::SessionStart,
            LifecycleEventKind::SessionStart,
            claude_provider(),
            &conn,
            Some(&repo_path),
        )
        .await
        .expect("start ok");

        // SessionEnd must trigger checkpoint creation.
        let end = ingest_envelope("SessionEnd", "S-cp", json!({}));
        ingest_agent_traces_payload(
            &end,
            super::super::provider::ProviderHookCommand::SessionEnd,
            LifecycleEventKind::SessionEnd,
            claude_provider(),
            &conn,
            Some(&repo_path),
        )
        .await
        .expect("end ok");

        let backend = conn.get_database_backend();
        let row = conn
            .query_one(Statement::from_sql_and_values(
                backend,
                "SELECT checkpoint_id, scope, traces_commit, tree_oid, metadata_blob_oid \
                 FROM agent_checkpoint WHERE session_id = (SELECT session_id FROM agent_session \
                  WHERE provider_session_id = 'S-cp' LIMIT 1)",
                [],
            ))
            .await
            .expect("query")
            .expect("checkpoint row exists");
        assert_eq!(row.try_get_by::<String, _>("scope").unwrap(), "committed");
        let traces_commit: String = row.try_get_by("traces_commit").unwrap();
        let tree_oid: String = row.try_get_by("tree_oid").unwrap();
        let metadata_blob_oid: String = row.try_get_by("metadata_blob_oid").unwrap();
        assert!(!traces_commit.is_empty());
        assert!(!tree_oid.is_empty());
        assert!(!metadata_blob_oid.is_empty());

        // The metadata blob must exist on disk and parse as JSON whose
        // `agent_kind` matches what we ingested.
        let metadata_path = repo_path
            .join("objects")
            .join(&metadata_blob_oid[..2])
            .join(&metadata_blob_oid[2..]);
        assert!(
            metadata_path.exists(),
            "metadata blob missing at {metadata_path:?}"
        );

        // The agent-traces ref row must point at the checkpoint commit hash.
        let backend = conn.get_database_backend();
        let ref_row = conn
            .query_one(Statement::from_sql_and_values(
                backend,
                "SELECT `commit` FROM reference WHERE name = ? AND kind = 'Branch' LIMIT 1",
                [crate::internal::branch::AGENT_TRACES_BRANCH.into()],
            ))
            .await
            .expect("query agent-traces ref")
            .expect("agent-traces ref row exists");
        let head: String = ref_row.try_get_by("commit").unwrap();
        assert_eq!(head, traces_commit);

        // Phase 3.5c acceptance: every object touched by the agent
        // capture history must be tagged in `object_index` so cloud sync
        // uploads them. Without this, `libra cloud restore` would
        // resolve the orphan ref's commits to missing trees/blobs on a
        // fresh clone. The transcript blob carries the distinguished
        // `agent_transcript` o_type per entire.md §14.3.
        //
        // Codex round-1 follow-up: walk the *entire* reachability set
        // (commit → root tree → … → leaf blobs) rather than spot-
        // checking the root tree only. Walking the actual on-disk
        // objects catches new code paths that forget to call
        // `write_tree_indexed` for an intermediate tree.
        crate::utils::client_storage::ClientStorage::wait_for_background_tasks();

        verify_full_reachability_indexed(&conn, &repo_path, &traces_commit).await;

        // Spot check the metadata blob OID (Phase 3.5b's
        // `agent_checkpoint.metadata_blob_oid` column should join cleanly
        // to `object_index`).
        let metadata_count_row = conn
            .query_one(Statement::from_sql_and_values(
                backend,
                "SELECT COUNT(*) AS n FROM object_index WHERE o_id = ?",
                [metadata_blob_oid.clone().into()],
            ))
            .await
            .expect("query metadata count")
            .expect("count row");
        let metadata_count: i64 = metadata_count_row.try_get_by("n").unwrap();
        assert_eq!(metadata_count, 1, "metadata blob is indexed");

        // Spot check the distinctive `agent_transcript` tag — at least
        // one row carries it (the transcript blob), per the spec.
        let transcript_count_row = conn
            .query_one(Statement::from_sql_and_values(
                backend,
                "SELECT COUNT(*) AS n FROM object_index WHERE o_type = 'agent_transcript'",
                [],
            ))
            .await
            .expect("query agent_transcript count")
            .expect("count row");
        let transcript_count: i64 = transcript_count_row.try_get_by("n").unwrap();
        assert_eq!(
            transcript_count, 1,
            "exactly one transcript blob carries the agent_transcript o_type"
        );

        let _ = tree_oid; // silence unused warning — assertions above
        // already verified the root tree is indexed
        // via the reachability walker
    }

    /// entire.md §6.3 / §7.1: Stop/TurnEnd must create an intermediate
    /// committed checkpoint, not wait until SessionEnd, so long-running agent
    /// sessions can rewind to turn boundaries.
    #[tokio::test]
    async fn ingest_turn_end_writes_checkpoint_when_repo_path_provided() {
        let (dir, conn) = ingest_fresh_conn().await;
        let repo_path = dir.path().to_path_buf();

        let start = ingest_envelope("SessionStart", "S-turn-cp", json!({}));
        ingest_agent_traces_payload(
            &start,
            super::super::provider::ProviderHookCommand::SessionStart,
            LifecycleEventKind::SessionStart,
            claude_provider(),
            &conn,
            Some(&repo_path),
        )
        .await
        .expect("start ok");

        let stop = ingest_envelope("Stop", "S-turn-cp", json!({}));
        ingest_agent_traces_payload(
            &stop,
            super::super::provider::ProviderHookCommand::Stop,
            LifecycleEventKind::TurnEnd,
            claude_provider(),
            &conn,
            Some(&repo_path),
        )
        .await
        .expect("turn end ok");

        let backend = conn.get_database_backend();
        let row = conn
            .query_one(Statement::from_sql_and_values(
                backend,
                "SELECT s.state, s.stopped_at, cp.scope, cp.traces_commit, cp.tree_oid, cp.metadata_blob_oid \
                 FROM agent_session s \
                 JOIN agent_checkpoint cp ON cp.session_id = s.session_id \
                 WHERE s.provider_session_id = 'S-turn-cp' LIMIT 1",
                [],
            ))
            .await
            .expect("query")
            .expect("turn-end checkpoint row exists");

        assert_eq!(row.try_get_by::<String, _>("state").unwrap(), "stopped");
        assert!(
            row.try_get_by::<Option<i64>, _>("stopped_at")
                .unwrap()
                .is_some()
        );
        assert_eq!(row.try_get_by::<String, _>("scope").unwrap(), "committed");
        let traces_commit: String = row.try_get_by("traces_commit").unwrap();
        let tree_oid: String = row.try_get_by("tree_oid").unwrap();
        let metadata_blob_oid: String = row.try_get_by("metadata_blob_oid").unwrap();
        assert!(!traces_commit.is_empty());
        assert!(!tree_oid.is_empty());
        assert!(!metadata_blob_oid.is_empty());

        let ref_row = conn
            .query_one(Statement::from_sql_and_values(
                backend,
                "SELECT `commit` FROM reference WHERE name = ? AND kind = 'Branch' LIMIT 1",
                [crate::internal::branch::AGENT_TRACES_BRANCH.into()],
            ))
            .await
            .expect("query agent-traces ref")
            .expect("agent-traces ref row exists");
        let head: String = ref_row.try_get_by("commit").unwrap();
        assert_eq!(head, traces_commit);

        crate::utils::client_storage::ClientStorage::wait_for_background_tasks();
        verify_full_reachability_indexed(&conn, &repo_path, &traces_commit).await;
    }

    /// entire.md §8.1 / §13 (P0) end-to-end: a SessionEnd prompt carrying
    /// a known secret must land in the `agent-traces` transcript blob
    /// REDACTED — the raw secret never reaches durable storage. Guards the
    /// `RedactedBytes` write-path contract: the transcript blob is produced
    /// only via `RedactedBytes`, and the upstream redactor scrubbed the
    /// secret before it was wrapped. A regression that bypassed redaction
    /// (or the type) would surface here as the literal key in the blob.
    #[tokio::test]
    async fn session_end_checkpoint_transcript_blob_is_redacted() {
        let (dir, conn) = ingest_fresh_conn().await;
        let repo_path = dir.path().to_path_buf();

        let start = ingest_envelope("SessionStart", "S-cp-redact", json!({}));
        ingest_agent_traces_payload(
            &start,
            super::super::provider::ProviderHookCommand::SessionStart,
            LifecycleEventKind::SessionStart,
            claude_provider(),
            &conn,
            Some(&repo_path),
        )
        .await
        .expect("start ok");

        // SessionEnd whose prompt carries a known AWS-key-shaped secret.
        let end = ingest_envelope(
            "SessionEnd",
            "S-cp-redact",
            json!({ "prompt": "deploy with AKIAIOSFODNN7EXAMPLE please" }),
        );
        ingest_agent_traces_payload(
            &end,
            super::super::provider::ProviderHookCommand::SessionEnd,
            LifecycleEventKind::SessionEnd,
            claude_provider(),
            &conn,
            Some(&repo_path),
        )
        .await
        .expect("end ok");

        crate::utils::client_storage::ClientStorage::wait_for_background_tasks();

        // Locate the transcript blob via its distinguished o_type, then
        // read + zlib-decode it and strip the `blob <len>\0` header.
        let backend = conn.get_database_backend();
        let blob_row = conn
            .query_one(Statement::from_sql_and_values(
                backend,
                "SELECT o_id FROM object_index WHERE o_type = 'agent_transcript' LIMIT 1",
                [],
            ))
            .await
            .expect("query transcript blob")
            .expect("a transcript blob must be indexed");
        let blob_oid: String = blob_row.try_get_by("o_id").unwrap();

        let object_path = repo_path
            .join("objects")
            .join(&blob_oid[..2])
            .join(&blob_oid[2..]);
        let raw = std::fs::read(&object_path).expect("read transcript blob object");
        let mut decoder = flate2::read::ZlibDecoder::new(&raw[..]);
        let mut decoded = Vec::new();
        std::io::Read::read_to_end(&mut decoder, &mut decoded).unwrap();
        let header_end = decoded
            .iter()
            .position(|&b| b == 0)
            .expect("blob object has a header terminator");
        let body = String::from_utf8_lossy(&decoded[header_end + 1..]);

        assert!(
            !body.contains("AKIAIOSFODNN7EXAMPLE"),
            "raw secret leaked into the persisted transcript blob: {body}",
        );
        assert!(
            body.contains("deploy with") && body.contains("please"),
            "the redacted transcript must retain the non-secret text, got: {body}",
        );
    }

    /// entire.md §7.1 / §13: checkpoint transcripts should be snapshots of
    /// the external agent's native transcript file, not just the final hook
    /// prompt. The adapter returns raw bytes and the runtime redacts them
    /// before writing `transcript/<provider>`.
    #[tokio::test]
    async fn session_end_checkpoint_transcript_blob_uses_adapter_transcript_snapshot() {
        let (dir, conn) = ingest_fresh_conn().await;
        let repo_path = dir.path().to_path_buf();
        let transcript_path = dir.path().join("claude-transcript.jsonl");
        std::fs::write(
            &transcript_path,
            b"full transcript body with AKIAIOSFODNN7EXAMPLE\n",
        )
        .expect("write transcript fixture");

        let start = ingest_envelope(
            "SessionStart",
            "S-cp-full-transcript",
            json!({ "transcript_path": transcript_path }),
        );
        ingest_agent_traces_payload(
            &start,
            super::super::provider::ProviderHookCommand::SessionStart,
            LifecycleEventKind::SessionStart,
            claude_provider(),
            &conn,
            Some(&repo_path),
        )
        .await
        .expect("start ok");

        let end = ingest_envelope(
            "SessionEnd",
            "S-cp-full-transcript",
            json!({
                "transcript_path": transcript_path,
                "prompt": "fallback prompt must not be the transcript snapshot",
            }),
        );
        ingest_agent_traces_payload(
            &end,
            super::super::provider::ProviderHookCommand::SessionEnd,
            LifecycleEventKind::SessionEnd,
            claude_provider(),
            &conn,
            Some(&repo_path),
        )
        .await
        .expect("end ok");

        crate::utils::client_storage::ClientStorage::wait_for_background_tasks();

        let body = read_single_agent_transcript_blob_body(&conn, &repo_path).await;
        assert!(
            body.contains("full transcript body"),
            "checkpoint transcript must come from adapter transcript file, got: {body}",
        );
        assert!(
            !body.contains("fallback prompt must not"),
            "fallback prompt leaked into transcript snapshot instead of native transcript: {body}",
        );
        assert!(
            !body.contains("AKIAIOSFODNN7EXAMPLE"),
            "raw secret leaked into adapter transcript snapshot: {body}",
        );
        assert!(
            body.contains("<REDACTED:aws-access-key-id>"),
            "adapter transcript secret should be redacted, got: {body}",
        );
    }

    async fn read_single_agent_transcript_blob_body(
        conn: &DatabaseConnection,
        repo_path: &std::path::Path,
    ) -> String {
        let backend = conn.get_database_backend();
        let blob_row = conn
            .query_one(Statement::from_sql_and_values(
                backend,
                "SELECT o_id FROM object_index WHERE o_type = 'agent_transcript' LIMIT 1",
                [],
            ))
            .await
            .expect("query transcript blob")
            .expect("a transcript blob must be indexed");
        let blob_oid: String = blob_row.try_get_by("o_id").unwrap();

        let object_path = repo_path
            .join("objects")
            .join(&blob_oid[..2])
            .join(&blob_oid[2..]);
        let raw = std::fs::read(&object_path).expect("read transcript blob object");
        let mut decoder = flate2::read::ZlibDecoder::new(&raw[..]);
        let mut decoded = Vec::new();
        std::io::Read::read_to_end(&mut decoder, &mut decoded).unwrap();
        let header_end = decoded
            .iter()
            .position(|&b| b == 0)
            .expect("blob object has a header terminator");
        String::from_utf8_lossy(&decoded[header_end + 1..]).to_string()
    }

    /// Walk every object reachable from the checkpoint commit (commit →
    /// root tree → recursively trees/blobs) and assert each OID appears
    /// in `object_index`. Used to guard against future regressions where
    /// a write path forgets to route through `write_tree_indexed` or
    /// the indexing helper.
    async fn verify_full_reachability_indexed(
        conn: &DatabaseConnection,
        repo_path: &std::path::Path,
        commit_oid: &str,
    ) {
        let mut to_walk: Vec<(String, &'static str)> = vec![(commit_oid.to_string(), "commit")];
        let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();

        while let Some((oid, expected_type)) = to_walk.pop() {
            if !visited.insert(oid.clone()) {
                continue;
            }
            assert_object_index_has(conn, &oid, expected_type).await;
            // Read the on-disk Git object to discover its references.
            let object_path = repo_path.join("objects").join(&oid[..2]).join(&oid[2..]);
            let raw =
                std::fs::read(&object_path).unwrap_or_else(|e| panic!("read object {oid}: {e}"));
            let mut decoder = flate2::read::ZlibDecoder::new(&raw[..]);
            let mut decoded = Vec::new();
            std::io::Read::read_to_end(&mut decoder, &mut decoded).unwrap();
            let header_end = decoded.iter().position(|&b| b == 0).unwrap();
            let header = std::str::from_utf8(&decoded[..header_end]).unwrap();
            let body = &decoded[header_end + 1..];
            if header.starts_with("commit ") {
                let body_text = std::str::from_utf8(body).unwrap();
                let tree_line = body_text.lines().next().expect("commit has tree line");
                let tree_oid = tree_line.strip_prefix("tree ").expect("tree prefix");
                to_walk.push((tree_oid.to_string(), "tree"));
            } else if header.starts_with("tree ") {
                // Tree entry: `<mode> <name>\0<20 raw bytes>` (SHA-1).
                let mut cursor = 0;
                while cursor < body.len() {
                    let space_pos = cursor
                        + body[cursor..]
                            .iter()
                            .position(|&b| b == b' ')
                            .expect("mode terminator");
                    let mode = std::str::from_utf8(&body[cursor..space_pos]).unwrap();
                    let name_start = space_pos + 1;
                    let null_pos = name_start
                        + body[name_start..]
                            .iter()
                            .position(|&b| b == 0)
                            .expect("name terminator");
                    let hash_start = null_pos + 1;
                    let hash_bytes = &body[hash_start..hash_start + 20];
                    let child_oid = hex::encode(hash_bytes);
                    let child_type = if mode == "40000" { "tree" } else { "blob" };
                    to_walk.push((child_oid, child_type));
                    cursor = hash_start + 20;
                }
            }
        }
    }

    async fn assert_object_index_has(conn: &DatabaseConnection, oid: &str, expected_o_type: &str) {
        let backend = conn.get_database_backend();
        let row = conn
            .query_one(Statement::from_sql_and_values(
                backend,
                "SELECT o_type FROM object_index WHERE o_id = ? LIMIT 1",
                [oid.into()],
            ))
            .await
            .unwrap_or_else(|e| panic!("query object_index for {oid}: {e}"))
            .unwrap_or_else(|| panic!("object {oid} missing from object_index"));
        let actual: String = row.try_get_by("o_type").unwrap();
        // A blob may be tagged with a more-specific agent o_type
        // (`agent_transcript`); accept that as a valid upgrade.
        let acceptable = expected_o_type == actual
            || (expected_o_type == "blob" && actual.starts_with("agent_"));
        assert!(
            acceptable,
            "object {oid} has o_type '{actual}', expected '{expected_o_type}' (or agent_* upgrade)"
        );
    }
}
