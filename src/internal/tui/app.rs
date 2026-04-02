//! Main application structure and event loop.
//!
//! The `App` struct manages the TUI state and coordinates between
//! user input, agent execution, and UI rendering.

use std::{
    collections::HashMap,
    future::Future,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use crossterm::event::{KeyCode, KeyModifiers};
use tokio::{
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    task::JoinHandle,
    time::{interval, sleep, timeout},
};
use tokio_stream::StreamExt;

use super::{
    app_event::{AgentEvent, AgentStatus, AppEvent, TaskMuxLogKind, TurnId},
    chatwidget::ChatWidget,
    diff::FileChange,
    history_cell::{
        AssistantHistoryCell, DiffHistoryCell, HistoryCell, OrchestratorResultHistoryCell,
        PlanSummaryHistoryCell, PlanUpdateHistoryCell, ToolCallHistoryCell, UserHistoryCell,
    },
    terminal::{TARGET_FRAME_INTERVAL, Tui, TuiEvent},
    welcome_shader::{self, WelcomeView},
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
                CreatePlanParams, CreateRunParams, CreateTaskParams, CreateToolInvocationParams,
            },
            server::LibraMcpServer,
        },
        orchestrator::{planner::compile_execution_plan_spec, types::ExecutionPlanSpec},
        sandbox::{ExecApprovalRequest, ReviewDecision},
        session::{SessionState, SessionStore},
        tools::{
            ToolOutput, ToolRegistry,
            context::{
                RequestUserInputArgs, SubmitIntentDraftArgs, UpdatePlanArgs, UserInputAnswer,
                UserInputRequest, UserInputResponse,
            },
        },
        workflow_objects::{build_git_plan, parse_object_id},
    },
};

#[derive(Debug, Clone, Default)]
struct McpWriteTracker {
    tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl McpWriteTracker {
    fn spawn<F>(&self, fut: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let handle = tokio::spawn(async move {
            if timeout(MCP_WRITE_TIMEOUT, fut).await.is_err() {
                tracing::warn!("MCP background write timed out");
            }
        });
        match self.tasks.lock() {
            Ok(mut tasks) => {
                tasks.retain(|task| !task.is_finished());
                tasks.push(handle);
            }
            Err(_) => handle.abort(),
        }
    }

    async fn drain(&self) {
        loop {
            let pending = match self.tasks.lock() {
                Ok(mut tasks) => std::mem::take(&mut *tasks),
                Err(_) => return,
            };
            if pending.is_empty() {
                return;
            }
            for handle in pending {
                let _ = handle.await;
            }
        }
    }
}

const LATEST_INTENTSPEC_INTENT_ID: &str = "latest_intentspec_intent_id";
const LATEST_EXECUTION_PLAN_ID: &str = "latest_execution_plan_id";
const LATEST_INTENTSPEC_JSON: &str = "latest_intentspec_json";
const LATEST_INTENTSPEC_WORKSPACE_KEY: &str = "latest_intentspec_workspace_key";
const LATEST_INTENTSPEC_BASE_REF: &str = "latest_intentspec_base_ref";
const LATEST_INTENTSPEC_BRANCH_LABEL: &str = "latest_intentspec_branch_label";
const MAX_INTENTSPEC_REPAIR_ATTEMPTS: usize = 2;
const MCP_WRITE_TIMEOUT: Duration = Duration::from_secs(8);
const MCP_TURN_TRACKING_TIMEOUT: Duration = Duration::from_secs(3);

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

/// Pending sandbox approval state.
struct PendingExecApproval {
    request: ExecApprovalRequest,
    selected: usize, // 0=Approve, 1=Approve Session, 2=Deny, 3=Abort
}

/// Configuration for creating an App.
pub struct AppConfig {
    pub welcome_message: String,
    pub command_dispatcher: CommandDispatcher,
    pub agent_router: AgentProfileRouter,
    pub session: SessionState,
    pub session_store: SessionStore,
    pub user_input_rx: UnboundedReceiver<UserInputRequest>,
    pub exec_approval_rx: UnboundedReceiver<ExecApprovalRequest>,
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
    /// Whether the animated welcome screen is shown.
    welcome_active: bool,
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
    /// Receiver for exec-approval requests from sandbox-governed handlers.
    exec_approval_rx: UnboundedReceiver<ExecApprovalRequest>,
    /// Currently pending user-input interaction, if any.
    pending_user_input: Option<PendingUserInput>,
    /// Currently pending exec approval interaction, if any.
    pending_exec_approval: Option<PendingExecApproval>,
    /// Post-plan dialog state (present when user is choosing Execute/Modify/Cancel).
    pending_post_plan: Option<PendingPostPlan>,
    /// Base IntentSpec JSON for the next spec-revision request, if the user chose Modify.
    pending_plan_revision: Option<String>,
    /// Display name of the active model.
    model_name: String,
    /// Provider identifier.
    provider_name: String,
    /// MCP server instance for writing data.
    mcp_server: Option<Arc<LibraMcpServer>>,
    /// Latest execution plan ID for attaching new turn runs.
    mcp_plan_id: Option<String>,
    /// Active turn run ID for appending decisions and tool invocations.
    mcp_run_id: Option<String>,
    /// Pending detached MCP write operations that must finish before shutdown.
    mcp_write_tracker: McpWriteTracker,
    /// Current active async turn. Events from stale turns are ignored.
    active_turn_id: Option<TurnId>,
    /// Monotonic turn counter.
    next_turn_id: TurnId,
    /// Shared view of active turn for global retry observer callbacks.
    active_turn_signal: Arc<AtomicU64>,
    /// Number of tool calls currently running in UI.
    running_tool_calls: usize,
    /// Shared run-id slot for the active turn, backfilled by MCP tracking.
    active_turn_run_id: Option<Arc<Mutex<Option<String>>>>,
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
        let active_turn_signal = Arc::new(AtomicU64::new(0));
        struct TuiRetryObserver {
            tx: UnboundedSender<AppEvent>,
            active_turn_signal: Arc<AtomicU64>,
        }

        impl CompletionRetryObserver for TuiRetryObserver {
            fn on_retry(&self, event: &CompletionRetryEvent) {
                let turn_id = self.active_turn_signal.load(Ordering::Relaxed);
                if turn_id == 0 {
                    return;
                }
                let _ = self.tx.send(AppEvent::AgentEvent {
                    turn_id,
                    event: AgentEvent::Retrying {
                        attempt: event.next_attempt,
                        total_attempts: event.total_attempts,
                        delay_ms: event.delay.as_millis().min(u128::from(u64::MAX)) as u64,
                        error: event.error.clone(),
                    },
                });
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
        widget
            .bottom_pane
            .set_git_branch(current_git_branch_label(registry.working_dir()));
        let mcp_plan_id = app_config
            .session
            .metadata
            .get(LATEST_EXECUTION_PLAN_ID)
            .and_then(|value| value.as_str())
            .map(ToString::to_string);
        Self {
            tui,
            widget,
            model: RetryingCompletionModel::new(model)
                .with_policy(CompletionRetryPolicy::default())
                .with_observer(Arc::new(TuiRetryObserver {
                    tx: app_event_tx.clone(),
                    active_turn_signal: active_turn_signal.clone(),
                })),
            registry,
            config,
            default_allowed_tools,
            history,
            app_event_rx,
            app_event_tx,
            exit_info: None,
            last_draw_time: Instant::now(),
            agent_task: None,
            scheduled_draw_task: None,
            welcome_message: app_config.welcome_message,
            welcome_active: true,
            command_dispatcher: app_config.command_dispatcher,
            agent_router: app_config.agent_router,
            session: app_config.session,
            session_store: app_config.session_store,
            user_input_rx: app_config.user_input_rx,
            exec_approval_rx: app_config.exec_approval_rx,
            pending_user_input: None,
            pending_exec_approval: None,
            pending_post_plan: None,
            pending_plan_revision: None,
            model_name: app_config.model_name,
            provider_name: app_config.provider_name,
            mcp_server: app_config.mcp_server,
            mcp_plan_id,
            mcp_run_id: None,
            mcp_write_tracker: McpWriteTracker::default(),
            active_turn_id: None,
            next_turn_id: 1,
            active_turn_signal,
            running_tool_calls: 0,
            active_turn_run_id: None,
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

        // Initial draw - ensure UI is rendered immediately
        self.draw()?;

        // Get the event stream
        let mut event_stream = self.tui.event_stream();
        let mut animation_tick = interval(Duration::from_millis(120));

        loop {
            // Check if we should exit
            if self.exit_info.is_some() {
                break;
            }

            tokio::select! {
                // Handle terminal events
                Some(event) = event_stream.next() => {
                    self.handle_tui_event(event).await?;
                }

                // Handle app events
                Some(event) = self.app_event_rx.recv() => {
                    self.handle_app_event(event).await?;
                }

                // Handle user-input requests from the tool handler
                Some(request) = self.user_input_rx.recv() => {
                    self.handle_user_input_request(request);
                }

                // Handle exec-approval requests from sandbox-governed handlers.
                Some(request) = self.exec_approval_rx.recv() => {
                    self.handle_exec_approval_request(request);
                }

                // Drive subtle status/tool animations while the agent is active.
                _ = animation_tick.tick() => {
                    if matches!(
                        self.widget.bottom_pane.status,
                        AgentStatus::Thinking | AgentStatus::Retrying | AgentStatus::ExecutingTool
                    ) || self.welcome_active {
                        self.schedule_draw();
                    }
                }
            }
        }

        self.interrupt_agent_task();
        self.mcp_write_tracker.drain().await;
        let exit_info = self.exit_info.clone().unwrap_or(AppExitInfo {
            reason: ExitReason::UserRequested,
        });
        self.create_mcp_exit_decision(&exit_info.reason).await;

        Ok(exit_info)
    }

    fn begin_turn(&mut self) -> TurnId {
        let turn_id = self.next_turn_id;
        self.next_turn_id = self.next_turn_id.saturating_add(1);
        self.active_turn_id = Some(turn_id);
        self.active_turn_signal.store(turn_id, Ordering::Relaxed);
        self.active_turn_run_id = Some(Arc::new(Mutex::new(None)));
        turn_id
    }

    fn clear_active_turn(&mut self) {
        self.active_turn_id = None;
        self.active_turn_signal.store(0, Ordering::Relaxed);
        self.active_turn_run_id = None;
    }

    fn clear_turn_tracking(&mut self) {
        self.clear_active_turn();
        self.clear_mcp_run_id();
    }

    fn is_active_turn(&self, turn_id: TurnId) -> bool {
        self.active_turn_id == Some(turn_id)
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
            self.cancel_pending_exec_approval();
            self.dismiss_post_plan_dialog();
            self.interrupt_agent_task();
            self.exit_info = Some(AppExitInfo {
                reason: ExitReason::UserRequested,
            });
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
                        if self.welcome_active {
                            self.welcome_active = false;
                            self.schedule_draw();
                        }
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
            AgentStatus::AwaitingApproval => match key.code {
                KeyCode::Up => {
                    if let Some(ref mut pending) = self.pending_exec_approval {
                        pending.selected = pending.selected.saturating_sub(1);
                        self.widget.bottom_pane.exec_approval_selected = pending.selected;
                    }
                    self.schedule_draw();
                }
                KeyCode::Down => {
                    if let Some(ref mut pending) = self.pending_exec_approval {
                        pending.selected = (pending.selected + 1).min(3);
                        self.widget.bottom_pane.exec_approval_selected = pending.selected;
                    }
                    self.schedule_draw();
                }
                KeyCode::Enter => {
                    self.submit_exec_approval_decision();
                }
                KeyCode::Esc => {
                    self.reject_pending_exec_approval();
                }
                _ => {}
            },
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
                let mux_visible = self.widget.has_task_mux();
                match key.code {
                    KeyCode::Tab if mux_visible => {
                        self.widget.task_mux_next();
                        self.sync_mux_input_context();
                        self.schedule_draw();
                    }
                    KeyCode::BackTab if mux_visible => {
                        self.widget.task_mux_prev();
                        self.sync_mux_input_context();
                        self.schedule_draw();
                    }
                    KeyCode::Char('o')
                        if mux_visible && key.modifiers.contains(KeyModifiers::CONTROL) =>
                    {
                        self.widget.task_mux_show_overview();
                        self.sync_mux_input_context();
                        self.schedule_draw();
                    }
                    KeyCode::Char('f')
                        if mux_visible && key.modifiers.contains(KeyModifiers::CONTROL) =>
                    {
                        self.widget.task_mux_focus_selected();
                        self.sync_mux_input_context();
                        self.schedule_draw();
                    }
                    KeyCode::Enter if mux_visible && !self.widget.bottom_pane.is_empty() => {
                        let command = self.widget.bottom_pane.take_input();
                        if let Some(args) = command.trim().strip_prefix("/mux") {
                            if !self.handle_mux_command(args) {
                                self.widget.add_cell(Box::new(AssistantHistoryCell::new(
                                    "Unknown mux command. Use `/mux next`, `/mux prev`, `/mux focus <n>`, `/mux overview`, or `/mux toggle`.".to_string(),
                                )));
                            }
                        } else {
                            self.widget.add_cell(Box::new(AssistantHistoryCell::new(
                                "Workflow is running. Only local `/mux ...` commands are accepted in the shared input box right now.".to_string(),
                            )));
                        }
                        self.schedule_draw();
                    }
                    KeyCode::Char(c) if mux_visible => {
                        self.widget.bottom_pane.insert_char(c);
                        self.schedule_draw();
                    }
                    KeyCode::Backspace if mux_visible => {
                        self.widget.bottom_pane.backspace();
                        self.schedule_draw();
                    }
                    KeyCode::Delete if mux_visible => {
                        self.widget.bottom_pane.delete();
                        self.schedule_draw();
                    }
                    KeyCode::Left if mux_visible => {
                        self.widget.bottom_pane.cursor_left();
                        self.schedule_draw();
                    }
                    KeyCode::Right if mux_visible => {
                        self.widget.bottom_pane.cursor_right();
                        self.schedule_draw();
                    }
                    KeyCode::Esc if mux_visible && !self.widget.bottom_pane.is_empty() => {
                        self.widget.bottom_pane.clear();
                        self.schedule_draw();
                    }
                    KeyCode::Esc => {
                        self.enqueue_mcp_turn_decision(
                            "abandon",
                            "Turn interrupted by user".to_string(),
                        );
                        self.interrupt_agent_task();
                        self.clear_mcp_run_id();
                        self.widget.bottom_pane.set_status(AgentStatus::Idle);
                        self.sync_mux_input_context();
                        self.complete_streaming_assistant_cell("Interrupted.".to_string());
                        self.complete_running_tool_cells_with_interrupt();
                        self.schedule_draw();
                    }
                    _ => {}
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
                    let max = max_selectable_option(base, q.is_other);
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
                    let max = max_selectable_option(base, q.is_other);
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
                self.sync_mux_input_context();
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

        let done = if let Some(pending) = self.pending_user_input.as_mut() {
            let question_id = pending.request.questions[pending.current_question]
                .id
                .clone();
            pending.answers.insert(question_id, answer);
            pending.current_question += 1;
            pending.selected_option = 0;
            pending.notes_focused = false;
            pending.notes_text.clear();
            self.widget.bottom_pane.clear();
            pending.current_question >= pending.request.questions.len()
        } else {
            return;
        };

        if done {
            // Send the response back to the handler.
            let Some(pending) = self.pending_user_input.take() else {
                return;
            };
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
        self.sync_mux_input_context();
        self.widget.bottom_pane.clear();
        self.sync_user_input_to_pane();
        self.schedule_draw();
    }

    fn handle_exec_approval_request(&mut self, request: ExecApprovalRequest) {
        if self.active_turn_id.is_none() {
            let _ = request.response_tx.send(ReviewDecision::Denied);
            return;
        }

        self.widget.bottom_pane.set_exec_approval(
            Some(request.command.clone()),
            Some(request.cwd.display().to_string()),
            request.reason.clone(),
            request.is_retry,
            Some(request.sandbox_label.clone()),
            request.network_access,
            request
                .writable_roots
                .iter()
                .map(|path| path.display().to_string())
                .collect(),
        );
        self.pending_exec_approval = Some(PendingExecApproval {
            request,
            selected: 0,
        });
        self.widget.bottom_pane.exec_approval_selected = 0;
        self.widget
            .bottom_pane
            .set_status(AgentStatus::AwaitingApproval);
        self.sync_mux_input_context();
        self.schedule_draw();
    }

    fn submit_exec_approval_decision(&mut self) {
        let Some(pending) = self.pending_exec_approval.take() else {
            return;
        };

        let decision = match pending.selected {
            0 => ReviewDecision::Approved,
            1 => ReviewDecision::ApprovedForSession,
            2 => ReviewDecision::Denied,
            _ => ReviewDecision::Abort,
        };
        let _ = pending.request.response_tx.send(decision);

        self.widget
            .bottom_pane
            .set_exec_approval(None, None, None, false, None, false, Vec::new());

        if decision == ReviewDecision::Abort {
            self.enqueue_mcp_turn_decision(
                "abandon",
                "Turn interrupted by approval dialog".to_string(),
            );
            self.interrupt_agent_task();
            self.clear_mcp_run_id();
            self.widget.bottom_pane.set_status(AgentStatus::Idle);
            self.complete_streaming_assistant_cell("Interrupted.".to_string());
            self.complete_running_tool_cells_with_interrupt();
            self.schedule_draw();
            return;
        }

        self.widget
            .bottom_pane
            .set_status(AgentStatus::ExecutingTool);
        self.sync_mux_input_context();
        self.schedule_draw();
    }

    fn reject_pending_exec_approval(&mut self) {
        if let Some(pending) = self.pending_exec_approval.take() {
            let _ = pending.request.response_tx.send(ReviewDecision::Denied);
        }
        self.widget
            .bottom_pane
            .set_exec_approval(None, None, None, false, None, false, Vec::new());
        self.widget
            .bottom_pane
            .set_status(AgentStatus::ExecutingTool);
        self.sync_mux_input_context();
        self.schedule_draw();
    }

    fn cancel_pending_exec_approval(&mut self) {
        if let Some(pending) = self.pending_exec_approval.take() {
            let _ = pending.request.response_tx.send(ReviewDecision::Denied);
        }
        self.widget
            .bottom_pane
            .set_exec_approval(None, None, None, false, None, false, Vec::new());
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
    async fn handle_app_event(&mut self, event: AppEvent) -> anyhow::Result<()> {
        if !self.is_active_turn(event.turn_id()) {
            return Ok(());
        }

        match event {
            AppEvent::SubmitUserMessage {
                turn_id,
                text,
                allowed_tools,
            } => {
                // Track in session
                self.running_tool_calls = 0;
                self.session.add_user_message(&text);

                // Add user cell immediately
                self.widget
                    .add_cell(Box::new(UserHistoryCell::new(text.clone())));

                // Add streaming assistant placeholder (kept as the last cell).
                self.widget
                    .add_cell(Box::new(AssistantHistoryCell::streaming()));
                self.widget.bottom_pane.set_status(AgentStatus::Thinking);
                self.sync_mux_input_context();
                self.schedule_draw();
                self.clear_mcp_run_id();
                if let Some(run_id_slot) = self.active_turn_run_id.as_ref()
                    && let Ok(mut slot) = run_id_slot.lock()
                {
                    *slot = None;
                }
                if let Some(mcp_server) = self.mcp_server.clone() {
                    let tx = self.app_event_tx.clone();
                    let working_dir = self.registry.working_dir().to_path_buf();
                    let plan_id = self.mcp_plan_id.clone();
                    let mcp_text = text.clone();
                    tokio::spawn(async move {
                        match timeout(
                            MCP_TURN_TRACKING_TIMEOUT,
                            resolve_mcp_turn_tracking(mcp_server, plan_id, working_dir, mcp_text),
                        )
                        .await
                        {
                            Ok(result) => {
                                let _ = tx.send(AppEvent::McpTurnTrackingReady {
                                    turn_id,
                                    run_id: result.run_id,
                                });
                            }
                            Err(_) => {
                                tracing::warn!("MCP turn tracking timed out before agent start");
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
                let run_id = self
                    .active_turn_run_id
                    .clone()
                    .unwrap_or_else(|| Arc::new(Mutex::new(None)));
                let mcp_write_tracker = self.mcp_write_tracker.clone();

                // Execute agent call in background task
                let handle = tokio::spawn(async move {
                    struct UiObserver {
                        tx: UnboundedSender<AppEvent>,
                        mcp_server: Option<Arc<LibraMcpServer>>,
                        run_id: Arc<Mutex<Option<String>>>,
                        mcp_write_tracker: McpWriteTracker,
                        turn_id: TurnId,
                    }

                    impl crate::internal::ai::agent::ToolLoopObserver for UiObserver {
                        fn on_assistant_step_text(&mut self, text: &str) {
                            let cell = Box::new(AssistantHistoryCell::new(text.to_string()));
                            let _ = self.tx.send(AppEvent::InsertHistoryCell {
                                turn_id: self.turn_id,
                                cell,
                            });
                        }

                        fn on_tool_call_begin(
                            &mut self,
                            call_id: &str,
                            tool_name: &str,
                            arguments: &serde_json::Value,
                        ) {
                            let _ = self.tx.send(AppEvent::ToolCallBegin {
                                turn_id: self.turn_id,
                                task_id: None,
                                call_id: call_id.to_string(),
                                tool_name: tool_name.to_string(),
                                arguments: arguments.clone(),
                            });
                        }

                        fn on_tool_call_end(
                            &mut self,
                            call_id: &str,
                            tool_name: &str,
                            result: &Result<ToolOutput, String>,
                        ) {
                            let _ = self.tx.send(AppEvent::ToolCallEnd {
                                turn_id: self.turn_id,
                                task_id: None,
                                call_id: call_id.to_string(),
                                tool_name: tool_name.to_string(),
                                result: result.clone(),
                            });

                            // Record tool invocation via MCP with final status.
                            let run_id = self.run_id.lock().ok().and_then(|slot| slot.clone());
                            if let (Some(mcp_server), Some(run_id)) =
                                (self.mcp_server.clone(), run_id)
                            {
                                let tool_name = tool_name.to_string();
                                let result = result.clone();
                                self.mcp_write_tracker.spawn(async move {
                                    let (status, result_summary) = match &result {
                                        Ok(output) => {
                                            ("ok".to_string(), Some(summarize_tool_output(output)))
                                        }
                                        Err(err) => ("error".to_string(), Some(err.clone())),
                                    };

                                    let invocation_params = CreateToolInvocationParams {
                                        run_id,
                                        tool_name,
                                        status: Some(status),
                                        args_json: None,
                                        io_footprint: None,
                                        result_summary,
                                        artifacts: None,
                                        tags: None,
                                        external_ids: None,
                                        actor_kind: Some("agent".to_string()),
                                        actor_id: Some("libra-agent".to_string()),
                                    };

                                    let actor = match mcp_server.resolve_actor_from_params(
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

                                    match mcp_server
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
                    }

                    let mut observer = UiObserver {
                        tx,
                        mcp_server,
                        run_id,
                        mcp_write_tracker,
                        turn_id,
                    };
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
                            let _ = observer.tx.send(AppEvent::AgentEvent {
                                turn_id: observer.turn_id,
                                event: AgentEvent::ResponseComplete {
                                    text: turn.final_text,
                                    new_history: turn.history,
                                },
                            });
                        }
                        Err(e) => {
                            let _ = observer.tx.send(AppEvent::AgentEvent {
                                turn_id: observer.turn_id,
                                event: AgentEvent::Error {
                                    message: e.to_string(),
                                },
                            });
                        }
                    }
                });

                self.agent_task = Some(handle);
            }
            AppEvent::AgentEvent {
                turn_id: _turn_id,
                event: agent_event,
            } => {
                match agent_event {
                    AgentEvent::ResponseComplete { text, new_history } => {
                        self.enqueue_mcp_turn_decision(
                            "checkpoint",
                            "Turn completed successfully".to_string(),
                        );
                        self.finish_turn_state();
                        self.history = new_history;

                        // Track in session
                        self.session.add_assistant_message(&text);
                        self.complete_streaming_assistant_cell(text);
                        self.set_idle_and_draw();
                    }
                    AgentEvent::Error { message } => {
                        self.enqueue_mcp_turn_decision(
                            "abandon",
                            format!("Turn failed: {message}"),
                        );
                        self.finish_turn_state();

                        self.complete_streaming_assistant_cell(format!("Error: {}", message));
                        self.set_idle_and_draw();
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
                turn_id: _turn_id,
                text,
                new_history,
                intent_id,
                plan_id,
                spec_json,
                spec,
                plan,
                warnings,
            } => {
                self.finish_turn_state();
                self.history = new_history;
                self.session.add_assistant_message(&text);
                self.session.metadata.insert(
                    LATEST_INTENTSPEC_JSON.to_string(),
                    serde_json::Value::String(spec_json.clone()),
                );
                let binding = current_intentspec_binding(self.registry.working_dir());
                self.session.metadata.insert(
                    LATEST_INTENTSPEC_WORKSPACE_KEY.to_string(),
                    serde_json::Value::String(binding.workspace_key),
                );
                self.session.metadata.insert(
                    LATEST_INTENTSPEC_BASE_REF.to_string(),
                    serde_json::Value::String(binding.base_ref),
                );
                if let Some(branch_label) = binding.branch_label {
                    self.session.metadata.insert(
                        LATEST_INTENTSPEC_BRANCH_LABEL.to_string(),
                        serde_json::Value::String(branch_label),
                    );
                } else {
                    self.session.metadata.remove(LATEST_INTENTSPEC_BRANCH_LABEL);
                }
                if let Some(ref id) = intent_id {
                    self.session.metadata.insert(
                        LATEST_INTENTSPEC_INTENT_ID.to_string(),
                        serde_json::Value::String(id.clone()),
                    );
                } else {
                    self.session.metadata.remove(LATEST_INTENTSPEC_INTENT_ID);
                }
                if let Some(id) = plan_id.clone() {
                    self.session.metadata.insert(
                        LATEST_EXECUTION_PLAN_ID.to_string(),
                        serde_json::Value::String(id.clone()),
                    );
                    self.mcp_plan_id = Some(id);
                } else {
                    self.session.metadata.remove(LATEST_EXECUTION_PLAN_ID);
                    self.mcp_plan_id = None;
                }

                self.widget.show_dag_panel((*plan).clone());
                self.replace_streaming_assistant_cell(Box::new(PlanSummaryHistoryCell::new(
                    *spec,
                    *plan,
                    intent_id.clone(),
                    plan_id.clone(),
                    warnings,
                )));

                // Show post-plan dialog instead of returning to Idle
                self.pending_post_plan = Some(PendingPostPlan {
                    spec_json,
                    selected: 0,
                });
                self.widget.bottom_pane.reset_post_plan_selection();
                self.widget
                    .bottom_pane
                    .set_status(AgentStatus::AwaitingPostPlanChoice);
                self.sync_mux_input_context();
                self.schedule_draw();
            }
            AppEvent::InsertHistoryCell {
                turn_id: _turn_id,
                cell,
            } => {
                self.insert_before_streaming_assistant(cell);
                self.schedule_draw();
            }
            AppEvent::DagGraphBegin {
                turn_id: _turn_id,
                plan,
            } => {
                self.widget.show_dag_panel(plan);
                self.sync_mux_input_context();
                self.schedule_draw();
            }
            AppEvent::DagTaskStatus {
                turn_id: _turn_id,
                task_id,
                status,
            } => {
                self.widget.update_dag_task_status(task_id, status);
                self.schedule_draw();
            }
            AppEvent::DagGraphProgress {
                turn_id: _turn_id,
                completed,
                total,
            } => {
                self.widget.update_dag_progress(completed, total);
                self.schedule_draw();
            }
            AppEvent::ToolCallBegin {
                turn_id: _turn_id,
                task_id,
                call_id,
                tool_name,
                arguments,
            } => {
                if let Some(task_id) = task_id {
                    let summary = summarize_task_tool_begin(&tool_name, &arguments);
                    self.widget
                        .push_task_mux_log(task_id, TaskMuxLogKind::Tool, summary);
                }
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
                } else if !append_to_last_tool_group_cell(
                    &mut self.widget.cells,
                    call_id.clone(),
                    tool_name.as_str(),
                    arguments.clone(),
                ) {
                    let cell = Box::new(ToolCallHistoryCell::new(call_id, tool_name, arguments));
                    self.insert_before_streaming_assistant(cell);
                }
                self.running_tool_calls = self.running_tool_calls.saturating_add(1);
                self.update_status_after_tool_progress();
                self.schedule_draw();
            }
            AppEvent::ToolCallEnd {
                turn_id: _turn_id,
                task_id,
                call_id,
                tool_name,
                result,
            } => {
                if let Some(task_id) = task_id {
                    let summary = summarize_task_tool_end(&tool_name, &result);
                    let kind = if result.is_ok() {
                        TaskMuxLogKind::Note
                    } else {
                        TaskMuxLogKind::Error
                    };
                    self.widget.push_task_mux_log(task_id, kind, summary);
                }
                let should_hide_failure = should_hide_tool_failure(&tool_name, &result);
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
                    let mut pending_result = Some(result);
                    for idx in (0..self.widget.cells.len()).rev() {
                        let Some(tool_cell) = self.widget.cells[idx]
                            .as_any_mut()
                            .downcast_mut::<ToolCallHistoryCell>()
                        else {
                            continue;
                        };
                        if !tool_cell.contains_call_id(&call_id) {
                            continue;
                        }
                        if should_hide_failure && tool_cell.hides_failed_calls() {
                            tool_cell.remove_call(&call_id);
                            if tool_cell.is_empty() {
                                self.widget.cells.remove(idx);
                            }
                        } else if let Some(result) = pending_result.take() {
                            tool_cell.complete_call(&call_id, result);
                        }
                        break;
                    }
                }
                self.running_tool_calls = self.running_tool_calls.saturating_sub(1);
                self.update_status_after_tool_progress();
                self.schedule_draw();
            }
            AppEvent::AgentStatusUpdate {
                turn_id: _turn_id,
                status,
            } => {
                self.widget.bottom_pane.set_status(status);
                self.sync_mux_input_context();
                self.schedule_draw();
            }
            AppEvent::TaskWorkspaceReady {
                turn_id: _turn_id,
                task_id,
                working_dir,
                isolated,
            } => {
                self.widget
                    .set_task_workspace(task_id, working_dir, isolated);
                self.schedule_draw();
            }
            AppEvent::TaskMuxLog {
                turn_id: _turn_id,
                task_id,
                kind,
                text,
            } => {
                self.widget.push_task_mux_log(task_id, kind, text);
                self.schedule_draw();
            }
            AppEvent::McpTurnTrackingReady {
                turn_id: _turn_id,
                run_id,
            } => {
                self.mcp_run_id = run_id.clone();
                if let Some(run_id_slot) = self.active_turn_run_id.as_ref()
                    && let Ok(mut slot) = run_id_slot.lock()
                {
                    *slot = run_id;
                }
            }
            AppEvent::ExecuteWorkflowComplete {
                turn_id: _turn_id,
                text,
                new_history,
                result,
            } => {
                self.finish_turn_state();
                self.history = new_history;
                self.session.add_assistant_message(&text);
                if let Some(result) = result {
                    self.replace_streaming_assistant_cell(Box::new(
                        OrchestratorResultHistoryCell::new(*result),
                    ));
                } else {
                    self.complete_streaming_assistant_cell(text);
                }
                self.set_idle_and_draw();
            }
        }

        Ok(())
    }

    /// Submit a user message, expanding slash commands and applying agent context.
    async fn submit_message(&mut self, text: String) {
        // 1. Check for built-in TUI commands first.
        if let Some((cmd, args)) = super::slash_command::parse_builtin(&text) {
            self.handle_builtin_command(cmd, args).await;
            return;
        }

        if let Some(spec_json) = self.pending_plan_revision.take() {
            if text.trim_start().starts_with('/') {
                self.pending_plan_revision = Some(spec_json);
                self.widget.add_cell(Box::new(AssistantHistoryCell::new(
                    pending_plan_revision_help_message(),
                )));
                self.sync_mux_input_context();
                self.schedule_draw();
                return;
            }
            self.begin_plan_revision_flow(spec_json, &text).await;
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

        self.widget.clear_dag_panel();
        self.sync_mux_input_context();
        let turn_id = self.begin_turn();
        let _ = self.app_event_tx.send(AppEvent::SubmitUserMessage {
            turn_id,
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
                self.mcp_plan_id = None;
                self.mcp_run_id = None;
                self.pending_plan_revision = None;
                self.sync_mux_input_context();
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
                if let Some(spec_json) = self.pending_plan_revision.take() {
                    match parse_pending_plan_revision_command(args) {
                        PendingPlanRevisionCommand::Modify(request) => {
                            self.begin_plan_revision_flow(spec_json, request).await;
                        }
                        PendingPlanRevisionCommand::Cancel => {
                            self.widget.add_cell(Box::new(AssistantHistoryCell::new(
                                "Spec revision canceled.".to_string(),
                            )));
                            self.widget.bottom_pane.set_status(AgentStatus::Idle);
                            self.sync_mux_input_context();
                            self.schedule_draw();
                        }
                        PendingPlanRevisionCommand::Invalid => {
                            self.pending_plan_revision = Some(spec_json);
                            self.widget.add_cell(Box::new(AssistantHistoryCell::new(
                                pending_plan_revision_help_message(),
                            )));
                            self.sync_mux_input_context();
                            self.schedule_draw();
                        }
                    }
                } else {
                    self.start_plan_workflow(args).await;
                }
            }
            BuiltinCommand::Intent => {
                self.handle_intent_command(args).await;
            }
            BuiltinCommand::Mux => {
                let handled = self.handle_mux_command(args);
                if !handled {
                    self.widget.add_cell(Box::new(AssistantHistoryCell::new(
                        "Mux unavailable. Start a parallel workflow first, or use `/mux next|prev|focus <n>|overview|toggle|list` while it is running.".to_string(),
                    )));
                }
                self.schedule_draw();
            }
            BuiltinCommand::Quit => {
                self.exit_info = Some(AppExitInfo {
                    reason: ExitReason::UserRequested,
                });
            }
        }
    }

    fn handle_mux_command(&mut self, args: &str) -> bool {
        if !self.widget.has_task_mux() {
            return false;
        }

        let trimmed = args.trim();
        let mut parts = trimmed.split_whitespace();
        let command = parts.next().unwrap_or("toggle");
        let handled = match command {
            "" | "toggle" => self.widget.task_mux_toggle_mode(),
            "next" => self.widget.task_mux_next(),
            "prev" => self.widget.task_mux_prev(),
            "overview" => self.widget.task_mux_show_overview(),
            "focus" => {
                if let Some(index_text) = parts.next() {
                    index_text
                        .parse::<usize>()
                        .ok()
                        .and_then(|index| index.checked_sub(1))
                        .is_some_and(|index| self.widget.task_mux_focus_index(index))
                } else {
                    self.widget.task_mux_focus_selected()
                }
            }
            "list" => {
                if let Some(label) = self.widget.task_mux_context_label() {
                    self.widget
                        .add_cell(Box::new(AssistantHistoryCell::new(format!(
                            "Task mux: {label}"
                        ))));
                    true
                } else {
                    false
                }
            }
            _ => false,
        };
        if handled {
            self.sync_mux_input_context();
        }
        handled
    }

    async fn create_mcp_exit_decision(&self, reason: &ExitReason) {
        let (Some(mcp_server), Some(run_id)) = (self.mcp_server.clone(), self.mcp_run_id.clone())
        else {
            return;
        };

        let (decision_type, rationale) = match reason {
            ExitReason::UserRequested => ("abandon", "Session ended by user request".to_string()),
            ExitReason::Fatal(message) => (
                "abandon",
                format!("Session ended due to fatal error: {message}"),
            ),
        };

        write_mcp_decision(mcp_server, run_id, decision_type.to_string(), rationale).await;
    }

    fn enqueue_mcp_turn_decision(&self, decision_type: &str, rationale: String) {
        let (Some(mcp_server), Some(run_id)) = (self.mcp_server.clone(), self.mcp_run_id.clone())
        else {
            return;
        };
        let decision_type = decision_type.to_string();
        self.mcp_write_tracker.spawn(async move {
            write_mcp_decision(mcp_server, run_id, decision_type, rationale).await;
        });
    }

    fn clear_mcp_run_id(&mut self) {
        self.mcp_run_id = None;
    }

    fn finish_turn_state(&mut self) {
        self.cancel_pending_exec_approval();
        self.agent_task = None;
        self.running_tool_calls = 0;
        self.clear_turn_tracking();
    }

    fn set_idle_and_draw(&mut self) {
        self.widget
            .bottom_pane
            .set_git_branch(current_git_branch_label(self.registry.working_dir()));
        self.widget.bottom_pane.set_status(AgentStatus::Idle);
        self.sync_mux_input_context();
        self.schedule_draw();
    }

    fn sync_mux_input_context(&mut self) {
        let show_mux_context = self.widget.has_task_mux()
            && matches!(
                self.widget.bottom_pane.status,
                AgentStatus::Thinking
                    | AgentStatus::Retrying
                    | AgentStatus::ExecutingTool
                    | AgentStatus::AwaitingApproval
            );
        if show_mux_context {
            self.widget
                .bottom_pane
                .set_input_context_label(self.widget.task_mux_context_label());
            self.widget
                .bottom_pane
                .set_input_hint(self.widget.task_mux_input_hint());
        } else if self.pending_plan_revision.is_some()
            && matches!(self.widget.bottom_pane.status, AgentStatus::Idle)
        {
            self.widget
                .bottom_pane
                .set_input_context_label(Some("Revise IntentSpec".to_string()));
            self.widget.bottom_pane.set_input_hint(Some(
                "Describe spec changes, or use /plan modify <changes> or /plan cancel"
                    .to_string(),
            ));
        } else {
            self.widget.bottom_pane.set_input_context_label(None);
            self.widget.bottom_pane.set_input_hint(None);
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
                    self.pending_plan_revision = Some(pending.spec_json);
                    let msg = format!(
                        "{} Your next plain-text message will revise the current spec.",
                        pending_plan_revision_help_message()
                    );
                    self.widget
                        .add_cell(Box::new(AssistantHistoryCell::new(msg.clone())));
                    self.history.push(Message::assistant(msg.clone()));
                    self.session.add_assistant_message(&msg);
                }
                self.widget.bottom_pane.set_status(AgentStatus::Idle);
                self.sync_mux_input_context();
            }
        }
        self.schedule_draw();
    }

    fn dismiss_post_plan_dialog(&mut self) {
        self.pending_post_plan = None;
        self.set_idle_and_draw();
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
                self.set_idle_and_draw();
                return;
            }
        };

        self.widget.clear_dag_panel();
        self.widget
            .add_cell(Box::new(AssistantHistoryCell::streaming()));
        self.widget.bottom_pane.set_status(AgentStatus::Thinking);
        self.sync_mux_input_context();
        self.schedule_draw();
        let turn_id = self.begin_turn();
        self.running_tool_calls = 0;

        let model = self.model.clone();
        let registry = self.registry.clone();
        let working_dir = self.registry.working_dir().to_path_buf();
        let coder_preamble = self
            .agent_router
            .get("coder")
            .map(|a| a.system_prompt.clone());
        let reviewer_preamble = self
            .agent_router
            .get("code_reviewer")
            .map(|a| a.system_prompt.clone());
        let mcp_server = self.mcp_server.clone();
        let tx = self.app_event_tx.clone();
        let history = self.history.clone();

        let handle = tokio::spawn(async move {
            struct UiOrchestratorObserver {
                tx: UnboundedSender<AppEvent>,
                turn_id: TurnId,
            }

            impl UiOrchestratorObserver {
                fn send_note(&self, text: String) {
                    let _ = self.tx.send(AppEvent::InsertHistoryCell {
                        turn_id: self.turn_id,
                        cell: Box::new(AssistantHistoryCell::new(text)),
                    });
                }

                fn scoped_call_id(task: &TaskSpec, call_id: &str) -> String {
                    format!("{}:{call_id}", task.id())
                }

                fn summarize_gate_check(
                    check: &crate::internal::ai::intentspec::types::Check,
                ) -> String {
                    match check.kind {
                        crate::internal::ai::intentspec::types::CheckKind::Policy => {
                            format!("policy {}", check.id)
                        }
                        crate::internal::ai::intentspec::types::CheckKind::Command
                        | crate::internal::ai::intentspec::types::CheckKind::TestSuite => check
                            .command
                            .as_deref()
                            .map(str::trim)
                            .filter(|command| !command.is_empty())
                            .map(|command| command.to_string())
                            .unwrap_or_else(|| check.id.clone()),
                    }
                }
            }

            impl OrchestratorObserver for UiOrchestratorObserver {
                fn on_plan_compiled(&self, plan: &ExecutionPlanSpec) {
                    let _ = self.tx.send(AppEvent::DagGraphBegin {
                        turn_id: self.turn_id,
                        plan: plan.clone(),
                    });
                }

                fn on_task_started(&self, task: &TaskSpec) {
                    let _ = self.tx.send(AppEvent::AgentStatusUpdate {
                        turn_id: self.turn_id,
                        status: AgentStatus::Thinking,
                    });
                    let _ = self.tx.send(AppEvent::TaskMuxLog {
                        turn_id: self.turn_id,
                        task_id: task.id(),
                        kind: TaskMuxLogKind::Note,
                        text: format!("started {}", task.title()),
                    });
                    let _ = self.tx.send(AppEvent::DagTaskStatus {
                        turn_id: self.turn_id,
                        task_id: task.id(),
                        status: crate::internal::ai::orchestrator::types::TaskNodeStatus::Running,
                    });
                }

                fn on_task_workspace_ready(
                    &self,
                    task: &TaskSpec,
                    working_dir: &std::path::Path,
                    isolated: bool,
                ) {
                    self.send_note(format_task_workspace_note(
                        task.title(),
                        working_dir,
                        isolated,
                    ));
                    let _ = self.tx.send(AppEvent::TaskWorkspaceReady {
                        turn_id: self.turn_id,
                        task_id: task.id(),
                        working_dir: working_dir.to_path_buf(),
                        isolated,
                    });
                }

                fn on_task_completed(
                    &self,
                    task: &TaskSpec,
                    result: &crate::internal::ai::orchestrator::types::TaskResult,
                ) {
                    let completion_summary = format_task_completion_note(task.title(), result);
                    let _ = self.tx.send(AppEvent::TaskMuxLog {
                        turn_id: self.turn_id,
                        task_id: task.id(),
                        kind: if result.status
                            == crate::internal::ai::orchestrator::types::TaskNodeStatus::Failed
                        {
                            TaskMuxLogKind::Error
                        } else {
                            TaskMuxLogKind::Note
                        },
                        text: completion_summary.clone(),
                    });
                    let _ = self.tx.send(AppEvent::DagTaskStatus {
                        turn_id: self.turn_id,
                        task_id: task.id(),
                        status: result.status.clone(),
                    });
                    self.send_note(completion_summary);
                }

                fn on_task_assistant_message(&self, task: &TaskSpec, text: &str) {
                    let _ = self.tx.send(AppEvent::TaskMuxLog {
                        turn_id: self.turn_id,
                        task_id: task.id(),
                        kind: TaskMuxLogKind::Assistant,
                        text: text.to_string(),
                    });
                }

                fn on_tool_call_begin(
                    &self,
                    task: &TaskSpec,
                    call_id: &str,
                    tool_name: &str,
                    arguments: &serde_json::Value,
                ) {
                    let _ = self.tx.send(AppEvent::ToolCallBegin {
                        turn_id: self.turn_id,
                        task_id: Some(task.id()),
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
                        turn_id: self.turn_id,
                        task_id: Some(task.id()),
                        call_id: Self::scoped_call_id(task, call_id),
                        tool_name: tool_name.to_string(),
                        result: result.clone(),
                    });
                }

                fn on_reviewer_started(&self, _task: &TaskSpec) {}

                fn on_reviewer_completed(
                    &self,
                    _task: &TaskSpec,
                    _review: Option<&crate::internal::ai::orchestrator::types::ReviewOutcome>,
                ) {
                }

                fn on_gate_check_started(
                    &self,
                    task: &TaskSpec,
                    check: &crate::internal::ai::intentspec::types::Check,
                ) {
                    let summary = Self::summarize_gate_check(check);
                    self.send_note(format!(
                        "Gate Check · {}  \nrunning · {}",
                        task.title(),
                        summary
                    ));
                    let _ = self.tx.send(AppEvent::TaskMuxLog {
                        turn_id: self.turn_id,
                        task_id: task.id(),
                        kind: TaskMuxLogKind::Tool,
                        text: format!("gate running · {}", summary),
                    });
                }

                fn on_gate_check_completed(
                    &self,
                    task: &TaskSpec,
                    check: &crate::internal::ai::intentspec::types::Check,
                    result: &crate::internal::ai::orchestrator::types::GateResult,
                ) {
                    let summary = Self::summarize_gate_check(check);
                    let outcome = if result.passed { "passed" } else { "failed" };
                    let detail = if result.timed_out {
                        "timed out".to_string()
                    } else {
                        format!("exit {}", result.exit_code)
                    };
                    let mut metrics = vec![outcome.to_string(), summary];
                    if result.duration_ms > 0 {
                        metrics.push(format!("{} ms", result.duration_ms));
                    }
                    metrics.push(detail);
                    self.send_note(format!(
                        "Gate Check · {}  \n{}",
                        task.title(),
                        metrics.join(" · ")
                    ));
                    let _ = self.tx.send(AppEvent::TaskMuxLog {
                        turn_id: self.turn_id,
                        task_id: task.id(),
                        kind: if result.passed {
                            TaskMuxLogKind::Note
                        } else {
                            TaskMuxLogKind::Error
                        },
                        text: format!("gate {} · {}", outcome, metrics.join(" · ")),
                    });
                }

                fn on_graph_progress(&self, completed: usize, total: usize) {
                    let _ = self.tx.send(AppEvent::DagGraphProgress {
                        turn_id: self.turn_id,
                        completed,
                        total,
                    });
                }

                fn on_graph_checkpoint_saved(
                    &self,
                    _checkpoint_id: &str,
                    _pc: usize,
                    _completed_nodes: usize,
                ) {
                }

                fn on_graph_checkpoint_restored(&self, _checkpoint_id: &str, _pc: usize) {}

                fn on_replan(
                    &self,
                    _current_revision: u32,
                    _next_revision: u32,
                    _reason: &str,
                    _diff_summary: &str,
                ) {
                }

                fn on_persistence_complete(&self, _execution: &PersistedExecution) {}
            }

            let observer: Arc<dyn OrchestratorObserver> = Arc::new(UiOrchestratorObserver {
                tx: tx.clone(),
                turn_id,
            });
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

            let (summary, ui_result) = match &result {
                Ok(r) => (format_orchestrator_result(r), Some(Box::new(r.clone()))),
                Err(e) => (format!("Orchestrator failed: {e}"), None),
            };

            let mut new_history = history;
            new_history.push(Message::assistant(summary.clone()));

            let _ = tx.send(AppEvent::ExecuteWorkflowComplete {
                turn_id,
                text: summary,
                new_history,
                result: ui_result,
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

        let prompt_body = build_plan_prompt(request);
        let prompt = if let Some(agent) = self.agent_router.get("planner") {
            format!("{}\n\n---\n\n{}", agent.system_prompt, prompt_body)
        } else {
            prompt_body
        };
        let user_text = format!("/plan {request}");
        self.begin_plan_workflow(user_text, prompt).await;
    }

    async fn begin_plan_revision_flow(&mut self, spec_json: String, request: &str) {
        let request = request.trim();
        if request.is_empty() {
            self.widget.add_cell(Box::new(AssistantHistoryCell::new(
                pending_plan_revision_help_message(),
            )));
            self.pending_plan_revision = Some(spec_json);
            self.sync_mux_input_context();
            self.schedule_draw();
            return;
        }

        let prompt_body = build_plan_revision_prompt(&spec_json, request);
        let prompt = if let Some(agent) = self.agent_router.get("planner") {
            format!("{}\n\n---\n\n{}", agent.system_prompt, prompt_body)
        } else {
            prompt_body
        };
        let user_text = format!("Modify current spec: {request}");
        self.begin_plan_workflow(user_text, prompt).await;
    }

    async fn begin_plan_workflow(&mut self, user_text: String, prompt: String) {
        self.pending_plan_revision = None;
        let turn_id = self.begin_turn();
        self.running_tool_calls = 0;
        self.session.add_user_message(&user_text);
        self.widget
            .add_cell(Box::new(UserHistoryCell::new(user_text.clone())));
        self.widget.clear_dag_panel();
        self.sync_mux_input_context();
        self.widget
            .add_cell(Box::new(AssistantHistoryCell::streaming()));
        self.widget.bottom_pane.set_status(AgentStatus::Thinking);
        self.sync_mux_input_context();
        self.schedule_draw();

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
                turn_id: TurnId,
                draft: Option<IntentDraft>,
                risk_prompted: bool,
                selected_risk: Option<RiskLevel>,
            }

            impl PlanObserver {
                fn new(tx: UnboundedSender<AppEvent>, turn_id: TurnId) -> Self {
                    Self {
                        tx,
                        turn_id,
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
                        turn_id: self.turn_id,
                        task_id: None,
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
                        turn_id: self.turn_id,
                        task_id: None,
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

            let mut observer = PlanObserver::new(tx.clone(), turn_id);
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
                        let _ = tx.send(AppEvent::InsertHistoryCell {
                            turn_id,
                            cell: Box::new(AssistantHistoryCell::new(format!(
                                "Planner response failed after draft submission. Continuing with the submitted draft.\nReason: {}",
                                e
                            ))),
                        });
                        None
                    } else {
                        let _ = tx.send(AppEvent::AgentEvent {
                            turn_id,
                            event: AgentEvent::Error {
                                message: e.to_string(),
                            },
                        });
                        return;
                    }
                }
            };

            if !observer.risk_prompted {
                let _ = tx.send(AppEvent::AgentEvent {
                    turn_id,
                    event: AgentEvent::Error {
                        message: "Plan failed: planner did not ask for risk profile.".to_string(),
                    },
                });
                return;
            }

            let risk_level = match observer.selected_risk.clone() {
                Some(level) => level,
                None => {
                    let _ = tx.send(AppEvent::AgentEvent {
                        turn_id,
                        event: AgentEvent::Error {
                            message: "Plan failed: risk profile was not selected.".to_string(),
                        },
                    });
                    return;
                }
            };

            let draft = match observer.draft.take() {
                Some(d) => d,
                None => {
                    let _ = tx.send(AppEvent::AgentEvent {
                        turn_id,
                        event: AgentEvent::Error {
                            message: "Plan failed: no intent draft was submitted.".to_string(),
                        },
                    });
                    return;
                }
            };

            let mut spec = resolve_intentspec(
                draft,
                risk_level,
                ResolveContext {
                    working_dir: canonical_working_dir_label(&working_dir),
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
                let _ = tx.send(AppEvent::AgentEvent {
                    turn_id,
                    event: AgentEvent::Error {
                        message: format!(
                            "Plan failed after automatic repair.\nValidation issues:\n{}",
                            report
                        ),
                    },
                });
                return;
            }

            let canonical =
                match crate::internal::ai::intentspec::canonical::to_canonical_json(&spec) {
                    Ok(v) => v,
                    Err(e) => {
                        let _ = tx.send(AppEvent::AgentEvent {
                            turn_id,
                            event: AgentEvent::Error {
                                message: format!("Plan failed: cannot serialize IntentSpec: {e}"),
                            },
                        });
                        return;
                    }
                };

            let mut persistence_warning = None;
            let intent_id = if let Some(ref mcp_server) = mcp_server {
                let params = CreateIntentParams {
                    content: "IntentSpec generated by planner".to_string(),
                    structured_content: Some(canonical),
                    parent_id: None,
                    parent_ids: None,
                    analysis_context_frame_ids: None,
                    status: Some("active".to_string()),
                    commit_sha: None,
                    reason: None,
                    next_intent_id: None,
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
                    let _ = tx.send(AppEvent::AgentEvent {
                        turn_id,
                        event: AgentEvent::Error {
                            message: format!("Plan failed: cannot compile execution plan: {e}"),
                        },
                    });
                    return;
                }
            };

            let mut summary = render_summary(&spec, intent_id.as_deref());
            let mut plan_warning = None;
            let plan_id = if let (Some(mcp_server), Some(intent_id)) =
                (mcp_server.as_ref(), intent_id.as_ref())
            {
                match persist_execution_plan(&execution_plan, intent_id, mcp_server).await {
                    Ok(id) => Some(id),
                    Err(e) => {
                        plan_warning = Some(format!("failed to persist execution plan: {e}"));
                        None
                    }
                }
            } else if mcp_server.is_some() {
                plan_warning = Some(
                    "intent persistence unavailable; execution plan not persisted.".to_string(),
                );
                None
            } else {
                plan_warning =
                    Some("MCP server unavailable; execution plan not persisted.".to_string());
                None
            };

            if let Some(ref warn) = persistence_warning {
                summary.push_str(&format!("\nWarning: {warn}"));
            }
            if let Some(ref warn) = plan_warning {
                summary.push_str(&format!("\nWarning: {warn}"));
            }
            summary.push_str("\n\nExecution plan ready. Review the workflow card and choose Execute / Modify / Cancel below.");

            let warnings = [persistence_warning, plan_warning]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();

            let mut new_history = turn.map(|turn| turn.history).unwrap_or(fallback_history);
            new_history.push(Message::assistant(summary.clone()));

            let _ = tx.send(AppEvent::PlanWorkflowComplete {
                turn_id,
                text: summary,
                new_history,
                intent_id,
                plan_id,
                spec_json: pretty_json,
                spec: Box::new(spec),
                plan: Box::new(execution_plan),
                warnings,
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
            "execute" => {
                let Some(spec_json) = self.load_latest_intentspec_json().await else {
                    self.widget.add_cell(Box::new(AssistantHistoryCell::new(
                        "No IntentSpec found. Run `/plan <requirement>` first.".to_string(),
                    )));
                    self.schedule_draw();
                    return;
                };
                self.start_execute_workflow(&spec_json).await;
            }
            _ => {
                self.widget.add_cell(Box::new(AssistantHistoryCell::new(
                    "Usage: /intent show|execute".to_string(),
                )));
                self.schedule_draw();
            }
        }
    }

    async fn load_latest_intentspec_json(&self) -> Option<String> {
        let binding = current_intentspec_binding(self.registry.working_dir());
        let binding_matches = latest_intentspec_binding_matches(&self.session.metadata, &binding);
        if !binding_matches {
            return None;
        }

        if let Some(json_text) = self
            .session
            .metadata
            .get(LATEST_INTENTSPEC_JSON)
            .and_then(|v| v.as_str())
            && let Ok(spec) =
                serde_json::from_str::<crate::internal::ai::intentspec::IntentSpec>(json_text)
            && intentspec_matches_workspace(&spec, &binding.workspace_key, &binding.base_ref)
        {
            return Some(json_text.to_string());
        }

        let mcp = self.mcp_server.clone()?;
        if let Some(id) = self
            .session
            .metadata
            .get(LATEST_INTENTSPEC_INTENT_ID)
            .and_then(|v| v.as_str())
            && let Some(spec) = fetch_intentspec_from_object_id(&mcp, id).await
            && intentspec_matches_workspace(&spec, &binding.workspace_key, &binding.base_ref)
        {
            return serde_json::to_string_pretty(&spec).ok();
        }

        None
    }

    fn interrupt_agent_task(&mut self) {
        if let Some(handle) = self.agent_task.take() {
            handle.abort();
        }
        self.clear_active_turn();
        self.running_tool_calls = 0;
    }

    fn update_status_after_tool_progress(&mut self) {
        let next_status = if self.pending_post_plan.is_some() {
            AgentStatus::AwaitingPostPlanChoice
        } else if self.pending_exec_approval.is_some() {
            AgentStatus::AwaitingApproval
        } else if self.pending_user_input.is_some() {
            AgentStatus::AwaitingUserInput
        } else if self.running_tool_calls > 0 {
            AgentStatus::ExecutingTool
        } else if self.agent_task.is_some() {
            AgentStatus::Thinking
        } else {
            AgentStatus::Idle
        };
        self.widget.bottom_pane.set_status(next_status);
        self.sync_mux_input_context();
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

    fn replace_streaming_assistant_cell(&mut self, replacement: Box<dyn HistoryCell>) {
        for idx in (0..self.widget.cells.len()).rev() {
            if let Some(assistant_cell) = self.widget.cells[idx]
                .as_any()
                .downcast_ref::<AssistantHistoryCell>()
                && assistant_cell.is_streaming
            {
                self.widget.cells[idx] = replacement;
                return;
            }
        }
        self.widget.add_cell(replacement);
    }

    fn complete_running_tool_cells_with_interrupt(&mut self) {
        for cell in self.widget.cells.iter_mut() {
            if let Some(tool_cell) = cell.as_any_mut().downcast_mut::<ToolCallHistoryCell>() {
                tool_cell.interrupt_running();
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
            let cursor_pos = if self.welcome_active {
                let chat_area = self.widget.chat_area_rect(area);
                let welcome_view = WelcomeView {
                    welcome_message: &self.welcome_message,
                    model_name: &self.model_name,
                    provider_name: &self.provider_name,
                    cwd: self.registry.working_dir(),
                };
                welcome_shader::render(chat_area, frame.buffer_mut(), welcome_view);
                self.widget
                    .render_bottom_pane_only(area, frame.buffer_mut())
            } else {
                self.widget.render(area, frame.buffer_mut())
            };
            if let Some(pos) = cursor_pos {
                frame.set_cursor_position(pos);
            }
        })?;
        Ok(())
    }
}

enum PendingPlanRevisionCommand<'a> {
    Modify(&'a str),
    Cancel,
    Invalid,
}

fn parse_pending_plan_revision_command(args: &str) -> PendingPlanRevisionCommand<'_> {
    let trimmed = args.trim();
    if trimmed.eq_ignore_ascii_case("cancel") {
        return PendingPlanRevisionCommand::Cancel;
    }
    if let Some(request) = trimmed
        .strip_prefix("modify ")
        .or_else(|| trimmed.strip_prefix("revise "))
        .map(str::trim)
        .filter(|request| !request.is_empty())
    {
        return PendingPlanRevisionCommand::Modify(request);
    }
    PendingPlanRevisionCommand::Invalid
}

fn pending_plan_revision_help_message() -> String {
    "Revise mode is active. Describe changes in plain text, use `/plan modify <changes>` to keep revising, or `/plan cancel` to exit.".to_string()
}

fn append_to_last_tool_group_cell(
    cells: &mut Vec<Box<dyn super::history_cell::HistoryCell>>,
    call_id: String,
    tool_name: &str,
    arguments: serde_json::Value,
) -> bool {
    let anchor_index = if let Some(streaming_index) = cells.iter().rposition(|cell| {
        cell.as_any()
            .downcast_ref::<AssistantHistoryCell>()
            .is_some_and(|assistant| assistant.is_streaming)
    }) {
        streaming_index.checked_sub(1)
    } else {
        cells.len().checked_sub(1)
    };

    let Some(anchor_index) = anchor_index else {
        return false;
    };

    let Some(tool_cell) = cells[anchor_index]
        .as_any_mut()
        .downcast_mut::<ToolCallHistoryCell>()
    else {
        return false;
    };

    if !tool_cell.can_merge(tool_name) {
        return false;
    }

    tool_cell.append_call(call_id, tool_name.to_string(), arguments);
    true
}

fn should_hide_tool_failure(tool_name: &str, result: &Result<ToolOutput, String>) -> bool {
    matches!(
        tool_name,
        "read_file" | "list_dir" | "grep_files" | "apply_patch"
    ) && !matches!(result, Ok(output) if output.is_success())
}

fn summarize_task_tool_begin(tool_name: &str, arguments: &serde_json::Value) -> String {
    let mut summary = tool_name.to_string();
    match tool_name {
        "read_file" | "list_dir" | "grep_files" | "apply_patch" | "shell" => {
            if let Some(path) = arguments.get("path").and_then(|value| value.as_str()) {
                summary.push_str(&format!(" · {}", truncate_for_tool_log(path, 36)));
            } else if let Some(command) = arguments.get("cmd").and_then(|value| value.as_str()) {
                summary.push_str(&format!(" · {}", truncate_for_tool_log(command, 36)));
            }
        }
        _ => {}
    }
    summary
}

fn summarize_task_tool_end(tool_name: &str, result: &Result<ToolOutput, String>) -> String {
    match result {
        Ok(output) => {
            let status = if output.is_success() {
                "ok"
            } else {
                "needs attention"
            };
            format!("{tool_name} · {status}")
        }
        Err(error) => format!(
            "{tool_name} · failed · {}",
            truncate_for_tool_log(error, 40)
        ),
    }
}

fn truncate_for_tool_log(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut truncated = text
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
}

fn format_orchestrator_result(
    result: &crate::internal::ai::orchestrator::types::OrchestratorResult,
) -> String {
    let mut lines = Vec::new();
    let decision_label = orchestrator_decision_label(&result.decision);
    let groups = result.execution_plan_spec.parallel_groups();
    let lane_count = groups.iter().map(Vec::len).max().unwrap_or(0);
    let layer_count = groups.len();
    lines.push(format!("# Execution Result: {decision_label}"));
    lines.push(String::new());

    lines.push("## Overview".to_string());
    lines.push("| Field | Value |".to_string());
    lines.push("| --- | --- |".to_string());
    lines.push(format!("| Decision | {decision_label} |"));
    lines.push(format!(
        "| Revision | {} |",
        result.execution_plan_spec.revision
    ));
    lines.push(format!(
        "| Tasks | {} |",
        result.execution_plan_spec.tasks.len()
    ));
    lines.push(format!(
        "| Max parallel | {} |",
        result.execution_plan_spec.max_parallel
    ));
    lines.push(format!("| Active lanes | {} |", lane_count));
    lines.push(format!("| Layers | {} |", layer_count));
    lines.push(format!("| Replans | {} |", result.replan_count));
    lines.push(format!(
        "| Intent | `{}` |",
        short_markdown_id(&result.intent_spec_id)
    ));
    if let Some(persistence) = &result.persistence {
        lines.push(format!(
            "| Run | `{}` |",
            short_markdown_id(&persistence.run_id)
        ));
        lines.push(format!("| Persisted tasks | {} |", persistence.tasks.len()));
        lines.push(format!(
            "| Checkpoints | {} |",
            persistence.checkpoints.len()
        ));
    }
    lines.push(String::new());

    lines.push("## Verification".to_string());
    lines.push("| Check | Status | Notes |".to_string());
    lines.push("| --- | --- | --- |".to_string());
    lines.push(format!(
        "| Integration | {} | {} |",
        bool_label(result.system_report.integration.all_required_passed),
        gate_report_summary(&result.system_report.integration)
    ));
    lines.push(format!(
        "| Security | {} | {} |",
        bool_label(result.system_report.security.all_required_passed),
        gate_report_summary(&result.system_report.security)
    ));
    lines.push(format!(
        "| Release | {} | {} |",
        bool_label(result.system_report.release.all_required_passed),
        gate_report_summary(&result.system_report.release)
    ));
    lines.push(format!(
        "| Review | {} | {} |",
        bool_label(result.system_report.review_passed),
        if result.system_report.review_findings.is_empty() {
            "No findings".to_string()
        } else {
            format!("{} findings", result.system_report.review_findings.len())
        }
    ));
    lines.push(format!(
        "| Artifacts | {} | {} |",
        bool_label(result.system_report.artifacts_complete),
        if result.system_report.missing_artifacts.is_empty() {
            "Complete".to_string()
        } else {
            format!(
                "Missing {}",
                result.system_report.missing_artifacts.join(", ")
            )
        }
    ));

    if !result.system_report.review_findings.is_empty() {
        lines.push(String::new());
        lines.push("### Review Findings".to_string());
        for finding in &result.system_report.review_findings {
            lines.push(format!("- {}", finding));
        }
    }
    if !result.system_report.missing_artifacts.is_empty() {
        lines.push(String::new());
        lines.push("### Missing Artifacts".to_string());
        for artifact in &result.system_report.missing_artifacts {
            lines.push(format!("- `{artifact}`"));
        }
    }

    lines.push(String::new());
    lines.push("## Tasks".to_string());
    lines.push("| Task | Status | Retries | Tools | Violations | Notes |".to_string());
    lines.push("| --- | --- | ---: | ---: | ---: | --- |".to_string());
    for (idx, task) in result.execution_plan_spec.tasks.iter().enumerate() {
        let task_result = result
            .task_results
            .iter()
            .find(|item| item.task_id == task.id());
        let kind = match task.kind {
            crate::internal::ai::orchestrator::types::TaskKind::Implementation => "I",
            crate::internal::ai::orchestrator::types::TaskKind::Analysis => "A",
            crate::internal::ai::orchestrator::types::TaskKind::Gate => "G",
        };
        let label = format!("{kind}{:02} {}", idx + 1, task.title());
        let (status, retries, tools, violations, notes) = if let Some(task_result) = task_result {
            let notes = if let Some(review) = task_result.review.as_ref() {
                let mut note = format!(
                    "Review: {} | approved: {}",
                    review.summary,
                    if review.approved { "yes" } else { "no" }
                );
                if !review.issues.is_empty() {
                    note.push_str(&format!(" | Issues: {}", review.issues.join("; ")));
                }
                note
            } else if task_result.status
                == crate::internal::ai::orchestrator::types::TaskNodeStatus::Failed
            {
                if let Some(reason) = task_result
                    .agent_output
                    .as_deref()
                    .map(str::trim)
                    .filter(|reason| !reason.is_empty())
                {
                    format!("Reason: {reason}")
                } else if let Some(reason) =
                    summarize_failed_gate_report(task_result.gate_report.as_ref())
                {
                    format!("Reason: {reason}")
                } else {
                    "-".to_string()
                }
            } else {
                "-".to_string()
            };
            (
                orchestrator_status_label(&task_result.status),
                task_result.retry_count.to_string(),
                task_result.tool_calls.len().to_string(),
                task_result.policy_violations.len().to_string(),
                notes,
            )
        } else {
            (
                "pending",
                "0".to_string(),
                "0".to_string(),
                "0".to_string(),
                "-".to_string(),
            )
        };
        lines.push(format!(
            "| {} | {} | {} | {} | {} | {} |",
            escape_markdown_cell(&label),
            status,
            retries,
            tools,
            violations,
            escape_markdown_cell(&notes)
        ));
    }

    if let Some(persistence) = &result.persistence {
        lines.push(String::new());
        lines.push("## Persistence".to_string());
        lines.push("| Object | Value |".to_string());
        lines.push("| --- | --- |".to_string());
        lines.push(format!(
            "| Provenance | `{}` |",
            persistence
                .provenance_id
                .as_deref()
                .map(short_markdown_id)
                .unwrap_or_else(|| "none".to_string())
        ));
        lines.push(format!(
            "| Decision object | `{}` |",
            persistence
                .decision_id
                .as_deref()
                .map(short_markdown_id)
                .unwrap_or_else(|| "none".to_string())
        ));
        lines.push(format!(
            "| Initial snapshot | `{}` |",
            persistence
                .initial_snapshot_id
                .as_deref()
                .map(short_markdown_id)
                .unwrap_or_else(|| "none".to_string())
        ));
        if !persistence.checkpoints.is_empty() {
            lines.push(String::new());
            lines.push("### Checkpoints".to_string());
            lines.push("| Rev | Snapshot | Decision | Reason |".to_string());
            lines.push("| --- | --- | --- | --- |".to_string());
            for checkpoint in &persistence.checkpoints {
                lines.push(format!(
                    "| {} | `{}` | `{}` | {} |",
                    checkpoint.revision,
                    checkpoint
                        .snapshot_id
                        .as_deref()
                        .map(short_markdown_id)
                        .unwrap_or_else(|| "none".to_string()),
                    checkpoint
                        .decision_id
                        .as_deref()
                        .map(short_markdown_id)
                        .unwrap_or_else(|| "none".to_string()),
                    escape_markdown_cell(&checkpoint.reason)
                ));
            }
        }
    }

    lines.join("\n")
}

fn orchestrator_decision_label(
    decision: &crate::internal::ai::orchestrator::types::DecisionOutcome,
) -> &'static str {
    match decision {
        crate::internal::ai::orchestrator::types::DecisionOutcome::Commit => "Commit",
        crate::internal::ai::orchestrator::types::DecisionOutcome::HumanReviewRequired => {
            "Human Review Required"
        }
        crate::internal::ai::orchestrator::types::DecisionOutcome::Abandon => "Abandon",
    }
}

fn orchestrator_status_label(
    status: &crate::internal::ai::orchestrator::types::TaskNodeStatus,
) -> &'static str {
    match status {
        crate::internal::ai::orchestrator::types::TaskNodeStatus::Pending => "pending",
        crate::internal::ai::orchestrator::types::TaskNodeStatus::Running => "running",
        crate::internal::ai::orchestrator::types::TaskNodeStatus::Completed => "done",
        crate::internal::ai::orchestrator::types::TaskNodeStatus::Failed => "failed",
        crate::internal::ai::orchestrator::types::TaskNodeStatus::Skipped => "skipped",
    }
}

fn gate_report_summary(report: &crate::internal::ai::orchestrator::types::GateReport) -> String {
    if report.results.is_empty() {
        return "No checks".to_string();
    }
    let passed = report.results.iter().filter(|item| item.passed).count();
    format!("{passed}/{} checks passed", report.results.len())
}

fn bool_label(value: bool) -> &'static str {
    if value { "Pass" } else { "Fail" }
}

fn short_markdown_id(id: &str) -> String {
    if id.len() <= 12 {
        id.to_string()
    } else {
        format!("{}…", &id[..12])
    }
}

fn escape_markdown_cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use git_internal::internal::object::{task::Task as GitTask, types::ActorRef};
    use serde_json::json;

    use super::{
        append_to_last_tool_group_cell, build_plan_revision_prompt, format_orchestrator_result,
        parse_pending_plan_revision_command, pending_plan_revision_help_message,
        should_hide_tool_failure, PendingPlanRevisionCommand,
    };
    use crate::internal::{
        ai::{
            orchestrator::types::{
                DecisionOutcome, ExecutionPlanSpec, GateReport, OrchestratorResult, SystemReport,
                TaskContract, TaskKind, TaskNodeStatus, TaskResult, TaskSpec,
            },
            tools::ToolOutput,
        },
        tui::history_cell::{AssistantHistoryCell, HistoryCell, ToolCallHistoryCell},
    };

    fn make_task(title: &str, kind: TaskKind) -> TaskSpec {
        let actor = ActorRef::agent("format-orchestrator-result").unwrap();
        let task = GitTask::new(actor, title, None).unwrap();
        TaskSpec {
            step: git_internal::internal::object::plan::PlanStep::new(title),
            task,
            objective: title.to_string(),
            kind,
            gate_stage: None,
            owner_role: None,
            scope_in: vec![],
            scope_out: vec![],
            checks: vec![],
            contract: TaskContract::default(),
        }
    }

    fn orchestrator_fixture() -> OrchestratorResult {
        let first = make_task("Inspect sources", TaskKind::Implementation);
        let second = make_task("Run checks", TaskKind::Gate);
        let plan = ExecutionPlanSpec {
            intent_spec_id: "intent-1".into(),
            revision: 4,
            parent_revision: Some(3),
            replan_reason: Some("task kept failing after retries".into()),
            tasks: vec![first.clone(), second.clone()],
            max_parallel: 1,
            checkpoints: vec![],
        };
        OrchestratorResult {
            decision: DecisionOutcome::Abandon,
            execution_plan_spec: plan.clone(),
            plan_revision_specs: vec![plan],
            run_state: Default::default(),
            task_results: vec![TaskResult {
                task_id: first.id(),
                status: TaskNodeStatus::Failed,
                gate_report: None,
                agent_output: None,
                retry_count: 4,
                tool_calls: vec![],
                policy_violations: vec![],
                review: None,
            }],
            system_report: SystemReport {
                integration: GateReport::empty(),
                security: GateReport::empty(),
                release: GateReport::empty(),
                review_passed: false,
                review_findings: vec!["missing regression test".into()],
                artifacts_complete: false,
                missing_artifacts: vec!["patchset@per-task".into()],
                overall_passed: false,
            },
            intent_spec_id: "019ce515-077c-7c12-8e90-755533e512e3".into(),
            lifecycle_change_log: vec![],
            replan_count: 3,
            persistence: None,
        }
    }

    #[test]
    fn appends_to_last_matching_tool_group_before_streaming_cell() {
        let mut cells: Vec<Box<dyn HistoryCell>> = vec![
            Box::new(ToolCallHistoryCell::new(
                "1".to_string(),
                "read_file".to_string(),
                json!({"file_path":"src/main.rs"}),
            )),
            Box::new(AssistantHistoryCell::streaming()),
        ];

        assert!(append_to_last_tool_group_cell(
            &mut cells,
            "2".to_string(),
            "list_dir",
            json!({"dir_path":"src"}),
        ));

        let tool_cell = cells[0]
            .as_any()
            .downcast_ref::<ToolCallHistoryCell>()
            .expect("expected grouped tool cell");
        assert!(tool_cell.contains_call_id("1"));
        assert!(tool_cell.contains_call_id("2"));
    }

    #[test]
    fn does_not_append_across_non_tool_cells() {
        let mut cells: Vec<Box<dyn HistoryCell>> = vec![
            Box::new(ToolCallHistoryCell::new(
                "1".to_string(),
                "read_file".to_string(),
                json!({"file_path":"src/main.rs"}),
            )),
            Box::new(AssistantHistoryCell::new("note".to_string())),
            Box::new(AssistantHistoryCell::streaming()),
        ];

        assert!(!append_to_last_tool_group_cell(
            &mut cells,
            "2".to_string(),
            "list_dir",
            json!({"dir_path":"src"}),
        ));
    }

    #[test]
    fn orchestrator_result_markdown_uses_tables_and_sections() {
        let rendered = format_orchestrator_result(&orchestrator_fixture());

        assert!(rendered.contains("# Execution Result: Abandon"));
        assert!(rendered.contains("## Overview"));
        assert!(rendered.contains("| Field | Value |"));
        assert!(rendered.contains("## Verification"));
        assert!(rendered.contains("| Task | Status | Retries | Tools | Violations | Notes |"));
        assert!(rendered.contains("### Missing Artifacts"));
    }

    #[test]
    fn hides_failed_explore_and_edit_calls() {
        assert!(should_hide_tool_failure(
            "read_file",
            &Err("file not found".to_string())
        ));
        assert!(should_hide_tool_failure(
            "apply_patch",
            &Err("context mismatch".to_string())
        ));
    }

    #[test]
    fn keeps_failed_shell_calls_visible() {
        assert!(!should_hide_tool_failure(
            "shell",
            &Err("command exited with status 1".to_string())
        ));
        assert!(!should_hide_tool_failure(
            "read_file",
            &Ok(ToolOutput::success("ok"))
        ));
    }

    #[test]
    fn plan_revision_prompt_uses_current_spec_as_baseline() {
        let prompt = build_plan_revision_prompt(
            "{\n  \"kind\": \"IntentSpec\"\n}",
            "add an integration gate for cargo test",
        );

        assert!(prompt.contains("You are revising an existing IntentSpec."));
        assert!(prompt.contains("Current IntentSpec:"));
        assert!(prompt.contains("\"kind\": \"IntentSpec\""));
        assert!(prompt.contains("Requested changes:\nadd an integration gate for cargo test"));
        assert!(prompt.contains("submit_intent_draft exactly once"));
    }

    #[test]
    fn parses_pending_revision_builtin_commands() {
        assert!(matches!(
            parse_pending_plan_revision_command("modify tighten sandbox"),
            PendingPlanRevisionCommand::Modify("tighten sandbox")
        ));
        assert!(matches!(
            parse_pending_plan_revision_command("revise add checks"),
            PendingPlanRevisionCommand::Modify("add checks")
        ));
        assert!(matches!(
            parse_pending_plan_revision_command("cancel"),
            PendingPlanRevisionCommand::Cancel
        ));
        assert!(matches!(
            parse_pending_plan_revision_command("status"),
            PendingPlanRevisionCommand::Invalid
        ));
    }

    #[test]
    fn pending_revision_help_mentions_escape_hatch() {
        let help = pending_plan_revision_help_message();
        assert!(help.contains("/plan modify <changes>"));
        assert!(help.contains("/plan cancel"));
    }
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

fn max_selectable_option(base: usize, is_other: bool) -> usize {
    if is_other {
        base
    } else {
        base.saturating_sub(1)
    }
}

async fn write_mcp_decision(
    mcp_server: Arc<LibraMcpServer>,
    run_id: String,
    decision_type: String,
    rationale: String,
) {
    let decision_params = CreateDecisionParams {
        run_id,
        decision_type,
        chosen_patchset_id: None,
        result_commit_sha: None,
        rationale: Some(rationale),
        checkpoint_id: None,
        tags: None,
        external_ids: None,
        actor_kind: Some("system".to_string()),
        actor_id: Some("libra-code".to_string()),
    };
    let actor = match mcp_server.resolve_actor_from_params(
        decision_params.actor_kind.as_deref(),
        decision_params.actor_id.as_deref(),
    ) {
        Ok(actor) => actor,
        Err(e) => {
            cli_error!(e, "error: failed to resolve actor for decision");
            return;
        }
    };

    match mcp_server
        .create_decision_impl(decision_params, actor)
        .await
    {
        Ok(result) => {
            if result.is_error.unwrap_or(false) {
                render_mcp_error("failed to create decision", result.content);
            }
        }
        Err(e) => {
            cli_error!(e, "error: failed to create decision");
        }
    }
}

fn summarize_tool_output(output: &ToolOutput) -> String {
    let raw = match output {
        ToolOutput::Function { content, .. } => content.as_str().trim().to_string(),
        ToolOutput::Mcp { result } => serde_json::to_string(result).unwrap_or_default(),
    };
    const MAX_LEN: usize = 240;
    if raw.chars().count() <= MAX_LEN {
        raw
    } else {
        let mut truncated: String = raw.chars().take(MAX_LEN).collect();
        truncated.push_str("...");
        truncated
    }
}

async fn persist_execution_plan(
    plan: &ExecutionPlanSpec,
    intent_id: &str,
    mcp_server: &Arc<LibraMcpServer>,
) -> Result<String, String> {
    let git_plan = build_git_plan(
        parse_object_id(intent_id).map_err(|e| format!("invalid intent id: {e}"))?,
        plan,
    )
    .map_err(|e| format!("failed to build git plan: {e}"))?;
    let steps = git_plan
        .steps()
        .iter()
        .map(|step| crate::internal::ai::mcp::resource::PlanStepParams {
            description: step.description().to_string(),
            inputs: step.inputs().cloned(),
            checks: step.checks().cloned(),
        })
        .collect::<Vec<_>>();

    let params = CreatePlanParams {
        intent_id: intent_id.to_string(),
        parent_plan_ids: None,
        context_frame_ids: None,
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

fn build_plan_revision_prompt(spec_json: &str, request: &str) -> String {
    format!(
        "You are revising an existing IntentSpec.\n\
First, you MUST call request_user_input with exactly one question id=risk_profile, header=Risk, and options Low/Medium/High.\n\
Use the current IntentSpec as the baseline, apply only the user's requested changes, and then call submit_intent_draft exactly once.\n\
If required information is missing, call request_user_input again for focused follow-up questions.\n\
Do not output a plain-text plan; finalize by submitting the draft tool call.\n\n\
Current IntentSpec:\n```json\n{spec_json}\n```\n\n\
Requested changes:\n{request}"
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

struct IntentSpecBinding {
    workspace_key: String,
    base_ref: String,
    branch_label: Option<String>,
}

fn canonical_working_dir_label(working_dir: &std::path::Path) -> String {
    std::fs::canonicalize(working_dir)
        .unwrap_or_else(|_| working_dir.to_path_buf())
        .display()
        .to_string()
}

fn current_intentspec_binding(working_dir: &std::path::Path) -> IntentSpecBinding {
    IntentSpecBinding {
        workspace_key: canonical_working_dir_label(working_dir),
        base_ref: current_head_sha(working_dir),
        branch_label: current_git_branch_label(working_dir),
    }
}

fn latest_intentspec_binding_matches(
    metadata: &HashMap<String, serde_json::Value>,
    binding: &IntentSpecBinding,
) -> bool {
    metadata
        .get(LATEST_INTENTSPEC_WORKSPACE_KEY)
        .and_then(|value| value.as_str())
        .is_some_and(|workspace| workspace == binding.workspace_key)
        && metadata
            .get(LATEST_INTENTSPEC_BASE_REF)
            .and_then(|value| value.as_str())
            .is_some_and(|base_ref| base_ref == binding.base_ref)
        && metadata
            .get(LATEST_INTENTSPEC_BRANCH_LABEL)
            .and_then(|value| value.as_str())
            == binding.branch_label.as_deref()
}

fn intentspec_matches_workspace(
    spec: &crate::internal::ai::intentspec::IntentSpec,
    workspace_locator: &str,
    head_sha: &str,
) -> bool {
    spec.metadata.target.repo.locator == workspace_locator && spec.metadata.target.base_ref == head_sha
}

fn current_git_branch_label(working_dir: &std::path::Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
        .current_dir(working_dir)
        .output()
        .ok()?;

    if output.status.success() {
        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !branch.is_empty() {
            return Some(branch);
        }
    }

    let detached = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(working_dir)
        .output()
        .ok()?;

    if !detached.status.success() {
        return None;
    }

    let sha = String::from_utf8_lossy(&detached.stdout).trim().to_string();
    if sha.is_empty() {
        None
    } else {
        Some(format!("detached@{sha}"))
    }
}

async fn current_head_sha_async(working_dir: std::path::PathBuf) -> String {
    tokio::task::spawn_blocking(move || current_head_sha(&working_dir))
        .await
        .unwrap_or_else(|_| "HEAD".to_string())
}

#[derive(Debug, Clone, Default)]
struct McpTurnTrackingResult {
    run_id: Option<String>,
}

async fn resolve_mcp_turn_tracking(
    mcp_server: Arc<LibraMcpServer>,
    plan_id: Option<String>,
    working_dir: std::path::PathBuf,
    text: String,
) -> McpTurnTrackingResult {
    let snapshot_params = CreateContextSnapshotParams {
        selection_strategy: "heuristic".to_string(),
        items: None,
        summary: Some(format!("Context for: {text}")),
        tags: None,
        external_ids: None,
        actor_kind: Some("system".to_string()),
        actor_id: Some("libra-code".to_string()),
    };
    let snapshot_actor = match mcp_server.resolve_actor_from_params(
        snapshot_params.actor_kind.as_deref(),
        snapshot_params.actor_id.as_deref(),
    ) {
        Ok(actor) => actor,
        Err(e) => {
            cli_error!(e, "error: failed to resolve actor for snapshot");
            return McpTurnTrackingResult::default();
        }
    };
    let context_snapshot_id = match mcp_server
        .create_context_snapshot_impl(snapshot_params, snapshot_actor)
        .await
    {
        Ok(result) => {
            if result.is_error.unwrap_or(false) {
                render_mcp_error("failed to create context snapshot", result.content);
                None
            } else {
                parse_created_id(&result)
            }
        }
        Err(e) => {
            cli_error!(e, "error: failed to create context snapshot");
            None
        }
    };

    let task_params = CreateTaskParams {
        title: summarize_turn_task_title(&text),
        description: Some("Interactive TUI user request".to_string()),
        goal_type: None,
        constraints: None,
        acceptance_criteria: None,
        requested_by_kind: Some("human".to_string()),
        requested_by_id: Some("user".to_string()),
        dependencies: None,
        intent_id: None,
        parent_task_id: None,
        origin_step_id: None,
        status: Some("created".to_string()),
        reason: Some("start user turn".to_string()),
        tags: None,
        external_ids: None,
        actor_kind: Some("human".to_string()),
        actor_id: Some("user".to_string()),
    };
    let task_actor = match mcp_server.resolve_actor_from_params(
        task_params.actor_kind.as_deref(),
        task_params.actor_id.as_deref(),
    ) {
        Ok(actor) => actor,
        Err(e) => {
            cli_error!(e, "error: failed to resolve actor for task");
            return McpTurnTrackingResult::default();
        }
    };

    let task_id = match mcp_server.create_task_impl(task_params, task_actor).await {
        Ok(result) => {
            if result.is_error.unwrap_or(false) {
                render_mcp_error("failed to create task", result.content);
                None
            } else {
                parse_created_id(&result)
            }
        }
        Err(e) => {
            cli_error!(e, "error: failed to create task");
            None
        }
    };
    let Some(task_id) = task_id else {
        return McpTurnTrackingResult::default();
    };

    let run_params = CreateRunParams {
        task_id: task_id.clone(),
        base_commit_sha: current_head_sha_async(working_dir).await,
        plan_id,
        status: Some("created".to_string()),
        context_snapshot_id: context_snapshot_id.clone(),
        error: None,
        agent_instances: None,
        metrics_json: None,
        reason: Some("start user turn".to_string()),
        orchestrator_version: None,
        tags: None,
        external_ids: None,
        actor_kind: Some("human".to_string()),
        actor_id: Some("user".to_string()),
    };
    let run_actor = match mcp_server.resolve_actor_from_params(
        run_params.actor_kind.as_deref(),
        run_params.actor_id.as_deref(),
    ) {
        Ok(actor) => actor,
        Err(e) => {
            cli_error!(e, "error: failed to resolve actor for run");
            return McpTurnTrackingResult::default();
        }
    };

    let run_id = match mcp_server.create_run_impl(run_params, run_actor).await {
        Ok(result) => {
            if result.is_error.unwrap_or(false) {
                render_mcp_error("failed to create run", result.content);
                None
            } else {
                parse_created_id(&result)
            }
        }
        Err(e) => {
            cli_error!(e, "error: failed to create run");
            None
        }
    };

    McpTurnTrackingResult { run_id }
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

fn summarize_turn_task_title(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return "TUI user request".to_string();
    }

    let mut title: String = trimmed.chars().take(72).collect();
    if trimmed.chars().count() > 72 {
        title.push_str("...");
    }
    format!("TUI: {title}")
}

fn format_task_completion_note(
    title: &str,
    result: &crate::internal::ai::orchestrator::types::TaskResult,
) -> String {
    let mut note = format!("{} · {}", task_status_heading(&result.status), title.trim());

    let mut metrics = Vec::new();
    if !result.tool_calls.is_empty() {
        metrics.push(format!("{} tools", result.tool_calls.len()));
    }
    if result.retry_count > 0 {
        metrics.push(format!("{} retries", result.retry_count));
    }
    if !result.policy_violations.is_empty() {
        let count = result.policy_violations.len();
        metrics.push(format!(
            "{} policy violation{}",
            count,
            if count == 1 { "" } else { "s" }
        ));
    }
    if !metrics.is_empty() {
        note.push_str(&format!("  \n{}", metrics.join(" · ")));
    }

    if let Some(review) = result.review.as_ref() {
        note.push_str(&format!(
            "  \nreview · {} · approved {}",
            review.summary,
            if review.approved { "yes" } else { "no" }
        ));
        if !review.issues.is_empty() {
            note.push_str(&format!("  \nissues · {}", review.issues.join("; ")));
        }
    } else if matches!(
        result.status,
        crate::internal::ai::orchestrator::types::TaskNodeStatus::Failed
    ) && let Some(reason) = result
        .agent_output
        .as_deref()
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
    {
        note.push_str(&format!("  \nreason · {}", reason));
    } else if matches!(
        result.status,
        crate::internal::ai::orchestrator::types::TaskNodeStatus::Failed
    ) && let Some(reason) = summarize_failed_gate_report(result.gate_report.as_ref())
    {
        note.push_str(&format!("  \nreason · {}", reason));
    }

    note
}

fn format_task_workspace_note(
    title: &str,
    working_dir: &std::path::Path,
    isolated: bool,
) -> String {
    let mode = if isolated {
        "isolated worktree"
    } else {
        "shared workspace"
    };
    format!(
        "Workspace · {}  \n{} · {}",
        title.trim(),
        mode,
        working_dir.display()
    )
}

fn task_status_heading(
    status: &crate::internal::ai::orchestrator::types::TaskNodeStatus,
) -> &'static str {
    match status {
        crate::internal::ai::orchestrator::types::TaskNodeStatus::Pending => "Pending",
        crate::internal::ai::orchestrator::types::TaskNodeStatus::Running => "Running",
        crate::internal::ai::orchestrator::types::TaskNodeStatus::Completed => "Completed",
        crate::internal::ai::orchestrator::types::TaskNodeStatus::Failed => "Failed",
        crate::internal::ai::orchestrator::types::TaskNodeStatus::Skipped => "Skipped",
    }
}

fn summarize_failed_gate_report(
    gate_report: Option<&crate::internal::ai::orchestrator::types::GateReport>,
) -> Option<String> {
    let report = gate_report?;
    let failed_checks: Vec<_> = report
        .results
        .iter()
        .filter(|result| !result.passed)
        .collect();
    if failed_checks.is_empty() {
        return None;
    }

    let summary = failed_checks
        .iter()
        .take(2)
        .map(|result| {
            let outcome = if result.timed_out {
                "timed out".to_string()
            } else {
                format!("exit {}", result.exit_code)
            };
            let detail = result
                .stderr
                .lines()
                .find(|line| !line.trim().is_empty())
                .or_else(|| result.stdout.lines().find(|line| !line.trim().is_empty()))
                .map(str::trim)
                .filter(|detail| !detail.is_empty())
                .map(|detail| format!(": {detail}"))
                .unwrap_or_default();
            format!("{} ({outcome}{detail})", result.check_id)
        })
        .collect::<Vec<_>>()
        .join("; ");

    let remainder = failed_checks.len().saturating_sub(2);
    if remainder > 0 {
        Some(format!("{summary}; +{remainder} more failed checks"))
    } else {
        Some(summary)
    }
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

#[cfg(test)]
mod orchestrator_result_tests {
    use std::collections::HashMap;

    use git_internal::internal::object::{plan::PlanStep, task::Task as GitTask, types::ActorRef};
    use uuid::Uuid;

    use super::{
        IntentSpecBinding, LATEST_INTENTSPEC_BASE_REF, LATEST_INTENTSPEC_BRANCH_LABEL,
        LATEST_INTENTSPEC_WORKSPACE_KEY, canonical_working_dir_label,
        format_orchestrator_result, format_task_completion_note, format_task_workspace_note,
        intentspec_matches_workspace, latest_intentspec_binding_matches,
    };
    use crate::internal::ai::orchestrator::{
        run_state::RunStateSnapshot,
        types::{
            DecisionOutcome, ExecutionPlanSpec, GateReport, GateResult, OrchestratorResult,
            ReviewOutcome, SystemReport, TaskContract, TaskKind, TaskNodeStatus, TaskResult,
            TaskSpec,
        },
    };
    use crate::internal::ai::intentspec::types::{
        Acceptance, Artifacts, ChangeType, ConstraintLicensing, ConstraintPlatform,
        ConstraintPrivacy, ConstraintResources, ConstraintSecurity, Constraints, CreatedBy,
        CreatorType, DomainAllowlistMode, EncodingPolicy, EvidencePolicy, EvidenceStrategy,
        ExecutionPolicy, HumanInLoop, Intent, IntentSpec, Lifecycle, LifecycleStatus, Metadata,
        NetworkPolicy, Objective, ObjectiveKind, OutputHandlingPolicy, PromptInjectionPolicy,
        ProvenanceBindings, ProvenancePolicy, RepoTarget, RepoType, RetryPolicy, Risk, RiskLevel,
        SecretAccessPolicy, SecretPolicy, SecurityPolicy, Target, ToolAcl,
        TransparencyLogPolicy, TransparencyMode, TrustTier, VerificationPlan,
    };

    fn test_task_spec(title: &str, kind: TaskKind) -> TaskSpec {
        let actor = ActorRef::agent("test-tui").unwrap();
        let task = GitTask::new(actor, title, None).unwrap();
        TaskSpec {
            step: PlanStep::new(title),
            task,
            objective: title.into(),
            kind,
            gate_stage: None,
            owner_role: Some("coder".into()),
            scope_in: vec![],
            scope_out: vec![],
            checks: vec![],
            contract: TaskContract::default(),
        }
    }

    fn intentspec_fixture(locator: &str, base_ref: &str) -> IntentSpec {
        IntentSpec {
            api_version: "intentspec.io/v1alpha1".into(),
            kind: "IntentSpec".into(),
            metadata: Metadata {
                id: "intent-1".into(),
                created_at: "2026-04-02T00:00:00Z".into(),
                created_by: CreatedBy {
                    creator_type: CreatorType::User,
                    id: "tester".into(),
                    display_name: None,
                },
                target: Target {
                    repo: RepoTarget {
                        repo_type: RepoType::Local,
                        locator: locator.into(),
                    },
                    base_ref: base_ref.into(),
                    workspace_id: None,
                    labels: Default::default(),
                },
            },
            intent: Intent {
                summary: "summary".into(),
                problem_statement: "problem".into(),
                change_type: ChangeType::Feature,
                objectives: vec![Objective {
                    title: "Ship feature".into(),
                    kind: ObjectiveKind::Implementation,
                }],
                in_scope: vec!["src/".into()],
                out_of_scope: vec![],
                touch_hints: None,
            },
            acceptance: Acceptance {
                success_criteria: vec!["tests pass".into()],
                verification_plan: VerificationPlan {
                    fast_checks: vec![],
                    integration_checks: vec![],
                    security_checks: vec![],
                    release_checks: vec![],
                },
                quality_gates: None,
            },
            constraints: Constraints {
                security: ConstraintSecurity {
                    network_policy: NetworkPolicy::Deny,
                    dependency_policy: crate::internal::ai::intentspec::types::DependencyPolicy::NoNew,
                    crypto_policy: String::new(),
                },
                privacy: ConstraintPrivacy {
                    data_classes_allowed: vec![],
                    redaction_required: false,
                    retention_days: 30,
                },
                licensing: ConstraintLicensing {
                    allowed_spdx: vec![],
                    forbid_new_licenses: false,
                },
                platform: ConstraintPlatform {
                    language_runtime: "rust".into(),
                    supported_os: vec![],
                },
                resources: ConstraintResources {
                    max_wall_clock_seconds: 30,
                    max_cost_units: 0,
                },
            },
            risk: Risk {
                level: RiskLevel::Low,
                rationale: "low".into(),
                factors: vec![],
                human_in_loop: HumanInLoop {
                    required: false,
                    min_approvers: 0,
                },
            },
            evidence: EvidencePolicy {
                strategy: EvidenceStrategy::RepoFirst,
                trust_tiers: vec![TrustTier::Repo],
                domain_allowlist_mode: DomainAllowlistMode::Disabled,
                allowed_domains: vec![],
                blocked_domains: vec![],
                min_citations_per_decision: 1,
            },
            security: SecurityPolicy {
                tool_acl: ToolAcl {
                    allow: vec![],
                    deny: vec![],
                },
                secrets: SecretPolicy {
                    policy: SecretAccessPolicy::DenyAll,
                    allowed_scopes: vec![],
                },
                prompt_injection: PromptInjectionPolicy {
                    treat_retrieved_content_as_untrusted: true,
                    enforce_output_schema: true,
                    disallow_instruction_from_evidence: true,
                },
                output_handling: OutputHandlingPolicy {
                    encoding_policy: EncodingPolicy::StrictJson,
                    no_direct_eval: true,
                },
            },
            execution: ExecutionPolicy {
                retry: RetryPolicy {
                    max_retries: 0,
                    backoff_seconds: 0,
                },
                replan: crate::internal::ai::intentspec::types::ReplanPolicy { triggers: vec![] },
                concurrency: crate::internal::ai::intentspec::types::ConcurrencyPolicy {
                    max_parallel_tasks: 1,
                },
            },
            artifacts: Artifacts {
                required: vec![],
                retention: crate::internal::ai::intentspec::types::ArtifactRetention { days: 30 },
            },
            provenance: ProvenancePolicy {
                require_slsa_provenance: false,
                require_sbom: false,
                transparency_log: TransparencyLogPolicy {
                    mode: TransparencyMode::None,
                },
                bindings: ProvenanceBindings {
                    embed_intent_spec_digest: false,
                    embed_evidence_digests: false,
                },
            },
            lifecycle: Lifecycle {
                schema_version: "1.0.0".into(),
                status: LifecycleStatus::Active,
                change_log: vec![],
            },
            libra: None,
            extensions: Default::default(),
        }
    }

    #[test]
    fn failed_task_note_includes_review_summary() {
        let note = format_task_completion_note(
            "Analyze requested scope",
            &TaskResult {
                task_id: Uuid::new_v4(),
                status: TaskNodeStatus::Failed,
                gate_report: None,
                agent_output: Some("partial analysis".into()),
                retry_count: 4,
                tool_calls: vec![],
                policy_violations: vec![],
                review: Some(ReviewOutcome {
                    approved: false,
                    summary: "response is incomplete".into(),
                    issues: vec!["missing final diagnosis".into()],
                }),
            },
        );

        assert!(note.contains("review · response is incomplete · approved no"));
        assert!(note.contains("issues · missing final diagnosis"));
    }

    #[test]
    fn failed_task_note_falls_back_to_failure_reason_when_review_is_missing() {
        let note = format_task_completion_note(
            "Analyze requested scope",
            &TaskResult {
                task_id: Uuid::new_v4(),
                status: TaskNodeStatus::Failed,
                gate_report: None,
                agent_output: Some("reviewer pass failed: invalid reviewer JSON".into()),
                retry_count: 4,
                tool_calls: vec![],
                policy_violations: vec![],
                review: None,
            },
        );

        assert!(note.contains("reason · reviewer pass failed: invalid reviewer JSON"));
    }

    #[test]
    fn failed_gate_note_includes_gate_failure_reason() {
        let note = format_task_completion_note(
            "Integration gate",
            &TaskResult {
                task_id: Uuid::new_v4(),
                status: TaskNodeStatus::Failed,
                gate_report: Some(GateReport {
                    results: vec![GateResult {
                        check_id: "cargo-test".into(),
                        kind: "command".into(),
                        passed: false,
                        exit_code: 101,
                        stdout: String::new(),
                        stderr: "tests failed".into(),
                        duration_ms: 1234,
                        timed_out: false,
                    }],
                    all_required_passed: false,
                }),
                agent_output: None,
                retry_count: 0,
                tool_calls: vec![],
                policy_violations: vec![],
                review: None,
            },
        );

        assert!(note.contains("reason · cargo-test (exit 101: tests failed)"));
    }

    #[test]
    fn workspace_note_includes_mode_and_directory() {
        let isolated_note = format_task_workspace_note(
            "Implement parser",
            std::path::Path::new("/tmp/libra/.libra/worktrees/task-1"),
            true,
        );
        assert!(isolated_note.contains("Workspace · Implement parser"));
        assert!(isolated_note.contains("isolated worktree"));
        assert!(isolated_note.contains("/tmp/libra/.libra/worktrees/task-1"));

        let shared_note =
            format_task_workspace_note("Run gate", std::path::Path::new("/tmp/libra"), false);
        assert!(shared_note.contains("shared workspace"));
        assert!(shared_note.contains("/tmp/libra"));
    }

    #[test]
    fn intentspec_match_requires_same_workspace_and_head() {
        let workspace = tempfile::tempdir().unwrap();
        let locator = canonical_working_dir_label(workspace.path());
        let spec = intentspec_fixture(&locator, "abc123");
        assert!(intentspec_matches_workspace(&spec, &locator, "abc123"));
        assert!(!intentspec_matches_workspace(&spec, &locator, "def456"));
    }

    #[test]
    fn intentspec_match_rejects_other_worktree_locator() {
        let workspace = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let locator = canonical_working_dir_label(workspace.path());
        let other_locator = canonical_working_dir_label(other.path());
        let spec = intentspec_fixture(&other_locator, "abc123");
        assert!(!intentspec_matches_workspace(&spec, &locator, "abc123"));
    }

    #[test]
    fn latest_intentspec_binding_requires_exact_worktree_head_and_branch() {
        let mut metadata = HashMap::new();
        metadata.insert(
            LATEST_INTENTSPEC_WORKSPACE_KEY.to_string(),
            serde_json::Value::String("/repo/worktree-a".into()),
        );
        metadata.insert(
            LATEST_INTENTSPEC_BASE_REF.to_string(),
            serde_json::Value::String("abc123".into()),
        );
        metadata.insert(
            LATEST_INTENTSPEC_BRANCH_LABEL.to_string(),
            serde_json::Value::String("feature/spec".into()),
        );

        assert!(latest_intentspec_binding_matches(
            &metadata,
            &IntentSpecBinding {
                workspace_key: "/repo/worktree-a".into(),
                base_ref: "abc123".into(),
                branch_label: Some("feature/spec".into()),
            }
        ));
        assert!(!latest_intentspec_binding_matches(
            &metadata,
            &IntentSpecBinding {
                workspace_key: "/repo/worktree-b".into(),
                base_ref: "abc123".into(),
                branch_label: Some("feature/spec".into()),
            }
        ));
        assert!(!latest_intentspec_binding_matches(
            &metadata,
            &IntentSpecBinding {
                workspace_key: "/repo/worktree-a".into(),
                base_ref: "def456".into(),
                branch_label: Some("feature/spec".into()),
            }
        ));
        assert!(!latest_intentspec_binding_matches(
            &metadata,
            &IntentSpecBinding {
                workspace_key: "/repo/worktree-a".into(),
                base_ref: "abc123".into(),
                branch_label: Some("main".into()),
            }
        ));
    }

    #[test]
    fn orchestrator_result_includes_task_review_and_failure_reason() {
        let reviewed_task = test_task_spec("Summarize findings", TaskKind::Analysis);
        let failed_task = test_task_spec("Count Rust modules", TaskKind::Analysis);
        let result = OrchestratorResult {
            decision: DecisionOutcome::Abandon,
            execution_plan_spec: ExecutionPlanSpec {
                intent_spec_id: "intent-1".into(),
                revision: 1,
                parent_revision: None,
                replan_reason: None,
                tasks: vec![reviewed_task.clone(), failed_task.clone()],
                max_parallel: 2,
                checkpoints: vec![],
            },
            plan_revision_specs: vec![],
            run_state: RunStateSnapshot::default(),
            task_results: vec![
                TaskResult {
                    task_id: reviewed_task.id(),
                    status: TaskNodeStatus::Completed,
                    gate_report: None,
                    agent_output: Some("done".into()),
                    retry_count: 0,
                    tool_calls: vec![],
                    policy_violations: vec![],
                    review: Some(ReviewOutcome {
                        approved: true,
                        summary: "analysis is complete".into(),
                        issues: vec![],
                    }),
                },
                TaskResult {
                    task_id: failed_task.id(),
                    status: TaskNodeStatus::Failed,
                    gate_report: None,
                    agent_output: Some(
                        "Agent reached final response without covering all objectives".into(),
                    ),
                    retry_count: 4,
                    tool_calls: vec![],
                    policy_violations: vec![],
                    review: None,
                },
            ],
            system_report: SystemReport {
                integration: GateReport::empty(),
                security: GateReport::empty(),
                release: GateReport::empty(),
                review_passed: true,
                review_findings: vec![],
                artifacts_complete: true,
                missing_artifacts: vec![],
                overall_passed: true,
            },
            intent_spec_id: "intent-1".into(),
            lifecycle_change_log: vec![],
            replan_count: 0,
            persistence: None,
        };

        let rendered = format_orchestrator_result(&result);

        assert!(rendered.contains("Review: analysis is complete \\| approved: yes"));
        assert!(
            rendered
                .contains("Reason: Agent reached final response without covering all objectives")
        );
    }

    #[test]
    fn orchestrator_result_includes_gate_failure_reason() {
        let gate_task = test_task_spec("Integration gate", TaskKind::Gate);
        let result = OrchestratorResult {
            decision: DecisionOutcome::Abandon,
            execution_plan_spec: ExecutionPlanSpec {
                intent_spec_id: "intent-2".into(),
                revision: 1,
                parent_revision: None,
                replan_reason: None,
                tasks: vec![gate_task.clone()],
                max_parallel: 1,
                checkpoints: vec![],
            },
            plan_revision_specs: vec![],
            run_state: RunStateSnapshot::default(),
            task_results: vec![TaskResult {
                task_id: gate_task.id(),
                status: TaskNodeStatus::Failed,
                gate_report: Some(GateReport {
                    results: vec![GateResult {
                        check_id: "clippy".into(),
                        kind: "command".into(),
                        passed: false,
                        exit_code: 101,
                        stdout: String::new(),
                        stderr: "lint failed".into(),
                        duration_ms: 900,
                        timed_out: false,
                    }],
                    all_required_passed: false,
                }),
                agent_output: None,
                retry_count: 0,
                tool_calls: vec![],
                policy_violations: vec![],
                review: None,
            }],
            system_report: SystemReport {
                integration: GateReport::empty(),
                security: GateReport::empty(),
                release: GateReport::empty(),
                review_passed: true,
                review_findings: vec![],
                artifacts_complete: true,
                missing_artifacts: vec![],
                overall_passed: false,
            },
            intent_spec_id: "intent-2".into(),
            lifecycle_change_log: vec![],
            replan_count: 0,
            persistence: None,
        };

        let rendered = format_orchestrator_result(&result);

        assert!(rendered.contains("Reason: clippy (exit 101: lint failed)"));
    }
}
