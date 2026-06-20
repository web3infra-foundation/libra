//! Goal state — replayable projection of the event stream.
//!
//! Per `docs/development/commands/_general.md` lines 567-576, [`GoalState`] is the
//! supervisor's view of an active Goal: spec, status, plan, completed
//! criteria, evidence refs, blockers, and the most recent assistant
//! summary. The state is **derived purely from the event stream** —
//! [`apply`] folds one event into a state, and [`replay`] folds the full
//! stream into a final state.
//!
//! No event ever clears state silently: every transition is explicit.
//! Unknown future variants deserialised as
//! [`super::event::GoalEvent::Future`] are no-ops (so old binaries
//! reading a newer JSONL stream do not panic) but the dispatcher does
//! NOT silently translate them into a known status. If an old binary
//! sees a future event that *would* have changed the status, it leaves
//! the status untouched — a conservative choice that surfaces the
//! semver gap (the user runs `--resume` and sees "Goal still active"
//! despite the new client having moved on).
//!
//! # Status semantics (from `docs/development/commands/_general.md` 557-564)
//!
//! | Status               | Meaning                                                |
//! |----------------------|--------------------------------------------------------|
//! | `Active`             | Goal exists; supervisor not currently in a tool loop   |
//! | `Running`            | Supervisor is inside `run_tool_loop`                   |
//! | `AwaitingUser`       | Paused for explicit user answer; resumes on input      |
//! | `Blocked`            | Recoverable blocker, see `blockers` for details        |
//! | `CompletionClaimed`  | Model called `submit_goal_complete`, verifier pending  |
//! | `Completed`          | Verifier accepted; terminal                            |
//! | `Cancelled`          | Explicit cancellation; terminal                        |
//!
//! Only `Completed` and `Cancelled` are terminal — everything else
//! either drives forward (`Active` / `Running`) or pauses recoverably
//! (`AwaitingUser` / `Blocked` / `CompletionClaimed`).

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    event::{
        GoalBlockReason, GoalCompletionClaim, GoalCompletionReport, GoalCompletionShapeError,
        GoalEvent, GoalEventEnvelope, validate_completion_claim_shape,
        validate_completion_report_shape,
    },
    spec::{GoalSpec, GoalSpecError},
};

/// Lifecycle status of a single Goal.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    /// Goal exists; the supervisor is between turns.
    Active,
    /// Supervisor is currently inside a tool loop iteration.
    Running,
    /// Supervisor parked waiting on explicit user input.
    AwaitingUser,
    /// Recoverable blocker; the user can take action and the
    /// supervisor will resume.
    Blocked,
    /// Model claimed completion via `submit_goal_complete`; the
    /// verifier has not yet decided.
    CompletionClaimed,
    /// Verifier accepted completion. Terminal.
    Completed,
    /// Explicit cancellation. Terminal.
    Cancelled,
}

impl GoalStatus {
    /// Whether the status is a terminal state (no further transitions).
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled)
    }
}

/// Status of one [`GoalPlanStep`] inside the live plan. The supervisor
/// drives transitions via `StepStarted` / `StepCompleted` events; a
/// `PlanUpdated` re-baselines the plan and resets steps to `Pending`
/// unless they were already `Completed` (so a replan does not undo
/// completed work — verifiers rely on that invariant).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GoalStepStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
    Skipped,
}

/// One ordered step in the supervisor's live plan. The plan is
/// regenerated on `PlanUpdated`; steps are not append-only.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GoalPlanStep {
    pub id: String,
    pub description: String,
    #[serde(default)]
    pub status: GoalStepStatus,
    /// Optional ids of acceptance criteria this step contributes to.
    /// The verifier consults this when deciding whether `Completed`
    /// criteria add up to the required set.
    #[serde(default)]
    pub criterion_ids: Vec<String>,
}

/// What an evidence ref points to. Forms a closed set — a foreign
/// pointer must be added as a new variant first so the verifier knows
/// how to interpret it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GoalEvidenceTarget {
    /// Reference to a [`crate::internal::ai::context_budget::ContextFrameEvent`].
    ContextFrame { event_id: String },
    /// Reference to a tool call by id (`ToolCall::id`). The verifier
    /// inspects the matching tool result for success/failure.
    ToolCall { call_id: String },
    /// Reference to a file at a specific content hash. The verifier
    /// checks that the file still has that hash on disk before
    /// accepting completion.
    File { path: String, sha256: String },
    /// Reference to an attachment in the session attachment store.
    Attachment { attachment_id: String },
    /// Reference to an `AgentRunEvent` produced by a sub-agent.
    /// Gated to the `subagent-scaffold` feature path; the verifier
    /// silently treats this as `Unrecognised` when the gate is off.
    AgentRun { event_id: String },
    /// Goal completed without any code/file change being necessary
    /// (e.g. a research-only or analysis-only Goal where the right
    /// answer is "no change required"). Per opencode.md:679 the
    /// verifier (P6.2) accepts this as evidence in lieu of a
    /// `git status` artefact for *any* criterion — including a
    /// `requires_workspace_change = true` criterion where the
    /// supervisor concluded after investigation that no edit was
    /// required. The schema floor and the verifier are aligned on
    /// this: a `NoChangesNeeded` ref satisfies the workspace
    /// evidence check.
    NoChangesNeeded { rationale: String },
    /// Forward-compatibility catch-all so an envelope carrying an
    /// evidence target shape we have not seen yet still replays
    /// cleanly. Cannot be constructed by hand; only emerges from
    /// serde when the `kind` discriminator is unknown.
    #[serde(other)]
    Future,
}

/// A piece of evidence supporting a completion claim. The supervisor
/// echoes a description into the JSONL stream so the audit log is
/// human-readable even when the underlying target is opaque.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GoalEvidenceRef {
    pub criterion_id: Option<String>,
    pub target: GoalEvidenceTarget,
    pub description: String,
}

/// What the verifier ran (or what the user attests to) to confirm a
/// criterion. Mirrors `docs/development/commands/_general.md` line 617's
/// `GoalVerificationRecord`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GoalVerificationRecord {
    pub criterion_id: String,
    /// Free-form description (`"cargo test --lib"`, `"manual review"`).
    pub method: String,
    /// `true` iff verification passed.
    pub passed: bool,
    /// Output excerpt or pointer to attachment with full output.
    #[serde(default)]
    pub output_summary: Option<String>,
}

/// A specific blocker the supervisor surfaced. Multiple blockers can
/// coexist (e.g. budget approval + provider unrecoverable); the
/// supervisor never silently drops one.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GoalBlocker {
    pub reason: GoalBlockReason,
    pub recorded_at: DateTime<Utc>,
    /// Optional concrete question shown to the user — single-question
    /// rule from `docs/development/commands/_general.md` line 597.
    #[serde(default)]
    pub requested_input: Option<String>,
}

/// The active completion claim plus the envelope that opened it,
/// bundled so the two values can never drift apart. The
/// `Completed` and `CompletionRejected` arms use `envelope_id` to
/// confirm the verifier's response refers to **this** claim, not an
/// unrelated one (Codex pass-8 P2 / pass-9 P1). Bundling at the type
/// level removes the previous `Option<...>` pair that an external
/// caller could split (Codex pass-9 P2).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PendingGoalClaim {
    /// Envelope id of the [`super::event::GoalEvent::CompletionClaimed`]
    /// that opened this claim.
    pub envelope_id: Uuid,
    /// The claim payload itself — passed through to P6.2 so the
    /// verifier reads `verification` and `residual_risks` directly,
    /// and a `--resume` can pick up the pending verification
    /// without re-prompting the model.
    pub claim: GoalCompletionClaim,
}

/// Snapshot of a Goal at a point in event-stream time.
///
/// Always derived from a [`GoalSpec`] + an event sequence — never
/// constructed standalone, and **never** rebuilt from arbitrary JSON.
/// The supervisor (P6.3) holds at most one `GoalState` per session
/// and rebuilds it via [`replay`] on resume; persisted snapshots are
/// advisory only (e.g. for debug dumps) and must NOT be trusted as a
/// completion-decision authority. The struct deliberately implements
/// `Serialize` (for snapshots / `Debug` rendering) but **not**
/// `Deserialize` — Codex pass-8 P2 flagged that a forged JSON
/// `GoalState` could pre-populate `pending_claim` /
/// `completion_report` / `status` and bypass every event-derived
/// guard. Forcing all state construction through `from_spec` +
/// `apply` / `replay` closes that surface at the type level.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GoalState {
    pub spec: GoalSpec,
    pub status: GoalStatus,
    pub plan: Vec<GoalPlanStep>,
    pub completed_criteria: BTreeSet<String>,
    pub evidence_refs: Vec<GoalEvidenceRef>,
    pub blockers: Vec<GoalBlocker>,
    pub last_assistant_summary: Option<String>,
    /// Most recent unverified completion claim with its opening
    /// envelope id. Populated when the model invokes
    /// `submit_goal_complete` and cleared when the claim is either
    /// accepted (-> `completion_report`) or rejected (->
    /// rolled back). The deterministic verifier (P6.2) reads this
    /// directly so a `--resume` can pick up a pending verification
    /// without re-running the model. Bundling the envelope id with
    /// the claim payload at the type level seals the
    /// "they always move together" invariant (Codex pass-9 P2).
    pub pending_claim: Option<PendingGoalClaim>,
    /// Final completion report once `status == Completed`.
    pub completion_report: Option<GoalCompletionReport>,
    pub updated_at: DateTime<Utc>,
}

impl GoalState {
    /// Build the initial `Active` state for a freshly-created Goal.
    pub fn from_spec(spec: GoalSpec) -> Self {
        let updated_at = spec.created_at;
        Self {
            spec,
            status: GoalStatus::Active,
            plan: Vec::new(),
            completed_criteria: BTreeSet::new(),
            evidence_refs: Vec::new(),
            blockers: Vec::new(),
            last_assistant_summary: None,
            pending_claim: None,
            completion_report: None,
            updated_at,
        }
    }
}

/// Schema-layer reasons [`apply`] refused to fold an envelope into
/// `state`. Pinned by the doc's "terminal boundary" / "cross-goal
/// guard" / "shape gate" rules (`docs/development/commands/_general.md` lines
/// 658-665, 1463-1467).
///
/// The supervisor's replay loop (P6.3) and the verifier (P6.2) consume
/// this enum to render concrete errors at the resume seam — a
/// previous bool-returning `apply` swallowed the *reason* a misrouted
/// or forged envelope was dropped, so a mixed-goal stream could look
/// successful to the caller (Codex pass-6 P2 finding).
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum GoalApplyReject {
    /// Envelope's `goal_id` does not match the state's spec's
    /// `goal_id`. Protects against a misrouted JSONL entry.
    #[error(
        "envelope goal_id {envelope_goal_id} does not match state spec goal_id \
         {state_goal_id} — misrouted envelope"
    )]
    CrossGoal {
        envelope_goal_id: Uuid,
        state_goal_id: Uuid,
    },
    /// `state.status` is already terminal (`Completed` / `Cancelled`).
    /// A late-arriving event must not reanimate a finished Goal.
    #[error(
        "state is already terminal ({status:?}); event ignored to preserve the terminal boundary"
    )]
    TerminalGuard { status: GoalStatus },
    /// `CriteriaRevised` payload failed
    /// [`super::spec::validate_criteria`].
    #[error("CriteriaRevised payload failed validation: {source}")]
    InvalidCriteriaRevised {
        #[source]
        source: GoalSpecError,
    },
    /// `Completed` payload failed
    /// [`super::event::validate_completion_report_shape`] — a
    /// forged JSONL stream tried to walk replay into terminal
    /// `Completed` without the verifier's accept path.
    #[error("Completed report does not match the spec: {source}")]
    InvalidCompletionReport {
        #[source]
        source: GoalCompletionShapeError,
    },
    /// `GoalEvent::Future` — an unknown future variant from a newer
    /// Libra version that the current binary cannot interpret.
    #[error("event uses a future variant this binary cannot apply (semver gap)")]
    UnknownFutureVariant,
    /// A `GoalEvent::Completed` envelope arrived while
    /// `state.pending_claim` was `None` — the doc state machine
    /// (opencode.md:1465-1467) requires `Created` →
    /// `CompletionClaimed` → verifier → `Completed`. A direct
    /// `Created` → `Completed` jump is always a forged stream or a
    /// corrupted log; the verifier could never have produced a
    /// report without a prior claim.
    #[error(
        "Completed envelope arrived without a prior CompletionClaimed — the doc state \
         machine forbids transitioning to terminal `Completed` without the verifier \
         (P6.2) accepting a pending claim"
    )]
    MissingCompletionClaim,
    /// A second `Created` envelope appeared after the seed Created
    /// already constructed the state. Replay seeds from the first
    /// envelope; any subsequent `Created` is conclusive evidence of
    /// a corrupted or forged stream (the supervisor only ever
    /// emits exactly one Created per Goal lifetime).
    #[error("duplicate Created envelope after the seed — only one Created is permitted per Goal")]
    DuplicateCreated,
    /// `report.claim_envelope_id` does not match the
    /// `pending_claim_envelope_id` recorded when the active claim
    /// was opened. A forged stream can only ship a report against
    /// the one claim the supervisor has open; a mismatch is
    /// conclusive evidence the verifier never accepted **this**
    /// claim (Codex pass-8 P2).
    #[error(
        "Completed report claim_envelope_id {actual} does not match the active pending claim's \
         envelope id {expected} — the verifier (P6.2) only ever emits a report bound to the \
         claim it just accepted"
    )]
    MismatchedClaimEnvelope { expected: Uuid, actual: Uuid },
    /// Successful envelope's `recorded_at` is older than
    /// `state.updated_at`, which would rewind the state's
    /// monotonic clock. Forged streams cannot reorder timestamps
    /// without surfacing here (Codex pass-8 P2).
    #[error(
        "envelope recorded_at {envelope_recorded_at} predates state updated_at \
         {state_updated_at} — Goal events must be monotonic on the timeline"
    )]
    TimestampNonMonotonic {
        envelope_recorded_at: DateTime<Utc>,
        state_updated_at: DateTime<Utc>,
    },
    /// `CompletionClaimed` payload failed
    /// [`super::event::validate_completion_claim_shape`] —
    /// duplicate / unknown criterion ids or fabricated
    /// verification / evidence cross-references would otherwise
    /// poison `state.pending_claim` and `state.evidence_refs`
    /// before P6.2 sees the claim (Codex pass-9 P1).
    #[error("CompletionClaimed payload failed validation: {source}")]
    InvalidCompletionClaim {
        #[source]
        source: GoalCompletionShapeError,
    },
    /// `CompletionRejected` arrived without an open pending
    /// claim. The doc state machine forbids a rejection without
    /// a prior `CompletionClaimed`; a forged stream cannot
    /// fabricate rejection blockers from thin air (Codex pass-9
    /// P1).
    #[error(
        "CompletionRejected envelope arrived without an open pending claim — \
         the verifier (P6.2) only ever rejects an active CompletionClaimed"
    )]
    RejectedWithoutPendingClaim,
}

/// Apply one envelope to `state`. Idempotent only when applied to the
/// state it produced — calling `apply` twice with the same envelope
/// against the same input state is **not** safe (e.g. `PlanUpdated`
/// would re-bind the plan, dropping intermediate progress).
///
/// On success returns `Ok(())` and advances `state.updated_at` to
/// `envelope.recorded_at`. On rejection returns
/// `Err(GoalApplyReject::...)` and leaves `state` byte-for-byte
/// unchanged — including `updated_at`, so a caller comparing
/// snapshots can distinguish a real mutation from a rejected envelope
/// without a second timestamp source.
///
/// # Guard order
///
/// Pre-mutation checks fire in this fixed order so a forged or
/// corrupted stream surfaces the *most specific* reason first:
///
/// 1. [`GoalApplyReject::CrossGoal`] — `envelope.goal_id` mismatch.
/// 2. [`GoalApplyReject::TerminalGuard`] — `state.status` already
///    terminal.
/// 3. [`GoalApplyReject::TimestampNonMonotonic`] — successful
///    envelopes must have `recorded_at >= state.updated_at`.
/// 4. Per-event guards (run only inside the matching arm):
///    - [`GoalApplyReject::DuplicateCreated`] for any post-seed
///      `Created`.
///    - [`GoalApplyReject::InvalidCriteriaRevised`] for a malformed
///      `CriteriaRevised` payload.
///    - [`GoalApplyReject::InvalidCompletionClaim`] for a malformed
///      `CompletionClaimed` payload.
///    - [`GoalApplyReject::RejectedWithoutPendingClaim`] +
///      [`GoalApplyReject::MismatchedClaimEnvelope`] for a stray /
///      misbound `CompletionRejected`.
///    - [`GoalApplyReject::MissingCompletionClaim`] +
///      [`GoalApplyReject::MismatchedClaimEnvelope`] +
///      [`GoalApplyReject::InvalidCompletionReport`] for a stray /
///      misbound / shape-bad `Completed`.
///    - [`GoalApplyReject::UnknownFutureVariant`] for any unknown
///      `kind` from a newer Libra version.
///
/// In every `Err`-returning case the caller (typically the
/// supervisor's replay loop) is expected to log the gap and proceed.
pub fn apply(state: &mut GoalState, envelope: &GoalEventEnvelope) -> Result<(), GoalApplyReject> {
    if envelope.goal_id != state.spec.goal_id {
        // Cross-Goal envelope; ignore. This protects against a misrouted
        // session JSONL entry from corrupting an unrelated Goal's state.
        return Err(GoalApplyReject::CrossGoal {
            envelope_goal_id: envelope.goal_id,
            state_goal_id: state.spec.goal_id,
        });
    }
    // Terminal-state guard: once a Goal hits `Completed` or
    // `Cancelled`, no subsequent event in the same JSONL slice may
    // reanimate it. The doc's "terminal boundary" semantics (line
    // 665) require this so a late-arriving event from a racy
    // supervisor (or a corrupted log replayed twice) cannot
    // surreptitiously walk a cancelled Goal back into `Running`.
    if state.status.is_terminal() {
        return Err(GoalApplyReject::TerminalGuard {
            status: state.status,
        });
    }
    // Monotonic-time guard: a successful envelope must have a
    // `recorded_at` >= the current `state.updated_at`. A forged
    // stream that ships an envelope with an older timestamp would
    // otherwise rewind the state's clock once the mutation lands,
    // muddying snapshot diffs and audit ordering (Codex pass-8 P2).
    // Equality (envelope.recorded_at == state.updated_at) is
    // permitted: events emitted in the same instant by the
    // supervisor are legitimate (e.g. the seed Created sets
    // updated_at = spec.created_at, and a same-tick PlanUpdated
    // can follow).
    if envelope.recorded_at < state.updated_at {
        return Err(GoalApplyReject::TimestampNonMonotonic {
            envelope_recorded_at: envelope.recorded_at,
            state_updated_at: state.updated_at,
        });
    }
    match &envelope.event {
        GoalEvent::Created(_) => {
            // `from_spec` already seeded the state from the first
            // envelope at the replay seam. A *second* Created
            // envelope is conclusive evidence of a corrupted or
            // forged stream — the supervisor only ever writes one
            // Created per Goal lifetime, and silently no-op'ing the
            // dupe (the prior behavior) advanced `updated_at` to a
            // timestamp that did not correspond to a real state
            // mutation, masking the corruption.
            return Err(GoalApplyReject::DuplicateCreated);
        }
        GoalEvent::CriteriaRevised { criteria, .. } => {
            // Validate the revised list with the same rules
            // `GoalSpec::new` enforces on construction — duplicate
            // or blank ids would let a single completion claim
            // satisfy multiple required criteria, which the
            // verifier (P6.2) cannot detect from
            // `completed_criteria: BTreeSet<String>`. Surfacing
            // the reason via `Err` lets the supervisor's replay
            // loop log "criterion id `x` blank/duplicate" instead
            // of a generic "envelope dropped".
            super::spec::validate_criteria(criteria)
                .map_err(|source| GoalApplyReject::InvalidCriteriaRevised { source })?;
            // Replace the spec's acceptance criteria — replay sees
            // the most recent revision as the source of truth. Any
            // criterion already present in `completed_criteria` but
            // missing from the revised list is dropped (the user
            // explicitly removed it from scope). We keep evidence
            // refs because they are factual records of what
            // happened, not declarative scope.
            state.spec.acceptance_criteria = criteria.clone();
            let revised_ids: std::collections::HashSet<&str> =
                criteria.iter().map(|c| c.id.as_str()).collect();
            state
                .completed_criteria
                .retain(|id| revised_ids.contains(id.as_str()));
        }
        GoalEvent::PlanUpdated { steps } => {
            // A replan keeps any step that was already completed — the
            // new plan must respect prior progress.
            let prior_completed: std::collections::HashSet<&str> = state
                .plan
                .iter()
                .filter(|s| s.status == GoalStepStatus::Completed)
                .map(|s| s.id.as_str())
                .collect();
            state.plan = steps
                .iter()
                .map(|step| {
                    if prior_completed.contains(step.id.as_str()) {
                        GoalPlanStep {
                            status: GoalStepStatus::Completed,
                            ..step.clone()
                        }
                    } else {
                        step.clone()
                    }
                })
                .collect();
        }
        GoalEvent::StepStarted { step_id } => {
            promote_step(&mut state.plan, step_id, GoalStepStatus::InProgress);
            state.status = GoalStatus::Running;
        }
        GoalEvent::StepCompleted {
            step_id,
            evidence_refs,
        } => {
            promote_step(&mut state.plan, step_id, GoalStepStatus::Completed);
            state.evidence_refs.extend(evidence_refs.clone());
            // A step finishing is *not* a terminal — keep the
            // supervisor's status at `Running` (loop in progress) or
            // bring it back to `Active` if no other step is running.
            let still_running = state
                .plan
                .iter()
                .any(|s| s.status == GoalStepStatus::InProgress);
            state.status = if still_running {
                GoalStatus::Running
            } else {
                GoalStatus::Active
            };
        }
        GoalEvent::ProgressRecorded(record) => {
            for crit in &record.completed_criteria {
                state.completed_criteria.insert(crit.clone());
            }
            state.evidence_refs.extend(record.evidence_refs.clone());
            state.last_assistant_summary = Some(record.summary.clone());
        }
        GoalEvent::Blocked {
            reason,
            requested_input,
        } => {
            state.blockers.push(GoalBlocker {
                reason: reason.clone(),
                recorded_at: envelope.recorded_at,
                requested_input: requested_input.clone(),
            });
            state.status = match reason {
                // `AwaitingScopeChange` is a special case: it
                // semantically means "waiting on the user", not
                // "blocked by external state". Surface it as
                // `AwaitingUser` so the TUI can render the right
                // affordance (single question + reply).
                GoalBlockReason::AwaitingScopeChange { .. } => GoalStatus::AwaitingUser,
                _ => GoalStatus::Blocked,
            };
        }
        GoalEvent::CompletionClaimed(claim) => {
            // Schema-layer claim shape gate: a forged stream that
            // ships a `CompletionClaimed` with duplicate / unknown
            // criterion ids or fabricated verification / evidence
            // cross-references would otherwise poison
            // `pending_claim` and `state.evidence_refs` before
            // P6.2 ever sees the claim. The verifier still owns
            // accept/reject; the floor only refuses payloads that
            // could not have come from any verifier-emitted accept
            // path (Codex pass-9 P1).
            validate_completion_claim_shape(&state.spec, claim)
                .map_err(|source| GoalApplyReject::InvalidCompletionClaim { source })?;
            // Retain the full claim payload alongside its
            // opening envelope id — bundled so they cannot drift
            // apart. The verifier (P6.2) reads
            // `pending_claim.claim.verification` /
            // `residual_risks` directly; the
            // `pending_claim.envelope_id` is checked by the
            // `Completed` / `CompletionRejected` arms to bind the
            // verifier's response to **this** claim.
            state.pending_claim = Some(PendingGoalClaim {
                envelope_id: envelope.envelope_id,
                claim: claim.clone(),
            });
            // Evidence accumulates immediately so the audit log shows
            // exactly what the model attached. Criteria, however,
            // are NOT stamped into `completed_criteria` until the
            // verifier accepts — a rejection rolls back without
            // having to remember which criteria the rejected claim
            // tried to introduce.
            state.evidence_refs.extend(claim.evidence_refs.clone());
            state.status = GoalStatus::CompletionClaimed;
        }
        GoalEvent::CompletionRejected {
            claim_envelope_id,
            missing,
            reason,
        } => {
            // State-machine guard: rejection must reference an
            // open pending claim. A forged stream cannot
            // fabricate a rejection blocker out of thin air
            // (Codex pass-9 P1).
            let Some(pending) = state.pending_claim.as_ref() else {
                return Err(GoalApplyReject::RejectedWithoutPendingClaim);
            };
            // Bind the rejection to the right claim — same
            // protection `Completed` already enjoys via
            // `MismatchedClaimEnvelope`.
            if pending.envelope_id != *claim_envelope_id {
                return Err(GoalApplyReject::MismatchedClaimEnvelope {
                    expected: pending.envelope_id,
                    actual: *claim_envelope_id,
                });
            }
            // Verifier said no — drop the pending claim so future
            // events do not see stale rejected work as "claimed".
            // The accompanying blocker carries the verifier's
            // rejection reason for the TUI / continuation prompt.
            state.pending_claim = None;
            state.blockers.push(GoalBlocker {
                reason: GoalBlockReason::CompletionRejected {
                    missing: missing.clone(),
                    reason: reason.clone(),
                },
                recorded_at: envelope.recorded_at,
                requested_input: None,
            });
            state.status = GoalStatus::Active;
        }
        GoalEvent::Completed(report) => {
            // State-machine guard: the doc's supervisor flow
            // (opencode.md:1465-1467) always passes through
            // `Created -> ... -> CompletionClaimed -> verifier ->
            // Completed`. A Completed envelope arriving while
            // `pending_claim` is `None` is conclusive evidence
            // the verifier never ran — a forged stream cannot
            // dodge this gate by emitting a well-formed report
            // because there is no claim for the verifier to have
            // accepted. Closing this here means a `--resume`
            // landing on a corrupt stream surfaces the gap as
            // `MissingCompletionClaim` instead of silently
            // transitioning the Goal to terminal.
            // Pattern-match on the bundled pending claim — if
            // there is no open claim, the doc state machine is
            // violated (Codex pass-7 P1). Otherwise extract the
            // bound envelope id directly; bundling at the type
            // level removes the `Option<envelope_id>` pair the
            // previous design required (Codex pass-9 P2).
            let Some(pending) = state.pending_claim.as_ref() else {
                return Err(GoalApplyReject::MissingCompletionClaim);
            };
            // Claim binding: refuse any report that names a
            // *different* claim — a forged stream cannot claim
            // under one envelope and ship a report against an
            // unrelated active claim (Codex pass-8 P2).
            if report.claim_envelope_id != pending.envelope_id {
                return Err(GoalApplyReject::MismatchedClaimEnvelope {
                    expected: pending.envelope_id,
                    actual: report.claim_envelope_id,
                });
            }
            // Schema-layer shape floor: even with a bound pending
            // claim, a forged Completed report must satisfy the
            // same invariants every verifier accept-path produces
            // (claimed-id sanity, required coverage, evidence
            // floor under Standard policy, verification id
            // sanity, budget vs. spec caps). The verifier (P6.2)
            // does the rich semantic check on top.
            validate_completion_report_shape(&state.spec, report)
                .map_err(|source| GoalApplyReject::InvalidCompletionReport { source })?;
            // Verifier-equivalent shape passed: NOW stamp the
            // report's criteria into `completed_criteria` (the
            // deterministic set the verifier itself produced after
            // gating the claim). Clear `pending_claim` since it has
            // been resolved.
            for crit in &report.completed_criteria {
                state.completed_criteria.insert(crit.clone());
            }
            state.evidence_refs.extend(report.evidence_refs.clone());
            state.pending_claim = None;
            state.completion_report = Some(report.clone());
            state.status = GoalStatus::Completed;
        }
        GoalEvent::Cancelled { .. } => {
            // Drop any open pending claim — P6.2/P6.3 read
            // `pending_claim` directly to decide whether the
            // verifier still has work, and a terminal Cancelled
            // must not look like "verifier in flight" on resume
            // (Codex pass-10 P1).
            state.pending_claim = None;
            state.status = GoalStatus::Cancelled;
        }
        GoalEvent::Future => {
            // Unknown future variant from a newer Libra version. Do
            // nothing and signal the gap to the caller; the supervisor
            // logs and proceeds.
            return Err(GoalApplyReject::UnknownFutureVariant);
        }
    }
    // Only advance `updated_at` on success. Rejected envelopes
    // (cross-goal, terminal-state guard, invalid CriteriaRevised,
    // invalid Completed report, GoalEvent::Future) leave the
    // timestamp untouched so a snapshot diff is a faithful signal
    // of "did the state actually change".
    state.updated_at = envelope.recorded_at;
    Ok(())
}

/// One envelope rejected during [`replay`], paired with the reason
/// [`apply`] refused to fold it. Surfaces concatenated mixed-goal
/// streams, late events past a terminal boundary, malformed
/// `CriteriaRevised` payloads, and forged `Completed` reports as
/// concrete diagnostics rather than the previous bool-returning
/// silence (Codex pass-6 P2 finding).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalReplayRejection {
    /// `envelope_id` of the offending envelope (NOT the goal id).
    pub envelope_id: Uuid,
    /// Why [`apply`] refused this envelope.
    pub reason: GoalApplyReject,
}

/// Hard cap on the number of [`GoalReplayRejection`] entries an
/// [`GoalReplayOutcome`] retains. Beyond this threshold the rejection
/// detail is dropped on the floor and only the count is kept (in
/// [`GoalReplayOutcome::truncated_rejection_count`]). The cap exists
/// so a forged or corrupted JSONL stream containing thousands of
/// cross-goal / future / terminal-guard envelopes cannot turn replay
/// diagnostics into an unbounded memory sink.
pub const MAX_REPLAY_REJECTIONS: usize = 64;

/// Outcome of replaying a sequence of envelopes against a freshly
/// seeded state. Carries both the projected `state` and the list of
/// per-envelope rejections [`apply`] surfaced. The supervisor's
/// resume seam (P6.3) renders `rejected` to the user / audit log so a
/// concatenated mixed-goal stream cannot look successful.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalReplayOutcome {
    /// The projected state after folding every envelope (rejected
    /// ones excepted) into the `Created` seed.
    pub state: GoalState,
    /// Envelopes [`apply`] refused, in the order they appeared,
    /// truncated to at most [`MAX_REPLAY_REJECTIONS`] entries. May
    /// be empty for a clean replay.
    pub rejected: Vec<GoalReplayRejection>,
    /// Number of rejection entries dropped on the floor because the
    /// `rejected` cap was reached. The supervisor surfaces this as
    /// "and {n} more" so a forged stream's full size is still
    /// observable without retaining every diagnostic.
    pub truncated_rejection_count: usize,
}

/// Replay a sequence of envelopes against a freshly-seeded state.
///
/// The first envelope must be a [`GoalEvent::Created`] carrying the
/// spec; the function returns `None` if the sequence does not start
/// with one, or if the embedded spec's `goal_id` does not match the
/// envelope's `goal_id`, or if the spec fails
/// [`GoalSpec::validate`]. These three checks are defensive against
/// a corrupted JSONL slice that would otherwise silently produce a
/// nonsense state at the supervisor's resume seam.
///
/// On success returns `Some(GoalReplayOutcome)` carrying the
/// projected `state` plus a [`GoalReplayRejection`] entry for every
/// envelope [`apply`] refused — the supervisor logs those gaps
/// instead of swallowing them.
pub fn replay<'a>(
    envelopes: impl IntoIterator<Item = &'a GoalEventEnvelope>,
) -> Option<GoalReplayOutcome> {
    let mut iter = envelopes.into_iter();
    let first = iter.next()?;
    let GoalEvent::Created(spec) = &first.event else {
        return None;
    };
    // Cross-goal sanity check: the envelope's `goal_id` must match
    // the embedded spec's `goal_id`. A misrouted or corrupted log
    // could ship a `Created` envelope whose envelope id points to
    // one Goal but whose payload describes another. Without this
    // check, [`from_spec`] would seed state for the *spec's* goal,
    // and every subsequent envelope (which the caller filtered for
    // the envelope id) would silently fail the cross-goal guard
    // inside [`apply`] — losing all post-Created progress with no
    // observable signal. Returning `None` makes the failure
    // surface immediately at the supervisor's resume seam.
    if first.goal_id != spec.goal_id {
        return None;
    }
    // Re-validate the deserialized spec the same way `GoalSpec::new`
    // does at construction. Without this, a corrupted JSONL stream
    // (or a future attacker forging a session log) could ship a
    // `Created` payload with duplicate / blank criterion ids or an
    // empty objective, bypassing the shape rules the verifier
    // (P6.2) depends on. Returning `None` surfaces the malformed
    // input at the supervisor's resume seam.
    if spec.validate().is_err() {
        return None;
    }
    let mut state = GoalState::from_spec(spec.clone());
    state.updated_at = first.recorded_at;
    let mut rejected: Vec<GoalReplayRejection> = Vec::new();
    let mut truncated_rejection_count: usize = 0;
    for envelope in iter {
        if let Err(reason) = apply(&mut state, envelope) {
            if rejected.len() < MAX_REPLAY_REJECTIONS {
                rejected.push(GoalReplayRejection {
                    envelope_id: envelope.envelope_id,
                    reason,
                });
            } else {
                truncated_rejection_count = truncated_rejection_count.saturating_add(1);
            }
        }
    }
    Some(GoalReplayOutcome {
        state,
        rejected,
        truncated_rejection_count,
    })
}

fn promote_step(plan: &mut Vec<GoalPlanStep>, step_id: &str, target: GoalStepStatus) {
    for step in plan.iter_mut() {
        if step.id == step_id {
            // Never demote a `Completed` step. The verifier relies on
            // completed steps staying completed across replans.
            if step.status == GoalStepStatus::Completed && target != GoalStepStatus::Completed {
                continue;
            }
            step.status = target;
            return;
        }
    }
    // Step id absent from the current plan: this happens when the
    // supervisor records a step it manages out-of-band (e.g. a
    // freeform "investigate" step that was never planned). Append a
    // synthetic step so the timeline still records the transition.
    plan.push(GoalPlanStep {
        id: step_id.to_string(),
        description: String::new(),
        status: target,
        criterion_ids: Vec::new(),
    });
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;
    use crate::internal::ai::goal::{
        event::{GoalCompletionClaim, GoalCompletionReport, GoalProgressRecord},
        spec::{GoalActor, GoalBudget, GoalCriterion, GoalEvidencePolicy},
    };

    fn fixture_now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-05-08T13:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn fixture_spec() -> GoalSpec {
        GoalSpec::new(
            Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap(),
            "thread-1",
            "session-1",
            "deliver feature X",
            vec![
                GoalCriterion {
                    id: "compiles".to_string(),
                    description: "cargo check passes".to_string(),
                    required: true,
                    verifier_hint: None,
                    requires_workspace_change: true,
                },
                GoalCriterion {
                    id: "tests".to_string(),
                    description: "cargo test passes".to_string(),
                    required: true,
                    verifier_hint: None,
                    requires_workspace_change: true,
                },
            ],
            Vec::new(),
            GoalEvidencePolicy::Standard,
            GoalBudget::default(),
            fixture_now(),
            GoalActor::User { id: None },
        )
        .expect("happy-path spec must construct")
    }

    fn envelope(goal_id: Uuid, event: GoalEvent) -> GoalEventEnvelope {
        GoalEventEnvelope::new(goal_id, fixture_now(), event)
    }

    #[test]
    fn replay_starts_from_created_event_and_drives_plan_updates() {
        let spec = fixture_spec();
        let goal_id = spec.spec_goal_id_for_tests();
        let envelopes = [
            envelope(goal_id, GoalEvent::Created(spec.clone())),
            envelope(
                goal_id,
                GoalEvent::PlanUpdated {
                    steps: vec![GoalPlanStep {
                        id: "step-1".to_string(),
                        description: "run cargo check".to_string(),
                        status: GoalStepStatus::Pending,
                        criterion_ids: vec!["compiles".to_string()],
                    }],
                },
            ),
            envelope(
                goal_id,
                GoalEvent::StepStarted {
                    step_id: "step-1".to_string(),
                },
            ),
            envelope(
                goal_id,
                GoalEvent::StepCompleted {
                    step_id: "step-1".to_string(),
                    evidence_refs: vec![GoalEvidenceRef {
                        criterion_id: Some("compiles".to_string()),
                        target: GoalEvidenceTarget::ToolCall {
                            call_id: "tool-1".to_string(),
                        },
                        description: "cargo check passed".to_string(),
                    }],
                },
            ),
        ];
        let outcome = replay(envelopes.iter()).expect("replay must succeed");
        let state = outcome.state;
        assert!(outcome.rejected.is_empty(), "no envelopes must be rejected");
        assert_eq!(state.status, GoalStatus::Active);
        assert_eq!(state.plan.len(), 1);
        assert_eq!(state.plan[0].status, GoalStepStatus::Completed);
        assert_eq!(state.evidence_refs.len(), 1);
    }

    #[test]
    fn replay_returns_none_when_first_event_is_not_created() {
        let spec = fixture_spec();
        let goal_id = spec.spec_goal_id_for_tests();
        let envelopes = [envelope(goal_id, GoalEvent::PlanUpdated { steps: vec![] })];
        assert!(replay(envelopes.iter()).is_none());
    }

    #[test]
    fn cross_goal_envelopes_are_ignored() {
        let spec = fixture_spec();
        let state_goal_id = spec.spec_goal_id_for_tests();
        let mut state = GoalState::from_spec(spec);
        let other_goal = Uuid::new_v4();
        let env = envelope(
            other_goal,
            GoalEvent::StepStarted {
                step_id: "step-x".to_string(),
            },
        );
        let result = apply(&mut state, &env);
        assert_eq!(
            result,
            Err(GoalApplyReject::CrossGoal {
                envelope_goal_id: other_goal,
                state_goal_id,
            }),
            "cross-goal envelope must surface CrossGoal reject reason",
        );
        assert!(state.plan.is_empty(), "state must not change");
    }

    #[test]
    fn unknown_future_variant_no_ops_and_signals_gap() {
        let spec = fixture_spec();
        let mut state = GoalState::from_spec(spec.clone());
        let env = envelope(spec.spec_goal_id_for_tests(), GoalEvent::Future);
        let result = apply(&mut state, &env);
        assert_eq!(
            result,
            Err(GoalApplyReject::UnknownFutureVariant),
            "Future variant must signal semver gap via UnknownFutureVariant",
        );
        assert_eq!(state.status, GoalStatus::Active);
    }

    #[test]
    fn completion_claim_then_rejection_keeps_goal_active() {
        let spec = fixture_spec();
        let goal_id = spec.spec_goal_id_for_tests();
        let mut state = GoalState::from_spec(spec);
        let claim_envelope_id = fixture_claim_envelope_id();
        apply(
            &mut state,
            &fixture_claim_envelope(
                goal_id,
                GoalCompletionClaim {
                    summary: "done".to_string(),
                    completed_criteria: vec!["compiles".to_string()],
                    evidence_refs: vec![],
                    verification: vec![],
                    residual_risks: vec![],
                },
            ),
        )
        .expect("CompletionClaimed apply must succeed");
        assert_eq!(state.status, GoalStatus::CompletionClaimed);
        apply(
            &mut state,
            &envelope(
                goal_id,
                GoalEvent::CompletionRejected {
                    claim_envelope_id,
                    missing: vec!["tests".to_string()],
                    reason: "no test evidence".to_string(),
                },
            ),
        )
        .expect("CompletionRejected apply must succeed");
        assert_eq!(state.status, GoalStatus::Active);
        assert_eq!(state.blockers.len(), 1);
    }

    /// Stable claim-envelope id reused across the legitimate
    /// claim / report fixtures so the test can construct a
    /// `CompletionClaimed` envelope with this id and a matching
    /// `Completed` report whose `claim_envelope_id` agrees.
    fn fixture_claim_envelope_id() -> Uuid {
        Uuid::parse_str("00000000-0000-0000-0000-0000c1a10000").unwrap()
    }

    /// Build the canonical claim + report pair the fixture spec
    /// expects: both required criteria claimed, each backed by a
    /// `File` evidence (the fixture marks both
    /// `requires_workspace_change = true`), plus a passed
    /// verification per criterion. The report's
    /// `claim_envelope_id` is bound to
    /// [`fixture_claim_envelope_id`] so the caller can pre-claim
    /// with that envelope id and the binding check passes.
    fn fixture_legitimate_claim_and_report() -> (GoalCompletionClaim, GoalCompletionReport) {
        let evidence_refs = vec![
            GoalEvidenceRef {
                criterion_id: Some("compiles".to_string()),
                target: GoalEvidenceTarget::File {
                    path: "src/main.rs".to_string(),
                    sha256: "deadbeef".to_string(),
                },
                description: "edit compiles".to_string(),
            },
            GoalEvidenceRef {
                criterion_id: Some("tests".to_string()),
                target: GoalEvidenceTarget::File {
                    path: "tests/feature.rs".to_string(),
                    sha256: "cafef00d".to_string(),
                },
                description: "test landed".to_string(),
            },
        ];
        let verification = vec![
            GoalVerificationRecord {
                criterion_id: "compiles".to_string(),
                method: "cargo check".to_string(),
                passed: true,
                output_summary: Some("clean".to_string()),
            },
            GoalVerificationRecord {
                criterion_id: "tests".to_string(),
                method: "cargo test --lib".to_string(),
                passed: true,
                output_summary: Some("ok".to_string()),
            },
        ];
        let claim = GoalCompletionClaim {
            summary: "all green".to_string(),
            completed_criteria: vec!["compiles".to_string(), "tests".to_string()],
            evidence_refs: evidence_refs.clone(),
            verification: verification.clone(),
            residual_risks: vec![],
        };
        let report = GoalCompletionReport {
            summary: "shipped".to_string(),
            completed_criteria: vec!["compiles".to_string(), "tests".to_string()],
            evidence_refs,
            verification,
            residual_risks: vec![],
            changed_files: vec!["src/main.rs".to_string()],
            claim_envelope_id: fixture_claim_envelope_id(),
            total_spent_micro_usd: 1_500_000,
            elapsed_wall_clock_seconds: 1_200,
            continuation_loops_used: 4,
            finalised_at: fixture_now(),
            finalised_by: GoalActor::System {
                reason: "deterministic verifier accepted".to_string(),
            },
        };
        (claim, report)
    }

    /// Construct a `CompletionClaimed` envelope whose
    /// `envelope_id` matches [`fixture_claim_envelope_id`] so the
    /// `Completed` arm's claim-binding check accepts a matching
    /// report.
    fn fixture_claim_envelope(goal_id: Uuid, claim: GoalCompletionClaim) -> GoalEventEnvelope {
        GoalEventEnvelope {
            envelope_id: fixture_claim_envelope_id(),
            goal_id,
            recorded_at: fixture_now(),
            event: GoalEvent::CompletionClaimed(claim),
        }
    }

    #[test]
    fn completed_event_is_terminal_and_records_report() {
        let spec = fixture_spec();
        let goal_id = spec.spec_goal_id_for_tests();
        let mut state = GoalState::from_spec(spec);
        let (claim, report) = fixture_legitimate_claim_and_report();
        apply(&mut state, &fixture_claim_envelope(goal_id, claim))
            .expect("CompletionClaimed must apply before Completed");
        apply(
            &mut state,
            &envelope(goal_id, GoalEvent::Completed(report.clone())),
        )
        .expect("well-formed Completed envelope must apply");
        assert_eq!(state.status, GoalStatus::Completed);
        assert!(state.status.is_terminal());
        assert_eq!(state.completion_report, Some(report));
    }

    /// A `Completed` envelope whose report omits a required spec
    /// criterion is refused at the schema-layer floor — the
    /// verifier's accept-path always emits a report covering every
    /// required criterion, so a forged JSONL stream that bypassed
    /// the verifier surfaces here as
    /// `GoalApplyReject::InvalidCompletionReport` instead of
    /// silently transitioning to terminal `Completed`.
    ///
    /// The test pre-claims with a valid claim (so
    /// `pending_claim` is `Some`, isolating the shape gate) and
    /// then submits a forged Completed report.
    #[test]
    fn completed_event_with_missing_required_criterion_is_rejected() {
        let spec = fixture_spec();
        let goal_id = spec.spec_goal_id_for_tests();
        let mut state = GoalState::from_spec(spec);
        let (claim, _) = fixture_legitimate_claim_and_report();
        apply(&mut state, &fixture_claim_envelope(goal_id, claim))
            .expect("legitimate claim must seed pending_claim");
        let bogus_report = GoalCompletionReport {
            summary: "forged".to_string(),
            completed_criteria: vec!["compiles".to_string()], // missing "tests"
            evidence_refs: vec![],
            verification: vec![],
            residual_risks: vec![],
            changed_files: vec![],
            // Bind to the pending claim's envelope id so the
            // binding gate passes — we want the shape gate to be
            // the one that fires.
            claim_envelope_id: fixture_claim_envelope_id(),
            total_spent_micro_usd: 0,
            elapsed_wall_clock_seconds: 0,
            continuation_loops_used: 0,
            finalised_at: fixture_now(),
            finalised_by: GoalActor::System {
                reason: "forged log".to_string(),
            },
        };
        let result = apply(
            &mut state,
            &envelope(goal_id, GoalEvent::Completed(bogus_report)),
        );
        match result {
            Err(GoalApplyReject::InvalidCompletionReport { source }) => {
                assert_eq!(
                    source,
                    GoalCompletionShapeError::MissingRequiredCriterion {
                        id: "tests".to_string(),
                    }
                );
            }
            other => panic!("expected InvalidCompletionReport, got {other:?}"),
        }
        assert_eq!(
            state.status,
            GoalStatus::CompletionClaimed,
            "rejected Completed must NOT transition the Goal to terminal — \
             the pending claim remains visible to the supervisor",
        );
        assert!(state.completion_report.is_none());
    }

    /// A `Completed` envelope arriving with `pending_claim = None`
    /// (no prior `CompletionClaimed`) is rejected with
    /// `MissingCompletionClaim` — the doc state machine forbids a
    /// direct `Created -> Completed` jump.
    #[test]
    fn completed_event_without_pending_claim_is_rejected() {
        let spec = fixture_spec();
        let goal_id = spec.spec_goal_id_for_tests();
        let mut state = GoalState::from_spec(spec);
        let (_, report) = fixture_legitimate_claim_and_report();
        let result = apply(&mut state, &envelope(goal_id, GoalEvent::Completed(report)));
        assert_eq!(
            result,
            Err(GoalApplyReject::MissingCompletionClaim),
            "Completed without a prior CompletionClaimed must surface \
             MissingCompletionClaim, not transition to terminal",
        );
        assert_eq!(state.status, GoalStatus::Active);
        assert!(state.completion_report.is_none());
    }

    /// A second `Created` envelope after the seed surfaces as
    /// `DuplicateCreated` — the supervisor only ever writes one
    /// Created per Goal, so duplicates are conclusive evidence of
    /// a forged or corrupted stream.
    #[test]
    fn duplicate_created_envelope_is_rejected() {
        let spec = fixture_spec();
        let goal_id = spec.spec_goal_id_for_tests();
        let baseline_ts = spec.created_at;
        let mut state = GoalState::from_spec(spec.clone());
        // A duplicate Created arriving later must NOT no-op silently.
        let later = chrono::DateTime::parse_from_rfc3339("2099-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let dup = GoalEventEnvelope::new(goal_id, later, GoalEvent::Created(spec));
        let result = apply(&mut state, &dup);
        assert_eq!(result, Err(GoalApplyReject::DuplicateCreated));
        assert_eq!(
            state.updated_at, baseline_ts,
            "DuplicateCreated must NOT advance updated_at",
        );
    }

    #[test]
    fn cancelled_event_is_terminal() {
        let spec = fixture_spec();
        let goal_id = spec.spec_goal_id_for_tests();
        let mut state = GoalState::from_spec(spec);
        apply(
            &mut state,
            &envelope(
                goal_id,
                GoalEvent::Cancelled {
                    reason: "user pressed Ctrl-C".to_string(),
                    cancelled_by: GoalActor::User { id: None },
                },
            ),
        )
        .expect("Cancelled apply must succeed");
        assert_eq!(state.status, GoalStatus::Cancelled);
        assert!(state.status.is_terminal());
    }

    /// Codex pass-10 P1: a `Cancelled` event arriving while a claim
    /// is in flight must clear `pending_claim` so a snapshot reader
    /// (P6.2 / P6.3) cannot see "Cancelled but with pending
    /// verification work".
    #[test]
    fn cancelled_clears_pending_claim() {
        let spec = fixture_spec();
        let goal_id = spec.spec_goal_id_for_tests();
        let mut state = GoalState::from_spec(spec);
        let (claim, _) = fixture_legitimate_claim_and_report();
        apply(&mut state, &fixture_claim_envelope(goal_id, claim))
            .expect("CompletionClaimed must seed pending_claim");
        assert!(state.pending_claim.is_some());
        apply(
            &mut state,
            &envelope(
                goal_id,
                GoalEvent::Cancelled {
                    reason: "user changed their mind mid-claim".to_string(),
                    cancelled_by: GoalActor::User { id: None },
                },
            ),
        )
        .expect("Cancelled apply must succeed");
        assert_eq!(state.status, GoalStatus::Cancelled);
        assert!(
            state.pending_claim.is_none(),
            "Cancelled must drop the open pending claim",
        );
    }

    #[test]
    fn awaiting_scope_change_routes_to_awaiting_user_status() {
        let spec = fixture_spec();
        let goal_id = spec.spec_goal_id_for_tests();
        let mut state = GoalState::from_spec(spec);
        apply(
            &mut state,
            &envelope(
                goal_id,
                GoalEvent::Blocked {
                    reason: GoalBlockReason::AwaitingScopeChange {
                        question: "Which file should I edit first?".to_string(),
                    },
                    requested_input: Some("Which file should I edit first?".to_string()),
                },
            ),
        )
        .expect("Blocked apply must succeed");
        assert_eq!(state.status, GoalStatus::AwaitingUser);
        assert_eq!(state.blockers.len(), 1);
    }

    #[test]
    fn progress_record_accumulates_summary_and_criteria() {
        let spec = fixture_spec();
        let goal_id = spec.spec_goal_id_for_tests();
        let mut state = GoalState::from_spec(spec);
        apply(
            &mut state,
            &envelope(
                goal_id,
                GoalEvent::ProgressRecorded(GoalProgressRecord {
                    summary: "halfway there".to_string(),
                    completed_criteria: vec!["compiles".to_string()],
                    evidence_refs: vec![],
                    next_steps: vec!["wire the test".to_string()],
                }),
            ),
        )
        .expect("ProgressRecorded apply must succeed");
        assert_eq!(
            state.last_assistant_summary,
            Some("halfway there".to_string())
        );
        assert!(state.completed_criteria.contains("compiles"));
    }

    /// `PlanUpdated` keeps prior `Completed` step status — replans
    /// must not undo verified progress.
    #[test]
    fn plan_updated_preserves_completed_step_status() {
        let spec = fixture_spec();
        let goal_id = spec.spec_goal_id_for_tests();
        let mut state = GoalState::from_spec(spec);
        // Initial plan + complete step-1.
        apply(
            &mut state,
            &envelope(
                goal_id,
                GoalEvent::PlanUpdated {
                    steps: vec![
                        GoalPlanStep {
                            id: "step-1".to_string(),
                            description: "first".to_string(),
                            status: GoalStepStatus::Pending,
                            criterion_ids: vec![],
                        },
                        GoalPlanStep {
                            id: "step-2".to_string(),
                            description: "second".to_string(),
                            status: GoalStepStatus::Pending,
                            criterion_ids: vec![],
                        },
                    ],
                },
            ),
        )
        .expect("initial PlanUpdated apply must succeed");
        apply(
            &mut state,
            &envelope(
                goal_id,
                GoalEvent::StepCompleted {
                    step_id: "step-1".to_string(),
                    evidence_refs: vec![],
                },
            ),
        )
        .expect("StepCompleted apply must succeed");
        // Replan: same step ids, all back to Pending in the new plan.
        apply(
            &mut state,
            &envelope(
                goal_id,
                GoalEvent::PlanUpdated {
                    steps: vec![
                        GoalPlanStep {
                            id: "step-1".to_string(),
                            description: "first (revised)".to_string(),
                            status: GoalStepStatus::Pending,
                            criterion_ids: vec![],
                        },
                        GoalPlanStep {
                            id: "step-3".to_string(),
                            description: "third".to_string(),
                            status: GoalStepStatus::Pending,
                            criterion_ids: vec![],
                        },
                    ],
                },
            ),
        )
        .expect("replan PlanUpdated apply must succeed");
        let step1 = state
            .plan
            .iter()
            .find(|s| s.id == "step-1")
            .expect("step-1 in plan");
        assert_eq!(
            step1.status,
            GoalStepStatus::Completed,
            "PlanUpdated must preserve Completed status from prior plan"
        );
    }

    /// Test helper: pull the goal_id out of a GoalSpec without taking
    /// a public dependency on `spec.goal_id` (the field is `pub` for
    /// runtime use; the helper documents that the test is pulling the
    /// id deliberately).
    impl GoalSpec {
        fn spec_goal_id_for_tests(&self) -> Uuid {
            self.goal_id
        }
    }
}
