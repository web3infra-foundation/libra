//! Protocol handling for Agent Codex WebSocket communication.
//! Handles all notification processing and message parsing.

use chrono::Utc;
use serde_json::Value;

use super::types::*;

// =============================================================================
// Protocol Handler
// =============================================================================

/// Handles incoming WebSocket messages and updates the session
pub struct ProtocolHandler {
    debug: bool,
    approval_mode: String,
}

impl ProtocolHandler {
    pub fn new(debug: bool, approval_mode: String) -> Self {
        Self { debug, approval_mode }
    }

    /// Process a notification message and update session
    pub fn process_notification(
        &self,
        method_str: &str,
        params: &Value,
        session: &mut CodexSession,
    ) -> Option<NotificationAction> {
        // Show hierarchical flow: Thread → Turn → Plan → Item → Detail
        if method_str.contains("thread/started") {
            // params: { thread: { threadId, ... } }
            let thread_id = params
                .get("thread")
                .and_then(|t| t.get("threadId"))
                .and_then(|t| t.as_str())
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
            session.update_thread(thread);
            return Some(NotificationAction::ThreadStarted(thread_id.to_string()));
        } else if method_str.contains("thread/status/changed") {
            // params: { threadId, status }
            let thread_id = params
                .get("threadId")
                .and_then(|t| t.as_str())
                .unwrap_or("");
            let status = params.get("status").and_then(|s| s.as_str()).unwrap_or("");

            let new_status = match status {
                "pending" => ThreadStatus::Pending,
                "running" => ThreadStatus::Running,
                "completed" => ThreadStatus::Completed,
                "archived" => ThreadStatus::Archived,
                "closed" => ThreadStatus::Closed,
                _ => ThreadStatus::Running,
            };

            if self.debug {
                eprintln!(
                    "[DEBUG] Thread status changed: {} -> {:?}",
                    thread_id, new_status
                );
            }

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

            if self.debug {
                eprintln!("[DEBUG] Thread name updated: {} -> {:?}", thread_id, name);
            }

            session.thread.name = name;
            session.thread.updated_at = Utc::now();
        } else if method_str.contains("thread/archived") {
            let thread_id = params.get("threadId").and_then(|t| t.as_str()).unwrap_or("");
            if self.debug {
                eprintln!("[DEBUG] Thread archived: {}", thread_id);
            }
            session.thread.status = ThreadStatus::Archived;
            session.thread.updated_at = Utc::now();
        } else if method_str.contains("thread/closed") {
            let thread_id = params.get("threadId").and_then(|t| t.as_str()).unwrap_or("");
            if self.debug {
                eprintln!("[DEBUG] Thread closed: {}", thread_id);
            }
            session.thread.status = ThreadStatus::Closed;
            session.thread.updated_at = Utc::now();
        } else if method_str.contains("turn/started") || method_str.contains("turnStarted") {
            // params: { turn: { id, ... }, threadId }
            let turn_id = params
                .get("turn")
                .and_then(|t| t.get("id"))
                .and_then(|t| t.as_str())
                .unwrap_or("");
            let thread_id = params.get("threadId").and_then(|t| t.as_str()).unwrap_or("");
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
            session.add_run(run);
            // Update thread's current turn
            session.thread.current_turn_id = Some(turn_id.to_string());
            session.thread.status = ThreadStatus::Running;
            session.thread.updated_at = Utc::now();
        } else if method_str.contains("turn/completed") || method_str.contains("turnCompleted") {
            let turn_id = params
                .get("turn")
                .and_then(|t| t.get("id"))
                .and_then(|t| t.as_str())
                .unwrap_or("");
            if !turn_id.is_empty() {
                println!("--- Turn completed: {} ---", &turn_id[..8.min(turn_id.len())]);

                // Update run status in session
                if let Some(run) = session.runs.iter_mut().find(|r| r.id == turn_id) {
                    run.status = RunStatus::Completed;
                    run.completed_at = Some(Utc::now());
                }
                session.thread.updated_at = Utc::now();
            } else {
                println!("--- Turn completed ---");
            }
        } else if method_str.contains("tokenUsage") {
            // params: { threadId, turnId, tokenUsage: { last, total, modelContextWindow? } }
            let thread_id = params.get("threadId").and_then(|t| t.as_str()).unwrap_or("");
            let turn_id = params.get("turnId").and_then(|t| t.as_str()).unwrap_or("");

            // Parse token usage
            let last = params
                .get("tokenUsage")
                .and_then(|tu| tu.get("last"))
                .cloned()
                .unwrap_or(Value::Null);
            let total = params
                .get("tokenUsage")
                .and_then(|tu| tu.get("total"))
                .cloned()
                .unwrap_or(Value::Null);
            let model_context_window = params
                .get("tokenUsage")
                .and_then(|tu| tu.get("modelContextWindow"))
                .and_then(|m| m.as_i64());

            let parse_token = |v: &Value| -> TokenUsage {
                TokenUsage {
                    cached_input_tokens: v.get("cachedInputTokens").and_then(|c| c.as_i64()),
                    input_tokens: v.get("inputTokens").and_then(|i| i.as_i64()),
                    output_tokens: v.get("outputTokens").and_then(|o| o.as_i64()),
                    reasoning_output_tokens: v
                        .get("reasoningOutputTokens")
                        .and_then(|r| r.as_i64()),
                    total_tokens: v.get("totalTokens").and_then(|t| t.as_i64()),
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

            if self.debug {
                eprintln!(
                    "[DEBUG] TokenUsage: turn={}, total_tokens={}",
                    turn_id,
                    usage.total.total_tokens.unwrap_or(0)
                );
            }
            session.add_token_usage(usage);
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
                if let Ok(plan_array) = serde_json::from_str::<Vec<Value>>(&plan.to_string()) {
                    for item in plan_array.iter() {
                        let status = item.get("status").and_then(|s| s.as_str()).unwrap_or("unknown");
                        let step = item.get("step").and_then(|s| s.as_str()).unwrap_or("");
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
                            id: plan_id,
                            text: step.to_string(),
                            intent_id: None,
                            thread_id: thread_id.to_string(),
                            turn_id: Some(turn_id.to_string()),
                            status: plan_status,
                            created_at: Utc::now(),
                        };
                        session.add_plan(plan);
                    }
                }
            }
        } else if method_str.contains("initialized") {
            println!("[Codex] Server initialized");
        } else if method_str.contains("codex/event/task_started") {
            let thread_id = params.get("threadId").and_then(|t| t.as_str()).unwrap_or("");
            let task_id = params.get("taskId").and_then(|t| t.as_str()).unwrap_or("");
            let task_name = params.get("taskName").and_then(|t| t.as_str()).unwrap_or("");

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
            session.add_task(task);
        } else if method_str.contains("codex/event/task_complete") {
            println!("\n✅ Task Completed");
        }

        // Handle item/started
        if method_str.contains("item/started") {
            // Get common fields
            let thread_id = params.get("threadId").and_then(|t| t.as_str()).unwrap_or("");
            let turn_id = params.get("turnId").and_then(|t| t.as_str()).unwrap_or("");

            // params.item.type contains the type
            if let Some(item) = params.get("item")
                && let Some(item_type) = item.get("type").and_then(|t| t.as_str()) {
                    let item_id = item
                        .get("id")
                        .and_then(|i| i.as_str())
                        .unwrap_or("")
                        .to_string();

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
                        session.add_tool_invocation(invocation);
                        return Some(NotificationAction::ToolInvocationStarted(item_id, tool.to_string(), Some(server.to_string())));
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
                        session.add_tool_invocation(invocation);
                        return Some(NotificationAction::ToolInvocationStarted(item_id, tool.to_string(), None));
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
                        session.add_tool_invocation(invocation);
                        return Some(NotificationAction::CommandStarted(item_id, cmd.to_string()));
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
                        session.add_reasoning(reasoning);
                        return Some(NotificationAction::ReasoningStarted(item_id));
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
                        session.add_plan(plan);
                        return Some(NotificationAction::PlanStarted(item_id));
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
                        session.add_patchset(patchset);
                        return Some(NotificationAction::FileChangeStarted(item_id));
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
                        session.add_tool_invocation(invocation);
                        return Some(NotificationAction::ToolInvocationStarted(item_id, tool.to_string(), None));
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
                        session.add_tool_invocation(invocation);
                        return Some(NotificationAction::WebSearchStarted(item_id, query.to_string()));
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
                        session.add_intent(intent);
                        return Some(NotificationAction::UserMessageReceived(item_id));
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
                        session.add_agent_message(msg);
                        return Some(NotificationAction::AgentMessageStarted(item_id));
                    } else {
                        println!("  Task: {} started", item_type);
                    }
                }
        }
        // Handle item/completed notification
        else if method_str.contains("item/completed") {
            let _thread_id = params.get("threadId").and_then(|t| t.as_str()).unwrap_or("");
            let _turn_id = params.get("turnId").and_then(|t| t.as_str()).unwrap_or("");

            if let Some(item) = params.get("item")
                && let Some(item_type) = item.get("type").and_then(|t| t.as_str()) {
                    let item_id = item
                        .get("id")
                        .and_then(|i| i.as_str())
                        .unwrap_or("")
                        .to_string();

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
                        let summary = item
                            .get("summary")
                            .and_then(|s| s.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|v| v.as_str().map(String::from))
                                    .collect()
                            })
                            .unwrap_or_default();
                        let text = item.get("text").and_then(|t| t.as_str()).map(String::from);

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
                        if let Some(plan) = session.plans.iter_mut().find(|p| p.id == item_id) {
                            plan.status = PlanStatus::Completed;
                            if !text.is_empty() {
                                plan.text = text.to_string();
                            }
                        }
                    } else if item_type == "fileChange" {
                        // File change - show files and diff
                        let status = item.get("status").and_then(|s| s.as_str()).unwrap_or("");

                        // Parse changes
                        let changes: Vec<FileChange> = item
                            .get("changes")
                            .and_then(|c| c.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|change| {
                                        let path = change.get("path")?.as_str()?.to_string();
                                        let diff = change.get("diff").and_then(|d| d.as_str()).unwrap_or("").to_string();
                                        let change_type = change
                                            .get("change_type")
                                            .or_else(|| change.get("changeType"))
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

                        let file_count = changes.len();
                        println!("  📝 File Change {} ({} files)", status, file_count);

                        // Show first few files
                        for change in changes.iter().take(5) {
                            println!("    - {} ({})", change.path, change.change_type);
                        }
                        if file_count > 5 {
                            println!("    ... and {} more", file_count - 5);
                        }

                        // Update PatchSet status
                        if let Some(patchset) = session.patchsets.iter_mut().find(|p| p.id == item_id) {
                            patchset.status = match status {
                                "completed" => PatchStatus::Completed,
                                "failed" => PatchStatus::Failed,
                                "declined" => PatchStatus::Declined,
                                _ => PatchStatus::Completed,
                            };
                            patchset.changes = changes;
                        }
                    } else if item_type == "toolCall" {
                        let tool = item.get("name").or_else(|| item.get("tool")).and_then(|t| t.as_str()).unwrap_or("unknown");
                        let status = item.get("status").and_then(|s| s.as_str()).unwrap_or("completed");
                        let result = item.get("result").cloned();
                        let error = item.get("error").and_then(|e| e.as_str()).map(String::from);
                        let duration_ms = item.get("durationMs").and_then(|d| d.as_i64());

                        print!("  Tool: {} - {}", tool, status);
                        if let Some(result) = item.get("result") {
                            let result_str = result.to_string();
                            if result_str.len() > 100 {
                                println!(" | Result: {}...", &result_str[..100]);
                            } else if !result_str.is_empty() && result_str != "null" {
                                println!(" | Result: {}", result_str);
                            } else {
                                println!();
                            }
                        } else {
                            println!();
                        }

                        // Update ToolInvocation status
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
                    } else if item_type == "userMessage" {
                        // Update intent if needed
                        println!("  User message completed");
                    } else if item_type == "agentMessage" {
                        // Update agent message content
                        let content = item.get("text").and_then(|t| t.as_str()).unwrap_or("");
                        if let Some(msg) = session.agent_messages.iter_mut().find(|m| m.id == item_id) {
                            msg.content = content.to_string();
                        }
                        println!("  🤖 Agent Response completed");
                    }
                }
        }

        None
    }

    /// Handle approval request
    pub fn handle_approval_request(
        &self,
        request_id: &str,
        method_str: &str,
        approval_params: &Value,
    ) -> bool {
        // Determine approval type
        let _approval_type = if method_str.contains("commandExecution") {
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
        let _approval_request = ApprovalRequest {
            id: request_id.to_string(),
            approval_type: _approval_type,
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

        // Use the correct resolve method based on the request type
        let _resolve_method = if method_str.contains("commandExecution") {
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

        let approved = if self.approval_mode == "accept" {
            // Auto-accept
            println!("[Auto-approved]");
            true
        } else if self.approval_mode == "decline" {
            // Auto-decline
            println!("[Auto-declined]");
            false
        } else {
            // Ask mode - return false to indicate we need user input
            false
        };

        // Return approval data
        if !approved {
            // For ask mode, we'll handle it in the main loop
            // This is a placeholder - actual user interaction happens in execute
        }

        approved
    }

    /// Get resolve method for approval type
    pub fn get_resolve_method(method_str: &str) -> &'static str {
        if method_str.contains("commandExecution") {
            "item/commandExecution/requestApproval/resolve"
        } else if method_str.contains("fileChange") {
            "item/fileChange/requestApproval/resolve"
        } else if method_str.contains("exec_approval") {
            "exec_approval_request/resolve"
        } else if method_str.contains("apply_patch") {
            "apply_patch_approval_request/resolve"
        } else {
            "requestApproval/resolve"
        }
    }
}

/// Actions that need to be performed after processing notifications
#[derive(Debug, Clone)]
pub enum NotificationAction {
    ThreadStarted(String),
    ToolInvocationStarted(String, String, Option<String>),
    CommandStarted(String, String),
    ReasoningStarted(String),
    PlanStarted(String),
    FileChangeStarted(String),
    WebSearchStarted(String, String),
    UserMessageReceived(String),
    AgentMessageStarted(String),
}
