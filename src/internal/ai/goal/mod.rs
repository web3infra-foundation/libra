//! Codex-like Goal mode runtime contract (OC-Phase 6 P6.1 — schema only).
//!
//! Per `docs/improvement/opencode.md` line 532, Goal mode is a runtime layer
//! that **prevents the assistant's normal final answer from putting the
//! session into idle while a Goal is still active**. The schema here is the
//! foundation for that contract: the immutable [`spec::GoalSpec`] (what the
//! user asked for + acceptance criteria), the append-only [`event::GoalEvent`]
//! stream (what has happened), and the replayable [`state::GoalState`]
//! projection (what currently is).
//!
//! The supervisor (P6.3), verifier (P6.2), tools (P6.4), CLI/TUI (P6.5),
//! Code Control NDJSON (P6.6), and end-to-end tests (P6.7) will plug into
//! this schema in later PRs. P6.1 deliberately stays schema-only — no
//! observer wiring, no tool registration, no CLI surface. The downstream
//! integrations all read and write the types defined here, so the schema
//! must round-trip through JSON cleanly and replay deterministically before
//! any executable behaviour ships.
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

pub mod event;
pub mod spec;
pub mod state;

pub use event::{
    GoalBlockReason, GoalCompletionClaim, GoalCompletionReport, GoalEvent, GoalEventEnvelope,
    GoalProgressRecord,
};
pub use spec::{
    GoalActor, GoalBudget, GoalCriterion, GoalEvidencePolicy, GoalSpec, GoalSpecError,
    MAX_OBJECTIVE_LEN,
};
pub use state::{
    GoalBlocker, GoalEvidenceRef, GoalEvidenceTarget, GoalPlanStep, GoalState, GoalStatus,
    GoalStepStatus, GoalVerificationRecord, apply, replay,
};
