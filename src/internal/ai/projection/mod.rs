//! Libra-side runtime projections over immutable AI history.
//!
//! The types in this module implement the rebuildable Libra layer described in
//! `docs/ai/object-model.md` and `docs/ai/workflow.md`.
//! `git-internal` remains the source of truth for immutable snapshots and
//! append-only events, while these projections capture the current thread view,
//! scheduler view, and denormalized lookup rows needed by the runtime and UI.

pub mod index;
pub mod rebuild;
pub mod resolver;
pub mod scheduler;
pub mod thread;

pub use index::{
    IntentContextFrameIndexRow, IntentPlanIndexRow, IntentTaskIndexRow, PlanStepTaskIndexRow,
    RunEventIndexRow, RunPatchSetIndexRow, TaskRunIndexRow,
};
pub use rebuild::{MaterializedProjection, ProjectionRebuilder};
pub use resolver::{
    ProjectionResolver, QueryIndexDiagnostic, ResumeAction, ResumeBundle, ResumeReason,
    ThreadBundle, ThreadQueryIndexes,
};
pub use scheduler::{
    LiveContextFrameRef, LiveContextPinKind, LiveContextSourceKind, PlanHeadRef, SchedulerState,
    SchedulerStateCasError, SchedulerStateRepository,
};
pub use thread::{
    ThreadId, ThreadIntentLinkReason, ThreadIntentRef, ThreadParticipant, ThreadParticipantRole,
    ThreadProjection,
};
