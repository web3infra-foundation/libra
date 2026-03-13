//! Agent Codex command - directly connect to Codex app-server via WebSocket.

pub mod protocol;
pub mod types;

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use chrono::Utc;
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
pub use types::*;

use crate::{
    cli_error,
    internal::{
        ai::{history::HistoryManager, mcp::server::LibraMcpServer},
        db,
    },
    utils::{storage_ext::StorageExt, util::try_get_storage_path},
};

const CODEX_WS_URL: &str = "ws://127.0.0.1:8080";

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

    /// Debug mode: print collected data
    #[arg(long, default_value = "false")]
    pub debug: bool,
}

/// Store an object to MCP storage
async fn store_to_mcp<T: serde::Serialize + Send + Sync>(
    mcp_server: &Arc<LibraMcpServer>,
    object_type: &str,
    object_id: &str,
    object: &T,
) {
    if let Some(storage) = &mcp_server.storage {
        match storage.put_json(object).await {
            Ok(hash) => {
                // Also add to history if available
                if let Some(history) = &mcp_server.intent_history_manager
                    && let Err(e) = history.append(object_type, object_id, hash).await
                {
                    eprintln!("[WARN] Failed to append to history: {e}");
                }
                eprintln!("[DEBUG] Stored {object_type} {object_id} to MCP (hash: {hash})");
            }
            Err(e) => {
                eprintln!("[WARN] Failed to store {object_type} to MCP: {e}");
            }
        }
    } else {
        eprintln!("[WARN] MCP storage not available");
    }
}

pub async fn execute(args: AgentCodexArgs) {
    // Initialize MCP server first
    let working_dir = PathBuf::from(&args.cwd);
    let mcp_server = init_mcp_server(&working_dir).await;
    println!("MCP server initialized.");

    println!("Connecting to Codex at {}...", args.url);

    let (ws_stream, _) = match connect_async(args.url.as_str()).await {
        Ok(s) => s,
        Err(e) => {
            cli_error!(e, "error: failed to connect to Codex at {}", args.url);
            return;
        }
    };

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

    // Session data storage (for MCP query)
    let session: Arc<Mutex<CodexSession>> = Arc::new(Mutex::new(CodexSession::new()));

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
    let tx_clone = tx.clone();
    let approval_tx_clone = approval_tx.clone();
    let approval_mode = Arc::new(Mutex::new(args.approval.clone()));
    let approval_mode_clone = approval_mode.clone();
    let debug_mode = args.debug;
    let session_clone = session.clone();
    let mcp_server_clone = mcp_server.clone();
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
                                    let mut resp = responses_clone.lock().unwrap();
                                    resp.insert(id, json);
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
                            // Filter out truly noisy notifications
                            let is_noise = method_str.contains("token/usage");
                            let show_notification = !is_noise
                                && (method_str.contains("initialized")
                                    || method_str.contains("task_started")
                                    || method_str.contains("task_complete")
                                    || method_str.contains("agent_reasoning")
                                    || method_str.contains("turn/completed")
                                    || method_str.contains("turn_started")
                                    || method_str.contains("turn/plan")
                                    || method_str.contains("thread/started")
                                    || method_str.contains("thread/status")
                                    || method_str.contains("item/")
                                    || method_str.contains("requestApproval")
                                    || method_str.contains("reasoning"));

                            // Extract and print useful info based on notification type
                            if let Some(params) = json.get("params") {
                                // Show hierarchical flow: Thread → Turn → Plan → Item → Detail
                                if method_str.contains("thread/started") {
                                    // params: { thread: { threadId, ... } } or { threadId } or { thread_id }
                                    // Fallback chain: thread.threadId -> params.threadId -> params.thread_id
                                    let thread_id = params
                                        .get("thread")
                                        .and_then(|t| t.get("threadId"))
                                        .and_then(|t| t.as_str())
                                        .or_else(|| params.get("threadId").and_then(|t| t.as_str()))
                                        .or_else(|| {
                                            params.get("thread_id").and_then(|t| t.as_str())
                                        })
                                        .unwrap_or("");
                                    println!(
                                        "\n=== New Thread: {} ===",
                                        &thread_id[..8.min(thread_id.len())]
                                    );

                                    // Store thread in session
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
                                    session_clone.lock().unwrap().update_thread(thread);

                                    // Store to MCP in background
                                    let mcp_server_for_thread = mcp_server_clone.clone();
                                    tokio::spawn(async move {
                                        store_to_mcp(
                                            &mcp_server_for_thread,
                                            "thread",
                                            &thread_id_for_mcp,
                                            &thread_for_mcp,
                                        )
                                        .await;
                                    });
                                } else if method_str.contains("thread/status/changed") {
                                    // params: { threadId, status }
                                    let thread_id = params
                                        .get("threadId")
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("");
                                    let status =
                                        params.get("status").and_then(|s| s.as_str()).unwrap_or("");

                                    let new_status = match status {
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

                                    let mut session = session_clone.lock().unwrap();
                                    session.thread.status = new_status;
                                    session.thread.updated_at = Utc::now();
                                } else if method_str.contains("thread/name/updated") {
                                    // params: { threadId, name }
                                    let thread_id = params
                                        .get("threadId")
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("");
                                    let name = params
                                        .get("name")
                                        .and_then(|n| n.as_str())
                                        .map(String::from);

                                    if debug_mode {
                                        eprintln!(
                                            "[DEBUG] Thread name updated: {} -> {:?}",
                                            thread_id, name
                                        );
                                    }

                                    let mut session = session_clone.lock().unwrap();
                                    session.thread.name = name;
                                    session.thread.updated_at = Utc::now();
                                } else if method_str.contains("thread/archived") {
                                    // params: { threadId }
                                    let thread_id = params
                                        .get("threadId")
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("");

                                    if debug_mode {
                                        eprintln!("[DEBUG] Thread archived: {}", thread_id);
                                    }

                                    let mut session = session_clone.lock().unwrap();
                                    session.thread.status = ThreadStatus::Archived;
                                    session.thread.updated_at = Utc::now();
                                } else if method_str.contains("thread/closed") {
                                    // params: { threadId }
                                    let thread_id = params
                                        .get("threadId")
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("");

                                    if debug_mode {
                                        eprintln!("[DEBUG] Thread closed: {}", thread_id);
                                    }

                                    let mut session = session_clone.lock().unwrap();
                                    session.thread.status = ThreadStatus::Closed;
                                    session.thread.updated_at = Utc::now();
                                } else if method_str.contains("turn/started")
                                    || method_str.contains("turnStarted")
                                {
                                    // params: { turn: { id, ... }, threadId }
                                    let turn_id = params
                                        .get("turn")
                                        .and_then(|t| t.get("id"))
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("");
                                    let thread_id = params
                                        .get("threadId")
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("");
                                    println!(
                                        "\n--- Turn started: {} (thread: {}) ---",
                                        &turn_id[..8.min(turn_id.len())],
                                        &thread_id[..8.min(thread_id.len())]
                                    );

                                    // Store run in session
                                    let run = Run {
                                        id: turn_id.to_string(),
                                        thread_id: thread_id.to_string(),
                                        status: RunStatus::InProgress,
                                        started_at: Utc::now(),
                                        completed_at: None,
                                    };
                                    let run_id = turn_id.to_string();
                                    let run_for_mcp = run.clone();
                                    let mut session = session_clone.lock().unwrap();
                                    session.add_run(run);
                                    // Update thread's current turn
                                    session.thread.current_turn_id = Some(turn_id.to_string());
                                    session.thread.status = ThreadStatus::Running;

                                    // Store to MCP in background
                                    let mcp_server_for_run = mcp_server_clone.clone();
                                    tokio::spawn(async move {
                                        store_to_mcp(
                                            &mcp_server_for_run,
                                            "run",
                                            &run_id,
                                            &run_for_mcp,
                                        )
                                        .await;
                                    });
                                    session.thread.updated_at = Utc::now();
                                } else if method_str.contains("turn/completed")
                                    || method_str.contains("turnCompleted")
                                {
                                    // params: { threadId, turn: { id, ... } }
                                    let turn_id = params
                                        .get("turn")
                                        .and_then(|t| t.get("id"))
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("");
                                    if !turn_id.is_empty() {
                                        println!(
                                            "--- Turn completed: {} ---",
                                            &turn_id[..8.min(turn_id.len())]
                                        );

                                        // Update run status in session and persist to MCP
                                        let run_to_store = {
                                            let mut session = session_clone.lock().unwrap();
                                            if let Some(run) =
                                                session.runs.iter_mut().find(|r| r.id == turn_id)
                                            {
                                                run.status = RunStatus::Completed;
                                                run.completed_at = Some(Utc::now());
                                                Some(run.clone())
                                            } else {
                                                None
                                            }
                                        };
                                        session_clone.lock().unwrap().thread.updated_at =
                                            Utc::now();

                                        // Store updated run to MCP
                                        if let Some(run) = run_to_store {
                                            let run_id = run.id.clone();
                                            let mcp_server_for_run = mcp_server_clone.clone();
                                            tokio::spawn(async move {
                                                store_to_mcp(
                                                    &mcp_server_for_run,
                                                    "run",
                                                    &run_id,
                                                    &run,
                                                )
                                                .await;
                                            });
                                        }
                                    } else {
                                        println!("--- Turn completed ---");
                                    }
                                } else if method_str.contains("tokenUsage") {
                                    // params: { threadId, turnId, tokenUsage: { last, total, modelContextWindow? } }
                                    let thread_id = params
                                        .get("threadId")
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("");
                                    let turn_id =
                                        params.get("turnId").and_then(|t| t.as_str()).unwrap_or("");

                                    // Parse token usage
                                    let last = params
                                        .get("tokenUsage")
                                        .and_then(|tu| tu.get("last"))
                                        .cloned()
                                        .unwrap_or(serde_json::Value::Null);
                                    let total = params
                                        .get("tokenUsage")
                                        .and_then(|tu| tu.get("total"))
                                        .cloned()
                                        .unwrap_or(serde_json::Value::Null);
                                    let model_context_window = params
                                        .get("tokenUsage")
                                        .and_then(|tu| tu.get("modelContextWindow"))
                                        .and_then(|m| m.as_i64());

                                    let parse_token = |v: &serde_json::Value| -> TokenUsage {
                                        TokenUsage {
                                            cached_input_tokens: v
                                                .get("cachedInputTokens")
                                                .and_then(|c| c.as_i64()),
                                            input_tokens: v
                                                .get("inputTokens")
                                                .and_then(|i| i.as_i64()),
                                            output_tokens: v
                                                .get("outputTokens")
                                                .and_then(|o| o.as_i64()),
                                            reasoning_output_tokens: v
                                                .get("reasoningOutputTokens")
                                                .and_then(|r| r.as_i64()),
                                            total_tokens: v
                                                .get("totalTokens")
                                                .and_then(|t| t.as_i64()),
                                        }
                                    };

                                    let usage = TurnTokenUsage {
                                        thread_id: thread_id.to_string(),
                                        turn_id: turn_id.to_string(),
                                        last: parse_token(&last),
                                        total: parse_token(&total),
                                        model_context_window,
                                        updated_at: Utc::now(),
                                    };

                                    // Debug output
                                    if debug_mode {
                                        eprintln!(
                                            "[DEBUG] TokenUsage: turn={}, total_tokens={}",
                                            turn_id,
                                            usage.total.total_tokens.unwrap_or(0)
                                        );
                                    }

                                    let usage_for_mcp = usage.clone();
                                    let usage_turn_id = turn_id.to_string();
                                    session_clone.lock().unwrap().add_token_usage(usage);

                                    // Store to MCP in background
                                    let mcp_server_for_usage = mcp_server_clone.clone();
                                    tokio::spawn(async move {
                                        store_to_mcp(
                                            &mcp_server_for_usage,
                                            "run_usage",
                                            &usage_turn_id,
                                            &usage_for_mcp,
                                        )
                                        .await;
                                    });
                                } else if method_str.contains("turn/plan/updated")
                                    || method_str.contains("plan/updated")
                                {
                                    // params: { plan: [...], threadId, turnId, explanation? }
                                    let thread_id = params
                                        .get("threadId")
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("");
                                    let turn_id =
                                        params.get("turnId").and_then(|t| t.as_str()).unwrap_or("");
                                    if let Some(plan) = params.get("plan") {
                                        let explanation =
                                            params.get("explanation").and_then(|e| e.as_str());
                                        println!("\n📋 Plan Updated:");
                                        if let Some(exp) = explanation {
                                            println!("  Explanation: {}", exp);
                                        }
                                        if let Ok(plan_array) =
                                            serde_json::from_str::<Vec<serde_json::Value>>(
                                                &plan.to_string(),
                                            )
                                        {
                                            for item in plan_array.iter() {
                                                let status = item
                                                    .get("status")
                                                    .and_then(|s| s.as_str())
                                                    .unwrap_or("unknown");
                                                let step = item
                                                    .get("step")
                                                    .and_then(|s| s.as_str())
                                                    .unwrap_or("");
                                                let marker = match status {
                                                    "completed" => "✓",
                                                    "inProgress" => "▶",
                                                    _ => "○",
                                                };
                                                println!("  {} {}", marker, step);

                                                // Store each plan step as a Plan
                                                let plan_id = format!("plan_{}_{}", turn_id, step);
                                                let plan_status = match status {
                                                    "completed" => PlanStatus::Completed,
                                                    "inProgress" => PlanStatus::InProgress,
                                                    _ => PlanStatus::Pending,
                                                };
                                                let plan = Plan {
                                                    id: plan_id.clone(),
                                                    text: step.to_string(),
                                                    intent_id: None,
                                                    thread_id: thread_id.to_string(),
                                                    turn_id: Some(turn_id.to_string()),
                                                    status: plan_status,
                                                    created_at: Utc::now(),
                                                };
                                                let plan_for_mcp = plan.clone();
                                                session_clone.lock().unwrap().add_plan(plan);

                                                // Store to MCP in background
                                                let mcp_server_for_plan = mcp_server_clone.clone();
                                                tokio::spawn(async move {
                                                    store_to_mcp(
                                                        &mcp_server_for_plan,
                                                        "plan",
                                                        &plan_id,
                                                        &plan_for_mcp,
                                                    )
                                                    .await;
                                                });
                                            }
                                        }
                                    }
                                } else if method_str.contains("initialized") {
                                    // Server initialized notification (after client sends initialize request)
                                    println!("[Codex] Server initialized");
                                } else if method_str.contains("codex/event/task_started") {
                                    // Task started - top level notification
                                    let thread_id = params
                                        .get("threadId")
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("");
                                    let task_id =
                                        params.get("taskId").and_then(|t| t.as_str()).unwrap_or("");
                                    let task_name = params
                                        .get("taskName")
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("");

                                    println!(
                                        "\n🚀 Task Started: {} (thread: {})",
                                        task_name,
                                        &thread_id[..8.min(thread_id.len())]
                                    );

                                    // Store Task (tool_name stores task name)
                                    let task = Task {
                                        id: task_id.to_string(),
                                        tool_name: Some(task_name.to_string()),
                                        plan_id: None,
                                        thread_id: thread_id.to_string(),
                                        turn_id: None,
                                        status: TaskStatus::InProgress,
                                        created_at: Utc::now(),
                                    };
                                    let task_id_for_mcp = task_id.to_string();
                                    let task_for_mcp = task.clone();

                                    let mut session = session_clone.lock().unwrap();
                                    session.add_task(task);

                                    // Store to MCP in background
                                    drop(session);
                                    let mcp_server_for_task = mcp_server_clone.clone();
                                    tokio::spawn(async move {
                                        store_to_mcp(
                                            &mcp_server_for_task,
                                            "task",
                                            &task_id_for_mcp,
                                            &task_for_mcp,
                                        )
                                        .await;
                                    });
                                } else if method_str.contains("codex/event/task_complete") {
                                    // Task completed - top level notification
                                    println!("\n✅ Task Completed");
                                } else if show_notification && !method_str.contains("item/") {
                                    // println!("[Codex] {}", method_str);
                                }
                                // Handle thread/started
                                if method_str.contains("item/started") {
                                    // Get common fields
                                    let thread_id = params
                                        .get("threadId")
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("");
                                    let turn_id =
                                        params.get("turnId").and_then(|t| t.as_str()).unwrap_or("");

                                    // params.item.type contains the type
                                    if let Some(item) = params.get("item")
                                        && let Some(item_type) =
                                            item.get("type").and_then(|t| t.as_str())
                                    {
                                        let item_id = item
                                            .get("id")
                                            .and_then(|i| i.as_str())
                                            .unwrap_or("")
                                            .to_string();

                                        // Get current run_id
                                        let run_id = turn_id.to_string();

                                        // Get tool name if it's a tool call
                                        if item_type == "mcpToolCall" {
                                            let tool = item
                                                .get("tool")
                                                .and_then(|t| t.as_str())
                                                .unwrap_or("unknown");
                                            let server = item
                                                .get("server")
                                                .and_then(|s| s.as_str())
                                                .unwrap_or("");
                                            let args = item.get("arguments").cloned();
                                            print!("  MCP Tool: {}", tool);
                                            if !server.is_empty() {
                                                print!(" (server: {})", server);
                                            }
                                            println!(" started");
                                            // Show arguments if available
                                            if let Some(arguments) = &args {
                                                let args_str = arguments.to_string();
                                                if args_str.len() > 200 {
                                                    println!("    Args: {}...", &args_str[..200]);
                                                } else {
                                                    println!("    Args: {}", args_str);
                                                }
                                            }

                                            // Store ToolInvocation
                                            let invocation = ToolInvocation {
                                                id: item_id.clone(),
                                                run_id: run_id.clone(),
                                                thread_id: thread_id.to_string(),
                                                tool_name: tool.to_string(),
                                                server: Some(server.to_string()),
                                                arguments: args,
                                                result: None,
                                                error: None,
                                                status: ToolStatus::InProgress,
                                                duration_ms: None,
                                                created_at: Utc::now(),
                                            };
                                            session_clone
                                                .lock()
                                                .unwrap()
                                                .add_tool_invocation(invocation);
                                        } else if item_type == "toolCall" {
                                            let tool = item
                                                .get("name")
                                                .or_else(|| item.get("tool"))
                                                .and_then(|t| t.as_str())
                                                .unwrap_or("unknown");
                                            let args = item.get("arguments").cloned();
                                            println!("  Tool: {} started", tool);

                                            // Store ToolInvocation
                                            let invocation = ToolInvocation {
                                                id: item_id.clone(),
                                                run_id: run_id.clone(),
                                                thread_id: thread_id.to_string(),
                                                tool_name: tool.to_string(),
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
                                            session_clone
                                                .lock()
                                                .unwrap()
                                                .add_tool_invocation(invocation);

                                            // Store to MCP in background
                                            let mcp_server_for_tool = mcp_server_clone.clone();
                                            tokio::spawn(async move {
                                                store_to_mcp(
                                                    &mcp_server_for_tool,
                                                    "tool_invocation",
                                                    &tool_id,
                                                    &tool_for_mcp,
                                                )
                                                .await;
                                            });
                                        } else if item_type == "commandExecution" {
                                            let cmd = item
                                                .get("command")
                                                .and_then(|c| c.as_str())
                                                .unwrap_or("");
                                            println!("  Command: {} started", cmd);

                                            // Store ToolInvocation
                                            let invocation = ToolInvocation {
                                                id: item_id.clone(),
                                                run_id: run_id.clone(),
                                                thread_id: thread_id.to_string(),
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
                                            session_clone
                                                .lock()
                                                .unwrap()
                                                .add_tool_invocation(invocation);

                                            // Store to MCP in background
                                            let mcp_server_for_cmd = mcp_server_clone.clone();
                                            tokio::spawn(async move {
                                                store_to_mcp(
                                                    &mcp_server_for_cmd,
                                                    "tool_invocation",
                                                    &cmd_id,
                                                    &cmd_for_mcp,
                                                )
                                                .await;
                                            });
                                        } else if item_type == "reasoning" {
                                            println!("  Thinking started");

                                            // Store Reasoning
                                            let reasoning = Reasoning {
                                                id: item_id.clone(),
                                                run_id: run_id.clone(),
                                                thread_id: thread_id.to_string(),
                                                summary: vec![],
                                                text: None,
                                                created_at: Utc::now(),
                                            };
                                            let reasoning_id = item_id.clone();
                                            let reasoning_for_mcp = reasoning.clone();
                                            session_clone.lock().unwrap().add_reasoning(reasoning);

                                            // Store to MCP in background
                                            let mcp_server_for_reasoning = mcp_server_clone.clone();
                                            tokio::spawn(async move {
                                                store_to_mcp(
                                                    &mcp_server_for_reasoning,
                                                    "reasoning",
                                                    &reasoning_id,
                                                    &reasoning_for_mcp,
                                                )
                                                .await;
                                            });
                                        } else if item_type == "plan" {
                                            // Plan item - show the plan text
                                            let text = item
                                                .get("text")
                                                .and_then(|t| t.as_str())
                                                .unwrap_or("");
                                            if !text.is_empty() {
                                                println!("  Plan started: {}", text);
                                            } else {
                                                println!("  Plan started");
                                            }

                                            // Store Plan
                                            let plan = Plan {
                                                id: item_id.clone(),
                                                text: text.to_string(),
                                                intent_id: None,
                                                thread_id: thread_id.to_string(),
                                                turn_id: Some(turn_id.to_string()),
                                                status: PlanStatus::InProgress,
                                                created_at: Utc::now(),
                                            };
                                            let plan_id_2 = item_id.clone();
                                            let plan_for_mcp_2 = plan.clone();
                                            session_clone.lock().unwrap().add_plan(plan);

                                            // Store to MCP in background
                                            let mcp_server_for_plan_2 = mcp_server_clone.clone();
                                            tokio::spawn(async move {
                                                store_to_mcp(
                                                    &mcp_server_for_plan_2,
                                                    "plan",
                                                    &plan_id_2,
                                                    &plan_for_mcp_2,
                                                )
                                                .await;
                                            });
                                        } else if item_type == "fileChange" {
                                            // File change - at item/started, changes may not be available yet
                                            // Just show that file change has started
                                            println!("  📝 File Change started");

                                            // Store PatchSet (empty for now, will be filled on complete)
                                            let patchset = PatchSet {
                                                id: item_id.clone(),
                                                run_id: run_id.clone(),
                                                thread_id: thread_id.to_string(),
                                                changes: vec![],
                                                status: PatchStatus::InProgress,
                                                created_at: Utc::now(),
                                            };
                                            let patchset_id = item_id.clone();
                                            let patchset_for_mcp = patchset.clone();
                                            session_clone.lock().unwrap().add_patchset(patchset);

                                            // Store to MCP in background
                                            let mcp_server_for_patchset = mcp_server_clone.clone();
                                            tokio::spawn(async move {
                                                store_to_mcp(
                                                    &mcp_server_for_patchset,
                                                    "patchset",
                                                    &patchset_id,
                                                    &patchset_for_mcp,
                                                )
                                                .await;
                                            });
                                        } else if item_type == "dynamicToolCall" {
                                            // Dynamic tool call
                                            let tool = item
                                                .get("tool")
                                                .and_then(|t| t.as_str())
                                                .unwrap_or("unknown");
                                            let args = item.get("arguments").cloned();
                                            println!("  Dynamic Tool: {} started", tool);

                                            // Store ToolInvocation
                                            let invocation = ToolInvocation {
                                                id: item_id.clone(),
                                                run_id: run_id.clone(),
                                                thread_id: thread_id.to_string(),
                                                tool_name: tool.to_string(),
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
                                            session_clone
                                                .lock()
                                                .unwrap()
                                                .add_tool_invocation(invocation);

                                            // Store to MCP in background
                                            let mcp_server_for_dyn = mcp_server_clone.clone();
                                            tokio::spawn(async move {
                                                store_to_mcp(
                                                    &mcp_server_for_dyn,
                                                    "tool_invocation",
                                                    &dyn_tool_id,
                                                    &dyn_tool_for_mcp,
                                                )
                                                .await;
                                            });
                                        } else if item_type == "webSearch" {
                                            // Web search
                                            let query = item
                                                .get("query")
                                                .and_then(|q| q.as_str())
                                                .unwrap_or("");
                                            println!("  Web Search: {}", query);

                                            // Store ToolInvocation
                                            let invocation = ToolInvocation {
                                                id: item_id.clone(),
                                                run_id: run_id.clone(),
                                                thread_id: thread_id.to_string(),
                                                tool_name: "webSearch".to_string(),
                                                server: None,
                                                arguments: Some(
                                                    serde_json::json!({ "query": query }),
                                                ),
                                                result: None,
                                                error: None,
                                                status: ToolStatus::InProgress,
                                                duration_ms: None,
                                                created_at: Utc::now(),
                                            };
                                            let ws_id = item_id.clone();
                                            let ws_for_mcp = invocation.clone();
                                            session_clone
                                                .lock()
                                                .unwrap()
                                                .add_tool_invocation(invocation);

                                            // Store to MCP in background
                                            let mcp_server_for_ws = mcp_server_clone.clone();
                                            tokio::spawn(async move {
                                                store_to_mcp(
                                                    &mcp_server_for_ws,
                                                    "tool_invocation",
                                                    &ws_id,
                                                    &ws_for_mcp,
                                                )
                                                .await;
                                            });
                                        } else if item_type == "userMessage" {
                                            // User message -> Intent
                                            let content = item
                                                .get("content")
                                                .and_then(|c| c.as_array())
                                                .and_then(|arr| arr.first())
                                                .and_then(|first| first.get("text"))
                                                .and_then(|t| t.as_str())
                                                .unwrap_or("");
                                            let truncated = if content.len() > 50 {
                                                &content[..50]
                                            } else {
                                                content
                                            };
                                            println!("  User: {}", truncated);

                                            // Store Intent
                                            let intent = Intent {
                                                id: item_id.clone(),
                                                content: content.to_string(),
                                                thread_id: thread_id.to_string(),
                                                created_at: Utc::now(),
                                            };
                                            let intent_id = item_id.clone();
                                            let intent_for_mcp = intent.clone();
                                            session_clone.lock().unwrap().add_intent(intent);

                                            // Store to MCP in background
                                            let mcp_server_for_intent = mcp_server_clone.clone();
                                            tokio::spawn(async move {
                                                store_to_mcp(
                                                    &mcp_server_for_intent,
                                                    "intent",
                                                    &intent_id,
                                                    &intent_for_mcp,
                                                )
                                                .await;
                                            });
                                        } else if item_type == "agentMessage" {
                                            // Agent message - will stream
                                            let content = item
                                                .get("text")
                                                .and_then(|t| t.as_str())
                                                .unwrap_or("");
                                            println!("\n  🤖 Agent Response started\n");

                                            // Store AgentMessage
                                            let msg = AgentMessage {
                                                id: item_id.clone(),
                                                run_id: run_id.clone(),
                                                thread_id: thread_id.to_string(),
                                                content: content.to_string(),
                                                created_at: Utc::now(),
                                            };
                                            session_clone.lock().unwrap().add_agent_message(msg);
                                        } else {
                                            println!("  Task: {} started", item_type);
                                        }
                                    }
                                }
                                // Handle item/completed notification
                                else if method_str.contains("item/completed") {
                                    let _thread_id = params
                                        .get("threadId")
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("");
                                    let _turn_id =
                                        params.get("turnId").and_then(|t| t.as_str()).unwrap_or("");

                                    if let Some(item) = params.get("item")
                                        && let Some(item_type) =
                                            item.get("type").and_then(|t| t.as_str())
                                    {
                                        let item_id = item
                                            .get("id")
                                            .and_then(|i| i.as_str())
                                            .unwrap_or("")
                                            .to_string();

                                        if item_type == "mcpToolCall" {
                                            let tool = item
                                                .get("tool")
                                                .and_then(|t| t.as_str())
                                                .unwrap_or("unknown");
                                            let status = item
                                                .get("status")
                                                .and_then(|s| s.as_str())
                                                .unwrap_or("completed");
                                            let result = item.get("result").cloned();
                                            let error = item
                                                .get("error")
                                                .and_then(|e| e.as_str())
                                                .map(|s| s.to_string());
                                            let duration_ms =
                                                item.get("durationMs").and_then(|d| d.as_i64());

                                            print!("  MCP Tool: {} - {}", tool, status);
                                            // Show result if available
                                            if let Some(result) = item.get("result") {
                                                let result_str = result.to_string();
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
                                            } else if let Some(error) = item.get("error") {
                                                println!(" | Error: {}", error);
                                            } else {
                                                println!();
                                            }

                                            // Update ToolInvocation status
                                            let mut session = session_clone.lock().unwrap();
                                            if let Some(invocation) = session
                                                .tool_invocations
                                                .iter_mut()
                                                .find(|i| i.id == item_id)
                                            {
                                                invocation.status = match status {
                                                    "completed" => ToolStatus::Completed,
                                                    "failed" => ToolStatus::Failed,
                                                    _ => ToolStatus::Completed,
                                                };
                                                invocation.result = result;
                                                invocation.error = error;
                                                invocation.duration_ms = duration_ms;
                                            }
                                        } else if item_type == "commandExecution" {
                                            let cmd = item
                                                .get("command")
                                                .and_then(|c| c.as_str())
                                                .unwrap_or("");
                                            let exit_code =
                                                item.get("exitCode").and_then(|c| c.as_i64());
                                            let duration_ms =
                                                item.get("durationMs").and_then(|d| d.as_i64());
                                            let output = item
                                                .get("aggregatedOutput")
                                                .and_then(|o| o.as_str());

                                            println!("  Command: {} exit={:?}", cmd, exit_code);

                                            // Update ToolInvocation status
                                            let mut session = session_clone.lock().unwrap();
                                            let updated_invocation = if let Some(invocation) =
                                                session
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
                                                    .map(|o| serde_json::json!({ "output": o }));
                                                invocation.duration_ms = duration_ms;
                                                Some(invocation.clone())
                                            } else {
                                                None
                                            };
                                            drop(session);

                                            // Store updated tool invocation to MCP
                                            if let Some(inv) = updated_invocation {
                                                let mcp_server_for_inv = mcp_server_clone.clone();
                                                let inv_id = item_id.clone();
                                                tokio::spawn(async move {
                                                    store_to_mcp(
                                                        &mcp_server_for_inv,
                                                        "tool_invocation",
                                                        &inv_id,
                                                        &inv,
                                                    )
                                                    .await;
                                                });
                                            }
                                        } else if item_type == "reasoning" {
                                            println!("  Thinking completed");

                                            // Update Reasoning
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

                                            let mut session = session_clone.lock().unwrap();
                                            if let Some(reasoning) = session
                                                .reasonings
                                                .iter_mut()
                                                .find(|r| r.id == item_id)
                                            {
                                                reasoning.summary = summary;
                                                reasoning.text = text;
                                            }
                                        } else if item_type == "plan" {
                                            // Plan item - show the plan text
                                            let text = item
                                                .get("text")
                                                .and_then(|t| t.as_str())
                                                .unwrap_or("");
                                            if !text.is_empty() {
                                                println!("  Plan completed: {}", text);
                                            } else {
                                                println!("  Plan completed");
                                            }

                                            // Update Plan status
                                            let mut session = session_clone.lock().unwrap();
                                            if let Some(plan) =
                                                session.plans.iter_mut().find(|p| p.id == item_id)
                                            {
                                                plan.status = PlanStatus::Completed;
                                                if !text.is_empty() {
                                                    plan.text = text.to_string();
                                                }
                                            }
                                        } else if item_type == "fileChange" {
                                            // File change - show files and diff
                                            let status = item
                                                .get("status")
                                                .and_then(|s| s.as_str())
                                                .unwrap_or("");

                                            if debug_mode {
                                                eprintln!("[DEBUG] fileChange item: {:?}", item);
                                            }

                                            // Parse changes
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
                                                                        .and_then(|k| k.get("type"))
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
                                                "  📝 File Change {} ({} files)",
                                                status, file_count
                                            );

                                            // Show first few files with diff
                                            for change in changes.iter().take(3) {
                                                println!(
                                                    "    - {} ({})",
                                                    change.path, change.change_type
                                                );
                                                // Show first few lines of diff
                                                if !change.diff.is_empty() {
                                                    let diff_lines: Vec<&str> =
                                                        change.diff.lines().take(10).collect();
                                                    for line in diff_lines {
                                                        println!("      {}", line);
                                                    }
                                                    if change.diff.lines().count() > 10 {
                                                        println!(
                                                            "      ... ({} more lines)",
                                                            change.diff.lines().count() - 10
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

                                            // Update PatchSet status
                                            if let Some(patchset) = session_clone
                                                .lock()
                                                .unwrap()
                                                .patchsets
                                                .iter_mut()
                                                .find(|p| p.id == item_id)
                                            {
                                                patchset.status = match status {
                                                    "completed" => PatchStatus::Completed,
                                                    "failed" => PatchStatus::Failed,
                                                    "declined" => PatchStatus::Declined,
                                                    _ => PatchStatus::Completed,
                                                };
                                                patchset.changes = changes;
                                            }

                                            // Store to MCP in background
                                            if let Some(patchset) = session_clone
                                                .lock()
                                                .unwrap()
                                                .patchsets
                                                .iter()
                                                .find(|p| p.id == item_id)
                                                .cloned()
                                            {
                                                let mcp_server_for_ps = mcp_server_clone.clone();
                                                let ps_id = item_id.clone();
                                                tokio::spawn(async move {
                                                    store_to_mcp(
                                                        &mcp_server_for_ps,
                                                        "patchset",
                                                        &ps_id,
                                                        &patchset,
                                                    )
                                                    .await;
                                                });
                                            }
                                        } else if item_type == "toolCall" {
                                            let tool = item
                                                .get("name")
                                                .or_else(|| item.get("tool"))
                                                .and_then(|t| t.as_str())
                                                .unwrap_or("unknown");
                                            let status = item
                                                .get("status")
                                                .and_then(|s| s.as_str())
                                                .unwrap_or("completed");
                                            let result = item.get("result").cloned();
                                            let error = item
                                                .get("error")
                                                .and_then(|e| e.as_str())
                                                .map(String::from);
                                            let duration_ms =
                                                item.get("durationMs").and_then(|d| d.as_i64());

                                            print!("  Tool: {} - {}", tool, status);
                                            if let Some(result) = item.get("result") {
                                                let result_str = result.to_string();
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

                                            // Update ToolInvocation status
                                            if let Some(invocation) = session_clone
                                                .lock()
                                                .unwrap()
                                                .tool_invocations
                                                .iter_mut()
                                                .find(|i| i.id == item_id)
                                            {
                                                invocation.status = match status {
                                                    "completed" => ToolStatus::Completed,
                                                    "failed" => ToolStatus::Failed,
                                                    _ => ToolStatus::Completed,
                                                };
                                                invocation.result = result;
                                                invocation.error = error;
                                                invocation.duration_ms = duration_ms;
                                            }

                                            // Store to MCP in background
                                            if let Some(invocation) = session_clone
                                                .lock()
                                                .unwrap()
                                                .tool_invocations
                                                .iter()
                                                .find(|i| i.id == item_id)
                                                .cloned()
                                            {
                                                let mcp_server_for_inv = mcp_server_clone.clone();
                                                let inv_id = item_id.clone();
                                                tokio::spawn(async move {
                                                    store_to_mcp(
                                                        &mcp_server_for_inv,
                                                        "tool_invocation",
                                                        &inv_id,
                                                        &invocation,
                                                    )
                                                    .await;
                                                });
                                            }
                                        } else if item_type == "userMessage" {
                                            // Update intent if needed
                                            println!("  User message completed");
                                        } else if item_type == "agentMessage" {
                                            // Update agent message content
                                            if debug_mode {
                                                eprintln!(
                                                    "[DEBUG] agentMessage completed item: {:?}",
                                                    item
                                                );
                                            }
                                            let content = item
                                                .get("text")
                                                .and_then(|t| t.as_str())
                                                .unwrap_or("");
                                            if let Some(msg) = session_clone
                                                .lock()
                                                .unwrap()
                                                .agent_messages
                                                .iter_mut()
                                                .find(|m| m.id == item_id)
                                            {
                                                msg.content = content.to_string();
                                                if !msg.content.is_empty() {
                                                    println!("\n  🤖 Agent: {}\n", msg.content);
                                                }
                                            }
                                            println!("  🤖 Agent Response completed");
                                        }
                                    }
                                }
                                // Handle approval request
                                else if method_str.contains("requestApproval") {
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
                                    let approval_type = if method_str.contains("commandExecution") {
                                        ApprovalType::CommandExecution
                                    } else if method_str.contains("fileChange") {
                                        ApprovalType::FileChange
                                    } else if method_str.contains("apply_patch") {
                                        ApprovalType::ApplyPatch
                                    } else {
                                        ApprovalType::Unknown
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
                                        approval_type,
                                        item_id,
                                        thread_id: thread_id.clone(),
                                        run_id: None,
                                        command,
                                        changes,
                                        description,
                                        decision: None,
                                        requested_at: Utc::now(),
                                        resolved_at: None,
                                    };
                                    let approval_id = request_id.clone();
                                    let approval_for_mcp = approval_request.clone();
                                    {
                                        let mut session = session_clone.lock().unwrap();
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
                                        )
                                        .await;
                                    });

                                    let current_mode = approval_mode.lock().unwrap().clone();
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
                                    let approval_to_store = {
                                        let mut session = session_clone.lock().unwrap();
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
                                            )
                                            .await;
                                        });
                                    }

                                    // Use the correct resolve method based on the request type
                                    let resolve_method = if method_str.contains("commandExecution")
                                    {
                                        "item/commandExecution/requestApproval/resolve"
                                    } else if method_str.contains("fileChange") {
                                        "item/fileChange/requestApproval/resolve"
                                    } else if method_str.contains("exec_approval") {
                                        "exec_approval_request/resolve"
                                    } else if method_str.contains("apply_patch") {
                                        "apply_patch_approval_request/resolve"
                                    } else {
                                        "requestApproval/resolve"
                                    };

                                    let approval_msg = CodexMessage::new_request(
                                        0,
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
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static REQUEST_ID: AtomicU64 = AtomicU64::new(1);
        let id = REQUEST_ID.fetch_add(1, Ordering::Relaxed);

        let msg = CodexMessage::new_request(id, method, params);
        tx.send(msg.to_json()).await.map_err(|e| e.to_string())?;

        // Wait for response
        for _ in 0..300 {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            let mut resp = responses.lock().unwrap();
            if let Some(response) = resp.remove(&id) {
                if let Some(error_obj) = response.get("error") {
                    return Err(format!("Error: {}", error_obj));
                }
                return Ok(response.get("result").cloned().unwrap_or(response));
            }
        }
        Err("Timeout".to_string())
    }

    // Initialize
    match send_request(
        &tx,
        &responses,
        "initialize",
        serde_json::json!({
            "protocolVersion": "1.0",
            "capabilities": {},
            "clientInfo": { "name": "libra", "version": "1.0.0" }
        }),
    )
    .await
    {
        Ok(_) => println!("Initialized"),
        Err(e) => {
            eprintln!("Init failed: {}", e);
            return;
        }
    }

    // Start thread
    match send_request(&tx, &responses, "thread/start", serde_json::json!({})).await {
        Ok(resp) => {
            if let Some(thread_obj) = resp.get("thread")
                && let Some(id) = thread_obj.get("id").and_then(|v| v.as_str())
            {
                thread_id = id.to_string();
                println!("Thread: {}", id);
            }
        }
        Err(e) => {
            eprintln!("Thread start failed: {}", e);
            return;
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
                    let is_approval = *waiting_for_approval_clone.lock().unwrap();

                    if is_approval {
                        // This input goes to the approval handler - ignore for chat
                        // The approval handler is waiting on a oneshot channel
                        continue;
                    }

                    if line.trim().is_empty() {
                        continue;
                    }

                    match send_request(&tx, &responses, "turn/start", serde_json::json!({
                        "input": [{ "type": "text", "text": line }],
                        "threadId": thread_id
                    })).await {
                        Ok(resp) => println!("Response: {:?}", resp),
                        Err(e) => eprintln!("Error: {}", e),
                    }
                }
            }
            approval_req = approval_rx.recv() => {
                if let Some((params, response_tx)) = approval_req {
                    // Set flag to route stdin to approval
                    *waiting_for_approval_clone.lock().unwrap() = true;

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
                                *approval_mode_clone.lock().unwrap() = "accept".to_string();
                                true
                            }
                            "dd" | "decline all" => {
                                println!("  → Declined (will auto-decline future)");
                                *approval_mode_clone.lock().unwrap() = "decline".to_string();
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
                    *waiting_for_approval_clone.lock().unwrap() = false;
                }
            }
            _ = tokio::time::sleep(tokio::time::Duration::from_secs(1)) => {}
        }
    }
}
