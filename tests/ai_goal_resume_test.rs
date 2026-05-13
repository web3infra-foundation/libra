//! OC-Phase 6 P6.7 — resume replay determinism.
//!
//! Spec: `docs/improvement/opencode.md` lines 1499 ("--resume <thread>
//! replay active Goal").
//!
//! When the user runs `libra code --resume <thread>` the supervisor
//! reads the session JSONL, filters for `SessionEvent::Goal` rows,
//! and feeds the resulting `GoalEventEnvelope` slice into
//! [`replay`]. The output `GoalState` MUST be byte-stable equal to
//! the state the original supervisor ran against — otherwise a
//! resumed Goal would diverge from the live one and the verifier
//! could see different inputs.
//!
//! This test pins that determinism by:
//!
//! 1. Constructing the full S6 envelope sequence (Created →
//!    ProgressRecorded → CompletionClaimed → CompletionRejected →
//!    CompletionClaimed → Completed) by hand using stable Uuids
//!    and timestamps.
//! 2. Calling `replay()` on the slice and asserting the resulting
//!    state's `status`, `completed_criteria`, `evidence_refs`
//!    count, `pending_claim`, and `completion_report` match the
//!    in-line constructed expectation.
//! 3. Running `replay()` a second time on the same slice and
//!    asserting byte-equal state — confirms determinism even
//!    when the schema's apply path is exercised twice.

use chrono::{DateTime, Duration, TimeZone, Utc};
use libra::internal::ai::goal::{
    GoalActor, GoalBudget, GoalCompletionClaim, GoalCompletionReport, GoalCriterion, GoalEvent,
    GoalEventEnvelope, GoalEvidencePolicy, GoalEvidenceRef, GoalEvidenceTarget, GoalProgressRecord,
    GoalSpec, GoalStatus, GoalVerificationRecord, replay,
};
use uuid::Uuid;

fn fixture_now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 5, 9, 13, 0, 0).unwrap()
}

fn fixture_spec() -> GoalSpec {
    GoalSpec::new(
        Uuid::parse_str("00000000-0000-0000-0000-0000000056e8").unwrap(),
        "thread-resume",
        "session-resume",
        "Add unit test for utils::path::join".to_string(),
        vec![
            GoalCriterion {
                id: "test-added".to_string(),
                description: "new test in tests/path_join_test.rs".to_string(),
                required: true,
                verifier_hint: None,
                requires_workspace_change: true,
            },
            GoalCriterion {
                id: "tests-pass".to_string(),
                description: "cargo test --lib green".to_string(),
                required: true,
                verifier_hint: None,
                requires_workspace_change: false,
            },
        ],
        Vec::new(),
        GoalEvidencePolicy::Standard,
        GoalBudget::default(),
        fixture_now(),
        GoalActor::User { id: None },
    )
    .expect("resume spec must construct")
}

/// Build the full S6 lifecycle envelope sequence by hand. Every
/// envelope id and timestamp is fixed so the resulting state is
/// byte-stable across replay calls.
fn s6_lifecycle_envelopes(spec: &GoalSpec) -> Vec<GoalEventEnvelope> {
    let goal_id = spec.goal_id;
    let claim_envelope_id = Uuid::parse_str("00000000-0000-0000-0000-0000c1a10101").unwrap();
    let final_claim_envelope_id = Uuid::parse_str("00000000-0000-0000-0000-0000c1a10202").unwrap();
    let env = |envelope_id: Uuid, secs: i64, event: GoalEvent| -> GoalEventEnvelope {
        GoalEventEnvelope {
            envelope_id,
            goal_id,
            recorded_at: fixture_now() + Duration::seconds(secs),
            event,
        }
    };
    vec![
        // Turn 0: Created (seed envelope; recorded_at == spec.created_at).
        env(
            Uuid::parse_str("00000000-0000-0000-0000-0000c1a10001").unwrap(),
            0,
            GoalEvent::Created(spec.clone()),
        ),
        // Turn 1: model emits final text without claiming → supervisor
        // synthesised ProgressRecorded.
        env(
            Uuid::parse_str("00000000-0000-0000-0000-0000c1a10002").unwrap(),
            10,
            GoalEvent::ProgressRecorded(GoalProgressRecord {
                summary: "I think we're done.".to_string(),
                completed_criteria: Vec::new(),
                evidence_refs: Vec::new(),
                next_steps: Vec::new(),
            }),
        ),
        // Turn 2: incomplete claim, then verifier rejection.
        env(
            claim_envelope_id,
            20,
            GoalEvent::CompletionClaimed(GoalCompletionClaim {
                summary: "I think tests pass — verification still pending".to_string(),
                completed_criteria: vec!["test-added".to_string(), "tests-pass".to_string()],
                evidence_refs: Vec::new(),
                verification: vec![GoalVerificationRecord {
                    criterion_id: "test-added".to_string(),
                    method: "ran cargo test path_join in my head".to_string(),
                    passed: true,
                    output_summary: None,
                }],
                residual_risks: Vec::new(),
            }),
        ),
        env(
            Uuid::parse_str("00000000-0000-0000-0000-0000c1a10003").unwrap(),
            30,
            GoalEvent::CompletionRejected {
                claim_envelope_id,
                missing: vec!["test-added".to_string()],
                reason: "criterion `test-added` has no matching evidence under Standard policy"
                    .to_string(),
            },
        ),
        // Turn 3: complete claim, then accept.
        env(
            final_claim_envelope_id,
            40,
            GoalEvent::CompletionClaimed(GoalCompletionClaim {
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
                        method: "manual review".to_string(),
                        passed: true,
                        output_summary: None,
                    },
                    GoalVerificationRecord {
                        criterion_id: "tests-pass".to_string(),
                        method: "cargo test --lib".to_string(),
                        passed: true,
                        output_summary: None,
                    },
                ],
                residual_risks: Vec::new(),
            }),
        ),
        env(
            Uuid::parse_str("00000000-0000-0000-0000-0000c1a10004").unwrap(),
            50,
            GoalEvent::Completed(GoalCompletionReport {
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
                        method: "manual review".to_string(),
                        passed: true,
                        output_summary: None,
                    },
                    GoalVerificationRecord {
                        criterion_id: "tests-pass".to_string(),
                        method: "cargo test --lib".to_string(),
                        passed: true,
                        output_summary: None,
                    },
                ],
                residual_risks: Vec::new(),
                changed_files: vec!["tests/path_join_test.rs".to_string()],
                claim_envelope_id: final_claim_envelope_id,
                total_spent_micro_usd: 750_000,
                elapsed_wall_clock_seconds: 900,
                continuation_loops_used: 2,
                finalised_at: fixture_now() + Duration::seconds(50),
                finalised_by: GoalActor::System {
                    reason: "deterministic verifier accepted".to_string(),
                },
            }),
        ),
    ]
}

/// `replay()` on the full S6 envelope sequence reproduces the
/// exact terminal state the live supervisor would have built. The
/// resumed snapshot must show `status == Completed`, both required
/// criteria stamped into `completed_criteria`, the rejection
/// blocker preserved, and the completion report intact.
#[test]
fn replay_resumes_full_s6_lifecycle_to_completed() {
    let spec = fixture_spec();
    let envelopes = s6_lifecycle_envelopes(&spec);
    let outcome = replay(envelopes.iter()).expect("replay must succeed for full S6 stream");
    assert!(
        outcome.rejected.is_empty(),
        "no envelope should be rejected in the canonical S6 stream",
    );
    let state = outcome.state;
    assert_eq!(state.status, GoalStatus::Completed);
    assert!(state.status.is_terminal());
    assert!(state.completed_criteria.contains("test-added"));
    assert!(state.completed_criteria.contains("tests-pass"));
    assert_eq!(
        state.blockers.len(),
        1,
        "the rejection blocker from turn 2 must survive into the terminal snapshot",
    );
    assert!(state.completion_report.is_some());
    // Pending claim must be cleared by `Completed`.
    assert!(state.pending_claim.is_none());
}

/// Replay determinism: running `replay()` twice on the same
/// envelope slice produces byte-equal `GoalReplayOutcome` values.
/// Any non-determinism (HashSet iteration order, random ids in
/// derived fields) would surface here.
#[test]
fn replay_is_deterministic_across_two_invocations() {
    let spec = fixture_spec();
    let envelopes = s6_lifecycle_envelopes(&spec);
    let first = replay(envelopes.iter()).expect("first replay must succeed");
    let second = replay(envelopes.iter()).expect("second replay must succeed");
    assert_eq!(
        first, second,
        "replay must be a pure function of the envelope slice",
    );
}

/// `replay()` re-validates the seed `GoalSpec` per the pass-5 P1
/// fix — a corrupted Created envelope (here: a hand-constructed
/// spec with duplicate criterion ids) is refused at the resume
/// seam instead of producing nonsense state. Pin that behaviour
/// here too because resume is the one path most likely to feed
/// pre-existing corrupted state.
#[test]
fn replay_rejects_resumed_stream_with_corrupted_spec() {
    let goal_id = Uuid::parse_str("00000000-0000-0000-0000-0000000056e9").unwrap();
    // Hand-craft a `Created` envelope whose embedded spec
    // bypasses `GoalSpec::new`'s validation by using
    // `serde_json::from_value` to reconstruct a spec with
    // duplicate criterion ids. `replay()` must refuse the slice
    // with `None` rather than seed a state from the corrupted
    // spec.
    let payload = serde_json::json!({
        "envelope_id": "00000000-0000-0000-0000-0000feed0001",
        "goal_id": goal_id,
        "recorded_at": fixture_now().to_rfc3339(),
        "event": {
            "kind": "created",
            "goal_id": goal_id,
            "thread_id": "thread-bad",
            "session_id": "session-bad",
            "objective": "corrupted spec for resume test",
            "acceptance_criteria": [
                {"id": "dup", "description": "first", "required": true},
                {"id": "dup", "description": "second", "required": true}
            ],
            "constraints": [],
            "evidence_policy": "standard",
            "budget": {
                "hard_cap_micro_usd": 0,
                "warn_threshold_micro_usd": 0,
                "wall_clock_seconds": 0,
                "max_continuation_loops": 16
            },
            "created_at": fixture_now().to_rfc3339(),
            "created_by": {"kind": "user", "id": null}
        }
    });
    let envelope: GoalEventEnvelope =
        serde_json::from_value(payload).expect("hand-crafted Created envelope must deserialise");
    let envelopes = [envelope];
    assert!(
        replay(envelopes.iter()).is_none(),
        "replay must reject a Created envelope whose embedded spec carries duplicate ids",
    );
}
