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
        GoalActor, GoalApplyReject, GoalBlockReason, GoalCompletionClaim, GoalCompletionReport,
        GoalCompletionShapeError, GoalCriterion, GoalEvent, GoalEventEnvelope, GoalEvidencePolicy,
        GoalEvidenceRef, GoalEvidenceTarget, GoalPlanStep, GoalProgressRecord, GoalSpec, GoalState,
        GoalStatus, GoalStepStatus, GoalVerificationRecord, MAX_REPLAY_REJECTIONS, apply, replay,
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
                requires_workspace_change: true,
            },
            GoalCriterion {
                id: "tests".to_string(),
                description: "cargo test passes".to_string(),
                required: true,
                verifier_hint: Some("cargo test --lib".to_string()),
                requires_workspace_change: true,
            },
            GoalCriterion {
                id: "docs".to_string(),
                description: "module docs updated".to_string(),
                required: false,
                verifier_hint: None,
                requires_workspace_change: false,
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
                evidence_refs: vec![
                    GoalEvidenceRef {
                        criterion_id: Some("compiles".to_string()),
                        target: GoalEvidenceTarget::File {
                            path: "src/feature.rs".to_string(),
                            sha256: "deadbeef".to_string(),
                        },
                        description: "edit landed".to_string(),
                    },
                    GoalEvidenceRef {
                        criterion_id: Some("tests".to_string()),
                        target: GoalEvidenceTarget::File {
                            path: "tests/feature.rs".to_string(),
                            sha256: "cafef00d".to_string(),
                        },
                        description: "test landed".to_string(),
                    },
                ],
                verification: vec![],
                residual_risks: vec![],
                changed_files: vec!["src/feature.rs".to_string()],
                total_spent_micro_usd: 2_400_000,
                elapsed_wall_clock_seconds: 1_350,
                continuation_loops_used: 5,
                finalised_at: fixture_now(),
                finalised_by: GoalActor::System {
                    reason: "deterministic verifier accepted".to_string(),
                },
            }),
        ),
    ];

    let state = replay(envelopes.iter()).expect("replay must succeed").state;
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
    // 2 step-completion + 0 progress + 0 claim refs (claim's
    // evidence list is empty in this fixture) + 2 report refs
    // (one File evidence per required criterion, since Standard
    // policy + requires_workspace_change=true on both demands it).
    assert_eq!(state.evidence_refs.len(), 4);
}

/// Scenario: cancellation is terminal regardless of prior state. A
/// late-arriving event (racy supervisor write, corrupted log
/// replayed twice, out-of-order JSONL slice) must NOT walk a
/// cancelled Goal back into `Running`. The schema layer enforces
/// this with the terminal-state guard at the top of `apply`; the
/// supervisor (P6.3) reads the rejection diagnostic from the
/// replay outcome and logs the gap.
#[test]
fn cancelled_state_is_terminal_and_freezes_replay() {
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
        envelope(
            goal_id,
            GoalEvent::StepStarted {
                step_id: "step-1".to_string(),
            },
        ),
    ];
    let outcome = replay(envelopes.iter()).expect("replay must succeed");
    assert_eq!(
        outcome.state.status,
        GoalStatus::Cancelled,
        "terminal status must NOT flip back to Running on a late event"
    );
    assert!(outcome.state.status.is_terminal());
    assert_eq!(
        outcome.rejected.len(),
        1,
        "the post-Cancelled StepStarted must surface as a rejection diagnostic",
    );
    assert!(
        matches!(
            outcome.rejected[0].reason,
            GoalApplyReject::TerminalGuard {
                status: GoalStatus::Cancelled,
            }
        ),
        "rejection reason must be TerminalGuard{{Cancelled}}, got {:?}",
        outcome.rejected[0].reason
    );
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
    let outcome = replay(envelopes.iter()).expect("replay must survive unknown future variant");
    assert_eq!(outcome.state.status, GoalStatus::Running);
    assert_eq!(
        outcome.rejected.len(),
        1,
        "the unknown future variant must surface as exactly one rejection diagnostic",
    );
    assert_eq!(
        outcome.rejected[0].reason,
        GoalApplyReject::UnknownFutureVariant,
        "rejection reason for an unknown variant must be UnknownFutureVariant",
    );
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
    )
    .expect("first Blocked apply must succeed");
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
    )
    .expect("second Blocked apply must succeed");
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
                evidence_refs: vec![
                    GoalEvidenceRef {
                        criterion_id: Some("compiles".to_string()),
                        target: GoalEvidenceTarget::File {
                            path: "src/feature.rs".to_string(),
                            sha256: "deadbeef".to_string(),
                        },
                        description: "edit landed".to_string(),
                    },
                    GoalEvidenceRef {
                        criterion_id: Some("tests".to_string()),
                        target: GoalEvidenceTarget::File {
                            path: "tests/feature.rs".to_string(),
                            sha256: "cafef00d".to_string(),
                        },
                        description: "test landed".to_string(),
                    },
                ],
                verification: vec![],
                residual_risks: vec![],
                changed_files: vec![],
                total_spent_micro_usd: 0,
                elapsed_wall_clock_seconds: 0,
                continuation_loops_used: 0,
                finalised_at: fixture_now(),
                finalised_by: GoalActor::System {
                    reason: "verifier accepted on second attempt".to_string(),
                },
            }),
        ),
    ];
    let state = replay(envelopes.iter()).expect("replay must succeed").state;
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
    assert!(
        state.pending_claim.is_none(),
        "pending_claim must be cleared once the verifier accepts"
    );
}

/// Scenario: `CriteriaRevised` (the `/goal criteria add <text>`
/// entry from `docs/improvement/opencode.md` line 690) replaces the
/// spec's acceptance criteria, and any `completed_criteria` no
/// longer in scope are dropped from the state. This guards the
/// verifier (P6.2) from accepting completion based on a stale
/// criterion the user has since removed.
#[test]
fn criteria_revised_drops_out_of_scope_completed_entries() {
    let spec = fixture_spec();
    let goal_id = spec.goal_id;
    let envelopes = [
        envelope(goal_id, GoalEvent::Created(spec)),
        envelope(
            goal_id,
            GoalEvent::ProgressRecorded(GoalProgressRecord {
                summary: "compiled".to_string(),
                completed_criteria: vec!["compiles".to_string(), "tests".to_string()],
                evidence_refs: vec![],
                next_steps: vec![],
            }),
        ),
        // User narrows the scope: `tests` removed, `audit` added.
        envelope(
            goal_id,
            GoalEvent::CriteriaRevised {
                criteria: vec![
                    GoalCriterion {
                        id: "compiles".to_string(),
                        description: "cargo check passes".to_string(),
                        required: true,
                        verifier_hint: None,
                        requires_workspace_change: true,
                    },
                    GoalCriterion {
                        id: "audit".to_string(),
                        description: "security audit attached".to_string(),
                        required: true,
                        verifier_hint: None,
                        requires_workspace_change: false,
                    },
                ],
                revised_by: GoalActor::User {
                    id: Some("alice".to_string()),
                },
            },
        ),
    ];
    let state = replay(envelopes.iter()).expect("replay must succeed").state;
    assert!(
        state.completed_criteria.contains("compiles"),
        "`compiles` must remain in completed_criteria (still in scope)"
    );
    assert!(
        !state.completed_criteria.contains("tests"),
        "`tests` must be dropped after the revision removes it from scope"
    );
    let revised_ids: std::collections::BTreeSet<&str> = state
        .spec
        .acceptance_criteria
        .iter()
        .map(|c| c.id.as_str())
        .collect();
    assert!(
        revised_ids.contains("audit"),
        "revised criteria must include audit"
    );
    assert!(
        !revised_ids.contains("tests"),
        "revised criteria must NOT include tests"
    );
}

/// Scenario: a `CriteriaRevised` event carrying duplicate criterion
/// ids is silently dropped — `apply` validates with the same rules
/// `GoalSpec::new` uses on construction. Without this, the
/// verifier (P6.2) keys completion through
/// `completed_criteria: BTreeSet<String>`, so a single claim could
/// satisfy multiple required criteria when the spec carries
/// duplicate ids. Pinned by an explicit replay assertion that the
/// state's spec criteria remain unchanged.
#[test]
fn criteria_revised_with_duplicate_ids_is_rejected() {
    let spec = fixture_spec();
    let goal_id = spec.goal_id;
    let original_criteria = spec.acceptance_criteria.clone();
    let envelopes = [
        envelope(goal_id, GoalEvent::Created(spec)),
        envelope(
            goal_id,
            GoalEvent::CriteriaRevised {
                criteria: vec![
                    GoalCriterion {
                        id: "compiles".to_string(),
                        description: "first".to_string(),
                        required: true,
                        verifier_hint: None,
                        requires_workspace_change: true,
                    },
                    GoalCriterion {
                        id: "compiles".to_string(),
                        description: "duplicate id".to_string(),
                        required: true,
                        verifier_hint: None,
                        requires_workspace_change: true,
                    },
                ],
                revised_by: GoalActor::User { id: None },
            },
        ),
    ];
    let outcome =
        replay(envelopes.iter()).expect("replay must succeed even when revision rejected");
    assert_eq!(
        outcome.state.spec.acceptance_criteria, original_criteria,
        "duplicate-id revision must NOT replace the original acceptance_criteria"
    );
    assert_eq!(
        outcome.rejected.len(),
        1,
        "duplicate-id CriteriaRevised must surface as a rejection diagnostic",
    );
    assert!(
        matches!(
            outcome.rejected[0].reason,
            GoalApplyReject::InvalidCriteriaRevised { .. }
        ),
        "rejection reason must be InvalidCriteriaRevised, got {:?}",
        outcome.rejected[0].reason
    );
}

/// Scenario: rejected envelopes leave `state.updated_at`
/// byte-for-byte unchanged. A snapshot consumer (the
/// supervisor's resume diff, the audit-trail compactor) must be
/// able to distinguish "real mutation" from "rejected envelope"
/// by checking the timestamp; a leaked update would muddy that
/// signal. This test feeds three rejection paths in turn and
/// asserts the timestamp survives each one.
#[test]
fn rejected_envelopes_do_not_leak_updated_at() {
    let spec = fixture_spec();
    let goal_id = spec.goal_id;
    let mut state = libra::internal::ai::goal::GoalState::from_spec(spec);
    let baseline_ts = state.updated_at;

    // ---- Cross-goal envelope: doesn't touch state at all.
    let other_goal = Uuid::parse_str("00000000-0000-0000-0000-0000aaaa0000").unwrap();
    let env = libra::internal::ai::goal::GoalEventEnvelope::new(
        other_goal,
        Utc.with_ymd_and_hms(2099, 1, 1, 0, 0, 0).unwrap(),
        GoalEvent::StepStarted {
            step_id: "noop".to_string(),
        },
    );
    let result = apply(&mut state, &env);
    assert!(
        matches!(result, Err(GoalApplyReject::CrossGoal { .. })),
        "cross-goal envelope must surface CrossGoal reject reason, got {result:?}",
    );
    assert_eq!(
        state.updated_at, baseline_ts,
        "cross-goal rejection must not advance updated_at"
    );

    // ---- Invalid CriteriaRevised (duplicate id).
    let env = libra::internal::ai::goal::GoalEventEnvelope::new(
        goal_id,
        Utc.with_ymd_and_hms(2099, 1, 1, 0, 0, 0).unwrap(),
        GoalEvent::CriteriaRevised {
            criteria: vec![
                GoalCriterion {
                    id: "dup".to_string(),
                    description: "x".to_string(),
                    required: true,
                    verifier_hint: None,
                    requires_workspace_change: true,
                },
                GoalCriterion {
                    id: "dup".to_string(),
                    description: "y".to_string(),
                    required: true,
                    verifier_hint: None,
                    requires_workspace_change: true,
                },
            ],
            revised_by: GoalActor::User { id: None },
        },
    );
    let result = apply(&mut state, &env);
    assert!(
        matches!(result, Err(GoalApplyReject::InvalidCriteriaRevised { .. })),
        "invalid CriteriaRevised must surface InvalidCriteriaRevised reason, got {result:?}",
    );
    assert_eq!(
        state.updated_at, baseline_ts,
        "invalid CriteriaRevised must not advance updated_at"
    );

    // ---- Terminal-state guard: cancel the Goal then send a
    //      late event — should also not advance updated_at past
    //      the cancellation.
    let cancel_ts = Utc.with_ymd_and_hms(2027, 1, 1, 0, 0, 0).unwrap();
    let cancel_env = libra::internal::ai::goal::GoalEventEnvelope::new(
        goal_id,
        cancel_ts,
        GoalEvent::Cancelled {
            reason: "x".to_string(),
            cancelled_by: GoalActor::User { id: None },
        },
    );
    apply(&mut state, &cancel_env).expect("Cancelled apply must succeed");
    assert_eq!(state.updated_at, cancel_ts);
    let late = libra::internal::ai::goal::GoalEventEnvelope::new(
        goal_id,
        Utc.with_ymd_and_hms(2099, 1, 1, 0, 0, 0).unwrap(),
        GoalEvent::StepStarted {
            step_id: "late".to_string(),
        },
    );
    let result = apply(&mut state, &late);
    assert!(
        matches!(result, Err(GoalApplyReject::TerminalGuard { .. })),
        "post-Cancelled event must surface TerminalGuard reason, got {result:?}",
    );
    assert_eq!(
        state.updated_at, cancel_ts,
        "terminal-state guard must not advance updated_at"
    );
}

/// Scenario: a corrupted JSONL stream ships a `Created`
/// envelope carrying a `GoalSpec` with duplicate criterion ids.
/// `replay` must reject the stream — without re-validating the
/// deserialised spec, the verifier (P6.2) keys completion off
/// `completed_criteria: BTreeSet<String>` and a duplicate id
/// would let one claim satisfy multiple required criteria.
#[test]
fn replay_rejects_created_envelope_with_invalid_spec_criteria() {
    let goal_id = Uuid::parse_str("00000000-0000-0000-0000-0000feed0001").unwrap();
    // Hand-craft a spec JSON payload that bypasses GoalSpec::new
    // validation. Direct serde_json::from_value lets us simulate
    // a JSONL row tampered after construction.
    // GoalEvent uses internally-tagged enum form (`#[serde(tag = "kind")]`)
    // so the `Created(GoalSpec)` newtype variant flattens the spec
    // fields alongside the discriminator at the same JSON level.
    // A `payload` sub-object would NOT round-trip — see
    // `event.rs::round_trips_every_documented_variant` for the
    // expected wire shape.
    let envelope_json = serde_json::json!({
        "envelope_id": "00000000-0000-0000-0000-0000feed0002",
        "goal_id": goal_id,
        "recorded_at": fixture_now().to_rfc3339(),
        "event": {
            "kind": "created",
            "goal_id": goal_id,
            "thread_id": "thread-x",
            "session_id": "session-x",
            "objective": "objective with duplicate ids",
            "acceptance_criteria": [
                {"id": "dup", "description": "first", "required": true},
                {"id": "dup", "description": "second", "required": true},
            ],
            "constraints": [],
            "evidence_policy": "standard",
            "budget": {
                "hard_cap_micro_usd": 0,
                "warn_threshold_micro_usd": 0,
                "wall_clock_seconds": 0,
                "max_continuation_loops": 16,
            },
            "created_at": fixture_now().to_rfc3339(),
            "created_by": {"kind": "user", "id": null},
        }
    });
    let envelope: GoalEventEnvelope = serde_json::from_value(envelope_json)
        .expect("hand-crafted Created envelope must deserialise");
    let envelopes = [envelope];
    assert!(
        replay(envelopes.iter()).is_none(),
        "replay must reject a Created envelope whose embedded spec carries duplicate criterion ids"
    );
}

/// Scenario: replay refuses a `Created` envelope whose own
/// `goal_id` does not match the embedded spec's `goal_id`. A
/// misrouted or corrupted log can ship a Created envelope tagged
/// with one Goal but carrying a spec for another — without this
/// check, subsequent envelopes (filtered by the *envelope* id)
/// would silently fail the cross-goal guard inside `apply` and
/// the user would lose all post-Created progress with no
/// observable signal. Asserting `replay` returns `None` makes the
/// failure surface immediately at the resume seam.
#[test]
fn replay_rejects_created_envelope_with_mismatched_goal_id() {
    let spec = fixture_spec();
    let envelope_goal_id = Uuid::parse_str("00000000-0000-0000-0000-000000000bad").unwrap();
    assert_ne!(envelope_goal_id, spec.goal_id);
    let envelopes = [libra::internal::ai::goal::GoalEventEnvelope::new(
        envelope_goal_id,
        fixture_now(),
        GoalEvent::Created(spec),
    )];
    assert!(
        replay(envelopes.iter()).is_none(),
        "replay must reject a Created envelope whose goal_id != spec.goal_id"
    );
}

/// Scenario: a `CriteriaRevised` event carrying a blank criterion id
/// is rejected for the same reason as the duplicate-id case — both
/// are shape errors `validate_criteria` enforces on construction.
#[test]
fn criteria_revised_with_blank_id_is_rejected() {
    let spec = fixture_spec();
    let goal_id = spec.goal_id;
    let original_criteria = spec.acceptance_criteria.clone();
    let envelopes = [
        envelope(goal_id, GoalEvent::Created(spec)),
        envelope(
            goal_id,
            GoalEvent::CriteriaRevised {
                criteria: vec![GoalCriterion {
                    id: "  ".to_string(),
                    description: "blank id".to_string(),
                    required: true,
                    verifier_hint: None,
                    requires_workspace_change: false,
                }],
                revised_by: GoalActor::User { id: None },
            },
        ),
    ];
    let outcome =
        replay(envelopes.iter()).expect("replay must succeed even when revision rejected");
    assert_eq!(
        outcome.state.spec.acceptance_criteria, original_criteria,
        "blank-id revision must NOT replace the original acceptance_criteria"
    );
    assert_eq!(
        outcome.rejected.len(),
        1,
        "blank-id CriteriaRevised must surface as a rejection diagnostic",
    );
    assert!(
        matches!(
            outcome.rejected[0].reason,
            GoalApplyReject::InvalidCriteriaRevised { .. }
        ),
        "rejection reason must be InvalidCriteriaRevised, got {:?}",
        outcome.rejected[0].reason
    );
}

/// Scenario: a rejected completion claim must NOT leave its claimed
/// criteria visible in `completed_criteria` — those were the
/// model's unverified assertions, and the verifier said no. The
/// `pending_claim` field is also cleared so a `--resume` does not
/// see stale claim payload.
#[test]
fn completion_rejected_rolls_back_pending_claim() {
    let spec = fixture_spec();
    let goal_id = spec.goal_id;
    let envelopes = [
        envelope(goal_id, GoalEvent::Created(spec)),
        envelope(
            goal_id,
            GoalEvent::CompletionClaimed(GoalCompletionClaim {
                summary: "first try".to_string(),
                completed_criteria: vec!["compiles".to_string(), "tests".to_string()],
                evidence_refs: vec![],
                verification: vec![],
                residual_risks: vec![],
            }),
        ),
        envelope(
            goal_id,
            GoalEvent::CompletionRejected {
                missing: vec!["tests".to_string()],
                reason: "no test evidence attached".to_string(),
            },
        ),
    ];
    let state = replay(envelopes.iter()).expect("replay must succeed").state;
    assert!(
        state.pending_claim.is_none(),
        "pending_claim must be dropped after rejection"
    );
    assert!(
        !state.completed_criteria.contains("compiles"),
        "claimed criteria must NOT pollute completed_criteria when rejected"
    );
    assert_eq!(state.status, GoalStatus::Active);
}

/// Scenario: a `SessionEvent::Goal` envelope carrying an unknown
/// nested variant inside a known `GoalEvent` (e.g. an unknown
/// `GoalBlockReason::kind` inside a known `Blocked` envelope) must
/// also deserialise cleanly via the nested `Future` catch-alls. A
/// regression that re-tightens the inner enums would surface as a
/// SessionEvent decode failure instead of skipping the unknown
/// inner variant.
#[test]
fn session_event_goal_with_unknown_nested_variant_deserialises() {
    use libra::internal::ai::session::jsonl::SessionEvent;

    let spec = fixture_spec();
    let payload = serde_json::json!({
        "kind": "goal",
        "payload": {
            "envelope_id": "00000000-0000-0000-0000-000000000abc",
            "goal_id": spec.goal_id,
            "recorded_at": fixture_now().to_rfc3339(),
            "event": {
                "kind": "blocked",
                "reason": {
                    "kind": "future_blocker_kind_not_yet_known",
                    "payload": {"foo": "bar"}
                },
                "requested_input": null
            }
        }
    });
    let session: SessionEvent = serde_json::from_value(payload)
        .expect("SessionEvent::Goal with unknown nested variant must deserialise");
    match session {
        SessionEvent::Goal(envelope) => match envelope.event {
            GoalEvent::Blocked { reason, .. } => {
                assert!(
                    matches!(reason, GoalBlockReason::Future),
                    "unknown nested reason must surface as GoalBlockReason::Future, got {reason:?}"
                );
            }
            other => panic!("expected GoalEvent::Blocked, got {other:?}"),
        },
        other => panic!("expected SessionEvent::Goal, got {other:?}"),
    }
}

/// Scenario (Codex pass-6 P1#1): a forged JSONL stream ships a
/// `Created` envelope followed directly by a `Completed` envelope
/// whose report omits a required spec criterion. The supervisor's
/// resume seam reads the stream verbatim and (per
/// `docs/improvement/opencode.md` lines 1463-1467) returns idle the
/// moment it sees `Completed` — the verifier (P6.2) is not invoked
/// on replay. Without the schema-layer floor enforced by
/// `validate_completion_report_shape`, the forged Goal would
/// transition straight to terminal `Completed`, bypassing the
/// verifier path entirely.
///
/// `apply` must refuse the forged `Completed`, leaving the Goal
/// `Active`, and the replay outcome must surface
/// `GoalApplyReject::InvalidCompletionReport` so the supervisor can
/// log the gap.
#[test]
fn replay_rejects_forged_completed_envelope_without_required_criteria() {
    let spec = fixture_spec();
    let goal_id = spec.goal_id;
    // Pre-claim with a legitimate `CompletionClaimed` so
    // `pending_claim` is `Some` — this isolates the schema-layer
    // shape gate from the `MissingCompletionClaim` state-machine
    // guard (a separate Codex pass-7 P1 finding). The forged
    // `Completed` then claims only one of the two required
    // criteria, which the floor must refuse.
    let legit_claim = GoalCompletionClaim {
        summary: "claim".to_string(),
        completed_criteria: vec!["compiles".to_string(), "tests".to_string()],
        evidence_refs: vec![],
        verification: vec![],
        residual_risks: vec![],
    };
    let envelopes = [
        envelope(goal_id, GoalEvent::Created(spec)),
        envelope(goal_id, GoalEvent::CompletionClaimed(legit_claim)),
        envelope(
            goal_id,
            GoalEvent::Completed(GoalCompletionReport {
                summary: "forged".to_string(),
                // Spec demands "compiles" + "tests" required; report
                // claims only "compiles". The deterministic verifier
                // (P6.2) would never have produced this report.
                completed_criteria: vec!["compiles".to_string()],
                evidence_refs: vec![],
                verification: vec![],
                residual_risks: vec![],
                changed_files: vec![],
                total_spent_micro_usd: 0,
                elapsed_wall_clock_seconds: 0,
                continuation_loops_used: 0,
                finalised_at: fixture_now(),
                finalised_by: GoalActor::System {
                    reason: "forged log entry".to_string(),
                },
            }),
        ),
    ];
    let outcome =
        replay(envelopes.iter()).expect("replay must succeed even when Completed rejected");
    assert_eq!(
        outcome.state.status,
        GoalStatus::CompletionClaimed,
        "forged Completed must NOT transition the Goal to terminal — \
         the verifier (P6.2) is the only legitimate path to `Completed`",
    );
    assert!(outcome.state.completion_report.is_none());
    assert_eq!(
        outcome.rejected.len(),
        1,
        "the forged Completed envelope must surface as a rejection diagnostic",
    );
    match &outcome.rejected[0].reason {
        GoalApplyReject::InvalidCompletionReport { source } => {
            assert_eq!(
                source,
                &GoalCompletionShapeError::MissingRequiredCriterion {
                    id: "tests".to_string(),
                },
            );
        }
        other => panic!(
            "rejection reason must be InvalidCompletionReport(MissingRequiredCriterion), got {other:?}",
        ),
    }
}

/// Scenario (Codex pass-6 P1#1, related): a forged `Completed`
/// envelope whose report claims a criterion id that the spec never
/// declared (e.g. "fabricated") is also refused at the schema-layer
/// floor. Symmetric to the missing-required case: a verifier
/// accept-path can never produce an unknown-id claim, so seeing one
/// in the JSONL is conclusive evidence the verifier was bypassed.
#[test]
fn replay_rejects_forged_completed_envelope_with_unknown_criterion_id() {
    let spec = fixture_spec();
    let goal_id = spec.goal_id;
    // Same isolation as the missing-required-criterion test:
    // pre-claim so `pending_claim` is `Some`, then ship a
    // forged Completed that claims a fabricated id.
    let legit_claim = GoalCompletionClaim {
        summary: "claim".to_string(),
        completed_criteria: vec!["compiles".to_string(), "tests".to_string()],
        evidence_refs: vec![],
        verification: vec![],
        residual_risks: vec![],
    };
    let envelopes = [
        envelope(goal_id, GoalEvent::Created(spec)),
        envelope(goal_id, GoalEvent::CompletionClaimed(legit_claim)),
        envelope(
            goal_id,
            GoalEvent::Completed(GoalCompletionReport {
                summary: "forged with phantom id".to_string(),
                completed_criteria: vec![
                    "compiles".to_string(),
                    "tests".to_string(),
                    "fabricated".to_string(),
                ],
                evidence_refs: vec![],
                verification: vec![],
                residual_risks: vec![],
                changed_files: vec![],
                total_spent_micro_usd: 0,
                elapsed_wall_clock_seconds: 0,
                continuation_loops_used: 0,
                finalised_at: fixture_now(),
                finalised_by: GoalActor::System {
                    reason: "forged log entry".to_string(),
                },
            }),
        ),
    ];
    let outcome = replay(envelopes.iter()).expect("replay must succeed");
    assert_eq!(outcome.state.status, GoalStatus::CompletionClaimed);
    assert_eq!(outcome.rejected.len(), 1);
    match &outcome.rejected[0].reason {
        GoalApplyReject::InvalidCompletionReport { source } => {
            assert_eq!(
                source,
                &GoalCompletionShapeError::UnknownCriterionId {
                    id: "fabricated".to_string(),
                },
            );
        }
        other => panic!(
            "rejection reason must be InvalidCompletionReport(UnknownCriterionId), got {other:?}",
        ),
    }
}

/// Scenario (Codex pass-6 P2#3): a JSONL slice that interleaves
/// envelopes from two different Goals — the kind a buggy
/// control-plane copy or a concatenated log rotation could produce
/// — must not "look successful" to the resume seam. The cross-goal
/// envelopes are still rejected inside `apply`, but now the
/// rejection diagnostic surfaces in
/// `GoalReplayOutcome::rejected` so the supervisor can render a
/// concrete error instead of treating the post-Created stream as a
/// clean replay.
#[test]
fn replay_surfaces_cross_goal_envelopes_as_rejection_diagnostics() {
    let spec = fixture_spec();
    let goal_id = spec.goal_id;
    let other_goal = Uuid::parse_str("00000000-0000-0000-0000-0000abcd0000").unwrap();
    let envelopes = [
        envelope(goal_id, GoalEvent::Created(spec)),
        // Legitimate envelope for the active Goal — must apply.
        envelope(
            goal_id,
            GoalEvent::StepStarted {
                step_id: "step-1".to_string(),
            },
        ),
        // Foreign Goal — must surface as a rejection diagnostic.
        envelope(
            other_goal,
            GoalEvent::StepStarted {
                step_id: "step-foreign".to_string(),
            },
        ),
    ];
    let outcome = replay(envelopes.iter()).expect("replay must succeed");
    assert_eq!(
        outcome.state.status,
        GoalStatus::Running,
        "the in-Goal StepStarted must still apply",
    );
    assert_eq!(
        outcome.rejected.len(),
        1,
        "the foreign-Goal envelope must surface as a rejection diagnostic, \
         not be silently dropped",
    );
    match &outcome.rejected[0].reason {
        GoalApplyReject::CrossGoal {
            envelope_goal_id,
            state_goal_id,
        } => {
            assert_eq!(*envelope_goal_id, other_goal);
            assert_eq!(*state_goal_id, goal_id);
        }
        other => panic!("expected CrossGoal reject reason, got {other:?}"),
    }
}

/// Scenario (Codex pass-6 P2#4): the doc-required budget summary
/// (`docs/improvement/opencode.md` line 1519) is now part of the
/// completion-report wire shape. A round-trip through serde keeps
/// every field — pinning regressions that drop one of the three
/// budget-summary fields.
#[test]
fn completion_report_budget_summary_round_trips() {
    let spec = fixture_spec();
    let goal_id = spec.goal_id;
    let report = GoalCompletionReport {
        summary: "shipped with budget metrics".to_string(),
        completed_criteria: vec!["compiles".to_string(), "tests".to_string()],
        evidence_refs: vec![],
        verification: vec![],
        residual_risks: vec!["coverage report still pending".to_string()],
        changed_files: vec!["src/feature.rs".to_string()],
        total_spent_micro_usd: 4_750_000,
        elapsed_wall_clock_seconds: 1_810,
        continuation_loops_used: 7,
        finalised_at: fixture_now(),
        finalised_by: GoalActor::System {
            reason: "verifier accepted".to_string(),
        },
    };
    let env = envelope(goal_id, GoalEvent::Completed(report.clone()));
    let json = serde_json::to_string(&env).expect("serialize");
    let back: GoalEventEnvelope = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(env, back);
    if let GoalEvent::Completed(r) = back.event {
        assert_eq!(r.total_spent_micro_usd, 4_750_000);
        assert_eq!(r.elapsed_wall_clock_seconds, 1_810);
        assert_eq!(r.continuation_loops_used, 7);
    } else {
        panic!("expected GoalEvent::Completed");
    }
}

/// Scenario (Codex pass-7 P2): a forged JSONL stream with thousands
/// of cross-goal envelopes after the legitimate `Created` must not
/// turn replay diagnostics into an unbounded memory sink. The cap
/// at [`MAX_REPLAY_REJECTIONS`] retains the first N entries and
/// records the overflow count separately so the supervisor can
/// render "and N more rejections" without amplifying the attack.
#[test]
fn replay_caps_rejection_vec_at_max_replay_rejections() {
    let spec = fixture_spec();
    let goal_id = spec.goal_id;
    let other_goal = Uuid::parse_str("00000000-0000-0000-0000-0000abcd0000").unwrap();
    // Build the legitimate `Created` plus
    // MAX_REPLAY_REJECTIONS + 50 cross-goal envelopes (well past
    // the cap so we exercise the saturating overflow counter).
    let bad_count = MAX_REPLAY_REJECTIONS + 50;
    let mut envelopes: Vec<GoalEventEnvelope> = Vec::with_capacity(bad_count + 1);
    envelopes.push(envelope(goal_id, GoalEvent::Created(spec)));
    for i in 0..bad_count {
        envelopes.push(envelope(
            other_goal,
            GoalEvent::StepStarted {
                step_id: format!("foreign-step-{i}"),
            },
        ));
    }
    let outcome = replay(envelopes.iter()).expect("replay must succeed");
    assert_eq!(
        outcome.rejected.len(),
        MAX_REPLAY_REJECTIONS,
        "rejected vec must be capped at MAX_REPLAY_REJECTIONS",
    );
    assert_eq!(
        outcome.truncated_rejection_count,
        bad_count - MAX_REPLAY_REJECTIONS,
        "truncated_rejection_count must record the overflow exactly",
    );
    // Every retained rejection is a CrossGoal — pins that the cap
    // does not skip the wrong reasons silently.
    assert!(
        outcome
            .rejected
            .iter()
            .all(|r| matches!(r.reason, GoalApplyReject::CrossGoal { .. })),
        "all retained rejections must be CrossGoal",
    );
}

/// Scenario (Codex pass-7 P3): a duplicate `Created` envelope after
/// the seed surfaces as `DuplicateCreated` — a corrupted stream
/// would previously no-op the duplicate and silently advance
/// `updated_at` past a non-mutation, masking the corruption from
/// the supervisor's snapshot diff.
#[test]
fn replay_surfaces_duplicate_created_envelope_as_rejection() {
    let spec = fixture_spec();
    let goal_id = spec.goal_id;
    let envelopes = [
        envelope(goal_id, GoalEvent::Created(spec.clone())),
        // Second Created — must be rejected.
        envelope(goal_id, GoalEvent::Created(spec)),
    ];
    let outcome = replay(envelopes.iter()).expect("replay must succeed");
    assert_eq!(outcome.rejected.len(), 1);
    assert_eq!(
        outcome.rejected[0].reason,
        GoalApplyReject::DuplicateCreated
    );
}

/// Scenario (Codex pass-7 P1): a `Completed` envelope that arrives
/// without a prior `CompletionClaimed` — i.e. the verifier never
/// ran — surfaces as `MissingCompletionClaim`, not as a successful
/// transition. Pins the doc state machine
/// `Created -> ... -> CompletionClaimed -> verifier -> Completed`.
#[test]
fn replay_rejects_completed_without_pending_claim() {
    let spec = fixture_spec();
    let goal_id = spec.goal_id;
    let report = GoalCompletionReport {
        summary: "shipped".to_string(),
        completed_criteria: vec!["compiles".to_string(), "tests".to_string()],
        evidence_refs: vec![
            GoalEvidenceRef {
                criterion_id: Some("compiles".to_string()),
                target: GoalEvidenceTarget::File {
                    path: "src/feature.rs".to_string(),
                    sha256: "deadbeef".to_string(),
                },
                description: "edit landed".to_string(),
            },
            GoalEvidenceRef {
                criterion_id: Some("tests".to_string()),
                target: GoalEvidenceTarget::File {
                    path: "tests/feature.rs".to_string(),
                    sha256: "cafef00d".to_string(),
                },
                description: "test landed".to_string(),
            },
        ],
        verification: vec![],
        residual_risks: vec![],
        changed_files: vec![],
        total_spent_micro_usd: 0,
        elapsed_wall_clock_seconds: 0,
        continuation_loops_used: 0,
        finalised_at: fixture_now(),
        finalised_by: GoalActor::System {
            reason: "no claim ever filed".to_string(),
        },
    };
    let envelopes = [
        envelope(goal_id, GoalEvent::Created(spec)),
        envelope(goal_id, GoalEvent::Completed(report)),
    ];
    let outcome = replay(envelopes.iter()).expect("replay must succeed");
    assert_eq!(
        outcome.state.status,
        GoalStatus::Active,
        "Completed without a prior CompletionClaimed must NOT transition the Goal",
    );
    assert!(outcome.state.completion_report.is_none());
    assert_eq!(
        outcome.rejected.len(),
        1,
        "the unclaimed Completed must surface as a rejection diagnostic",
    );
    assert_eq!(
        outcome.rejected[0].reason,
        GoalApplyReject::MissingCompletionClaim,
        "rejection reason must be MissingCompletionClaim",
    );
}

/// Scenario (Codex pass-7 P1#4): a `Completed` envelope whose
/// reported total_spent exceeds the spec's hard cap surfaces as
/// `BudgetSpendOverrun` — the doc forbids transitioning to
/// `Completed` once a budget cap is exhausted.
#[test]
fn replay_rejects_completed_with_budget_overrun() {
    let mut spec = fixture_spec();
    // Override the fixture's default budget so we exercise the
    // overrun gate without pulling in spec construction
    // boilerplate.
    spec.budget = libra::internal::ai::goal::GoalBudget {
        hard_cap_micro_usd: 1_000_000,
        warn_threshold_micro_usd: 500_000,
        wall_clock_seconds: 0,
        max_continuation_loops: 16,
    };
    let goal_id = spec.goal_id;
    let legit_claim = GoalCompletionClaim {
        summary: "claim".to_string(),
        completed_criteria: vec!["compiles".to_string(), "tests".to_string()],
        evidence_refs: vec![],
        verification: vec![],
        residual_risks: vec![],
    };
    let report = GoalCompletionReport {
        summary: "over budget".to_string(),
        completed_criteria: vec!["compiles".to_string(), "tests".to_string()],
        evidence_refs: vec![
            GoalEvidenceRef {
                criterion_id: Some("compiles".to_string()),
                target: GoalEvidenceTarget::File {
                    path: "src/feature.rs".to_string(),
                    sha256: "deadbeef".to_string(),
                },
                description: "edit landed".to_string(),
            },
            GoalEvidenceRef {
                criterion_id: Some("tests".to_string()),
                target: GoalEvidenceTarget::File {
                    path: "tests/feature.rs".to_string(),
                    sha256: "cafef00d".to_string(),
                },
                description: "test landed".to_string(),
            },
        ],
        verification: vec![],
        residual_risks: vec![],
        changed_files: vec![],
        total_spent_micro_usd: 5_000_000,
        elapsed_wall_clock_seconds: 0,
        continuation_loops_used: 0,
        finalised_at: fixture_now(),
        finalised_by: GoalActor::System {
            reason: "forged terminal at over-budget".to_string(),
        },
    };
    let envelopes = [
        envelope(goal_id, GoalEvent::Created(spec)),
        envelope(goal_id, GoalEvent::CompletionClaimed(legit_claim)),
        envelope(goal_id, GoalEvent::Completed(report)),
    ];
    let outcome = replay(envelopes.iter()).expect("replay must succeed");
    assert_eq!(outcome.state.status, GoalStatus::CompletionClaimed);
    assert_eq!(outcome.rejected.len(), 1);
    match &outcome.rejected[0].reason {
        GoalApplyReject::InvalidCompletionReport { source } => {
            assert_eq!(
                source,
                &GoalCompletionShapeError::BudgetSpendOverrun {
                    reported: 5_000_000,
                    cap: 1_000_000,
                },
            );
        }
        other => panic!("expected InvalidCompletionReport(BudgetSpendOverrun), got {other:?}"),
    }
}
