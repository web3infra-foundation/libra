//! Agent Codex command - directly connect to Codex app-server via WebSocket.

pub mod history;
pub mod model;
pub mod protocol;
pub mod schema_v2;
pub mod schema_v2_generated;
pub mod types;
pub mod view;

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use anyhow;
use chrono::Utc;
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use history::{HistoryReader, HistoryRecorder, HistoryWriter};
use model::{
    ContextFrameEvent, ContextSnapshot, DecisionEvent, EvidenceEvent, IntentEvent, IntentSnapshot,
    PatchSetSnapshot, PlanSnapshot, PlanStepEvent, PlanStepSnapshot, ProvenanceSnapshot, RunEvent,
    RunSnapshot, RunUsage, TaskEvent, TaskSnapshot,
};
use protocol::MethodKind;
use schema_v2::*;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
pub use types::*;

use crate::{
    internal::{
        ai::{history::HistoryManager, mcp::server::LibraMcpServer},
        db,
    },
    utils::{storage_ext::StorageExt, util::try_get_storage_path},
};

const CODEX_WS_URL: &str = "ws://127.0.0.1:8080";

fn lock_or_warn<'a, T>(mutex: &'a Arc<Mutex<T>>, context: &str) -> Option<MutexGuard<'a, T>> {
    match mutex.lock() {
        Ok(guard) => Some(guard),
        Err(e) => {
            eprintln!("[WARN] {context}: failed to lock mutex: {e}");
            None
        }
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

    /// Debug mode: print collected data
    #[arg(long, default_value = "false")]
    pub debug: bool,
}

/// Store an object to MCP storage
pub async fn store_to_mcp<T: serde::Serialize + Send + Sync>(
    mcp_server: &Arc<LibraMcpServer>,
    object_type: &str,
    object_id: &str,
    object: &T,
    debug: bool,
) {
    if let Some(storage) = &mcp_server.storage {
        match storage.put_json(object).await {
            Ok(hash) => {
                // Also add to history if available
                if let Some(history) = &mcp_server.intent_history_manager {
                    let should_append = match history.get_object_hash(object_type, object_id).await
                    {
                        Ok(Some(existing)) => existing != hash,
                        Ok(None) => true,
                        Err(e) => {
                            eprintln!(
                                "[WARN] Failed to check history for {object_type}/{object_id}: {e}"
                            );
                            true
                        }
                    };
                    if should_append
                        && let Err(e) = history.append(object_type, object_id, hash).await
                    {
                        eprintln!("[WARN] Failed to append to history: {e}");
                    }
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
        if !rebuild.thread.thread_id.is_empty() {
            session_guard.thread.id = rebuild.thread.thread_id.clone();
        }
        session_guard.thread.current_turn_id = rebuild.scheduler.active_run_id.clone();
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
                status: PlanStatus::Pending,
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
                status: TaskStatus::Pending,
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
                status: RunStatus::Pending,
                started_at: r.started_at,
                completed_at: None,
            })
            .collect();
        session_guard.patchsets = rebuild
            .thread
            .patchsets
            .values()
            .map(|p| PatchSet {
                id: p.id.clone(),
                run_id: p.run_id.clone(),
                thread_id: p.thread_id.clone(),
                changes: Vec::new(),
                status: PatchStatus::Pending,
                created_at: p.created_at,
            })
            .collect();
    }

    // Spawn writer task
    let _write_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                Some(msg) = rx.recv() => {
                    if write.send(Message::Text(msg)).await.is_err() {
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
                                        println!("\nPlan Updated:");
                                        if let Some(exp) = explanation.as_ref() {
                                            println!("  Explanation: {}", exp);
                                        }
                                        for item in plan_steps.iter() {
                                            let status_string = item.status.as_str();
                                            let step_string = item.step.as_str();
                                            let marker = match status_string {
                                                "completed" => "[x]",
                                                "inProgress" => "[>]",
                                                _ => "[ ]",
                                            };
                                            println!("  {} {}", marker, step_string);

                                            // Store each plan step as a Plan
                                            let plan_id =
                                                format!("plan_{}_{}", turn_id, step_string);
                                            let plan_status = match status_string {
                                                "completed" => PlanStatus::Completed,
                                                "inProgress" => PlanStatus::InProgress,
                                                _ => PlanStatus::Pending,
                                            };
                                            let plan = Plan {
                                                id: plan_id.clone(),
                                                text: step_string.to_string(),
                                                intent_id: lock_or_warn(
                                                    &session_clone,
                                                    "intent lookup for plan",
                                                )
                                                .and_then(|s| {
                                                    s.intents.last().map(|i| i.id.clone())
                                                }),
                                                thread_id: thread_id.to_string(),
                                                turn_id: Some(turn_id.to_string()),
                                                status: plan_status,
                                                created_at: Utc::now(),
                                            };
                                            let plan_snapshot = PlanSnapshot {
                                                id: plan_id.clone(),
                                                thread_id: thread_id.to_string(),
                                                intent_id: plan.intent_id.clone(),
                                                turn_id: Some(turn_id.to_string()),
                                                step_text: step_string.to_string(),
                                                created_at: Utc::now(),
                                            };
                                            let plan_step_snapshot = PlanStepSnapshot {
                                                id: plan_id.clone(),
                                                plan_id: plan_id.clone(),
                                                text: step_string.to_string(),
                                                created_at: Utc::now(),
                                            };
                                            let plan_step_event = PlanStepEvent {
                                                id: format!("plan_step_event_{}", plan_id),
                                                plan_id: plan_id.clone(),
                                                step_id: plan_id.clone(),
                                                status: status_string.to_string(),
                                                at: Utc::now(),
                                                run_id: Some(turn_id.to_string()),
                                            };
                                            let history_writer = history_writer_clone.clone();
                                            let plan_id_for_write = plan_id.clone();
                                            tokio::spawn(async move {
                                                history_writer
                                                    .write(
                                                        "plan_snapshot",
                                                        &plan_id_for_write,
                                                        &plan_snapshot,
                                                    )
                                                    .await;
                                                history_writer
                                                    .write(
                                                        "plan_step_snapshot",
                                                        &plan_id_for_write,
                                                        &plan_step_snapshot,
                                                    )
                                                    .await;
                                                history_writer
                                                    .write(
                                                        "plan_step_event",
                                                        &plan_step_event.id,
                                                        &plan_step_event,
                                                    )
                                                    .await;
                                            });
                                            let plan_for_mcp = plan.clone();
                                            let status_for_event = status_string.to_string();
                                            let step_for_event = step_string.to_string();
                                            if status_string == "pending"
                                                && let Some(mut session) = lock_or_warn(
                                                    &session_clone,
                                                    "scheduler ready queue update",
                                                )
                                                && session.tasks.iter().all(|t| t.id != plan_id)
                                            {
                                                session.tasks.push(Task {
                                                    id: plan_id.clone(),
                                                    tool_name: Some(step_string.to_string()),
                                                    plan_id: Some(plan_id.clone()),
                                                    thread_id: thread_id.to_string(),
                                                    turn_id: Some(turn_id.to_string()),
                                                    status: TaskStatus::Pending,
                                                    created_at: Utc::now(),
                                                });
                                            }
                                            if let Some(mut session) =
                                                lock_or_warn(&session_clone, "plan update")
                                            {
                                                session.add_plan(plan);
                                            }

                                            // Store to MCP in background
                                            let mcp_server_for_plan = mcp_server_clone.clone();
                                            let history = history_recorder_clone.clone();
                                            tokio::spawn(async move {
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
                                                        status_for_event,
                                                        serde_json::json!({"step": step_for_event}),
                                                    )
                                                    .await;
                                            });
                                        }
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
                                    let thread_id = params
                                        .get("threadId")
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    let task_id = params
                                        .get("taskId")
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    if let Some(mut session) =
                                        lock_or_warn(&session_clone, "task started update")
                                        && let Some(task) =
                                            session.tasks.iter_mut().find(|t| t.id == task_id)
                                    {
                                        task.status = TaskStatus::InProgress;
                                    }
                                    let _intent_id_for_event = if let Some(session) = lock_or_warn(
                                        &session_clone,
                                        "intent lookup for task completion",
                                    ) {
                                        let mut intent_id = None;
                                        if let Some(task) =
                                            session.tasks.iter().find(|t| t.id == task_id)
                                            && let Some(plan_id) = task.plan_id.as_ref()
                                            && let Some(plan) =
                                                session.plans.iter().find(|p| &p.id == plan_id)
                                        {
                                            intent_id = plan.intent_id.clone();
                                        }
                                        intent_id
                                            .or_else(|| {
                                                session.intents.last().map(|i| i.id.clone())
                                            })
                                            .unwrap_or_default()
                                    } else {
                                        String::new()
                                    };
                                    let task_name = params
                                        .get("taskName")
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("")
                                        .to_string();

                                    println!(
                                        "\n🚀 Task Started: {} (thread: {})",
                                        task_name,
                                        &thread_id[..8.min(thread_id.len())]
                                    );

                                    // Store Task (tool_name stores task name)
                                    let task = Task {
                                        id: task_id.clone(),
                                        tool_name: Some(task_name.clone()),
                                        plan_id: None,
                                        thread_id: thread_id.clone(),
                                        turn_id: None,
                                        status: TaskStatus::InProgress,
                                        created_at: Utc::now(),
                                    };
                                    let task_snapshot = TaskSnapshot {
                                        id: task_id.clone(),
                                        thread_id: thread_id.clone(),
                                        plan_id: None,
                                        turn_id: None,
                                        title: Some(task_name.clone()),
                                        created_at: Utc::now(),
                                    };
                                    let task_event = TaskEvent {
                                        id: format!("task_event_{}", task_id),
                                        task_id: task_id.clone(),
                                        status: "in_progress".to_string(),
                                        at: Utc::now(),
                                        run_id: None,
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
                                    let task_id = params
                                        .get("taskId")
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    if let Some(mut session) =
                                        lock_or_warn(&session_clone, "task completed update")
                                        && let Some(task) =
                                            session.tasks.iter_mut().find(|t| t.id == task_id)
                                    {
                                        task.status = TaskStatus::Completed;
                                    }
                                    let intent_id_for_event = if let Some(session) = lock_or_warn(
                                        &session_clone,
                                        "intent lookup for task completion",
                                    ) {
                                        if let Some(task) =
                                            session.tasks.iter().find(|t| t.id == task_id)
                                        {
                                            if let Some(plan_id) = task.plan_id.as_ref() {
                                                if let Some(plan) =
                                                    session.plans.iter().find(|p| &p.id == plan_id)
                                                {
                                                    if let Some(intent_id) = plan.intent_id.as_ref()
                                                    {
                                                        intent_id.clone()
                                                    } else {
                                                        session
                                                            .intents
                                                            .last()
                                                            .map(|i| i.id.clone())
                                                            .unwrap_or_default()
                                                    }
                                                } else {
                                                    session
                                                        .intents
                                                        .last()
                                                        .map(|i| i.id.clone())
                                                        .unwrap_or_default()
                                                }
                                            } else {
                                                session
                                                    .intents
                                                    .last()
                                                    .map(|i| i.id.clone())
                                                    .unwrap_or_default()
                                            }
                                        } else {
                                            session
                                                .intents
                                                .last()
                                                .map(|i| i.id.clone())
                                                .unwrap_or_default()
                                        }
                                    } else {
                                        String::new()
                                    };
                                    let task_event = TaskEvent {
                                        id: format!("task_event_completed_{}", task_id),
                                        task_id,
                                        status: "completed".to_string(),
                                        at: Utc::now(),
                                        run_id: None,
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
                                                        if args_str.len() > 200 {
                                                            println!(
                                                                "    Args: {}...",
                                                                &args_str[..200]
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
                                                    let tool_id = item_id.clone();
                                                    let tool_for_mcp = invocation.clone();
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
                                                    });
                                                }
                                                "commandExecution" => {
                                                    let cmd = item
                                                        .get("command")
                                                        .and_then(|c| c.as_str())
                                                        .unwrap_or("")
                                                        .to_string();
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
                                                    }

                                                    let mcp_server_for_cmd =
                                                        mcp_server_clone.clone();
                                                    let history = history_recorder_clone.clone();
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
                                                    tokio::spawn(async move {
                                                        store_to_mcp(
                                                            &mcp_server_for_dyn,
                                                            "tool_invocation",
                                                            &dyn_tool_id,
                                                            &dyn_tool_for_mcp,
                                                            debug_mode,
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
                                                    tokio::spawn(async move {
                                                        store_to_mcp(
                                                            &mcp_server_for_ws,
                                                            "tool_invocation",
                                                            &ws_id,
                                                            &ws_for_mcp,
                                                            debug_mode,
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
                                                    let truncated = if content.len() > 50 {
                                                        content[..50].to_string()
                                                    } else {
                                                        content.clone()
                                                    };
                                                    println!("  User: {}", truncated);

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
                                                        created_at: Utc::now(),
                                                    };
                                                    let intent_event = IntentEvent {
                                                        id: format!("intent_event_{}", item_id),
                                                        intent_id: item_id.clone(),
                                                        status: "created".to_string(),
                                                        at: Utc::now(),
                                                        next_intent_id: None,
                                                    };
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
                                                    println!(
                                                        "
  ?? Agent Response started
"
                                                    );

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
                                                    tokio::spawn(async move {
                                                        store_to_mcp(
                                                            &mcp_server_for_inv,
                                                            "tool_invocation",
                                                            &inv_id,
                                                            &inv_for_mcp,
                                                            debug_mode,
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
                                                        if result_str.len() > 100 {
                                                            println!(
                                                                " | Result: {}...",
                                                                &result_str[..100]
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
                                                        let history =
                                                            history_recorder_clone.clone();
                                                        let inv_id = inv.id.clone();
                                                        let tool_name = tool.clone();
                                                        tokio::spawn(async move {
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
                                                        });
                                                    }
                                                }
                                                "commandExecution" => {
                                                    let cmd = item
                                                        .get("command")
                                                        .and_then(|c| c.as_str())
                                                        .unwrap_or("")
                                                        .to_string();
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
                                                        let mcp_server_for_inv =
                                                            mcp_server_clone.clone();
                                                        let history =
                                                            history_recorder_clone.clone();
                                                        let history_writer =
                                                            history_writer_clone.clone();
                                                        let inv_id = item_id.clone();
                                                        let cmd_name = cmd.clone();
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

                                                    let changes: Vec<FileChange> = item
                                                        .get("changes")
                                                        .and_then(|c| c.as_array())
                                                        .map(|arr| {
                                                            arr.iter()
                                                                .filter_map(|change| {
                                                                    let path = change
                                                                        .get("path")?
                                                                        .as_str()?
                                                                        .to_string();
                                                                    let diff = change
                                                                        .get("diff")
                                                                        .and_then(|d| d.as_str())
                                                                        .unwrap_or("")
                                                                        .to_string();
                                                                    let change_type = change
                                                                        .get("change_type")
                                                                        .or_else(|| {
                                                                            change.get("changeType")
                                                                        })
                                                                        .or_else(|| {
                                                                            change
                                                                                .get("kind")
                                                                                .and_then(|k| {
                                                                                    k.get("type")
                                                                                })
                                                                        })
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
                                                        .unwrap_or_default();

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
                                                    ) && let Some(patchset) = session
                                                        .patchsets
                                                        .iter_mut()
                                                        .find(|p| p.id == item_id)
                                                    {
                                                        patchset.status = match status.as_str() {
                                                            "completed" => PatchStatus::Completed,
                                                            "failed" => PatchStatus::Failed,
                                                            "declined" => PatchStatus::Declined,
                                                            _ => PatchStatus::Completed,
                                                        };
                                                        patchset.changes = changes.clone();
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
                                                        let mcp_server_for_ps =
                                                            mcp_server_clone.clone();
                                                        let ps_id = item_id.clone();
                                                        let status_string = status.clone();
                                                        let history =
                                                            history_recorder_clone.clone();
                                                        let history_writer =
                                                            history_writer_clone.clone();
                                                        let files = file_count;
                                                        let patchset_snapshot = PatchSetSnapshot {
                                                            id: ps_id.clone(),
                                                            run_id: _turn_id.to_string(),
                                                            thread_id: _thread_id.to_string(),
                                                            created_at: Utc::now(),
                                                        };
                                                        let evidence = EvidenceEvent {
                                                            id: format!("evidence_{}", ps_id),
                                                            run_id: _turn_id.to_string(),
                                                            at: Utc::now(),
                                                            kind: "patchset".to_string(),
                                                            data: serde_json::json!({"files": files}),
                                                        };
                                                        tokio::spawn(async move {
                                                            store_to_mcp(
                                                                &mcp_server_for_ps,
                                                                "patchset",
                                                                &ps_id,
                                                                &patchset,
                                                                debug_mode,
                                                            )
                                                            .await;
                                                            history
                                                                .event(
                                                                    history::EventKind::ToolInvocationStatus,
                                                                    &ps_id,
                                                                    status_string,
                                                                    serde_json::json!({"files": files}),
                                                                )
                                                                .await;
                                                            history_writer
                                                                .write(
                                                                    "patchset_snapshot",
                                                                    &ps_id,
                                                                    &patchset_snapshot,
                                                                )
                                                                .await;
                                                            history_writer
                                                                .write(
                                                                    "evidence",
                                                                    &evidence.id,
                                                                    &evidence,
                                                                )
                                                                .await;
                                                        });
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
                                                        if result_str.len() > 100 {
                                                            println!(
                                                                " | Result: {}...",
                                                                &result_str[..100]
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
                                                        let inv_id = item_id.clone();
                                                        tokio::spawn(async move {
                                                            store_to_mcp(
                                                                &mcp_server_for_inv,
                                                                "tool_invocation",
                                                                &inv_id,
                                                                &invocation,
                                                                debug_mode,
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
                                                        let inv_id = item_id.clone();
                                                        tokio::spawn(async move {
                                                            store_to_mcp(
                                                                &mcp_server_for_inv,
                                                                "tool_invocation",
                                                                &inv_id,
                                                                &invocation,
                                                                debug_mode,
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
                                                    if let Some(mut session) = lock_or_warn(
                                                        &session_clone,
                                                        "web search completed update",
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
                                                    if let Some(mut session) = lock_or_warn(
                                                        &session_clone,
                                                        "agent message completed update",
                                                    ) && let Some(msg) = session
                                                        .agent_messages
                                                        .iter_mut()
                                                        .find(|m| m.id == item_id)
                                                    {
                                                        msg.content = content.clone();
                                                        if !msg.content.is_empty() {
                                                            println!(
                                                                "
                                                                ?? Agent: {}
                                                                ",
                                                                msg.content
                                                            );
                                                        }
                                                    }
                                                    println!("  ?? Agent Response completed");
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
                                        .and_then(|v| v.as_str())
                                        .map(String::from)
                                        .unwrap_or_default();

                                    // Get thread_id if available
                                    let thread_id = approval_params
                                        .get("threadId")
                                        .and_then(|v| v.as_str())
                                        .map(String::from)
                                        .unwrap_or_default();

                                    // Get command or changes from approval_params
                                    let command = approval_params
                                        .get("command")
                                        .and_then(|v| v.as_str())
                                        .map(String::from);
                                    let changes = approval_params
                                        .get("changes")
                                        .and_then(|c| c.as_array())
                                        .map(|arr| {
                                            arr.iter()
                                                .filter_map(|v| v.as_str().map(String::from))
                                                .collect()
                                        });
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
            "sandbox": serde_json::Value::Null,
            "developerInstructions": serde_json::Value::Null,
            "baseInstructions": serde_json::Value::Null,
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
