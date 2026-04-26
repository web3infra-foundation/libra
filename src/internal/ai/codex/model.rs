//! Domain model for the Codex executor's persistence layer.
//!
//! Codex (the external `codex` binary that powers complex agent runs) emits two
//! categories of records:
//!
//! - **Snapshots** — point-in-time captures of an entity (`IntentSnapshot`,
//!   `PlanSnapshot`, `TaskSnapshot`, `RunSnapshot`, `PatchSetSnapshot`,
//!   `ContextSnapshot`, `ProvenanceSnapshot`).
//! - **Events** — state transitions or activity records keyed to an entity ID
//!   (`IntentEvent`, `TaskEvent`, `RunEvent`, `PlanStepEvent`, `RunUsage`,
//!   `ToolInvocationEvent`, `EvidenceEvent`, `DecisionEvent`, `ContextFrameEvent`).
//!
//! Both groups are pure DTOs with `serde` derives because Codex serializes them as
//! JSON over the WebSocket and persists them in SQLite. The `#[serde(default)]`
//! attributes mirror the Codex schema's optional fields so older payloads still
//! deserialize as new fields are added.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::types::{FileChange, PatchStatus};

// ========================= Snapshots =========================

/// A user-authored intent — the highest-level "goal" record. Plans and tasks descend
/// from an intent and link back via `intent_id` on subsequent snapshots.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentSnapshot {
    pub id: String,
    pub content: String,
    pub thread_id: String,
    #[serde(default)]
    pub parents: Vec<String>,
    #[serde(default)]
    pub analysis_context_frames: Vec<String>,
    pub created_at: DateTime<Utc>,
}

/// A plan: the ordered list of steps Codex intends to execute for an intent. Each
/// plan can be amended (new revisions linked through `parents`) so the agent can
/// branch exploration without losing prior context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanSnapshot {
    pub id: String,
    pub thread_id: String,
    pub intent_id: Option<String>,
    pub turn_id: Option<String>,
    pub step_text: String,
    #[serde(default)]
    pub parents: Vec<String>,
    #[serde(default)]
    pub context_frames: Vec<String>,
    pub created_at: DateTime<Utc>,
}

/// A single step within a plan. `ordinal` preserves order across reorderings; the
/// step text is what the user sees in the TUI plan widget.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStepSnapshot {
    pub id: String,
    pub plan_id: String,
    pub text: String,
    #[serde(default)]
    pub ordinal: i64,
    pub created_at: DateTime<Utc>,
}

/// A task — concrete unit of work spawned from a plan step or directly from the
/// thread. `parent_task_id` allows hierarchical decomposition; `dependencies` express
/// "must finish before me" ordering for parallel execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSnapshot {
    pub id: String,
    pub thread_id: String,
    pub plan_id: Option<String>,
    #[serde(default)]
    pub intent_id: Option<String>,
    pub turn_id: Option<String>,
    pub title: Option<String>,
    #[serde(default)]
    pub parent_task_id: Option<String>,
    #[serde(default)]
    pub origin_step_id: Option<String>,
    #[serde(default)]
    pub dependencies: Vec<String>,
    pub created_at: DateTime<Utc>,
}

/// A run — a single executor invocation. Multiple runs may exist per task (retries,
/// parallel exploration paths). The run owns the patch sets, tool calls, and usage
/// metrics that follow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSnapshot {
    pub id: String,
    pub thread_id: String,
    pub plan_id: Option<String>,
    pub task_id: Option<String>,
    pub started_at: DateTime<Utc>,
}

/// A bundle of file changes proposed by a run. The `status` field tracks approval
/// state (`pending` → `approved` / `rejected`) so the UI can render review cards.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchSetSnapshot {
    pub id: String,
    pub run_id: String,
    pub thread_id: String,
    pub created_at: DateTime<Utc>,
    #[serde(default = "default_patchset_snapshot_status")]
    pub status: PatchStatus,
    #[serde(default)]
    pub changes: Vec<FileChange>,
}

/// Default `status` for a freshly deserialized [`PatchSetSnapshot`] — used when a
/// legacy snapshot lacks the `status` field altogether.
fn default_patchset_snapshot_status() -> PatchStatus {
    PatchStatus::Pending
}

/// A captured slice of context (filesystem state, search results, etc.) that informed
/// a run. Stored opaquely as `serde_json::Value` because each capture kind has its
/// own schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSnapshot {
    pub id: String,
    pub thread_id: String,
    pub run_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub data: serde_json::Value,
}

/// Reproducibility metadata: which model, provider, and parameters produced a run.
/// Used by the audit log and to replay agent decisions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvenanceSnapshot {
    pub id: String,
    pub run_id: String,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub parameters: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

// ========================= Events =========================

/// State transition for an intent (e.g. "queued" → "running" → "completed"). The
/// optional `next_intent_id` chains intents when the agent forks the conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentEvent {
    pub id: String,
    pub intent_id: String,
    pub status: String,
    pub at: DateTime<Utc>,
    pub next_intent_id: Option<String>,
}

/// State transition for a task; `run_id` is set when the transition is caused by a
/// specific run (e.g. completion or failure of a run executing the task).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskEvent {
    pub id: String,
    pub task_id: String,
    pub status: String,
    pub at: DateTime<Utc>,
    pub run_id: Option<String>,
}

/// State transition for a run. `error` is `None` on success and contains the human-
/// readable failure reason otherwise.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunEvent {
    pub id: String,
    pub run_id: String,
    pub status: String,
    pub at: DateTime<Utc>,
    pub error: Option<String>,
}

/// Per-step lifecycle event within a plan execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStepEvent {
    pub id: String,
    pub plan_id: String,
    pub step_id: String,
    pub status: String,
    pub at: DateTime<Utc>,
    pub run_id: Option<String>,
}

/// Token / cost usage for a run, stored opaquely so each model provider can include
/// its own counters without schema migrations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunUsage {
    pub run_id: String,
    pub thread_id: String,
    pub at: DateTime<Utc>,
    pub usage: serde_json::Value,
}

/// Record of a tool call made inside a run. `server` is set for MCP-hosted tools and
/// `None` for built-in function tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInvocationEvent {
    pub id: String,
    pub run_id: String,
    pub thread_id: String,
    pub tool: String,
    pub server: Option<String>,
    pub status: String,
    pub at: DateTime<Utc>,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceEvent {
    pub id: String,
    pub run_id: String,
    #[serde(default)]
    pub patchset_id: Option<String>,
    pub at: DateTime<Utc>,
    pub kind: String,
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionEvent {
    pub id: String,
    pub run_id: String,
    pub chosen_patchset_id: Option<String>,
    pub approved: bool,
    pub at: DateTime<Utc>,
    pub rationale: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextFrameEvent {
    pub id: String,
    pub run_id: String,
    pub plan_id: Option<String>,
    pub step_id: Option<String>,
    pub at: DateTime<Utc>,
    pub delta: serde_json::Value,
}
