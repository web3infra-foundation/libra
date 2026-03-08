//! MCP tools and resources implementation.
//!
//! # Tools Interface
//!
//! | Tool Name | Description |
//! |-----------|-------------|
//! | `create_intent` | Create an immutable Intent revision. Input: `CreateIntentParams`. Output: Intent ID. |
//! | `update_intent` | Append an Intent lifecycle event (analyzed/completed/cancelled). Input: `UpdateIntentParams`. |
//! | `list_intents` | List recent Intent revisions with event-derived status. |
//! | `create_task` | Create a new AI Task. Input: `CreateTaskParams`. Output: Task ID. |
//! | `list_tasks` | List recent Tasks with status reconstructed from `task_event`. |
//! | `create_run` | Create a new Run for a Task. Input: `CreateRunParams`. Output: Run ID. |
//! | `list_runs` | List recent Runs with status reconstructed from `run_event`. |
//! | `create_context_snapshot` | Create a Context Snapshot. Input: `CreateContextSnapshotParams`. Output: Snapshot ID. |
//! | `list_context_snapshots` | List recent Snapshots. Input: `ListContextSnapshotsParams`. Output: List of summaries. |
//! | `create_plan` | Create a Plan for an Intent revision. Input: `CreatePlanParams`. Output: Plan ID. |
//! | `list_plans` | List recent Plans. Input: `ListPlansParams`. Output: List of plan summaries. |
//! | `create_patchset` | Create a PatchSet candidate (sequence + touched files + artifact). |
//! | `list_patchsets` | List recent PatchSets. Input: `ListPatchSetsParams`. Output: List of summaries. |
//! | `create_evidence` | Create Evidence (test/lint results). Input: `CreateEvidenceParams`. Output: Evidence ID. |
//! | `list_evidences` | List recent Evidence. Input: `ListEvidencesParams`. Output: List of summaries. |
//! | `create_tool_invocation` | Record a Tool Invocation. Input: `CreateToolInvocationParams`. Output: Invocation ID. |
//! | `list_tool_invocations` | List recent Tool Invocations. Input: `ListToolInvocationsParams`. Output: List of summaries. |
//! | `create_provenance` | Record Model Provenance. Input: `CreateProvenanceParams`. Output: Provenance ID. |
//! | `list_provenances` | List recent Provenance records. Input: `ListProvenancesParams`. Output: List of summaries. |
//! | `create_decision` | Record a Decision (Commit/Checkpoint). Input: `CreateDecisionParams`. Output: Decision ID. |
//! | `list_decisions` | List recent Decisions. Input: `ListDecisionsParams`. Output: List of summaries. |
//! | `create_context_frame` | Create a ContextFrame (incremental context window entry). Input: `CreateContextFrameParams`. Output: Frame ID. |
//! | `list_context_frames` | List recent ContextFrames. Input: `ListContextFramesParams`. Output: List of summaries. |
//! | `create_plan_step_event` | Create a PlanStepEvent (step execution lifecycle event). Input: `CreatePlanStepEventParams`. Output: Event ID. |
//! | `list_plan_step_events` | List recent PlanStepEvents. Input: `ListPlanStepEventsParams`. Output: List of summaries. |
//! | `create_run_usage` | Record token/cost usage for a run. Input: `CreateRunUsageParams`. Output: Usage ID. |
//! | `list_run_usages` | List recent RunUsage records. Input: `ListRunUsagesParams`. Output: List of summaries. |
//!
//! See `resource.rs` for detailed parameter structures.

pub mod resource;
pub mod server;
#[cfg(test)]
mod tests;
