//! Application-level events used to coordinate UI actions.
//!
//! `AppEvent` is the internal message bus between UI components and the top-level `App` loop.
//! Widgets emit events to request actions that must be handled at the app layer.

use serde_json::Value;

use super::history_cell::HistoryCell;
use crate::internal::ai::{
    completion::Message,
    tools::{ToolOutput, context::UserInputRequest},
};

/// Events emitted by agent execution to notify the UI.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// Streaming text delta from the model.
    TextDelta { delta: String },
    /// Complete response text from the model.
    ResponseComplete {
        text: String,
        new_history: Vec<Message>,
    },
    /// Error during agent execution.
    Error { message: String },
}

/// Current status of the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentStatus {
    /// Agent is idle, waiting for input.
    #[default]
    Idle,
    /// Agent is thinking/processing.
    Thinking,
    /// Agent is executing a tool.
    ExecutingTool,
    /// Agent is waiting for user input (via `request_user_input` tool).
    AwaitingUserInput,
}

/// The exit strategy requested by the UI layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitMode {
    /// Shutdown core and exit after completion.
    ShutdownFirst,
    /// Exit the UI loop immediately.
    Immediate,
}

/// Application-level events.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum AppEvent {
    /// Event from the agent execution.
    AgentEvent(AgentEvent),
    /// Request to exit the application.
    Exit(ExitMode),
    /// Submit a user message.
    SubmitUserMessage {
        text: String,
        /// If set, restrict tools for this message (agent tool restriction).
        allowed_tools: Option<Vec<String>>,
    },
    /// Insert a history cell into the chat.
    InsertHistoryCell(Box<dyn HistoryCell>),
    /// Tool call is starting.
    ToolCallBegin {
        call_id: String,
        tool_name: String,
        arguments: Value,
    },
    /// Tool call has completed.
    ToolCallEnd {
        call_id: String,
        tool_name: String,
        result: Result<ToolOutput, String>,
    },
    /// Agent status has changed.
    AgentStatusUpdate { status: AgentStatus },
    /// The agent is requesting user input via the `request_user_input` tool.
    RequestUserInput { request: UserInputRequest },
}
