//! `AgentRun[S]` snapshot: one sub-agent execution attempt for an `AgentTask`.

#![cfg(feature = "subagent-scaffold")]

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{AgentRunId, AgentTaskId};

/// Lifecycle status of an `AgentRun`. Five reachable states matching
/// CEX-S2-16 TUI agent pane requirements.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRunStatus {
    Queued,
    Running,
    Blocked,
    Completed,
    Failed,
}

/// One sub-agent execution attempt. Bound to a provider/model and an isolated
/// workspace at spawn time.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentRun {
    pub id: AgentRunId,

    pub task_id: AgentTaskId,

    /// Thread id from the parent Layer 1 session. Used for trace id chain
    /// `thread_id → agent_run_id → tool_call_id → source_call_id`.
    pub thread_id: Uuid,

    /// Provider slug (e.g. `"deepseek"`, `"ollama"`, `"anthropic"`). The
    /// runtime maps this to a real provider client at dispatch time.
    pub provider: String,

    /// Model id within the provider (e.g. `"deepseek-chat"`).
    pub model: String,

    /// Path on disk to the JSONL transcript for this run. Lives under
    /// `.libra/sessions/{thread_id}/agents/{run_id}.jsonl` per CEX-S2-10 (3).
    pub transcript_path: String,

    /// Path on disk to the isolated workspace (worktree / sparse / blocked /
    /// full-copy fallback). `None` until CEX-S2-11 materializes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_path: Option<String>,

    /// Current status. Mutated only by Runtime via append-only events.
    pub status: AgentRunStatus,
}
