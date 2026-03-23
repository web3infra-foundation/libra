use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{
    model::{
        ContextFrameEvent, ContextSnapshot, DecisionEvent, EvidenceEvent, IntentEvent,
        IntentSnapshot, PatchSetSnapshot, PlanSnapshot, PlanStepEvent, PlanStepSnapshot,
        ProvenanceSnapshot, RunEvent, RunSnapshot, RunUsage, TaskEvent, TaskSnapshot,
    },
    types::ToolInvocation,
};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ThreadView {
    pub thread_id: String,
    pub intents: HashMap<String, IntentSnapshot>,
    pub intent_heads: Vec<String>,
    pub plans: HashMap<String, PlanSnapshot>,
    pub plan_steps: HashMap<String, PlanStepSnapshot>,
    pub tasks: HashMap<String, TaskSnapshot>,
    pub runs: HashMap<String, RunSnapshot>,
    pub patchsets: HashMap<String, PatchSetSnapshot>,
    pub context_snapshots: HashMap<String, ContextSnapshot>,
    pub provenance: HashMap<String, ProvenanceSnapshot>,
    pub current_intent_id: Option<String>,
    pub latest_intent_id: Option<String>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SchedulerView {
    pub ready_queue: Vec<String>,
    pub selected_plan_id: Option<String>,
    pub current_plan_heads: Vec<String>,
    pub active_task_id: Option<String>,
    pub active_run_id: Option<String>,
    pub active_plan_step_id: Option<String>,
    pub live_context_window: Vec<String>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QueryIndex {
    pub plan_task_ids: HashMap<String, Vec<String>>,
    pub task_run_ids: HashMap<String, Vec<String>>,
    pub task_latest_run_id: HashMap<String, String>,
    pub run_latest_patchset_id: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ViewRebuildResult {
    pub thread: ThreadView,
    pub scheduler: SchedulerView,
    pub index: QueryIndex,
    pub tool_invocations: Vec<ToolInvocation>,
    pub intent_events: Vec<IntentEvent>,
    pub task_events: Vec<TaskEvent>,
    pub run_events: Vec<RunEvent>,
    pub run_usage: Vec<RunUsage>,
    pub plan_step_events: Vec<PlanStepEvent>,
    pub evidence: Vec<EvidenceEvent>,
    pub decisions: Vec<DecisionEvent>,
    pub context_frames: Vec<ContextFrameEvent>,
}
