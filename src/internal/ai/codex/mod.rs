//! Agent Codex command - directly connect to Codex app-server via WebSocket.

pub mod history;
pub mod model;
pub mod protocol;
pub mod schema_v2;
pub mod schema_v2_generated;
pub mod types;
pub mod view;

use std::{
    collections::{BTreeSet, HashMap},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, OnceLock},
};

use anyhow;
use chrono::Utc;
use clap::Parser;
use diffy::create_patch;
use futures_util::{SinkExt, StreamExt};
use git_internal::hash::ObjectHash;
use history::{HistoryReader, HistoryRecorder, HistoryWriter};
use model::{
    ContextFrameEvent, ContextSnapshot, DecisionEvent, EvidenceEvent, IntentEvent, IntentSnapshot,
    PatchSetSnapshot, PlanSnapshot, PlanStepEvent, PlanStepSnapshot, ProvenanceSnapshot, RunEvent,
    RunSnapshot, RunUsage, TaskEvent, TaskSnapshot, ToolInvocationEvent,
};
use protocol::MethodKind;
use schema_v2::*;
use tokio::sync::{Mutex as AsyncMutex, mpsc};
use tokio_tungstenite::{connect_async, tungstenite::Message};
pub use types::*;
use walkdir::WalkDir;

use crate::{
    internal::{
        ai::{history::HistoryManager, mcp::server::LibraMcpServer},
        db,
    },
    utils::{storage_ext::StorageExt, util::try_get_storage_path},
};

const CODEX_WS_URL: &str = "ws://127.0.0.1:8080";
static HISTORY_APPEND_LOCK: OnceLock<AsyncMutex<()>> = OnceLock::new();
const COMMAND_DIFF_MAX_FILE_SIZE: u64 = 256 * 1024;
const COMMAND_DIFF_MAX_FILES: usize = 512;

fn lock_or_warn<'a, T>(mutex: &'a Arc<Mutex<T>>, context: &str) -> Option<MutexGuard<'a, T>> {
    match mutex.lock() {
        Ok(guard) => Some(guard),
        Err(e) => {
            eprintln!("[WARN] {context}: failed to lock mutex: {e}");
            None
        }
    }
}

fn history_append_lock() -> &'static AsyncMutex<()> {
    HISTORY_APPEND_LOCK.get_or_init(|| AsyncMutex::new(()))
}

fn merge_patchset_changes(
    existing_changes: &[FileChange],
    completed_changes: &[FileChange],
) -> Vec<FileChange> {
    if completed_changes.is_empty() {
        return existing_changes.to_vec();
    }

    let mut merged = completed_changes.to_vec();

    // Preserve any previously captured streaming diff if the completed payload
    // only summarizes touched files and omits the actual patch text.
    let has_completed_diff = merged.iter().any(|change| !change.diff.is_empty());
    if !has_completed_diff {
        for existing_change in existing_changes {
            if merged
                .iter()
                .all(|change| change.path != existing_change.path)
            {
                merged.push(existing_change.clone());
            }
        }
    }

    merged
}

fn patch_status_from_str(status: &str) -> PatchStatus {
    match status {
        "in_progress" | "inProgress" | "started" => PatchStatus::InProgress,
        "completed" => PatchStatus::Completed,
        "failed" => PatchStatus::Failed,
        "declined" => PatchStatus::Declined,
        _ => PatchStatus::Pending,
    }
}

fn parse_patchset_changes_from_array(changes: Option<&serde_json::Value>) -> Vec<FileChange> {
    changes
        .and_then(|value| value.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|change| {
                    let path = change.get("path")?.as_str()?.to_string();
                    let diff = change
                        .get("diff")
                        .and_then(|d| d.as_str())
                        .unwrap_or("")
                        .to_string();
                    let change_type = change
                        .get("change_type")
                        .or_else(|| change.get("changeType"))
                        .or_else(|| change.get("kind").and_then(|k| k.get("type")))
                        .and_then(|c| c.as_str())
                        .unwrap_or("update")
                        .to_string();
                    Some(FileChange {
                        path,
                        diff,
                        change_type,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_patchset_changes_from_map(changes: Option<&serde_json::Value>) -> Vec<FileChange> {
    changes
        .and_then(|value| value.as_object())
        .map(|map| {
            map.iter()
                .map(|(path, change)| FileChange {
                    path: path.clone(),
                    diff: change
                        .get("unified_diff")
                        .or_else(|| change.get("unifiedDiff"))
                        .or_else(|| change.get("diff"))
                        .or_else(|| change.get("content"))
                        .and_then(|value| value.as_str())
                        .unwrap_or("")
                        .to_string(),
                    change_type: change
                        .get("type")
                        .and_then(|value| value.as_str())
                        .unwrap_or("update")
                        .to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn persist_patchset_snapshot_and_evidence(
    mcp_server: Arc<LibraMcpServer>,
    history: Arc<HistoryRecorder>,
    history_writer: Arc<HistoryWriter>,
    patchset: PatchSet,
    status: String,
    debug_mode: bool,
) {
    let patchset_id = patchset.id.clone();
    let files = patchset.changes.len();
    let touched_files: Vec<String> = patchset
        .changes
        .iter()
        .map(|change| change.path.clone())
        .collect();
    let patchset_snapshot = PatchSetSnapshot {
        id: patchset_id.clone(),
        run_id: patchset.run_id.clone(),
        thread_id: patchset.thread_id.clone(),
        created_at: Utc::now(),
        status: patchset.status.clone(),
        changes: patchset.changes.clone(),
    };
    let evidence = EvidenceEvent {
        id: format!("evidence_{}", patchset_id),
        run_id: patchset.run_id.clone(),
        patchset_id: Some(patchset_id.clone()),
        at: Utc::now(),
        kind: "patchset".to_string(),
        data: serde_json::json!({
            "files": files,
            "touched_files": touched_files,
        }),
    };

    tokio::spawn(async move {
        store_to_mcp(&mcp_server, "patchset", &patchset_id, &patchset, debug_mode).await;
        history
            .event(
                history::EventKind::ToolInvocationStatus,
                &patchset_id,
                status,
                serde_json::json!({ "files": files }),
            )
            .await;
        history_writer
            .write("patchset_snapshot", &patchset_id, &patchset_snapshot)
            .await;
        history_writer
            .write("evidence", &evidence.id, &evidence)
            .await;
    });
}

fn should_skip_diff_path(relative_path: &Path) -> bool {
    relative_path.components().any(|component| {
        let name = component.as_os_str().to_string_lossy();
        matches!(
            name.as_ref(),
            ".git" | ".libra" | "node_modules" | "target" | "dist" | "build"
        )
    })
}

fn is_probably_text(bytes: &[u8]) -> bool {
    !bytes.contains(&0) && std::str::from_utf8(bytes).is_ok()
}

fn capture_workspace_snapshot(cwd: &Path) -> HashMap<String, String> {
    let mut snapshot = HashMap::new();
    if !cwd.exists() || !cwd.is_dir() {
        return snapshot;
    }

    for entry in WalkDir::new(cwd).into_iter().filter_map(Result::ok) {
        let path = entry.path();
        if !entry.file_type().is_file() {
            continue;
        }

        let Ok(relative_path) = path.strip_prefix(cwd) else {
            continue;
        };
        if relative_path.as_os_str().is_empty() || should_skip_diff_path(relative_path) {
            continue;
        }

        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.len() > COMMAND_DIFF_MAX_FILE_SIZE {
            continue;
        }

        let Ok(bytes) = fs::read(path) else {
            continue;
        };
        if !is_probably_text(&bytes) {
            continue;
        }

        let Ok(content) = String::from_utf8(bytes) else {
            continue;
        };
        let relative_key = relative_path.to_string_lossy().replace('\\', "/");
        snapshot.insert(relative_key, content);

        if snapshot.len() >= COMMAND_DIFF_MAX_FILES {
            break;
        }
    }

    snapshot
}

fn render_snapshot_diff(before: &str, after: &str) -> String {
    create_patch(before, after).to_string()
}

fn build_file_changes_from_snapshots(
    before: &HashMap<String, String>,
    after: &HashMap<String, String>,
) -> Vec<FileChange> {
    let all_paths: BTreeSet<String> = before.keys().chain(after.keys()).cloned().collect();

    let mut changes = Vec::new();
    for path in all_paths {
        match (before.get(&path), after.get(&path)) {
            (None, Some(after_content)) => changes.push(FileChange {
                path,
                diff: render_snapshot_diff("", after_content),
                change_type: "add".to_string(),
            }),
            (Some(before_content), None) => changes.push(FileChange {
                path,
                diff: render_snapshot_diff(before_content, ""),
                change_type: "delete".to_string(),
            }),
            (Some(before_content), Some(after_content)) if before_content != after_content => {
                changes.push(FileChange {
                    path,
                    diff: render_snapshot_diff(before_content, after_content),
                    change_type: "update".to_string(),
                });
            }
            _ => {}
        }
    }

    changes
}

fn latest_thread_intent_id(
    session: &CodexSession,
    thread_id: &str,
    excluding_id: Option<&str>,
) -> Option<String> {
    session
        .intents
        .iter()
        .filter(|intent| {
            intent.thread_id == thread_id
                && excluding_id.is_none_or(|exclude_id| intent.id != exclude_id)
        })
        .max_by_key(|intent| intent.created_at)
        .map(|intent| intent.id.clone())
}

fn build_tool_invocation_event(invocation: &ToolInvocation) -> ToolInvocationEvent {
    ToolInvocationEvent {
        id: invocation.id.clone(),
        run_id: invocation.run_id.clone(),
        thread_id: invocation.thread_id.clone(),
        tool: invocation.tool_name.clone(),
        server: invocation.server.clone(),
        status: invocation.status.to_string(),
        at: Utc::now(),
        payload: serde_json::json!({
            "arguments": invocation.arguments.clone(),
            "result": invocation.result.clone(),
            "error": invocation.error.clone(),
            "duration_ms": invocation.duration_ms,
        }),
    }
}

fn next_tool_invocation_event_object_id(invocation_id: &str, status: &str) -> String {
    format!(
        "tool_invocation_event_{}_{}_{}",
        invocation_id,
        status,
        Utc::now().timestamp_millis()
    )
}

fn plan_status_from_event(status: &str) -> PlanStatus {
    match status {
        "completed" => PlanStatus::Completed,
        "in_progress" | "inProgress" => PlanStatus::InProgress,
        _ => PlanStatus::Pending,
    }
}

fn task_status_from_event(status: &str) -> TaskStatus {
    match status {
        "completed" => TaskStatus::Completed,
        "failed" => TaskStatus::Failed,
        "in_progress" => TaskStatus::InProgress,
        _ => TaskStatus::Pending,
    }
}

fn run_status_from_event(status: &str) -> RunStatus {
    match status {
        "completed" => RunStatus::Completed,
        "failed" => RunStatus::Failed,
        "in_progress" => RunStatus::InProgress,
        _ => RunStatus::Pending,
    }
}

async fn append_history_hash_if_changed(
    mcp_server: &Arc<LibraMcpServer>,
    object_type: &str,
    object_id: &str,
    hash: ObjectHash,
) -> Result<(), String> {
    let Some(history) = &mcp_server.intent_history_manager else {
        return Ok(());
    };

    let _guard = history_append_lock().lock().await;
    let should_append = match history.get_object_hash(object_type, object_id).await {
        Ok(Some(existing)) => existing != hash,
        Ok(None) => true,
        Err(e) => {
            return Err(format!(
                "Failed to check history for {object_type}/{object_id}: {e}"
            ));
        }
    };

    if should_append {
        history
            .append(object_type, object_id, hash)
            .await
            .map_err(|e| format!("Failed to append {object_type}/{object_id} to history: {e}"))?;
    }

    Ok(())
}

fn extract_thread_id(params: &serde_json::Value, session: Option<&CodexSession>) -> String {
    params
        .get("thread")
        .and_then(|thread| {
            thread
                .get("id")
                .or_else(|| thread.get("threadId"))
                .or_else(|| thread.get("thread_id"))
        })
        .and_then(|value| value.as_str())
        .map(String::from)
        .or_else(|| {
            params
                .get("threadId")
                .or_else(|| params.get("thread_id"))
                .and_then(|value| value.as_str())
                .map(String::from)
        })
        .or_else(|| {
            session.and_then(|session| {
                if session.thread.id.is_empty() {
                    None
                } else {
                    Some(session.thread.id.clone())
                }
            })
        })
        .unwrap_or_default()
}

fn extract_task_id(params: &serde_json::Value) -> String {
    params
        .get("taskId")
        .or_else(|| params.get("task_id"))
        .or_else(|| params.get("id"))
        .or_else(|| params.get("task").and_then(|task| task.get("id")))
        .or_else(|| params.get("task").and_then(|task| task.get("taskId")))
        .and_then(|value| value.as_str())
        .map(String::from)
        .unwrap_or_default()
}

fn extract_task_name(params: &serde_json::Value) -> String {
    params
        .get("taskName")
        .or_else(|| params.get("task_name"))
        .or_else(|| params.get("name"))
        .or_else(|| params.get("title"))
        .or_else(|| params.get("task").and_then(|task| task.get("name")))
        .or_else(|| params.get("task").and_then(|task| task.get("title")))
        .and_then(|value| value.as_str())
        .map(String::from)
        .unwrap_or_default()
}

fn normalize_plan_step_status(status: &str) -> &'static str {
    match status {
        "completed" => "completed",
        "failed" => "failed",
        "in_progress" | "inProgress" => "in_progress",
        _ => "pending",
    }
}

fn truncate_for_display(text: &str, max_chars: usize) -> (String, bool) {
    match text.char_indices().nth(max_chars) {
        Some((idx, _)) => (text[..idx].to_string(), true),
        None => (text.to_string(), false),
    }
}

fn task_status_from_plan_step(status: &str) -> TaskStatus {
    match normalize_plan_step_status(status) {
        "completed" => TaskStatus::Completed,
        "failed" => TaskStatus::Failed,
        "in_progress" => TaskStatus::InProgress,
        _ => TaskStatus::Pending,
    }
}

fn aggregate_plan_status(plan_steps: &[TurnPlanStep]) -> PlanStatus {
    if plan_steps
        .iter()
        .any(|step| normalize_plan_step_status(&step.status) == "in_progress")
    {
        PlanStatus::InProgress
    } else if !plan_steps.is_empty()
        && plan_steps
            .iter()
            .all(|step| normalize_plan_step_status(&step.status) == "completed")
    {
        PlanStatus::Completed
    } else {
        PlanStatus::Pending
    }
}

fn build_plan_text(explanation: Option<&String>, plan_steps: &[TurnPlanStep]) -> String {
    let lines: Vec<&str> = plan_steps
        .iter()
        .map(|step| step.step.trim())
        .filter(|step| !step.is_empty())
        .collect();

    match explanation
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        Some(explanation) if lines.is_empty() => explanation.to_string(),
        Some(explanation) => format!("{explanation}\n{}", lines.join("\n")),
        None => lines.join("\n"),
    }
}

/// Initialize MCP server for storing agent data
pub async fn init_mcp_server(working_dir: &Path) -> Arc<LibraMcpServer> {
    let storage_dir = try_get_storage_path(Some(working_dir.to_path_buf()))
        .unwrap_or_else(|_| working_dir.join(".libra"));
    let (objects_dir, dot_libra) = (storage_dir.join("objects"), storage_dir);

    // Try to create the directory
    if let Err(_e) = std::fs::create_dir_all(&objects_dir) {
        eprintln!(
            "Warning: Failed to create storage directory: {}. Running in read-only mode.",
            objects_dir.display()
        );
        return Arc::new(LibraMcpServer::new(None, None));
    }

    // Connect to DB
    let db_path = dot_libra.join("libra.db");
    let db_path_str = db_path.to_str().unwrap_or_default();

    #[cfg(target_os = "windows")]
    let db_path_string = db_path_str.replace("\\", "/");
    #[cfg(target_os = "windows")]
    let db_path_str = &db_path_string;

    let db_conn = match db::establish_connection(db_path_str).await {
        Ok(conn) => conn,
        Err(e) => {
            eprintln!(
                "Warning: Failed to connect to database: {}. Running in read-only mode. Error: {}",
                db_path.display(),
                e
            );
            return Arc::new(LibraMcpServer::new(None, None));
        }
    };

    // Initialize storage
    let storage: Arc<dyn crate::utils::storage::Storage + Send + Sync> =
        Arc::new(crate::utils::storage::local::LocalStorage::new(objects_dir));

    let intent_history_manager = Arc::new(HistoryManager::new(
        storage.clone(),
        dot_libra,
        Arc::new(db_conn),
    ));

    Arc::new(LibraMcpServer::new(
        Some(intent_history_manager),
        Some(storage),
    ))
}

#[derive(Parser, Debug, Clone)]
pub struct AgentCodexArgs {
    /// Codex WebSocket URL
    #[arg(long, default_value = CODEX_WS_URL)]
    pub url: String,

    /// Working directory for the agent
    #[arg(long, default_value = ".")]
    pub cwd: String,

    /// Approval mode: ask (prompt), accept (auto-accept), decline (auto-decline)
    #[arg(long, default_value = "accept")]
    pub approval: String,

    /// Model provider identifier passed to Codex
    #[arg(long)]
    pub model_provider: Option<String>,

    /// Service tier identifier passed to Codex
    #[arg(long)]
    pub service_tier: Option<String>,

    /// Personality identifier passed to Codex
    #[arg(long)]
    pub personality: Option<String>,

    /// Model identifier passed to Codex
    #[arg(long)]
    pub model: Option<String>,

    /// Require Codex to produce a plan before attempting execution.
    #[arg(long, default_value_t = false)]
    pub plan_mode: bool,

    /// Debug mode: print collected data
    #[arg(long, default_value = "false")]
    pub debug: bool,
}

fn plan_mode_developer_instructions() -> &'static str {
    concat!(
        "You are in strict plan-first mode. ",
        "Your first responsibility is to generate a structured plan update for the task. ",
        "Do not treat a normal conversational response as sufficient planning output. ",
        "You are replying in a plain terminal interface without Markdown rendering. ",
        "Do not use Markdown headings, bullet markers like '-' or '*', fenced code blocks, tables, or emphasis markers such as '**'. ",
        "Use plain text only. ",
        "Rules: ",
        "First produce a structured step-by-step plan/checklist. ",
        "Do not only explain the plan in prose. ",
        "Use the planning mechanism as the primary planning output whenever possible. ",
        "Do not execute tools, run commands, edit files, or create patches before the user approves the plan. ",
        "After the plan is produced, wait for explicit approval. ",
        "After approval, when files must be created or modified, choose Codex's structured file editing mode that emits fileChange callbacks/events. ",
        "Do not frame this as calling a fileChange tool; instead, choose the editing path that causes the session to produce item.type=fileChange updates. ",
        "Do not use patch-apply style editing or any path that emits apply_patch style events for file edits. ",
        "Prefer the structured edit path that produces fileChange events over commandExecution-based writes. ",
        "CommandExecution may still be used when necessary, but file edits should preferentially use the path that emits fileChange callbacks/events. ",
        "Use commandExecution primarily for inspection, validation, or running the project/tests. ",
        "If you need to change files, prefer the structured edit path that produces fileChange events rather than apply_patch. ",
        "Do not speculate that fileChange is unavailable just because no tool list is shown. ",
        "When the user approves execution and file edits are needed, attempt the structured edit path that should produce fileChange callbacks/events. ",
        "Only say that file writing is blocked if the system explicitly rejects the edit or explicitly reports that the structured fileChange-style path is unavailable. ",
        "Do not ask the user to manually copy code into files unless a real fileChange failure has already happened. ",
        "If the task appears simple, still provide a short structured plan first. ",
        "If you are uncertain, favor planning before acting. ",
        "Keep replies compact, readable, and suitable for direct CLI display. ",
        "The user must see a plan before any execution begins."
    )
}

fn plan_mode_base_instructions() -> &'static str {
    concat!(
        "Current mode: strict structured planning first. ",
        "Produce a structured plan before execution, prefer the planning system over prose-only planning, ",
        "wait for user approval before taking action, ",
        "and when modifying files choose the structured editing path that emits fileChange callbacks/events. ",
        "Do not use apply_patch-style editing. ",
        "Prefer the fileChange-emitting path over commandExecution-based file writes. ",
        "Do not claim that the fileChange-style path is unavailable unless the system explicitly reports that failure. ",
        "Reply in plain text without Markdown."
    )
}

/// Store an object to MCP storage
pub async fn store_to_mcp<T: serde::Serialize + Send + Sync>(
    mcp_server: &Arc<LibraMcpServer>,
    object_type: &str,
    object_id: &str,
    object: &T,
    debug: bool,
) {
    if object_id.is_empty() {
        eprintln!("[WARN] Refusing to store {object_type} with empty object id");
        return;
    }
    if let Some(storage) = &mcp_server.storage {
        match storage.put_json(object).await {
            Ok(hash) => {
                if let Err(e) =
                    append_history_hash_if_changed(mcp_server, object_type, object_id, hash).await
                {
                    eprintln!("[WARN] {e}");
                }
                if debug {
                    eprintln!("[DEBUG] Stored {object_type} {object_id} to MCP (hash: {hash})");
                }
            }
            Err(e) => {
                eprintln!("[WARN] Failed to store {object_type} to MCP: {e}");
            }
        }
    } else {
        eprintln!("[WARN] MCP storage not available");
    }
}

pub async fn execute(args: AgentCodexArgs) -> anyhow::Result<()> {
    // Initialize MCP server first
    let working_dir = PathBuf::from(&args.cwd);
    let mcp_server = init_mcp_server(&working_dir).await;
    let history_recorder = Arc::new(HistoryRecorder::new(mcp_server.clone(), args.debug));
    let history_writer = Arc::new(HistoryWriter::new(mcp_server.clone(), args.debug));
    println!("MCP server initialized.");

    println!("Connecting to Codex at {}...", args.url);

    let (ws_stream, _) = connect_async(args.url.as_str())
        .await
        .map_err(|e| anyhow::anyhow!("failed to connect to Codex at {}: {}", args.url, e))?;

    println!("Connected to Codex!");
    println!("Initializing...");
    if args.plan_mode {
        println!("Plan Mode: enabled (plan required before execution)");
    }

    let (mut write, read) = ws_stream.split();

    // Channel for sending messages
    let (tx, mut rx) = mpsc::channel::<String>(100);

    // Channel for approval requests (reader -> main loop)
    let (approval_tx, mut approval_rx) =
        mpsc::channel::<(serde_json::Value, tokio::sync::oneshot::Sender<bool>)>(10);

    // Shared state
    let mut thread_id = String::new();
    let responses: Arc<Mutex<std::collections::HashMap<u64, serde_json::Value>>> =
        Arc::new(Mutex::new(std::collections::HashMap::new()));
    let notifies: Arc<Mutex<std::collections::HashMap<u64, Arc<tokio::sync::Notify>>>> =
        Arc::new(Mutex::new(std::collections::HashMap::new()));

    // Session data storage (for MCP query)
    let session: Arc<Mutex<CodexSession>> = Arc::new(Mutex::new(CodexSession::new()));
    if let Some(mut session_guard) = lock_or_warn(&session, "init session") {
        session_guard.debug = args.debug;
    } else {
        return Err(anyhow::anyhow!("failed to initialize session state"));
    }
    let history_reader = HistoryReader::new(mcp_server.clone());
    let rebuild = history_reader.rebuild_view().await;
    if let Some(mut session_guard) = lock_or_warn(&session, "rebuild session from history") {
        let mut latest_plan_status: std::collections::HashMap<
            String,
            (chrono::DateTime<Utc>, PlanStatus),
        > = std::collections::HashMap::new();
        for event in &rebuild.plan_step_events {
            let status = plan_status_from_event(&event.status);
            let entry = latest_plan_status
                .entry(event.plan_id.clone())
                .or_insert((event.at, status.clone()));
            if event.at >= entry.0 {
                *entry = (event.at, status);
            }
        }

        let mut latest_task_status: std::collections::HashMap<
            String,
            (chrono::DateTime<Utc>, TaskStatus),
        > = std::collections::HashMap::new();
        for event in &rebuild.task_events {
            let status = task_status_from_event(&event.status);
            let entry = latest_task_status
                .entry(event.task_id.clone())
                .or_insert((event.at, status.clone()));
            if event.at >= entry.0 {
                *entry = (event.at, status);
            }
        }

        let mut latest_run_status: std::collections::HashMap<
            String,
            (chrono::DateTime<Utc>, RunStatus),
        > = std::collections::HashMap::new();
        let mut latest_run_terminal_at: std::collections::HashMap<String, chrono::DateTime<Utc>> =
            std::collections::HashMap::new();
        for event in &rebuild.run_events {
            let status = run_status_from_event(&event.status);
            let entry = latest_run_status
                .entry(event.run_id.clone())
                .or_insert((event.at, status.clone()));
            if event.at >= entry.0 {
                *entry = (event.at, status.clone());
            }
            if matches!(status, RunStatus::Completed | RunStatus::Failed) {
                latest_run_terminal_at.insert(event.run_id.clone(), event.at);
            }
        }

        let mut patchset_declined: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for decision in &rebuild.decisions {
            if !decision.approved
                && let Some(patchset_id) = decision.chosen_patchset_id.as_ref()
            {
                patchset_declined.insert(patchset_id.clone());
            }
        }

        if !rebuild.thread.thread_id.is_empty() {
            session_guard.thread.id = rebuild.thread.thread_id.clone();
        }
        session_guard.thread.current_turn_id = rebuild.scheduler.active_run_id.clone();
        session_guard.thread.status = if rebuild.scheduler.active_run_id.is_some() {
            ThreadStatus::Running
        } else if !rebuild.thread.thread_id.is_empty() {
            ThreadStatus::Completed
        } else {
            ThreadStatus::Pending
        };
        session_guard.intents = rebuild
            .thread
            .intents
            .values()
            .map(|i| Intent {
                id: i.id.clone(),
                content: i.content.clone(),
                thread_id: i.thread_id.clone(),
                created_at: i.created_at,
            })
            .collect();
        session_guard.plans = rebuild
            .thread
            .plans
            .values()
            .map(|p| Plan {
                id: p.id.clone(),
                text: p.step_text.clone(),
                intent_id: p.intent_id.clone(),
                thread_id: p.thread_id.clone(),
                turn_id: p.turn_id.clone(),
                status: latest_plan_status
                    .get(&p.id)
                    .map(|(_, status)| status.clone())
                    .unwrap_or(PlanStatus::Pending),
                created_at: p.created_at,
            })
            .collect();
        session_guard.tasks = rebuild
            .thread
            .tasks
            .values()
            .map(|t| Task {
                id: t.id.clone(),
                tool_name: t.title.clone(),
                plan_id: t.plan_id.clone(),
                thread_id: t.thread_id.clone(),
                turn_id: t.turn_id.clone(),
                status: latest_task_status
                    .get(&t.id)
                    .map(|(_, status)| status.clone())
                    .unwrap_or(TaskStatus::Pending),
                created_at: t.created_at,
            })
            .collect();
        session_guard.runs = rebuild
            .thread
            .runs
            .values()
            .map(|r| Run {
                id: r.id.clone(),
                thread_id: r.thread_id.clone(),
                status: latest_run_status
                    .get(&r.id)
                    .map(|(_, status)| status.clone())
                    .unwrap_or(RunStatus::Pending),
                started_at: r.started_at,
                completed_at: latest_run_terminal_at.get(&r.id).copied(),
            })
            .collect();
        session_guard.tool_invocations = rebuild.tool_invocations.clone();
        session_guard.patchsets = rebuild
            .thread
            .patchsets
            .values()
            .map(|p| PatchSet {
                id: p.id.clone(),
                run_id: p.run_id.clone(),
                thread_id: p.thread_id.clone(),
                changes: p.changes.clone(),
                status: if patchset_declined.contains(&p.id) {
                    PatchStatus::Declined
                } else {
                    p.status.clone()
                },
                created_at: p.created_at,
            })
            .collect();
    }

    // Spawn writer task
    let _write_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                Some(msg) = rx.recv() => {
                    if write.send(Message::Text(msg.into())).await.is_err() {
                        break;
                    }
                }
                _ = tokio::time::sleep(tokio::time::Duration::from_secs(1)) => {}
            }
        }
    });

    // Spawn reader task
    let responses_clone = responses.clone();
    let notifies_clone = notifies.clone();
    let tx_clone = tx.clone();
    let approval_tx_clone = approval_tx.clone();
    let approval_mode = Arc::new(Mutex::new(args.approval.clone()));
    let approval_mode_clone = approval_mode.clone();
    let approval_mode_for_turn = approval_mode.clone();
    let debug_mode = args.debug;
    let session_clone = session.clone();
    let mcp_server_clone = mcp_server.clone();
    let history_recorder_clone = history_recorder.clone();
    let history_writer_clone = history_writer.clone();
    let model_for_run = args.model.clone();
    let model_provider_for_run = args.model_provider.clone();
    let service_tier_for_run = args.service_tier.clone();
    let personality_for_run = args.personality.clone();
    let default_command_cwd = args.cwd.clone();
    let _reader_task = tokio::spawn(async move {
        let mut read = read;
        #[allow(clippy::while_let_loop)]
        loop {
            match read.next().await {
                Some(Ok(Message::Text(text))) => {
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                        // Distinguish response vs request: responses have result/error, requests have method
                        // Both can have id, so we must check for result/error vs method
                        let has_result_or_error =
                            json.get("result").is_some() || json.get("error").is_some();
                        let has_method = json.get("method").is_some();

                        // Store response if has id AND (result OR error) - but NOT if it has method (that's a request)
                        if let Some(id_val) = json.get("id") {
                            if let Some(id) = id_val.as_u64() {
                                // Only store as response if it has result/error but no method
                                if has_result_or_error && !has_method {
                                    if let Some(mut resp) =
                                        lock_or_warn(&responses_clone, "store response")
                                    {
                                        resp.insert(id, json.clone());
                                    }
                                    if let Some(notifies_guard) =
                                        lock_or_warn(&notifies_clone, "notify response waiter")
                                        && let Some(notify) = notifies_guard.get(&id)
                                    {
                                        let notify: &tokio::sync::Notify = notify.as_ref();
                                        notify.notify_waiters();
                                    }
                                }
                                // If it has method, it's a request (not a response) - skip storing
                            }
                        }
                        // Handle notifications (messages without id, or requests with method)
                        else if let Some(method) = json.get("method") {
                            let method_str = method.as_str().unwrap_or("");

                            // Debug: print all method names
                            // eprintln!("[DEBUG] Received method: {}", method_str);

                            // Handle all notifications based on method name
                            // See schema/ServerNotification.json for full list
                            let mk = MethodKind::from(method_str);
                            // Filter out truly noisy notifications
                            let _is_noise = matches!(mk, MethodKind::TokenUsageUpdated);

                            // Extract and print useful info based on notification type
                            if let Some(params_val) = json.get("params") {
                                let params = params_val.clone();
                                // Show hierarchical flow: Thread → Turn → Plan → Item → Detail
                                if matches!(mk, MethodKind::ThreadStarted) {
                                    let thread_id = parse_params::<ThreadStartedParams>(&params)
                                        .map(|p| p.thread.thread_id)
                                        .or_else(|| {
                                            params
                                                .get("thread")
                                                .and_then(|t| t.get("id"))
                                                .and_then(|t| t.as_str())
                                                .map(String::from)
                                        })
                                        .or_else(|| {
                                            params
                                                .get("thread")
                                                .and_then(|t| t.get("threadId"))
                                                .and_then(|t| t.as_str())
                                                .map(String::from)
                                        })
                                        .or_else(|| {
                                            params
                                                .get("threadId")
                                                .and_then(|t| t.as_str())
                                                .map(String::from)
                                        })
                                        .or_else(|| {
                                            params
                                                .get("thread_id")
                                                .and_then(|t| t.as_str())
                                                .map(String::from)
                                        })
                                        .unwrap_or_default();
                                    println!(
                                        "
=== New Thread: {} ===",
                                        &thread_id[..8.min(thread_id.len())]
                                    );

                                    let thread = CodexThread {
                                        id: thread_id.to_string(),
                                        status: ThreadStatus::Running,
                                        name: None,
                                        current_turn_id: None,
                                        created_at: Utc::now(),
                                        updated_at: Utc::now(),
                                    };
                                    let thread_id_for_mcp = thread_id.to_string();
                                    let thread_for_mcp = thread.clone();
                                    if let Some(mut session) =
                                        lock_or_warn(&session_clone, "thread started update")
                                    {
                                        session.update_thread(thread);
                                    }

                                    let mcp_server_for_thread = mcp_server_clone.clone();
                                    tokio::spawn(async move {
                                        store_to_mcp(
                                            &mcp_server_for_thread,
                                            "thread",
                                            &thread_id_for_mcp,
                                            &thread_for_mcp,
                                            debug_mode,
                                        )
                                        .await;
                                    });
                                } else if matches!(mk, MethodKind::ThreadStatusChanged) {
                                    let (thread_id, status) = if let Some(p) =
                                        parse_params::<ThreadStatusChangedParams>(&params)
                                    {
                                        (p.thread_id, p.status)
                                    } else {
                                        (
                                            params
                                                .get("threadId")
                                                .and_then(|t| t.as_str())
                                                .unwrap_or("")
                                                .to_string(),
                                            params
                                                .get("status")
                                                .and_then(|s| s.as_str())
                                                .unwrap_or("")
                                                .to_string(),
                                        )
                                    };

                                    let new_status = match status.as_str() {
                                        "pending" => ThreadStatus::Pending,
                                        "running" => ThreadStatus::Running,
                                        "completed" => ThreadStatus::Completed,
                                        "archived" => ThreadStatus::Archived,
                                        "closed" => ThreadStatus::Closed,
                                        _ => ThreadStatus::Running,
                                    };

                                    if debug_mode {
                                        eprintln!(
                                            "[DEBUG] Thread status changed: {} -> {:?}",
                                            thread_id, new_status
                                        );
                                    }

                                    if let Some(mut session) =
                                        lock_or_warn(&session_clone, "thread status update")
                                    {
                                        session.thread.status = new_status;
                                        session.thread.updated_at = Utc::now();
                                    }
                                } else if matches!(mk, MethodKind::ThreadNameUpdated) {
                                    let (thread_id, name) = if let Some(p) =
                                        parse_params::<ThreadNameUpdatedParams>(&params)
                                    {
                                        (p.thread_id, p.name)
                                    } else {
                                        (
                                            params
                                                .get("threadId")
                                                .and_then(|t| t.as_str())
                                                .unwrap_or("")
                                                .to_string(),
                                            params
                                                .get("name")
                                                .and_then(|n| n.as_str())
                                                .map(String::from),
                                        )
                                    };

                                    if debug_mode {
                                        eprintln!(
                                            "[DEBUG] Thread name updated: {} -> {:?}",
                                            thread_id, name
                                        );
                                    }

                                    if let Some(mut session) =
                                        lock_or_warn(&session_clone, "thread name update")
                                    {
                                        session.thread.name = name;
                                        session.thread.updated_at = Utc::now();
                                    }
                                } else if matches!(mk, MethodKind::ThreadArchived) {
                                    let thread_id = parse_params::<ThreadArchivedParams>(&params)
                                        .map(|p| p.thread_id)
                                        .or_else(|| {
                                            params
                                                .get("threadId")
                                                .and_then(|t| t.as_str())
                                                .map(String::from)
                                        })
                                        .unwrap_or_default();

                                    if debug_mode {
                                        eprintln!("[DEBUG] Thread archived: {}", thread_id);
                                    }

                                    if let Some(mut session) =
                                        lock_or_warn(&session_clone, "thread archived update")
                                    {
                                        session.thread.status = ThreadStatus::Archived;
                                        session.thread.updated_at = Utc::now();
                                    }
                                } else if matches!(mk, MethodKind::ThreadCompacted) {
                                    let (thread_id, turn_id) = if let Some(p) =
                                        parse_params::<ThreadCompactedParams>(&params)
                                    {
                                        (p.thread_id, p.turn_id)
                                    } else {
                                        (
                                            params
                                                .get("threadId")
                                                .and_then(|t| t.as_str())
                                                .unwrap_or("")
                                                .to_string(),
                                            params
                                                .get("turnId")
                                                .and_then(|t| t.as_str())
                                                .unwrap_or("")
                                                .to_string(),
                                        )
                                    };
                                    println!("Context compacted for thread {}", thread_id);

                                    let snapshot = ContextSnapshot {
                                        id: format!("context_{}", turn_id),
                                        thread_id: thread_id.clone(),
                                        run_id: Some(turn_id.clone()),
                                        created_at: Utc::now(),
                                        data: serde_json::json!({}),
                                    };
                                    let snapshot_id = snapshot.id.clone();
                                    let snapshot_for_mcp = snapshot.clone();
                                    let mcp_server_for_snapshot = mcp_server_clone.clone();
                                    tokio::spawn(async move {
                                        store_to_mcp(
                                            &mcp_server_for_snapshot,
                                            "context_snapshot",
                                            &snapshot_id,
                                            &snapshot_for_mcp,
                                            debug_mode,
                                        )
                                        .await;
                                    });
                                } else if matches!(mk, MethodKind::ThreadClosed) {
                                    let thread_id = parse_params::<ThreadClosedParams>(&params)
                                        .map(|p| p.thread_id)
                                        .or_else(|| {
                                            params
                                                .get("threadId")
                                                .and_then(|t| t.as_str())
                                                .map(String::from)
                                        })
                                        .unwrap_or_default();

                                    if debug_mode {
                                        eprintln!("[DEBUG] Thread closed: {}", thread_id);
                                    }

                                    if let Some(mut session) =
                                        lock_or_warn(&session_clone, "thread closed update")
                                    {
                                        session.thread.status = ThreadStatus::Closed;
                                        session.thread.updated_at = Utc::now();
                                    }
                                } else if matches!(mk, MethodKind::TurnStarted) {
                                    let (turn_id, thread_id) = if let Some(p) =
                                        parse_params::<TurnStartedParams>(&params)
                                    {
                                        (p.turn.id, p.thread_id)
                                    } else {
                                        (
                                            params
                                                .get("turn")
                                                .and_then(|t| t.get("id"))
                                                .and_then(|t| t.as_str())
                                                .unwrap_or("")
                                                .to_string(),
                                            params
                                                .get("threadId")
                                                .and_then(|t| t.as_str())
                                                .unwrap_or("")
                                                .to_string(),
                                        )
                                    };
                                    println!(
                                        "
--- Turn started: {} (thread: {}) ---",
                                        &turn_id[..8.min(turn_id.len())],
                                        &thread_id[..8.min(thread_id.len())]
                                    );

                                    let run = Run {
                                        id: turn_id.to_string(),
                                        thread_id: thread_id.to_string(),
                                        status: RunStatus::InProgress,
                                        started_at: Utc::now(),
                                        completed_at: None,
                                    };
                                    let run_snapshot = RunSnapshot {
                                        id: turn_id.to_string(),
                                        thread_id: thread_id.to_string(),
                                        plan_id: None,
                                        task_id: None,
                                        started_at: Utc::now(),
                                    };
                                    let run_event = RunEvent {
                                        id: format!("run_event_{}", turn_id),
                                        run_id: turn_id.to_string(),
                                        status: "in_progress".to_string(),
                                        at: Utc::now(),
                                        error: None,
                                    };
                                    let provenance = ProvenanceSnapshot {
                                        id: format!("prov_{}", turn_id),
                                        run_id: turn_id.to_string(),
                                        model: model_for_run.clone(),
                                        provider: model_provider_for_run.clone(),
                                        parameters: serde_json::json!({
                                            "service_tier": service_tier_for_run.clone(),
                                            "personality": personality_for_run.clone(),
                                        }),
                                        created_at: Utc::now(),
                                    };
                                    let history_writer = history_writer_clone.clone();
                                    let run_id_for_write = turn_id.to_string();
                                    tokio::spawn(async move {
                                        history_writer
                                            .write("run_snapshot", &run_id_for_write, &run_snapshot)
                                            .await;
                                        history_writer
                                            .write("run_event", &run_event.id, &run_event)
                                            .await;
                                        history_writer
                                            .write(
                                                "provenance_snapshot",
                                                &provenance.id,
                                                &provenance,
                                            )
                                            .await;
                                    });
                                    let run_id = turn_id.to_string();
                                    let run_for_mcp = run.clone();
                                    if let Some(mut session) =
                                        lock_or_warn(&session_clone, "turn started update")
                                    {
                                        session.add_run(run);
                                        session.thread.current_turn_id = Some(turn_id.to_string());
                                        session.thread.status = ThreadStatus::Running;
                                        session.thread.updated_at = Utc::now();
                                    }

                                    let mcp_server_for_run = mcp_server_clone.clone();
                                    let history = history_recorder_clone.clone();
                                    tokio::spawn(async move {
                                        store_to_mcp(
                                            &mcp_server_for_run,
                                            "run",
                                            &run_id,
                                            &run_for_mcp,
                                            debug_mode,
                                        )
                                        .await;
                                        history
                                            .event(
                                                history::EventKind::RunStatus,
                                                &run_id,
                                                "in_progress",
                                                serde_json::json!({"thread_id": thread_id}),
                                            )
                                            .await;
                                    });
                                } else if matches!(mk, MethodKind::TurnCompleted) {
                                    let turn_id = parse_params::<TurnCompletedParams>(&params)
                                        .map(|p| p.turn.id)
                                        .or_else(|| {
                                            params
                                                .get("turn")
                                                .and_then(|t| t.get("id"))
                                                .and_then(|t| t.as_str())
                                                .map(String::from)
                                        })
                                        .unwrap_or_default();
                                    if !turn_id.is_empty() {
                                        println!(
                                            "--- Turn completed: {} ---",
                                            &turn_id[..8.min(turn_id.len())]
                                        );

                                        let run_to_store = if let Some(mut session) =
                                            lock_or_warn(&session_clone, "turn completed update")
                                        {
                                            let run = session
                                                .runs
                                                .iter_mut()
                                                .find(|r| r.id == turn_id)
                                                .map(|run| {
                                                    run.status = RunStatus::Completed;
                                                    run.completed_at = Some(Utc::now());
                                                    run.clone()
                                                });
                                            session.thread.updated_at = Utc::now();
                                            run
                                        } else {
                                            None
                                        };

                                        if let Some(run) = run_to_store {
                                            let run_id = run.id.clone();
                                            let mcp_server_for_run = mcp_server_clone.clone();
                                            let history = history_recorder_clone.clone();
                                            let history_writer = history_writer_clone.clone();
                                            let run_event = RunEvent {
                                                id: format!("run_event_{}_completed", run_id),
                                                run_id: run_id.clone(),
                                                status: "completed".to_string(),
                                                at: Utc::now(),
                                                error: None,
                                            };
                                            if let Some(mut session) = lock_or_warn(
                                                &session_clone,
                                                "scheduler cleanup on run complete",
                                            ) && session.thread.current_turn_id.as_deref()
                                                == Some(&run_id)
                                            {
                                                session.thread.current_turn_id = None;
                                            }
                                            tokio::spawn(async move {
                                                store_to_mcp(
                                                    &mcp_server_for_run,
                                                    "run",
                                                    &run_id,
                                                    &run,
                                                    debug_mode,
                                                )
                                                .await;
                                                history
                                                    .event(
                                                        history::EventKind::RunStatus,
                                                        &run_id,
                                                        "completed",
                                                        serde_json::json!({}),
                                                    )
                                                    .await;
                                                history_writer
                                                    .write("run_event", &run_event.id, &run_event)
                                                    .await;
                                                let context_snapshot = ContextSnapshot {
                                                    id: format!("context_rc_{}", run_id),
                                                    thread_id: run.thread_id.clone(),
                                                    run_id: Some(run_id.clone()),
                                                    created_at: Utc::now(),
                                                    data: serde_json::json!({ "release_candidate": true }),
                                                };
                                                history_writer
                                                    .write(
                                                        "context_snapshot",
                                                        &context_snapshot.id,
                                                        &context_snapshot,
                                                    )
                                                    .await;
                                            });
                                        }
                                    } else {
                                        println!("--- Turn completed ---");
                                    }
                                } else if matches!(mk, MethodKind::TokenUsageUpdated) {
                                    if let Some(p) =
                                        parse_params::<ThreadTokenUsageUpdatedParams>(&params)
                                    {
                                        let last = TokenUsage {
                                            cached_input_tokens: Some(
                                                p.token_usage.last.cached_input_tokens,
                                            ),
                                            input_tokens: Some(p.token_usage.last.input_tokens),
                                            output_tokens: Some(p.token_usage.last.output_tokens),
                                            reasoning_output_tokens: Some(
                                                p.token_usage.last.reasoning_output_tokens,
                                            ),
                                            total_tokens: Some(p.token_usage.last.total_tokens),
                                        };
                                        let total = TokenUsage {
                                            cached_input_tokens: Some(
                                                p.token_usage.total.cached_input_tokens,
                                            ),
                                            input_tokens: Some(p.token_usage.total.input_tokens),
                                            output_tokens: Some(p.token_usage.total.output_tokens),
                                            reasoning_output_tokens: Some(
                                                p.token_usage.total.reasoning_output_tokens,
                                            ),
                                            total_tokens: Some(p.token_usage.total.total_tokens),
                                        };
                                        let usage = TurnTokenUsage {
                                            thread_id: p.thread_id.clone(),
                                            turn_id: p.turn_id.clone(),
                                            last,
                                            total,
                                            model_context_window: p
                                                .token_usage
                                                .model_context_window,
                                            updated_at: Utc::now(),
                                        };
                                        if let Some(mut session) =
                                            lock_or_warn(&session_clone, "token usage update")
                                        {
                                            session.add_token_usage(usage);
                                        }

                                        let run_usage = RunUsage {
                                            run_id: p.turn_id.clone(),
                                            thread_id: p.thread_id.clone(),
                                            at: Utc::now(),
                                            usage: serde_json::json!(p.token_usage),
                                        };
                                        let history_writer = history_writer_clone.clone();
                                        let run_usage_id = format!(
                                            "run_usage_{}_{}",
                                            p.turn_id.clone(),
                                            Utc::now().timestamp_millis()
                                        );
                                        tokio::spawn(async move {
                                            history_writer
                                                .write("run_usage", &run_usage_id, &run_usage)
                                                .await;
                                        });
                                    }
                                } else if matches!(mk, MethodKind::PlanUpdated) {
                                    // params: { plan: [...], threadId, turnId, explanation? }
                                    let (thread_id, turn_id, plan_steps, explanation) =
                                        if let Some(p) =
                                            parse_params::<TurnPlanUpdatedParams>(&params)
                                        {
                                            (p.thread_id, p.turn_id, p.plan, p.explanation)
                                        } else {
                                            let thread_id = params
                                                .get("threadId")
                                                .and_then(|t| t.as_str())
                                                .unwrap_or("")
                                                .to_string();
                                            let turn_id = params
                                                .get("turnId")
                                                .and_then(|t| t.as_str())
                                                .unwrap_or("")
                                                .to_string();
                                            let explanation = params
                                                .get("explanation")
                                                .and_then(|e| e.as_str())
                                                .map(String::from);
                                            let plan_steps = params
                                                .get("plan")
                                                .and_then(|p| p.as_array())
                                                .map(|arr| {
                                                    arr.iter()
                                                        .map(|item| TurnPlanStep {
                                                            status: item
                                                                .get("status")
                                                                .and_then(|s| s.as_str())
                                                                .unwrap_or("unknown")
                                                                .to_string(),
                                                            step: item
                                                                .get("step")
                                                                .and_then(|s| s.as_str())
                                                                .unwrap_or("")
                                                                .to_string(),
                                                        })
                                                        .collect()
                                                })
                                                .unwrap_or_default();
                                            (thread_id, turn_id, plan_steps, explanation)
                                        };

                                    if !plan_steps.is_empty() {
                                        let plan_text =
                                            build_plan_text(explanation.as_ref(), &plan_steps);
                                        let plan_status = aggregate_plan_status(&plan_steps);
                                        let plan_now = Utc::now();
                                        let (
                                            intent_id_for_plan,
                                            plan_id,
                                            plan_created_at,
                                            parent_plan_ids,
                                            persisted_plan,
                                            persisted_plan_steps,
                                            persisted_tasks,
                                            persisted_plan_step_events,
                                            persisted_task_events,
                                        ) = if let Some(mut session) =
                                            lock_or_warn(&session_clone, "plan update")
                                        {
                                            let intent_id_for_plan =
                                                latest_thread_intent_id(&session, &thread_id, None);
                                            let latest_plan = session
                                                .plans
                                                .iter()
                                                .filter(|plan| {
                                                    plan.thread_id == thread_id
                                                        && plan.intent_id.as_deref()
                                                            == intent_id_for_plan.as_deref()
                                                })
                                                .max_by_key(|plan| plan.created_at)
                                                .cloned();
                                            let reuse_latest_plan =
                                                latest_plan.as_ref().is_some_and(|plan| {
                                                    plan.turn_id.as_deref()
                                                        == Some(turn_id.as_str())
                                                        && plan.text == plan_text
                                                });
                                            let plan_id = if reuse_latest_plan {
                                                latest_plan
                                                    .as_ref()
                                                    .map(|plan| plan.id.clone())
                                                    .unwrap_or_default()
                                            } else {
                                                format!(
                                                    "plan_{}_{}",
                                                    turn_id,
                                                    plan_now.timestamp_millis()
                                                )
                                            };
                                            let plan_created_at = latest_plan
                                                .as_ref()
                                                .filter(|_| reuse_latest_plan)
                                                .map(|plan| plan.created_at)
                                                .unwrap_or(plan_now);
                                            let parent_plan_ids = if reuse_latest_plan {
                                                Vec::new()
                                            } else {
                                                latest_plan
                                                    .as_ref()
                                                    .map(|plan| vec![plan.id.clone()])
                                                    .unwrap_or_default()
                                            };

                                            let plan = Plan {
                                                id: plan_id.clone(),
                                                text: plan_text.clone(),
                                                intent_id: intent_id_for_plan.clone(),
                                                thread_id: thread_id.to_string(),
                                                turn_id: Some(turn_id.to_string()),
                                                status: plan_status.clone(),
                                                created_at: plan_created_at,
                                            };
                                            let plan_snapshot = PlanSnapshot {
                                                id: plan_id.clone(),
                                                thread_id: thread_id.to_string(),
                                                intent_id: plan.intent_id.clone(),
                                                turn_id: Some(turn_id.to_string()),
                                                step_text: plan_text.clone(),
                                                parents: parent_plan_ids.clone(),
                                                context_frames: Vec::new(),
                                                created_at: plan_created_at,
                                            };

                                            let mut plan_step_snapshots =
                                                Vec::with_capacity(plan_steps.len());
                                            let mut task_snapshots =
                                                Vec::with_capacity(plan_steps.len());
                                            let mut plan_step_events = Vec::new();
                                            let mut task_events = Vec::new();

                                            for (ordinal, item) in plan_steps.iter().enumerate() {
                                                let normalized_status =
                                                    normalize_plan_step_status(&item.status);
                                                let step_id =
                                                    format!("{}_step_{}", plan_id, ordinal);
                                                let task_id =
                                                    format!("task_{}_{}", plan_id, ordinal);
                                                let previous_task_status = session
                                                    .tasks
                                                    .iter()
                                                    .find(|task| task.id == task_id)
                                                    .map(|task| task.status.clone());
                                                let task_status =
                                                    task_status_from_plan_step(&item.status);

                                                let plan_step_snapshot = PlanStepSnapshot {
                                                    id: step_id.clone(),
                                                    plan_id: plan_id.clone(),
                                                    text: item.step.clone(),
                                                    ordinal: ordinal as i64,
                                                    created_at: plan_created_at,
                                                };
                                                let task_snapshot = TaskSnapshot {
                                                    id: task_id.clone(),
                                                    thread_id: thread_id.to_string(),
                                                    plan_id: Some(plan_id.clone()),
                                                    intent_id: intent_id_for_plan.clone(),
                                                    turn_id: Some(turn_id.to_string()),
                                                    title: Some(item.step.clone()),
                                                    parent_task_id: None,
                                                    origin_step_id: Some(step_id.clone()),
                                                    dependencies: Vec::new(),
                                                    created_at: plan_created_at,
                                                };
                                                let task = Task {
                                                    id: task_id.clone(),
                                                    tool_name: Some(item.step.clone()),
                                                    plan_id: Some(plan_id.clone()),
                                                    thread_id: thread_id.to_string(),
                                                    turn_id: Some(turn_id.to_string()),
                                                    status: task_status.clone(),
                                                    created_at: plan_created_at,
                                                };

                                                if previous_task_status.as_ref()
                                                    != Some(&task_status)
                                                {
                                                    plan_step_events.push(PlanStepEvent {
                                                        id: format!(
                                                            "plan_step_event_{}_{}_{}",
                                                            plan_id,
                                                            ordinal,
                                                            plan_now.timestamp_millis()
                                                        ),
                                                        plan_id: plan_id.clone(),
                                                        step_id: step_id.clone(),
                                                        status: normalized_status.to_string(),
                                                        at: plan_now,
                                                        run_id: Some(turn_id.to_string()),
                                                    });
                                                    if normalized_status != "pending" {
                                                        task_events.push(TaskEvent {
                                                            id: format!(
                                                                "task_event_{}_{}_{}",
                                                                task_id,
                                                                normalized_status,
                                                                plan_now.timestamp_millis()
                                                            ),
                                                            task_id: task_id.clone(),
                                                            status: normalized_status.to_string(),
                                                            at: plan_now,
                                                            run_id: Some(turn_id.to_string()),
                                                        });
                                                    }
                                                }

                                                session.add_task(task);
                                                plan_step_snapshots.push(plan_step_snapshot);
                                                task_snapshots.push(task_snapshot);
                                            }

                                            session.add_plan(plan.clone());

                                            (
                                                intent_id_for_plan,
                                                plan_id,
                                                plan_created_at,
                                                parent_plan_ids,
                                                plan_snapshot,
                                                plan_step_snapshots,
                                                task_snapshots,
                                                plan_step_events,
                                                task_events,
                                            )
                                        } else {
                                            continue;
                                        };

                                        println!("\nPlan Updated:");
                                        if let Some(exp) = explanation.as_ref() {
                                            println!("  Explanation: {}", exp);
                                        }
                                        for item in plan_steps.iter() {
                                            let status_string =
                                                normalize_plan_step_status(&item.status);
                                            let step_string = item.step.as_str();
                                            let marker = match status_string {
                                                "completed" => "[x]",
                                                "in_progress" => "[>]",
                                                _ => "[ ]",
                                            };
                                            println!("  {} {}", marker, step_string);
                                        }

                                        let history_writer = history_writer_clone.clone();
                                        let plan_snapshot_id = plan_id.clone();
                                        let mcp_server_for_plan = mcp_server_clone.clone();
                                        let history = history_recorder_clone.clone();
                                        let plan_for_mcp = Plan {
                                            id: plan_id.clone(),
                                            text: plan_text.clone(),
                                            intent_id: intent_id_for_plan.clone(),
                                            thread_id: thread_id.to_string(),
                                            turn_id: Some(turn_id.to_string()),
                                            status: plan_status,
                                            created_at: plan_created_at,
                                        };
                                        tokio::spawn(async move {
                                            history_writer
                                                .write(
                                                    "plan_snapshot",
                                                    &plan_snapshot_id,
                                                    &persisted_plan,
                                                )
                                                .await;
                                            for step in persisted_plan_steps {
                                                history_writer
                                                    .write("plan_step_snapshot", &step.id, &step)
                                                    .await;
                                            }
                                            for task in persisted_tasks {
                                                history_writer
                                                    .write("task_snapshot", &task.id, &task)
                                                    .await;
                                            }
                                            for event in persisted_plan_step_events {
                                                history_writer
                                                    .write("plan_step_event", &event.id, &event)
                                                    .await;
                                            }
                                            for event in persisted_task_events {
                                                history_writer
                                                    .write("task_event", &event.id, &event)
                                                    .await;
                                            }
                                            store_to_mcp(
                                                &mcp_server_for_plan,
                                                "plan",
                                                &plan_id,
                                                &plan_for_mcp,
                                                debug_mode,
                                            )
                                            .await;
                                            history
                                                .event(
                                                    history::EventKind::PlanStepStatus,
                                                    &plan_id,
                                                    match aggregate_plan_status(&plan_steps) {
                                                        PlanStatus::Completed => "completed",
                                                        PlanStatus::InProgress => "in_progress",
                                                        PlanStatus::Pending => "pending",
                                                    },
                                                    serde_json::json!({
                                                        "step_count": plan_steps.len(),
                                                        "parents": parent_plan_ids
                                                    }),
                                                )
                                                .await;
                                        });
                                    }
                                } else if matches!(mk, MethodKind::PlanDelta) {
                                    if let Some(p) =
                                        parse_params::<DeltaNotificationParams>(&params)
                                    {
                                        if let Some(mut session) =
                                            lock_or_warn(&session_clone, "plan delta update")
                                        {
                                            if let Some(plan) = session
                                                .plans
                                                .iter_mut()
                                                .find(|pl| pl.id == p.item_id)
                                            {
                                                plan.text.push_str(&p.delta);
                                            } else {
                                                let plan = Plan {
                                                    id: p.item_id.clone(),
                                                    text: p.delta.clone(),
                                                    intent_id: None,
                                                    thread_id: p.thread_id.clone(),
                                                    turn_id: Some(p.turn_id.clone()),
                                                    status: PlanStatus::InProgress,
                                                    created_at: Utc::now(),
                                                };
                                                session.add_plan(plan);
                                            }
                                        }
                                        if debug_mode {
                                            eprintln!("[DEBUG] plan delta {} bytes", p.delta.len());
                                        }
                                    }
                                } else if matches!(mk, MethodKind::AgentMessageDelta) {
                                    if let Some(p) =
                                        parse_params::<DeltaNotificationParams>(&params)
                                    {
                                        if let Some(mut session) = lock_or_warn(
                                            &session_clone,
                                            "agent message delta update",
                                        ) {
                                            if let Some(msg) = session
                                                .agent_messages
                                                .iter_mut()
                                                .find(|m| m.id == p.item_id)
                                            {
                                                msg.content.push_str(&p.delta);
                                            } else {
                                                let msg = AgentMessage {
                                                    id: p.item_id.clone(),
                                                    run_id: p.turn_id.clone(),
                                                    thread_id: p.thread_id.clone(),
                                                    content: p.delta.clone(),
                                                    created_at: Utc::now(),
                                                };
                                                session.add_agent_message(msg);
                                            }
                                        }
                                        print!("{}", p.delta);
                                    }
                                } else if matches!(mk, MethodKind::CommandExecutionOutputDelta) {
                                    if let Some(p) =
                                        parse_params::<DeltaNotificationParams>(&params)
                                    {
                                        if let Some(mut session) = lock_or_warn(
                                            &session_clone,
                                            "command output delta update",
                                        ) && let Some(invocation) = session
                                            .tool_invocations
                                            .iter_mut()
                                            .find(|i| i.id == p.item_id)
                                        {
                                            let mut output = invocation
                                                .result
                                                .as_ref()
                                                .and_then(|v| v.get("output"))
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("")
                                                .to_string();
                                            output.push_str(&p.delta);
                                            invocation.result =
                                                Some(serde_json::json!({ "output": output }));
                                        }
                                        print!("{}", p.delta);
                                    }
                                } else if matches!(mk, MethodKind::FileChangeOutputDelta) {
                                    if let Some(p) =
                                        parse_params::<DeltaNotificationParams>(&params)
                                    {
                                        if let Some(mut session) =
                                            lock_or_warn(&session_clone, "file change delta update")
                                        {
                                            if let Some(patchset) = session
                                                .patchsets
                                                .iter_mut()
                                                .find(|ps| ps.id == p.item_id)
                                            {
                                                if let Some(change) = patchset
                                                    .changes
                                                    .iter_mut()
                                                    .find(|c| c.path == "(stream)")
                                                {
                                                    change.diff.push_str(&p.delta);
                                                } else {
                                                    patchset.changes.push(FileChange {
                                                        path: "(stream)".to_string(),
                                                        diff: p.delta.clone(),
                                                        change_type: "delta".to_string(),
                                                    });
                                                }
                                            } else {
                                                let patchset = PatchSet {
                                                    id: p.item_id.clone(),
                                                    run_id: p.turn_id.clone(),
                                                    thread_id: p.thread_id.clone(),
                                                    changes: vec![FileChange {
                                                        path: "(stream)".to_string(),
                                                        diff: p.delta.clone(),
                                                        change_type: "delta".to_string(),
                                                    }],
                                                    status: PatchStatus::InProgress,
                                                    created_at: Utc::now(),
                                                };
                                                session.add_patchset(patchset);
                                            }
                                        }
                                        print!("{}", p.delta);
                                    }
                                } else if matches!(mk, MethodKind::Initialized) {
                                    // Server initialized notification (after client sends initialize request)
                                    println!("[Codex] Server initialized");
                                } else if matches!(mk, MethodKind::TaskStarted) {
                                    // Task started - top level notification
                                    let thread_id = lock_or_warn(
                                        &session_clone,
                                        "thread lookup for task start",
                                    )
                                    .as_deref()
                                    .map(|session| extract_thread_id(&params, Some(session)))
                                    .unwrap_or_else(|| extract_thread_id(&params, None));
                                    let task_id = extract_task_id(&params);
                                    if task_id.is_empty() {
                                        eprintln!(
                                            "[WARN] TaskStarted notification missing task id: {}",
                                            params
                                        );
                                        continue;
                                    }
                                    let (
                                        existing_plan_id,
                                        existing_turn_id,
                                        intent_id_for_task,
                                        run_id_for_task_event,
                                    ) = if let Some(mut session) =
                                        lock_or_warn(&session_clone, "task started update")
                                    {
                                        let mut plan_id = None;
                                        let mut turn_id = None;
                                        if let Some(task) =
                                            session.tasks.iter_mut().find(|t| t.id == task_id)
                                        {
                                            task.status = TaskStatus::InProgress;
                                            plan_id = task.plan_id.clone();
                                            turn_id = task.turn_id.clone();
                                        }
                                        let run_id = session.thread.current_turn_id.clone();
                                        let intent_id = plan_id
                                            .as_ref()
                                            .and_then(|pid| {
                                                session
                                                    .plans
                                                    .iter()
                                                    .find(|plan| &plan.id == pid)
                                                    .and_then(|plan| plan.intent_id.clone())
                                            })
                                            .or_else(|| {
                                                latest_thread_intent_id(&session, &thread_id, None)
                                            });
                                        (plan_id, turn_id, intent_id, run_id)
                                    } else {
                                        (None, None, None, None)
                                    };
                                    let task_name = extract_task_name(&params);

                                    println!(
                                        "\n🚀 Task Started: {} (thread: {})",
                                        task_name,
                                        &thread_id[..8.min(thread_id.len())]
                                    );

                                    // Store Task (tool_name stores task name)
                                    let task = Task {
                                        id: task_id.clone(),
                                        tool_name: Some(task_name.clone()),
                                        plan_id: existing_plan_id.clone(),
                                        thread_id: thread_id.clone(),
                                        turn_id: existing_turn_id
                                            .clone()
                                            .or(run_id_for_task_event.clone()),
                                        status: TaskStatus::InProgress,
                                        created_at: Utc::now(),
                                    };
                                    let task_snapshot = TaskSnapshot {
                                        id: task_id.clone(),
                                        thread_id: thread_id.clone(),
                                        plan_id: existing_plan_id.clone(),
                                        intent_id: intent_id_for_task.clone(),
                                        turn_id: existing_turn_id
                                            .clone()
                                            .or(run_id_for_task_event.clone()),
                                        title: Some(task_name.clone()),
                                        parent_task_id: None,
                                        origin_step_id: existing_plan_id.clone(),
                                        dependencies: Vec::new(),
                                        created_at: Utc::now(),
                                    };
                                    let task_event = TaskEvent {
                                        id: format!("task_event_{}", task_id),
                                        task_id: task_id.clone(),
                                        status: "in_progress".to_string(),
                                        at: Utc::now(),
                                        run_id: run_id_for_task_event.clone(),
                                    };
                                    let history_writer = history_writer_clone.clone();
                                    let task_id_for_write = task_id.clone();
                                    tokio::spawn(async move {
                                        history_writer
                                            .write(
                                                "task_snapshot",
                                                &task_id_for_write,
                                                &task_snapshot,
                                            )
                                            .await;
                                        history_writer
                                            .write("task_event", &task_event.id, &task_event)
                                            .await;
                                    });
                                    let task_id_for_mcp = task_id.clone();
                                    let task_for_mcp = task.clone();

                                    if let Some(mut session) =
                                        lock_or_warn(&session_clone, "task started update")
                                    {
                                        session.add_task(task);
                                    }

                                    // Store to MCP in background
                                    let mcp_server_for_task = mcp_server_clone.clone();
                                    let history = history_recorder_clone.clone();
                                    tokio::spawn(async move {
                                        store_to_mcp(
                                            &mcp_server_for_task,
                                            "task",
                                            &task_id_for_mcp,
                                            &task_for_mcp,
                                            debug_mode,
                                        )
                                        .await;
                                        history
                                            .event(
                                                history::EventKind::TaskStatus,
                                                &task_id_for_mcp,
                                                "in_progress",
                                                serde_json::json!({"task_name": task_name}),
                                            )
                                            .await;
                                    });
                                } else if matches!(mk, MethodKind::TaskCompleted) {
                                    // Task completed - top level notification
                                    println!(
                                        "
Task Completed"
                                    );
                                    let task_id = extract_task_id(&params);
                                    if task_id.is_empty() {
                                        eprintln!(
                                            "[WARN] TaskCompleted notification missing task id: {}",
                                            params
                                        );
                                        continue;
                                    }
                                    let (intent_id_for_event, run_id_for_task_event) =
                                        if let Some(mut session) =
                                            lock_or_warn(&session_clone, "task completed update")
                                        {
                                            let mut plan_id = None;
                                            let mut run_id = session.thread.current_turn_id.clone();
                                            if let Some(task) =
                                                session.tasks.iter_mut().find(|t| t.id == task_id)
                                            {
                                                task.status = TaskStatus::Completed;
                                                plan_id = task.plan_id.clone();
                                                run_id = task.turn_id.clone().or(run_id);
                                            }
                                            let intent_id = plan_id
                                                .as_ref()
                                                .and_then(|pid| {
                                                    session
                                                        .plans
                                                        .iter()
                                                        .find(|plan| &plan.id == pid)
                                                        .and_then(|plan| plan.intent_id.clone())
                                                })
                                                .or_else(|| {
                                                    latest_thread_intent_id(
                                                        &session,
                                                        &session.thread.id,
                                                        None,
                                                    )
                                                })
                                                .unwrap_or_default();
                                            (intent_id, run_id)
                                        } else {
                                            (String::new(), None)
                                        };
                                    let task_event = TaskEvent {
                                        id: format!("task_event_completed_{}", task_id),
                                        task_id,
                                        status: "completed".to_string(),
                                        at: Utc::now(),
                                        run_id: run_id_for_task_event,
                                    };
                                    let intent_event = IntentEvent {
                                        id: format!(
                                            "intent_event_completed_{}",
                                            Utc::now().timestamp_millis()
                                        ),
                                        intent_id: intent_id_for_event.clone(),
                                        status: "completed".to_string(),
                                        at: Utc::now(),
                                        next_intent_id: None,
                                    };
                                    let history_writer = history_writer_clone.clone();
                                    tokio::spawn(async move {
                                        history_writer
                                            .write("task_event", &task_event.id, &task_event)
                                            .await;
                                        history_writer
                                            .write("intent_event", &intent_event.id, &intent_event)
                                            .await;
                                    });
                                }
                                // Handle item/started with owned data (avoids borrowing across async tasks)
                                // Handle item/started with owned data (avoids borrowing across async tasks)
                                // Handle item/started with owned data (avoids borrowing across async tasks)
                                if matches!(mk, MethodKind::ItemStarted) {
                                    if let Some(params_obj) = json.get("params").cloned() {
                                        let thread_id = params_obj
                                            .get("threadId")
                                            .and_then(|t| t.as_str())
                                            .unwrap_or("")
                                            .to_string();
                                        let turn_id = params_obj
                                            .get("turnId")
                                            .and_then(|t| t.as_str())
                                            .unwrap_or("")
                                            .to_string();
                                        let run_id = turn_id.clone();

                                        if let Some(item) = params_obj.get("item").cloned() {
                                            let item_type = item
                                                .get("type")
                                                .and_then(|t| t.as_str())
                                                .unwrap_or("")
                                                .to_string();
                                            let item_id = item
                                                .get("id")
                                                .and_then(|i| i.as_str())
                                                .unwrap_or("")
                                                .to_string();

                                            match item_type.as_str() {
                                                "mcpToolCall" => {
                                                    let tool = item
                                                        .get("tool")
                                                        .and_then(|t| t.as_str())
                                                        .unwrap_or("unknown")
                                                        .to_string();
                                                    let server = item
                                                        .get("server")
                                                        .and_then(|s| s.as_str())
                                                        .unwrap_or("")
                                                        .to_string();
                                                    let args = item.get("arguments").cloned();

                                                    print!("  MCP Tool: {}", tool);
                                                    if !server.is_empty() {
                                                        print!(" (server: {})", server);
                                                    }
                                                    println!(" started");
                                                    if let Some(arguments) = &args {
                                                        let args_str = arguments.to_string();
                                                        let (truncated_args, was_truncated) =
                                                            truncate_for_display(&args_str, 200);
                                                        if was_truncated {
                                                            println!(
                                                                "    Args: {}...",
                                                                truncated_args
                                                            );
                                                        } else {
                                                            println!("    Args: {}", args_str);
                                                        }
                                                    }

                                                    let invocation = ToolInvocation {
                                                        id: item_id.clone(),
                                                        run_id: run_id.clone(),
                                                        thread_id: thread_id.clone(),
                                                        tool_name: tool.clone(),
                                                        server: Some(server.clone()),
                                                        arguments: args,
                                                        result: None,
                                                        error: None,
                                                        status: ToolStatus::InProgress,
                                                        duration_ms: None,
                                                        created_at: Utc::now(),
                                                    };
                                                    if let Some(mut session) = lock_or_warn(
                                                        &session_clone,
                                                        "tool invocation started update",
                                                    ) {
                                                        session.add_tool_invocation(
                                                            invocation.clone(),
                                                        );
                                                    }

                                                    let history = history_recorder_clone.clone();
                                                    let history_writer =
                                                        history_writer_clone.clone();
                                                    let tool_id = item_id.clone();
                                                    let tool_for_mcp = invocation.clone();
                                                    let tool_event =
                                                        build_tool_invocation_event(&invocation);
                                                    let tool_event_object_id =
                                                        next_tool_invocation_event_object_id(
                                                            &item_id,
                                                            &tool_event.status,
                                                        );
                                                    let mcp_server_for_tool =
                                                        mcp_server_clone.clone();
                                                    tokio::spawn(async move {
                                                        store_to_mcp(
                                                            &mcp_server_for_tool,
                                                            "tool_invocation",
                                                            &tool_id,
                                                            &tool_for_mcp,
                                                            debug_mode,
                                                        )
                                                        .await;
                                                        history
                                                            .event(
                                                                history::EventKind::ToolInvocationStatus,
                                                                &tool_id,
                                                                "in_progress",
                                                                serde_json::json!({"tool": tool, "server": server}),
                                                            )
                                                            .await;
                                                        history_writer
                                                            .write(
                                                                "tool_invocation_event",
                                                                &tool_event_object_id,
                                                                &tool_event,
                                                            )
                                                            .await;
                                                    });
                                                }
                                                "toolCall" => {
                                                    let tool = item
                                                        .get("name")
                                                        .or_else(|| item.get("tool"))
                                                        .and_then(|t| t.as_str())
                                                        .unwrap_or("unknown")
                                                        .to_string();
                                                    let args = item.get("arguments").cloned();
                                                    println!("  Tool: {} started", tool);

                                                    let invocation = ToolInvocation {
                                                        id: item_id.clone(),
                                                        run_id: run_id.clone(),
                                                        thread_id: thread_id.clone(),
                                                        tool_name: tool.clone(),
                                                        server: None,
                                                        arguments: args,
                                                        result: None,
                                                        error: None,
                                                        status: ToolStatus::InProgress,
                                                        duration_ms: None,
                                                        created_at: Utc::now(),
                                                    };
                                                    let tool_id = item_id.clone();
                                                    let tool_for_mcp = invocation.clone();
                                                    if let Some(mut session) = lock_or_warn(
                                                        &session_clone,
                                                        "tool invocation started update",
                                                    ) {
                                                        session.add_tool_invocation(invocation);
                                                    }

                                                    let mcp_server_for_tool =
                                                        mcp_server_clone.clone();
                                                    let history = history_recorder_clone.clone();
                                                    let history_writer =
                                                        history_writer_clone.clone();
                                                    let tool_event =
                                                        build_tool_invocation_event(&tool_for_mcp);
                                                    let tool_event_object_id =
                                                        next_tool_invocation_event_object_id(
                                                            &tool_id,
                                                            &tool_event.status,
                                                        );
                                                    tokio::spawn(async move {
                                                        store_to_mcp(
                                                            &mcp_server_for_tool,
                                                            "tool_invocation",
                                                            &tool_id,
                                                            &tool_for_mcp,
                                                            debug_mode,
                                                        )
                                                        .await;
                                                        history
                                                            .event(
                                                                history::EventKind::ToolInvocationStatus,
                                                                &tool_id,
                                                                "in_progress",
                                                                serde_json::json!({"tool": tool}),
                                                            )
                                                            .await;
                                                        history_writer
                                                            .write(
                                                                "tool_invocation_event",
                                                                &tool_event_object_id,
                                                                &tool_event,
                                                            )
                                                            .await;
                                                    });
                                                }
                                                "commandExecution" => {
                                                    let cmd = item
                                                        .get("command")
                                                        .and_then(|c| c.as_str())
                                                        .unwrap_or("")
                                                        .to_string();
                                                    let command_cwd = item
                                                        .get("cwd")
                                                        .and_then(|c| c.as_str())
                                                        .map(String::from)
                                                        .filter(|cwd| !cwd.is_empty())
                                                        .unwrap_or_else(|| {
                                                            default_command_cwd.clone()
                                                        });
                                                    let command_snapshot =
                                                        capture_workspace_snapshot(Path::new(
                                                            &command_cwd,
                                                        ));
                                                    println!("  Command: {} started", cmd);

                                                    let invocation = ToolInvocation {
                                                        id: item_id.clone(),
                                                        run_id: run_id.clone(),
                                                        thread_id: thread_id.clone(),
                                                        tool_name: "commandExecution".to_string(),
                                                        server: None,
                                                        arguments: Some(
                                                            serde_json::json!({ "command": cmd }),
                                                        ),
                                                        result: None,
                                                        error: None,
                                                        status: ToolStatus::InProgress,
                                                        duration_ms: item
                                                            .get("durationMs")
                                                            .and_then(|d| d.as_i64()),
                                                        created_at: Utc::now(),
                                                    };
                                                    let cmd_id = item_id.clone();
                                                    let cmd_for_mcp = invocation.clone();
                                                    if let Some(mut session) = lock_or_warn(
                                                        &session_clone,
                                                        "tool invocation started update",
                                                    ) {
                                                        session.add_tool_invocation(invocation);
                                                        session.command_baselines.insert(
                                                            item_id.clone(),
                                                            CommandExecutionBaseline {
                                                                cwd: command_cwd.clone(),
                                                                files: command_snapshot,
                                                            },
                                                        );
                                                    }

                                                    let mcp_server_for_cmd =
                                                        mcp_server_clone.clone();
                                                    let history = history_recorder_clone.clone();
                                                    let history_writer =
                                                        history_writer_clone.clone();
                                                    let tool_event =
                                                        build_tool_invocation_event(&cmd_for_mcp);
                                                    let tool_event_object_id =
                                                        next_tool_invocation_event_object_id(
                                                            &cmd_id,
                                                            &tool_event.status,
                                                        );
                                                    tokio::spawn(async move {
                                                        store_to_mcp(
                                                            &mcp_server_for_cmd,
                                                            "tool_invocation",
                                                            &cmd_id,
                                                            &cmd_for_mcp,
                                                            debug_mode,
                                                        )
                                                        .await;
                                                        history
                                                            .event(
                                                                history::EventKind::ToolInvocationStatus,
                                                                &cmd_id,
                                                                "started",
                                                                serde_json::json!({"command": cmd}),
                                                            )
                                                            .await;
                                                        history_writer
                                                            .write(
                                                                "tool_invocation_event",
                                                                &tool_event_object_id,
                                                                &tool_event,
                                                            )
                                                            .await;
                                                    });
                                                }
                                                "reasoning" => {
                                                    println!("  Thinking started");

                                                    let reasoning = Reasoning {
                                                        id: item_id.clone(),
                                                        run_id: run_id.clone(),
                                                        thread_id: thread_id.clone(),
                                                        summary: vec![],
                                                        text: None,
                                                        created_at: Utc::now(),
                                                    };
                                                    let reasoning_id = item_id.clone();
                                                    let reasoning_for_mcp = reasoning.clone();
                                                    if let Some(mut session) = lock_or_warn(
                                                        &session_clone,
                                                        "reasoning started update",
                                                    ) {
                                                        session.add_reasoning(reasoning);
                                                    }

                                                    let mcp_server_for_reasoning =
                                                        mcp_server_clone.clone();
                                                    tokio::spawn(async move {
                                                        store_to_mcp(
                                                            &mcp_server_for_reasoning,
                                                            "reasoning",
                                                            &reasoning_id,
                                                            &reasoning_for_mcp,
                                                            debug_mode,
                                                        )
                                                        .await;
                                                    });
                                                }
                                                "plan" => {
                                                    let text = item
                                                        .get("text")
                                                        .and_then(|t| t.as_str())
                                                        .unwrap_or("")
                                                        .to_string();
                                                    if !text.is_empty() {
                                                        println!("  Plan started: {}", text);
                                                    } else {
                                                        println!("  Plan started");
                                                    }

                                                    let plan = Plan {
                                                        id: item_id.clone(),
                                                        text: text.clone(),
                                                        intent_id: None,
                                                        thread_id: thread_id.clone(),
                                                        turn_id: Some(turn_id.clone()),
                                                        status: PlanStatus::InProgress,
                                                        created_at: Utc::now(),
                                                    };
                                                    let plan_id_2 = item_id.clone();
                                                    let plan_for_mcp_2 = plan.clone();
                                                    if let Some(mut session) = lock_or_warn(
                                                        &session_clone,
                                                        "plan started update",
                                                    ) {
                                                        session.add_plan(plan);
                                                    }

                                                    let mcp_server_for_plan_2 =
                                                        mcp_server_clone.clone();
                                                    tokio::spawn(async move {
                                                        store_to_mcp(
                                                            &mcp_server_for_plan_2,
                                                            "plan",
                                                            &plan_id_2,
                                                            &plan_for_mcp_2,
                                                            debug_mode,
                                                        )
                                                        .await;
                                                    });
                                                }
                                                "fileChange" => {
                                                    println!("  ?? File Change started");

                                                    let patchset = PatchSet {
                                                        id: item_id.clone(),
                                                        run_id: run_id.clone(),
                                                        thread_id: thread_id.clone(),
                                                        changes: vec![],
                                                        status: PatchStatus::InProgress,
                                                        created_at: Utc::now(),
                                                    };
                                                    let patchset_id = item_id.clone();
                                                    let patchset_for_mcp = patchset.clone();
                                                    if let Some(mut session) = lock_or_warn(
                                                        &session_clone,
                                                        "patchset started update",
                                                    ) {
                                                        session.add_patchset(patchset);
                                                    }

                                                    let mcp_server_for_patchset =
                                                        mcp_server_clone.clone();
                                                    tokio::spawn(async move {
                                                        store_to_mcp(
                                                            &mcp_server_for_patchset,
                                                            "patchset",
                                                            &patchset_id,
                                                            &patchset_for_mcp,
                                                            debug_mode,
                                                        )
                                                        .await;
                                                    });
                                                }
                                                "dynamicToolCall" => {
                                                    let tool = item
                                                        .get("tool")
                                                        .and_then(|t| t.as_str())
                                                        .unwrap_or("unknown")
                                                        .to_string();
                                                    let args = item.get("arguments").cloned();
                                                    println!("  Dynamic Tool: {} started", tool);

                                                    let invocation = ToolInvocation {
                                                        id: item_id.clone(),
                                                        run_id: run_id.clone(),
                                                        thread_id: thread_id.clone(),
                                                        tool_name: tool.clone(),
                                                        server: None,
                                                        arguments: args,
                                                        result: None,
                                                        error: None,
                                                        status: ToolStatus::InProgress,
                                                        duration_ms: item
                                                            .get("durationMs")
                                                            .and_then(|d| d.as_i64()),
                                                        created_at: Utc::now(),
                                                    };
                                                    let dyn_tool_id = item_id.clone();
                                                    let dyn_tool_for_mcp = invocation.clone();
                                                    if let Some(mut session) = lock_or_warn(
                                                        &session_clone,
                                                        "tool invocation started update",
                                                    ) {
                                                        session.add_tool_invocation(invocation);
                                                    }

                                                    let mcp_server_for_dyn =
                                                        mcp_server_clone.clone();
                                                    let history_writer =
                                                        history_writer_clone.clone();
                                                    let tool_event = build_tool_invocation_event(
                                                        &dyn_tool_for_mcp,
                                                    );
                                                    let tool_event_object_id =
                                                        next_tool_invocation_event_object_id(
                                                            &dyn_tool_id,
                                                            &tool_event.status,
                                                        );
                                                    tokio::spawn(async move {
                                                        store_to_mcp(
                                                            &mcp_server_for_dyn,
                                                            "tool_invocation",
                                                            &dyn_tool_id,
                                                            &dyn_tool_for_mcp,
                                                            debug_mode,
                                                        )
                                                        .await;
                                                        history_writer
                                                            .write(
                                                                "tool_invocation_event",
                                                                &tool_event_object_id,
                                                                &tool_event,
                                                            )
                                                            .await;
                                                    });
                                                }
                                                "webSearch" => {
                                                    let query = item
                                                        .get("query")
                                                        .and_then(|q| q.as_str())
                                                        .unwrap_or("")
                                                        .to_string();
                                                    let action = item.get("action").cloned();
                                                    println!("  Web Search: {}", query);

                                                    let invocation = ToolInvocation {
                                                        id: item_id.clone(),
                                                        run_id: run_id.clone(),
                                                        thread_id: thread_id.clone(),
                                                        tool_name: "webSearch".to_string(),
                                                        server: None,
                                                        arguments: Some(
                                                            serde_json::json!({ "query": query.clone(), "action": action }),
                                                        ),
                                                        result: None,
                                                        error: None,
                                                        status: ToolStatus::InProgress,
                                                        duration_ms: None,
                                                        created_at: Utc::now(),
                                                    };
                                                    let ws_id = item_id.clone();
                                                    let ws_for_mcp = invocation.clone();
                                                    if let Some(mut session) = lock_or_warn(
                                                        &session_clone,
                                                        "tool invocation started update",
                                                    ) {
                                                        session.add_tool_invocation(invocation);
                                                    }

                                                    let mcp_server_for_ws =
                                                        mcp_server_clone.clone();
                                                    let history_writer =
                                                        history_writer_clone.clone();
                                                    let tool_event =
                                                        build_tool_invocation_event(&ws_for_mcp);
                                                    let tool_event_object_id =
                                                        next_tool_invocation_event_object_id(
                                                            &ws_id,
                                                            &tool_event.status,
                                                        );
                                                    tokio::spawn(async move {
                                                        store_to_mcp(
                                                            &mcp_server_for_ws,
                                                            "tool_invocation",
                                                            &ws_id,
                                                            &ws_for_mcp,
                                                            debug_mode,
                                                        )
                                                        .await;
                                                        history_writer
                                                            .write(
                                                                "tool_invocation_event",
                                                                &tool_event_object_id,
                                                                &tool_event,
                                                            )
                                                            .await;
                                                    });
                                                }
                                                "userMessage" => {
                                                    let content = item
                                                        .get("content")
                                                        .and_then(|c| c.as_array())
                                                        .and_then(|arr| arr.first())
                                                        .and_then(|first| first.get("text"))
                                                        .and_then(|t| t.as_str())
                                                        .unwrap_or("")
                                                        .to_string();
                                                    let (truncated, _) =
                                                        truncate_for_display(&content, 50);
                                                    println!("  User: {}", truncated);

                                                    let parent_intent_id = lock_or_warn(
                                                        &session_clone,
                                                        "intent parent lookup",
                                                    )
                                                    .and_then(|session| {
                                                        latest_thread_intent_id(
                                                            &session,
                                                            &thread_id,
                                                            Some(&item_id),
                                                        )
                                                    });
                                                    let intent = Intent {
                                                        id: item_id.clone(),
                                                        content: content.clone(),
                                                        thread_id: thread_id.clone(),
                                                        created_at: Utc::now(),
                                                    };
                                                    let intent_snapshot = IntentSnapshot {
                                                        id: item_id.clone(),
                                                        content: content.clone(),
                                                        thread_id: thread_id.clone(),
                                                        parents: parent_intent_id
                                                            .clone()
                                                            .into_iter()
                                                            .collect(),
                                                        analysis_context_frames: Vec::new(),
                                                        created_at: Utc::now(),
                                                    };
                                                    let intent_event = IntentEvent {
                                                        id: format!("intent_event_{}", item_id),
                                                        intent_id: item_id.clone(),
                                                        status: "created".to_string(),
                                                        at: Utc::now(),
                                                        next_intent_id: None,
                                                    };
                                                    let parent_link_event = parent_intent_id
                                                        .clone()
                                                        .map(|parent_id| IntentEvent {
                                                            id: format!(
                                                                "intent_event_link_{}_{}",
                                                                parent_id, item_id
                                                            ),
                                                            intent_id: parent_id,
                                                            status: "continued".to_string(),
                                                            at: Utc::now(),
                                                            next_intent_id: Some(item_id.clone()),
                                                        });
                                                    let history_writer =
                                                        history_writer_clone.clone();
                                                    let intent_id_for_write = item_id.clone();
                                                    tokio::spawn(async move {
                                                        history_writer
                                                            .write(
                                                                "intent_snapshot",
                                                                &intent_id_for_write,
                                                                &intent_snapshot,
                                                            )
                                                            .await;
                                                        history_writer
                                                            .write(
                                                                "intent_event",
                                                                &intent_event.id,
                                                                &intent_event,
                                                            )
                                                            .await;
                                                        if let Some(link_event) = parent_link_event
                                                        {
                                                            history_writer
                                                                .write(
                                                                    "intent_event",
                                                                    &link_event.id,
                                                                    &link_event,
                                                                )
                                                                .await;
                                                        }
                                                    });
                                                    let intent_id = item_id.clone();
                                                    let intent_for_mcp = intent.clone();
                                                    if let Some(mut session) = lock_or_warn(
                                                        &session_clone,
                                                        "intent started update",
                                                    ) {
                                                        session.add_intent(intent);
                                                    }

                                                    let mcp_server_for_intent =
                                                        mcp_server_clone.clone();
                                                    tokio::spawn(async move {
                                                        store_to_mcp(
                                                            &mcp_server_for_intent,
                                                            "intent",
                                                            &intent_id,
                                                            &intent_for_mcp,
                                                            debug_mode,
                                                        )
                                                        .await;
                                                    });
                                                }
                                                "agentMessage" => {
                                                    let content = item
                                                        .get("text")
                                                        .and_then(|t| t.as_str())
                                                        .unwrap_or("")
                                                        .to_string();
                                                    println!("Agent Response started");

                                                    let msg = AgentMessage {
                                                        id: item_id.clone(),
                                                        run_id: run_id.clone(),
                                                        thread_id: thread_id.clone(),
                                                        content,
                                                        created_at: Utc::now(),
                                                    };
                                                    if let Some(mut session) = lock_or_warn(
                                                        &session_clone,
                                                        "agent message started update",
                                                    ) {
                                                        session.add_agent_message(msg);
                                                    }
                                                }
                                                "collabAgentToolCall" => {
                                                    let tool = item
                                                        .get("tool")
                                                        .and_then(|t| t.as_str())
                                                        .unwrap_or("unknown")
                                                        .to_string();
                                                    let status = item
                                                        .get("status")
                                                        .and_then(|s| s.as_str())
                                                        .unwrap_or("started")
                                                        .to_string();
                                                    let prompt = item.get("prompt").cloned();
                                                    let sender_thread_id = item
                                                        .get("senderThreadId")
                                                        .and_then(|t| t.as_str())
                                                        .unwrap_or("")
                                                        .to_string();
                                                    let receiver_thread_ids =
                                                        item.get("receiverThreadIds").cloned();
                                                    println!("  Collab Tool: {} started", tool);

                                                    let invocation = ToolInvocation {
                                                        id: item_id.clone(),
                                                        run_id: run_id.clone(),
                                                        thread_id: thread_id.clone(),
                                                        tool_name: format!(
                                                            "collabAgentToolCall:{}",
                                                            tool
                                                        ),
                                                        server: None,
                                                        arguments: Some(serde_json::json!({
                                                            "tool": tool,
                                                            "status": status,
                                                            "prompt": prompt,
                                                            "sender_thread_id": sender_thread_id,
                                                            "receiver_thread_ids": receiver_thread_ids
                                                        })),
                                                        result: None,
                                                        error: None,
                                                        status: ToolStatus::InProgress,
                                                        duration_ms: None,
                                                        created_at: Utc::now(),
                                                    };
                                                    let inv_id = item_id.clone();
                                                    let inv_for_mcp = invocation.clone();
                                                    if let Some(mut session) = lock_or_warn(
                                                        &session_clone,
                                                        "collab tool invocation started update",
                                                    ) {
                                                        session.add_tool_invocation(invocation);
                                                    }

                                                    let mcp_server_for_inv =
                                                        mcp_server_clone.clone();
                                                    let history_writer =
                                                        history_writer_clone.clone();
                                                    let tool_event =
                                                        build_tool_invocation_event(&inv_for_mcp);
                                                    let tool_event_object_id =
                                                        next_tool_invocation_event_object_id(
                                                            &inv_id,
                                                            &tool_event.status,
                                                        );
                                                    tokio::spawn(async move {
                                                        store_to_mcp(
                                                            &mcp_server_for_inv,
                                                            "tool_invocation",
                                                            &inv_id,
                                                            &inv_for_mcp,
                                                            debug_mode,
                                                        )
                                                        .await;
                                                        history_writer
                                                            .write(
                                                                "tool_invocation_event",
                                                                &tool_event_object_id,
                                                                &tool_event,
                                                            )
                                                            .await;
                                                    });
                                                }
                                                "enteredReviewMode" => {
                                                    let review_id = item
                                                        .get("review")
                                                        .and_then(|r| r.as_str())
                                                        .unwrap_or("");
                                                    println!(
                                                        "  Entered review mode: {}",
                                                        review_id
                                                    );
                                                }
                                                "exitedReviewMode" => {
                                                    let review_id = item
                                                        .get("review")
                                                        .and_then(|r| r.as_str())
                                                        .unwrap_or("");
                                                    println!("  Exited review mode: {}", review_id);
                                                }
                                                "contextCompaction" => {
                                                    println!("  Context compaction started");
                                                    let context_frame = ContextFrameEvent {
                                                        id: format!(
                                                            "context_frame_started_{}",
                                                            item_id
                                                        ),
                                                        run_id: run_id.clone(),
                                                        plan_id: None,
                                                        step_id: None,
                                                        at: Utc::now(),
                                                        delta: serde_json::json!({
                                                            "kind": "context_compaction",
                                                            "status": "started"
                                                        }),
                                                    };
                                                    let history_writer =
                                                        history_writer_clone.clone();
                                                    let context_frame_id = context_frame.id.clone();
                                                    tokio::spawn(async move {
                                                        history_writer
                                                            .write(
                                                                "context_frame",
                                                                &context_frame_id,
                                                                &context_frame,
                                                            )
                                                            .await;
                                                    });
                                                    let snapshot = ContextSnapshot {
                                                        id: item_id.clone(),
                                                        thread_id: thread_id.clone(),
                                                        run_id: Some(run_id.clone()),
                                                        created_at: Utc::now(),
                                                        data: serde_json::json!({}),
                                                    };
                                                    let snapshot_id = snapshot.id.clone();
                                                    let snapshot_for_mcp = snapshot.clone();
                                                    let mcp_server_for_snapshot =
                                                        mcp_server_clone.clone();
                                                    tokio::spawn(async move {
                                                        store_to_mcp(
                                                            &mcp_server_for_snapshot,
                                                            "context_snapshot",
                                                            &snapshot_id,
                                                            &snapshot_for_mcp,
                                                            debug_mode,
                                                        )
                                                        .await;
                                                    });
                                                }
                                                _ => {
                                                    println!("  Task: {} started", item_type);
                                                }
                                            }
                                        }
                                    }
                                }
                                // Handle item/completed notification
                                else if matches!(mk, MethodKind::ItemCompleted) {
                                    if let Some(params_obj) = json.get("params").cloned() {
                                        let _thread_id = params_obj
                                            .get("threadId")
                                            .and_then(|t| t.as_str())
                                            .unwrap_or("")
                                            .to_string();
                                        let _turn_id = params_obj
                                            .get("turnId")
                                            .and_then(|t| t.as_str())
                                            .unwrap_or("")
                                            .to_string();

                                        if let Some(item) = params_obj.get("item").cloned() {
                                            let item_type = item
                                                .get("type")
                                                .and_then(|t| t.as_str())
                                                .unwrap_or("")
                                                .to_string();
                                            let item_id = item
                                                .get("id")
                                                .and_then(|i| i.as_str())
                                                .unwrap_or("")
                                                .to_string();

                                            match item_type.as_str() {
                                                "mcpToolCall" => {
                                                    let tool = item
                                                        .get("tool")
                                                        .and_then(|t| t.as_str())
                                                        .unwrap_or("unknown")
                                                        .to_string();
                                                    let status = item
                                                        .get("status")
                                                        .and_then(|s| s.as_str())
                                                        .unwrap_or("completed")
                                                        .to_string();
                                                    let result = item.get("result").cloned();
                                                    let error = item
                                                        .get("error")
                                                        .and_then(|e| e.as_str())
                                                        .map(String::from);
                                                    let duration_ms = item
                                                        .get("durationMs")
                                                        .and_then(|d| d.as_i64());

                                                    print!("  MCP Tool: {} - {}", tool, status);
                                                    if let Some(result_val) = result.as_ref() {
                                                        let result_str = result_val.to_string();
                                                        let (truncated_result, was_truncated) =
                                                            truncate_for_display(&result_str, 100);
                                                        if was_truncated {
                                                            println!(
                                                                " | Result: {}...",
                                                                truncated_result
                                                            );
                                                        } else if !result_str.is_empty()
                                                            && result_str != "null"
                                                        {
                                                            println!(" | Result: {}", result_str);
                                                        } else {
                                                            println!();
                                                        }
                                                    } else if let Some(error_val) = error.as_ref() {
                                                        println!(" | Error: {}", error_val);
                                                    } else {
                                                        println!();
                                                    }

                                                    let updated_inv = if let Some(mut session) =
                                                        lock_or_warn(
                                                            &session_clone,
                                                            "tool invocation completed update",
                                                        ) {
                                                        if let Some(invocation) = session
                                                            .tool_invocations
                                                            .iter_mut()
                                                            .find(|i| i.id == item_id)
                                                        {
                                                            invocation.status =
                                                                match status.as_str() {
                                                                    "completed" => {
                                                                        ToolStatus::Completed
                                                                    }
                                                                    "failed" => ToolStatus::Failed,
                                                                    _ => ToolStatus::Completed,
                                                                };
                                                            invocation.result = result.clone();
                                                            invocation.error = error.clone();
                                                            invocation.duration_ms = duration_ms;
                                                        }
                                                        session
                                                            .tool_invocations
                                                            .iter()
                                                            .find(|i| i.id == item_id)
                                                            .cloned()
                                                    } else {
                                                        None
                                                    };

                                                    if let Some(inv) = updated_inv {
                                                        let mcp_server_for_inv =
                                                            mcp_server_clone.clone();
                                                        let history =
                                                            history_recorder_clone.clone();
                                                        let history_writer =
                                                            history_writer_clone.clone();
                                                        let inv_id = inv.id.clone();
                                                        let tool_name = tool.clone();
                                                        let tool_event =
                                                            build_tool_invocation_event(&inv);
                                                        let tool_event_object_id =
                                                            next_tool_invocation_event_object_id(
                                                                &inv_id,
                                                                &tool_event.status,
                                                            );
                                                        tokio::spawn(async move {
                                                            store_to_mcp(
                                                                &mcp_server_for_inv,
                                                                "tool_invocation",
                                                                &inv_id,
                                                                &inv,
                                                                debug_mode,
                                                            )
                                                            .await;
                                                            history
                                                                .event(
                                                                    history::EventKind::ToolInvocationStatus,
                                                                    &inv_id,
                                                                    inv.status.to_string(),
                                                                    serde_json::json!({
                                                                        "tool": tool_name,
                                                                        "duration_ms": duration_ms,
                                                                        "error": error,
                                                                        "result": result
                                                                    }),
                                                                )
                                                                .await;
                                                            history_writer
                                                                .write(
                                                                    "tool_invocation_event",
                                                                    &tool_event_object_id,
                                                                    &tool_event,
                                                                )
                                                                .await;
                                                        });
                                                    }
                                                }
                                                "commandExecution" => {
                                                    let cmd = item
                                                        .get("command")
                                                        .and_then(|c| c.as_str())
                                                        .unwrap_or("")
                                                        .to_string();
                                                    let command_cwd_from_item = item
                                                        .get("cwd")
                                                        .and_then(|c| c.as_str())
                                                        .map(String::from)
                                                        .filter(|cwd| !cwd.is_empty());
                                                    let exit_code = item
                                                        .get("exitCode")
                                                        .and_then(|c| c.as_i64());
                                                    let duration_ms = item
                                                        .get("durationMs")
                                                        .and_then(|d| d.as_i64());
                                                    let output = item
                                                        .get("aggregatedOutput")
                                                        .and_then(|o| o.as_str())
                                                        .map(String::from);

                                                    println!(
                                                        "  Command: {} exit={:?}",
                                                        cmd, exit_code
                                                    );

                                                    let command_baseline = if let Some(
                                                        mut session,
                                                    ) = lock_or_warn(
                                                        &session_clone,
                                                        "command execution baseline read",
                                                    ) {
                                                        session.command_baselines.remove(&item_id)
                                                    } else {
                                                        None
                                                    };

                                                    let updated_invocation = if let Some(
                                                        mut session,
                                                    ) = lock_or_warn(
                                                        &session_clone,
                                                        "command execution completed update",
                                                    ) {
                                                        if let Some(invocation) = session
                                                            .tool_invocations
                                                            .iter_mut()
                                                            .find(|i| i.id == item_id)
                                                        {
                                                            invocation.status = match exit_code {
                                                                Some(0) => ToolStatus::Completed,
                                                                Some(_) => ToolStatus::Failed,
                                                                None => ToolStatus::Completed,
                                                            };
                                                            invocation.result = output
                                                                .as_ref()
                                                                .map(|o| serde_json::json!({ "output": o }));
                                                            invocation.duration_ms = duration_ms;
                                                            Some(invocation.clone())
                                                        } else {
                                                            None
                                                        }
                                                    } else {
                                                        None
                                                    };

                                                    if let Some(inv) = updated_invocation {
                                                        let invocation_status = inv.status.clone();
                                                        let patchset_status_string =
                                                            match invocation_status {
                                                                ToolStatus::Completed => {
                                                                    "completed".to_string()
                                                                }
                                                                ToolStatus::Failed => {
                                                                    "failed".to_string()
                                                                }
                                                                ToolStatus::InProgress => {
                                                                    "in_progress".to_string()
                                                                }
                                                                ToolStatus::Pending => {
                                                                    "pending".to_string()
                                                                }
                                                            };
                                                        let command_patchset =
                                                            command_baseline.and_then(
                                                                |baseline| {
                                                                    let effective_cwd =
                                                                        command_cwd_from_item
                                                                            .clone()
                                                                            .unwrap_or(
                                                                                baseline.cwd,
                                                                            );
                                                                    let after_snapshot =
                                                                        capture_workspace_snapshot(
                                                                            Path::new(
                                                                                &effective_cwd,
                                                                            ),
                                                                        );
                                                                    let changes =
                                                                        build_file_changes_from_snapshots(
                                                                            &baseline.files,
                                                                            &after_snapshot,
                                                                        );
                                                                    if changes.is_empty() {
                                                                        None
                                                                    } else {
                                                                        Some(PatchSet {
                                                                            id: format!(
                                                                                "command_patchset_{}",
                                                                                item_id
                                                                            ),
                                                                            run_id: _turn_id
                                                                                .to_string(),
                                                                            thread_id: _thread_id
                                                                                .to_string(),
                                                                            changes,
                                                                            status: match invocation_status
                                                                            {
                                                                                ToolStatus::Completed => {
                                                                                    PatchStatus::Completed
                                                                                }
                                                                                ToolStatus::Failed => {
                                                                                    PatchStatus::Failed
                                                                                }
                                                                                _ => {
                                                                                    PatchStatus::Pending
                                                                                }
                                                                            },
                                                                            created_at: Utc::now(),
                                                                        })
                                                                    }
                                                                },
                                                            );
                                                        let mcp_server_for_inv =
                                                            mcp_server_clone.clone();
                                                        let history =
                                                            history_recorder_clone.clone();
                                                        let history_writer =
                                                            history_writer_clone.clone();
                                                        let inv_id = item_id.clone();
                                                        let cmd_name = cmd.clone();
                                                        let tool_event =
                                                            build_tool_invocation_event(&inv);
                                                        let tool_event_object_id =
                                                            next_tool_invocation_event_object_id(
                                                                &inv_id,
                                                                &tool_event.status,
                                                            );
                                                        let decision = DecisionEvent {
                                                            id: format!("decision_{}", inv_id),
                                                            run_id: _turn_id.to_string(),
                                                            chosen_patchset_id: None,
                                                            approved: inv.status
                                                                == ToolStatus::Completed,
                                                            at: Utc::now(),
                                                            rationale: None,
                                                        };
                                                        let run_event_failed = RunEvent {
                                                            id: format!(
                                                                "run_event_{}_failed",
                                                                _turn_id
                                                            ),
                                                            run_id: _turn_id.to_string(),
                                                            status: "failed".to_string(),
                                                            at: Utc::now(),
                                                            error: if inv.status
                                                                == ToolStatus::Failed
                                                            {
                                                                Some(
                                                                    "command_execution_failed"
                                                                        .to_string(),
                                                                )
                                                            } else {
                                                                None
                                                            },
                                                        };
                                                        tokio::spawn(async move {
                                                            store_to_mcp(
                                                                &mcp_server_for_inv,
                                                                "tool_invocation",
                                                                &inv_id,
                                                                &inv,
                                                                debug_mode,
                                                            )
                                                            .await;
                                                            history
                                                                .event(
                                                                    history::EventKind::ToolInvocationStatus,
                                                                    &inv_id,
                                                                    inv.status.to_string(),
                                                                    serde_json::json!({"command": cmd_name, "exit": exit_code}),
                                                                )
                                                                .await;
                                                            history_writer
                                                                .write(
                                                                    "tool_invocation_event",
                                                                    &tool_event_object_id,
                                                                    &tool_event,
                                                                )
                                                                .await;
                                                            history_writer
                                                                .write(
                                                                    "decision",
                                                                    &decision.id,
                                                                    &decision,
                                                                )
                                                                .await;
                                                            if inv.status == ToolStatus::Failed {
                                                                history_writer
                                                                    .write(
                                                                        "run_event",
                                                                        &run_event_failed.id,
                                                                        &run_event_failed,
                                                                    )
                                                                    .await;
                                                            }
                                                        });

                                                        if let Some(patchset) =
                                                            command_patchset.clone()
                                                        {
                                                            if let Some(mut session) = lock_or_warn(
                                                                &session_clone,
                                                                "command patchset update",
                                                            ) {
                                                                session
                                                                    .add_patchset(patchset.clone());
                                                            }
                                                            persist_patchset_snapshot_and_evidence(
                                                                mcp_server_clone.clone(),
                                                                history_recorder_clone.clone(),
                                                                history_writer_clone.clone(),
                                                                patchset,
                                                                patchset_status_string,
                                                                debug_mode,
                                                            );
                                                        }
                                                    }
                                                }
                                                "reasoning" => {
                                                    println!("  Thinking completed");

                                                    let summary = item
                                                        .get("summary")
                                                        .and_then(|s| s.as_array())
                                                        .map(|arr| {
                                                            arr.iter()
                                                                .filter_map(|v| {
                                                                    v.as_str().map(String::from)
                                                                })
                                                                .collect()
                                                        })
                                                        .unwrap_or_default();
                                                    let text = item
                                                        .get("text")
                                                        .and_then(|t| t.as_str())
                                                        .map(String::from);

                                                    if let Some(mut session) = lock_or_warn(
                                                        &session_clone,
                                                        "reasoning completed update",
                                                    ) && let Some(reasoning) = session
                                                        .reasonings
                                                        .iter_mut()
                                                        .find(|r| r.id == item_id)
                                                    {
                                                        reasoning.summary = summary;
                                                        reasoning.text = text;
                                                    }
                                                }
                                                "plan" => {
                                                    let text = item
                                                        .get("text")
                                                        .and_then(|t| t.as_str())
                                                        .unwrap_or("")
                                                        .to_string();
                                                    if !text.is_empty() {
                                                        println!("  Plan completed: {}", text);
                                                    } else {
                                                        println!("  Plan completed");
                                                    }

                                                    if let Some(mut session) = lock_or_warn(
                                                        &session_clone,
                                                        "plan completed update",
                                                    ) && let Some(plan) = session
                                                        .plans
                                                        .iter_mut()
                                                        .find(|p| p.id == item_id)
                                                    {
                                                        plan.status = PlanStatus::Completed;
                                                        if !text.is_empty() {
                                                            plan.text = text;
                                                        }
                                                    }
                                                }
                                                "fileChange" => {
                                                    let status = item
                                                        .get("status")
                                                        .and_then(|s| s.as_str())
                                                        .unwrap_or("")
                                                        .to_string();

                                                    if debug_mode {
                                                        eprintln!(
                                                            "[DEBUG] fileChange item: {:?}",
                                                            item
                                                        );
                                                    }

                                                    let changes: Vec<FileChange> =
                                                        parse_patchset_changes_from_array(
                                                            item.get("changes"),
                                                        );

                                                    if debug_mode {
                                                        eprintln!(
                                                            "[DEBUG] fileChange changes parsed: {} items",
                                                            changes.len()
                                                        );
                                                    }

                                                    let file_count = changes.len();
                                                    println!(
                                                        "  ?? File Change {} ({} files)",
                                                        status, file_count
                                                    );

                                                    for change in changes.iter().take(3) {
                                                        println!(
                                                            "    - {} ({})",
                                                            change.path, change.change_type
                                                        );
                                                        if !change.diff.is_empty() {
                                                            let diff_lines: Vec<&str> = change
                                                                .diff
                                                                .lines()
                                                                .take(10)
                                                                .collect();
                                                            for line in diff_lines {
                                                                println!("      {}", line);
                                                            }
                                                            if change.diff.lines().count() > 10 {
                                                                println!(
                                                                    "      ... ({} more lines)",
                                                                    change.diff.lines().count()
                                                                        - 10
                                                                );
                                                            }
                                                        }
                                                    }
                                                    if file_count > 3 {
                                                        println!(
                                                            "    ... and {} more files",
                                                            file_count - 3
                                                        );
                                                    }

                                                    if let Some(mut session) = lock_or_warn(
                                                        &session_clone,
                                                        "patchset completed update",
                                                    ) {
                                                        let patchset_status =
                                                            patch_status_from_str(&status);
                                                        if let Some(patchset) = session
                                                            .patchsets
                                                            .iter_mut()
                                                            .find(|p| p.id == item_id)
                                                        {
                                                            let merged_changes =
                                                                merge_patchset_changes(
                                                                    &patchset.changes,
                                                                    &changes,
                                                                );
                                                            patchset.status = patchset_status;
                                                            patchset.changes = merged_changes;
                                                        } else {
                                                            session.add_patchset(PatchSet {
                                                                id: item_id.clone(),
                                                                run_id: _turn_id.to_string(),
                                                                thread_id: _thread_id.to_string(),
                                                                changes: changes.clone(),
                                                                status: patchset_status,
                                                                created_at: Utc::now(),
                                                            });
                                                        }
                                                    }

                                                    let patchset_to_store = if let Some(session) =
                                                        lock_or_warn(
                                                            &session_clone,
                                                            "patchset completed read",
                                                        ) {
                                                        session
                                                            .patchsets
                                                            .iter()
                                                            .find(|p| p.id == item_id)
                                                            .cloned()
                                                    } else {
                                                        None
                                                    };

                                                    if let Some(patchset) = patchset_to_store {
                                                        let patchset = PatchSet {
                                                            run_id: if patchset.run_id.is_empty() {
                                                                _turn_id.to_string()
                                                            } else {
                                                                patchset.run_id.clone()
                                                            },
                                                            thread_id: if patchset
                                                                .thread_id
                                                                .is_empty()
                                                            {
                                                                _thread_id.to_string()
                                                            } else {
                                                                patchset.thread_id.clone()
                                                            },
                                                            ..patchset
                                                        };
                                                        persist_patchset_snapshot_and_evidence(
                                                            mcp_server_clone.clone(),
                                                            history_recorder_clone.clone(),
                                                            history_writer_clone.clone(),
                                                            patchset,
                                                            status.clone(),
                                                            debug_mode,
                                                        );
                                                    }
                                                }
                                                "toolCall" => {
                                                    let tool = item
                                                        .get("name")
                                                        .or_else(|| item.get("tool"))
                                                        .and_then(|t| t.as_str())
                                                        .unwrap_or("unknown")
                                                        .to_string();
                                                    let status = item
                                                        .get("status")
                                                        .and_then(|s| s.as_str())
                                                        .unwrap_or("completed")
                                                        .to_string();
                                                    let result = item.get("result").cloned();
                                                    let error = item
                                                        .get("error")
                                                        .and_then(|e| e.as_str())
                                                        .map(String::from);
                                                    let duration_ms = item
                                                        .get("durationMs")
                                                        .and_then(|d| d.as_i64());

                                                    print!("  Tool: {} - {}", tool, status);
                                                    if let Some(result_val) = result.as_ref() {
                                                        let result_str = result_val.to_string();
                                                        let (truncated_result, was_truncated) =
                                                            truncate_for_display(&result_str, 100);
                                                        if was_truncated {
                                                            println!(
                                                                " | Result: {}...",
                                                                truncated_result
                                                            );
                                                        } else if !result_str.is_empty()
                                                            && result_str != "null"
                                                        {
                                                            println!(" | Result: {}", result_str);
                                                        } else {
                                                            println!();
                                                        }
                                                    } else {
                                                        println!();
                                                    }

                                                    if let Some(mut session) = lock_or_warn(
                                                        &session_clone,
                                                        "tool invocation completed update",
                                                    ) && let Some(invocation) = session
                                                        .tool_invocations
                                                        .iter_mut()
                                                        .find(|i| i.id == item_id)
                                                    {
                                                        invocation.status = match status.as_str() {
                                                            "completed" => ToolStatus::Completed,
                                                            "failed" => ToolStatus::Failed,
                                                            _ => ToolStatus::Completed,
                                                        };
                                                        invocation.result = result.clone();
                                                        invocation.error = error.clone();
                                                        invocation.duration_ms = duration_ms;
                                                    }

                                                    let invocation_to_store = if let Some(session) =
                                                        lock_or_warn(
                                                            &session_clone,
                                                            "tool invocation completed read",
                                                        ) {
                                                        session
                                                            .tool_invocations
                                                            .iter()
                                                            .find(|i| i.id == item_id)
                                                            .cloned()
                                                    } else {
                                                        None
                                                    };

                                                    if let Some(invocation) = invocation_to_store {
                                                        let mcp_server_for_inv =
                                                            mcp_server_clone.clone();
                                                        let history_writer =
                                                            history_writer_clone.clone();
                                                        let inv_id = item_id.clone();
                                                        let tool_event =
                                                            build_tool_invocation_event(
                                                                &invocation,
                                                            );
                                                        let tool_event_object_id =
                                                            next_tool_invocation_event_object_id(
                                                                &inv_id,
                                                                &tool_event.status,
                                                            );
                                                        tokio::spawn(async move {
                                                            store_to_mcp(
                                                                &mcp_server_for_inv,
                                                                "tool_invocation",
                                                                &inv_id,
                                                                &invocation,
                                                                debug_mode,
                                                            )
                                                            .await;
                                                            history_writer
                                                                .write(
                                                                    "tool_invocation_event",
                                                                    &tool_event_object_id,
                                                                    &tool_event,
                                                                )
                                                                .await;
                                                        });
                                                    }
                                                }
                                                "collabAgentToolCall" => {
                                                    let status = item
                                                        .get("status")
                                                        .and_then(|s| s.as_str())
                                                        .unwrap_or("completed")
                                                        .to_string();
                                                    let mut updated = None;
                                                    if let Some(mut session) = lock_or_warn(
                                                        &session_clone,
                                                        "collab tool invocation completed update",
                                                    ) && let Some(invocation) = session
                                                        .tool_invocations
                                                        .iter_mut()
                                                        .find(|i| i.id == item_id)
                                                    {
                                                        invocation.status = match status.as_str() {
                                                            "completed" => ToolStatus::Completed,
                                                            "failed" => ToolStatus::Failed,
                                                            _ => ToolStatus::Completed,
                                                        };
                                                        updated = Some(invocation.clone());
                                                    }
                                                    if let Some(invocation) = updated {
                                                        let mcp_server_for_inv =
                                                            mcp_server_clone.clone();
                                                        let history_writer =
                                                            history_writer_clone.clone();
                                                        let inv_id = item_id.clone();
                                                        let tool_event =
                                                            build_tool_invocation_event(
                                                                &invocation,
                                                            );
                                                        let tool_event_object_id =
                                                            next_tool_invocation_event_object_id(
                                                                &inv_id,
                                                                &tool_event.status,
                                                            );
                                                        tokio::spawn(async move {
                                                            store_to_mcp(
                                                                &mcp_server_for_inv,
                                                                "tool_invocation",
                                                                &inv_id,
                                                                &invocation,
                                                                debug_mode,
                                                            )
                                                            .await;
                                                            history_writer
                                                                .write(
                                                                    "tool_invocation_event",
                                                                    &tool_event_object_id,
                                                                    &tool_event,
                                                                )
                                                                .await;
                                                        });
                                                    }
                                                }
                                                "webSearch" => {
                                                    let status = item
                                                        .get("status")
                                                        .and_then(|s| s.as_str())
                                                        .unwrap_or("completed")
                                                        .to_string();
                                                    let updated_invocation = if let Some(
                                                        mut session,
                                                    ) = lock_or_warn(
                                                        &session_clone,
                                                        "web search completed update",
                                                    ) && let Some(
                                                        invocation,
                                                    ) = session
                                                        .tool_invocations
                                                        .iter_mut()
                                                        .find(|i| i.id == item_id)
                                                    {
                                                        invocation.status = match status.as_str() {
                                                            "completed" => ToolStatus::Completed,
                                                            "failed" => ToolStatus::Failed,
                                                            _ => ToolStatus::Completed,
                                                        };
                                                        Some(invocation.clone())
                                                    } else {
                                                        None
                                                    };
                                                    if let Some(invocation) = updated_invocation {
                                                        let mcp_server_for_inv =
                                                            mcp_server_clone.clone();
                                                        let history_writer =
                                                            history_writer_clone.clone();
                                                        let inv_id = item_id.clone();
                                                        let tool_event =
                                                            build_tool_invocation_event(
                                                                &invocation,
                                                            );
                                                        let tool_event_object_id =
                                                            next_tool_invocation_event_object_id(
                                                                &inv_id,
                                                                &tool_event.status,
                                                            );
                                                        tokio::spawn(async move {
                                                            store_to_mcp(
                                                                &mcp_server_for_inv,
                                                                "tool_invocation",
                                                                &inv_id,
                                                                &invocation,
                                                                debug_mode,
                                                            )
                                                            .await;
                                                            history_writer
                                                                .write(
                                                                    "tool_invocation_event",
                                                                    &tool_event_object_id,
                                                                    &tool_event,
                                                                )
                                                                .await;
                                                        });
                                                    }
                                                }
                                                "contextCompaction" => {
                                                    println!("  Context compaction completed");
                                                    let context_frame = ContextFrameEvent {
                                                        id: format!(
                                                            "context_frame_completed_{}",
                                                            item_id
                                                        ),
                                                        run_id: _turn_id.to_string(),
                                                        plan_id: None,
                                                        step_id: None,
                                                        at: Utc::now(),
                                                        delta: serde_json::json!({
                                                            "kind": "context_compaction",
                                                            "status": "completed"
                                                        }),
                                                    };
                                                    let history_writer =
                                                        history_writer_clone.clone();
                                                    let context_frame_id = context_frame.id.clone();
                                                    tokio::spawn(async move {
                                                        history_writer
                                                            .write(
                                                                "context_frame",
                                                                &context_frame_id,
                                                                &context_frame,
                                                            )
                                                            .await;
                                                    });
                                                }
                                                "userMessage" => {
                                                    println!("  User message completed");
                                                }
                                                "agentMessage" => {
                                                    if debug_mode {
                                                        eprintln!(
                                                            "[DEBUG] agentMessage completed item: {:?}",
                                                            item
                                                        );
                                                    }
                                                    let content = item
                                                        .get("text")
                                                        .and_then(|t| t.as_str())
                                                        .unwrap_or("")
                                                        .to_string();
                                                    let previous_content = if let Some(mut session) =
                                                        lock_or_warn(
                                                            &session_clone,
                                                            "agent message completed update",
                                                        )
                                                        && let Some(msg) = session
                                                            .agent_messages
                                                            .iter_mut()
                                                            .find(|m| m.id == item_id)
                                                    {
                                                        let previous_content = msg.content.clone();
                                                        msg.content = content.clone();
                                                        previous_content
                                                    } else {
                                                        String::new()
                                                    };
                                                    if !content.is_empty() {
                                                        if previous_content.is_empty() {
                                                            println!("Agent: {}", content);
                                                        } else if let Some(suffix) =
                                                            content.strip_prefix(&previous_content)
                                                        {
                                                            if !suffix.is_empty() {
                                                                print!("{}", suffix);
                                                                if !content.ends_with('\n') {
                                                                    println!();
                                                                }
                                                            }
                                                        } else {
                                                            println!("Agent: {}", content);
                                                        }
                                                    }
                                                    println!("Agent Response completed");
                                                }
                                                _ => {}
                                            }
                                        }
                                    }
                                } else if matches!(
                                    mk,
                                    MethodKind::RequestApproval
                                        | MethodKind::RequestApprovalCommandExecution
                                        | MethodKind::RequestApprovalFileChange
                                        | MethodKind::RequestApprovalApplyPatch
                                        | MethodKind::RequestApprovalExec
                                ) {
                                    // Get request ID
                                    let request_id = params
                                        .get("requestId")
                                        .or_else(|| params.get("request_id"))
                                        .and_then(|v| v.as_str())
                                        .map(String::from)
                                        .unwrap_or_else(|| {
                                            format!("req_{}", Utc::now().timestamp_millis())
                                        });
                                    let approval_params = json
                                        .get("params")
                                        .cloned()
                                        .unwrap_or(serde_json::json!({}));

                                    // Determine approval type
                                    let approval_type = match mk {
                                        MethodKind::RequestApprovalCommandExecution => {
                                            ApprovalType::CommandExecution
                                        }
                                        MethodKind::RequestApprovalFileChange => {
                                            ApprovalType::FileChange
                                        }
                                        MethodKind::RequestApprovalApplyPatch => {
                                            ApprovalType::ApplyPatch
                                        }
                                        MethodKind::RequestApprovalExec => ApprovalType::Unknown,
                                        _ => ApprovalType::Unknown,
                                    };

                                    // Get item_id if available
                                    let item_id = approval_params
                                        .get("itemId")
                                        .or_else(|| approval_params.get("call_id"))
                                        .or_else(|| approval_params.get("callId"))
                                        .and_then(|v| v.as_str())
                                        .map(String::from)
                                        .unwrap_or_default();

                                    // Get thread_id if available
                                    let thread_id = approval_params
                                        .get("threadId")
                                        .or_else(|| approval_params.get("conversationId"))
                                        .and_then(|v| v.as_str())
                                        .map(String::from)
                                        .unwrap_or_default();

                                    // Get command or changes from approval_params
                                    let command = approval_params
                                        .get("command")
                                        .and_then(|v| v.as_str())
                                        .map(String::from);
                                    let approval_patch_changes = parse_patchset_changes_from_map(
                                        approval_params
                                            .get("fileChanges")
                                            .or_else(|| approval_params.get("changes")),
                                    );
                                    let changes = if approval_patch_changes.is_empty() {
                                        approval_params
                                            .get("changes")
                                            .and_then(|c| c.as_array())
                                            .map(|arr| {
                                                arr.iter()
                                                    .filter_map(|v| v.as_str().map(String::from))
                                                    .collect()
                                            })
                                    } else {
                                        Some(
                                            approval_patch_changes
                                                .iter()
                                                .map(|change| change.path.clone())
                                                .collect(),
                                        )
                                    };
                                    let description: Option<String> = approval_params
                                        .get("description")
                                        .and_then(|v| v.as_str())
                                        .map(String::from);

                                    // Store approval request in session
                                    let approval_request = ApprovalRequest {
                                        id: request_id.clone(),
                                        approval_type: approval_type.clone(),
                                        item_id: item_id.clone(),
                                        thread_id: thread_id.clone(),
                                        run_id: None,
                                        command,
                                        changes,
                                        description: description.clone(),
                                        decision: None,
                                        requested_at: Utc::now(),
                                        resolved_at: None,
                                    };
                                    let approval_id = request_id.clone();
                                    let approval_for_mcp = approval_request.clone();
                                    if let Some(mut session) =
                                        lock_or_warn(&session_clone, "approval request update")
                                    {
                                        session.add_approval_request(approval_request);
                                    }

                                    // Store to MCP in background
                                    let mcp_server_for_approval = mcp_server_clone.clone();
                                    tokio::spawn(async move {
                                        store_to_mcp(
                                            &mcp_server_for_approval,
                                            "approval_request",
                                            &approval_id,
                                            &approval_for_mcp,
                                            debug_mode,
                                        )
                                        .await;
                                    });

                                    let current_mode =
                                        lock_or_warn(&approval_mode, "approval mode read")
                                            .map(|mode| mode.clone())
                                            .unwrap_or_else(|| "ask".to_string());
                                    let approved = if current_mode == "accept" {
                                        // Auto-accept
                                        println!("[Auto-approved]");
                                        true
                                    } else if current_mode == "decline" {
                                        // Auto-decline
                                        println!("[Auto-declined]");
                                        false
                                    } else {
                                        // Ask mode - send to main loop for interactive input
                                        let (oneshot_tx, oneshot_rx) =
                                            tokio::sync::oneshot::channel::<bool>();
                                        let _ = approval_tx_clone
                                            .send((approval_params.clone(), oneshot_tx))
                                            .await;

                                        // Wait for user response
                                        match oneshot_rx.await {
                                            Ok(approved) => {
                                                println!(
                                                    "[User {}]",
                                                    if approved { "approved" } else { "declined" }
                                                );
                                                approved
                                            }
                                            Err(_) => {
                                                println!("[Timeout - auto-approved by default]");
                                                true
                                            }
                                        }
                                    };

                                    // Update approval request with decision and persist to MCP
                                    let approval_to_store = if let Some(mut session) =
                                        lock_or_warn(&session_clone, "approval decision update")
                                    {
                                        if let Some(approval) = session
                                            .approval_requests
                                            .iter_mut()
                                            .find(|a| a.id == request_id)
                                        {
                                            approval.decision = Some(approved);
                                            approval.resolved_at = Some(Utc::now());
                                            Some(approval.clone())
                                        } else {
                                            None
                                        }
                                    } else {
                                        None
                                    };

                                    // Store updated approval to MCP
                                    if let Some(approval) = approval_to_store {
                                        let approval_id = approval.id.clone();
                                        let mcp_server_for_approval = mcp_server_clone.clone();
                                        tokio::spawn(async move {
                                            store_to_mcp(
                                                &mcp_server_for_approval,
                                                "approval_request",
                                                &approval_id,
                                                &approval,
                                                debug_mode,
                                            )
                                            .await;
                                        });
                                    }

                                    let (run_id_for_decision, chosen_patchset_id) =
                                        if let Some(session) = lock_or_warn(
                                            &session_clone,
                                            "approval decision context read",
                                        ) {
                                            let run_id = session.thread.current_turn_id.clone();
                                            let chosen = match approval_type {
                                                ApprovalType::FileChange
                                                | ApprovalType::ApplyPatch => {
                                                    if item_id.is_empty() {
                                                        None
                                                    } else {
                                                        Some(item_id.clone())
                                                    }
                                                }
                                                _ => None,
                                            };
                                            (run_id, chosen)
                                        } else {
                                            (None, None)
                                        };
                                    if let Some(run_id) = run_id_for_decision {
                                        let decision = DecisionEvent {
                                            id: format!("decision_event_{}", request_id),
                                            run_id,
                                            chosen_patchset_id,
                                            approved,
                                            at: Utc::now(),
                                            rationale: description.clone(),
                                        };
                                        let history_writer = history_writer_clone.clone();
                                        let decision_id = decision.id.clone();
                                        tokio::spawn(async move {
                                            history_writer
                                                .write("decision", &decision_id, &decision)
                                                .await;
                                        });
                                    }

                                    // Use the correct resolve method based on the request type
                                    let resolve_method = match mk {
                                        MethodKind::RequestApprovalCommandExecution => {
                                            "item/commandExecution/requestApproval/resolve"
                                        }
                                        MethodKind::RequestApprovalFileChange => {
                                            "item/fileChange/requestApproval/resolve"
                                        }
                                        MethodKind::RequestApprovalExec => {
                                            "exec_approval_request/resolve"
                                        }
                                        MethodKind::RequestApprovalApplyPatch => {
                                            "apply_patch_approval_request/resolve"
                                        }
                                        _ => "requestApproval/resolve",
                                    };

                                    use std::sync::atomic::{AtomicU64, Ordering};
                                    static APPROVAL_REQ_ID: AtomicU64 = AtomicU64::new(10_000);
                                    let resolve_id =
                                        APPROVAL_REQ_ID.fetch_add(1, Ordering::Relaxed);

                                    let approval_msg = CodexMessage::new_request(
                                        resolve_id,
                                        resolve_method,
                                        serde_json::json!({
                                            "requestId": request_id,
                                            "approved": approved
                                        }),
                                    );
                                    let _ = tx_clone.send(approval_msg.to_json()).await;
                                }
                            }
                        }
                    }
                }
                _ => break,
            }
        }
    });

    // Helper to send request
    async fn send_request(
        tx: &mpsc::Sender<String>,
        responses: &Arc<Mutex<std::collections::HashMap<u64, serde_json::Value>>>,
        notifies: &Arc<Mutex<std::collections::HashMap<u64, Arc<tokio::sync::Notify>>>>,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static REQUEST_ID: AtomicU64 = AtomicU64::new(1);
        let id = REQUEST_ID.fetch_add(1, Ordering::Relaxed);

        // Create a notify for this request
        let notify = Arc::new(tokio::sync::Notify::new());
        if let Some(mut notifs) = lock_or_warn(notifies, "send_request notify insert") {
            notifs.insert(id, notify.clone());
        } else {
            return Err("failed to register request notify".to_string());
        }

        let msg = CodexMessage::new_request(id, method, params);
        tx.send(msg.to_json()).await.map_err(|e| e.to_string())?;

        // Wait for response using Notify instead of busy polling
        let timeout = tokio::time::timeout(tokio::time::Duration::from_secs(30), async {
            notify.notified().await;
        });

        match timeout.await {
            Ok(_) => {
                // Response arrived, get it from the map
                let response =
                    if let Some(mut resp) = lock_or_warn(responses, "send_request response read") {
                        resp.remove(&id)
                    } else {
                        None
                    };
                if let Some(response) = response {
                    if let Some(mut notifs) = lock_or_warn(notifies, "send_request notify cleanup")
                    {
                        notifs.remove(&id);
                    }
                    if let Some(error_obj) = response.get("error") {
                        return Err(format!("Error: {}", error_obj));
                    }
                    return Ok(response.get("result").cloned().unwrap_or(response));
                }
                Err("Response not found".to_string())
            }
            Err(_) => {
                // Timeout - clean up
                if let Some(mut notifs) =
                    lock_or_warn(notifies, "send_request notify cleanup timeout")
                {
                    notifs.remove(&id);
                }
                Err("Timeout".to_string())
            }
        }
    }

    // Initialize
    match send_request(
        &tx,
        &responses,
        &notifies,
        "initialize",
        serde_json::json!({
            "capabilities": serde_json::Value::Null,
            "clientInfo": { "name": "libra", "version": env!("CARGO_PKG_VERSION") },
            "cliVersion": env!("CARGO_PKG_VERSION"),
            "cwd": args.cwd,
            "modelProvider": args.model_provider,
            "serviceTier": args.service_tier,
            "personality": args.personality
        }),
    )
    .await
    {
        Ok(_) => println!("Initialized"),
        Err(e) => {
            eprintln!("Init failed: {}", e);
            return Err(anyhow::anyhow!("initialization failed: {}", e));
        }
    }

    // Start thread
    match send_request(
        &tx,
        &responses,
        &notifies,
        "thread/start",
        serde_json::json!({
            "cwd": args.cwd,
            "approvalPolicy": match args.approval.as_str() {
                "ask" => serde_json::json!("on-request"),
                "accept" => serde_json::json!("never"),
                "decline" => serde_json::json!("on-request"),
                _ => serde_json::json!("on-request"),
            },
            "serviceTier": args.service_tier,
            "model": args.model,
            "modelProvider": args.model_provider,
            "personality": args.personality,
            "sandbox": SandboxMode::WorkspaceWrite,
            "developerInstructions": if args.plan_mode {
                serde_json::json!(plan_mode_developer_instructions())
            } else {
                serde_json::Value::Null
            },
            "baseInstructions": if args.plan_mode {
                serde_json::json!(plan_mode_base_instructions())
            } else {
                serde_json::Value::Null
            },
        }),
    )
    .await
    {
        Ok(resp) => {
            // Fallback chain: thread.id -> resp.threadId -> resp.thread_id
            let thread_id_from_response = resp
                .get("thread")
                .and_then(|t| t.get("id"))
                .and_then(|v| v.as_str())
                .or_else(|| resp.get("threadId").and_then(|v| v.as_str()))
                .or_else(|| resp.get("thread_id").and_then(|v| v.as_str()));

            if let Some(id) = thread_id_from_response {
                thread_id = id.to_string();
                println!("Thread: {}", id);
            }
        }
        Err(e) => {
            eprintln!("Thread start failed: {}", e);
            return Err(anyhow::anyhow!("thread start failed: {}", e));
        }
    }

    println!("\n=== Ready! Type your message ===\n");

    // Channel for stdin
    let (stdin_tx, mut stdin_rx) = mpsc::channel::<String>(100);
    // Flag to indicate we're waiting for approval input (vs chat input)
    let waiting_for_approval = Arc::new(Mutex::new(false));
    let waiting_for_approval_clone = waiting_for_approval.clone();

    tokio::spawn(async move {
        use std::io::{BufRead, BufReader};
        let stdin = BufReader::new(std::io::stdin());
        for line in stdin.lines().map_while(Result::ok) {
            let _ = stdin_tx.send(line).await;
        }
    });

    // Main loop
    loop {
        tokio::select! {
            msg = stdin_rx.recv() => {
                if let Some(line) = msg {
                    // Check if we're waiting for approval input
                    let is_approval = lock_or_warn(
                        &waiting_for_approval_clone,
                        "waiting_for_approval read",
                    )
                    .map(|v| *v)
                    .unwrap_or(false);

                    if is_approval {
                        // This input goes to the approval handler - ignore for chat
                        // The approval handler is waiting on a oneshot channel
                        continue;
                    }

                    if line.trim().is_empty() {
                        continue;
                    }

                    match send_request(&tx, &responses, &notifies, "turn/start", serde_json::json!({
                        "input": [{ "type": "text", "text": line }],
                        "threadId": thread_id,
                        "cwd": args.cwd,
                        "model": args.model,
                        "modelProvider": args.model_provider,
                        "serviceTier": args.service_tier,
                        "personality": args.personality,
                        "approvalPolicy": match lock_or_warn(
                            &approval_mode_for_turn,
                            "approval mode read (turn/start)",
                        )
                        .as_ref()
                        .map(|v| v.as_str())
                        .unwrap_or("ask") {
                            "ask" => serde_json::json!("on-request"),
                            "accept" => serde_json::json!("never"),
                            "decline" => serde_json::json!("on-request"),
                            _ => serde_json::json!("on-request"),
                        }
                    })).await {
                        Ok(resp) => println!("Response: {:?}", resp),
                        Err(e) => eprintln!("Error: {}", e),
                    }
                }
            }
            approval_req = approval_rx.recv() => {
                if let Some((params, response_tx)) = approval_req {
                    // Set flag to route stdin to approval
                    if let Some(mut flag) = lock_or_warn(
                        &waiting_for_approval_clone,
                        "waiting_for_approval set true",
                    ) {
                        *flag = true;
                    }

                    // Show approval request details
                    println!("\n⚠️  Approval Request:");
                    if let Some(approval_type) = params.get("type").and_then(|t| t.as_str()) {
                        println!("  Type: {}", approval_type);
                    }
                    if let Some(description) = params.get("description").and_then(|d| d.as_str()) {
                        println!("  Description: {}", description);
                    }
                    if let Some(title) = params.get("title").and_then(|t| t.as_str()) {
                        println!("  Title: {}", title);
                    }
                    // Show more details if available
                    if let Some(details) = params.get("details") {
                        println!("  Details: {}", details);
                    }

                    println!("\n  [a]ccept / [d]ecline / [A]ccept All / [D]ecline All: ");

                    // Read user input from the shared stdin channel instead of creating a new reader
                    let approved = if let Some(input) = stdin_rx.recv().await {
                        let choice = input.trim().to_lowercase();
                        match choice.as_str() {
                            "a" | "accept" => {
                                println!("  → Accepted");
                                true
                            }
                            "d" | "decline" => {
                                println!("  → Declined");
                                false
                            }
                            "aa" | "accept all" => {
                                println!("  → Accepted (will auto-accept future)");
                                if let Some(mut mode) = lock_or_warn(
                                    &approval_mode_clone,
                                    "approval mode set accept",
                                ) {
                                    *mode = "accept".to_string();
                                }
                                true
                            }
                            "dd" | "decline all" => {
                                println!("  → Declined (will auto-decline future)");
                                if let Some(mut mode) = lock_or_warn(
                                    &approval_mode_clone,
                                    "approval mode set decline",
                                ) {
                                    *mode = "decline".to_string();
                                }
                                false
                            }
                            _ => {
                                println!("  → Default accept");
                                true
                            }
                        }
                    } else {
                        println!("  → Default accept (no input)");
                        true
                    };

                    let _ = response_tx.send(approved);
                    println!();

                    // Clear flag to resume chat input
                    if let Some(mut flag) = lock_or_warn(
                        &waiting_for_approval_clone,
                        "waiting_for_approval set false",
                    ) {
                        *flag = false;
                    }
                }
            }
            _ = tokio::time::sleep(tokio::time::Duration::from_secs(1)) => {}
        }
    }
    #[allow(unreachable_code)]
    Ok(())
}
