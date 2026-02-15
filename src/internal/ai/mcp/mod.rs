//! MCP tools and resources implementation.
//!
//! # Tools Interface
//!
//! | Tool Name | Description |
//! |-----------|-------------|
//! | `create_task` | Create a new AI Task (Goal/Intent). Input: `CreateTaskParams`. Output: Task ID. |
//! | `list_tasks` | List recent Tasks. Input: `ListTasksParams`. Output: List of task summaries. |
//! | `create_run` | Create a new Run for a Task. Input: `CreateRunParams`. Output: Run ID. |
//! | `list_runs` | List recent Runs. Input: `ListRunsParams`. Output: List of run summaries. |
//! | `create_context_snapshot` | Create a Context Snapshot. Input: `CreateContextSnapshotParams`. Output: Snapshot ID. |
//! | `list_context_snapshots` | List recent Snapshots. Input: `ListContextSnapshotsParams`. Output: List of summaries. |
//! | `create_plan` | Create a Plan for a Run. Input: `CreatePlanParams`. Output: Plan ID. |
//! | `list_plans` | List recent Plans. Input: `ListPlansParams`. Output: List of plan summaries. |
//! | `create_patchset` | Create a PatchSet (proposed changes). Input: `CreatePatchSetParams`. Output: PatchSet ID. |
//! | `list_patchsets` | List recent PatchSets. Input: `ListPatchSetsParams`. Output: List of summaries. |
//! | `create_evidence` | Create Evidence (test/lint results). Input: `CreateEvidenceParams`. Output: Evidence ID. |
//! | `list_evidences` | List recent Evidence. Input: `ListEvidencesParams`. Output: List of summaries. |
//! | `create_tool_invocation` | Record a Tool Invocation. Input: `CreateToolInvocationParams`. Output: Invocation ID. |
//! | `list_tool_invocations` | List recent Tool Invocations. Input: `ListToolInvocationsParams`. Output: List of summaries. |
//! | `create_provenance` | Record Model Provenance. Input: `CreateProvenanceParams`. Output: Provenance ID. |
//! | `list_provenances` | List recent Provenance records. Input: `ListProvenancesParams`. Output: List of summaries. |
//! | `create_decision` | Record a Decision (Commit/Checkpoint). Input: `CreateDecisionParams`. Output: Decision ID. |
//! | `list_decisions` | List recent Decisions. Input: `ListDecisionsParams`. Output: List of summaries. |
//! | `create_intent` | Create a new Intent (Prompt). Input: `CreateIntentParams`. Output: Intent ID. |
//! | `list_intents` | List recent Intents. Input: `ListIntentsParams`. Output: List of intent summaries. |
//!
//! See `resource.rs` for detailed parameter structures.

pub mod resource;
pub mod server;
#[cfg(test)]
mod tests;
