//! Scheduler projection types for the Libra runtime layer.
//!
//! These projections capture mutable execution state derived from immutable
//! `Plan`, `Task`, `Run`, and `ContextFrame` history. They represent the
//! scheduler's current selection, active work, and live context window without
//! rewriting the underlying snapshot or event objects.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::thread::ThreadId;

/// Current scheduler view for one thread.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SchedulerState {
    /// Thread whose execution view this scheduler row belongs to.
    pub thread_id: ThreadId,
    /// Canonical Plan head currently selected for UI and execution decisions.
    pub selected_plan_id: Option<Uuid>,
    /// Active Plan leaves that still exist in the current planning frontier.
    #[serde(default)]
    pub current_plan_heads: Vec<PlanHeadRef>,
    /// Task currently emphasized by the scheduler or UI, if any.
    pub active_task_id: Option<Uuid>,
    /// Live Run attempt currently executing within the thread, if any.
    pub active_run_id: Option<Uuid>,
    /// Ordered visible context frames that form the live working set.
    #[serde(default)]
    pub live_context_window: Vec<LiveContextFrameRef>,
    /// Optional projection-only scheduler hints or implementation metadata.
    pub metadata: Option<Value>,
    /// Last time Libra updated the scheduler projection.
    pub updated_at: DateTime<Utc>,
    /// Projection revision maintained for scheduler updates.
    pub version: i64,
}

/// Reference to one currently active Plan head in the scheduler frontier.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanHeadRef {
    /// Plan snapshot that remains active in the current frontier.
    pub plan_id: Uuid,
    /// Stable order of the head within the projected frontier list.
    pub ordinal: i64,
}

/// One entry in the scheduler's live context window.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LiveContextFrameRef {
    /// ContextFrame event currently exposed to the active runtime window.
    pub context_frame_id: Uuid,
    /// Stable position of the frame within the visible window.
    pub position: i64,
    /// Phase or subsystem that introduced the frame into the window.
    pub source_kind: LiveContextSourceKind,
    /// Optional reason the frame is pinned instead of being freely evicted.
    pub pin_kind: Option<LiveContextPinKind>,
    /// Time at which the frame entered the projected live window.
    pub inserted_at: DateTime<Utc>,
}

/// Source category for a frame in the live context window.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LiveContextSourceKind {
    /// Frame came from Intent analysis during Phase 0.
    IntentAnalysis,
    /// Frame was added while building or revising a Plan.
    Planning,
    /// Frame was produced during task execution or tool use.
    Execution,
    /// Frame was added during validation, audit, or review work.
    Validation,
    /// Frame was inserted manually outside the automated workflow phases.
    Manual,
}

/// Pin reason for a live context frame that should remain visible.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LiveContextPinKind {
    /// Seed material that anchors the initial working context.
    Seed,
    /// Checkpoint material preserved across execution transitions.
    Checkpoint,
    /// Manual operator pin that should survive normal window churn.
    Manual,
    /// System-level pin reserved for mandatory runtime context.
    System,
}
