//! Agent Codex command - directly connect to Codex app-server via WebSocket.

use std::sync::{Arc, Mutex};

use chrono::Utc;
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::cli_error;

const CODEX_WS_URL: &str = "ws://127.0.0.1:8080";

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

// =============================================================================
// Data Structures for Agent Storage 
// =============================================================================

/// Thread status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ThreadStatus {
    Pending,
    Running,
    Completed,
    Archived,
    Closed,
}

/// Run status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

/// Task status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

/// Plan status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PlanStatus {
    Pending,
    InProgress,
    Completed,
}

/// Tool invocation status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ToolStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

/// Patch apply status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PatchStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
    Declined,
}

/// Approval type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalType {
    CommandExecution,
    FileChange,
    ApplyPatch,
    Unknown,
}

/// Thread - represents a conversation session
/// Corresponds to: Thread[L] in agent-overview-zh.md
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexThread {
    pub id: String,
    pub status: ThreadStatus,
    pub current_turn_id: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl Default for CodexThread {
    fn default() -> Self {
        let now = chrono::Utc::now();
        Self {
            id: String::new(),
            status: ThreadStatus::Pending,
            current_turn_id: None,
            created_at: now,
            updated_at: now,
        }
    }
}

/// Intent - user request snapshot
/// Corresponds to: Intent[S] in agent-overview-zh.md
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Intent {
    pub id: String,
    pub content: String,
    pub thread_id: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Plan - strategy and steps snapshot
/// Corresponds to: Plan[S] in agent-overview-zh.md
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub id: String,
    pub text: String,
    pub intent_id: Option<String>,
    pub thread_id: String,
    pub turn_id: Option<String>,
    pub status: PlanStatus,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Task - work unit definition
/// Corresponds to: Task[S] in agent-overview-zh.md
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub tool_name: Option<String>,
    pub plan_id: Option<String>,
    pub thread_id: String,
    pub turn_id: Option<String>,
    pub status: TaskStatus,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Run - execution attempt
/// Corresponds to: Run[S] in agent-overview-zh.md
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub thread_id: String,
    pub status: RunStatus,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// ToolInvocation - tool call record
/// Corresponds to: ToolInvocation[E] in agent-overview-zh.md
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInvocation {
    pub id: String,
    pub run_id: String,
    pub thread_id: String,
    pub tool_name: String,
    pub server: Option<String>,
    pub arguments: Option<serde_json::Value>,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
    pub status: ToolStatus,
    pub duration_ms: Option<i64>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Reasoning - agent reasoning process
/// Corresponds to: reasoning items in Codex protocol
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reasoning {
    pub id: String,
    pub run_id: String,
    pub thread_id: String,
    pub summary: Vec<String>,
    pub text: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// FileChange - represents a file change
/// Corresponds to: PatchSet[S] in agent-overview-zh.md
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    pub diff: String,
    pub change_type: String, // add, delete, update
}

/// PatchSet - candidate patch snapshot
/// Corresponds to: PatchSet[S] in agent-overview-zh.md
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchSet {
    pub id: String,
    pub run_id: String,
    pub thread_id: String,
    pub changes: Vec<FileChange>,
    pub status: PatchStatus,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// ApprovalRequest - approval request record
/// Corresponds to: Decision[E] in agent-overview-zh.md (pre-decision)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub id: String,
    pub approval_type: ApprovalType,
    pub item_id: String,
    pub thread_id: String,
    pub run_id: Option<String>,
    pub command: Option<String>,
    pub changes: Option<Vec<String>>,
    pub description: Option<String>,
    pub decision: Option<bool>,
    pub requested_at: chrono::DateTime<chrono::Utc>,
    pub resolved_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// AgentMessage - agent response content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessage {
    pub id: String,
    pub run_id: String,
    pub thread_id: String,
    pub content: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Complete session data that can be queried by MCP
/// This is the main data container that MCP will query
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CodexSession {
    pub thread: CodexThread,
    pub intents: Vec<Intent>,
    pub plans: Vec<Plan>,
    pub tasks: Vec<Task>,
    pub runs: Vec<Run>,
    pub tool_invocations: Vec<ToolInvocation>,
    pub reasonings: Vec<Reasoning>,
    pub patchsets: Vec<PatchSet>,
    pub approval_requests: Vec<ApprovalRequest>,
    pub agent_messages: Vec<AgentMessage>,
}

impl CodexSession {
    pub fn new() -> Self {
        Self::default()
    }

    /// Print summary of current session (for debug)
    pub fn print_summary(&self) {
        eprintln!("\n========== SESSION DATA ==========");
        eprintln!("Thread: id={}, status={:?}, current_turn={:?}",
            self.thread.id, self.thread.status, self.thread.current_turn_id);
        eprintln!("Intents: {}", self.intents.len());
        for intent in &self.intents {
            eprintln!("  - Intent: {} - {}", intent.id, &intent.content[..intent.content.len().min(50)]);
        }
        eprintln!("Plans: {}", self.plans.len());
        for plan in &self.plans {
            eprintln!("  - Plan: {} - {} (status: {:?})", plan.id, &plan.text[..plan.text.len().min(30)], plan.status);
        }
        eprintln!("Runs: {}", self.runs.len());
        for run in &self.runs {
            eprintln!("  - Run: {} (status: {:?})", run.id, run.status);
        }
        eprintln!("ToolInvocations: {}", self.tool_invocations.len());
        for inv in &self.tool_invocations {
            eprintln!("  - Tool: {} - {} (status: {:?})", inv.id, inv.tool_name, inv.status);
        }
        eprintln!("Reasonings: {}", self.reasonings.len());
        eprintln!("PatchSets: {}", self.patchsets.len());
        for ps in &self.patchsets {
            eprintln!("  - PatchSet: {} (status: {:?}, {} changes)", ps.id, ps.status, ps.changes.len());
        }
        eprintln!("ApprovalRequests: {}", self.approval_requests.len());
        for ar in &self.approval_requests {
            eprintln!("  - Approval: {} - {:?} (decision: {:?})", ar.id, ar.approval_type, ar.decision);
        }
        eprintln!("AgentMessages: {}", self.agent_messages.len());
        eprintln!("================================\n");
    }

    /// Add a new intent
    pub fn add_intent(&mut self, intent: Intent) {
        eprintln!("[DEBUG] Added Intent: {} - {}", intent.id, &intent.content[..intent.content.len().min(50)]);
        self.intents.push(intent);
    }

    /// Add a new plan
    pub fn add_plan(&mut self, plan: Plan) {
        eprintln!("[DEBUG] Added Plan: {} - {} (status: {:?})", plan.id, &plan.text[..plan.text.len().min(30)], plan.status);
        self.plans.push(plan);
    }

    /// Add a new task
    pub fn add_task(&mut self, task: Task) {
        eprintln!("[DEBUG] Added Task: {}", task.id);
        self.tasks.push(task);
    }

    /// Add a new run
    pub fn add_run(&mut self, run: Run) {
        eprintln!("[DEBUG] Added Run: {} (status: {:?})", run.id, run.status);
        self.runs.push(run);
    }

    /// Add a new tool invocation
    pub fn add_tool_invocation(&mut self, invocation: ToolInvocation) {
        eprintln!("[DEBUG] Added ToolInvocation: {} - {}", invocation.id, invocation.tool_name);
        self.tool_invocations.push(invocation);
    }

    /// Add a new reasoning
    pub fn add_reasoning(&mut self, reasoning: Reasoning) {
        eprintln!("[DEBUG] Added Reasoning: {}", reasoning.id);
        self.reasonings.push(reasoning);
    }

    /// Add a new patchset
    pub fn add_patchset(&mut self, patchset: PatchSet) {
        eprintln!("[DEBUG] Added PatchSet: {}", patchset.id);
        self.patchsets.push(patchset);
    }

    /// Add a new approval request
    pub fn add_approval_request(&mut self, approval: ApprovalRequest) {
        eprintln!("[DEBUG] Added ApprovalRequest: {} - {:?}", approval.id, approval.approval_type);
        self.approval_requests.push(approval);
    }

    /// Add a new agent message
    pub fn add_agent_message(&mut self, msg: AgentMessage) {
        eprintln!("[DEBUG] Added AgentMessage: {}", msg.id);
        self.agent_messages.push(msg);
    }

    /// Update thread status
    pub fn update_thread(&mut self, thread: CodexThread) {
        eprintln!("[DEBUG] Updated Thread: {} (status: {:?})", thread.id, thread.status);
        self.thread = thread;
    }

    /// Get current active run
    pub fn get_active_run(&self) -> Option<&Run> {
        self.runs.iter().find(|r| r.status == RunStatus::InProgress)
    }

    /// Get current active turn
    pub fn get_current_turn_id(&self) -> Option<&str> {
        self.thread.current_turn_id.as_deref()
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

    // Channel for approval requests (reader -> main loop)
    let (approval_tx, mut approval_rx) = mpsc::channel::<(serde_json::Value, tokio::sync::oneshot::Sender<bool>)>(10);

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
    let approval_mode = args.approval.clone();
    let _debug_mode = args.debug;
    let session_clone = session.clone();
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

                            // Debug: print all method names
                            // eprintln!("[DEBUG] Received method: {}", method_str);

                            // Handle all notifications based on method name
                            // See schema/ServerNotification.json for full list
                            // Filter out noisy notifications like tokenUsage
                            let is_noise = method_str.contains("tokenUsage")
                                || method_str.contains("token/usage");
                            let show_notification = !is_noise && (
                                method_str.contains("initialized")
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

                                    // Store thread in session
                                    let thread = CodexThread {
                                        id: thread_id.to_string(),
                                        status: ThreadStatus::Running,
                                        current_turn_id: None,
                                        created_at: Utc::now(),
                                        updated_at: Utc::now(),
                                    };
                                    session_clone.lock().unwrap().update_thread(thread);
                                } else if method_str.contains("turn/started") || method_str.contains("turnStarted") {
                                    // params: { turn: { id, ... }, threadId }
                                    let turn_id = params.get("turn")
                                        .and_then(|t| t.get("id"))
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("");
                                    let thread_id = params.get("threadId").and_then(|t| t.as_str()).unwrap_or("");
                                    println!("\n--- Turn started: {} (thread: {}) ---", &turn_id[..8.min(turn_id.len())], &thread_id[..8.min(thread_id.len())]);

                                    // Store run in session
                                    let run = Run {
                                        id: turn_id.to_string(),
                                        thread_id: thread_id.to_string(),
                                        status: RunStatus::InProgress,
                                        started_at: Utc::now(),
                                        completed_at: None,
                                    };
                                    let mut session = session_clone.lock().unwrap();
                                    session.add_run(run);
                                    // Update thread's current turn
                                    session.thread.current_turn_id = Some(turn_id.to_string());
                                    session.thread.status = ThreadStatus::Running;
                                    session.thread.updated_at = Utc::now();
                                } else if method_str.contains("turn/completed") || method_str.contains("turnCompleted") {
                                    // params: { threadId, turn: { id, ... } }
                                    let turn_id = params.get("turn")
                                        .and_then(|t| t.get("id"))
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("");
                                    if !turn_id.is_empty() {
                                        println!("--- Turn completed: {} ---", &turn_id[..8.min(turn_id.len())]);

                                        // Update run status in session
                                        let mut session = session_clone.lock().unwrap();
                                        if let Some(run) = session.runs.iter_mut().find(|r| r.id == turn_id) {
                                            run.status = RunStatus::Completed;
                                            run.completed_at = Some(Utc::now());
                                        }
                                        session.thread.updated_at = Utc::now();
                                    } else {
                                        println!("--- Turn completed ---");
                                    }
                                } else if method_str.contains("turn/plan/updated") || method_str.contains("plan/updated") {
                                    // params: { plan: [...], threadId, turnId, explanation? }
                                    let thread_id = params.get("threadId").and_then(|t| t.as_str()).unwrap_or("");
                                    let turn_id = params.get("turnId").and_then(|t| t.as_str()).unwrap_or("");
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

                                                // Store each plan step as a Plan
                                                let plan_id = format!("plan_{}_{}", turn_id, step);
                                                let plan_status = match status {
                                                    "completed" => PlanStatus::Completed,
                                                    "inProgress" => PlanStatus::InProgress,
                                                    _ => PlanStatus::Pending,
                                                };
                                                let plan = Plan {
                                                    id: plan_id,
                                                    text: step.to_string(),
                                                    intent_id: None,
                                                    thread_id: thread_id.to_string(),
                                                    turn_id: Some(turn_id.to_string()),
                                                    status: plan_status,
                                                    created_at: Utc::now(),
                                                };
                                                session_clone.lock().unwrap().add_plan(plan);
                                            }
                                        }
                                    }
                                } else if method_str.contains("initialized") {
                                    // Server initialized notification (after client sends initialize request)
                                    println!("[Codex] Server initialized");
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
                                    // println!("[Codex] {}", method_str);
                                }
                                // Handle thread/started
                                if method_str.contains("item/started") {
                                    // Get common fields
                                    let thread_id = params.get("threadId").and_then(|t| t.as_str()).unwrap_or("");
                                    let turn_id = params.get("turnId").and_then(|t| t.as_str()).unwrap_or("");

                                    // params.item.type contains the type
                                    if let Some(item) = params.get("item") {
                                        if let Some(item_type) = item.get("type").and_then(|t| t.as_str()) {
                                            let item_id = item.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();

                                            // Get current run_id
                                            let run_id = turn_id.to_string();

                                            // Get tool name if it's a tool call
                                            if item_type == "mcpToolCall" {
                                                let tool = item.get("tool").and_then(|t| t.as_str()).unwrap_or("unknown");
                                                let server = item.get("server").and_then(|s| s.as_str()).unwrap_or("");
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
                                                session_clone.lock().unwrap().add_tool_invocation(invocation);

                                            } else if item_type == "toolCall" {
                                                let tool = item.get("name").or_else(|| item.get("tool"))
                                                    .and_then(|t| t.as_str()).unwrap_or("unknown");
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
                                                session_clone.lock().unwrap().add_tool_invocation(invocation);

                                            } else if item_type == "commandExecution" {
                                                let cmd = item.get("command").and_then(|c| c.as_str()).unwrap_or("");
                                                println!("  Command: {} started", cmd);

                                                // Store ToolInvocation
                                                let invocation = ToolInvocation {
                                                    id: item_id.clone(),
                                                    run_id: run_id.clone(),
                                                    thread_id: thread_id.to_string(),
                                                    tool_name: "commandExecution".to_string(),
                                                    server: None,
                                                    arguments: Some(serde_json::json!({ "command": cmd })),
                                                    result: None,
                                                    error: None,
                                                    status: ToolStatus::InProgress,
                                                    duration_ms: item.get("durationMs").and_then(|d| d.as_i64()),
                                                    created_at: Utc::now(),
                                                };
                                                session_clone.lock().unwrap().add_tool_invocation(invocation);

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
                                                session_clone.lock().unwrap().add_reasoning(reasoning);

                                            } else if item_type == "plan" {
                                                // Plan item - show the plan text
                                                let text = item.get("text").and_then(|t| t.as_str()).unwrap_or("");
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
                                                session_clone.lock().unwrap().add_plan(plan);

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
                                                session_clone.lock().unwrap().add_patchset(patchset);

                                            } else if item_type == "dynamicToolCall" {
                                                // Dynamic tool call
                                                let tool = item.get("tool").and_then(|t| t.as_str()).unwrap_or("unknown");
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
                                                    duration_ms: item.get("durationMs").and_then(|d| d.as_i64()),
                                                    created_at: Utc::now(),
                                                };
                                                session_clone.lock().unwrap().add_tool_invocation(invocation);

                                            } else if item_type == "webSearch" {
                                                // Web search
                                                let query = item.get("query").and_then(|q| q.as_str()).unwrap_or("");
                                                println!("  Web Search: {}", query);

                                                // Store ToolInvocation
                                                let invocation = ToolInvocation {
                                                    id: item_id.clone(),
                                                    run_id: run_id.clone(),
                                                    thread_id: thread_id.to_string(),
                                                    tool_name: "webSearch".to_string(),
                                                    server: None,
                                                    arguments: Some(serde_json::json!({ "query": query })),
                                                    result: None,
                                                    error: None,
                                                    status: ToolStatus::InProgress,
                                                    duration_ms: None,
                                                    created_at: Utc::now(),
                                                };
                                                session_clone.lock().unwrap().add_tool_invocation(invocation);

                                            } else if item_type == "userMessage" {
                                                // User message -> Intent
                                                let content = item.get("content").and_then(|c| c.as_array())
                                                    .and_then(|arr| arr.first())
                                                    .and_then(|first| first.get("text"))
                                                    .and_then(|t| t.as_str())
                                                    .unwrap_or("");
                                                let truncated = if content.len() > 50 { &content[..50] } else { content };
                                                println!("  User: {}", truncated);

                                                // Store Intent
                                                let intent = Intent {
                                                    id: item_id.clone(),
                                                    content: content.to_string(),
                                                    thread_id: thread_id.to_string(),
                                                    created_at: Utc::now(),
                                                };
                                                session_clone.lock().unwrap().add_intent(intent);

                                            } else if item_type == "agentMessage" {
                                                // Agent message - will stream
                                                let content = item.get("text").and_then(|t| t.as_str()).unwrap_or("");
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
                                }
                                // Handle item/completed notification
                                else if method_str.contains("item/completed") {
                                    let thread_id = params.get("threadId").and_then(|t| t.as_str()).unwrap_or("");
                                    let turn_id = params.get("turnId").and_then(|t| t.as_str()).unwrap_or("");

                                    if let Some(item) = params.get("item") {
                                        if let Some(item_type) = item.get("type").and_then(|t| t.as_str()) {
                                            let item_id = item.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();

                                            if item_type == "mcpToolCall" {
                                                let tool = item.get("tool").and_then(|t| t.as_str()).unwrap_or("unknown");
                                                let status = item.get("status").and_then(|s| s.as_str()).unwrap_or("completed");
                                                let result = item.get("result").cloned();
                                                let error = item.get("error").and_then(|e| e.as_str()).map(|s| s.to_string());
                                                let duration_ms = item.get("durationMs").and_then(|d| d.as_i64());

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

                                                // Update ToolInvocation status
                                                let mut session = session_clone.lock().unwrap();
                                                if let Some(invocation) = session.tool_invocations.iter_mut().find(|i| i.id == item_id) {
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
                                                let cmd = item.get("command").and_then(|c| c.as_str()).unwrap_or("");
                                                let exit_code = item.get("exitCode").and_then(|c| c.as_i64());
                                                let duration_ms = item.get("durationMs").and_then(|d| d.as_i64());
                                                let output = item.get("aggregatedOutput").and_then(|o| o.as_str());

                                                println!("  Command: {} exit={:?}", cmd, exit_code);

                                                // Update ToolInvocation status
                                                let mut session = session_clone.lock().unwrap();
                                                if let Some(invocation) = session.tool_invocations.iter_mut().find(|i| i.id == item_id) {
                                                    invocation.status = match exit_code {
                                                        Some(0) => ToolStatus::Completed,
                                                        Some(_) => ToolStatus::Failed,
                                                        None => ToolStatus::Completed,
                                                    };
                                                    invocation.result = output.map(|o| serde_json::json!({ "output": o }));
                                                    invocation.duration_ms = duration_ms;
                                                }

                                            } else if item_type == "reasoning" {
                                                println!("  Thinking completed");

                                                // Update Reasoning
                                                let summary = item.get("summary")
                                                    .and_then(|s| s.as_array())
                                                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                                                    .unwrap_or_default();
                                                let text = item.get("text").and_then(|t| t.as_str()).map(String::from);

                                                let mut session = session_clone.lock().unwrap();
                                                if let Some(reasoning) = session.reasonings.iter_mut().find(|r| r.id == item_id) {
                                                    reasoning.summary = summary;
                                                    reasoning.text = text;
                                                }

                                            } else if item_type == "plan" {
                                                // Plan item - show the plan text
                                                let text = item.get("text").and_then(|t| t.as_str()).unwrap_or("");
                                                if !text.is_empty() {
                                                    println!("  Plan completed: {}", text);
                                                } else {
                                                    println!("  Plan completed");
                                                }

                                                // Update Plan status
                                                let mut session = session_clone.lock().unwrap();
                                                if let Some(plan) = session.plans.iter_mut().find(|p| p.id == item_id) {
                                                    plan.status = PlanStatus::Completed;
                                                    if !text.is_empty() {
                                                        plan.text = text.to_string();
                                                    }
                                                }

                                            } else if item_type == "fileChange" {
                                                // File change - show files and diff
                                                let status = item.get("status").and_then(|s| s.as_str()).unwrap_or("completed");
                                                let changes = item.get("changes").and_then(|c| c.as_array());

                                                println!("  📝 File Change: {}", status);

                                                // Parse and update PatchSet
                                                let mut file_changes = vec![];
                                                if let Some(changes) = changes {
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
                                                        // Show diff if available (full content)
                                                        let diff = change.get("diff").and_then(|d| d.as_str()).unwrap_or("");
                                                        if !diff.is_empty() {
                                                            for line in diff.lines() {
                                                                println!("      {}", line);
                                                            }
                                                            file_changes.push(FileChange {
                                                                path: path.to_string(),
                                                                diff: diff.to_string(),
                                                                change_type: kind.to_string(),
                                                            });
                                                        }
                                                    }
                                                    if changes.len() > 5 {
                                                        println!("    ... and {} more", changes.len() - 5);
                                                    }
                                                }

                                                // Update PatchSet
                                                let mut session = session_clone.lock().unwrap();
                                                if let Some(patchset) = session.patchsets.iter_mut().find(|p| p.id == item_id) {
                                                    patchset.status = match status {
                                                        "completed" => PatchStatus::Completed,
                                                        "failed" => PatchStatus::Failed,
                                                        "declined" => PatchStatus::Declined,
                                                        _ => PatchStatus::Completed,
                                                    };
                                                    if !file_changes.is_empty() {
                                                        patchset.changes = file_changes;
                                                    }
                                                }

                                            } else if item_type == "dynamicToolCall" {
                                                let tool = item.get("tool").and_then(|t| t.as_str()).unwrap_or("unknown");
                                                let status = item.get("status").and_then(|s| s.as_str()).unwrap_or("completed");
                                                let duration_ms = item.get("durationMs").and_then(|d| d.as_i64());
                                                let success = item.get("success").and_then(|s| s.as_bool());

                                                println!("  Dynamic Tool: {} - {}", tool, status);

                                                // Update ToolInvocation status
                                                let mut session = session_clone.lock().unwrap();
                                                if let Some(invocation) = session.tool_invocations.iter_mut().find(|i| i.id == item_id) {
                                                    invocation.status = match (status, success) {
                                                        ("completed", Some(true)) | ("completed", None) => ToolStatus::Completed,
                                                        ("failed", _) | (_, Some(false)) => ToolStatus::Failed,
                                                        _ => ToolStatus::Completed,
                                                    };
                                                    invocation.duration_ms = duration_ms;
                                                }

                                            } else if item_type == "webSearch" {
                                                let query = item.get("query").and_then(|q| q.as_str()).unwrap_or("");
                                                println!("  Web Search done: {}", query);

                                                // Update ToolInvocation status
                                                let mut session = session_clone.lock().unwrap();
                                                if let Some(invocation) = session.tool_invocations.iter_mut().find(|i| i.id == item_id) {
                                                    invocation.status = ToolStatus::Completed;
                                                }

                                            } else if item_type == "agentMessage" {
                                                println!("\n  ✅ Agent Response completed\n");

                                                // Update AgentMessage content
                                                let content = item.get("text").and_then(|t| t.as_str()).map(String::from).unwrap_or_default();
                                                let mut session = session_clone.lock().unwrap();
                                                if let Some(msg) = session.agent_messages.iter_mut().find(|m| m.id == item_id) {
                                                    msg.content = content;
                                                }

                                            } else {
                                                println!("  Task: {} completed", item_type);
                                            }
                                        }
                                    }
                                }
                                // Handle agent message delta - direct text output
                                // Only handle specific delta types to avoid duplicates
                                else if method_str.contains("agentMessage") || method_str.contains("agent_message") {
                                    // Only process agent_message_content_delta to avoid duplicates
                                    // Codex sends the same content in multiple formats, we only need one
                                    let msg_type = params.get("type")
                                        .or_else(|| params.get("msg").and_then(|m| m.get("type")))
                                        .and_then(|t| t.as_str());

                                    // Only handle content_delta types, skip others to avoid duplicates
                                    if !msg_type.map(|t| t.contains("content_delta")).unwrap_or(false) {
                                        continue;
                                    }

                                    // Debug: print raw delta when debug mode is enabled
                                    if args.debug {
                                        eprintln!("[DEBUG] agentMessage delta: {:?}", params);
                                    }

                                    // Check for delta at different levels
                                    let delta = params.get("delta")
                                        .or_else(|| params.get("msg").and_then(|m| m.get("delta")))
                                        .or_else(|| params.get("text"))
                                        .and_then(|d| d.as_str());

                                    if let Some(text) = delta {
                                        // Only print non-empty, non-whitespace-only text
                                        if !text.trim().is_empty() {
                                            // Stream text with proper handling
                                            print!("{}", text);
                                            use std::io::Write;
                                            std::io::stdout().flush().ok();
                                        }
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
                                        let output_str = output.as_str().unwrap_or("");
                                        if !output_str.trim().is_empty() {
                                            print!("{}", output_str);
                                            use std::io::Write;
                                            std::io::stdout().flush().ok();
                                        }
                                    }
                                }
                                // Handle file change output delta (diff streaming)
                                else if method_str.contains("fileChange/outputDelta") || method_str.contains("filechange/outputDelta") {
                                    if let Some(delta) = params.get("delta").or_else(|| params.get("output")) {
                                        let delta_str = delta.as_str().unwrap_or("");
                                        if !delta_str.trim().is_empty() {
                                            print!("{}", delta_str);
                                            use std::io::Write;
                                            std::io::stdout().flush().ok();
                                        }
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
                                        if !text.trim().is_empty() {
                                            // Stream thinking directly
                                            print!("{}", text);
                                            use std::io::Write;
                                            std::io::stdout().flush().ok();
                                        }
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

                            // Handle approval requests (as notification or request)
                            // ServerRequest methods: item/commandExecution/requestApproval, item/fileChange/requestApproval
                            // Also check for exec_approval_request, apply_patch_approval_request types
                            let is_approval_request = method_str.contains("requestApproval")
                                || method_str.contains("exec_approval_request")
                                || method_str.contains("apply_patch_approval");

                            if is_approval_request {
                                // Check if it's a request (has id) or notification (no id)
                                let is_request = json.get("id").is_some();
                                if is_request {
                                    println!("\n⚠️  Approval Request (requires response):");
                                } else {
                                    println!("\n⚠️  Approval Notification:");
                                }
                                println!("  Method: {}", method_str);

                                // Get approval ID from different possible fields
                                let request_id = json.get("params")
                                    .and_then(|p| p.get("requestId"))
                                    .or_else(|| json.get("params").and_then(|p| p.get("approvalId")))
                                    .or_else(|| json.get("params").and_then(|p| p.get("call_id")))
                                    .cloned()
                                    .and_then(|v| v.as_str().map(String::from))
                                    .unwrap_or_else(|| format!("approval_{}", Utc::now().timestamp_millis()));
                                let approval_params = json.get("params").cloned().unwrap_or(serde_json::json!({}));

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
                                let item_id = approval_params.get("itemId")
                                    .or_else(|| approval_params.get("call_id"))
                                    .and_then(|v| v.as_str())
                                    .map(String::from)
                                    .unwrap_or_default();

                                // Get thread_id if available
                                let thread_id = approval_params.get("threadId")
                                    .and_then(|v| v.as_str())
                                    .map(String::from)
                                    .unwrap_or_default();

                                // Get command or changes from approval_params
                                let command = approval_params.get("command").and_then(|v| v.as_str()).map(String::from);
                                let changes = approval_params.get("changes").and_then(|c| c.as_array())
                                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect());
                                let description: Option<String> = approval_params.get("description").and_then(|v| v.as_str()).map(String::from);

                                // Store approval request in session
                                let approval_request = ApprovalRequest {
                                    id: request_id.clone(),
                                    approval_type,
                                    item_id,
                                    thread_id,
                                    run_id: None,
                                    command,
                                    changes,
                                    description,
                                    decision: None,
                                    requested_at: Utc::now(),
                                    resolved_at: None,
                                };
                                {
                                    let mut session = session_clone.lock().unwrap();
                                    session.add_approval_request(approval_request);
                                }

                                let approved = if approval_mode == "accept" {
                                    // Auto-accept
                                    println!("[Auto-approved]");
                                    true
                                } else if approval_mode == "decline" {
                                    // Auto-decline
                                    println!("[Auto-declined]");
                                    false
                                } else {
                                    // Ask mode - send to main loop for interactive input
                                    let (oneshot_tx, oneshot_rx) = tokio::sync::oneshot::channel::<bool>();
                                    let _ = approval_tx_clone.send((approval_params.clone(), oneshot_tx)).await;

                                    // Wait for user response
                                    match oneshot_rx.await {
                                        Ok(approved) => {
                                            println!("[User {}]", if approved { "approved" } else { "declined" });
                                            approved
                                        }
                                        Err(_) => {
                                            println!("[Timeout - auto-approved by default]");
                                            true
                                        }
                                    }
                                };

                                // Update approval request with decision
                                {
                                    let mut session = session_clone.lock().unwrap();
                                    if let Some(approval) = session.approval_requests.iter_mut().find(|a| a.id == request_id) {
                                        approval.decision = Some(approved);
                                        approval.resolved_at = Some(Utc::now());
                                    }
                                }

                                // Use the correct resolve method based on the request type
                                let resolve_method = if method_str.contains("commandExecution") {
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
            approval_req = approval_rx.recv() => {
                if let Some((params, response_tx)) = approval_req {
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

                    // Read user input for approval
                    use std::io::{BufRead, BufReader};
                    let stdin = std::io::stdin();
                    let mut reader = BufReader::new(stdin);
                    let mut input = String::new();
                    if reader.read_line(&mut input).is_ok() {
                        let choice = input.trim().to_lowercase();
                        let approved = match choice.as_str() {
                            "a" | "accept" => {
                                println!("  → Accepted");
                                true
                            }
                            "d" | "decline" => {
                                println!("  → Declined");
                                false
                            }
                            "A" | "A" if choice == "A" || choice == "accept all" => {
                                println!("  → Accepted (will auto-accept future)");
                                true
                            }
                            "D" | "D" if choice == "D" || choice == "decline all" => {
                                println!("  → Declined (will auto-decline future)");
                                false
                            }
                            _ => {
                                println!("  → Default accept");
                                true
                            }
                        };
                        let _ = response_tx.send(approved);
                    } else {
                        let _ = response_tx.send(true);
                    }
                    println!();
                }
            }
            _ = tokio::time::sleep(tokio::time::Duration::from_secs(1)) => {}
        }
    }
}
