//! OC-Phase 6 P6.7 — completion gate scenarios.
//!
//! Spec: `docs/improvement/opencode.md` lines 670-681 + 1722.
//!
//! S6 names two test files: `tests/ai_goal_supervisor_test.rs`
//! drives the supervisor's three-turn happy path; this file pins
//! the deterministic verifier's *rejection* paths from the same
//! doc (Rule 2 evidence floor, Rule 3 workspace-change hash check,
//! Rule 5 recent-tool-failure). Each test is structured the way
//! P6.5/P6.6 surfaces will exercise it: build a real
//! `GoalCompletionClaim`, call the verifier through its public
//! trait method, assert the rejection reason carries the missing
//! id and a precise reason string the continuation prompt builder
//! can render verbatim.

use std::cell::RefCell;

use chrono::{DateTime, TimeZone, Utc};
use libra::internal::ai::goal::{
    DeterministicGoalVerifier, GoalActor, GoalBudget, GoalCompletionClaim, GoalCriterion,
    GoalEvidencePolicy, GoalEvidenceRef, GoalEvidenceTarget, GoalSpec, GoalVerificationRecord,
    GoalVerifier, GoalVerifierContext, GoalVerifyOutcome, RecentToolCall, ToolResultStatus,
};
use uuid::Uuid;

fn fixture_now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 5, 9, 13, 0, 0).unwrap()
}

fn fixture_spec(criteria: Vec<GoalCriterion>) -> GoalSpec {
    GoalSpec::new(
        Uuid::parse_str("00000000-0000-0000-0000-0000000056e7").unwrap(),
        "thread-gate",
        "session-gate",
        "ship feature X".to_string(),
        criteria,
        Vec::new(),
        GoalEvidencePolicy::Standard,
        GoalBudget::default(),
        fixture_now(),
        GoalActor::User { id: None },
    )
    .expect("happy-path spec must construct")
}

struct GateCtx {
    file_hashes: RefCell<std::collections::BTreeMap<String, String>>,
    tool_results: RefCell<Vec<RecentToolCall>>,
    changed_files: RefCell<Vec<String>>,
}

impl GateCtx {
    fn new() -> Self {
        Self {
            file_hashes: RefCell::new(std::collections::BTreeMap::new()),
            tool_results: RefCell::new(Vec::new()),
            changed_files: RefCell::new(Vec::new()),
        }
    }

    fn with_file(self, path: &str, sha256: &str) -> Self {
        self.file_hashes
            .borrow_mut()
            .insert(path.to_string(), sha256.to_string());
        self.changed_files.borrow_mut().push(path.to_string());
        self
    }

    fn with_tool_result(self, call_id: &str, status: ToolResultStatus) -> Self {
        self.tool_results.borrow_mut().push(RecentToolCall {
            call_id: call_id.to_string(),
            tool_name: "shell".to_string(),
            status,
        });
        self
    }
}

impl GoalVerifierContext for GateCtx {
    fn file_sha256(&self, path: &str) -> Option<String> {
        self.file_hashes.borrow().get(path).cloned()
    }
    fn recent_tool_results(&self) -> Vec<RecentToolCall> {
        self.tool_results.borrow().clone()
    }
    fn changed_files(&self) -> Vec<String> {
        self.changed_files.borrow().clone()
    }
    fn now(&self) -> DateTime<Utc> {
        fixture_now()
    }
    fn finalised_by(&self) -> GoalActor {
        GoalActor::System {
            reason: "deterministic verifier accepted".to_string(),
        }
    }
    fn total_spent_micro_usd(&self) -> u64 {
        500_000
    }
    fn elapsed_wall_clock_seconds(&self) -> u64 {
        300
    }
    fn continuation_loops_used(&self) -> u32 {
        2
    }
}

fn fixture_claim_envelope_id() -> Uuid {
    Uuid::parse_str("00000000-0000-0000-0000-0000c1a10056").unwrap()
}

/// Doc Rule 1 / opencode.md:674 — every required criterion must
/// appear in `claim.completed_criteria`.
#[test]
fn rejects_claim_missing_required_criterion() {
    let spec = fixture_spec(vec![
        GoalCriterion {
            id: "compiles".to_string(),
            description: "x".to_string(),
            required: true,
            verifier_hint: None,
            requires_workspace_change: false,
        },
        GoalCriterion {
            id: "tests".to_string(),
            description: "x".to_string(),
            required: true,
            verifier_hint: None,
            requires_workspace_change: false,
        },
    ]);
    let claim = GoalCompletionClaim {
        summary: "partial".to_string(),
        completed_criteria: vec!["compiles".to_string()],
        evidence_refs: vec![GoalEvidenceRef {
            criterion_id: Some("compiles".to_string()),
            target: GoalEvidenceTarget::ToolCall {
                call_id: "tc-1".to_string(),
            },
            description: "x".to_string(),
        }],
        verification: vec![GoalVerificationRecord {
            criterion_id: "compiles".to_string(),
            method: "cargo check".to_string(),
            passed: true,
            output_summary: None,
        }],
        residual_risks: Vec::new(),
    };
    let ctx = GateCtx::new();
    let outcome =
        DeterministicGoalVerifier.verify(&ctx, &spec, &claim, fixture_claim_envelope_id());
    let GoalVerifyOutcome::Reject { missing, .. } = outcome else {
        panic!("expected Reject, got {outcome:?}");
    };
    assert_eq!(missing, vec!["tests".to_string()]);
}

/// Doc Rule 3 / opencode.md:676 — workspace-change criteria need
/// a `File` evidence ref whose sha256 matches the file's current
/// on-disk hash. A stale hash blocks completion until the model
/// re-runs the verification.
#[test]
fn rejects_workspace_change_with_stale_file_sha256() {
    let spec = fixture_spec(vec![GoalCriterion {
        id: "patch".to_string(),
        description: "edit landed".to_string(),
        required: true,
        verifier_hint: None,
        requires_workspace_change: true,
    }]);
    let claim = GoalCompletionClaim {
        summary: "stale".to_string(),
        completed_criteria: vec!["patch".to_string()],
        evidence_refs: vec![GoalEvidenceRef {
            criterion_id: Some("patch".to_string()),
            target: GoalEvidenceTarget::File {
                path: "src/feature.rs".to_string(),
                sha256: "stale-hash".to_string(),
            },
            description: "claim's hash is out of date".to_string(),
        }],
        verification: vec![GoalVerificationRecord {
            criterion_id: "patch".to_string(),
            method: "cargo check".to_string(),
            passed: true,
            output_summary: None,
        }],
        residual_risks: Vec::new(),
    };
    let ctx = GateCtx::new().with_file("src/feature.rs", "fresh-hash");
    let outcome =
        DeterministicGoalVerifier.verify(&ctx, &spec, &claim, fixture_claim_envelope_id());
    let GoalVerifyOutcome::Reject { missing, .. } = outcome else {
        panic!("expected Reject, got {outcome:?}");
    };
    assert_eq!(missing, vec!["patch".to_string()]);
}

/// Doc Rule 5 / opencode.md:678 — any non-success in recent tool
/// results blocks acceptance. The reason must name the failing
/// call so the continuation prompt (P6.3) can surface it.
#[test]
fn rejects_when_recent_tool_call_failed() {
    let spec = fixture_spec(vec![GoalCriterion {
        id: "compiles".to_string(),
        description: "cargo check".to_string(),
        required: true,
        verifier_hint: None,
        requires_workspace_change: false,
    }]);
    let claim = GoalCompletionClaim {
        summary: "claim".to_string(),
        completed_criteria: vec!["compiles".to_string()],
        evidence_refs: vec![GoalEvidenceRef {
            criterion_id: Some("compiles".to_string()),
            target: GoalEvidenceTarget::ToolCall {
                call_id: "tc-1".to_string(),
            },
            description: "x".to_string(),
        }],
        verification: vec![GoalVerificationRecord {
            criterion_id: "compiles".to_string(),
            method: "cargo check".to_string(),
            passed: true,
            output_summary: None,
        }],
        residual_risks: Vec::new(),
    };
    let ctx = GateCtx::new().with_tool_result("tc-flaky", ToolResultStatus::Failed);
    let outcome =
        DeterministicGoalVerifier.verify(&ctx, &spec, &claim, fixture_claim_envelope_id());
    let GoalVerifyOutcome::Reject { reason, .. } = outcome else {
        panic!("expected Reject, got {outcome:?}");
    };
    assert!(
        reason.contains("tc-flaky"),
        "rejection reason must name the failing call_id: {reason}",
    );
}

/// Doc Rule 4 / opencode.md:677 — under Standard policy a claim
/// must include at least one verification record. Empty
/// `verification` is rejected; the model is expected to emit at
/// least one method/passed record per claim.
#[test]
fn rejects_empty_verification_under_standard_policy() {
    let spec = fixture_spec(vec![GoalCriterion {
        id: "compiles".to_string(),
        description: "x".to_string(),
        required: true,
        verifier_hint: None,
        requires_workspace_change: false,
    }]);
    let claim = GoalCompletionClaim {
        summary: "no verification".to_string(),
        completed_criteria: vec!["compiles".to_string()],
        evidence_refs: vec![GoalEvidenceRef {
            criterion_id: Some("compiles".to_string()),
            target: GoalEvidenceTarget::ToolCall {
                call_id: "tc-1".to_string(),
            },
            description: "x".to_string(),
        }],
        verification: Vec::new(),
        residual_risks: Vec::new(),
    };
    let ctx = GateCtx::new();
    let outcome =
        DeterministicGoalVerifier.verify(&ctx, &spec, &claim, fixture_claim_envelope_id());
    assert!(
        matches!(outcome, GoalVerifyOutcome::Reject { ref reason, .. }
            if reason.contains("verification records")),
        "expected verification-empty rejection, got {outcome:?}",
    );
}
