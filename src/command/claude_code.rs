//! Entry point for ingesting Claude Code hook events and persisting them as AI history.

use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use clap::{Parser, Subcommand};
use git_internal::hash::{HashKind, set_hash_kind};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    internal::{
        ai::{
            history::HistoryManager,
            session::{
                AgentHookParser, ClaudeCodeAgentParser, LifecycleEvent, LifecycleEventKind,
                SessionHookEnvelope, SessionState, append_raw_hook_event, apply_lifecycle_event,
                make_dedup_key, normalize_json_value, validate_session_hook_envelope,
            },
        },
        db,
    },
    utils::{object::write_git_object, storage::local::LocalStorage, util},
};

const PROCESSED_EVENT_KEYS: &str = "processed_event_keys";
const MAX_STDIN_BYTES: usize = 1_048_576;
const MAX_PROCESSED_EVENT_KEYS: usize = 200;
const MAX_RAW_HOOK_EVENTS: usize = 200;
const MAX_TOOL_EVENTS: usize = 200;
const MAX_TRANSCRIPT_PATH_BYTES: usize = 4096;
const CLAUDE_SESSION_TYPE: &str = "claude_session";
/// Persisted blob schema identifier for Claude hook-ingested session payloads.
const CLAUDE_SESSION_SCHEMA: &str = "libra.claude_session.v1";
const CLAUDE_SETTINGS_DIR: &str = ".claude";
const CLAUDE_SETTINGS_FILE: &str = "settings.json";
const DEFAULT_HOOK_TIMEOUT_SECS: u64 = 10;
const CLAUDE_HOOK_FORWARD_MAP: &[(&str, &str)] = &[
    ("SessionStart", "session-start"),
    ("UserPromptSubmit", "prompt"),
    ("PostToolUse", "tool-use"),
    ("Stop", "stop"),
    ("SessionEnd", "session-end"),
];

/// Subcommands that map to Claude Code hook events.
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
    #[command(about = "Install Claude hook forwarding into .claude/settings.json")]
    InstallHooks(InstallHooksArgs),
}

/// Placeholder args reserved for future `claude-code` command options.
#[derive(Parser, Debug, Clone)]
pub struct ClaudeCodeArgs {}

#[derive(Parser, Debug, Clone)]
pub struct InstallHooksArgs {
    #[arg(
        long,
        default_value = "libra",
        help = "Command prefix used when generating Claude hook command entries"
    )]
    pub command_prefix: String,
    #[arg(
        long,
        default_value_t = DEFAULT_HOOK_TIMEOUT_SECS,
        help = "Timeout in seconds for each generated Claude hook command"
    )]
    pub timeout: u64,
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

#[derive(Debug, Serialize, Deserialize, Default)]
struct ClaudeSettings {
    #[serde(default)]
    hooks: BTreeMap<String, Vec<ClaudeHookMatcher>>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
struct ClaudeHookMatcher {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    matcher: Option<String>,
    hooks: Vec<ClaudeHookEntry>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
struct ClaudeHookEntry {
    #[serde(rename = "type")]
    entry_type: String,
    command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    timeout: Option<u64>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

/// Ingest one Claude hook event from stdin JSON and update/persist session state.
///
/// Stdin must be UTF-8 JSON with at least:
/// - `hook_event_name`
/// - `session_id`
/// - `cwd`
pub async fn execute(cmd: ClaudeCodeCommand) -> Result<()> {
    match cmd {
        ClaudeCodeCommand::InstallHooks(args) => install_hooks(args),
        other => {
            let expected_hook = claude_command_expected_hook(&other)?;
            let parser = ClaudeCodeAgentParser;
            process_hook_event(expected_hook, &parser).await
        }
    }
}

fn claude_command_expected_hook(cmd: &ClaudeCodeCommand) -> Result<&'static str> {
    match cmd {
        ClaudeCodeCommand::SessionStart(_) => Ok("SessionStart"),
        ClaudeCodeCommand::Prompt(_) => Ok("UserPromptSubmit"),
        ClaudeCodeCommand::ToolUse(_) => Ok("PostToolUse"),
        ClaudeCodeCommand::Stop(_) => Ok("Stop"),
        ClaudeCodeCommand::SessionEnd(_) => Ok("SessionEnd"),
        ClaudeCodeCommand::InstallHooks(_) => {
            bail!("install-hooks does not map to hook event name")
        }
    }
}

fn install_hooks(args: InstallHooksArgs) -> Result<()> {
    if args.command_prefix.trim().is_empty() {
        bail!("invalid --command-prefix: value cannot be empty");
    }
    if args.timeout == 0 {
        bail!("invalid --timeout: value must be greater than 0");
    }

    let project_root = resolve_project_root()?;
    let settings_path = project_root
        .join(CLAUDE_SETTINGS_DIR)
        .join(CLAUDE_SETTINGS_FILE);
    let mut settings = load_claude_settings(&settings_path)?;
    let changed = upsert_libra_hook_forwarding(&mut settings, &args.command_prefix, args.timeout);

    if changed {
        write_claude_settings(&settings_path, &settings)?;
        println!(
            "Installed Claude hook forwarding at {}",
            settings_path.display()
        );
    } else {
        println!(
            "Claude hook forwarding is already up to date at {}",
            settings_path.display()
        );
    }

    Ok(())
}

fn resolve_project_root() -> Result<PathBuf> {
    if let Ok(repo_root) = util::try_working_dir() {
        return Ok(repo_root);
    }
    std::env::current_dir().context("failed to read current directory")
}

fn load_claude_settings(path: &Path) -> Result<ClaudeSettings> {
    if !path.exists() {
        return Ok(ClaudeSettings::default());
    }

    let content = fs::read_to_string(path)
        .with_context(|| format!("failed to read Claude settings file '{}'", path.display()))?;
    if content.trim().is_empty() {
        return Ok(ClaudeSettings::default());
    }

    serde_json::from_str(&content)
        .map_err(|e| anyhow!("invalid Claude settings JSON at '{}': {e}", path.display()))
}

fn write_claude_settings(path: &Path, settings: &ClaudeSettings) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        anyhow!(
            "invalid Claude settings path without parent: '{}'",
            path.display()
        )
    })?;
    fs::create_dir_all(parent).with_context(|| {
        format!(
            "failed to create Claude settings directory '{}'",
            parent.display()
        )
    })?;

    let mut data = serde_json::to_vec_pretty(settings)
        .context("failed to serialize Claude settings to JSON")?;
    data.push(b'\n');

    let tmp_path = path.with_extension("json.tmp");
    fs::write(&tmp_path, &data).with_context(|| {
        format!(
            "failed to write temporary Claude settings file '{}'",
            tmp_path.display()
        )
    })?;

    #[cfg(windows)]
    {
        if path.exists() {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    let _ = fs::remove_file(&tmp_path);
                    return Err(anyhow!(
                        "failed to replace existing Claude settings file '{}': {e}",
                        path.display()
                    ));
                }
            }
        }
    }

    fs::rename(&tmp_path, path).with_context(|| {
        format!(
            "failed to replace Claude settings file '{}' with '{}'",
            path.display(),
            tmp_path.display()
        )
    })?;
    Ok(())
}

fn upsert_libra_hook_forwarding(
    settings: &mut ClaudeSettings,
    command_prefix: &str,
    timeout: u64,
) -> bool {
    let mut changed = false;

    for (event_name, subcommand) in CLAUDE_HOOK_FORWARD_MAP {
        let desired_entry = ClaudeHookEntry {
            entry_type: "command".to_string(),
            command: format!("{command_prefix} claude-code {subcommand}"),
            timeout: Some(timeout),
            extra: BTreeMap::new(),
        };

        let original_matchers = settings.hooks.remove(*event_name).unwrap_or_default();
        let mut rebuilt_matchers = Vec::with_capacity(original_matchers.len() + 1);
        let mut has_desired_entry = false;

        for mut matcher in original_matchers {
            if matcher.matcher.is_none() && matcher.hooks == vec![desired_entry.clone()] {
                has_desired_entry = true;
                rebuilt_matchers.push(matcher);
                continue;
            }

            let matcher_name = matcher.matcher.as_deref();
            let original_hook_count = matcher.hooks.len();
            matcher.hooks.retain(|hook| {
                !is_replaced_managed_hook(matcher_name, hook, &desired_entry.command, subcommand)
            });
            if matcher.hooks.len() != original_hook_count {
                changed = true;
            }
            if matcher.hooks.is_empty() {
                continue;
            }
            rebuilt_matchers.push(matcher);
        }

        if !has_desired_entry {
            rebuilt_matchers.push(ClaudeHookMatcher {
                matcher: None,
                hooks: vec![desired_entry],
                extra: BTreeMap::new(),
            });
            changed = true;
        }

        settings
            .hooks
            .insert((*event_name).to_string(), rebuilt_matchers);
    }

    changed
}

fn is_replaced_managed_hook(
    matcher: Option<&str>,
    hook: &ClaudeHookEntry,
    desired_command: &str,
    subcommand: &str,
) -> bool {
    hook.command == desired_command
        || (matcher == Some("libra")
            && hook
                .command
                .ends_with(&format!(" claude-code {subcommand}")))
}

#[cfg(test)]
fn matcher_manages_command(matcher: &ClaudeHookMatcher, command: &str) -> bool {
    matcher.hooks.iter().any(|hook| hook.command == command)
}

async fn process_hook_event(expected_hook: &str, parser: &impl AgentHookParser) -> Result<()> {
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
        serde_json::from_str(&stdin).map_err(|e| anyhow!("invalid hook JSON payload: {e}"))?;
    validate_session_hook_envelope(&envelope, MAX_TRANSCRIPT_PATH_BYTES)?;

    if envelope.hook_event_name != expected_hook
        && !(expected_hook == "Stop" && envelope.hook_event_name == "SessionStop")
    {
        bail!(
            "hook_event_name mismatch: expected '{}', got '{}'",
            expected_hook,
            envelope.hook_event_name
        );
    }

    // Use process cwd as the trust boundary and resolve a canonical repo storage root from it.
    let process_cwd = std::env::current_dir().context("failed to read current directory")?;
    let storage_path = util::try_get_storage_path(Some(process_cwd.clone()))
        .context("failed to resolve libra storage path from current directory")?;
    set_hash_kind_from_repo()
        .await
        .context("failed to configure hash kind from repo config")?;
    let process_cwd_str = process_cwd.to_string_lossy().to_string();
    let session_store =
        crate::internal::ai::session::SessionStore::from_storage_path(&storage_path);

    let mut session = match session_store.load(&envelope.session_id) {
        Ok(s) => s,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let mut recovered = SessionState::new(&process_cwd_str);
            recovered.id = envelope.session_id.clone();
            recovered.working_dir = process_cwd_str.clone();
            recovered
                .metadata
                .insert("recovered_from_out_of_order".to_string(), json!(true));
            recovered
        }
        Err(err) => return Err(anyhow!("failed to load session: {err}")),
    };
    session.working_dir = process_cwd_str.clone();
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

    let dedup_key = make_dedup_key(parser, &envelope);
    if dedup_hit(&session, dedup_key.as_deref()) {
        if envelope.hook_event_name != "SessionEnd" {
            return Ok(());
        }
        // For SessionEnd, only skip when persistence is already confirmed.
        // This allows retried end events to recover after a previous failure.
        if session_persisted(&session) {
            return Ok(());
        }
    }

    let event = parser.parse_hook_event(&envelope.hook_event_name, &envelope)?;
    apply_hook_event(&mut session, &envelope, &event)?;
    if let Some(event_key) = dedup_key {
        append_processed_event_key(&mut session, event_key);
    }

    if event.kind == LifecycleEventKind::SessionEnd {
        match persist_session_history(&storage_path, &session, parser.source_name()).await {
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
                eprintln!("warning: failed to persist session history: {err}");
                session_store.save(&session).map_err(|e| {
                    anyhow!("failed to save session after persistence failure: {e}")
                })?;
                return Err(err.context("session history persistence failed"));
            }
        }
    }

    session_store
        .save(&session)
        .map_err(|e| anyhow!("failed to save session: {e}"))?;

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
) -> Result<()> {
    session.updated_at = Utc::now();

    if let Some(session_ref) = &event.session_ref {
        // Keep this as opaque metadata only; do not use it for file I/O without validation.
        session.metadata.insert(
            "transcript_path".to_string(),
            Value::String(session_ref.clone()),
        );
    }

    append_raw_hook_event(session, envelope, MAX_RAW_HOOK_EVENTS);
    apply_lifecycle_event(session, event, MAX_TOOL_EVENTS);

    match event.kind {
        LifecycleEventKind::SessionStart
        | LifecycleEventKind::TurnStart
        | LifecycleEventKind::ToolUse => {
            set_phase(session, ClaudeSessionPhase::Active);
        }
        LifecycleEventKind::TurnEnd => set_phase(session, ClaudeSessionPhase::Stopped),
        LifecycleEventKind::SessionEnd => set_phase(session, ClaudeSessionPhase::Ended),
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

fn dedup_hit(session: &SessionState, key: Option<&str>) -> bool {
    let Some(key) = key else {
        return false;
    };
    session
        .metadata
        .get(PROCESSED_EVENT_KEYS)
        .and_then(Value::as_array)
        .map(|items| items.iter().any(|v| v.as_str() == Some(key)))
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
    source_name: &str,
) -> anyhow::Result<PersistOutcome> {
    let objects_dir = storage_path.join("objects");
    std::fs::create_dir_all(&objects_dir)?;

    let storage = Arc::new(LocalStorage::new(objects_dir));
    let db_conn = Arc::new(db::get_db_conn_instance().await.clone());
    let history_manager = HistoryManager::new(storage, storage_path.to_path_buf(), db_conn);
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
        "schema": CLAUDE_SESSION_SCHEMA,
        "session": session,
        "raw_hook_events": session.metadata.get("raw_hook_events").cloned().unwrap_or(Value::Array(vec![])),
        "ingest_meta": {
            "source": source_name,
            "ingested_at": Utc::now().to_rfc3339(),
        }
    });

    // Canonical JSON keeps blob content deterministic for the same semantic payload.
    let blob_data = to_canonical_json_bytes(&payload)?;
    let blob_hash = write_git_object(storage_path, "blob", &blob_data)?;
    history_manager
        .append(CLAUDE_SESSION_TYPE, &session.id, blob_hash)
        .await?;

    Ok(PersistOutcome {
        object_hash: blob_hash.to_string(),
        already_exists: false,
    })
}

fn to_canonical_json_bytes(value: &Value) -> Result<Vec<u8>, serde_json::Error> {
    let normalized = normalize_json_value(value.clone());
    serde_json::to_vec(&normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_key_is_stable_for_same_payload() {
        let provider = ClaudeCodeAgentParser;
        let env = SessionHookEnvelope {
            hook_event_name: "UserPromptSubmit".to_string(),
            session_id: "s1".to_string(),
            cwd: "/tmp".to_string(),
            transcript_path: None,
            extra: {
                let mut m = serde_json::Map::new();
                m.insert("prompt".to_string(), Value::String("hello".to_string()));
                m.insert("event_id".to_string(), Value::String("evt-1".to_string()));
                m
            },
        };

        let k1 = make_dedup_key(&provider, &env);
        let k2 = make_dedup_key(&provider, &env);
        assert_eq!(k1, k2);
        assert!(k1.is_some());
    }

    #[test]
    fn processed_event_keys_capped() {
        let mut s = SessionState::new("/tmp");
        for i in 0..(MAX_PROCESSED_EVENT_KEYS + 50) {
            append_processed_event_key(&mut s, format!("k{i}"));
        }

        let len = s
            .metadata
            .get(PROCESSED_EVENT_KEYS)
            .and_then(Value::as_array)
            .map(std::vec::Vec::len)
            .unwrap_or(0);
        assert_eq!(len, MAX_PROCESSED_EVENT_KEYS);
    }

    #[test]
    fn validate_core_fields_rejects_missing() {
        let env = SessionHookEnvelope {
            hook_event_name: "".to_string(),
            session_id: "a".to_string(),
            cwd: "/tmp".to_string(),
            transcript_path: None,
            extra: serde_json::Map::new(),
        };
        assert!(validate_session_hook_envelope(&env, MAX_TRANSCRIPT_PATH_BYTES).is_err());
    }

    #[test]
    fn validate_core_fields_rejects_invalid_session_id() {
        let env = SessionHookEnvelope {
            hook_event_name: "SessionStart".to_string(),
            session_id: "../unsafe".to_string(),
            cwd: "/tmp".to_string(),
            transcript_path: None,
            extra: serde_json::Map::new(),
        };
        assert!(validate_session_hook_envelope(&env, MAX_TRANSCRIPT_PATH_BYTES).is_err());
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
    fn validate_core_fields_rejects_invalid_transcript_path() {
        let env = SessionHookEnvelope {
            hook_event_name: "SessionStart".to_string(),
            session_id: "s1".to_string(),
            cwd: "/tmp".to_string(),
            transcript_path: Some("\0bad".to_string()),
            extra: serde_json::Map::new(),
        };
        assert!(validate_session_hook_envelope(&env, MAX_TRANSCRIPT_PATH_BYTES).is_err());
    }

    #[test]
    fn event_key_absent_without_identity() {
        let provider = ClaudeCodeAgentParser;
        let env = SessionHookEnvelope {
            hook_event_name: "UserPromptSubmit".to_string(),
            session_id: "s1".to_string(),
            cwd: "/tmp".to_string(),
            transcript_path: None,
            extra: {
                let mut m = serde_json::Map::new();
                m.insert("prompt".to_string(), Value::String("hello".to_string()));
                m
            },
        };
        assert!(make_dedup_key(&provider, &env).is_none());
    }

    #[test]
    fn lifecycle_event_uses_fallback_key_without_identity() {
        let provider = ClaudeCodeAgentParser;
        let env = SessionHookEnvelope {
            hook_event_name: "SessionStart".to_string(),
            session_id: "s1".to_string(),
            cwd: "/tmp".to_string(),
            transcript_path: None,
            extra: serde_json::Map::new(),
        };
        assert!(make_dedup_key(&provider, &env).is_some());
    }

    #[test]
    fn upsert_libra_hook_forwarding_is_idempotent() {
        let mut settings = ClaudeSettings::default();
        assert!(upsert_libra_hook_forwarding(&mut settings, "libra", 10));
        assert!(!upsert_libra_hook_forwarding(&mut settings, "libra", 10));

        for (event_name, subcommand) in CLAUDE_HOOK_FORWARD_MAP {
            let matchers = settings
                .hooks
                .get(*event_name)
                .expect("expected event key to be present");
            let libra_matchers: Vec<&ClaudeHookMatcher> = matchers
                .iter()
                .filter(|matcher| {
                    matcher.matcher.is_none()
                        && matcher_manages_command(
                            matcher,
                            &format!("libra claude-code {subcommand}"),
                        )
                })
                .collect();
            assert_eq!(
                libra_matchers.len(),
                1,
                "expected a single managed hook entry for {}",
                event_name
            );
            assert_eq!(
                libra_matchers[0].hooks[0].command,
                format!("libra claude-code {subcommand}")
            );
        }
    }

    #[test]
    fn upsert_libra_hook_forwarding_preserves_existing_matchers() {
        let mut settings = ClaudeSettings::default();
        settings.hooks.insert(
            "SessionStart".to_string(),
            vec![ClaudeHookMatcher {
                matcher: Some("startup".to_string()),
                hooks: vec![ClaudeHookEntry {
                    entry_type: "command".to_string(),
                    command: "echo keep".to_string(),
                    timeout: Some(3),
                    extra: BTreeMap::new(),
                }],
                extra: BTreeMap::new(),
            }],
        );

        assert!(upsert_libra_hook_forwarding(&mut settings, "libra", 10));

        let session_start = settings
            .hooks
            .get("SessionStart")
            .expect("SessionStart should exist");
        assert!(
            session_start.iter().any(|matcher| {
                matcher.matcher.as_deref() == Some("startup")
                    && matcher.hooks[0].command == "echo keep"
            }),
            "existing matcher should be preserved"
        );
        assert!(
            session_start.iter().any(|matcher| {
                matcher.matcher.is_none()
                    && matcher_manages_command(matcher, "libra claude-code session-start")
            }),
            "managed hook should be added without matcher"
        );
    }
}
