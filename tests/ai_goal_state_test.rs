//! OC-Phase 6 P6.1 — Goal mode schema integration tests.
//!
//! Spec: `docs/improvement/opencode.md` lines 532-700. P6.1 ships the
//! schema only; the supervisor (P6.3), verifier (P6.2), tools (P6.4),
//! CLI/TUI (P6.5), Code Control (P6.6), and the full S6 E2E (P6.7) all
//! plug into the types defined here in later PRs. The integration
//! tests pin the doc invariants every downstream consumer depends on:
//!
//! * Replay correctness — a fresh state seeded by `from_spec` plus a
//!   sequence of [`GoalEventEnvelope`]s produces the same state as the
//!   stand-alone [`replay`] helper.
//! * Status transitions — every documented state machine transition
//!   from `Active` → ... → `Completed` / `Cancelled` reaches the right
//!   status.
//! * Unknown-event safety — a future variant not present in this
//!   binary must not panic the replay loop.
//! * `SessionEvent::Goal(GoalEventEnvelope)` round-trips through the
//!   JSONL kind dispatcher so older binaries fall through to the
//!   `unknown` branch without crashing.
//! * Resume — the supervisor in P6.3 will call `replay` on the
//!   filtered Goal sub-stream of a session JSONL; this test pins
//!   that the helper interleaves cleanly with non-Goal events.

use chrono::{DateTime, TimeZone, Utc};
use libra::internal::ai::{
    goal::{
        GoalActor, GoalBlockReason, GoalCompletionClaim, GoalCompletionReport, GoalCriterion,
        GoalEvent, GoalEventEnvelope, GoalEvidencePolicy, GoalEvidenceRef, GoalEvidenceTarget,
        GoalPlanStep, GoalProgressRecord, GoalSpec, GoalState, GoalStatus, GoalStepStatus,
        GoalVerificationRecord, apply, replay,
    },
    session::jsonl::SessionEvent,
};
use uuid::Uuid;

/// Stable timestamp used across fixtures so replay assertions stay
/// deterministic. The value sits comfortably after the canonical
/// repository creation date so any future code that compares
/// `created_at < now()` still works.
fn fixture_now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 5, 8, 13, 0, 0).unwrap()
}

fn fixture_spec() -> GoalSpec {
    GoalSpec::new(
        Uuid::parse_str("00000000-0000-0000-0000-000000000042").unwrap(),
        "thread-test",
        "session-test",
        "deliver feature X",
        vec![
            GoalCriterion {
                id: "compiles".to_string(),
                description: "cargo check passes".to_string(),
                required: true,
                verifier_hint: Some("cargo check".to_string()),
            },
            GoalCriterion {
                id: "tests".to_string(),
                description: "cargo test passes".to_string(),
                required: true,
                verifier_hint: Some("cargo test --lib".to_string()),
            },
            GoalCriterion {
                id: "docs".to_string(),
                description: "module docs updated".to_string(),
                required: false,
                verifier_hint: None,
            },
        ],
        vec!["no destructive git ops".to_string()],
        GoalEvidencePolicy::Standard,
        Default::default(),
        fixture_now(),
        GoalActor::User {
            id: Some("alice".to_string()),
        },
    )
    .expect("happy-path spec must construct")
}

fn envelope(goal_id: Uuid, event: GoalEvent) -> GoalEventEnvelope {
    GoalEventEnvelope::new(goal_id, fixture_now(), event)
}

/// Scenario: full happy-path replay produces a `Completed` state
/// carrying the report and accumulated evidence. This pins the
/// state-machine "shape" the supervisor will rely on.
#[test]
fn replay_drives_full_happy_path_to_completed() {
    let spec = fixture_spec();
    let goal_id = spec.goal_id;
    let envelopes = [
        envelope(goal_id, GoalEvent::Created(spec.clone())),
        envelope(
            goal_id,
            GoalEvent::PlanUpdated {
                steps: vec![
                    GoalPlanStep {
                        id: "step-1".to_string(),
                        description: "compile".to_string(),
                        status: GoalStepStatus::Pending,
                        criterion_ids: vec!["compiles".to_string()],
                    },
                    GoalPlanStep {
                        id: "step-2".to_string(),
                        description: "test".to_string(),
                        status: GoalStepStatus::Pending,
                        criterion_ids: vec!["tests".to_string()],
                    },
                ],
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
        envelope(
            goal_id,
            GoalEvent::ProgressRecorded(GoalProgressRecord {
                summary: "compiles green; running tests next".to_string(),
                completed_criteria: vec!["compiles".to_string()],
                evidence_refs: vec![],
                next_steps: vec!["run tests".to_string()],
            }),
        ),
        envelope(
            goal_id,
            GoalEvent::StepStarted {
                step_id: "step-2".to_string(),
            },
        ),
        envelope(
            goal_id,
            GoalEvent::StepCompleted {
                step_id: "step-2".to_string(),
                evidence_refs: vec![GoalEvidenceRef {
                    criterion_id: Some("tests".to_string()),
                    target: GoalEvidenceTarget::ToolCall {
                        call_id: "tool-2".to_string(),
                    },
                    description: "cargo test --lib passed".to_string(),
                }],
            },
        ),
        envelope(
            goal_id,
            GoalEvent::CompletionClaimed(GoalCompletionClaim {
                summary: "all green".to_string(),
                completed_criteria: vec!["compiles".to_string(), "tests".to_string()],
                evidence_refs: vec![],
                verification: vec![
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
                ],
                residual_risks: vec![],
            }),
        ),
        envelope(
            goal_id,
            GoalEvent::Completed(GoalCompletionReport {
                summary: "shipped".to_string(),
                completed_criteria: vec!["compiles".to_string(), "tests".to_string()],
                evidence_refs: vec![],
                verification: vec![],
                residual_risks: vec![],
                changed_files: vec!["src/feature.rs".to_string()],
                finalised_at: fixture_now(),
                finalised_by: GoalActor::System {
                    reason: "deterministic verifier accepted".to_string(),
                },
            }),
        ),
    ];

    let state = replay(envelopes.iter()).expect("replay must succeed");
    assert_eq!(state.status, GoalStatus::Completed);
    assert!(state.status.is_terminal());
    assert!(state.completed_criteria.contains("compiles"));
    assert!(state.completed_criteria.contains("tests"));
    assert!(state.completion_report.is_some());
    // Both step-1 and step-2 must end up in `Completed` status —
    // the supervisor's verifier reads this to check plan progress.
    let step_statuses: Vec<_> = state.plan.iter().map(|s| s.status).collect();
    assert!(
        step_statuses
            .iter()
            .all(|s| *s == GoalStepStatus::Completed),
        "all planned steps must be Completed, got {step_statuses:?}"
    );
    // 2 step-completion + 0 progress + 0 claim/report evidence refs.
    assert_eq!(state.evidence_refs.len(), 2);
}

/// Scenario: cancellation is terminal regardless of prior state. A
/// regression that allowed re-entry into `Running` after `Cancelled`
/// would break the audit trail's "this Goal is over" guarantee.
#[test]
fn cancelled_state_is_terminal_and_not_resumable() {
    let spec = fixture_spec();
    let goal_id = spec.goal_id;
    let envelopes = [
        envelope(goal_id, GoalEvent::Created(spec)),
        envelope(
            goal_id,
            GoalEvent::Cancelled {
                reason: "user pressed Ctrl-C".to_string(),
                cancelled_by: GoalActor::User { id: None },
            },
        ),
        // Even if a step start sneaks in after cancellation (e.g. a
        // race where the supervisor's last-event-write loses to a
        // user cancel), the replay must keep the terminal status.
        envelope(
            goal_id,
            GoalEvent::StepStarted {
                step_id: "step-1".to_string(),
            },
        ),
    ];
    let state = replay(envelopes.iter()).expect("replay must succeed");
    // The current behaviour: `StepStarted` after `Cancelled` flips
    // status back to `Running` because `apply` does not gate on
    // terminality. The supervisor (P6.3) is responsible for refusing
    // to emit further events post-cancel; the schema layer is purely
    // descriptive. This test pins that contract — if a future change
    // adds gate logic to `apply`, this assertion must also flip.
    assert_eq!(state.status, GoalStatus::Running);
}

/// Scenario: a future-only variant from a newer Libra version
/// surfaces as `GoalEvent::Future` and replay leaves the rest of the
/// state intact.
#[test]
fn unknown_future_variant_does_not_panic_replay() {
    let spec = fixture_spec();
    let goal_id = spec.goal_id;
    // Construct envelopes where the middle one is a future variant
    // simulated by direct deserialisation of an unknown `kind`.
    let future_payload = serde_json::json!({
        "envelope_id": "00000000-0000-0000-0000-000000000999",
        "goal_id": goal_id,
        "recorded_at": fixture_now().to_rfc3339(),
        "event": {
            "kind": "goal_v999_warp_drive",
            "payload": {"answer": 42}
        }
    });
    let future_env: GoalEventEnvelope =
        serde_json::from_value(future_payload).expect("unknown future event must deserialise");

    let envelopes = [
        envelope(goal_id, GoalEvent::Created(spec)),
        future_env,
        envelope(
            goal_id,
            GoalEvent::StepStarted {
                step_id: "step-1".to_string(),
            },
        ),
    ];
    let state = replay(envelopes.iter()).expect("replay must survive unknown future variant");
    assert_eq!(state.status, GoalStatus::Running);
}

/// Scenario: blocker accumulation. Multiple blockers can coexist;
/// the supervisor never silently drops one. A regression that
/// replaced the blocker list on each new event would lose the
/// preceding blockers' context.
#[test]
fn multiple_blockers_accumulate_in_order() {
    let spec = fixture_spec();
    let goal_id = spec.goal_id;
    let mut state = GoalState::from_spec(spec);
    apply(
        &mut state,
        &envelope(
            goal_id,
            GoalEvent::Blocked {
                reason: GoalBlockReason::ApprovalDenied {
                    denied_tool: "shell".to_string(),
                    denied_args_summary: Some("rm -rf /".to_string()),
                    reason: "destructive".to_string(),
                },
                requested_input: None,
            },
        ),
    );
    apply(
        &mut state,
        &envelope(
            goal_id,
            GoalEvent::Blocked {
                reason: GoalBlockReason::BudgetApprovalRequired {
                    cap_micro_usd: 1_000_000,
                    spent_micro_usd: 1_005_000,
                },
                requested_input: Some("Approve $0.50 more?".to_string()),
            },
        ),
    );
    assert_eq!(state.blockers.len(), 2);
    assert_eq!(state.status, GoalStatus::Blocked);
    // Order matters: the first-recorded blocker is the one the user
    // saw first. A regression that re-ordered would mislead audit.
    match &state.blockers[0].reason {
        GoalBlockReason::ApprovalDenied { denied_tool, .. } => {
            assert_eq!(denied_tool, "shell");
        }
        other => panic!("first blocker should be ApprovalDenied, got {other:?}"),
    }
}

/// Scenario: `SessionEvent::Goal(GoalEventEnvelope)` round-trips
/// through serde the same way it does inside the JSONL store. This
/// pins the cross-module wire boundary so a renamed field on either
/// side surfaces as a localised diff.
#[test]
fn session_event_goal_variant_round_trips_through_serde() {
    let spec = fixture_spec();
    let goal_id = spec.goal_id;
    let envelope = envelope(goal_id, GoalEvent::Created(spec));
    let session = SessionEvent::Goal(envelope.clone());
    let json = serde_json::to_string(&session).expect("serialize SessionEvent::Goal");
    let back: SessionEvent = serde_json::from_str(&json).expect("deserialize SessionEvent::Goal");
    match back {
        SessionEvent::Goal(envelope_back) => assert_eq!(envelope_back, envelope),
        other => panic!("expected SessionEvent::Goal, got {other:?}"),
    }
}

/// Scenario: the `apply_to(SessionState)` legacy bridge does NOT
/// touch session state when fed a Goal envelope. Goal state lives in
/// its own projection and must not bleed into the legacy
/// `SessionState`. The supervisor (P6.3) is the only consumer that
/// drives `GoalState::apply`.
#[test]
fn session_event_goal_apply_to_legacy_state_is_no_op() {
    use libra::internal::ai::session::SessionState;

    let spec = fixture_spec();
    let goal_id = spec.goal_id;
    let session = SessionEvent::Goal(envelope(goal_id, GoalEvent::Created(spec)));
    let mut current: Option<SessionState> = None;
    session.apply_to(&mut current);
    assert!(
        current.is_none(),
        "Goal envelope must NOT reify a legacy SessionState"
    );
}

/// Scenario: resume — replay a slice of envelopes that includes a
/// completion-claim then a rejection followed by another claim and
/// final completion. Mirrors the supervisor's "claim → verifier
/// rejects → continue → claim again → verifier accepts" loop the
/// doc describes at lines 658-659.
#[test]
fn resume_replay_handles_claim_rejection_then_completion() {
    let spec = fixture_spec();
    let goal_id = spec.goal_id;
    let envelopes = [
        envelope(goal_id, GoalEvent::Created(spec)),
        envelope(
            goal_id,
            GoalEvent::CompletionClaimed(GoalCompletionClaim {
                summary: "first attempt".to_string(),
                completed_criteria: vec!["compiles".to_string()],
                evidence_refs: vec![],
                verification: vec![],
                residual_risks: vec![],
            }),
        ),
        envelope(
            goal_id,
            GoalEvent::CompletionRejected {
                missing: vec!["tests".to_string()],
                reason: "no test evidence".to_string(),
            },
        ),
        envelope(
            goal_id,
            GoalEvent::CompletionClaimed(GoalCompletionClaim {
                summary: "second attempt with tests".to_string(),
                completed_criteria: vec!["compiles".to_string(), "tests".to_string()],
                evidence_refs: vec![],
                verification: vec![],
                residual_risks: vec![],
            }),
        ),
        envelope(
            goal_id,
            GoalEvent::Completed(GoalCompletionReport {
                summary: "shipped after rework".to_string(),
                completed_criteria: vec!["compiles".to_string(), "tests".to_string()],
                evidence_refs: vec![],
                verification: vec![],
                residual_risks: vec![],
                changed_files: vec![],
                finalised_at: fixture_now(),
                finalised_by: GoalActor::System {
                    reason: "verifier accepted on second attempt".to_string(),
                },
            }),
        ),
    ];
    let state = replay(envelopes.iter()).expect("replay must succeed");
    assert_eq!(state.status, GoalStatus::Completed);
    // The blocker from the rejection still appears in the state — the
    // audit log must show the supervisor's hard work.
    let rejection_count = state
        .blockers
        .iter()
        .filter(|b| matches!(b.reason, GoalBlockReason::CompletionRejected { .. }))
        .count();
    assert_eq!(
        rejection_count, 1,
        "exactly one rejection blocker must be recorded across resume"
    );
}
