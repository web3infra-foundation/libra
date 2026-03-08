//! Main application structure and event loop.
//!
//! The `App` struct manages the TUI state and coordinates between
//! user input, agent execution, and UI rendering.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use crossterm::event::{KeyCode, KeyModifiers};
use tokio::{
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    task::JoinHandle,
    time::{interval, sleep},
};
use tokio_stream::StreamExt;

use super::{
    app_event::{AgentEvent, AgentStatus, AppEvent, ExitMode},
    chatwidget::ChatWidget,
    diff::FileChange,
    history_cell::{
        AssistantHistoryCell, DiffHistoryCell, PlanUpdateHistoryCell, ToolCallHistoryCell,
        UserHistoryCell,
    },
    terminal::{TARGET_FRAME_INTERVAL, Tui, TuiEvent},
};
use crate::{
    cli_error,
    internal::ai::{
        agent::{
            ToolLoopConfig, profile::AgentProfileRouter, run_tool_loop_with_history_and_observer,
        },
        commands::CommandDispatcher,
        completion::{
            CompletionModel, CompletionRetryEvent, CompletionRetryObserver, CompletionRetryPolicy,
            Message, RetryingCompletionModel,
        },
        intentspec::{
            IntentDraft, ResolveContext, RiskLevel, render_summary, repair_intentspec,
            resolve_intentspec, validate_intentspec,
        },
        mcp::{
            resource::{
                CreateContextSnapshotParams, CreateDecisionParams, CreateIntentParams,
                CreatePlanParams, CreateRunParams, CreateToolInvocationParams,
            },
            server::LibraMcpServer,
        },
        orchestrator::{planner::compile_execution_plan_spec, types::ExecutionPlanSpec},
        session::{SessionState, SessionStore},
        tools::{
            ToolOutput, ToolRegistry,
            context::{
                RequestUserInputArgs, SubmitIntentDraftArgs, UpdatePlanArgs, UserInputAnswer,
                UserInputRequest, UserInputResponse,
            },
        },
    },
};

/// MCP resource IDs for tracking the workflow
#[derive(Debug, Clone, Default)]
pub struct McpIds {
    pub _intent_id: Option<String>,
    pub plan_id: Option<String>,
    pub task_id: Option<String>,
    pub run_id: Option<String>,
    pub _context_snapshot_id: Option<String>,
}

const LATEST_INTENTSPEC_INTENT_ID: &str = "latest_intentspec_intent_id";
const LATEST_EXECUTION_PLAN_ID: &str = "latest_execution_plan_id";
const LATEST_INTENTSPEC_JSON: &str = "latest_intentspec_json";
const MAX_INTENTSPEC_REPAIR_ATTEMPTS: usize = 2;

fn summarize_mcp_content(content: &[rmcp::model::Content]) -> Option<String> {
    let mut parts = Vec::new();
    for item in content {
        if let Some(text) = item
            .as_text()
            .map(|text| text.text.trim())
            .filter(|text| !text.is_empty())
        {
            parts.push(text.to_string());
        }
    }

    if !parts.is_empty() {
        return Some(parts.join(" | "));
    }

    serde_json::to_string(content)
        .ok()
        .filter(|text| !text.trim().is_empty())
}

fn render_mcp_error(context: &str, content: Vec<rmcp::model::Content>) {
    if let Some(content) = summarize_mcp_content(&content) {
        cli_error!("error" => format!("{context}: {content}"));
    } else {
        cli_error!("error" => context);
    }
}

/// The reason for exiting the application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitReason {
    /// User requested exit.
    UserRequested,
    /// Fatal error occurred.
    Fatal(String),
}

/// Information about the app exit state.
#[derive(Debug, Clone)]
pub struct AppExitInfo {
    /// The reason for exiting.
    pub reason: ExitReason,
}

/// Pending user-input state while the TUI waits for the user to answer.
struct PendingUserInput {
    /// The original request (questions, etc.).
    request: UserInputRequest,
    /// Index of the question currently being answered.
    current_question: usize,
    /// Answers collected so far, keyed by question id.
    answers: HashMap<String, UserInputAnswer>,
    /// Currently selected option index (0-based) for the active question.
    selected_option: usize,
    /// Whether the notes input is currently focused (Tab toggles).
    notes_focused: bool,
    /// Notes text being composed for the current question.
    notes_text: String,
}

/// Post-plan dialog state: stores the spec and user selection.
struct PendingPostPlan {
    spec_json: String,
    selected: usize, // 0=Execute, 1=Modify, 2=Cancel
}

/// Configuration for creating an App.
pub struct AppConfig {
    pub welcome_message: String,
    pub command_dispatcher: CommandDispatcher,
    pub agent_router: AgentProfileRouter,
    pub session: SessionState,
    pub session_store: SessionStore,
    pub user_input_rx: UnboundedReceiver<UserInputRequest>,
    /// Display name of the active model (e.g. "gemini-2.5-flash").
    pub model_name: String,
    /// Provider identifier (e.g. "gemini", "anthropic").
    pub provider_name: String,
    /// MCP server instance for workflow tracking.
    pub mcp_server: Option<Arc<LibraMcpServer>>,
}

/// The main application struct.
pub struct App<M: CompletionModel> {
    /// The TUI instance.
    tui: Tui,
    /// The chat widget.
    widget: ChatWidget,
    /// The completion model used by the agent loop.
    model: RetryingCompletionModel<M>,
    /// The tool registry.
    registry: Arc<ToolRegistry>,
    /// Tool loop runtime config.
    config: ToolLoopConfig,
    /// Default tool allow-list for regular chat turns.
    default_allowed_tools: Vec<String>,
    /// Conversation history (model-facing).
    history: Vec<Message>,
    /// Receiver for app events.
    app_event_rx: UnboundedReceiver<AppEvent>,
    /// Sender for app events.
    app_event_tx: UnboundedSender<AppEvent>,
    /// Whether the app should exit.
    should_exit: bool,
    /// The exit info, if any.
    exit_info: Option<AppExitInfo>,
    /// Last draw time for frame rate control.
    last_draw_time: Instant,
    /// Background agent task handle (used for interrupt).
    agent_task: Option<JoinHandle<()>>,
    /// Delayed draw task for frame coalescing inside frame interval.
    scheduled_draw_task: Option<JoinHandle<()>>,
    /// Initial welcome message.
    welcome_message: String,
    /// Slash command dispatcher.
    command_dispatcher: CommandDispatcher,
    /// Agent router for auto-selection.
    agent_router: AgentProfileRouter,
    /// Session state for persistence.
    session: SessionState,
    /// Session store for saving/loading.
    session_store: SessionStore,
    /// Receiver for user-input requests from the `request_user_input` tool handler.
    user_input_rx: UnboundedReceiver<UserInputRequest>,
    /// Currently pending user-input interaction, if any.
    pending_user_input: Option<PendingUserInput>,
    /// Post-plan dialog state (present when user is choosing Execute/Modify/Cancel).
    pending_post_plan: Option<PendingPostPlan>,
    /// Display name of the active model.
    model_name: String,
    /// Provider identifier.
    provider_name: String,
    /// MCP server instance for writing data.
    mcp_server: Option<Arc<LibraMcpServer>>,
    /// MCP resource IDs for tracking the workflow
    mcp_ids: McpIds,
}

impl<M: CompletionModel + Clone + 'static> App<M> {
    /// Create a new App instance.
    pub fn new(
        tui: Tui,
        model: M,
        registry: Arc<ToolRegistry>,
        config: ToolLoopConfig,
        app_config: AppConfig,
    ) -> Self {
        let (app_event_tx, app_event_rx) = mpsc::unbounded_channel();
        struct TuiRetryObserver {
            tx: UnboundedSender<AppEvent>,
        }

        impl CompletionRetryObserver for TuiRetryObserver {
            fn on_retry(&self, event: &CompletionRetryEvent) {
                let _ = self.tx.send(AppEvent::AgentEvent(AgentEvent::Retrying {
                    attempt: event.next_attempt,
                    total_attempts: event.total_attempts,
                    delay_ms: event.delay.as_millis().min(u128::from(u64::MAX)) as u64,
                    error: event.error.clone(),
                }));
            }
        }
        let history = app_config.session.to_history();
        let default_allowed_tools = registry
            .tool_specs()
            .into_iter()
            .map(|s| s.function.name)
            .filter(|name| name != "submit_intent_draft")
            .collect();
        let mut widget = ChatWidget::new();
        widget
            .bottom_pane
            .set_cwd(registry.working_dir().to_path_buf());
        Self {
            tui,
            widget,
            model: RetryingCompletionModel::new(model)
                .with_policy(CompletionRetryPolicy::default())
                .with_observer(Arc::new(TuiRetryObserver {
                    tx: app_event_tx.clone(),
                })),
            registry,
            config,
            default_allowed_tools,
            history,
            app_event_rx,
            app_event_tx,
            should_exit: false,
            exit_info: None,
            last_draw_time: Instant::now(),
            agent_task: None,
            scheduled_draw_task: None,
            welcome_message: app_config.welcome_message,
            command_dispatcher: app_config.command_dispatcher,
            agent_router: app_config.agent_router,
            session: app_config.session,
            session_store: app_config.session_store,
            user_input_rx: app_config.user_input_rx,
            pending_user_input: None,
            pending_post_plan: None,
            model_name: app_config.model_name,
            provider_name: app_config.provider_name,
            mcp_server: app_config.mcp_server,
            mcp_ids: McpIds::default(),
        }
    }

    /// Run the main event loop.
    pub async fn run(&mut self) -> anyhow::Result<AppExitInfo> {
        // Enter alternate screen
        self.tui.enter_alt_screen()?;
        let run_result = self.run_in_alt_screen().await;
        let leave_result = self.tui.leave_alt_screen();

        // Save session on exit (best-effort)
        if self.session.message_count() > 0
            && let Err(e) = self.session_store.save(&self.session)
        {
            tracing::warn!("Failed to save session: {}", e);
        }

        match (run_result, leave_result) {
            (Ok(exit_info), Ok(())) => Ok(exit_info),
            (Err(run_err), Ok(())) => Err(run_err),
            (Ok(_), Err(leave_err)) => Err(leave_err.into()),
            (Err(run_err), Err(_leave_err)) => Err(run_err),
        }
    }

    async fn run_in_alt_screen(&mut self) -> anyhow::Result<AppExitInfo> {
        self.tui.clear()?;

        // Set up slash-command autocomplete hints (built-in + YAML-defined).
        let mut hints: Vec<(String, String)> = super::slash_command::BuiltinCommand::all_hints();
        hints.extend(
            self.command_dispatcher
                .commands()
                .iter()
                .map(|c| (c.name.clone(), c.description.clone())),
        );
        self.widget.bottom_pane.set_command_hints(hints);

        // Welcome message
        self.widget.add_cell(Box::new(AssistantHistoryCell::new(
            self.welcome_message.clone(),
        )));

        // Initial draw - ensure UI is rendered immediately
        self.draw()?;

        // Get the event stream
        let mut event_stream = self.tui.event_stream();
        let mut animation_tick = interval(Duration::from_millis(120));

        loop {
            // Check if we should exit
            if self.should_exit {
                break;
            }

            tokio::select! {
                // Handle terminal events
                Some(event) = event_stream.next() => {
                    self.handle_tui_event(event).await?;
                }

                // Handle app events
                Some(event) = self.app_event_rx.recv() => {
                    if self.handle_app_event(event).await? {
                        break;
                    }
                }

                // Handle user-input requests from the tool handler
                Some(request) = self.user_input_rx.recv() => {
                    self.handle_user_input_request(request);
                }

                // Drive subtle status/tool animations while the agent is active.
                _ = animation_tick.tick() => {
                    if matches!(
                        self.widget.bottom_pane.status,
                        AgentStatus::Thinking | AgentStatus::Retrying | AgentStatus::ExecutingTool
                    ) {
                        self.schedule_draw();
                    }
                }
            }
        }

        // Create decision via MCP when exiting
        if let Some(ref mcp_server) = self.mcp_server {
            let mcp_ids_clone = self.mcp_ids.clone();
            let mcp_server_clone = mcp_server.clone();

            tokio::spawn(async move {
                // Create decision
                let decision_params = CreateDecisionParams {
                    run_id: mcp_ids_clone
                        .run_id
                        .clone()
                        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                    decision_type: "complete".to_string(),
                    chosen_patchset_id: None,
                    result_commit_sha: None,
                    rationale: Some("Session completed successfully".to_string()),
                    checkpoint_id: None,
                    tags: None,
                    external_ids: None,
                    actor_kind: Some("system".to_string()),
                    actor_id: Some("libra-code".to_string()),
                };

                // Resolve actor
                let actor = match mcp_server_clone.resolve_actor_from_params(
                    decision_params.actor_kind.as_deref(),
                    decision_params.actor_id.as_deref(),
                ) {
                    Ok(actor) => actor,
                    Err(e) => {
                        cli_error!(e, "error: failed to resolve actor for decision");
                        return;
                    }
                };

                // Call MCP interface to create decision
                match mcp_server_clone
                    .create_decision_impl(decision_params, actor)
                    .await
                {
                    Ok(result) => {
                        if !result.is_error.unwrap_or(false) {
                            println!("Decision created successfully");
                        } else {
                            render_mcp_error("failed to create decision", result.content);
                        }
                    }
                    Err(e) => {
                        cli_error!(e, "error: failed to create decision");
                    }
                }
            });
        }

        Ok(self.exit_info.clone().unwrap_or(AppExitInfo {
            reason: ExitReason::UserRequested,
        }))
    }

    /// Handle a terminal event.
    async fn handle_tui_event(&mut self, event: TuiEvent) -> anyhow::Result<()> {
        match event {
            TuiEvent::Key(key) => {
                if key.kind == crossterm::event::KeyEventKind::Press {
                    self.handle_key_event(key).await?;
                }
            }
            TuiEvent::Paste(text) => {
                for c in text.chars() {
                    self.widget.bottom_pane.insert_char(c);
                }
                self.widget.bottom_pane.sync_command_popup();
                self.schedule_draw();
            }
            TuiEvent::Mouse(mouse) => {
                self.handle_mouse_event(mouse);
                self.schedule_draw();
            }
            TuiEvent::Resize => {
                self.schedule_draw();
            }
            TuiEvent::Draw => {
                self.scheduled_draw_task = None;
                self.last_draw_time = Instant::now();
                self.draw()?;
            }
        }
        Ok(())
    }

    /// Handle a key press event.
    async fn handle_key_event(&mut self, key: crossterm::event::KeyEvent) -> anyhow::Result<()> {
        // Check for Ctrl+C first (always handled)
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.cancel_pending_user_input();
            self.dismiss_post_plan_dialog();
            self.interrupt_agent_task();
            self.exit_info = Some(AppExitInfo {
                reason: ExitReason::UserRequested,
            });
            self.should_exit = true;
            return Ok(());
        }

        // Handle input based on agent status
        match self.widget.bottom_pane.status {
            AgentStatus::Idle => match key.code {
                // ── Command popup intercepts (when visible) ──────────
                KeyCode::Tab if self.widget.bottom_pane.is_command_popup_visible() => {
                    self.widget.bottom_pane.complete_command();
                    self.widget.bottom_pane.sync_command_popup();
                    self.schedule_draw();
                }
                KeyCode::Up if self.widget.bottom_pane.is_command_popup_visible() => {
                    self.widget.bottom_pane.command_popup_up();
                    self.schedule_draw();
                }
                KeyCode::Down if self.widget.bottom_pane.is_command_popup_visible() => {
                    self.widget.bottom_pane.command_popup_down();
                    self.schedule_draw();
                }
                KeyCode::Esc if self.widget.bottom_pane.is_command_popup_visible() => {
                    self.widget.bottom_pane.dismiss_command_popup();
                    self.schedule_draw();
                }
                // ── Normal idle handlers ─────────────────────────────
                KeyCode::Enter => {
                    if !self.widget.bottom_pane.is_empty() {
                        let text = self.widget.bottom_pane.take_input();
                        self.submit_message(text).await;
                    }
                }
                // Clear screen (Ctrl+K) - must come before generic Char handler
                KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.widget.clear();
                    self.widget.bottom_pane.clear();
                    self.widget.bottom_pane.sync_command_popup();
                    self.schedule_draw();
                }
                // Scroll to top (Home)
                KeyCode::Home => {
                    self.widget.scroll_to_top();
                    self.schedule_draw();
                }
                // Scroll to bottom (End)
                KeyCode::End => {
                    self.widget.scroll_to_bottom();
                    self.schedule_draw();
                }
                // Scroll
                KeyCode::PageUp => {
                    self.widget.scroll_up_lines(10);
                    self.schedule_draw();
                }
                KeyCode::PageDown => {
                    self.widget.scroll_down_lines(10);
                    self.schedule_draw();
                }
                KeyCode::Up => {
                    self.widget.scroll_up_lines(1);
                    self.schedule_draw();
                }
                KeyCode::Down => {
                    self.widget.scroll_down_lines(1);
                    self.schedule_draw();
                }
                // Cursor to beginning of input (Ctrl+A)
                KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.widget.bottom_pane.cursor_home();
                    self.schedule_draw();
                }
                // Cursor to end of input (Ctrl+E)
                KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.widget.bottom_pane.cursor_end();
                    self.schedule_draw();
                }
                KeyCode::Char(c) => {
                    self.widget.bottom_pane.insert_char(c);
                    self.widget.bottom_pane.sync_command_popup();
                    self.schedule_draw();
                }
                KeyCode::Backspace => {
                    self.widget.bottom_pane.backspace();
                    self.widget.bottom_pane.sync_command_popup();
                    self.schedule_draw();
                }
                KeyCode::Delete => {
                    self.widget.bottom_pane.delete();
                    self.widget.bottom_pane.sync_command_popup();
                    self.schedule_draw();
                }
                KeyCode::Left => {
                    self.widget.bottom_pane.cursor_left();
                    self.schedule_draw();
                }
                KeyCode::Right => {
                    self.widget.bottom_pane.cursor_right();
                    self.schedule_draw();
                }
                _ => {}
            },
            AgentStatus::AwaitingUserInput => {
                self.handle_user_input_key(key);
            }
            AgentStatus::AwaitingPostPlanChoice => match key.code {
                KeyCode::Up => {
                    if let Some(ref mut p) = self.pending_post_plan {
                        p.selected = p.selected.saturating_sub(1);
                        self.widget.bottom_pane.post_plan_selected = p.selected;
                    }
                    self.schedule_draw();
                }
                KeyCode::Down => {
                    if let Some(ref mut p) = self.pending_post_plan {
                        p.selected = (p.selected + 1).min(2);
                        self.widget.bottom_pane.post_plan_selected = p.selected;
                    }
                    self.schedule_draw();
                }
                KeyCode::Enter => {
                    self.handle_post_plan_choice().await;
                }
                KeyCode::Esc => {
                    self.dismiss_post_plan_dialog();
                }
                _ => {}
            },
            AgentStatus::Thinking | AgentStatus::Retrying | AgentStatus::ExecutingTool => {
                // During processing, only handle Escape for interrupt
                if key.code == KeyCode::Esc {
                    self.interrupt_agent_task();
                    self.widget.bottom_pane.set_status(AgentStatus::Idle);
                    self.complete_streaming_assistant_cell("Interrupted.".to_string());
                    self.complete_running_tool_cells_with_interrupt();
                    self.schedule_draw();
                }
            }
        }

        Ok(())
    }

    /// Handle keyboard input while in the AwaitingUserInput state.
    fn handle_user_input_key(&mut self, key: crossterm::event::KeyEvent) {
        let is_freeform = self.pending_user_input.as_ref().is_some_and(|p| {
            let q = &p.request.questions[p.current_question];
            q.options.as_ref().is_none_or(|o| o.is_empty())
        });

        // If notes are focused, route most keys to the input field.
        let notes_focused = self
            .pending_user_input
            .as_ref()
            .is_some_and(|p| p.notes_focused);

        match key.code {
            // Tab: toggle between options and notes
            KeyCode::Tab if !is_freeform => {
                if let Some(ref mut pending) = self.pending_user_input {
                    pending.notes_focused = !pending.notes_focused;
                }
                self.sync_user_input_to_pane();
                self.schedule_draw();
            }
            // Navigate options with Up/Down (only when options focused)
            KeyCode::Up if !notes_focused => {
                if let Some(ref mut pending) = self.pending_user_input
                    && pending.selected_option > 0
                {
                    pending.selected_option -= 1;
                }
                self.sync_user_input_to_pane();
                self.schedule_draw();
            }
            KeyCode::Down if !notes_focused => {
                if let Some(ref mut pending) = self.pending_user_input {
                    let q = &pending.request.questions[pending.current_question];
                    let base = q.options.as_ref().map_or(0, |o| o.len());
                    let max = if q.is_other {
                        base
                    } else if base > 0 {
                        base - 1
                    } else {
                        0
                    };
                    if pending.selected_option < max {
                        pending.selected_option += 1;
                    }
                }
                self.sync_user_input_to_pane();
                self.schedule_draw();
            }
            // Quick-select by number key (1-9), only when options focused
            KeyCode::Char(c @ '1'..='9') if !notes_focused && !is_freeform => {
                let idx = (c as usize) - ('1' as usize);
                if let Some(ref mut pending) = self.pending_user_input {
                    let q = &pending.request.questions[pending.current_question];
                    let base = q.options.as_ref().map_or(0, |o| o.len());
                    let max = if q.is_other {
                        base
                    } else if base > 0 {
                        base - 1
                    } else {
                        0
                    };
                    if idx <= max {
                        pending.selected_option = idx;
                    }
                }
                self.sync_user_input_to_pane();
                self.schedule_draw();
            }
            // Type text (notes when notes_focused, or freeform input)
            KeyCode::Char(c) if notes_focused || is_freeform => {
                if notes_focused {
                    if let Some(ref mut pending) = self.pending_user_input {
                        pending.notes_text.push(c);
                    }
                    self.sync_user_input_to_pane();
                } else {
                    self.widget.bottom_pane.insert_char(c);
                }
                self.schedule_draw();
            }
            KeyCode::Backspace if notes_focused => {
                if let Some(ref mut pending) = self.pending_user_input {
                    pending.notes_text.pop();
                }
                self.sync_user_input_to_pane();
                self.schedule_draw();
            }
            KeyCode::Backspace if is_freeform => {
                self.widget.bottom_pane.backspace();
                self.schedule_draw();
            }
            // Submit answer
            KeyCode::Enter => {
                self.submit_user_input_answer();
            }
            // Cancel
            KeyCode::Esc => {
                self.cancel_pending_user_input();
                self.widget.bottom_pane.set_status(AgentStatus::Thinking);
                self.schedule_draw();
            }
            _ => {}
        }
    }

    /// Submit the currently selected answer for the active question.
    fn submit_user_input_answer(&mut self) {
        let answer = if let Some(ref pending) = self.pending_user_input {
            let q = &pending.request.questions[pending.current_question];
            let options = q.options.as_deref().unwrap_or_default();
            let mut answer_list: Vec<String> = Vec::new();

            if options.is_empty() {
                // Freeform question: take text from input field
                let text = self.widget.bottom_pane.take_input();
                if !text.is_empty() {
                    answer_list.push(text);
                }
            } else if pending.selected_option < options.len() {
                // Predefined option selected
                answer_list.push(options[pending.selected_option].label.clone());
            } else if q.is_other && pending.selected_option == options.len() {
                // "None of the above"
                answer_list.push("None of the above".to_string());
            }

            // Append notes if present
            if !pending.notes_text.is_empty() {
                answer_list.push(format!("user_note: {}", pending.notes_text));
            }

            UserInputAnswer {
                answers: answer_list,
            }
        } else {
            return;
        };

        let pending = self.pending_user_input.as_mut().unwrap();
        let question_id = pending.request.questions[pending.current_question]
            .id
            .clone();
        pending.answers.insert(question_id, answer);
        pending.current_question += 1;
        pending.selected_option = 0;
        pending.notes_focused = false;
        pending.notes_text.clear();
        self.widget.bottom_pane.clear();

        // Check if all questions have been answered.
        let done = {
            let p = self.pending_user_input.as_ref().unwrap();
            p.current_question >= p.request.questions.len()
        };

        if done {
            // Send the response back to the handler.
            let pending = self.pending_user_input.take().unwrap();
            let response = UserInputResponse {
                answers: pending.answers,
            };
            let _ = pending.request.response_tx.send(response);
            self.widget
                .bottom_pane
                .set_status(AgentStatus::ExecutingTool);
            self.widget.bottom_pane.set_user_input_questions(None);
        } else {
            self.sync_user_input_to_pane();
        }
        self.schedule_draw();
    }

    /// Cancel the pending user-input interaction (drops the oneshot sender).
    fn cancel_pending_user_input(&mut self) {
        if let Some(pending) = self.pending_user_input.take() {
            // Dropping response_tx signals cancellation to the handler.
            drop(pending.request.response_tx);
            self.widget.bottom_pane.set_user_input_questions(None);
        }
    }

    /// Sync the pending user-input state to the bottom pane for rendering.
    fn sync_user_input_to_pane(&mut self) {
        if let Some(ref pending) = self.pending_user_input {
            self.widget.bottom_pane.user_input_current_question = pending.current_question;
            self.widget.bottom_pane.user_input_selected_option = pending.selected_option;
            self.widget.bottom_pane.user_input_notes_focused = pending.notes_focused;
            self.widget.bottom_pane.user_input_notes_text = pending.notes_text.clone();
        }
    }

    /// Handle a user-input request from the tool handler.
    fn handle_user_input_request(&mut self, request: UserInputRequest) {
        // Store question info for the bottom pane to render.
        self.widget
            .bottom_pane
            .set_user_input_questions(Some(&request.questions));

        self.pending_user_input = Some(PendingUserInput {
            request,
            current_question: 0,
            answers: HashMap::new(),
            selected_option: 0,
            notes_focused: false,
            notes_text: String::new(),
        });
        self.widget
            .bottom_pane
            .set_status(AgentStatus::AwaitingUserInput);
        self.widget.bottom_pane.clear();
        self.sync_user_input_to_pane();
        self.schedule_draw();
    }

    fn handle_mouse_event(&mut self, mouse: crossterm::event::MouseEvent) {
        use crossterm::event::{MouseButton, MouseEventKind};

        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.widget.scroll_up_lines(3);
            }
            MouseEventKind::ScrollDown => {
                self.widget.scroll_down_lines(3);
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let x = mouse.column;
                let y = mouse.row;
                self.widget.bottom_pane.focused = self.widget.is_in_input_area(x, y);
            }
            _ => {}
        }
    }

    /// Handle an app event.
    async fn handle_app_event(&mut self, event: AppEvent) -> anyhow::Result<bool> {
        match event {
            AppEvent::Exit(mode) => match mode {
                ExitMode::Immediate => {
                    self.should_exit = true;
                    return Ok(true);
                }
                ExitMode::ShutdownFirst => {
                    self.should_exit = true;
                    return Ok(true);
                }
            },
            AppEvent::SubmitUserMessage {
                text,
                allowed_tools,
            } => {
                // Track in session
                self.session.add_user_message(&text);

                // Add user cell immediately
                self.widget
                    .add_cell(Box::new(UserHistoryCell::new(text.clone())));

                // Add streaming assistant placeholder (kept as the last cell).
                self.widget
                    .add_cell(Box::new(AssistantHistoryCell::streaming()));
                self.widget.bottom_pane.set_status(AgentStatus::Thinking);
                self.schedule_draw();

                // Create run and context snapshot via MCP if available
                if let Some(ref mcp_server) = self.mcp_server {
                    let text_clone = text.clone();
                    let mcp_ids_clone = self.mcp_ids.clone();
                    let mcp_server_clone = mcp_server.clone();

                    tokio::spawn(async move {
                        // Create run
                        let run_params = CreateRunParams {
                            task_id: mcp_ids_clone
                                .task_id
                                .clone()
                                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                            base_commit_sha: "0000000000000000000000000000000000000000".to_string(),
                            status: Some("created".to_string()),
                            context_snapshot_id: None,
                            error: None,
                            agent_instances: None,
                            metrics_json: None,
                            orchestrator_version: None,
                            tags: None,
                            external_ids: None,
                            actor_kind: Some("human".to_string()),
                            actor_id: Some("user".to_string()),
                        };

                        // Resolve actor
                        let actor = match mcp_server_clone.resolve_actor_from_params(
                            run_params.actor_kind.as_deref(),
                            run_params.actor_id.as_deref(),
                        ) {
                            Ok(actor) => actor,
                            Err(e) => {
                                cli_error!(e, "error: failed to resolve actor for run");
                                return;
                            }
                        };

                        // Call MCP interface to create run
                        match mcp_server_clone.create_run_impl(run_params, actor).await {
                            Ok(result) => {
                                if result.is_error.unwrap_or(false) {
                                    render_mcp_error("failed to create run", result.content);
                                }
                            }
                            Err(e) => {
                                cli_error!(e, "error: failed to create run");
                            }
                        }

                        // Create context snapshot
                        let snapshot_params = CreateContextSnapshotParams {
                            base_commit_sha: "0000000000000000000000000000000000000000".to_string(),
                            selection_strategy: "heuristic".to_string(),
                            items: None,
                            summary: Some(format!("Context for: {}", text_clone)),
                            tags: None,
                            external_ids: None,
                            actor_kind: Some("system".to_string()),
                            actor_id: Some("libra-code".to_string()),
                        };

                        // Resolve actor for snapshot
                        let snapshot_actor = match mcp_server_clone.resolve_actor_from_params(
                            snapshot_params.actor_kind.as_deref(),
                            snapshot_params.actor_id.as_deref(),
                        ) {
                            Ok(actor) => actor,
                            Err(e) => {
                                cli_error!(e, "error: failed to resolve actor for snapshot");
                                return;
                            }
                        };

                        // Call MCP interface to create context snapshot
                        match mcp_server_clone
                            .create_context_snapshot_impl(snapshot_params, snapshot_actor)
                            .await
                        {
                            Ok(result) => {
                                if result.is_error.unwrap_or(false) {
                                    render_mcp_error(
                                        "failed to create context snapshot",
                                        result.content,
                                    );
                                }
                            }
                            Err(e) => {
                                cli_error!(e, "error: failed to create context snapshot");
                            }
                        }
                    });
                }

                // Prepare components for background task
                let model = self.model.clone();
                let registry = self.registry.clone();
                let mut config = self.config.clone();
                config.allowed_tools =
                    Some(allowed_tools.unwrap_or_else(|| self.default_allowed_tools.clone()));
                let history = self.history.clone();
                let tx = self.app_event_tx.clone();
                let user_text = text;
                let mcp_server = self.mcp_server.clone();

                // Execute agent call in background task
                let handle = tokio::spawn(async move {
                    struct UiObserver {
                        tx: UnboundedSender<AppEvent>,
                        mcp_server: Option<Arc<LibraMcpServer>>,
                    }

                    impl crate::internal::ai::agent::ToolLoopObserver for UiObserver {
                        fn on_assistant_step_text(&mut self, text: &str) {
                            let cell = Box::new(AssistantHistoryCell::new(text.to_string()));
                            let _ = self.tx.send(AppEvent::InsertHistoryCell(cell));
                        }

                        fn on_tool_call_begin(
                            &mut self,
                            call_id: &str,
                            tool_name: &str,
                            arguments: &serde_json::Value,
                        ) {
                            let _ = self.tx.send(AppEvent::ToolCallBegin {
                                call_id: call_id.to_string(),
                                tool_name: tool_name.to_string(),
                                arguments: arguments.clone(),
                            });

                            // Record tool invocation via MCP if available
                            if let Some(ref mcp_server) = self.mcp_server {
                                let _call_id = call_id.to_string();
                                let tool_name = tool_name.to_string();
                                let arguments = arguments.clone();
                                let mcp_server_clone = mcp_server.clone();

                                tokio::spawn(async move {
                                    // Create tool invocation
                                    let invocation_params = CreateToolInvocationParams {
                                        run_id: uuid::Uuid::new_v4().to_string(),
                                        tool_name: tool_name.clone(),
                                        args_json: Some(arguments.to_string()),
                                        status: Some("ok".to_string()),
                                        io_footprint: None,
                                        result_summary: None,
                                        artifacts: None,
                                        tags: None,
                                        external_ids: None,
                                        actor_kind: Some("agent".to_string()),
                                        actor_id: Some("libra-agent".to_string()),
                                    };

                                    // Resolve actor
                                    let actor = match mcp_server_clone.resolve_actor_from_params(
                                        invocation_params.actor_kind.as_deref(),
                                        invocation_params.actor_id.as_deref(),
                                    ) {
                                        Ok(actor) => actor,
                                        Err(e) => {
                                            cli_error!(
                                                e,
                                                "error: failed to resolve actor for tool invocation"
                                            );
                                            return;
                                        }
                                    };

                                    // Call MCP interface to create tool invocation
                                    match mcp_server_clone
                                        .create_tool_invocation_impl(invocation_params, actor)
                                        .await
                                    {
                                        Ok(result) => {
                                            if result.is_error.unwrap_or(false) {
                                                render_mcp_error(
                                                    "failed to record tool invocation",
                                                    result.content,
                                                );
                                            }
                                        }
                                        Err(e) => {
                                            cli_error!(
                                                e,
                                                "error: failed to record tool invocation"
                                            );
                                        }
                                    }
                                });
                            }
                        }

                        fn on_tool_call_end(
                            &mut self,
                            call_id: &str,
                            tool_name: &str,
                            result: &Result<ToolOutput, String>,
                        ) {
                            let _ = self.tx.send(AppEvent::ToolCallEnd {
                                call_id: call_id.to_string(),
                                tool_name: tool_name.to_string(),
                                result: result.clone(),
                            });
                        }
                    }

                    let mut observer = UiObserver { tx, mcp_server };
                    let result = run_tool_loop_with_history_and_observer(
                        &model,
                        history,
                        user_text,
                        &registry,
                        config,
                        &mut observer,
                    )
                    .await;

                    match result {
                        Ok(turn) => {
                            let _ = observer.tx.send(AppEvent::AgentEvent(
                                AgentEvent::ResponseComplete {
                                    text: turn.final_text,
                                    new_history: turn.history,
                                },
                            ));
                        }
                        Err(e) => {
                            let _ = observer.tx.send(AppEvent::AgentEvent(AgentEvent::Error {
                                message: e.to_string(),
                            }));
                        }
                    }
                });

                self.agent_task = Some(handle);
            }
            AppEvent::AgentEvent(agent_event) => {
                match agent_event {
                    AgentEvent::TextDelta { delta } => {
                        // Find and update the streaming assistant cell
                        for cell in self.widget.cells.iter_mut().rev() {
                            if let Some(assistant_cell) =
                                cell.as_any_mut().downcast_mut::<AssistantHistoryCell>()
                                && assistant_cell.is_streaming
                            {
                                assistant_cell.append(&delta);
                                break;
                            }
                        }
                        self.schedule_draw();
                    }
                    AgentEvent::ResponseComplete { text, new_history } => {
                        self.agent_task = None;
                        self.history = new_history;

                        // Track in session
                        self.session.add_assistant_message(&text);

                        // Find and complete the streaming assistant cell
                        // (may not be the last cell if tool calls were made)
                        for cell in self.widget.cells.iter_mut().rev() {
                            if let Some(assistant_cell) =
                                cell.as_any_mut().downcast_mut::<AssistantHistoryCell>()
                                && assistant_cell.is_streaming
                            {
                                assistant_cell.content = text;
                                assistant_cell.complete();
                                break;
                            }
                        }
                        self.widget.bottom_pane.set_status(AgentStatus::Idle);
                        self.schedule_draw();
                    }
                    AgentEvent::Error { message } => {
                        self.agent_task = None;

                        self.complete_streaming_assistant_cell(format!("Error: {}", message));
                        self.widget.bottom_pane.set_status(AgentStatus::Idle);
                        self.schedule_draw();
                    }
                    AgentEvent::Retrying {
                        attempt,
                        total_attempts,
                        delay_ms,
                        error,
                    } => {
                        let reason = summarize_retry_error(&error);
                        self.widget.bottom_pane.set_retry_notice(format!(
                            "● Retrying request {attempt}/{total_attempts} in {:.1}s ({reason})",
                            delay_ms as f64 / 1000.0
                        ));
                        self.schedule_draw();
                    }
                }
            }
            AppEvent::PlanWorkflowComplete {
                text,
                new_history,
                intent_id,
                plan_id,
                spec_json,
            } => {
                self.agent_task = None;
                self.history = new_history;
                self.session.add_assistant_message(&text);
                self.session.metadata.insert(
                    LATEST_INTENTSPEC_JSON.to_string(),
                    serde_json::Value::String(spec_json.clone()),
                );
                if let Some(id) = intent_id {
                    self.session.metadata.insert(
                        LATEST_INTENTSPEC_INTENT_ID.to_string(),
                        serde_json::Value::String(id),
                    );
                } else {
                    self.session.metadata.remove(LATEST_INTENTSPEC_INTENT_ID);
                }
                if let Some(id) = plan_id.clone() {
                    self.session.metadata.insert(
                        LATEST_EXECUTION_PLAN_ID.to_string(),
                        serde_json::Value::String(id.clone()),
                    );
                    self.mcp_ids.plan_id = Some(id);
                } else {
                    self.session.metadata.remove(LATEST_EXECUTION_PLAN_ID);
                    self.mcp_ids.plan_id = None;
                }

                self.complete_streaming_assistant_cell(text);

                // Show post-plan dialog instead of returning to Idle
                self.pending_post_plan = Some(PendingPostPlan {
                    spec_json,
                    selected: 0,
                });
                self.widget.bottom_pane.reset_post_plan_selection();
                self.widget
                    .bottom_pane
                    .set_status(AgentStatus::AwaitingPostPlanChoice);
                self.schedule_draw();
            }
            AppEvent::InsertHistoryCell(cell) => {
                self.insert_before_streaming_assistant(cell);
                self.schedule_draw();
            }
            AppEvent::ToolCallBegin {
                call_id,
                tool_name,
                arguments,
            } => {
                if tool_name == "update_plan" {
                    // Parse the plan arguments and render a specialised cell.
                    let (explanation, steps) =
                        if let Ok(args) = serde_json::from_value::<UpdatePlanArgs>(arguments) {
                            (args.explanation, args.plan)
                        } else {
                            (None, Vec::new())
                        };
                    let cell = Box::new(PlanUpdateHistoryCell::new(call_id, explanation, steps));
                    self.insert_before_streaming_assistant(cell);
                } else {
                    let cell = Box::new(ToolCallHistoryCell::new(call_id, tool_name, arguments));
                    self.insert_before_streaming_assistant(cell);
                }
                self.widget
                    .bottom_pane
                    .set_status(AgentStatus::ExecutingTool);
                self.schedule_draw();
            }
            AppEvent::ToolCallEnd {
                call_id,
                tool_name,
                result,
            } => {
                // For successful apply_patch, insert a visual diff cell.
                if tool_name == "apply_patch"
                    && let Ok(ref output) = result
                {
                    self.try_insert_diff_cell(output);
                }

                // Try to find a PlanUpdateHistoryCell first, then fall back to ToolCallHistoryCell.
                let mut found = false;
                for cell in self.widget.cells.iter_mut().rev() {
                    if let Some(plan_cell) =
                        cell.as_any_mut().downcast_mut::<PlanUpdateHistoryCell>()
                        && plan_cell.call_id == call_id
                        && plan_cell.is_running
                    {
                        plan_cell.complete();
                        found = true;
                        break;
                    }
                }
                if !found {
                    for cell in self.widget.cells.iter_mut().rev() {
                        if let Some(tool_cell) =
                            cell.as_any_mut().downcast_mut::<ToolCallHistoryCell>()
                            && tool_cell.call_id == call_id
                            && tool_cell.is_running
                        {
                            tool_cell.complete(result);
                            break;
                        }
                    }
                }
                self.widget.bottom_pane.set_status(AgentStatus::Thinking);
                self.schedule_draw();
            }
            AppEvent::AgentStatusUpdate { status } => {
                self.widget.bottom_pane.set_status(status);
                self.schedule_draw();
            }
            AppEvent::RequestUserInput { request } => {
                self.handle_user_input_request(request);
            }
            AppEvent::ExecuteWorkflowComplete { text, new_history } => {
                self.agent_task = None;
                self.history = new_history;
                self.session.add_assistant_message(&text);
                self.complete_streaming_assistant_cell(text);
                self.widget.bottom_pane.set_status(AgentStatus::Idle);
                self.schedule_draw();
            }
        }

        Ok(false)
    }

    /// Submit a user message, expanding slash commands and applying agent context.
    async fn submit_message(&mut self, text: String) {
        // 1. Check for built-in TUI commands first.
        if let Some((cmd, args)) = super::slash_command::parse_builtin(&text) {
            self.handle_builtin_command(cmd, args).await;
            return;
        }

        // 2. Try YAML-defined slash commands (sent to model).
        let (effective_text, agent_name) =
            if let Some(result) = self.command_dispatcher.dispatch(&text) {
                (result.prompt, result.agent)
            } else {
                (text.clone(), None)
            };

        // Agent is only selected via slash command, not auto-detected
        let agent = agent_name
            .as_deref()
            .and_then(|name| self.agent_router.get(name));

        let agent_prompt = agent.map(|a| a.system_prompt.clone());
        let allowed_tools = agent.map(|a| a.tools.clone()).filter(|t| !t.is_empty());

        // If an agent was selected, prepend its system prompt to the user message
        let final_text = if let Some(prompt) = agent_prompt {
            format!("{prompt}\n\n---\n\n{effective_text}")
        } else {
            effective_text
        };

        let _ = self.app_event_tx.send(AppEvent::SubmitUserMessage {
            text: final_text,
            allowed_tools,
        });
    }

    /// Handle a built-in TUI command (does not send to model).
    async fn handle_builtin_command(
        &mut self,
        cmd: super::slash_command::BuiltinCommand,
        args: &str,
    ) {
        use super::slash_command::BuiltinCommand;
        match cmd {
            BuiltinCommand::Help => {
                let mut lines = String::from("Available commands:\n");
                // Built-in commands
                for b in BuiltinCommand::all() {
                    lines.push_str(&format!("  /{:<14} {}\n", b.name(), b.description()));
                }
                // YAML-defined commands
                for c in self.command_dispatcher.commands() {
                    lines.push_str(&format!("  /{:<14} {}\n", c.name, c.description));
                }
                self.widget
                    .add_cell(Box::new(AssistantHistoryCell::new(lines)));
            }
            BuiltinCommand::Clear => {
                self.widget.clear();
                self.history.clear();
                self.session = SessionState::new(&self.registry.working_dir().to_string_lossy());
            }
            BuiltinCommand::Model => {
                let info = format!(
                    "Provider: {}\nModel: {}",
                    self.provider_name, self.model_name,
                );
                self.widget
                    .add_cell(Box::new(AssistantHistoryCell::new(info)));
            }
            BuiltinCommand::Status => {
                let status = format!(
                    "Status: {:?}\nHistory: {} messages\nWorking dir: {}",
                    self.widget.bottom_pane.status,
                    self.history.len(),
                    self.registry.working_dir().display(),
                );
                self.widget
                    .add_cell(Box::new(AssistantHistoryCell::new(status)));
            }
            BuiltinCommand::Plan => {
                self.start_plan_workflow(args).await;
            }
            BuiltinCommand::Intent => {
                self.handle_intent_command(args).await;
            }
            BuiltinCommand::Quit => {
                self.should_exit = true;
                self.exit_info = Some(AppExitInfo {
                    reason: ExitReason::UserRequested,
                });
            }
        }
    }

    // ── Post-plan dialog ────────────────────────────────────────────

    async fn handle_post_plan_choice(&mut self) {
        let pending = match self.pending_post_plan.take() {
            Some(p) => p,
            None => return,
        };

        match pending.selected {
            0 => {
                // Execute: validate spec and show placeholder
                self.start_execute_workflow(&pending.spec_json).await;
            }
            _ => {
                // Modify (1) or Cancel (2+)
                if pending.selected == 1 {
                    let msg = format!(
                        "Here is the current IntentSpec. Please tell me what you'd like to change:\n\n```json\n{}\n```",
                        pending.spec_json
                    );
                    self.widget
                        .add_cell(Box::new(AssistantHistoryCell::new(msg.clone())));
                    self.history.push(Message::assistant(msg.clone()));
                    self.session.add_assistant_message(&msg);
                }
                self.widget.bottom_pane.set_status(AgentStatus::Idle);
            }
        }
        self.schedule_draw();
    }

    fn dismiss_post_plan_dialog(&mut self) {
        self.pending_post_plan = None;
        self.widget.bottom_pane.set_status(AgentStatus::Idle);
        self.schedule_draw();
    }

    async fn start_execute_workflow(&mut self, spec_json: &str) {
        use crate::internal::ai::{
            intentspec::types::IntentSpec,
            orchestrator::{
                Orchestrator,
                types::{OrchestratorConfig, OrchestratorObserver, PersistedExecution, TaskSpec},
            },
        };

        let spec: IntentSpec = match serde_json::from_str(spec_json) {
            Ok(s) => s,
            Err(e) => {
                self.widget
                    .add_cell(Box::new(AssistantHistoryCell::new(format!(
                        "Failed to parse IntentSpec: {e}"
                    ))));
                self.widget.bottom_pane.set_status(AgentStatus::Idle);
                self.schedule_draw();
                return;
            }
        };

        self.widget
            .add_cell(Box::new(AssistantHistoryCell::streaming()));
        self.widget.bottom_pane.set_status(AgentStatus::Thinking);
        self.schedule_draw();

        let model = self.model.clone();
        let registry = self.registry.clone();
        let working_dir = self.registry.working_dir().to_path_buf();
        let coder_preamble = self
            .agent_router
            .get("coder")
            .map(|a| a.system_prompt.clone());
        let reviewer_preamble = self
            .agent_router
            .get("reviewer")
            .map(|a| a.system_prompt.clone());
        let mcp_server = self.mcp_server.clone();
        let tx = self.app_event_tx.clone();
        let history = self.history.clone();

        let handle = tokio::spawn(async move {
            struct UiOrchestratorObserver {
                tx: UnboundedSender<AppEvent>,
            }

            impl UiOrchestratorObserver {
                fn send_note(&self, text: String) {
                    let _ = self.tx.send(AppEvent::InsertHistoryCell(Box::new(
                        AssistantHistoryCell::new(text),
                    )));
                }

                fn scoped_call_id(task: &TaskSpec, call_id: &str) -> String {
                    format!("{}:{call_id}", task.id)
                }
            }

            impl OrchestratorObserver for UiOrchestratorObserver {
                fn on_plan_compiled(&self, plan: &ExecutionPlanSpec) {
                    self.send_note(format!(
                        "Compiled execution plan rev {}: {}  \nTasks: {} | Parallel groups: {} | Checkpoints: {}",
                        plan.revision,
                        plan.summary,
                        plan.tasks.len(),
                        plan.parallel_groups.len(),
                        plan.checkpoints.len()
                    ));
                }

                fn on_task_started(&self, task: &TaskSpec) {
                    let kind = match task.gate_stage {
                        Some(ref stage) => format!("gate:{stage:?}"),
                        None => "implementation".to_string(),
                    };
                    let _ = self.tx.send(AppEvent::AgentStatusUpdate {
                        status: AgentStatus::Thinking,
                    });
                    self.send_note(format!("Starting [{kind}] {}", task.title));
                }

                fn on_task_completed(
                    &self,
                    task: &TaskSpec,
                    result: &crate::internal::ai::orchestrator::types::TaskResult,
                ) {
                    self.send_note(format!(
                        "Finished {}  \nStatus: {:?} | Retries: {} | Tools: {} | Policy violations: {}",
                        task.title,
                        result.status,
                        result.retry_count,
                        result.tool_calls.len(),
                        result.policy_violations.len()
                    ));
                }

                fn on_task_assistant_message(&self, task: &TaskSpec, text: &str) {
                    let text = text.trim();
                    if text.is_empty() {
                        return;
                    }
                    self.send_note(format!("{}  \n{}", task.title, text));
                }

                fn on_tool_call_begin(
                    &self,
                    task: &TaskSpec,
                    call_id: &str,
                    tool_name: &str,
                    arguments: &serde_json::Value,
                ) {
                    let _ = self.tx.send(AppEvent::ToolCallBegin {
                        call_id: Self::scoped_call_id(task, call_id),
                        tool_name: tool_name.to_string(),
                        arguments: arguments.clone(),
                    });
                }

                fn on_tool_call_end(
                    &self,
                    task: &TaskSpec,
                    call_id: &str,
                    tool_name: &str,
                    result: &Result<ToolOutput, String>,
                ) {
                    let _ = self.tx.send(AppEvent::ToolCallEnd {
                        call_id: Self::scoped_call_id(task, call_id),
                        tool_name: tool_name.to_string(),
                        result: result.clone(),
                    });
                }

                fn on_reviewer_started(&self, task: &TaskSpec) {
                    self.send_note(format!("Reviewer pass started for {}", task.title));
                }

                fn on_reviewer_completed(
                    &self,
                    task: &TaskSpec,
                    review: Option<&crate::internal::ai::orchestrator::types::ReviewOutcome>,
                ) {
                    let summary = review
                        .map(|r| {
                            format!(
                                "{} | approved: {}",
                                r.summary,
                                if r.approved { "yes" } else { "no" }
                            )
                        })
                        .unwrap_or_else(|| "no reviewer output".to_string());
                    self.send_note(format!(
                        "Reviewer pass finished for {}  \n{}",
                        task.title, summary
                    ));
                }

                fn on_graph_progress(&self, completed: usize, total: usize) {
                    if completed == total && total > 0 {
                        self.send_note(format!("dagrs graph finished: {completed}/{total} tasks reached terminal state"));
                    }
                }

                fn on_graph_checkpoint_saved(
                    &self,
                    checkpoint_id: &str,
                    pc: usize,
                    completed_nodes: usize,
                ) {
                    self.send_note(format!(
                        "dagrs checkpoint saved  \nid: {} | pc: {} | completed: {}",
                        checkpoint_id, pc, completed_nodes
                    ));
                }

                fn on_graph_checkpoint_restored(&self, checkpoint_id: &str, pc: usize) {
                    self.send_note(format!(
                        "dagrs checkpoint restored  \nid: {} | pc: {}",
                        checkpoint_id, pc
                    ));
                }

                fn on_replan(
                    &self,
                    current_revision: u32,
                    next_revision: u32,
                    reason: &str,
                    diff_summary: &str,
                ) {
                    self.send_note(format!(
                        "Replan triggered  \nRevision {} -> {}  \nReason: {}  \n{}",
                        current_revision, next_revision, reason, diff_summary
                    ));
                }

                fn on_persistence_complete(&self, execution: &PersistedExecution) {
                    self.send_note(format!(
                        "Persisted execution  \nRun: {} | Plans: {} | Checkpoints: {}",
                        execution.run_id,
                        execution.plan_ids.len(),
                        execution.checkpoints.len()
                    ));
                }
            }

            let observer: Arc<dyn OrchestratorObserver> =
                Arc::new(UiOrchestratorObserver { tx: tx.clone() });
            let config = OrchestratorConfig {
                working_dir,
                base_commit: None,
                dagrs_resume_checkpoint_id: None,
                coder_preamble,
                reviewer_preamble,
                mcp_server,
                observer: Some(observer),
            };
            let orchestrator = Orchestrator::new(model, registry, config);

            let result = orchestrator.run(spec).await;

            let summary = match &result {
                Ok(r) => format_orchestrator_result(r),
                Err(e) => format!("Orchestrator failed: {e}"),
            };

            let mut new_history = history;
            new_history.push(Message::assistant(summary.clone()));

            let _ = tx.send(AppEvent::ExecuteWorkflowComplete {
                text: summary,
                new_history,
            });
        });

        self.agent_task = Some(handle);
    }

    async fn start_plan_workflow(&mut self, request: &str) {
        let request = request.trim();
        if request.is_empty() {
            self.widget.add_cell(Box::new(AssistantHistoryCell::new(
                "Usage: /plan <your requirement>".to_string(),
            )));
            self.schedule_draw();
            return;
        }

        let user_text = format!("/plan {request}");
        self.session.add_user_message(&user_text);
        self.widget
            .add_cell(Box::new(UserHistoryCell::new(user_text.clone())));
        self.widget
            .add_cell(Box::new(AssistantHistoryCell::streaming()));
        self.widget.bottom_pane.set_status(AgentStatus::Thinking);
        self.schedule_draw();

        let prompt = if let Some(agent) = self.agent_router.get("planner") {
            format!(
                "{}\n\n---\n\n{}",
                agent.system_prompt,
                build_plan_prompt(request)
            )
        } else {
            build_plan_prompt(request)
        };
        let model = self.model.clone();
        let registry = self.registry.clone();
        let mut config = self.config.clone();
        config.allowed_tools = Some(vec![
            "read_file".to_string(),
            "list_dir".to_string(),
            "grep_files".to_string(),
            "request_user_input".to_string(),
            "submit_intent_draft".to_string(),
        ]);
        let history = self.history.clone();
        let tx = self.app_event_tx.clone();
        let mcp_server = self.mcp_server.clone();
        let working_dir = self.registry.working_dir().to_path_buf();

        let handle = tokio::spawn(async move {
            struct PlanObserver {
                tx: UnboundedSender<AppEvent>,
                draft: Option<IntentDraft>,
                risk_prompted: bool,
                selected_risk: Option<RiskLevel>,
            }

            impl PlanObserver {
                fn new(tx: UnboundedSender<AppEvent>) -> Self {
                    Self {
                        tx,
                        draft: None,
                        risk_prompted: false,
                        selected_risk: None,
                    }
                }
            }

            impl crate::internal::ai::agent::ToolLoopObserver for PlanObserver {
                fn on_tool_call_begin(
                    &mut self,
                    call_id: &str,
                    tool_name: &str,
                    arguments: &serde_json::Value,
                ) {
                    let _ = self.tx.send(AppEvent::ToolCallBegin {
                        call_id: call_id.to_string(),
                        tool_name: tool_name.to_string(),
                        arguments: arguments.clone(),
                    });

                    if tool_name == "request_user_input"
                        && let Ok(req) =
                            parse_value_or_json_string::<RequestUserInputArgs>(arguments)
                        && req
                            .questions
                            .iter()
                            .any(|q| q.id.trim().eq_ignore_ascii_case("risk_profile"))
                    {
                        self.risk_prompted = true;
                    }

                    if tool_name == "submit_intent_draft"
                        && let Ok(args) =
                            parse_value_or_json_string::<SubmitIntentDraftArgs>(arguments)
                    {
                        self.draft = Some(args.draft);
                    }
                }

                fn on_tool_call_end(
                    &mut self,
                    call_id: &str,
                    tool_name: &str,
                    result: &Result<ToolOutput, String>,
                ) {
                    let _ = self.tx.send(AppEvent::ToolCallEnd {
                        call_id: call_id.to_string(),
                        tool_name: tool_name.to_string(),
                        result: result.clone(),
                    });

                    if tool_name == "request_user_input"
                        && let Ok(output) = result
                        && let Some(content) = output.as_text()
                        && let Ok(resp) = serde_json::from_str::<UserInputResponse>(content)
                        && let Some(level) = extract_risk_level_from_response(&resp)
                    {
                        self.selected_risk = Some(level);
                    }
                }
            }

            let mut observer = PlanObserver::new(tx.clone());
            let fallback_history = history.clone();
            let run_result = run_tool_loop_with_history_and_observer(
                &model,
                history,
                prompt,
                &registry,
                config.clone(),
                &mut observer,
            )
            .await;

            let turn = match run_result {
                Ok(turn) => Some(turn),
                Err(e) => {
                    if observer.risk_prompted
                        && observer.selected_risk.is_some()
                        && observer.draft.is_some()
                    {
                        let _ = tx.send(AppEvent::InsertHistoryCell(Box::new(
                            AssistantHistoryCell::new(format!(
                                "Planner response failed after draft submission. Continuing with the submitted draft.\nReason: {}",
                                e
                            )),
                        )));
                        None
                    } else {
                        let _ = tx.send(AppEvent::AgentEvent(AgentEvent::Error {
                            message: e.to_string(),
                        }));
                        return;
                    }
                }
            };

            if !observer.risk_prompted {
                let _ = tx.send(AppEvent::AgentEvent(AgentEvent::Error {
                    message: "Plan failed: planner did not ask for risk profile.".to_string(),
                }));
                return;
            }

            let risk_level = match observer.selected_risk.clone() {
                Some(level) => level,
                None => {
                    let _ = tx.send(AppEvent::AgentEvent(AgentEvent::Error {
                        message: "Plan failed: risk profile was not selected.".to_string(),
                    }));
                    return;
                }
            };

            let draft = match observer.draft.take() {
                Some(d) => d,
                None => {
                    let _ = tx.send(AppEvent::AgentEvent(AgentEvent::Error {
                        message: "Plan failed: no intent draft was submitted.".to_string(),
                    }));
                    return;
                }
            };

            let mut spec = resolve_intentspec(
                draft,
                risk_level,
                ResolveContext {
                    working_dir: working_dir.display().to_string(),
                    base_ref: current_head_sha(&working_dir),
                    created_by_id: "tui-user".to_string(),
                },
            );

            let mut issues = validate_intentspec(&spec);
            for _ in 0..MAX_INTENTSPEC_REPAIR_ATTEMPTS {
                if issues.is_empty() {
                    break;
                }
                repair_intentspec(&mut spec, &issues);
                issues = validate_intentspec(&spec);
            }

            if !issues.is_empty() {
                let report = issues
                    .iter()
                    .map(|i| format!("- {}: {}", i.path, i.message))
                    .collect::<Vec<_>>()
                    .join("\n");
                let _ = tx.send(AppEvent::AgentEvent(AgentEvent::Error {
                    message: format!(
                        "Plan failed after automatic repair.\nValidation issues:\n{}",
                        report
                    ),
                }));
                return;
            }

            let canonical =
                match crate::internal::ai::intentspec::canonical::to_canonical_json(&spec) {
                    Ok(v) => v,
                    Err(e) => {
                        let _ = tx.send(AppEvent::AgentEvent(AgentEvent::Error {
                            message: format!("Plan failed: cannot serialize IntentSpec: {e}"),
                        }));
                        return;
                    }
                };

            let mut persistence_warning = None;
            let intent_id = if let Some(ref mcp_server) = mcp_server {
                let params = CreateIntentParams {
                    content: "IntentSpec generated by planner".to_string(),
                    structured_content: Some(canonical),
                    parent_id: None,
                    status: Some("active".to_string()),
                    task_id: None,
                    commit_sha: None,
                    actor_kind: Some("system".to_string()),
                    actor_id: Some("libra-plan".to_string()),
                };
                let actor_kind = params.actor_kind.clone();
                let actor_id = params.actor_id.clone();
                match mcp_server
                    .resolve_actor_from_params(actor_kind.as_deref(), actor_id.as_deref())
                {
                    Ok(actor) => match mcp_server.create_intent_impl(params, actor).await {
                        Ok(call_result) => parse_created_id(&call_result),
                        Err(e) => {
                            persistence_warning =
                                Some(format!("failed to persist intent into MCP: {e:?}"));
                            None
                        }
                    },
                    Err(e) => {
                        persistence_warning =
                            Some(format!("failed to resolve MCP actor for intent: {e:?}"));
                        None
                    }
                }
            } else {
                persistence_warning =
                    Some("MCP server unavailable; intent not persisted.".to_string());
                None
            };

            let pretty_json =
                serde_json::to_string_pretty(&spec).unwrap_or_else(|_| "{}".to_string());
            let execution_plan = match compile_execution_plan_spec(&spec) {
                Ok(plan) => plan,
                Err(e) => {
                    let _ = tx.send(AppEvent::AgentEvent(AgentEvent::Error {
                        message: format!("Plan failed: cannot compile execution plan: {e}"),
                    }));
                    return;
                }
            };

            let mut summary = render_summary(&spec, intent_id.as_deref());
            let mut plan_warning = None;
            let plan_id = if let Some(ref mcp_server) = mcp_server {
                match persist_execution_plan(&execution_plan, mcp_server).await {
                    Ok(id) => Some(id),
                    Err(e) => {
                        plan_warning = Some(format!("failed to persist execution plan: {e}"));
                        None
                    }
                }
            } else {
                plan_warning =
                    Some("MCP server unavailable; execution plan not persisted.".to_string());
                None
            };

            if let Some(warn) = persistence_warning {
                summary.push_str(&format!("\nWarning: {warn}"));
            }
            if let Some(warn) = plan_warning {
                summary.push_str(&format!("\nWarning: {warn}"));
            }

            let plan_summary = render_execution_plan_summary(&execution_plan, plan_id.as_deref());
            summary.push_str("\n\n");
            summary.push_str(&plan_summary);

            let mut new_history = turn.map(|turn| turn.history).unwrap_or(fallback_history);
            new_history.push(Message::assistant(summary.clone()));

            let _ = tx.send(AppEvent::PlanWorkflowComplete {
                text: summary,
                new_history,
                intent_id,
                plan_id,
                spec_json: pretty_json,
            });
        });

        self.agent_task = Some(handle);
    }

    async fn handle_intent_command(&mut self, args: &str) {
        match args.trim() {
            "show" => {
                let rendered = self.load_latest_intentspec_json().await.unwrap_or_else(|| {
                    "No IntentSpec found. Run `/plan <requirement>` first.".to_string()
                });
                self.widget
                    .add_cell(Box::new(AssistantHistoryCell::new(rendered)));
                self.schedule_draw();
            }
            _ => {
                self.widget.add_cell(Box::new(AssistantHistoryCell::new(
                    "Usage: /intent show".to_string(),
                )));
                self.schedule_draw();
            }
        }
    }

    async fn load_latest_intentspec_json(&self) -> Option<String> {
        if let (Some(id), Some(mcp)) = (
            self.session
                .metadata
                .get(LATEST_INTENTSPEC_INTENT_ID)
                .and_then(|v| v.as_str()),
            self.mcp_server.clone(),
        ) && let Some(spec) = fetch_intentspec_from_object_id(&mcp, id).await
        {
            return serde_json::to_string_pretty(&spec).ok();
        }

        if let Some(json_text) = self
            .session
            .metadata
            .get(LATEST_INTENTSPEC_JSON)
            .and_then(|v| v.as_str())
        {
            return Some(json_text.to_string());
        }

        let mcp = self.mcp_server.clone()?;
        let ids = list_intent_object_ids(&mcp).await;
        for id in ids.into_iter().rev() {
            if let Some(spec) = fetch_intentspec_from_object_id(&mcp, &id).await {
                return serde_json::to_string_pretty(&spec).ok();
            }
        }
        None
    }

    fn interrupt_agent_task(&mut self) {
        if let Some(handle) = self.agent_task.take() {
            handle.abort();
        }
    }

    fn insert_before_streaming_assistant(
        &mut self,
        cell: Box<dyn super::history_cell::HistoryCell>,
    ) {
        if let Some(index) = self.widget.cells.iter().rposition(|c| {
            c.as_any()
                .downcast_ref::<AssistantHistoryCell>()
                .is_some_and(|a| a.is_streaming)
        }) {
            self.widget.insert_cell(index, cell);
        } else {
            self.widget.add_cell(cell);
        }
    }

    /// Extract diff metadata from a successful `apply_patch` result and insert
    /// a [`DiffHistoryCell`] for visual diff rendering.
    fn try_insert_diff_cell(&mut self, result: &ToolOutput) {
        let ToolOutput::Function {
            metadata: Some(meta),
            ..
        } = result
        else {
            return;
        };
        let Some(diffs) = meta.get("diffs").and_then(|v| v.as_array()) else {
            return;
        };

        let cwd = self.registry.working_dir().to_path_buf();
        let mut changes: HashMap<PathBuf, FileChange> = HashMap::new();

        for entry in diffs {
            let Some(path_str) = entry.get("path").and_then(|v| v.as_str()) else {
                continue;
            };
            let Some(diff_type) = entry.get("type").and_then(|v| v.as_str()) else {
                continue;
            };
            let diff_text = entry
                .get("diff")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let path = PathBuf::from(path_str);

            let change = match diff_type {
                "add" => FileChange::Add {
                    unified_diff: diff_text.to_string(),
                },
                "delete" => FileChange::Delete {
                    unified_diff: diff_text.to_string(),
                },
                _ => FileChange::Update {
                    unified_diff: diff_text.to_string(),
                    move_path: None,
                },
            };
            changes.insert(path, change);
        }

        if !changes.is_empty() {
            let cell = Box::new(DiffHistoryCell::new(changes, cwd));
            self.insert_before_streaming_assistant(cell);
        }
    }

    fn complete_streaming_assistant_cell(&mut self, content: String) {
        for cell in self.widget.cells.iter_mut().rev() {
            if let Some(assistant_cell) = cell.as_any_mut().downcast_mut::<AssistantHistoryCell>()
                && assistant_cell.is_streaming
            {
                assistant_cell.content = content;
                assistant_cell.complete();
                return;
            }
        }
        self.widget
            .add_cell(Box::new(AssistantHistoryCell::new(content)));
    }

    fn complete_running_tool_cells_with_interrupt(&mut self) {
        for cell in self.widget.cells.iter_mut() {
            if let Some(tool_cell) = cell.as_any_mut().downcast_mut::<ToolCallHistoryCell>()
                && tool_cell.is_running
            {
                tool_cell.complete(Err("Interrupted".to_string()));
            }
        }
    }

    /// Schedule a frame draw with frame rate limiting.
    fn schedule_draw(&mut self) {
        if self
            .scheduled_draw_task
            .as_ref()
            .is_some_and(tokio::task::JoinHandle::is_finished)
        {
            self.scheduled_draw_task = None;
        }

        let now = Instant::now();
        let elapsed = now.duration_since(self.last_draw_time);
        if elapsed >= TARGET_FRAME_INTERVAL {
            if let Some(task) = self.scheduled_draw_task.take() {
                task.abort();
            }
            let _ = self.tui.frame_requester().send(());
            return;
        }

        if self.scheduled_draw_task.is_some() {
            return;
        }

        let delay = TARGET_FRAME_INTERVAL - elapsed;
        let draw_tx = self.tui.frame_requester();
        self.scheduled_draw_task = Some(tokio::spawn(async move {
            sleep(delay).await;
            let _ = draw_tx.send(());
        }));
    }

    /// Draw the current frame.
    fn draw(&mut self) -> anyhow::Result<()> {
        self.tui.draw(|frame| {
            let area = frame.area();
            let cursor_pos = self.widget.render(area, frame.buffer_mut());
            if let Some(pos) = cursor_pos {
                frame.set_cursor_position(pos);
            }
        })?;
        Ok(())
    }
}

fn format_orchestrator_result(
    result: &crate::internal::ai::orchestrator::types::OrchestratorResult,
) -> String {
    use crate::internal::ai::orchestrator::types::{DecisionOutcome, TaskNodeStatus};

    let mut lines = Vec::new();
    let decision_label = match result.decision {
        DecisionOutcome::Commit => "Commit",
        DecisionOutcome::HumanReviewRequired => "Human Review Required",
        DecisionOutcome::Abandon => "Abandon",
    };
    lines.push(format!("## Orchestrator Result: {decision_label}"));
    lines.push(String::new());
    lines.push(format!(
        "Plan: {} | Parallel groups: {} | Replans: {}",
        result.execution_plan_spec.summary,
        result.execution_plan_spec.parallel_groups.len(),
        result.replan_count
    ));
    if let Some(persistence) = &result.persistence {
        lines.push(format!(
            "Persisted Run: {} | Decision: {} | Checkpoints: {} | Tasks with MCP artifacts: {}",
            persistence.run_id,
            persistence.decision_id.as_deref().unwrap_or("none"),
            persistence.checkpoints.len(),
            persistence.tasks.len()
        ));
        if let Some(snapshot_id) = &persistence.initial_snapshot_id {
            lines.push(format!("Initial Snapshot: {snapshot_id}"));
        }
    }
    lines.push(String::new());

    let task_titles: HashMap<_, _> = result
        .execution_plan_spec
        .tasks
        .iter()
        .map(|task| (task.id, task.title.as_str()))
        .collect();

    for tr in &result.task_results {
        let status_icon = match tr.status {
            TaskNodeStatus::Completed => "✓",
            TaskNodeStatus::Failed => "✗",
            _ => "○",
        };
        let task_label = task_titles.get(&tr.task_id).copied().unwrap_or("unknown");
        lines.push(format!(
            "{} Task {} ({}) - {:?} (retries: {}, tools: {}, policy violations: {})",
            status_icon,
            tr.task_id,
            task_label,
            tr.status,
            tr.retry_count,
            tr.tool_calls.len(),
            tr.policy_violations.len()
        ));
    }

    if !result.system_report.overall_passed {
        lines.push(String::new());
        lines.push("System verification: FAILED".to_string());
    }
    if !result.system_report.review_passed {
        lines.push(format!(
            "Review findings: {}",
            result.system_report.review_findings.join(" | ")
        ));
    }
    if !result.system_report.artifacts_complete {
        lines.push(format!(
            "Missing artifacts: {}",
            result.system_report.missing_artifacts.join(", ")
        ));
    }

    if let Some(persistence) = &result.persistence {
        lines.push(String::new());
        lines.push("MCP Objects".to_string());
        if let Some(provenance_id) = &persistence.provenance_id {
            lines.push(format!("Provenance: {provenance_id}"));
        }
        for checkpoint in &persistence.checkpoints {
            lines.push(format!(
                "- Checkpoint rev {} | snapshot: {} | decision: {} | dagrs: {} | reason: {}",
                checkpoint.revision,
                checkpoint.snapshot_id.as_deref().unwrap_or("none"),
                checkpoint.decision_id.as_deref().unwrap_or("none"),
                checkpoint.dagrs_checkpoint_id.as_deref().unwrap_or("none"),
                checkpoint.reason
            ));
        }
        for task in &persistence.tasks {
            lines.push(format!(
                "- Task {} | tools: {} | patchset: {} | evidence: {}",
                task.task_id,
                task.tool_invocation_ids.len(),
                task.patchset_id.as_deref().unwrap_or("none"),
                task.evidence_ids.len()
            ));
        }
    }

    lines.join("\n")
}

fn summarize_retry_error(error: &str) -> String {
    let lowered = error.to_ascii_lowercase();
    if lowered.contains("timeout") {
        "timeout".to_string()
    } else if lowered.contains("429") || lowered.contains("rate limit") {
        "rate limited".to_string()
    } else if lowered.contains("503") || lowered.contains("overloaded") {
        "upstream overloaded".to_string()
    } else if lowered.contains("connection") || lowered.contains("sending request") {
        "network issue".to_string()
    } else {
        "transient error".to_string()
    }
}

fn render_execution_plan_summary(plan: &ExecutionPlanSpec, plan_id: Option<&str>) -> String {
    let mut lines = Vec::new();
    lines.push("## Execution Plan".to_string());
    if let Some(id) = plan_id {
        lines.push(format!("Plan ID: {id}"));
    }
    lines.push(plan.summary.clone());
    lines.push(format!(
        "Parallel groups: {} | Checkpoints: {}",
        plan.parallel_groups.len(),
        plan.checkpoints.len()
    ));
    lines.push(String::new());

    for task in &plan.tasks {
        let kind = match &task.gate_stage {
            Some(stage) => format!("gate:{stage:?}"),
            None => "implementation".to_string(),
        };
        lines.push(format!("- [{}] {}", kind.to_lowercase(), task.title));
        if !task.contract.touch_files.is_empty() {
            lines.push(format!("  files: {}", task.contract.touch_files.join(", ")));
        }
        if !task.dependencies.is_empty() {
            lines.push(format!("  depends on: {}", task.dependencies.len()));
        }
        if !task.checks.is_empty() {
            lines.push(format!("  checks: {}", task.checks.len()));
        }
    }

    lines.join("\n")
}

async fn persist_execution_plan(
    plan: &ExecutionPlanSpec,
    mcp_server: &Arc<LibraMcpServer>,
) -> Result<String, String> {
    let steps = plan
        .tasks
        .iter()
        .map(|task| {
            let checks = serde_json::to_value(&task.checks)
                .map_err(|e| format!("failed to encode plan checks: {e}"))?;
            Ok(crate::internal::ai::mcp::resource::PlanStepParams {
                description: task.title.clone(),
                inputs: Some(serde_json::json!({
                    "objective": task.objective,
                    "kind": format!("{:?}", task.kind),
                    "gateStage": task.gate_stage.as_ref().map(|stage| format!("{:?}", stage)),
                    "scopeIn": task.scope_in,
                    "scopeOut": task.scope_out,
                    "touchFiles": task.contract.touch_files,
                    "touchSymbols": task.contract.touch_symbols,
                    "touchApis": task.contract.touch_apis,
                    "constraints": task.constraints,
                })),
                outputs: Some(serde_json::json!({
                    "expectedOutputs": task.contract.expected_outputs,
                    "acceptanceCriteria": task.acceptance_criteria,
                })),
                checks: Some(checks),
                status: Some("pending".to_string()),
                owner_role: task.owner_role.clone(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    let params = CreatePlanParams {
        plan_version: Some(plan.revision),
        pipeline_id: None,
        fwindow: None,
        steps: Some(steps),
        tags: None,
        external_ids: None,
        actor_kind: Some("system".to_string()),
        actor_id: Some("libra-plan".to_string()),
    };

    let actor = mcp_server
        .resolve_actor_from_params(params.actor_kind.as_deref(), params.actor_id.as_deref())
        .map_err(|e| format!("failed to resolve plan actor: {e:?}"))?;
    let result = mcp_server
        .create_plan_impl(params, actor)
        .await
        .map_err(|e| format!("MCP create_plan failed: {e:?}"))?;

    if result.is_error.unwrap_or(false) {
        return Err(
            summarize_mcp_content(&result.content).unwrap_or_else(|| "unknown MCP error".into())
        );
    }

    parse_created_id(&result).ok_or_else(|| "failed to parse plan id from MCP result".to_string())
}

fn build_plan_prompt(request: &str) -> String {
    format!(
        "You are running /plan mode.\n\
First, you MUST call request_user_input with exactly one question id=risk_profile, header=Risk, and options Low/Medium/High.\n\
After receiving user choice, analyze the repository and then call submit_intent_draft exactly once.\n\
If required information is missing, call request_user_input again for focused follow-up questions.\n\
Do not output a plain-text plan; finalize by submitting the draft tool call.\n\n\
User request:\n{request}"
    )
}

fn parse_value_or_json_string<T: serde::de::DeserializeOwned>(
    value: &serde_json::Value,
) -> Result<T, serde_json::Error> {
    match value {
        serde_json::Value::String(raw) => serde_json::from_str(raw),
        _ => serde_json::from_value(value.clone()),
    }
}

fn extract_risk_level_from_response(resp: &UserInputResponse) -> Option<RiskLevel> {
    for answer in resp.answers.values() {
        for item in &answer.answers {
            let normalized = item.to_lowercase();
            if normalized.contains("low") {
                return Some(RiskLevel::Low);
            }
            if normalized.contains("medium") {
                return Some(RiskLevel::Medium);
            }
            if normalized.contains("high") {
                return Some(RiskLevel::High);
            }
        }
    }
    None
}

fn current_head_sha(working_dir: &std::path::Path) -> String {
    let output = std::process::Command::new("git")
        .arg("rev-parse")
        .arg("HEAD")
        .current_dir(working_dir)
        .output();
    match output {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if text.is_empty() {
                "HEAD".to_string()
            } else {
                text
            }
        }
        _ => "HEAD".to_string(),
    }
}

fn parse_created_id(result: &rmcp::model::CallToolResult) -> Option<String> {
    for content in &result.content {
        if let Some(text) = content.as_text().map(|t| t.text.as_str())
            && let Some(id) = text.split("ID:").nth(1)
        {
            let id = id.trim();
            if !id.is_empty() {
                return Some(id.to_string());
            }
        }
    }
    None
}

async fn list_intent_object_ids(mcp: &Arc<LibraMcpServer>) -> Vec<String> {
    let mut ids = Vec::new();
    let resources = match mcp.read_resource_impl("libra://objects/intent").await {
        Ok(v) => v,
        Err(_) => return ids,
    };
    let Some(content) = resources.first() else {
        return ids;
    };
    let Some(text) = resource_text(content) else {
        return ids;
    };
    for line in text.lines() {
        if let Some(id) = line.split_whitespace().next() {
            ids.push(id.to_string());
        }
    }
    ids
}

async fn fetch_intentspec_from_object_id(
    mcp: &Arc<LibraMcpServer>,
    object_id: &str,
) -> Option<crate::internal::ai::intentspec::IntentSpec> {
    let uri = format!("libra://object/{object_id}");
    let resources = mcp.read_resource_impl(&uri).await.ok()?;
    let content = resources.first()?;
    let text = resource_text(content)?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let intent_content = extract_content_field(&value)?;
    serde_json::from_str::<crate::internal::ai::intentspec::IntentSpec>(&intent_content).ok()
}

fn resource_text(content: &rmcp::model::ResourceContents) -> Option<String> {
    let value = serde_json::to_value(content).ok()?;
    value
        .get("text")
        .and_then(|v| v.as_str())
        .map(|v| v.to_string())
}

fn extract_content_field(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(v) = map.get("content").and_then(|v| v.as_str()) {
                return Some(v.to_string());
            }
            map.values().find_map(extract_content_field)
        }
        serde_json::Value::Array(items) => items.iter().find_map(extract_content_field),
        _ => None,
    }
}
