use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::types::{FileChange, PatchStatus};

// ========================= Snapshots =========================

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStepSnapshot {
    pub id: String,
    pub plan_id: String,
    pub text: String,
    #[serde(default)]
    pub ordinal: i64,
    pub created_at: DateTime<Utc>,
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSnapshot {
    pub id: String,
    pub thread_id: String,
    pub plan_id: Option<String>,
    pub task_id: Option<String>,
    pub started_at: DateTime<Utc>,
}

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

fn default_patchset_snapshot_status() -> PatchStatus {
    PatchStatus::Pending
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSnapshot {
    pub id: String,
    pub thread_id: String,
    pub run_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub data: serde_json::Value,
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentEvent {
    pub id: String,
    pub intent_id: String,
    pub status: String,
    pub at: DateTime<Utc>,
    pub next_intent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskEvent {
    pub id: String,
    pub task_id: String,
    pub status: String,
    pub at: DateTime<Utc>,
    pub run_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunEvent {
    pub id: String,
    pub run_id: String,
    pub status: String,
    pub at: DateTime<Utc>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStepEvent {
    pub id: String,
    pub plan_id: String,
    pub step_id: String,
    pub status: String,
    pub at: DateTime<Utc>,
    pub run_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunUsage {
    pub run_id: String,
    pub thread_id: String,
    pub at: DateTime<Utc>,
    pub usage: serde_json::Value,
}

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
