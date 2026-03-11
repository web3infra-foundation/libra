//! Agent Codex command - directly connect to Codex app-server via WebSocket.

use std::sync::{Arc, Mutex};

use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::cli_error;

const CODEX_WS_URL: &str = "ws://127.0.0.1:8080";

#[derive(Parser, Debug)]
pub struct AgentCodexArgs {
    /// Codex WebSocket URL
    #[arg(long, default_value = CODEX_WS_URL)]
    pub url: String,

    /// Working directory for the agent
    #[arg(long, default_value = ".")]
    pub cwd: String,
}

/// Codex JSON-RPC message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexMessage {
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<u64>,
    pub method: Option<String>,
    pub params: Option<serde_json::Value>,
    pub result: Option<serde_json::Value>,
    pub error: Option<serde_json::Value>,
}

impl CodexMessage {
    pub fn new_request(id: u64, method: &str, params: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: Some(id),
            method: Some(method.to_string()),
            params: Some(params),
            result: None,
            error: None,
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

pub async fn execute(args: AgentCodexArgs) {
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

    // Shared state
    let mut thread_id = String::new();
    let responses: Arc<Mutex<std::collections::HashMap<u64, serde_json::Value>>> =
        Arc::new(Mutex::new(std::collections::HashMap::new()));

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
    let _reader_task = tokio::spawn(async move {
        let mut read = read;
        loop {
            match read.next().await {
                Some(Ok(Message::Text(text))) => {
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                        // Store response if has id
                        if let Some(id_val) = json.get("id") {
                            if let Some(id) = id_val.as_u64() {
                                let mut resp = responses_clone.lock().unwrap();
                                resp.insert(id, json);
                            }
                        }
                        // Handle notifications
                        else if let Some(method) = json.get("method") {
                            let method_str = method.as_str().unwrap_or("");

                            // Handle all notifications based on method name
                            // See schema/ServerNotification.json for full list
                            // Filter out noisy notifications like tokenUsage
                            let is_noise = method_str.contains("tokenUsage")
                                || method_str.contains("token/usage");
                            let show_notification = !is_noise && (
                                method_str.contains("task_started")
                                || method_str.contains("task_complete")
                                || method_str.contains("agent_reasoning")
                                || method_str.contains("turn/completed")
                                || method_str.contains("turn_started")
                                || method_str.contains("turn/plan")
                                || method_str.contains("thread/started")
                                || method_str.contains("thread/status")
                                || method_str.contains("item/")
                                || method_str.contains("requestApproval")
                                || method_str.contains("reasoning")
                            );

                            // Extract and print useful info based on notification type
                            if let Some(params) = json.get("params") {
                                // Show hierarchical flow: Thread → Turn → Plan → Item → Detail
                                if method_str.contains("thread/started") {
                                    // params: { thread: { threadId, ... } }
                                    let thread_id = params.get("thread")
                                        .and_then(|t| t.get("threadId"))
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("");
                                    println!("\n=== New Thread: {} ===", thread_id);
                                } else if method_str.contains("turn/started") || method_str.contains("turnStarted") {
                                    // params: { turn: { id, ... }, threadId }
                                    let turn_id = params.get("turn")
                                        .and_then(|t| t.get("id"))
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("");
                                    let thread_id = params.get("threadId").and_then(|t| t.as_str()).unwrap_or("");
                                    println!("\n--- Turn started: {} (thread: {}) ---", &turn_id[..8.min(turn_id.len())], &thread_id[..8.min(thread_id.len())]);
                                } else if method_str.contains("turn/completed") || method_str.contains("turnCompleted") {
                                    // params: { threadId, turnId }
                                    let turn_id = params.get("turnId").and_then(|t| t.as_str()).unwrap_or("");
                                    println!("--- Turn completed: {} ---", &turn_id[..8.min(turn_id.len())]);
                                } else if method_str.contains("turn/plan/updated") || method_str.contains("plan/updated") {
                                    // params: { plan: [...], threadId, turnId, explanation? }
                                    if let Some(plan) = params.get("plan") {
                                        let explanation = params.get("explanation").and_then(|e| e.as_str());
                                        println!("\n📋 Plan Updated:");
                                        if let Some(exp) = explanation {
                                            println!("  Explanation: {}", exp);
                                        }
                                        if let Ok(plan_array) = serde_json::from_str::<Vec<serde_json::Value>>(&plan.to_string()) {
                                            for item in plan_array.iter() {
                                                let status = item.get("status").and_then(|s| s.as_str()).unwrap_or("unknown");
                                                let step = item.get("step").and_then(|s| s.as_str()).unwrap_or("");
                                                let marker = match status {
                                                    "completed" => "✓",
                                                    "inProgress" => "▶",
                                                    _ => "○"
                                                };
                                                println!("  {} {}", marker, step);
                                            }
                                        }
                                    }
                                } else if method_str.contains("codex/event/task_started") {
                                    // Task started - top level notification
                                    let thread_id = params.get("threadId").and_then(|t| t.as_str()).unwrap_or("");
                                    if !thread_id.is_empty() {
                                        println!("\n🚀 Task Started (thread: {})", &thread_id[..8.min(thread_id.len())]);
                                    } else {
                                        println!("\n🚀 Task Started");
                                    }
                                } else if method_str.contains("codex/event/task_complete") {
                                    // Task completed - top level notification
                                    println!("\n✅ Task Completed");
                                } else if show_notification && !method_str.contains("item/") {
                                    println!("[Codex] {}", method_str);
                                }
                                // Handle thread/started
                                if method_str.contains("item/started") {
                                    // params.item.type contains the type
                                    if let Some(item) = params.get("item") {
                                        if let Some(item_type) = item.get("type").and_then(|t| t.as_str()) {
                                            // Get tool name if it's a tool call
                                            if item_type == "mcpToolCall" {
                                                let tool = item.get("tool").and_then(|t| t.as_str()).unwrap_or("unknown");
                                                let server = item.get("server").and_then(|s| s.as_str()).unwrap_or("");
                                                let args = item.get("arguments");
                                                print!("  MCP Tool: {}", tool);
                                                if !server.is_empty() {
                                                    print!(" (server: {})", server);
                                                }
                                                println!(" started");
                                                // Show arguments if available
                                                if let Some(arguments) = args {
                                                    let args_str = arguments.to_string();
                                                    if args_str.len() > 200 {
                                                        println!("    Args: {}...", &args_str[..200]);
                                                    } else {
                                                        println!("    Args: {}", args_str);
                                                    }
                                                }
                                            } else if item_type == "toolCall" {
                                                if let Some(tool) = item.get("name").or_else(|| item.get("tool")) {
                                                    println!("  Tool: {} started", tool);
                                                } else {
                                                    println!("  Task: {} started", item_type);
                                                }
                                            } else if item_type == "commandExecution" {
                                                if let Some(cmd) = item.get("command") {
                                                    println!("  Command: {} started", cmd);
                                                } else {
                                                    println!("  Task: {} started", item_type);
                                                }
                                            } else if item_type == "reasoning" {
                                                println!("  Thinking started");
                                            } else if item_type == "plan" {
                                                // Plan item - show the plan text
                                                if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                                                    println!("  Plan started: {}", text);
                                                } else {
                                                    println!("  Plan started");
                                                }
                                            } else if item_type == "fileChange" {
                                                // File change - at item/started, changes may not be available yet
                                                // Just show that file change has started
                                                println!("  📝 File Change started");
                                            } else if item_type == "dynamicToolCall" {
                                                // Dynamic tool call
                                                let tool = item.get("tool").and_then(|t| t.as_str()).unwrap_or("unknown");
                                                println!("  Dynamic Tool: {} started", tool);
                                            } else if item_type == "webSearch" {
                                                // Web search
                                                let query = item.get("query").and_then(|q| q.as_str()).unwrap_or("");
                                                println!("  Web Search: {}", query);
                                            } else if item_type == "userMessage" {
                                                // User message
                                                if let Some(content) = item.get("content").and_then(|c| c.as_array()) {
                                                    if let Some(first) = content.first() {
                                                        let text = first.get("text").and_then(|t| t.as_str()).unwrap_or("");
                                                        let truncated = if text.len() > 50 { &text[..50] } else { text };
                                                        println!("  User: {}", truncated);
                                                    }
                                                }
                                            } else if item_type == "agentMessage" {
                                                // Agent message - will stream
                                                println!("  Agent Response started");
                                            } else {
                                                println!("  Task: {} started", item_type);
                                            }
                                        }
                                    }
                                }
                                // Handle item/completed notification
                                else if method_str.contains("item/completed") {
                                    if let Some(item) = params.get("item") {
                                        if let Some(item_type) = item.get("type").and_then(|t| t.as_str()) {
                                            if item_type == "mcpToolCall" {
                                                let tool = item.get("tool").and_then(|t| t.as_str()).unwrap_or("unknown");
                                                let status = item.get("status").and_then(|s| s.as_str()).unwrap_or("completed");
                                                print!("  MCP Tool: {} - {}", tool, status);
                                                // Show result if available
                                                if let Some(result) = item.get("result") {
                                                    let result_str = result.to_string();
                                                    if result_str.len() > 100 {
                                                        println!(" | Result: {}...", &result_str[..100]);
                                                    } else if !result_str.is_empty() && result_str != "null" {
                                                        println!(" | Result: {}", result_str);
                                                    } else {
                                                        println!();
                                                    }
                                                } else if let Some(error) = item.get("error") {
                                                    println!(" | Error: {}", error);
                                                } else {
                                                    println!();
                                                }
                                            } else if item_type == "commandExecution" {
                                                if let Some(cmd) = item.get("command") {
                                                    let exit_code = item.get("exitCode").and_then(|c| c.as_i64());
                                                    println!("  Command: {} exit={:?}", cmd, exit_code);
                                                }
                                            } else if item_type == "reasoning" {
                                                println!("  Thinking completed");
                                            } else if item_type == "plan" {
                                                // Plan item - show the plan text
                                                if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                                                    println!("  Plan completed: {}", text);
                                                } else {
                                                    println!("  Plan completed");
                                                }
                                            } else if item_type == "fileChange" {
                                                // File change - show files and diff
                                                let status = item.get("status").and_then(|s| s.as_str()).unwrap_or("completed");
                                                println!("  📝 File Change: {}", status);
                                                if let Some(changes) = item.get("changes").and_then(|c| c.as_array()) {
                                                    for change in changes.iter().take(5) {
                                                        let path = change.get("path").and_then(|p| p.as_str()).unwrap_or("?");
                                                        let kind = change.get("kind").and_then(|k| {
                                                            if let Some(t) = k.get("type").and_then(|t| t.as_str()) {
                                                                Some(t)
                                                            } else {
                                                                None
                                                            }
                                                        }).unwrap_or("update");
                                                        let marker = match kind {
                                                            "add" => "+",
                                                            "delete" => "-",
                                                            _ => "~"
                                                        };
                                                        println!("    {} {}", marker, path);
                                                        // Show diff if available
                                                        if let Some(diff) = change.get("diff").and_then(|d| d.as_str()) {
                                                            let diff_lines: Vec<&str> = diff.lines().collect();
                                                            let show_lines = diff_lines.len().min(20);
                                                            for line in diff_lines.iter().take(show_lines) {
                                                                println!("      {}", line);
                                                            }
                                                            if diff_lines.len() > 20 {
                                                                println!("      ... ({} more lines)", diff_lines.len() - 20);
                                                            }
                                                        }
                                                    }
                                                    if changes.len() > 5 {
                                                        println!("    ... and {} more", changes.len() - 5);
                                                    }
                                                }
                                            } else if item_type == "dynamicToolCall" {
                                                let tool = item.get("tool").and_then(|t| t.as_str()).unwrap_or("unknown");
                                                let status = item.get("status").and_then(|s| s.as_str()).unwrap_or("completed");
                                                println!("  Dynamic Tool: {} - {}", tool, status);
                                            } else if item_type == "webSearch" {
                                                let query = item.get("query").and_then(|q| q.as_str()).unwrap_or("");
                                                println!("  Web Search done: {}", query);
                                            } else if item_type == "agentMessage" {
                                                println!("  Agent Response completed");
                                            } else {
                                                println!("  Task: {} completed", item_type);
                                            }
                                        }
                                    }
                                }
                                // Handle agent message delta - direct text output
                                else if method_str.contains("agentMessage") || method_str.contains("agent_message") {
                                    // Check for delta at different levels
                                    let delta = params.get("delta")
                                        .or_else(|| params.get("msg").and_then(|m| m.get("delta")))
                                        .or_else(|| params.get("text"))
                                        .and_then(|d| d.as_str());

                                    if let Some(text) = delta {
                                        print!("{}", text);
                                        use std::io::Write;
                                        std::io::stdout().flush().ok();
                                    }
                                }
                                // Handle plan delta
                                else if method_str.contains("plan") {
                                    if let Some(plan_val) = params.get("plan").or_else(|| params.get("delta")) {
                                        // Try to parse as JSON array and format nicely
                                        let plan_str = plan_val.to_string();
                                        if let Ok(plan_array) = serde_json::from_str::<Vec<serde_json::Value>>(&plan_str) {
                                            println!("  Plan:");
                                            for item in plan_array.iter() {
                                                let status = item.get("status").and_then(|s| s.as_str()).unwrap_or("unknown");
                                                let step = item.get("step").and_then(|s| s.as_str()).unwrap_or("");
                                                let marker = match status {
                                                    "completed" => "✓",
                                                    "inProgress" => "▶",
                                                    _ => "○"
                                                };
                                                println!("    {} {}", marker, step);
                                            }
                                        } else {
                                            // Fallback: just print as string
                                            println!("  Plan: {}", plan_val);
                                        }
                                    }
                                }
                                // Handle command output delta
                                else if method_str.contains("commandExecution/outputDelta") {
                                    if let Some(output) = params.get("output").or_else(|| params.get("delta")) {
                                        print!("{}", output);
                                        use std::io::Write;
                                        std::io::stdout().flush().ok();
                                    }
                                }
                                // Handle file change output delta (diff streaming)
                                else if method_str.contains("fileChange/outputDelta") || method_str.contains("filechange/outputDelta") {
                                    if let Some(delta) = params.get("delta").or_else(|| params.get("output")) {
                                        print!("{}", delta);
                                        use std::io::Write;
                                        std::io::stdout().flush().ok();
                                    }
                                }
                                // Handle reasoning/thinking process
                                else if method_str.contains("reasoning") {
                                    // Check for textDelta, summaryTextDelta, or delta
                                    let reasoning_text = params.get("delta")
                                        .or_else(|| params.get("textDelta"))
                                        .or_else(|| params.get("summaryTextDelta"))
                                        .or_else(|| params.get("summary"))
                                        .and_then(|d| d.as_str());

                                    if let Some(text) = reasoning_text {
                                        print!("{}", text);
                                        use std::io::Write;
                                        std::io::stdout().flush().ok();
                                    }
                                }
                                // Handle MCP tool call progress
                                else if method_str.contains("mcpToolCall") || method_str.contains("tool") {
                                    if let Some(name) = params.get("name") {
                                        println!("  Tool: {}", name);
                                    } else if let Some(tool_call) = params.get("toolCall").or_else(|| params.get("tool")) {
                                        if let Some(name) = tool_call.get("name") {
                                            println!("  Tool: {}", name);
                                        }
                                    }
                                }
                                // Handle turn completion
                                else if method_str.contains("turn/completed") || method_str.contains("turnCompleted") {
                                    println!("\n[Turn completed]");
                                }
                                // Handle turn started
                                else if method_str.contains("turn/started") || method_str.contains("turnStarted") {
                                    println!("[Turn started]");
                                }
                            }

                            // Handle approval requests - auto approve
                            if method_str.contains("requestApproval") {
                                let request_id = json.get("params")
                                    .and_then(|p| p.get("requestId"))
                                    .cloned();

                                let approval_msg = CodexMessage::new_request(
                                    0,
                                    "requestApproval/resolve",
                                    serde_json::json!({
                                        "requestId": request_id,
                                        "approved": true
                                    }),
                                );
                                let _ = tx_clone.send(approval_msg.to_json()).await;
                                println!("[Auto-approved]");
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
    match send_request(&tx, &responses, "initialize", serde_json::json!({
        "protocolVersion": "1.0",
        "capabilities": {},
        "clientInfo": { "name": "libra", "version": "1.0.0" }
    })).await {
        Ok(_) => println!("Initialized"),
        Err(e) => { eprintln!("Init failed: {}", e); return; }
    }

    // Start thread
    match send_request(&tx, &responses, "thread/start", serde_json::json!({})).await {
        Ok(resp) => {
            if let Some(thread_obj) = resp.get("thread") {
                if let Some(id) = thread_obj.get("id").and_then(|v| v.as_str()) {
                    thread_id = id.to_string();
                    println!("Thread: {}", id);
                }
            }
        }
        Err(e) => { eprintln!("Thread start failed: {}", e); return; }
    }

    println!("\n=== Ready! Type your message ===\n");

    // Channel for stdin
    let (stdin_tx, mut stdin_rx) = mpsc::channel::<String>(100);
    tokio::spawn(async move {
        use std::io::{BufRead, BufReader};
        let stdin = BufReader::new(std::io::stdin());
        for line in stdin.lines() {
            if let Ok(line) = line {
                let _ = stdin_tx.send(line).await;
            }
        }
    });

    // Main loop
    loop {
        tokio::select! {
            msg = stdin_rx.recv() => {
                if let Some(line) = msg {
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
            _ = tokio::time::sleep(tokio::time::Duration::from_secs(1)) => {}
        }
    }
}
