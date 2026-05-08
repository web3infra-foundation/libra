//! Goal event stream — append-only log of everything that happened.
//!
//! Per `docs/improvement/opencode.md` lines 578-590, every state change
//! the supervisor records flows through a [`GoalEvent`] variant. Wrapped
//! in a [`GoalEventEnvelope`] (id, goal_id, recorded_at), each event is
//! persisted to the same JSONL stream as the rest of the session — see
//! `docs/improvement/opencode.md` line 595 for the
//! `SessionEvent::Goal(GoalEventEnvelope)` integration.
//!
//! Replay is the only way to reconstitute [`super::state::GoalState`].
//! That means:
//!
//! 1. The variant set is **stable** once shipped — adding fields uses
//!    `#[serde(default)]`, never reordering.
//! 2. Readers are **unknown-event-safe**: future variants we have not
//!    yet written must skip cleanly without panicking older binaries.
//!    The dispatch loop in [`super::state::apply`] enforces this with
//!    `#[serde(other)]` on a `Future` catch-all.
//! 3. Events carry **only** structured data — no raw transcripts, no
//!    big inline payloads. Evidence references go through
//!    [`super::state::GoalEvidenceRef`] (file path + content hash, tool
//!    call id, attachment id, ...) so the stream stays small and
//!    redaction-friendly.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    spec::{GoalActor, GoalSpec},
    state::{GoalEvidenceRef, GoalPlanStep, GoalVerificationRecord},
};
use crate::internal::ai::runtime::event::Event;

/// Why the supervisor put a Goal into `Blocked` status.
///
/// Each variant is a recoverable pause: the user can fix the underlying
/// situation (approve more budget, change scope, retry the provider)
/// and the supervisor resumes the same Goal. **None** of these are
/// terminal — the audit trail must still show the Goal as active until
/// the user either resolves the blocker or cancels the Goal explicitly.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GoalBlockReason {
    /// `submit_goal_complete` rejected because acceptance criteria
    /// were missing or evidence was insufficient. The supervisor
    /// continues the loop and surfaces the missing items.
    CompletionRejected {
        missing: Vec<String>,
        reason: String,
    },
    /// Approval was denied by the permission layer. The user must
    /// adjust scope (re-grant the tool, change the request).
    ApprovalDenied {
        denied_tool: String,
        denied_args_summary: Option<String>,
        reason: String,
    },
    /// Hard budget cap reached; the user must approve more spend
    /// (`/budget goal approve <amount>`) or cancel.
    BudgetApprovalRequired {
        cap_micro_usd: u64,
        spent_micro_usd: u64,
    },
    /// Wall-clock budget exhausted.
    WallClockExpired { wall_clock_seconds: u64 },
    /// Provider returned a non-recoverable error (e.g. quota
    /// exhausted, structured `UserActionRequired`). The user must
    /// switch model / refresh keys.
    ProviderUnrecoverable {
        provider_id: String,
        message: String,
    },
    /// Continuation loop count exceeded `max_continuation_loops`. The
    /// supervisor stops auto-progressing and waits for the user.
    LoopLimitNeedsUser { loops_run: u32 },
    /// Out-of-scope situation that needs explicit user input — the
    /// supervisor includes a single concrete question in the matching
    /// `Blocked` event.
    AwaitingScopeChange { question: String },
    /// Single-turn `max_turns` cap reached without forward progress.
    /// The supervisor parks the Goal so the user can decide whether
    /// to extend `max_turns`, change scope, or cancel.
    MaxTurnsReached { turns: u32 },
    /// Repeat-abort kicked in (the model kept calling the same
    /// tool/argument signature). The supervisor stops auto-progress
    /// rather than burning tokens on a stalled loop.
    RepeatAborted { signature: String, repetitions: u32 },
    /// Context overflow that compaction failed to resolve. The
    /// supervisor surfaces this so the user can shrink scope or
    /// pick a model with a larger window.
    ContextOverflowExhausted { attempts: u32, last_error: String },
    /// Forward-compatibility catch-all for variants emitted by future
    /// Libra versions — same role as `GoalEvent::Future` but for the
    /// nested blocker discriminator. Keeps a Goal envelope replayable
    /// even when its embedded `Blocked` carries a tomorrow-only kind.
    #[serde(other)]
    Future,
}

/// Payload accompanying `update_goal_progress` invocations. Echoed
/// into [`GoalEvent::ProgressRecorded`] so replay re-derives the same
/// state without re-running the tool.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GoalProgressRecord {
    pub summary: String,
    #[serde(default)]
    pub completed_criteria: Vec<String>,
    #[serde(default)]
    pub evidence_refs: Vec<GoalEvidenceRef>,
    #[serde(default)]
    pub next_steps: Vec<String>,
}

/// Payload the model emits via `submit_goal_complete`. The supervisor
/// turns this into either [`GoalEvent::CompletionRejected`] (verifier
/// failed) or [`GoalEvent::Completed`] (verifier passed).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GoalCompletionClaim {
    pub summary: String,
    pub completed_criteria: Vec<String>,
    pub evidence_refs: Vec<GoalEvidenceRef>,
    pub verification: Vec<GoalVerificationRecord>,
    #[serde(default)]
    pub residual_risks: Vec<String>,
}

/// Final report written into [`GoalEvent::Completed`]. This is the
/// audit-grade artefact the user sees in `/goal status` after the Goal
/// finishes; it is the verifier's signed-off view of the claim.
///
/// Per `docs/improvement/opencode.md` line 1519 the report must carry
/// "changed files, verification, residual risk, **budget summary**".
/// The first three already live above; the budget-summary trio
/// ([`Self::total_spent_micro_usd`],
/// [`Self::elapsed_wall_clock_seconds`],
/// [`Self::continuation_loops_used`]) is shipped here so the wire
/// shape is final before P6.2/P6.3 land verifier and supervisor code
/// against it. All three default to `0` so older logs (which never
/// existed: P6.1 has not shipped) and forged streams that omit them
/// surface as "unmetered" rather than crashing replay.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GoalCompletionReport {
    pub summary: String,
    pub completed_criteria: Vec<String>,
    pub evidence_refs: Vec<GoalEvidenceRef>,
    pub verification: Vec<GoalVerificationRecord>,
    #[serde(default)]
    pub residual_risks: Vec<String>,
    #[serde(default)]
    pub changed_files: Vec<String>,
    /// Total Goal-loop spend at the moment the verifier accepted, in
    /// micro-USD. Mirrors [`super::spec::GoalBudget::hard_cap_micro_usd`]
    /// units so a `/goal status` view can compute "{spent}/{cap}"
    /// without reconciling denominations. `0` means the Goal ran with
    /// no cost meter wired (e.g. fake provider / smoke test) — **not**
    /// "free run". The supervisor (P6.3) is the source of truth.
    #[serde(default)]
    pub total_spent_micro_usd: u64,
    /// Wall-clock duration from `GoalSpec.created_at` to
    /// `finalised_at`, in seconds. `0` means unknown / unmetered.
    #[serde(default)]
    pub elapsed_wall_clock_seconds: u64,
    /// Continuation loops the supervisor consumed before reaching
    /// completion. Bounded by
    /// [`super::spec::GoalBudget::max_continuation_loops`].
    /// `0` means the Goal completed on the very first turn.
    #[serde(default)]
    pub continuation_loops_used: u32,
    pub finalised_at: DateTime<Utc>,
    pub finalised_by: GoalActor,
}

/// Schema-layer reasons a [`GoalCompletionReport`] cannot be honoured.
///
/// The deterministic verifier (P6.2) does the rich semantic check
/// (evidence quality, file-hash matching, tool-call success). This
/// schema-layer error type encodes the *minimum floor* every Completed
/// envelope must satisfy regardless of policy: claimed criterion ids
/// must exist in the spec, every required criterion must be claimed.
///
/// `apply`'s `GoalEvent::Completed` arm runs
/// [`validate_completion_report_shape`] before transitioning to
/// `GoalStatus::Completed`. The check is strictly weaker than the
/// verifier's check, so a verifier-blessed report always passes; a
/// forged or corrupted JSONL stream that ships a `Created` →
/// `Completed` pair without ever invoking the verifier is rejected
/// here.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum GoalCompletionShapeError {
    #[error(
        "GoalCompletionReport.completed_criteria contains id `{id}` which does not exist in \
         GoalSpec.acceptance_criteria — the verifier (P6.2) cannot reconcile a claim against \
         a criterion the spec does not declare"
    )]
    UnknownCriterionId { id: String },
    #[error(
        "GoalCompletionReport omits required criterion `{id}` — every criterion with \
         `required = true` must appear in `completed_criteria` before a Goal can transition \
         to `Completed`"
    )]
    MissingRequiredCriterion { id: String },
    #[error(
        "GoalCompletionReport.completed_criteria contains duplicate id `{id}` — the \
         verifier keys completion off a `BTreeSet<String>` so duplicates would let one \
         claim satisfy multiple required criteria"
    )]
    DuplicateClaimedCriterion { id: String },
}

/// Schema-layer shape gate run on every `Completed` envelope before the
/// state transitions to terminal `Completed`. Pinned by the doc's
/// "submit_goal_complete" rules at lines 1474-1476: `Completed` is only
/// emitted after the verifier accepts. This function enforces the
/// minimum invariants every verifier accept-path must also produce, so
/// a corrupted or forged JSONL stream that bypassed the verifier is
/// rejected at the resume seam.
///
/// The function is **read-only** — it never mutates either argument.
/// Callers that hold a mutable state borrow can run this safely
/// before the transition.
pub fn validate_completion_report_shape(
    spec: &GoalSpec,
    report: &GoalCompletionReport,
) -> Result<(), GoalCompletionShapeError> {
    use std::collections::BTreeSet;
    let mut spec_ids: BTreeSet<&str> = BTreeSet::new();
    for criterion in &spec.acceptance_criteria {
        spec_ids.insert(criterion.id.as_str());
    }
    let mut seen_in_report: BTreeSet<&str> = BTreeSet::new();
    for claimed in &report.completed_criteria {
        if !seen_in_report.insert(claimed.as_str()) {
            return Err(GoalCompletionShapeError::DuplicateClaimedCriterion {
                id: claimed.clone(),
            });
        }
        if !spec_ids.contains(claimed.as_str()) {
            return Err(GoalCompletionShapeError::UnknownCriterionId {
                id: claimed.clone(),
            });
        }
    }
    for criterion in &spec.acceptance_criteria {
        if criterion.required && !seen_in_report.contains(criterion.id.as_str()) {
            return Err(GoalCompletionShapeError::MissingRequiredCriterion {
                id: criterion.id.clone(),
            });
        }
    }
    Ok(())
}

/// Append-only event variants. Each variant carries enough information
/// for [`super::state::apply`] to derive the new state purely from the
/// event sequence; the supervisor never relies on out-of-band context.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GoalEvent {
    /// Goal seeded from a [`GoalSpec`].
    Created(GoalSpec),
    /// Plan refreshed (initial draft, replan, pruned dead steps).
    PlanUpdated { steps: Vec<GoalPlanStep> },
    /// User-driven criteria revision — `docs/improvement/opencode.md`
    /// line 690's `/goal criteria add <text>` entry point. The full
    /// post-revision criteria list is carried inline so replay can
    /// produce a self-consistent state without consulting prior
    /// events. The supervisor also accepts revisions that **add**
    /// new criteria; **removing** criteria mid-Goal is intentionally
    /// allowed at the schema layer (the gate lives in the user
    /// interface, not here).
    CriteriaRevised {
        criteria: Vec<super::spec::GoalCriterion>,
        revised_by: GoalActor,
    },
    /// Supervisor entered a step (drives `GoalStepStatus::InProgress`).
    StepStarted { step_id: String },
    /// Supervisor finished a step with at least one evidence ref.
    StepCompleted {
        step_id: String,
        evidence_refs: Vec<GoalEvidenceRef>,
    },
    /// Model invoked `update_goal_progress` (non-terminal tool).
    ProgressRecorded(GoalProgressRecord),
    /// Supervisor parked the Goal pending external input. Optional
    /// `requested_input` is a single concrete question shown to the
    /// user.
    Blocked {
        reason: GoalBlockReason,
        #[serde(default)]
        requested_input: Option<String>,
    },
    /// Model invoked `submit_goal_complete`. State transitions to
    /// `CompletionClaimed` and the supervisor runs the verifier.
    CompletionClaimed(GoalCompletionClaim),
    /// Verifier rejected the most recent claim. The supervisor keeps
    /// the Goal active and continues the loop.
    CompletionRejected {
        missing: Vec<String>,
        reason: String,
    },
    /// Verifier accepted the claim. Terminal; the session may go idle.
    Completed(GoalCompletionReport),
    /// User / automation owner / lease owner explicitly cancelled
    /// the Goal. Terminal.
    Cancelled {
        reason: String,
        cancelled_by: GoalActor,
    },
    /// Forward-compatibility catch-all for variants emitted by future
    /// Libra versions. Older binaries deserialise unknown payloads
    /// here and [`super::state::apply`] no-ops them so replay never
    /// panics. **Never** construct this variant by hand — encode the
    /// new variant with its real `kind` discriminator.
    #[serde(other)]
    Future,
}

/// Wire envelope persisted alongside other [`crate::internal::ai::session::jsonl::SessionEvent`]
/// variants. Carries the stable event id (used for cross-event
/// references like `step_id → step_started_event_id`), the goal id
/// (lets replay filter to a single Goal), and the wall-clock receipt
/// time (for ordering when multiple events share the same logical
/// step).
///
/// `SessionEvent::Goal(GoalEventEnvelope)` is added in P6.1 too so the
/// integration is byte-stable from day one — but the supervisor that
/// actually emits these envelopes lands in P6.3.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GoalEventEnvelope {
    pub envelope_id: Uuid,
    pub goal_id: Uuid,
    pub recorded_at: DateTime<Utc>,
    pub event: GoalEvent,
}

impl GoalEventEnvelope {
    /// Build an envelope around `event`, attributing it to `goal_id`
    /// at `recorded_at`. The envelope id is freshly generated; callers
    /// that need deterministic ids (replay tests, control-plane
    /// handoff) should construct the struct directly.
    pub fn new(goal_id: Uuid, recorded_at: DateTime<Utc>, event: GoalEvent) -> Self {
        Self {
            envelope_id: Uuid::new_v4(),
            goal_id,
            recorded_at,
            event,
        }
    }
}

impl Event for GoalEventEnvelope {
    fn event_kind(&self) -> &'static str {
        // Stable wire kind for SessionEvent::Goal — MUST match the
        // `kind` discriminator the JSONL reader dispatches on
        // (see `parse_session_event_value` in
        // `crate::internal::ai::session::jsonl`). The discriminator
        // is `"goal"` because that is what the serde-driven
        // `SessionEvent::Goal` variant serialises as
        // (`#[serde(rename_all = "snake_case")]`).
        "goal"
    }

    fn event_id(&self) -> Uuid {
        self.envelope_id
    }

    fn event_summary(&self) -> String {
        let inner = match &self.event {
            GoalEvent::Created(_) => "created",
            GoalEvent::CriteriaRevised { .. } => "criteria_revised",
            GoalEvent::PlanUpdated { .. } => "plan_updated",
            GoalEvent::StepStarted { .. } => "step_started",
            GoalEvent::StepCompleted { .. } => "step_completed",
            GoalEvent::ProgressRecorded(_) => "progress_recorded",
            GoalEvent::Blocked { .. } => "blocked",
            GoalEvent::CompletionClaimed(_) => "completion_claimed",
            GoalEvent::CompletionRejected { .. } => "completion_rejected",
            GoalEvent::Completed(_) => "completed",
            GoalEvent::Cancelled { .. } => "cancelled",
            GoalEvent::Future => "unknown_future_variant",
        };
        format!("goal {} {inner}", self.goal_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unknown-event-safe contract: a payload whose `kind` discriminator
    /// is not one of the documented variants must deserialise as
    /// [`GoalEvent::Future`] without panicking. Older binaries reading
    /// a JSONL stream from a future Libra version land here.
    #[test]
    fn unknown_kind_deserialises_as_future_variant() {
        let json = r#"{"kind":"goal_v999_warp_drive","payload":{"answer":42}}"#;
        let event: GoalEvent = serde_json::from_str(json).expect("unknown kinds must not panic");
        assert!(matches!(event, GoalEvent::Future));
    }

    /// JSON round-trip pins the wire shape for every variant whose
    /// payload appears on the supervisor's hot path.
    #[test]
    fn round_trips_every_documented_variant() {
        let goal_id = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        let progress = GoalProgressRecord {
            summary: "compiled successfully".to_string(),
            completed_criteria: vec!["compiles".to_string()],
            evidence_refs: vec![],
            next_steps: vec!["run tests".to_string()],
        };
        let claim = GoalCompletionClaim {
            summary: "done".to_string(),
            completed_criteria: vec!["compiles".to_string(), "tests-pass".to_string()],
            evidence_refs: vec![],
            verification: vec![],
            residual_risks: vec!["coverage report still pending".to_string()],
        };
        let blocker = GoalBlockReason::BudgetApprovalRequired {
            cap_micro_usd: 1_000_000,
            spent_micro_usd: 1_005_000,
        };

        let variants = [
            GoalEvent::PlanUpdated { steps: vec![] },
            GoalEvent::StepStarted {
                step_id: "step-1".to_string(),
            },
            GoalEvent::StepCompleted {
                step_id: "step-1".to_string(),
                evidence_refs: vec![],
            },
            GoalEvent::ProgressRecorded(progress.clone()),
            GoalEvent::Blocked {
                reason: blocker.clone(),
                requested_input: Some("Approve $0.50 more?".to_string()),
            },
            GoalEvent::CompletionClaimed(claim.clone()),
            GoalEvent::CompletionRejected {
                missing: vec!["tests-pass".to_string()],
                reason: "no test evidence".to_string(),
            },
            GoalEvent::Cancelled {
                reason: "user pressed Ctrl-C".to_string(),
                cancelled_by: GoalActor::User { id: None },
            },
        ];
        for variant in variants {
            let envelope = GoalEventEnvelope::new(goal_id, fixture_now(), variant.clone());
            let json = serde_json::to_string(&envelope).expect("serialize");
            let back: GoalEventEnvelope = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(envelope, back, "round-trip diverged for {variant:?}");
        }
    }

    fn fixture_now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-05-08T13:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    /// `event_summary()` includes the goal id and the variant name.
    /// The TUI / audit log surface this verbatim, so a regression that
    /// drops either piece would silently degrade the log signal.
    #[test]
    fn envelope_summary_carries_goal_id_and_variant_label() {
        let goal_id = Uuid::parse_str("00000000-0000-0000-0000-000000000abc").unwrap();
        let envelope = GoalEventEnvelope::new(
            goal_id,
            fixture_now(),
            GoalEvent::StepStarted {
                step_id: "step-1".to_string(),
            },
        );
        let summary = envelope.event_summary();
        assert!(summary.contains(&goal_id.to_string()));
        assert!(summary.contains("step_started"));
    }

    fn shape_fixture_spec(criteria: Vec<super::super::spec::GoalCriterion>) -> GoalSpec {
        GoalSpec::new(
            Uuid::parse_str("00000000-0000-0000-0000-0000feed1234").unwrap(),
            "thread-1",
            "session-1",
            "deliver feature X",
            criteria,
            Vec::new(),
            super::super::spec::GoalEvidencePolicy::Standard,
            super::super::spec::GoalBudget::default(),
            fixture_now(),
            GoalActor::User { id: None },
        )
        .expect("happy-path spec must construct")
    }

    fn shape_fixture_report(completed_criteria: Vec<String>) -> GoalCompletionReport {
        GoalCompletionReport {
            summary: "shipped".to_string(),
            completed_criteria,
            evidence_refs: vec![],
            verification: vec![],
            residual_risks: vec![],
            changed_files: vec![],
            total_spent_micro_usd: 0,
            elapsed_wall_clock_seconds: 0,
            continuation_loops_used: 0,
            finalised_at: fixture_now(),
            finalised_by: GoalActor::System {
                reason: "verifier accepted".to_string(),
            },
        }
    }

    /// A report whose `completed_criteria` covers every required spec
    /// criterion (and nothing extra) passes the schema-layer floor.
    /// This is the legitimate verifier-emitted shape; the floor must
    /// not reject it.
    #[test]
    fn shape_check_accepts_well_formed_report() {
        let spec = shape_fixture_spec(vec![
            super::super::spec::GoalCriterion {
                id: "compiles".to_string(),
                description: "cargo check".to_string(),
                required: true,
                verifier_hint: None,
                requires_workspace_change: false,
            },
            super::super::spec::GoalCriterion {
                id: "tests".to_string(),
                description: "cargo test".to_string(),
                required: true,
                verifier_hint: None,
                requires_workspace_change: false,
            },
        ]);
        let report = shape_fixture_report(vec!["compiles".to_string(), "tests".to_string()]);
        assert!(validate_completion_report_shape(&spec, &report).is_ok());
    }

    /// A report that omits a required criterion is rejected. This is
    /// the central P1 attack closure: a forged JSONL `Completed` that
    /// claims fewer required criteria than the spec demands cannot
    /// transition replay into `Completed`.
    #[test]
    fn shape_check_rejects_report_missing_required_criterion() {
        let spec = shape_fixture_spec(vec![
            super::super::spec::GoalCriterion {
                id: "compiles".to_string(),
                description: "cargo check".to_string(),
                required: true,
                verifier_hint: None,
                requires_workspace_change: false,
            },
            super::super::spec::GoalCriterion {
                id: "tests".to_string(),
                description: "cargo test".to_string(),
                required: true,
                verifier_hint: None,
                requires_workspace_change: false,
            },
        ]);
        let report = shape_fixture_report(vec!["compiles".to_string()]);
        let err = validate_completion_report_shape(&spec, &report)
            .expect_err("missing required criterion must fail");
        assert_eq!(
            err,
            GoalCompletionShapeError::MissingRequiredCriterion {
                id: "tests".to_string(),
            },
        );
    }

    /// A report that claims a criterion the spec never declared is
    /// rejected. This protects against forged streams that fabricate
    /// criterion ids out of band.
    #[test]
    fn shape_check_rejects_report_with_unknown_criterion_id() {
        let spec = shape_fixture_spec(vec![super::super::spec::GoalCriterion {
            id: "compiles".to_string(),
            description: "cargo check".to_string(),
            required: true,
            verifier_hint: None,
            requires_workspace_change: false,
        }]);
        let report = shape_fixture_report(vec!["compiles".to_string(), "fabricated".to_string()]);
        let err = validate_completion_report_shape(&spec, &report)
            .expect_err("unknown criterion id must fail");
        assert_eq!(
            err,
            GoalCompletionShapeError::UnknownCriterionId {
                id: "fabricated".to_string(),
            },
        );
    }

    /// A report whose `completed_criteria` repeats the same id is
    /// rejected — the verifier keys completion off a `BTreeSet`, so
    /// duplicates would silently let one claim count twice.
    #[test]
    fn shape_check_rejects_report_with_duplicate_claimed_id() {
        let spec = shape_fixture_spec(vec![super::super::spec::GoalCriterion {
            id: "compiles".to_string(),
            description: "cargo check".to_string(),
            required: true,
            verifier_hint: None,
            requires_workspace_change: false,
        }]);
        let report = shape_fixture_report(vec!["compiles".to_string(), "compiles".to_string()]);
        let err = validate_completion_report_shape(&spec, &report)
            .expect_err("duplicate claimed id must fail");
        assert_eq!(
            err,
            GoalCompletionShapeError::DuplicateClaimedCriterion {
                id: "compiles".to_string(),
            },
        );
    }

    /// Non-required criteria can be omitted from the report — the
    /// spec marks them as nice-to-have.
    #[test]
    fn shape_check_allows_optional_criteria_to_be_omitted() {
        let spec = shape_fixture_spec(vec![
            super::super::spec::GoalCriterion {
                id: "compiles".to_string(),
                description: "cargo check".to_string(),
                required: true,
                verifier_hint: None,
                requires_workspace_change: false,
            },
            super::super::spec::GoalCriterion {
                id: "docs".to_string(),
                description: "module docs updated".to_string(),
                required: false,
                verifier_hint: None,
                requires_workspace_change: false,
            },
        ]);
        let report = shape_fixture_report(vec!["compiles".to_string()]);
        assert!(validate_completion_report_shape(&spec, &report).is_ok());
    }

    /// Budget summary fields default to `0` when missing from the
    /// JSON payload — pins forward-compat against logs written before
    /// these fields existed (none in the wild yet, but the schema
    /// must round-trip cleanly anyway).
    #[test]
    fn completion_report_budget_summary_defaults_to_zero() {
        let json = r#"{
            "summary": "shipped",
            "completed_criteria": [],
            "evidence_refs": [],
            "verification": [],
            "finalised_at": "2026-05-08T13:00:00Z",
            "finalised_by": {"kind":"user","id":null}
        }"#;
        let report: GoalCompletionReport = serde_json::from_str(json).expect("deserialize");
        assert_eq!(report.total_spent_micro_usd, 0);
        assert_eq!(report.elapsed_wall_clock_seconds, 0);
        assert_eq!(report.continuation_loops_used, 0);
    }
}
