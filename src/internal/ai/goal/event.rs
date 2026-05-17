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
    spec::{GoalActor, GoalEvidencePolicy, GoalSpec},
    state::{GoalEvidenceRef, GoalEvidenceTarget, GoalPlanStep, GoalVerificationRecord},
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
///
/// `claim_envelope_id` binds a report to the specific
/// `GoalEvent::CompletionClaimed` envelope the verifier accepted.
/// `apply` checks it against `GoalState::pending_claim_envelope_id`
/// before transitioning to `Completed`, so a forged stream cannot
/// claim under one envelope and then ship a different report
/// against an unrelated active claim (Codex pass-8 P2). The field
/// is required: legacy logs do not exist (P6.1 has not shipped) and
/// every verifier-emitted report knows the claim envelope it just
/// resolved.
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
    /// Envelope id of the [`GoalEvent::CompletionClaimed`] this
    /// report resolves. Pinned by the verifier so the schema can
    /// confirm the claim being completed is the one the supervisor
    /// has open in `pending_claim`.
    pub claim_envelope_id: Uuid,
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

/// Schema-layer shape gate for a [`GoalCompletionClaim`] before it
/// is stored in `state.pending_claim`. The verifier (P6.2) is the
/// authority on whether a claim *passes* — but a claim must at
/// minimum reference real spec ids, with no duplicates and no
/// fabricated verification / evidence cross-references. Without this
/// gate a forged stream can poison `pending_claim` and
/// `state.evidence_refs` (which apply() merges immediately) before
/// the verifier ever sees the claim.
///
/// Reuses [`GoalCompletionShapeError`] — the duplicate-id, unknown-id,
/// and unknown-verification-id variants apply equally to claims and
/// reports. Variants specific to the report (required-coverage,
/// evidence floors, budget overruns) do **not** fire here: a claim is
/// an attempt to complete, not an assertion of completion.
pub fn validate_completion_claim_shape(
    spec: &GoalSpec,
    claim: &GoalCompletionClaim,
) -> Result<(), GoalCompletionShapeError> {
    use std::collections::BTreeSet;
    let mut spec_ids: BTreeSet<&str> = BTreeSet::new();
    for criterion in &spec.acceptance_criteria {
        spec_ids.insert(criterion.id.as_str());
    }
    let mut seen_in_claim: BTreeSet<&str> = BTreeSet::new();
    for claimed in &claim.completed_criteria {
        if !seen_in_claim.insert(claimed.as_str()) {
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
    for record in &claim.verification {
        if !spec_ids.contains(record.criterion_id.as_str()) {
            return Err(GoalCompletionShapeError::UnknownVerificationCriterionId {
                id: record.criterion_id.clone(),
            });
        }
    }
    // Evidence refs that name a `criterion_id` must point at a real
    // spec id. Refs with `criterion_id: None` are kept (they may
    // belong to ambient context, not a specific criterion).
    for evidence in &claim.evidence_refs {
        if let Some(id) = evidence.criterion_id.as_deref()
            && !spec_ids.contains(id)
        {
            return Err(GoalCompletionShapeError::UnknownCriterionId { id: id.to_string() });
        }
    }
    Ok(())
}

/// Schema-layer reasons a [`GoalCompletionReport`] cannot be honoured.
///
/// The deterministic verifier (P6.2) does the rich semantic check
/// (evidence quality, file-hash matching, tool-call success). This
/// schema-layer error type encodes the *minimum floor* every Completed
/// envelope must satisfy regardless of policy: claimed criterion ids
/// must exist in the spec, every required criterion must be claimed,
/// the report's evidence and verification must reference real spec
/// ids, and the report's budget summary must not contradict the
/// spec's budget caps.
///
/// `apply`'s `GoalEvent::Completed` arm runs
/// [`validate_completion_report_shape`] before transitioning to
/// `GoalStatus::Completed`. The check is strictly weaker than the
/// verifier's check, so a verifier-blessed report always passes; a
/// forged or corrupted JSONL stream that bypassed the verifier is
/// rejected here.
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
    #[error(
        "required criterion `{id}` has no matching evidence ref under \
         GoalEvidencePolicy::Standard — every required criterion must be backed by at \
         least one `GoalEvidenceRef` whose `criterion_id` equals the criterion's id"
    )]
    MissingEvidenceForRequiredCriterion { id: String },
    #[error(
        "required criterion `{id}` is marked `requires_workspace_change = true` but no \
         matching evidence ref carries a workspace-bound target \
         (`GoalEvidenceTarget::File`) — Standard policy mandates VCS state evidence for \
         workspace-change criteria"
    )]
    MissingWorkspaceEvidenceForCriterion { id: String },
    #[error(
        "GoalVerificationRecord references id `{id}` which does not exist in \
         GoalSpec.acceptance_criteria — verification records cannot attest to criteria \
         the spec never declared"
    )]
    UnknownVerificationCriterionId { id: String },
    #[error(
        "GoalCompletionReport.total_spent_micro_usd ({reported}) exceeds the spec's \
         hard budget cap ({cap}) — the doc rules at opencode.md:660-661 require a \
         budget overrun to transition to `Blocked {{ BudgetApprovalRequired }}`, never \
         `Completed`"
    )]
    BudgetSpendOverrun { reported: u64, cap: u64 },
    #[error(
        "GoalCompletionReport.elapsed_wall_clock_seconds ({reported}) exceeds the \
         spec's wall-clock cap ({cap}) — wall-clock overrun must transition the Goal \
         to `Blocked {{ WallClockExpired }}`, never `Completed`"
    )]
    BudgetWallClockOverrun { reported: u64, cap: u64 },
    #[error(
        "GoalCompletionReport.continuation_loops_used ({reported}) exceeds the spec's \
         max_continuation_loops cap ({cap}) — loop overrun must transition the Goal to \
         `Blocked {{ LoopLimitNeedsUser }}`, never `Completed`"
    )]
    BudgetLoopOverrun { reported: u32, cap: u32 },
}

/// Schema-layer shape gate run on every `Completed` envelope before the
/// state transitions to terminal `Completed`. Pinned by the doc's
/// "submit_goal_complete" rules at opencode.md:1474-1476 (`Completed`
/// is only emitted after the verifier accepts) and the budget /
/// evidence rules at opencode.md:660-661 + 677-680. This function
/// enforces the minimum invariants every verifier accept-path must
/// also produce, so a corrupted or forged JSONL stream that bypassed
/// the verifier is rejected at the resume seam.
///
/// Floors enforced:
///
/// 1. **Claimed-id sanity** — every entry in `report.completed_criteria`
///    is a real spec id; no duplicates.
/// 2. **Required-coverage** — every `required = true` criterion in
///    the spec appears in `report.completed_criteria`.
/// 3. **Standard-policy evidence floor** — under
///    [`GoalEvidencePolicy::Standard`], every required criterion has
///    at least one matching evidence ref; criteria with
///    [`super::spec::GoalCriterion::requires_workspace_change`] also
///    carry a workspace-bound `GoalEvidenceTarget::File` evidence.
///    `DocumentationOnly` policy relaxes both checks (the verifier
///    accepts human-written explanations in `verification`).
/// 4. **Verification-id sanity** — every verification record points
///    at a real spec id (no fabricated attestations).
/// 5. **Budget summary vs. spec caps** — reported spend / wall-clock /
///    loop counters do not exceed the spec's caps; an overrun must
///    have transitioned the Goal to `Blocked`, not `Completed`.
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
    // Verification record ids must be real spec ids — a forged
    // attestation that names a fabricated criterion would otherwise
    // sail through the floor.
    for record in &report.verification {
        if !spec_ids.contains(record.criterion_id.as_str()) {
            return Err(GoalCompletionShapeError::UnknownVerificationCriterionId {
                id: record.criterion_id.clone(),
            });
        }
    }
    // Standard policy mandates per-claimed-criterion evidence and
    // workspace evidence for workspace-change criteria
    // (opencode.md:677-680, mirrored in the doc comment on
    // `GoalEvidencePolicy::Standard`). DocumentationOnly relaxes
    // both checks: the verifier accepts narrative `verification`
    // records as evidence in lieu of structured refs.
    //
    // Iterate **claimed** criteria (not just `required` ones): an
    // optional criterion claimed in `completed_criteria` still
    // gets stamped into `state.completed_criteria` by `apply()`,
    // so its evidence floor must run too. The `required` flag
    // governs whether a criterion *must be claimed*, not what
    // evidence depth is required *when it is claimed* (Codex
    // pass-8 P2). Evidence refs whose target is the unknown
    // `GoalEvidenceTarget::Future` catch-all are excluded from
    // the count: the verifier cannot deterministically validate
    // an unknown target kind, so a forged ref of that variant
    // would otherwise satisfy a non-workspace floor (Codex pass-8
    // P1).
    if matches!(spec.evidence_policy, GoalEvidencePolicy::Standard) {
        let mut spec_by_id: std::collections::BTreeMap<&str, &super::spec::GoalCriterion> =
            std::collections::BTreeMap::new();
        for criterion in &spec.acceptance_criteria {
            spec_by_id.insert(criterion.id.as_str(), criterion);
        }
        for claimed_id in &report.completed_criteria {
            // `UnknownCriterionId` already rejected above; skip if
            // the lookup somehow fails (impossible after the prior
            // pass).
            let Some(criterion) = spec_by_id.get(claimed_id.as_str()) else {
                continue;
            };
            let matching_refs: Vec<&GoalEvidenceRef> = report
                .evidence_refs
                .iter()
                .filter(|r| r.criterion_id.as_deref() == Some(criterion.id.as_str()))
                .filter(|r| !matches!(r.target, GoalEvidenceTarget::Future))
                .collect();
            if matching_refs.is_empty() {
                return Err(
                    GoalCompletionShapeError::MissingEvidenceForRequiredCriterion {
                        id: criterion.id.clone(),
                    },
                );
            }
            if criterion.requires_workspace_change {
                // The verifier (P6.2) accepts either an actual
                // workspace mutation (`File` target with a hash
                // it can re-validate against disk) OR an explicit
                // `NoChangesNeeded` rationale (research / analysis
                // Goals where the right answer is "no change
                // required" — opencode.md:679). The schema floor
                // must be a strict subset of the verifier's
                // accept logic, so it accepts both targets here.
                // Anything else (ToolCall / Attachment / etc.) is
                // not workspace-bound and cannot stand in for VCS
                // state evidence.
                let has_workspace_ref = matching_refs.iter().any(|r| {
                    matches!(
                        r.target,
                        GoalEvidenceTarget::File { .. }
                            | GoalEvidenceTarget::NoChangesNeeded { .. }
                    )
                });
                if !has_workspace_ref {
                    return Err(
                        GoalCompletionShapeError::MissingWorkspaceEvidenceForCriterion {
                            id: criterion.id.clone(),
                        },
                    );
                }
            }
        }
    }
    // Budget summary cannot contradict the spec's caps. `0` on a cap
    // means "unset / unmetered"; only enforce the comparison when
    // the cap is non-zero. The doc forbids transitioning to
    // `Completed` once any cap is exhausted (opencode.md:660-667).
    if spec.budget.hard_cap_micro_usd > 0
        && report.total_spent_micro_usd > spec.budget.hard_cap_micro_usd
    {
        return Err(GoalCompletionShapeError::BudgetSpendOverrun {
            reported: report.total_spent_micro_usd,
            cap: spec.budget.hard_cap_micro_usd,
        });
    }
    if spec.budget.wall_clock_seconds > 0
        && report.elapsed_wall_clock_seconds > spec.budget.wall_clock_seconds
    {
        return Err(GoalCompletionShapeError::BudgetWallClockOverrun {
            reported: report.elapsed_wall_clock_seconds,
            cap: spec.budget.wall_clock_seconds,
        });
    }
    if spec.budget.max_continuation_loops > 0
        && report.continuation_loops_used > spec.budget.max_continuation_loops
    {
        return Err(GoalCompletionShapeError::BudgetLoopOverrun {
            reported: report.continuation_loops_used,
            cap: spec.budget.max_continuation_loops,
        });
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
    ///
    /// `claim_envelope_id` binds the rejection to the
    /// `CompletionClaimed` envelope it concerns, mirroring
    /// [`GoalCompletionReport::claim_envelope_id`]. `apply()` refuses
    /// the event when no pending claim is open or the binding does
    /// not match — a forged stream cannot otherwise clear an
    /// unrelated active claim or fabricate rejection blockers from
    /// thin air (Codex pass-9 P1).
    CompletionRejected {
        claim_envelope_id: Uuid,
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
                claim_envelope_id: Uuid::nil(),
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
        // Under Standard policy the floor demands matching evidence
        // for every required criterion. The fixture criteria are
        // authored without `requires_workspace_change`, so a
        // non-workspace evidence ref (a `ToolCall` here) suffices —
        // the floor's File-target requirement only kicks in for
        // workspace-change criteria.
        let evidence_refs = completed_criteria
            .iter()
            .map(|id| GoalEvidenceRef {
                criterion_id: Some(id.clone()),
                target: GoalEvidenceTarget::ToolCall {
                    call_id: format!("call-for-{id}"),
                },
                description: format!("evidence for {id}"),
            })
            .collect();
        GoalCompletionReport {
            summary: "shipped".to_string(),
            completed_criteria,
            evidence_refs,
            verification: vec![],
            residual_risks: vec![],
            changed_files: vec![],
            // Stable claim envelope id so shape-only tests (which
            // exercise the validator without involving `apply`)
            // round-trip without churn. The shape validator does
            // NOT inspect this field; it is checked exclusively
            // by `apply()`'s claim-binding gate.
            claim_envelope_id: Uuid::nil(),
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
            "claim_envelope_id": "00000000-0000-0000-0000-000000000000",
            "finalised_at": "2026-05-08T13:00:00Z",
            "finalised_by": {"kind":"user","id":null}
        }"#;
        let report: GoalCompletionReport = serde_json::from_str(json).expect("deserialize");
        assert_eq!(report.total_spent_micro_usd, 0);
        assert_eq!(report.elapsed_wall_clock_seconds, 0);
        assert_eq!(report.continuation_loops_used, 0);
    }

    /// Under `GoalEvidencePolicy::Standard` a required criterion
    /// without any matching evidence ref fails the floor — pinned
    /// by Codex pass-7 P1#2.
    #[test]
    fn shape_check_rejects_required_criterion_without_evidence() {
        let spec = shape_fixture_spec(vec![super::super::spec::GoalCriterion {
            id: "compiles".to_string(),
            description: "cargo check".to_string(),
            required: true,
            verifier_hint: None,
            requires_workspace_change: false,
        }]);
        let mut report = shape_fixture_report(vec!["compiles".to_string()]);
        report.evidence_refs.clear();
        let err = validate_completion_report_shape(&spec, &report)
            .expect_err("required criterion without evidence must fail");
        assert_eq!(
            err,
            GoalCompletionShapeError::MissingEvidenceForRequiredCriterion {
                id: "compiles".to_string(),
            },
        );
    }

    /// A required criterion marked `requires_workspace_change = true`
    /// needs a `GoalEvidenceTarget::File` evidence ref, not just
    /// any evidence — pinned by Codex pass-7 P1#2.
    #[test]
    fn shape_check_rejects_workspace_change_criterion_without_file_target() {
        let spec = shape_fixture_spec(vec![super::super::spec::GoalCriterion {
            id: "patch-applied".to_string(),
            description: "the source file was edited".to_string(),
            required: true,
            verifier_hint: None,
            requires_workspace_change: true,
        }]);
        // ToolCall evidence is non-workspace — must not satisfy a
        // workspace-change criterion.
        let report = shape_fixture_report(vec!["patch-applied".to_string()]);
        let err = validate_completion_report_shape(&spec, &report)
            .expect_err("workspace-change criterion without File evidence must fail");
        assert_eq!(
            err,
            GoalCompletionShapeError::MissingWorkspaceEvidenceForCriterion {
                id: "patch-applied".to_string(),
            },
        );
    }

    /// A workspace-change criterion accompanied by a `File` evidence
    /// passes the floor — confirms the symmetric happy path.
    #[test]
    fn shape_check_accepts_workspace_change_criterion_with_file_evidence() {
        let spec = shape_fixture_spec(vec![super::super::spec::GoalCriterion {
            id: "patch-applied".to_string(),
            description: "the source file was edited".to_string(),
            required: true,
            verifier_hint: None,
            requires_workspace_change: true,
        }]);
        let report = GoalCompletionReport {
            summary: "shipped".to_string(),
            completed_criteria: vec!["patch-applied".to_string()],
            evidence_refs: vec![GoalEvidenceRef {
                criterion_id: Some("patch-applied".to_string()),
                target: GoalEvidenceTarget::File {
                    path: "src/feature.rs".to_string(),
                    sha256: "deadbeef".to_string(),
                },
                description: "edit landed".to_string(),
            }],
            verification: vec![],
            residual_risks: vec![],
            changed_files: vec!["src/feature.rs".to_string()],
            claim_envelope_id: Uuid::nil(),
            total_spent_micro_usd: 0,
            elapsed_wall_clock_seconds: 0,
            continuation_loops_used: 0,
            finalised_at: fixture_now(),
            finalised_by: GoalActor::System {
                reason: "verifier accepted".to_string(),
            },
        };
        assert!(validate_completion_report_shape(&spec, &report).is_ok());
    }

    /// A workspace-change criterion accompanied only by a
    /// `NoChangesNeeded` rationale also satisfies the floor — the
    /// verifier (P6.2) accepts the explicit "no change required"
    /// escape hatch (opencode.md:679), and the schema gate must be
    /// a strict subset of the verifier's accept logic.
    #[test]
    fn shape_check_accepts_workspace_change_criterion_with_no_changes_needed_evidence() {
        let spec = shape_fixture_spec(vec![super::super::spec::GoalCriterion {
            id: "investigation".to_string(),
            description: "research-only criterion".to_string(),
            required: true,
            verifier_hint: None,
            requires_workspace_change: true,
        }]);
        let report = GoalCompletionReport {
            summary: "no change required".to_string(),
            completed_criteria: vec!["investigation".to_string()],
            evidence_refs: vec![GoalEvidenceRef {
                criterion_id: Some("investigation".to_string()),
                target: GoalEvidenceTarget::NoChangesNeeded {
                    rationale: "spec already correct".to_string(),
                },
                description: "research outcome".to_string(),
            }],
            verification: vec![],
            residual_risks: vec![],
            changed_files: vec![],
            claim_envelope_id: Uuid::nil(),
            total_spent_micro_usd: 0,
            elapsed_wall_clock_seconds: 0,
            continuation_loops_used: 0,
            finalised_at: fixture_now(),
            finalised_by: GoalActor::System {
                reason: "verifier accepted".to_string(),
            },
        };
        assert!(validate_completion_report_shape(&spec, &report).is_ok());
    }

    /// `DocumentationOnly` policy relaxes both the per-required
    /// evidence requirement and the workspace-change check. Pinned
    /// by the doc on `GoalEvidencePolicy::DocumentationOnly`.
    #[test]
    fn shape_check_documentation_only_policy_relaxes_evidence_floor() {
        let mut spec = shape_fixture_spec(vec![super::super::spec::GoalCriterion {
            id: "decision-recorded".to_string(),
            description: "ADR drafted".to_string(),
            required: true,
            verifier_hint: None,
            requires_workspace_change: true,
        }]);
        spec.evidence_policy = super::super::spec::GoalEvidencePolicy::DocumentationOnly;
        // No evidence_refs at all — under DocumentationOnly the
        // floor accepts.
        let report = GoalCompletionReport {
            summary: "research note attached in verification".to_string(),
            completed_criteria: vec!["decision-recorded".to_string()],
            evidence_refs: vec![],
            verification: vec![],
            residual_risks: vec![],
            changed_files: vec![],
            claim_envelope_id: Uuid::nil(),
            total_spent_micro_usd: 0,
            elapsed_wall_clock_seconds: 0,
            continuation_loops_used: 0,
            finalised_at: fixture_now(),
            finalised_by: GoalActor::System {
                reason: "verifier accepted".to_string(),
            },
        };
        assert!(validate_completion_report_shape(&spec, &report).is_ok());
    }

    /// A verification record naming a criterion id the spec never
    /// declared is rejected — pinned by Codex pass-7 P1#3 (lighter
    /// schema-floor variant).
    #[test]
    fn shape_check_rejects_verification_with_unknown_criterion_id() {
        let spec = shape_fixture_spec(vec![super::super::spec::GoalCriterion {
            id: "compiles".to_string(),
            description: "cargo check".to_string(),
            required: true,
            verifier_hint: None,
            requires_workspace_change: false,
        }]);
        let mut report = shape_fixture_report(vec!["compiles".to_string()]);
        report.verification.push(GoalVerificationRecord {
            criterion_id: "fabricated-id".to_string(),
            method: "manual".to_string(),
            passed: true,
            output_summary: None,
        });
        let err = validate_completion_report_shape(&spec, &report)
            .expect_err("verification with fabricated id must fail");
        assert_eq!(
            err,
            GoalCompletionShapeError::UnknownVerificationCriterionId {
                id: "fabricated-id".to_string(),
            },
        );
    }

    /// Reported total spend exceeding the spec's hard cap is
    /// rejected — pinned by Codex pass-7 P1#4. The doc forbids
    /// transitioning to `Completed` once a budget cap is exhausted.
    #[test]
    fn shape_check_rejects_completed_report_with_budget_overrun() {
        let mut spec = shape_fixture_spec(vec![super::super::spec::GoalCriterion {
            id: "compiles".to_string(),
            description: "cargo check".to_string(),
            required: true,
            verifier_hint: None,
            requires_workspace_change: false,
        }]);
        spec.budget = super::super::spec::GoalBudget {
            hard_cap_micro_usd: 1_000_000,
            warn_threshold_micro_usd: 500_000,
            wall_clock_seconds: 0,
            max_continuation_loops: 0,
        };
        let mut report = shape_fixture_report(vec!["compiles".to_string()]);
        report.total_spent_micro_usd = 2_000_000;
        let err = validate_completion_report_shape(&spec, &report)
            .expect_err("budget overrun in Completed report must fail");
        assert_eq!(
            err,
            GoalCompletionShapeError::BudgetSpendOverrun {
                reported: 2_000_000,
                cap: 1_000_000,
            },
        );
    }

    /// Reported wall-clock exceeding the spec's cap is rejected.
    #[test]
    fn shape_check_rejects_completed_report_with_wall_clock_overrun() {
        let mut spec = shape_fixture_spec(vec![super::super::spec::GoalCriterion {
            id: "compiles".to_string(),
            description: "cargo check".to_string(),
            required: true,
            verifier_hint: None,
            requires_workspace_change: false,
        }]);
        spec.budget = super::super::spec::GoalBudget {
            hard_cap_micro_usd: 0,
            warn_threshold_micro_usd: 0,
            wall_clock_seconds: 600,
            max_continuation_loops: 0,
        };
        let mut report = shape_fixture_report(vec!["compiles".to_string()]);
        report.elapsed_wall_clock_seconds = 1_200;
        let err = validate_completion_report_shape(&spec, &report)
            .expect_err("wall-clock overrun in Completed report must fail");
        assert_eq!(
            err,
            GoalCompletionShapeError::BudgetWallClockOverrun {
                reported: 1_200,
                cap: 600,
            },
        );
    }

    /// Reported continuation loops exceeding the spec's cap is
    /// rejected.
    #[test]
    fn shape_check_rejects_completed_report_with_loop_overrun() {
        let mut spec = shape_fixture_spec(vec![super::super::spec::GoalCriterion {
            id: "compiles".to_string(),
            description: "cargo check".to_string(),
            required: true,
            verifier_hint: None,
            requires_workspace_change: false,
        }]);
        spec.budget = super::super::spec::GoalBudget {
            hard_cap_micro_usd: 0,
            warn_threshold_micro_usd: 0,
            wall_clock_seconds: 0,
            max_continuation_loops: 8,
        };
        let mut report = shape_fixture_report(vec!["compiles".to_string()]);
        report.continuation_loops_used = 32;
        let err = validate_completion_report_shape(&spec, &report)
            .expect_err("loop overrun in Completed report must fail");
        assert_eq!(
            err,
            GoalCompletionShapeError::BudgetLoopOverrun {
                reported: 32,
                cap: 8,
            },
        );
    }

    /// `0` on a budget cap means "unmetered / unset"; the floor must
    /// not enforce against a zero cap. Otherwise every Goal with
    /// the default `GoalBudget` (all-zero) would refuse a non-zero
    /// reported spend.
    #[test]
    fn shape_check_skips_budget_caps_when_unset() {
        let spec = shape_fixture_spec(vec![super::super::spec::GoalCriterion {
            id: "compiles".to_string(),
            description: "cargo check".to_string(),
            required: true,
            verifier_hint: None,
            requires_workspace_change: false,
        }]);
        // Default GoalBudget has every cap = 0 except
        // max_continuation_loops = 16, so set something > 16 only
        // to confirm we *do* still enforce non-zero caps.
        let mut report = shape_fixture_report(vec!["compiles".to_string()]);
        report.total_spent_micro_usd = 999_999_999;
        report.elapsed_wall_clock_seconds = 999_999;
        // Stay at or below the default loop cap of 16.
        report.continuation_loops_used = 8;
        assert!(validate_completion_report_shape(&spec, &report).is_ok());
    }

    #[test]
    fn goal_completion_shape_error_display_pins_each_variant() {
        assert_eq!(
            GoalCompletionShapeError::UnknownCriterionId {
                id: "compiles".to_string(),
            }
            .to_string(),
            "GoalCompletionReport.completed_criteria contains id `compiles` which does not \
             exist in GoalSpec.acceptance_criteria — the verifier (P6.2) cannot reconcile a \
             claim against a criterion the spec does not declare",
        );
        assert_eq!(
            GoalCompletionShapeError::MissingRequiredCriterion {
                id: "tests-pass".to_string(),
            }
            .to_string(),
            "GoalCompletionReport omits required criterion `tests-pass` — every criterion \
             with `required = true` must appear in `completed_criteria` before a Goal can \
             transition to `Completed`",
        );
        assert_eq!(
            GoalCompletionShapeError::DuplicateClaimedCriterion {
                id: "compiles".to_string(),
            }
            .to_string(),
            "GoalCompletionReport.completed_criteria contains duplicate id `compiles` — the \
             verifier keys completion off a `BTreeSet<String>` so duplicates would let one \
             claim satisfy multiple required criteria",
        );
        assert_eq!(
            GoalCompletionShapeError::MissingEvidenceForRequiredCriterion {
                id: "tests-pass".to_string(),
            }
            .to_string(),
            "required criterion `tests-pass` has no matching evidence ref under \
             GoalEvidencePolicy::Standard — every required criterion must be backed by at \
             least one `GoalEvidenceRef` whose `criterion_id` equals the criterion's id",
        );
        assert_eq!(
            GoalCompletionShapeError::MissingWorkspaceEvidenceForCriterion {
                id: "edits-file".to_string(),
            }
            .to_string(),
            "required criterion `edits-file` is marked `requires_workspace_change = true` but \
             no matching evidence ref carries a workspace-bound target \
             (`GoalEvidenceTarget::File`) — Standard policy mandates VCS state evidence for \
             workspace-change criteria",
        );
        assert_eq!(
            GoalCompletionShapeError::UnknownVerificationCriterionId {
                id: "non-existent".to_string(),
            }
            .to_string(),
            "GoalVerificationRecord references id `non-existent` which does not exist in \
             GoalSpec.acceptance_criteria — verification records cannot attest to criteria \
             the spec never declared",
        );
        assert_eq!(
            GoalCompletionShapeError::BudgetSpendOverrun {
                reported: 6_000_000,
                cap: 5_000_000,
            }
            .to_string(),
            "GoalCompletionReport.total_spent_micro_usd (6000000) exceeds the spec's hard \
             budget cap (5000000) — the doc rules at opencode.md:660-661 require a budget \
             overrun to transition to `Blocked { BudgetApprovalRequired }`, never `Completed`",
        );
        assert_eq!(
            GoalCompletionShapeError::BudgetWallClockOverrun {
                reported: 3_600,
                cap: 1_800,
            }
            .to_string(),
            "GoalCompletionReport.elapsed_wall_clock_seconds (3600) exceeds the spec's \
             wall-clock cap (1800) — wall-clock overrun must transition the Goal to \
             `Blocked { WallClockExpired }`, never `Completed`",
        );
        assert_eq!(
            GoalCompletionShapeError::BudgetLoopOverrun {
                reported: 24,
                cap: 16,
            }
            .to_string(),
            "GoalCompletionReport.continuation_loops_used (24) exceeds the spec's \
             max_continuation_loops cap (16) — loop overrun must transition the Goal to \
             `Blocked { LoopLimitNeedsUser }`, never `Completed`",
        );
    }
}
