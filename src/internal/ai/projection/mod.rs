//! Libra-side runtime projections over immutable AI history.
//!
//! These types model the mutable operational view described in
//! `docs/agent/agent.md` and `docs/agent/agent-workflow.md`.

pub mod index;
pub mod scheduler;
pub mod thread;

pub use index::{
    IntentContextFrameIndexRow, IntentPlanIndexRow, IntentTaskIndexRow, PlanStepTaskIndexRow,
    RunEventIndexRow, RunPatchSetIndexRow, TaskRunIndexRow,
};
pub use scheduler::{
    LiveContextFrameRef, LiveContextPinKind, LiveContextSourceKind, PlanHeadRef, SchedulerState,
};
pub use thread::{
    ThreadId, ThreadIntentLinkReason, ThreadIntentRef, ThreadParticipant, ThreadParticipantRole,
    ThreadProjection,
};
