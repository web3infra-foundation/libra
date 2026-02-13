//! Main application structure and event loop.
//!
//! The `App` struct manages the TUI state and coordinates between
//! user input, agent execution, and UI rendering.

use std::{sync::Arc, time::Instant};

use crossterm::event::{KeyCode, KeyModifiers};
use tokio::{
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    task::JoinHandle,
};
use tokio_stream::StreamExt;

use super::{
    app_event::{AgentEvent, AgentStatus, AppEvent, ExitMode},
    chatwidget::ChatWidget,
    history_cell::{AssistantHistoryCell, ToolCallHistoryCell, UserHistoryCell},
    tui::{TARGET_FRAME_INTERVAL, Tui, TuiEvent},
};
use crate::internal::ai::{
    agent::{ToolLoopConfig, run_tool_loop_with_history_and_observer},
    completion::{CompletionModel, Message},
    tools::{ToolOutput, ToolRegistry},
};

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

/// The main application struct.
pub struct App<M: CompletionModel> {
    /// The TUI instance.
    tui: Tui,
    /// The chat widget.
    widget: ChatWidget,
    /// The completion model used by the agent loop.
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
    /// Initial welcome message.
    welcome_message: String,
}

impl<M: CompletionModel + Clone + 'static> App<M> {
    /// Create a new App instance.
    pub fn new(
        tui: Tui,
        model: M,
        registry: Arc<ToolRegistry>,
        config: ToolLoopConfig,
        welcome_message: String,
    ) -> Self {
        let (app_event_tx, app_event_rx) = mpsc::unbounded_channel();
        Self {
            tui,
            widget: ChatWidget::new(),
            model,
            registry,
            config,
            history: Vec::new(),
            app_event_rx,
            app_event_tx,
            should_exit: false,
            exit_info: None,
            last_draw_time: Instant::now(),
            agent_task: None,
            welcome_message,
        }
    }

    /// Run the main event loop.
    pub async fn run(&mut self) -> anyhow::Result<AppExitInfo> {
        // Enter alternate screen
        self.tui.enter_alt_screen()?;
        self.tui.clear()?;

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
            }
        }

        // Restore terminal
        self.tui.leave_alt_screen()?;

        Ok(self.exit_info.clone().unwrap_or(AppExitInfo {
            reason: ExitReason::UserRequested,
        }))
    }

    /// Handle a terminal event.
    async fn handle_tui_event(&mut self, event: TuiEvent) -> anyhow::Result<()> {
        match event {
            TuiEvent::Key(key) => {
                self.handle_key_event(key).await?;
            }
            TuiEvent::Paste(text) => {
                for c in text.chars() {
                    self.widget.bottom_pane.insert_char(c);
                }
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
                self.draw()?;
            }
        }
        Ok(())
    }

    /// Handle a key press event.
    async fn handle_key_event(&mut self, key: crossterm::event::KeyEvent) -> anyhow::Result<()> {
        // Check for Ctrl+C first (always handled)
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.exit_info = Some(AppExitInfo {
                reason: ExitReason::UserRequested,
            });
            self.should_exit = true;
            return Ok(());
        }

        // Handle input based on agent status
        match self.widget.bottom_pane.status {
            AgentStatus::Idle => match key.code {
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
                    self.schedule_draw();
                }
                KeyCode::Backspace => {
                    self.widget.bottom_pane.backspace();
                    self.schedule_draw();
                }
                KeyCode::Delete => {
                    self.widget.bottom_pane.delete();
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
            AppEvent::SubmitUserMessage { text } => {
                // Add user cell immediately
                self.widget
                    .add_cell(Box::new(UserHistoryCell::new(text.clone())));

                // Add streaming assistant placeholder (kept as the last cell).
                self.widget
                    .add_cell(Box::new(AssistantHistoryCell::streaming()));
                self.widget.bottom_pane.set_status(AgentStatus::Thinking);
                self.schedule_draw();

                // Prepare components for background task
                let model = self.model.clone();
                let registry = self.registry.clone();
                let config = self.config.clone();
                let history = self.history.clone();
                let tx = self.app_event_tx.clone();
                let user_text = text.clone();

                // Execute agent call in background task
                let handle = tokio::spawn(async move {
                    struct UiObserver {
                        tx: UnboundedSender<AppEvent>,
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

                    let mut observer = UiObserver { tx };
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
                let cell = Box::new(ToolCallHistoryCell::new(call_id, tool_name, arguments));
                self.insert_before_streaming_assistant(cell);
                self.widget
                    .bottom_pane
                    .set_status(AgentStatus::ExecutingTool);
                self.schedule_draw();
            }
            AppEvent::ToolCallEnd {
                call_id,
                tool_name: _,
                result,
            } => {
                // Update the tool call cell
                for cell in self.widget.cells.iter_mut().rev() {
                    if let Some(tool_cell) = cell.as_any_mut().downcast_mut::<ToolCallHistoryCell>()
                        && tool_cell.call_id == call_id
                        && tool_cell.is_running
                    {
                        tool_cell.complete(result);
                        break;
                    }
                }
                self.widget.bottom_pane.set_status(AgentStatus::Thinking);
                self.schedule_draw();
            }
            AppEvent::AgentStatusUpdate { status } => {
                self.widget.bottom_pane.set_status(status);
                self.schedule_draw();
            }
        }

        Ok(false)
    }

    /// Submit a user message.
    fn submit_message(&mut self, text: String) {
        let _ = self.app_event_tx.send(AppEvent::SubmitUserMessage { text });
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
            self.widget.cells.insert(index, cell);
        } else {
            self.widget.add_cell(cell);
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
        let now = Instant::now();
        if now.duration_since(self.last_draw_time) >= TARGET_FRAME_INTERVAL {
            let _ = self.tui.frame_requester().send(());
            self.last_draw_time = now;
        }
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
