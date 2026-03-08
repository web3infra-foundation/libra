use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::thread::ThreadId;

/// Current scheduler view for one thread.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SchedulerState {
    pub thread_id: ThreadId,
    pub selected_plan_id: Option<Uuid>,
    #[serde(default)]
    pub current_plan_heads: Vec<PlanHeadRef>,
    pub active_task_id: Option<Uuid>,
    pub active_run_id: Option<Uuid>,
    #[serde(default)]
    pub live_context_window: Vec<LiveContextFrameRef>,
    pub metadata: Option<Value>,
    pub updated_at: DateTime<Utc>,
    pub version: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanHeadRef {
    pub plan_id: Uuid,
    pub ordinal: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LiveContextFrameRef {
    pub context_frame_id: Uuid,
    pub position: i64,
    pub source_kind: LiveContextSourceKind,
    pub pin_kind: Option<LiveContextPinKind>,
    pub inserted_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LiveContextSourceKind {
    IntentAnalysis,
    Planning,
    Execution,
    Validation,
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LiveContextPinKind {
    Seed,
    Checkpoint,
    Manual,
    System,
}
