//! Codex-like Goal mode runtime contract (OC-Phase 6).
//!
//! Per `docs/improvement/opencode.md` line 532, Goal mode is a runtime layer
//! that **prevents the assistant's normal final answer from putting the
//! session into idle while a Goal is still active**. The schema here is the
//! foundation for that contract: the immutable [`spec::GoalSpec`] (what the
//! user asked for + acceptance criteria), the append-only [`event::GoalEvent`]
//! stream (what has happened), and the replayable [`state::GoalState`]
//! projection (what currently is).
//!
//! The schema, deterministic verifier, supervisor loop, Goal protocol tools,
//! CLI/TUI surface, Code Control NDJSON methods, resume replay, and flag-off
//! regression coverage all read and write the types defined here. Goal events
//! must round-trip through JSON cleanly and replay deterministically because
//! the runtime persists supervisor envelopes in the same session JSONL stream
//! as the rest of `libra code`.
//!
//! # Module map
//!
//! - [`spec`] — immutable Goal definition (`GoalSpec`, `GoalCriterion`,
//!   `GoalBudget`, `GoalEvidencePolicy`, `GoalActor`).
//! - [`event`] — append-only event stream (`GoalEvent`,
//!   `GoalEventEnvelope`, completion claim/report payloads).
//! - [`state`] — replayable projection (`GoalState`, `GoalStatus`,
//!   plan/evidence/blocker types, [`state::apply`]).
//!
//! # Why a fresh module instead of extending `agent_run`
//!
//! Per `docs/improvement/opencode.md` lines 1551-1556, Goal mode lives in
//! the `libra code` namespace and **must not** touch `agent_session`,
//! `agent_checkpoint`, or `refs/libra/agent-traces` (entire.md ownership
//! boundary). A fresh module makes the boundary visible and stops a
//! drive-by edit from accidentally cross-wiring Goal events into the
//! external `ObservedAgent` capture path.

pub mod driver;
pub mod event;
pub mod prompt;
pub mod spec;
pub mod state;
pub mod supervisor;
pub mod verifier;

pub use driver::{
    GoalSupervisedRun, GoalSupervisedToolLoopRequest, goal_turn_outcome_from_tool_loop_turn,
    run_goal_supervised_tool_loop,
};
pub use event::{
    GoalBlockReason, GoalCompletionClaim, GoalCompletionReport, GoalCompletionShapeError,
    GoalEvent, GoalEventEnvelope, GoalProgressRecord, validate_completion_claim_shape,
    validate_completion_report_shape,
};
pub use prompt::{DefaultGoalContinuationPromptBuilder, GoalContinuationPromptBuilder};
pub use spec::{
    GoalActor, GoalBudget, GoalCriterion, GoalEvidencePolicy, GoalSpec, GoalSpecError,
    MAX_OBJECTIVE_LEN,
};
pub use state::{
    GoalApplyReject, GoalBlocker, GoalEvidenceRef, GoalEvidenceTarget, GoalPlanStep,
    GoalReplayOutcome, GoalReplayRejection, GoalState, GoalStatus, GoalStepStatus,
    GoalVerificationRecord, MAX_REPLAY_REJECTIONS, PendingGoalClaim, apply, replay,
};
pub use supervisor::{
    GoalEventClock, GoalLoopDecision, GoalStopPolicy, GoalSupervisor, GoalSupervisorStep,
    GoalTurnOutcome,
};
pub use verifier::{
    DeterministicGoalVerifier, GoalVerifier, GoalVerifierContext, GoalVerifyOutcome,
    RecentToolCall, ToolResultStatus,
};
