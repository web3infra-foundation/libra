//! Application-level events used to coordinate UI actions.
//!
//! `AppEvent` is the internal message bus between UI components and the top-level `App` loop.
//! Widgets emit events to request actions that must be handled at the app layer.

use serde_json::Value;
use uuid::Uuid;

use super::history_cell::HistoryCell;
use crate::internal::ai::{
    completion::Message,
    intentspec::types::IntentSpec,
    orchestrator::types::{ExecutionPlanSpec, OrchestratorResult, TaskNodeStatus},
    tools::ToolOutput,
};

/// Logical turn identifier for isolating async event streams.
pub type TurnId = u64;

/// Events emitted by agent execution to notify the UI.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// Complete response text from the model.
    ResponseComplete {
        text: String,
        new_history: Vec<Message>,
    },
    /// Managed provider produced a streamed delta for the current response.
    ResponseDelta { delta: String },
    /// Managed provider completed a turn and returned follow-up session context.
    ManagedResponseComplete {
        text: String,
        provider_session_id: String,
    },
    /// Error during agent execution.
    Error { message: String },
    /// The underlying model request is being retried after a transient failure.
    Retrying {
        attempt: u32,
        total_attempts: u32,
        delay_ms: u64,
        error: String,
    },
}

/// Current status of the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentStatus {
    /// Agent is idle, waiting for input.
    #[default]
    Idle,
    /// Agent is thinking/processing.
    Thinking,
    /// Agent is retrying a transient model request.
    Retrying,
    /// Agent is executing a tool.
    ExecutingTool,
    /// Agent is waiting for user input (via `request_user_input` tool).
    AwaitingUserInput,
    /// Agent is waiting for sandbox permission approval.
    AwaitingApproval,
    /// Waiting for user to choose post-plan action (Execute / Modify / Cancel).
    AwaitingPostPlanChoice,
}

/// Application-level events.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum AppEvent {
    /// Event from the agent execution.
    AgentEvent { turn_id: TurnId, event: AgentEvent },
    /// Submit a user message.
    SubmitUserMessage {
        turn_id: TurnId,
        text: String,
        /// If set, restrict tools for this message (agent tool restriction).
        allowed_tools: Option<Vec<String>>,
    },
    /// Complete result for a `/plan` workflow run.
    PlanWorkflowComplete {
        turn_id: TurnId,
        text: String,
        new_history: Vec<Message>,
        intent_id: Option<String>,
        plan_id: Option<String>,
        spec_json: String,
        spec: Box<IntentSpec>,
        plan: Box<ExecutionPlanSpec>,
        warnings: Vec<String>,
    },
    /// Insert a history cell into the chat.
    InsertHistoryCell {
        turn_id: TurnId,
        cell: Box<dyn HistoryCell>,
    },
    /// Insert a simple managed-provider info note into the transcript.
    ManagedInfoNote { turn_id: TurnId, message: String },
    /// Tool call is starting.
    ToolCallBegin {
        turn_id: TurnId,
        call_id: String,
        tool_name: String,
        arguments: Value,
    },
    /// Tool call has completed.
    ToolCallEnd {
        turn_id: TurnId,
        call_id: String,
        tool_name: String,
        result: Result<ToolOutput, String>,
    },
    /// Agent status has changed.
    AgentStatusUpdate {
        turn_id: TurnId,
        status: AgentStatus,
    },
    /// MCP turn-tracking IDs became available for this turn.
    McpTurnTrackingReady {
        turn_id: TurnId,
        run_id: Option<String>,
    },
    /// A compiled execution plan should be shown as a DAG in the transcript.
    DagGraphBegin {
        turn_id: TurnId,
        plan: ExecutionPlanSpec,
    },
    /// A task node inside the DAG changed status.
    DagTaskStatus {
        turn_id: TurnId,
        task_id: Uuid,
        status: TaskNodeStatus,
    },
    /// DAG execution progress changed.
    DagGraphProgress {
        turn_id: TurnId,
        completed: usize,
        total: usize,
    },
    /// Orchestrator workflow completed.
    ExecuteWorkflowComplete {
        turn_id: TurnId,
        text: String,
        new_history: Vec<Message>,
        result: Option<Box<OrchestratorResult>>,
    },
}

impl AppEvent {
    pub fn turn_id(&self) -> TurnId {
        match self {
            AppEvent::AgentEvent { turn_id, .. }
            | AppEvent::SubmitUserMessage { turn_id, .. }
            | AppEvent::PlanWorkflowComplete { turn_id, .. }
            | AppEvent::InsertHistoryCell { turn_id, .. }
            | AppEvent::ManagedInfoNote { turn_id, .. }
            | AppEvent::ToolCallBegin { turn_id, .. }
            | AppEvent::ToolCallEnd { turn_id, .. }
            | AppEvent::AgentStatusUpdate { turn_id, .. }
            | AppEvent::McpTurnTrackingReady { turn_id, .. }
            | AppEvent::DagGraphBegin { turn_id, .. }
            | AppEvent::DagTaskStatus { turn_id, .. }
            | AppEvent::DagGraphProgress { turn_id, .. }
            | AppEvent::ExecuteWorkflowComplete { turn_id, .. } => *turn_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_id_is_exposed_for_turn_scoped_events() {
        let event = AppEvent::SubmitUserMessage {
            turn_id: 42,
            text: "hello".to_string(),
            allowed_tools: None,
        };
        assert_eq!(event.turn_id(), 42);
    }
}
