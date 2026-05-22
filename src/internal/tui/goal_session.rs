//! In-memory Goal session state owned by the TUI App.
//!
//! Per `docs/improvement/opencode.md` lines 540-700, an active Goal
//! is a session-level construct: there is at most one Goal in flight
//! per `libra code` session, and its events flow into the same
//! JSONL stream as the rest of the session (P6.7 wires the
//! persistence side). This module defines the in-memory handle the
//! App holds while a Goal is active.
//!
//! The handle is intentionally thin:
//!
//! * It carries the replayable [`GoalState`] and the cumulative
//!   [`GoalEventEnvelope`] log that built it. Any future persistence
//!   layer can flush `events` to disk verbatim and replay it back —
//!   no extra serialisation surface.
//! * The state is mutated only through the [`apply`] path (schema
//!   floor), so any consumer reading `state` sees a verifier-safe
//!   projection.
//! * Exactly one envelope is appended by `create` and `cancel`; the
//!   Goal supervisor appends in-flight envelopes
//!   (`CompletionClaimed`, `Completed`, `CompletionRejected`,
//!   `Blocked`, `ProgressRecorded`) through
//!   [`GoalSession::append_supervisor_events`].
//!
//! User-facing surfaces (TUI `/goal` slash commands and Code Control
//! NDJSON `goal.*` methods) both bottom out in the App methods that
//! own a single `Option<GoalSession>` field — the contract is shared.

use chrono::Utc;
use uuid::Uuid;

use super::goal_command::{
    GoalCommandParseError, validate_objective as validate_objective_via_command,
};
use crate::internal::ai::goal::{
    GoalActor, GoalBudget, GoalEvent, GoalEventEnvelope, GoalEvidencePolicy, GoalSpec,
    GoalSpecError, GoalState, apply,
};

/// Schema-floor wrapper that re-runs `GoalSpec::new`'s objective
/// rules without going through full spec construction. The validator
/// from [`super::goal_command::validate_objective`] is the single
/// source of truth shared between the CLI flag, the slash command,
/// and this module — calling through it here keeps the three
/// surfaces aligned by construction.
fn validate_goal_objective(objective: &str) -> Result<(), GoalSpecError> {
    match validate_objective_via_command(objective) {
        Ok(()) => Ok(()),
        Err(GoalCommandParseError::InvalidObjective { source }) => Err(source),
        // `validate_objective` only ever returns the
        // `InvalidObjective` arm; pattern-match exhaustively so a
        // future broader error variant doesn't slip through silently.
        Err(_) => Err(GoalSpecError::EmptyObjective),
    }
}

/// In-memory handle for one active Goal session. The App holds
/// `Option<GoalSession>`; `None` means "no active Goal".
#[derive(Debug, Clone)]
pub struct GoalSession {
    /// Replayable projection of `events`. Always derived through
    /// `apply()` so any consumer sees a verifier-safe view.
    state: GoalState,
    /// Append-only event log. The first envelope is `Created`; any
    /// subsequent envelope is appended through `apply()` so the
    /// schema floor still gates each addition.
    events: Vec<GoalEventEnvelope>,
}

/// Errors returned by [`GoalSession`] mutators. The error variants
/// are designed to flow into both the TUI slash-command response
/// cell and the Code Control NDJSON error response. The
/// "already-active" gate lives one layer up (in the App) because
/// `GoalSession::create` builds a fresh handle from scratch — only
/// the App holds the `Option<GoalSession>` slot that can be
/// occupied; that's where `GoalAlreadyActive` is checked, against
/// `TuiControlError::GoalAlreadyActive`.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum GoalSessionError {
    /// `status` / `cancel` was called when no Goal is active.
    #[error("no active Goal in this session — start one with `/goal start <objective>`")]
    NotActive,
    /// The objective failed `GoalSpec`'s shape rules.
    #[error("Goal objective failed validation: {source}")]
    InvalidObjective {
        #[source]
        source: GoalSpecError,
    },
    /// The schema's `apply()` refused the envelope this method
    /// constructed. Should never fire in normal use because the
    /// session only emits well-formed envelopes; surfaced as a
    /// last-line internal error.
    #[error("internal: Goal session apply rejected envelope: {detail}")]
    InternalApply { detail: String },
}

/// Snapshot of one cancellation outcome. The mutator returns this
/// so callers (TUI cell, NDJSON response) can render the new state
/// without a follow-up read.
#[derive(Debug, Clone)]
pub struct GoalCancelOutcome {
    pub state: GoalState,
}

impl GoalSession {
    /// Read-only access to the projection.
    pub fn state(&self) -> &GoalState {
        &self.state
    }

    /// Read-only access to the cumulative event log. The TUI flushes
    /// new envelopes to the session JSONL whenever a Goal mutation
    /// succeeds; tests also use this to assert the schema-floor event
    /// order.
    pub fn events(&self) -> &[GoalEventEnvelope] {
        &self.events
    }

    /// Whether this session has reached a terminal state
    /// (`Completed` / `Cancelled`). The App treats a terminal
    /// session as "no longer active" for the purposes of starting
    /// a new one.
    pub fn is_terminal(&self) -> bool {
        self.state.status.is_terminal()
    }

    /// Build a fresh Goal session. Mints a `Uuid` for the goal id,
    /// stamps `created_at = Utc::now()`, and emits the initial
    /// `GoalEvent::Created` envelope. The objective is validated
    /// against the same shape rules `GoalSpec::new` enforces; an
    /// invalid objective surfaces as
    /// [`GoalSessionError::InvalidObjective`] without mutating any
    /// state.
    ///
    /// `acceptance_criteria` defaults to empty — the supervisor
    /// (P6.3 / P6.4) is expected to populate it via
    /// `update_goal_progress` / `/goal criteria add` calls. The
    /// verifier (P6.2) treats an empty required-criteria set as
    /// "no required gating", so a Goal with no criteria can still
    /// be claimed completed via `submit_goal_complete` once it has
    /// at least one piece of evidence; users wanting strict gating
    /// should add criteria explicitly.
    pub fn create(
        thread_id: impl Into<String>,
        session_id: impl Into<String>,
        objective: String,
        actor: GoalActor,
    ) -> Result<Self, GoalSessionError> {
        let trimmed = objective.trim();
        validate_goal_objective(trimmed)
            .map_err(|source| GoalSessionError::InvalidObjective { source })?;
        let now = Utc::now();
        let goal_id = Uuid::new_v4();
        let spec = GoalSpec::new(
            goal_id,
            thread_id,
            session_id,
            trimmed.to_string(),
            Vec::new(),
            Vec::new(),
            GoalEvidencePolicy::Standard,
            GoalBudget::default(),
            now,
            actor,
        )
        .map_err(|source| GoalSessionError::InvalidObjective { source })?;
        let state = GoalState::from_spec(spec.clone());
        // The `Created` envelope's `recorded_at` mirrors
        // `spec.created_at` so the apply()'s monotonic-time guard
        // accepts every subsequent envelope (whose recorded_at >=
        // now).
        let envelope = GoalEventEnvelope {
            envelope_id: Uuid::new_v4(),
            goal_id,
            recorded_at: now,
            event: GoalEvent::Created(spec),
        };
        Ok(Self {
            state,
            events: vec![envelope],
        })
    }

    /// Append a `Cancelled` envelope and apply it. Returns the
    /// updated state. Refuses with [`GoalSessionError::NotActive`]
    /// if the session is already terminal — the caller (App) is
    /// expected to clear the `Option<GoalSession>` after a
    /// successful cancel, but defensive in case of double-call.
    pub fn cancel(
        &mut self,
        reason: String,
        cancelled_by: GoalActor,
    ) -> Result<GoalCancelOutcome, GoalSessionError> {
        if self.state.status.is_terminal() {
            return Err(GoalSessionError::NotActive);
        }
        let now = Utc::now();
        let envelope = GoalEventEnvelope {
            envelope_id: Uuid::new_v4(),
            goal_id: self.state.spec.goal_id,
            recorded_at: now.max(self.state.updated_at),
            event: GoalEvent::Cancelled {
                reason,
                cancelled_by,
            },
        };
        apply(&mut self.state, &envelope).map_err(|reject| GoalSessionError::InternalApply {
            detail: reject.to_string(),
        })?;
        self.events.push(envelope);
        Ok(GoalCancelOutcome {
            state: self.state.clone(),
        })
    }

    /// Append envelopes emitted by the Goal supervisor and apply
    /// them through the same schema floor used by slash-command
    /// mutations. This is the only path for runtime envelopes such
    /// as `ProgressRecorded`, `CompletionClaimed`,
    /// `CompletionRejected`, `Blocked`, and `Completed`.
    pub fn append_supervisor_events(
        &mut self,
        events: &[GoalEventEnvelope],
    ) -> Result<(), GoalSessionError> {
        if self.state.status.is_terminal() {
            return Err(GoalSessionError::NotActive);
        }
        for envelope in events {
            apply(&mut self.state, envelope).map_err(|reject| GoalSessionError::InternalApply {
                detail: reject.to_string(),
            })?;
            self.events.push(envelope.clone());
        }
        Ok(())
    }
}

/// Render `state` as a multi-line human-readable summary — used by
/// both the TUI `/goal status` cell and the NDJSON `goal.status`
/// response body. Stable shape for golden tests.
pub fn render_goal_status(state: &GoalState) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Goal {} — {:?}\n",
        state.spec.goal_id, state.status
    ));
    out.push_str(&format!("Objective: {}\n", state.spec.objective));
    out.push_str(&format!(
        "Acceptance criteria ({}): ",
        state.spec.acceptance_criteria.len()
    ));
    if state.spec.acceptance_criteria.is_empty() {
        out.push_str("(none yet — add via `/goal criteria add <description>`)\n");
    } else {
        let names: Vec<&str> = state
            .spec
            .acceptance_criteria
            .iter()
            .map(|c| c.id.as_str())
            .collect();
        out.push_str(&names.join(", "));
        out.push('\n');
    }
    out.push_str(&format!(
        "Completed criteria: {}\n",
        if state.completed_criteria.is_empty() {
            "(none)".to_string()
        } else {
            state
                .completed_criteria
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        }
    ));
    out.push_str(&format!(
        "Evidence refs accumulated: {}\n",
        state.evidence_refs.len()
    ));
    if !state.blockers.is_empty() {
        out.push_str(&format!("Blockers ({}):\n", state.blockers.len()));
        for blocker in &state.blockers {
            out.push_str(&format!("  - {:?}\n", blocker.reason));
        }
    }
    if let Some(report) = &state.completion_report {
        out.push_str(&format!(
            "Completion report: {} ({} evidence refs, finalised at {})\n",
            report.summary,
            report.evidence_refs.len(),
            report.finalised_at,
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::goal::{
        GoalEvent, GoalEventEnvelope, GoalProgressRecord, GoalStatus, MAX_OBJECTIVE_LEN,
    };

    fn user_actor() -> GoalActor {
        GoalActor::User { id: None }
    }

    #[test]
    fn create_seeds_state_and_emits_created_envelope() {
        let session = GoalSession::create(
            "thread-1",
            "session-1",
            "ship feature".to_string(),
            user_actor(),
        )
        .expect("happy-path create must succeed");
        assert_eq!(session.state().status, GoalStatus::Active);
        assert_eq!(session.events().len(), 1);
        assert!(matches!(session.events()[0].event, GoalEvent::Created(_)));
        assert_eq!(session.state().spec.objective, "ship feature");
    }

    #[test]
    fn create_trims_objective_whitespace() {
        let session = GoalSession::create(
            "thread-1",
            "session-1",
            "   ship feature   ".to_string(),
            user_actor(),
        )
        .unwrap();
        assert_eq!(session.state().spec.objective, "ship feature");
    }

    #[test]
    fn create_rejects_blank_objective() {
        let err = GoalSession::create("thread-1", "session-1", "  ".to_string(), user_actor())
            .unwrap_err();
        assert!(matches!(
            err,
            GoalSessionError::InvalidObjective {
                source: GoalSpecError::EmptyObjective
            }
        ));
    }

    #[test]
    fn create_rejects_oversized_objective() {
        let big = "z".repeat(MAX_OBJECTIVE_LEN + 1);
        let err = GoalSession::create("thread-1", "session-1", big, user_actor()).unwrap_err();
        assert!(matches!(
            err,
            GoalSessionError::InvalidObjective {
                source: GoalSpecError::ObjectiveTooLong { .. }
            }
        ));
    }

    #[test]
    fn cancel_appends_cancelled_envelope_and_marks_terminal() {
        let mut session = GoalSession::create(
            "thread-1",
            "session-1",
            "ship feature".to_string(),
            user_actor(),
        )
        .unwrap();
        let outcome = session
            .cancel("user changed mind".to_string(), user_actor())
            .expect("cancel must succeed on an active session");
        assert_eq!(outcome.state.status, GoalStatus::Cancelled);
        assert!(session.is_terminal());
        assert_eq!(session.events().len(), 2);
        assert!(matches!(
            session.events()[1].event,
            GoalEvent::Cancelled { .. }
        ));
    }

    #[test]
    fn cancel_refuses_when_already_terminal() {
        let mut session = GoalSession::create(
            "thread-1",
            "session-1",
            "ship feature".to_string(),
            user_actor(),
        )
        .unwrap();
        session
            .cancel("first cancel".to_string(), user_actor())
            .unwrap();
        let err = session
            .cancel("second cancel".to_string(), user_actor())
            .unwrap_err();
        assert_eq!(err, GoalSessionError::NotActive);
    }

    #[test]
    fn append_supervisor_events_replays_and_extends_log() {
        let mut session = GoalSession::create(
            "thread-1",
            "session-1",
            "ship feature".to_string(),
            user_actor(),
        )
        .unwrap();
        let event = GoalEventEnvelope::new(
            session.state().spec.goal_id,
            session.state().updated_at + chrono::Duration::seconds(1),
            GoalEvent::ProgressRecorded(GoalProgressRecord {
                summary: "implemented first slice".to_string(),
                completed_criteria: Vec::new(),
                evidence_refs: Vec::new(),
                next_steps: vec!["run tests".to_string()],
            }),
        );

        session
            .append_supervisor_events(std::slice::from_ref(&event))
            .expect("supervisor event should apply");

        assert_eq!(session.events().last(), Some(&event));
        assert_eq!(
            session.state().last_assistant_summary,
            Some("implemented first slice".to_string())
        );
    }

    #[test]
    fn render_status_includes_objective_and_status_flag() {
        let session = GoalSession::create(
            "thread-1",
            "session-1",
            "ship feature".to_string(),
            user_actor(),
        )
        .unwrap();
        let out = render_goal_status(session.state());
        assert!(out.contains("Active"));
        assert!(out.contains("ship feature"));
        assert!(out.contains("Acceptance criteria (0)"));
    }

    #[test]
    fn goal_session_error_display_pins_each_variant() {
        assert_eq!(
            GoalSessionError::NotActive.to_string(),
            "no active Goal in this session — start one with `/goal start <objective>`",
        );
        let invalid = GoalSessionError::InvalidObjective {
            source: GoalSpecError::EmptyObjective,
        };
        assert_eq!(
            invalid.to_string(),
            format!(
                "Goal objective failed validation: {}",
                GoalSpecError::EmptyObjective,
            ),
        );
        assert_eq!(
            GoalSessionError::InternalApply {
                detail: "criterion id duplicate".to_string(),
            }
            .to_string(),
            "internal: Goal session apply rejected envelope: criterion id duplicate",
        );
    }
}
