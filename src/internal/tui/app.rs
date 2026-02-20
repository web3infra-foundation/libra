//! Main application structure and event loop.
//!
//! The `App` struct manages the TUI state and coordinates between
//! user input, agent execution, and UI rendering.

use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Instant};
use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Instant};

use crossterm::event::{KeyCode, KeyModifiers};
use crossterm::event::{KeyCode, KeyModifiers};
use tokio::{
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    task::JoinHandle,
    time::sleep,
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
    diff::FileChange,
    history_cell::{
        AssistantHistoryCell, DiffHistoryCell, PlanUpdateHistoryCell, ToolCallHistoryCell,
        UserHistoryCell,
    },
    terminal::{TARGET_FRAME_INTERVAL, Tui, TuiEvent},
};
use crate::internal::ai::{
    agent::{ToolLoopConfig, run_tool_loop_with_history_and_observer},
    agents::AgentRouter,
    commands::CommandDispatcher,
    completion::{CompletionModel, Message},
    mcp::{
        resource::{
            CreateContextSnapshotParams, CreateDecisionParams, CreateRunParams,
            CreateToolInvocationParams,
        },
        server::LibraMcpServer,
    },
    session::{SessionState, SessionStore},
    tools::{
        ToolOutput, ToolRegistry,
        context::{UpdatePlanArgs, UserInputAnswer, UserInputRequest, UserInputResponse},
    },
};

/// MCP resource IDs for tracking the workflow
#[derive(Debug, Clone, Default)]
pub struct McpIds {
    pub _intent_id: Option<String>,
    pub task_id: Option<String>,
    pub run_id: Option<String>,
    pub _context_snapshot_id: Option<String>,
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

/// Configuration for creating an App.
pub struct AppConfig {
    pub welcome_message: String,
    pub command_dispatcher: CommandDispatcher,
    pub agent_router: AgentRouter,
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

/// Configuration for creating an App.
pub struct AppConfig {
    pub welcome_message: String,
    pub command_dispatcher: CommandDispatcher,
    pub agent_router: AgentRouter,
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
pub struct App<M: CompletionModel> {
    /// The TUI instance.
    tui: Tui,
    /// The chat widget.
    widget: ChatWidget,
    /// The completion model used by the agent loop.
    model: M,
    model: M,
    /// The tool registry.
    registry: Arc<ToolRegistry>,
    /// Tool loop runtime config.
    config: ToolLoopConfig,
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
    agent_router: AgentRouter,
    /// Session state for persistence.
    session: SessionState,
    /// Session store for saving/loading.
    session_store: SessionStore,
    /// Receiver for user-input requests from the `request_user_input` tool handler.
    user_input_rx: UnboundedReceiver<UserInputRequest>,
    /// Currently pending user-input interaction, if any.
    pending_user_input: Option<PendingUserInput>,
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
impl<M: CompletionModel + Clone + 'static> App<M> {
    /// Create a new App instance.
    pub fn new(
        tui: Tui,
        model: M,
        model: M,
        registry: Arc<ToolRegistry>,
        config: ToolLoopConfig,
        app_config: AppConfig,
    ) -> Self {
        let (app_event_tx, app_event_rx) = mpsc::unbounded_channel();
        let history = app_config.session.to_history();
        let history = app_config.session.to_history();
        Self {
            tui,
            widget: ChatWidget::new(),
            model,
            registry,
            config,
            history,
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

                // Handle user-input requests from the tool handler
                Some(request) = self.user_input_rx.recv() => {
                    self.handle_user_input_request(request);
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
                        eprintln!("Failed to resolve actor for decision: {:?}", e);
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
                            eprintln!("Failed to create decision: {:?}", result.content);
                        }
                    }
                    Err(e) => {
                        eprintln!("Error creating decision: {:?}", e);
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
                if key.kind == crossterm::event::KeyEventKind::Press {
                    self.handle_key_event(key).await?;
                }
            }
            TuiEvent::Paste(text) => {
                for c in text.chars() {
                    self.widget.bottom_pane.insert_char(c);
                }
                self.widget.bottom_pane.sync_command_popup();
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
            self.cancel_pending_user_input();
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
                KeyCode::Down if self.widget.bottom_pane.is_command_popup_visible() => {
                    self.widget.bottom_pane.command_popup_down();
                    self.schedule_draw();
                }
                KeyCode::Esc if self.widget.bottom_pane.is_command_popup_visible() => {
                    self.widget.bottom_pane.dismiss_command_popup();
                    self.schedule_draw();
                }
                // ── Normal idle handlers ─────────────────────────────
                }
                // ── Normal idle handlers ─────────────────────────────
                KeyCode::Enter => {
                    if !self.widget.bottom_pane.is_empty() {
                        let text = self.widget.bottom_pane.take_input();
                        self.submit_message(text);
                    }
                }
                // Clear screen (Ctrl+K) - must come before generic Char handler
                KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.widget.clear();
                    self.widget.bottom_pane.clear();
                    self.widget.bottom_pane.sync_command_popup();
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
                    self.widget.bottom_pane.sync_command_popup();
                    self.schedule_draw();
                }
                KeyCode::Backspace => {
                    self.widget.bottom_pane.backspace();
                    self.widget.bottom_pane.sync_command_popup();
                    self.widget.bottom_pane.sync_command_popup();
                    self.schedule_draw();
                }
                KeyCode::Delete => {
                    self.widget.bottom_pane.delete();
                    self.widget.bottom_pane.sync_command_popup();
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
            AgentStatus::AwaitingUserInput => {
                self.handle_user_input_key(key);
            }
            AgentStatus::Thinking | AgentStatus::ExecutingTool => {
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
                                eprintln!("Failed to resolve actor for run: {:?}", e);
                                return;
                            }
                        };

                        // Call MCP interface to create run
                        match mcp_server_clone.create_run_impl(run_params, actor).await {
                            Ok(result) => {
                                if result.is_error.unwrap_or(false) {
                                    eprintln!("Failed to create run: {:?}", result.content);
                                }
                            }
                            Err(e) => {
                                eprintln!("Error creating run: {:?}", e);
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
                                eprintln!("Failed to resolve actor for snapshot: {:?}", e);
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
                                    eprintln!(
                                        "Failed to create context snapshot: {:?}",
                                        result.content
                                    );
                                }
                            }
                            Err(e) => {
                                eprintln!("Error creating context snapshot: {:?}", e);
                            }
                        }
                    });
                }

                // Prepare components for background task
                let model = self.model.clone();
                let registry = self.registry.clone();
                let mut config = self.config.clone();
                config.allowed_tools = allowed_tools;
                let mut config = self.config.clone();
                config.allowed_tools = allowed_tools;
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
                                            eprintln!(
                                                "Failed to resolve actor for tool invocation: {:?}",
                                                e
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
                                            if !result.is_error.unwrap_or(false) {
                                            } else {
                                                eprintln!(
                                                    "Failed to record tool invocation: {:?}",
                                                    result.content
                                                );
                                            }
                                        }
                                        Err(e) => {
                                            eprintln!("Error recording tool invocation: {:?}", e);
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
                }
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
            AppEvent::RequestUserInput { request } => {
                self.handle_user_input_request(request);
            }
        }

        Ok(false)
    }

    /// Submit a user message, expanding slash commands and applying agent context.
    /// Submit a user message, expanding slash commands and applying agent context.
    fn submit_message(&mut self, text: String) {
        // 1. Check for built-in TUI commands first.
        if let Some((cmd, _args)) = super::slash_command::parse_builtin(&text) {
            self.handle_builtin_command(cmd);
        // 1. Check for built-in TUI commands first.
        if let Some((cmd, _args)) = super::slash_command::parse_builtin(&text) {
            self.handle_builtin_command(cmd);
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
    fn handle_builtin_command(&mut self, cmd: super::slash_command::BuiltinCommand) {
        use super::slash_command::BuiltinCommand;
        match cmd {
            BuiltinCommand::Help => {
                let mut lines = String::from("Available commands:\n");
                // Built-in commands
                for b in BuiltinCommand::all() {
                    lines.push_str(&format!("  /{:<14} {}\n", b.name(), b.description()));

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
    fn handle_builtin_command(&mut self, cmd: super::slash_command::BuiltinCommand) {
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
            BuiltinCommand::Quit => {
                self.should_exit = true;
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
            BuiltinCommand::Quit => {
                self.should_exit = true;
                self.exit_info = Some(AppExitInfo {
                    reason: ExitReason::UserRequested,
                });
            }
        }
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
