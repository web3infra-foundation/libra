//! Application-level events used to coordinate UI actions.
//!
//! [`AppEvent`] is the internal message bus between UI components and the top-level
//! [`super::app::App`] loop. Widgets emit events to request actions that must be
//! handled at the app layer (mutating shared state, dispatching to the agent, or
//! triggering screen transitions). Most variants carry a [`TurnId`] so that stale
//! events from a cancelled or superseded turn can be filtered out cleanly.
//!
//! The event types live in two layers:
//! - [`AgentEvent`] represents low-level agent execution progress (tokens, errors,
//!   retries) — pushed by the agent runtime and forwarded to the UI.
//! - [`AppEvent`] is the larger UI-side bus that wraps `AgentEvent` plus
//!   transcript inserts, tool-call lifecycle markers, DAG progress, and
//!   confirmation prompts that need a `oneshot` reply channel.

use serde_json::Value;
use tokio::sync::oneshot;
use uuid::Uuid;

use super::history_cell::HistoryCell;
use crate::internal::ai::{
    agent::TaskIntent,
    completion::{CompletionUsageSummary, Message},
    intentspec::types::IntentSpec,
    orchestrator::types::{
        ExecutionPlanSpec, OrchestratorResult, PersistedPlanReviewBundle,
        PhaseConfirmationDecision, PhaseConfirmationPrompt, TaskNodeStatus, TaskRuntimeEvent,
    },
    tools::ToolOutput,
};

/// Logical turn identifier for isolating async event streams.
///
/// Every user submit increments this counter. Late events arriving from a turn
/// that has already been superseded (e.g. user pressed Esc and submitted a new
/// message) are dropped by comparing their `turn_id` against the current turn,
/// keeping the transcript free of stale tool-call results or retries.
pub type TurnId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnInputSource {
    Local,
    Automation,
}

/// Events emitted by agent execution to notify the UI.
///
/// These are produced by the agent runtime and forwarded into [`AppEvent::AgentEvent`].
/// The variants intentionally cover both "completion-style" (one final response) and
/// "streaming-style" (incremental deltas) providers because the UI must handle both
/// without branching on provider type.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// Complete response text from the model along with the updated message
    /// history that should be persisted into the session.
    ResponseComplete {
        text: String,
        new_history: Vec<Message>,
    },
    /// Managed provider produced a streamed delta for the current response.
    /// The TUI accumulates these into the in-flight assistant cell.
    ResponseDelta { delta: String },
    /// Provider produced a streamed thinking/reasoning delta. Surfaced to
    /// developers for visibility; not all providers emit this.
    ThinkingDelta { delta: String },
    /// Managed provider completed a turn and returned follow-up session context.
    /// The `provider_session_id` is needed to chain subsequent turns on the same
    /// managed runtime.
    ManagedResponseComplete {
        text: String,
        provider_session_id: String,
    },
    /// Fatal error during agent execution; rendered as a red error cell.
    Error { message: String },
    /// The underlying model request is being retried after a transient failure.
    /// Used to drive the [`AgentStatus::Retrying`] indicator and inform the user
    /// without filling the transcript with noise.
    Retrying {
        attempt: u32,
        total_attempts: u32,
        delay_ms: u64,
        error: String,
    },
    /// Provider usage for one completed model request.
    UsageUpdated {
        usage: CompletionUsageSummary,
        wall_clock_ms: u64,
    },
}

/// Current status of the agent.
///
/// Used as a state-machine label that both renders (via the status indicator)
/// and gates input dispatch. For example, while in `AwaitingApproval` the
/// composer is intercepted to drive the approval popup instead of sending a
/// new message to the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentStatus {
    /// Agent is idle, waiting for input.
    #[default]
    Idle,
    /// Agent is thinking/processing a model response.
    Thinking,
    /// Agent is retrying a transient model request.
    Retrying,
    /// Agent is executing a tool.
    ExecutingTool,
    /// Agent is waiting for user input (via `request_user_input` tool).
    AwaitingUserInput,
    /// Agent is waiting for sandbox permission approval.
    AwaitingApproval,
    /// Waiting for user to choose post-plan action (Execute Plan / Modify Plan / Cancel).
    AwaitingPostPlanChoice,
    /// Waiting for user to choose the network policy for an approved plan.
    AwaitingNetworkPolicyChoice,
    /// Waiting for user to confirm, modify, or cancel a generated IntentSpec.
    AwaitingIntentReviewChoice,
}

/// Provider-submitted Phase 1 planning draft captured from `submit_plan_draft`.
///
/// Carries the natural-language explanation and the ordered step list so the
/// UI can render it as a checklist before the user approves execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderPlanDraft {
    /// Optional human-readable rationale rendered above the step list.
    pub explanation: Option<String>,
    /// Ordered step titles; rendered as bullet points with checkboxes.
    pub steps: Vec<ProviderPlanDraftStep>,
}

/// One ordered provider draft step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderPlanDraftStep {
    /// Single-line title for the step shown in the plan checklist.
    pub title: String,
}

/// Application-level events flowing through the TUI bus.
///
/// Every variant carries a [`TurnId`] so that out-of-order or stale events from
/// a cancelled turn can be filtered without losing the live turn's progress.
/// The enum is intentionally large and `clippy::large_enum_variant` is silenced
/// because the variants represent rare, distinct UI transitions that benefit
/// from being a single dispatch type rather than a hierarchy of channels.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum AppEvent {
    /// Event from the agent execution. Wraps an [`AgentEvent`] together with
    /// the originating turn so the dispatcher can correlate it to the right
    /// in-flight cell.
    AgentEvent { turn_id: TurnId, event: AgentEvent },
    /// Submit a user message to the agent. Emitted by the composer on Enter.
    SubmitUserMessage {
        turn_id: TurnId,
        text: String,
        source: TurnInputSource,
        /// If set, restrict tools for this message (agent tool restriction).
        allowed_tools: Option<Vec<String>>,
    },
    /// First-turn model classification resolved the task intent. The TUI stores
    /// the updated base prompt and direct-chat tool policy so later turns stay
    /// aligned with the initial request.
    TaskIntentClassified {
        turn_id: TurnId,
        intent: TaskIntent,
        preamble: String,
        allowed_tools: Vec<String>,
    },
    /// Complete result for a `/plan` workflow run. Carries the persisted bundle
    /// plus the in-memory spec/plan so the UI can transition to the post-plan
    /// review gate without another round-trip to disk.
    PlanWorkflowComplete {
        turn_id: TurnId,
        text: String,
        llm_output: Option<String>,
        new_history: Vec<Message>,
        intent_id: Option<String>,
        plan_id: Option<String>,
        persisted_plan_bundle: Option<PersistedPlanReviewBundle>,
        spec_json: String,
        spec: Box<IntentSpec>,
        plan: Box<ExecutionPlanSpec>,
        plan_draft: ProviderPlanDraft,
        warnings: Vec<String>,
        automatic_repair_attempts: u8,
        automatic_repair_max_attempts: u8,
    },
    /// Complete result for the Phase 0 IntentSpec review gate. Triggers the
    /// confirm / modify / cancel choice popup.
    IntentSpecReviewReady {
        turn_id: TurnId,
        text: String,
        llm_output: Option<String>,
        new_history: Vec<Message>,
        intent_id: Option<String>,
        spec_json: String,
        warnings: Vec<String>,
    },
    /// Insert a history cell into the chat transcript. Cells are boxed because
    /// `HistoryCell` is a trait object whose concrete size varies by cell type.
    InsertHistoryCell {
        turn_id: TurnId,
        cell: Box<dyn HistoryCell>,
    },
    /// Insert a simple managed-provider info note into the transcript.
    ManagedInfoNote { turn_id: TurnId, message: String },
    /// Tool call is starting. The dispatcher creates a tool-call cell and
    /// places it in a "running" state until [`Self::ToolCallEnd`] arrives.
    ToolCallBegin {
        turn_id: TurnId,
        call_id: String,
        tool_name: String,
        arguments: Value,
    },
    /// Tool call appeared in a streamed model chunk but is not executing yet.
    /// Used for live preview of arguments while the model is still thinking.
    ToolCallPreview {
        turn_id: TurnId,
        call_id: String,
        tool_name: String,
        arguments: Value,
    },
    /// Tool call has completed. The matching cell is updated in-place using
    /// `call_id` so output appears under the same header that begin printed.
    ToolCallEnd {
        turn_id: TurnId,
        call_id: String,
        tool_name: String,
        result: Result<ToolOutput, String>,
    },
    /// Task-scoped runtime progress for the workflow mux. Routed to the
    /// per-task cell identified by `task_id`.
    TaskRuntimeEvent {
        turn_id: TurnId,
        task_id: Uuid,
        event: TaskRuntimeEvent,
    },
    /// Agent status has changed. Drives the spinner/elapsed-time indicator
    /// and gates which input handlers are active.
    AgentStatusUpdate {
        turn_id: TurnId,
        status: AgentStatus,
    },
    /// Orchestrator is waiting for user confirmation before entering a gated
    /// phase. The reply is sent back via `response_tx` — exactly once — so
    /// failing to respond would deadlock the orchestrator.
    PhaseConfirmationRequired {
        turn_id: TurnId,
        prompt: PhaseConfirmationPrompt,
        response_tx: oneshot::Sender<PhaseConfirmationDecision>,
    },
    /// MCP turn-tracking IDs became available for this turn. Used by the MCP
    /// integration tests to correlate events across processes.
    McpTurnTrackingReady {
        turn_id: TurnId,
        run_id: Option<String>,
    },
    /// A compiled execution plan should be shown as a DAG in the transcript.
    DagGraphBegin {
        turn_id: TurnId,
        plan: ExecutionPlanSpec,
    },
    /// A task node inside the DAG changed status (running, completed, failed).
    DagTaskStatus {
        turn_id: TurnId,
        task_id: Uuid,
        status: TaskNodeStatus,
    },
    /// DAG execution progress changed. Updates the `n/m` counter shown above
    /// the DAG.
    DagGraphProgress {
        turn_id: TurnId,
        completed: usize,
        total: usize,
    },
    /// System validation finished and should update the DAG terminal validation row.
    DagValidationStatus { turn_id: TurnId, passed: bool },
    /// Final release decision finished and should update the DAG terminal release row.
    DagReleaseStatus { turn_id: TurnId, passed: bool },
    /// The task mux should leave focus mode while keeping the workflow DAG visible.
    DagTaskMuxClear { turn_id: TurnId },
    /// Orchestrator workflow completed. Carries the full result bundle so the
    /// UI can render summaries, warnings, and any generated artifacts.
    ExecuteWorkflowComplete {
        turn_id: TurnId,
        text: String,
        new_history: Vec<Message>,
        result: Option<Box<OrchestratorResult>>,
        spec_json: String,
        intent_id: Option<String>,
        plan_draft: ProviderPlanDraft,
        warnings: Vec<String>,
        network_access: bool,
        automatic_repair_attempts: u8,
        automatic_repair_max_attempts: u8,
    },
}

impl AppEvent {
    /// Return the [`TurnId`] embedded in this event.
    ///
    /// Functional scope: enables the dispatcher to short-circuit events whose
    /// turn has been superseded (e.g. user pressed Esc and started a new turn)
    /// without exhaustively matching every variant at the call site.
    ///
    /// Boundary conditions: every variant must contribute a turn id; the
    /// `match` is intentionally exhaustive so adding a new variant without a
    /// `turn_id` field becomes a compile error.
    ///
    /// See: [`tests::turn_id_is_exposed_for_turn_scoped_events`].
    pub fn turn_id(&self) -> TurnId {
        match self {
            AppEvent::AgentEvent { turn_id, .. }
            | AppEvent::SubmitUserMessage { turn_id, .. }
            | AppEvent::TaskIntentClassified { turn_id, .. }
            | AppEvent::PlanWorkflowComplete { turn_id, .. }
            | AppEvent::IntentSpecReviewReady { turn_id, .. }
            | AppEvent::InsertHistoryCell { turn_id, .. }
            | AppEvent::ManagedInfoNote { turn_id, .. }
            | AppEvent::ToolCallBegin { turn_id, .. }
            | AppEvent::ToolCallPreview { turn_id, .. }
            | AppEvent::ToolCallEnd { turn_id, .. }
            | AppEvent::TaskRuntimeEvent { turn_id, .. }
            | AppEvent::AgentStatusUpdate { turn_id, .. }
            | AppEvent::PhaseConfirmationRequired { turn_id, .. }
            | AppEvent::McpTurnTrackingReady { turn_id, .. }
            | AppEvent::DagGraphBegin { turn_id, .. }
            | AppEvent::DagTaskStatus { turn_id, .. }
            | AppEvent::DagGraphProgress { turn_id, .. }
            | AppEvent::DagValidationStatus { turn_id, .. }
            | AppEvent::DagReleaseStatus { turn_id, .. }
            | AppEvent::DagTaskMuxClear { turn_id }
            | AppEvent::ExecuteWorkflowComplete { turn_id, .. } => *turn_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scenario: every event carries a turn id. Pin the public API so refactors
    /// that change the variant shape still expose the embedded turn for the
    /// dispatcher's stale-event filter.
    #[test]
    fn turn_id_is_exposed_for_turn_scoped_events() {
        let event = AppEvent::SubmitUserMessage {
            turn_id: 42,
            text: "hello".to_string(),
            source: TurnInputSource::Local,
            allowed_tools: None,
        };
        assert_eq!(event.turn_id(), 42);
    }
}
