//! `AgentTask[S]` snapshot: a Phase-2 dispatch unit derived from a confirmed
//! `Task`. References — does not copy — the persistent `Task` business fields.

#![cfg(feature = "subagent-scaffold")]

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{AgentRunId, AgentTaskId};

/// A sub-agent dispatch unit. Layer 1 generates one `AgentTask` per confirmed
/// `Task` it wants to delegate. The `AgentTask` is immutable once written;
/// further state lives on `AgentRun` (run.rs).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentTask {
    pub id: AgentTaskId,

    /// `thread_id` of the owning Layer 1 thread (stable across resume).
    pub thread_id: Uuid,

    /// Confirmed `Task` snapshot id this dispatch derives from.
    /// References `git_internal::internal::object::task::Task`.
    pub source_task_id: Uuid,

    /// Confirmed `Plan` snapshot id (the plan that contains the source task).
    pub source_plan_id: Uuid,

    /// Confirmed `IntentSpec` id, if available, for prompt context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_intent_id: Option<Uuid>,

    /// `agent_run_id` once the task has been picked up by an `AgentRun`.
    /// `None` while queued.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assigned_run: Option<AgentRunId>,
}
