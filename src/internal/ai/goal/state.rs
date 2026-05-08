//! Goal state — replayable projection of the event stream.
//!
//! Per `docs/improvement/opencode.md` lines 567-576, [`GoalState`] is the
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
//! # Status semantics (from `docs/improvement/opencode.md` 557-564)
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

use super::{
    event::{
        GoalBlockReason, GoalCompletionClaim, GoalCompletionReport, GoalEvent, GoalEventEnvelope,
    },
    spec::GoalSpec,
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
    /// answer is "no change required"). The verifier accepts this
    /// as evidence in lieu of a `git status` artefact when the
    /// matching criterion's `evidence_policy` permits it.
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
/// criterion. Mirrors `docs/improvement/opencode.md` line 617's
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
    /// rule from `docs/improvement/opencode.md` line 597.
    #[serde(default)]
    pub requested_input: Option<String>,
}

/// Snapshot of a Goal at a point in event-stream time.
///
/// Always derived from a [`GoalSpec`] + an event sequence — never
/// constructed standalone. The supervisor (P6.3) holds at most one
/// `GoalState` per session.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GoalState {
    pub spec: GoalSpec,
    pub status: GoalStatus,
    #[serde(default)]
    pub plan: Vec<GoalPlanStep>,
    pub completed_criteria: BTreeSet<String>,
    #[serde(default)]
    pub evidence_refs: Vec<GoalEvidenceRef>,
    #[serde(default)]
    pub blockers: Vec<GoalBlocker>,
    #[serde(default)]
    pub last_assistant_summary: Option<String>,
    /// Most recent unverified completion claim. Populated when the
    /// model invokes `submit_goal_complete` and cleared when the
    /// claim is either accepted (-> `completion_report`) or
    /// rejected (-> rolled back). The deterministic verifier (P6.2)
    /// reads this directly so a `--resume` can pick up a pending
    /// verification without re-running the model.
    #[serde(default)]
    pub pending_claim: Option<GoalCompletionClaim>,
    /// Final completion report once `status == Completed`.
    #[serde(default)]
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

/// Apply one envelope to `state`. Idempotent only when applied to the
/// state it produced — calling `apply` twice with the same envelope
/// against the same input state is **not** safe (e.g. `PlanUpdated`
/// would re-bind the plan, dropping intermediate progress).
///
/// On success returns `true` and advances `state.updated_at` to
/// `envelope.recorded_at`. On rejection returns `false` and leaves
/// `state` byte-for-byte unchanged — including `updated_at`, so a
/// caller comparing snapshots can distinguish a real mutation from
/// a rejected envelope without a second timestamp source.
///
/// Returns `false` in any of these documented cases:
///
/// - **Cross-goal envelope** — `envelope.goal_id` does not match the
///   spec's `goal_id`. Protects against a misrouted JSONL entry.
/// - **Terminal-state guard** — `state.status` is already terminal
///   (`Completed` or `Cancelled`). A late-arriving event from a racy
///   supervisor or a replayed-twice log cannot walk a finished Goal
///   back into `Running`.
/// - **Invalid `CriteriaRevised`** — the embedded criteria list
///   fails [`super::spec::validate_criteria`] (duplicate or blank
///   id). The verifier keys completion off
///   `completed_criteria: BTreeSet<String>`, so a duplicate id
///   would let one claim satisfy multiple required criteria.
/// - **`GoalEvent::Future`** — an unknown future variant from a
///   newer Libra version that the current binary cannot interpret.
///
/// In every `false`-returning case the caller (typically the
/// supervisor's replay loop) is expected to log the gap and proceed.
pub fn apply(state: &mut GoalState, envelope: &GoalEventEnvelope) -> bool {
    if envelope.goal_id != state.spec.goal_id {
        // Cross-Goal envelope; ignore. This protects against a misrouted
        // session JSONL entry from corrupting an unrelated Goal's state.
        return false;
    }
    // Terminal-state guard: once a Goal hits `Completed` or
    // `Cancelled`, no subsequent event in the same JSONL slice may
    // reanimate it. The doc's "terminal boundary" semantics (line
    // 665) require this so a late-arriving event from a racy
    // supervisor (or a corrupted log replayed twice) cannot
    // surreptitiously walk a cancelled Goal back into `Running`.
    if state.status.is_terminal() {
        return false;
    }
    let applied = match &envelope.event {
        GoalEvent::Created(_) => {
            // `from_spec` already seeded the state; receiving a second
            // Created for the same goal_id is a no-op (replay safety).
            true
        }
        GoalEvent::CriteriaRevised { criteria, .. } => {
            // Validate the revised list with the same rules
            // `GoalSpec::new` enforces on construction — duplicate
            // or blank ids would let a single completion claim
            // satisfy multiple required criteria, which the
            // verifier (P6.2) cannot detect from
            // `completed_criteria: BTreeSet<String>`. Returning
            // `false` from here leaves `state` byte-for-byte
            // untouched (including `updated_at`).
            if super::spec::validate_criteria(criteria).is_err() {
                return false;
            }
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
            true
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
            true
        }
        GoalEvent::StepStarted { step_id } => {
            promote_step(&mut state.plan, step_id, GoalStepStatus::InProgress);
            state.status = GoalStatus::Running;
            true
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
            true
        }
        GoalEvent::ProgressRecorded(record) => {
            for crit in &record.completed_criteria {
                state.completed_criteria.insert(crit.clone());
            }
            state.evidence_refs.extend(record.evidence_refs.clone());
            state.last_assistant_summary = Some(record.summary.clone());
            true
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
            true
        }
        GoalEvent::CompletionClaimed(claim) => {
            // Retain the full claim payload — the verifier (P6.2) reads
            // `pending_claim.verification` and `residual_risks`
            // directly. Without this, a `--resume` that lands on a
            // pending claim would have to re-prompt the model.
            state.pending_claim = Some(claim.clone());
            // Evidence accumulates immediately so the audit log shows
            // exactly what the model attached. Criteria, however,
            // are NOT stamped into `completed_criteria` until the
            // verifier accepts — a rejection rolls back without
            // having to remember which criteria the rejected claim
            // tried to introduce.
            state.evidence_refs.extend(claim.evidence_refs.clone());
            state.status = GoalStatus::CompletionClaimed;
            true
        }
        GoalEvent::CompletionRejected { missing, reason } => {
            // Verifier said no — drop the pending claim so future
            // events do not see stale rejected work as
            // "claimed". The accompanying blocker carries the
            // verifier's rejection reason for the TUI / continuation
            // prompt.
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
            true
        }
        GoalEvent::Completed(report) => {
            // Verifier accepted: NOW stamp the report's criteria
            // into `completed_criteria` (the deterministic set the
            // verifier itself produced after gating the claim).
            // Clear `pending_claim` since it has been resolved.
            for crit in &report.completed_criteria {
                state.completed_criteria.insert(crit.clone());
            }
            state.evidence_refs.extend(report.evidence_refs.clone());
            state.pending_claim = None;
            state.completion_report = Some(report.clone());
            state.status = GoalStatus::Completed;
            true
        }
        GoalEvent::Cancelled { .. } => {
            state.status = GoalStatus::Cancelled;
            true
        }
        GoalEvent::Future => {
            // Unknown future variant from a newer Libra version. Do
            // nothing and signal the gap to the caller; the supervisor
            // logs and proceeds.
            false
        }
    };
    if applied {
        // Only advance `updated_at` on success. Rejected envelopes
        // (cross-goal, terminal-state guard, invalid CriteriaRevised,
        // GoalEvent::Future) leave the timestamp untouched so a
        // snapshot diff is a faithful signal of "did the state
        // actually change".
        state.updated_at = envelope.recorded_at;
    }
    applied
}

/// Replay a sequence of envelopes against a freshly-seeded state.
///
/// The first envelope must be a [`GoalEvent::Created`] carrying the
/// spec; the function returns `None` if the sequence does not start
/// with one (a defensive check so a corrupted JSONL slice does not
/// silently produce a nonsense state).
pub fn replay<'a>(envelopes: impl IntoIterator<Item = &'a GoalEventEnvelope>) -> Option<GoalState> {
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
    let mut state = GoalState::from_spec(spec.clone());
    state.updated_at = first.recorded_at;
    for envelope in iter {
        apply(&mut state, envelope);
    }
    Some(state)
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
                },
                GoalCriterion {
                    id: "tests".to_string(),
                    description: "cargo test passes".to_string(),
                    required: true,
                    verifier_hint: None,
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
        let state = replay(envelopes.iter()).expect("replay must succeed");
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
        let mut state = GoalState::from_spec(spec);
        let other_goal = Uuid::new_v4();
        let env = envelope(
            other_goal,
            GoalEvent::StepStarted {
                step_id: "step-x".to_string(),
            },
        );
        let applied = apply(&mut state, &env);
        assert!(!applied, "cross-goal envelope must be ignored");
        assert!(state.plan.is_empty(), "state must not change");
    }

    #[test]
    fn unknown_future_variant_no_ops_and_signals_gap() {
        let spec = fixture_spec();
        let mut state = GoalState::from_spec(spec.clone());
        let env = envelope(spec.spec_goal_id_for_tests(), GoalEvent::Future);
        let applied = apply(&mut state, &env);
        assert!(!applied, "Future variant must signal semver gap to caller");
        assert_eq!(state.status, GoalStatus::Active);
    }

    #[test]
    fn completion_claim_then_rejection_keeps_goal_active() {
        let spec = fixture_spec();
        let goal_id = spec.spec_goal_id_for_tests();
        let mut state = GoalState::from_spec(spec);
        apply(
            &mut state,
            &envelope(
                goal_id,
                GoalEvent::CompletionClaimed(GoalCompletionClaim {
                    summary: "done".to_string(),
                    completed_criteria: vec!["compiles".to_string()],
                    evidence_refs: vec![],
                    verification: vec![],
                    residual_risks: vec![],
                }),
            ),
        );
        assert_eq!(state.status, GoalStatus::CompletionClaimed);
        apply(
            &mut state,
            &envelope(
                goal_id,
                GoalEvent::CompletionRejected {
                    missing: vec!["tests".to_string()],
                    reason: "no test evidence".to_string(),
                },
            ),
        );
        assert_eq!(state.status, GoalStatus::Active);
        assert_eq!(state.blockers.len(), 1);
    }

    #[test]
    fn completed_event_is_terminal_and_records_report() {
        let spec = fixture_spec();
        let goal_id = spec.spec_goal_id_for_tests();
        let mut state = GoalState::from_spec(spec);
        let report = GoalCompletionReport {
            summary: "shipped".to_string(),
            completed_criteria: vec!["compiles".to_string(), "tests".to_string()],
            evidence_refs: vec![],
            verification: vec![],
            residual_risks: vec![],
            changed_files: vec!["src/main.rs".to_string()],
            finalised_at: fixture_now(),
            finalised_by: GoalActor::System {
                reason: "deterministic verifier accepted".to_string(),
            },
        };
        apply(
            &mut state,
            &envelope(goal_id, GoalEvent::Completed(report.clone())),
        );
        assert_eq!(state.status, GoalStatus::Completed);
        assert!(state.status.is_terminal());
        assert_eq!(state.completion_report, Some(report));
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
        );
        assert_eq!(state.status, GoalStatus::Cancelled);
        assert!(state.status.is_terminal());
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
        );
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
        );
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
        );
        apply(
            &mut state,
            &envelope(
                goal_id,
                GoalEvent::StepCompleted {
                    step_id: "step-1".to_string(),
                    evidence_refs: vec![],
                },
            ),
        );
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
        );
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
