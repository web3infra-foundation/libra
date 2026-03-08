use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Denormalized index row for resolving intent -> plan relationships.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IntentPlanIndexRow {
    pub intent_id: Uuid,
    pub plan_id: Uuid,
    pub created_at: DateTime<Utc>,
}

/// Denormalized index row for resolving intent -> task relationships.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IntentTaskIndexRow {
    pub intent_id: Uuid,
    pub task_id: Uuid,
    pub parent_task_id: Option<Uuid>,
    pub origin_step_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

/// Denormalized index row for resolving plan step -> task relationships.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanStepTaskIndexRow {
    pub plan_id: Uuid,
    pub step_id: Uuid,
    pub task_id: Uuid,
    pub created_at: DateTime<Utc>,
}

/// Denormalized index row for resolving task -> run relationships.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskRunIndexRow {
    pub task_id: Uuid,
    pub run_id: Uuid,
    pub is_latest: bool,
    pub created_at: DateTime<Utc>,
}

/// Denormalized index row for resolving run -> event relationships.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunEventIndexRow {
    pub run_id: Uuid,
    pub event_id: Uuid,
    pub event_kind: String,
    pub is_latest: bool,
    pub created_at: DateTime<Utc>,
}

/// Denormalized index row for resolving run -> patchset relationships.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunPatchSetIndexRow {
    pub run_id: Uuid,
    pub patchset_id: Uuid,
    pub sequence: i64,
    pub is_latest: bool,
    pub created_at: DateTime<Utc>,
}

/// Denormalized index row for resolving intent -> context frame relationships.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IntentContextFrameIndexRow {
    pub intent_id: Uuid,
    pub context_frame_id: Uuid,
    pub relation_kind: String,
    pub created_at: DateTime<Utc>,
}
