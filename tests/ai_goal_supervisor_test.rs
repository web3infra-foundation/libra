//! OC-Phase 6 P6.7 — S6 supervisor non-completion E2E.
//!
//! Spec: `docs/improvement/opencode.md` lines 1706-1722.
//!
//! S6 walks the supervisor through three turns:
//!
//! 1. The model emits a final text without calling
//!    `submit_goal_complete`. The supervisor MUST NOT let the
//!    session idle: it appends `GoalEvent::ProgressRecorded` and
//!    re-enters the tool loop with a continuation prompt that
//!    surfaces the missing required criteria.
//! 2. The model calls `submit_goal_complete` with **incomplete**
//!    evidence. The verifier rejects, the supervisor appends
//!    `GoalEvent::CompletionClaimed` followed by
//!    `GoalEvent::CompletionRejected`, and the loop continues with
//!    a fresh continuation prompt that lists what was missing.
//! 3. The model retries with full evidence + verification. The
//!    verifier accepts; the supervisor appends `Completed` and
//!    returns `GoalLoopDecision::Completed { report }`. The Goal
//!    enters terminal `Completed`.
//!
//! Plus a variant that hits the supervisor's continuation-loop cap
//! and lands on `Blocked { LoopLimitNeedsUser }` (non-terminal),
//! pinning the doc invariant that "a budget exhaustion never
//! marks a Goal complete".
//!
//! Driving the supervisor directly (without `run_tool_loop`) is
//! sufficient for S6: the supervisor's `step()` is pure on
//! `(state, outcome, ctx, clock)`, so the test substitutes each
//! turn's `GoalTurnOutcome` and asserts the events + decision the
//! supervisor produces. The `run_tool_loop` integration that
//! actually invokes the model + tool handlers is owned by
//! P6.5/P6.6 wiring and lands incrementally.

use std::cell::Cell;

use chrono::{DateTime, Duration, TimeZone, Utc};
use libra::internal::ai::goal::{
    DefaultGoalContinuationPromptBuilder, DeterministicGoalVerifier, GoalActor, GoalApplyReject,
    GoalBlockReason, GoalBudget, GoalCompletionClaim, GoalCompletionShapeError, GoalCriterion,
    GoalEvent, GoalEventClock, GoalEventEnvelope, GoalEvidencePolicy, GoalEvidenceRef,
    GoalEvidenceTarget, GoalLoopDecision, GoalSpec, GoalState, GoalStatus, GoalStopPolicy,
    GoalSupervisor, GoalSupervisorStep, GoalTurnOutcome, GoalVerificationRecord,
    GoalVerifierContext, RecentToolCall, apply,
};
use uuid::Uuid;

fn fixture_now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 5, 9, 13, 0, 0).unwrap()
}

/// Spec for "Add unit test for utils::path::join" — taken verbatim
/// from the S6 doc input. Two required criteria so the verifier
/// has something to check, and `requires_workspace_change=true`
/// so the workspace evidence floor matches the doc's
/// implementation-class Goal description.
fn s6_spec() -> GoalSpec {
    GoalSpec::new(
        Uuid::parse_str("00000000-0000-0000-0000-0000000056e6").unwrap(),
        "thread-s6",
        "session-s6",
        "Add unit test for utils::path::join".to_string(),
        vec![
            GoalCriterion {
                id: "test-added".to_string(),
                description: "new test in tests/path_join_test.rs".to_string(),
                required: true,
                verifier_hint: Some("cargo test path_join".to_string()),
                requires_workspace_change: true,
            },
            GoalCriterion {
                id: "tests-pass".to_string(),
                description: "cargo test --lib green after the edit".to_string(),
                required: true,
                verifier_hint: Some("cargo test --lib".to_string()),
                requires_workspace_change: false,
            },
        ],
        Vec::new(),
        GoalEvidencePolicy::Standard,
        GoalBudget {
            // Loop cap pinned at 3 so the variant test can exhaust
            // it deterministically. The S6 happy path uses 3 turns
            // and never exceeds the cap.
            max_continuation_loops: 3,
            ..GoalBudget::default()
        },
        fixture_now(),
        GoalActor::User { id: None },
    )
    .expect("S6 spec must construct")
}

/// Deterministic supervisor clock — emits a stable sequence of
/// envelope ids and an advancing wall-clock so the supervisor's
/// emitted events pass `apply()`'s monotonic-time guard while
/// staying byte-stable.
struct FixedClock {
    next: Cell<u128>,
}

impl FixedClock {
    fn new() -> Self {
        Self { next: Cell::new(1) }
    }
}

impl GoalEventClock for FixedClock {
    fn mint_envelope_id(&self) -> Uuid {
        let n = self.next.get();
        self.next.set(n + 1);
        Uuid::from_u128(n)
    }

    fn now(&self) -> DateTime<Utc> {
        let n = self.next.get();
        fixture_now() + Duration::seconds(n as i64)
    }
}

/// Verifier ctx fixture — workspace-change criterion satisfied,
/// no failed tool calls, budget metering plausible. The S6 happy
/// path's third turn flows through this ctx.
struct AcceptingCtx;

impl GoalVerifierContext for AcceptingCtx {
    fn file_sha256(&self, path: &str) -> Option<String> {
        match path {
            "tests/path_join_test.rs" => Some("deadbeef".to_string()),
            _ => None,
        }
    }

    fn recent_tool_results(&self) -> Vec<RecentToolCall> {
        Vec::new()
    }

    fn changed_files(&self) -> Vec<String> {
        vec!["tests/path_join_test.rs".to_string()]
    }

    fn now(&self) -> DateTime<Utc> {
        fixture_now() + Duration::seconds(900)
    }

    fn finalised_by(&self) -> GoalActor {
        GoalActor::System {
            reason: "deterministic verifier accepted".to_string(),
        }
    }

    fn total_spent_micro_usd(&self) -> u64 {
        750_000
    }

    fn elapsed_wall_clock_seconds(&self) -> u64 {
        900
    }

    fn continuation_loops_used(&self) -> u32 {
        2
    }
}

fn fixture_supervisor()
-> GoalSupervisor<DeterministicGoalVerifier, DefaultGoalContinuationPromptBuilder> {
    GoalSupervisor {
        stop_policy: GoalStopPolicy::GoalBound {
            goal_id: s6_spec().goal_id,
        },
        verifier: DeterministicGoalVerifier,
        prompt_builder: DefaultGoalContinuationPromptBuilder,
    }
}

/// Apply every envelope a supervisor step emitted to the live
/// state, asserting each `apply()` succeeds. The supervisor must
/// never emit an envelope `apply()` would refuse — that's the
/// "supervisor's events apply cleanly" defense-in-depth that
/// also runs in the lib unit tests.
fn drive(state: &mut GoalState, step: &GoalSupervisorStep) {
    for envelope in &step.events {
        apply(state, envelope).unwrap_or_else(|reject: GoalApplyReject| {
            panic!("supervisor-emitted envelope must apply cleanly: {reject:?}");
        });
    }
}

/// Build the "incomplete-evidence" claim used in turn 2: claims
/// every required criterion but ships ZERO evidence_refs, which
/// the verifier rejects under Standard policy (Rule 2: matching
/// evidence per claimed criterion).
fn incomplete_claim() -> GoalCompletionClaim {
    GoalCompletionClaim {
        summary: "I think the tests pass — verification still pending".to_string(),
        completed_criteria: vec!["test-added".to_string(), "tests-pass".to_string()],
        evidence_refs: Vec::new(),
        verification: vec![GoalVerificationRecord {
            criterion_id: "test-added".to_string(),
            method: "ran cargo test path_join in my head".to_string(),
            passed: true,
            output_summary: None,
        }],
        residual_risks: Vec::new(),
    }
}

/// Build the "well-formed" claim used in turn 3: matching evidence
/// per claimed criterion + a `File` ref for the workspace-change
/// criterion + non-empty verification.
fn complete_claim() -> GoalCompletionClaim {
    GoalCompletionClaim {
        summary: "test landed; cargo test --lib green".to_string(),
        completed_criteria: vec!["test-added".to_string(), "tests-pass".to_string()],
        evidence_refs: vec![
            GoalEvidenceRef {
                criterion_id: Some("test-added".to_string()),
                target: GoalEvidenceTarget::File {
                    path: "tests/path_join_test.rs".to_string(),
                    sha256: "deadbeef".to_string(),
                },
                description: "new path_join test landed".to_string(),
            },
            GoalEvidenceRef {
                criterion_id: Some("tests-pass".to_string()),
                target: GoalEvidenceTarget::ToolCall {
                    call_id: "tc-cargo-test".to_string(),
                },
                description: "cargo test --lib exit 0".to_string(),
            },
        ],
        verification: vec![
            GoalVerificationRecord {
                criterion_id: "test-added".to_string(),
                method: "manual review of tests/path_join_test.rs".to_string(),
                passed: true,
                output_summary: Some("covers basic + edge cases".to_string()),
            },
            GoalVerificationRecord {
                criterion_id: "tests-pass".to_string(),
                method: "cargo test --lib".to_string(),
                passed: true,
                output_summary: Some("ok. 1234 passed; 0 failed".to_string()),
            },
        ],
        residual_risks: Vec::new(),
    }
}

/// S6 happy-path: three turns drive the Goal from `Active` →
/// `Completed` through one rejection. Pins the doc's
/// non-completion-doesn't-idle + verifier-is-the-gate +
/// rejection-doesn't-end-the-Goal invariants in one test.
#[test]
fn s6_three_turn_supervisor_drives_goal_to_completed() {
    let spec = s6_spec();
    let mut state = GoalState::from_spec(spec);
    let supervisor = fixture_supervisor();
    let clock = FixedClock::new();
    let ctx = AcceptingCtx;

    // Turn 1: model emits final text without calling
    // `submit_goal_complete`. Supervisor appends a synthetic
    // ProgressRecorded so the audit log captures the model's
    // narrative and the next prompt nudges it back into the
    // completion protocol (opencode.md:657).
    let turn1 = supervisor.step(
        &state,
        GoalTurnOutcome::FinalTextWithoutClaim {
            text: "I think we're done.".to_string(),
        },
        &ctx,
        &clock,
    );
    assert_eq!(turn1.events.len(), 1, "turn 1 emits one ProgressRecorded");
    match &turn1.events[0].event {
        GoalEvent::ProgressRecorded(record) => {
            assert!(record.summary.contains("I think we're done."));
        }
        other => panic!("turn 1 expected ProgressRecorded, got {other:?}"),
    }
    let GoalLoopDecision::Continue { prompt: prompt1 } = &turn1.decision else {
        panic!("turn 1 expected Continue, got {:?}", turn1.decision);
    };
    assert!(
        prompt1.contains("Final text alone does not complete a Goal"),
        "turn 1 prompt must nudge toward submit_goal_complete: {prompt1}",
    );
    drive(&mut state, &turn1);
    assert_eq!(state.status, GoalStatus::Active);

    // Turn 2: model calls `submit_goal_complete` with no evidence
    // refs. Verifier rejects under Standard policy Rule 2
    // (per-criterion matching evidence). Supervisor appends
    // CompletionClaimed THEN CompletionRejected, so a `--resume`
    // landing mid-rejection sees both envelopes in order.
    let turn2 = supervisor.step(
        &state,
        GoalTurnOutcome::CompletionClaim {
            claim: incomplete_claim(),
        },
        &ctx,
        &clock,
    );
    assert_eq!(turn2.events.len(), 2, "turn 2 emits Claimed + Rejected");
    assert!(matches!(
        &turn2.events[0].event,
        GoalEvent::CompletionClaimed(_)
    ));
    let claim_envelope_id = turn2.events[0].envelope_id;
    match &turn2.events[1].event {
        GoalEvent::CompletionRejected {
            claim_envelope_id: bound_to,
            missing,
            ..
        } => {
            assert_eq!(*bound_to, claim_envelope_id, "rejection must bind to claim");
            assert!(!missing.is_empty(), "rejection must list missing items");
        }
        other => panic!("turn 2 expected CompletionRejected, got {other:?}"),
    }
    let GoalLoopDecision::Continue { prompt: prompt2 } = &turn2.decision else {
        panic!("turn 2 expected Continue, got {:?}", turn2.decision);
    };
    assert!(prompt2.contains("Verifier rejected the last claim"));
    drive(&mut state, &turn2);
    assert_eq!(
        state.status,
        GoalStatus::Active,
        "rejection must NOT mark the Goal terminal — opencode.md:665",
    );
    assert_eq!(
        state.blockers.len(),
        1,
        "rejection blocker must be recorded"
    );

    // Turn 3: model retries with full evidence + verification.
    // Verifier accepts; supervisor appends Completed.
    let turn3 = supervisor.step(
        &state,
        GoalTurnOutcome::CompletionClaim {
            claim: complete_claim(),
        },
        &ctx,
        &clock,
    );
    assert_eq!(
        turn3.events.len(),
        2,
        "turn 3 emits CompletionClaimed + Completed",
    );
    assert!(matches!(
        &turn3.events[0].event,
        GoalEvent::CompletionClaimed(_)
    ));
    assert!(matches!(&turn3.events[1].event, GoalEvent::Completed(_)));
    let GoalLoopDecision::Completed { report } = &turn3.decision else {
        panic!("turn 3 expected Completed, got {:?}", turn3.decision);
    };
    assert_eq!(
        report.completed_criteria,
        vec!["test-added".to_string(), "tests-pass".to_string()],
    );
    assert_eq!(
        report.changed_files,
        vec!["tests/path_join_test.rs".to_string()]
    );
    drive(&mut state, &turn3);
    assert_eq!(state.status, GoalStatus::Completed);
    assert!(state.status.is_terminal());
    assert!(
        state.completion_report.is_some(),
        "post-Completed state must carry the verifier's report",
    );

    // Confirm the rejection blocker survived into the terminal
    // snapshot — the audit log must show the model's hard work.
    let rejection_blockers = state
        .blockers
        .iter()
        .filter(|b| matches!(b.reason, GoalBlockReason::CompletionRejected { .. }))
        .count();
    assert_eq!(
        rejection_blockers, 1,
        "exactly one rejection blocker must survive"
    );
}

/// S6 variant: continuation-loop cap reached. The supervisor MUST
/// NOT mark the Goal complete or cancelled — it appends
/// `Blocked { LoopLimitNeedsUser }` and returns `AwaitUser`.
/// `/goal status` would still display the Goal as active.
#[test]
fn s6_loop_limit_blocks_with_loop_limit_needs_user() {
    let spec = s6_spec();
    let state = GoalState::from_spec(spec);
    let supervisor = fixture_supervisor();
    let clock = FixedClock::new();
    let ctx = AcceptingCtx;
    let step = supervisor.step(
        &state,
        GoalTurnOutcome::LoopLimitReached { loops_run: 3 },
        &ctx,
        &clock,
    );
    assert_eq!(step.events.len(), 1, "loop limit emits one Blocked");
    match &step.events[0].event {
        GoalEvent::Blocked { reason, .. } => {
            assert!(matches!(
                reason,
                GoalBlockReason::LoopLimitNeedsUser { loops_run: 3 }
            ));
        }
        other => panic!("expected Blocked, got {other:?}"),
    }
    let GoalLoopDecision::AwaitUser { question } = &step.decision else {
        panic!("expected AwaitUser, got {:?}", step.decision);
    };
    assert!(
        question.contains("Continuation loop cap"),
        "AwaitUser question must surface the loop cap: {question}",
    );

    // Apply and confirm the Goal is NOT terminal — opencode.md:667
    // forbids the loop cap from masquerading as Completed.
    let mut applied = state.clone();
    drive(&mut applied, &step);
    assert!(!applied.status.is_terminal());
    assert!(matches!(
        applied.status,
        GoalStatus::Blocked | GoalStatus::AwaitingUser
    ));
}

/// The supervisor's `Cancelled` decision is an independent
/// terminal path — distinct from `Completed`. Pins that the
/// `/goal cancel` flow (P6.5/P6.6) lands the Goal in `Cancelled`
/// without ever calling the verifier.
#[test]
fn s6_user_cancel_drives_goal_to_cancelled_without_verifier() {
    let spec = s6_spec();
    let mut state = GoalState::from_spec(spec);
    let supervisor = fixture_supervisor();
    let clock = FixedClock::new();
    let ctx = AcceptingCtx;
    let step = supervisor.step(
        &state,
        GoalTurnOutcome::UserCancelled {
            reason: "user changed scope".to_string(),
            cancelled_by: GoalActor::User { id: None },
        },
        &ctx,
        &clock,
    );
    assert!(matches!(step.decision, GoalLoopDecision::Cancelled));
    drive(&mut state, &step);
    assert_eq!(state.status, GoalStatus::Cancelled);
    // `Cancelled` clears any in-flight pending claim per the
    // pass-10 P1 fix; pin that here too so a future regression
    // doesn't silently re-introduce stale claim state in
    // terminal snapshots.
    assert!(state.pending_claim.is_none());
}

/// Pin that a forged `Completed` envelope without a prior claim
/// does NOT slip through the supervisor → apply path. The
/// supervisor never produces such a sequence on its own (S6
/// always routes through `CompletionClaim`), but defense-in-depth
/// against a misbuilt envelope arriving via replay or a buggy
/// caller is part of the doc's "verifier is the only path to
/// terminal Completed" invariant.
#[test]
fn s6_forged_completed_without_claim_is_refused_by_apply() {
    let spec = s6_spec();
    let goal_id = spec.goal_id;
    let mut state = GoalState::from_spec(spec);
    let report = libra::internal::ai::goal::GoalCompletionReport {
        summary: "forged".to_string(),
        completed_criteria: vec!["test-added".to_string(), "tests-pass".to_string()],
        evidence_refs: vec![GoalEvidenceRef {
            criterion_id: Some("test-added".to_string()),
            target: GoalEvidenceTarget::File {
                path: "tests/path_join_test.rs".to_string(),
                sha256: "deadbeef".to_string(),
            },
            description: "forged".to_string(),
        }],
        verification: vec![GoalVerificationRecord {
            criterion_id: "test-added".to_string(),
            method: "x".to_string(),
            passed: true,
            output_summary: None,
        }],
        residual_risks: Vec::new(),
        changed_files: Vec::new(),
        claim_envelope_id: Uuid::new_v4(),
        total_spent_micro_usd: 0,
        elapsed_wall_clock_seconds: 0,
        continuation_loops_used: 0,
        finalised_at: fixture_now(),
        finalised_by: GoalActor::System {
            reason: "forged".to_string(),
        },
    };
    let envelope = GoalEventEnvelope {
        envelope_id: Uuid::new_v4(),
        goal_id,
        recorded_at: fixture_now() + Duration::seconds(1),
        event: GoalEvent::Completed(report),
    };
    let result = apply(&mut state, &envelope);
    assert_eq!(
        result,
        Err(GoalApplyReject::MissingCompletionClaim),
        "Completed without a prior CompletionClaimed must refuse",
    );
    assert_eq!(state.status, GoalStatus::Active);
    // Spot-check that the unused shape error variants compile
    // (catches a refactor that drops a variant the doc named).
    let _: GoalCompletionShapeError = GoalCompletionShapeError::MissingRequiredCriterion {
        id: "test-added".to_string(),
    };
}
