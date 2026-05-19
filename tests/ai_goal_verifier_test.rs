//! OC-Phase 6 P6.2 — deterministic GoalVerifier integration tests.
//!
//! These tests pin the verifier's public contract from outside the
//! crate, using the `libra::internal::ai::goal` re-exports the
//! supervisor (P6.3) will consume. They exercise the verifier's six
//! rules end-to-end with a hand-rolled `GoalVerifierContext` so we
//! can stage workspace + tool-result fixtures without touching the
//! filesystem.

use std::{cell::RefCell, collections::BTreeMap};

use chrono::{DateTime, TimeZone, Utc};
use libra::internal::ai::goal::{
    DeterministicGoalVerifier, GoalActor, GoalBudget, GoalCompletionClaim, GoalCriterion,
    GoalEvidencePolicy, GoalEvidenceRef, GoalEvidenceTarget, GoalSpec, GoalVerificationRecord,
    GoalVerifier, GoalVerifierContext, GoalVerifyOutcome, RecentToolCall, ToolResultStatus,
};
use uuid::Uuid;

fn fixture_now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 5, 8, 13, 0, 0).unwrap()
}

fn fixture_claim_envelope_id() -> Uuid {
    Uuid::parse_str("00000000-0000-0000-0000-0000c1a10042").unwrap()
}

fn fixture_spec_with_criteria(
    criteria: Vec<GoalCriterion>,
    policy: GoalEvidencePolicy,
) -> GoalSpec {
    GoalSpec::new(
        Uuid::parse_str("00000000-0000-0000-0000-000000000099").unwrap(),
        "thread-test",
        "session-test",
        "ship feature",
        criteria,
        Vec::new(),
        policy,
        GoalBudget::default(),
        fixture_now(),
        GoalActor::User { id: None },
    )
    .expect("happy-path spec must construct")
}

struct CtxFixture {
    file_hashes: RefCell<BTreeMap<String, String>>,
    tool_results: RefCell<Vec<RecentToolCall>>,
    changed_files: RefCell<Vec<String>>,
    spend_micro_usd: u64,
    elapsed_seconds: u64,
    loops_used: u32,
}

impl CtxFixture {
    fn new() -> Self {
        Self {
            file_hashes: RefCell::new(BTreeMap::new()),
            tool_results: RefCell::new(Vec::new()),
            changed_files: RefCell::new(Vec::new()),
            spend_micro_usd: 250_000,
            elapsed_seconds: 600,
            loops_used: 2,
        }
    }

    fn with_file_change(self, path: &str, sha256: &str) -> Self {
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

impl GoalVerifierContext for CtxFixture {
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
        self.spend_micro_usd
    }

    fn elapsed_wall_clock_seconds(&self) -> u64 {
        self.elapsed_seconds
    }

    fn continuation_loops_used(&self) -> u32 {
        self.loops_used
    }
}

/// End-to-end: a well-formed claim against a workspace-change spec
/// produces an Accept whose report carries the verifier's stamped
/// fields verbatim. Pins the legitimate flow the supervisor (P6.3)
/// will rely on.
#[test]
fn accepts_well_formed_claim_and_stamps_report() {
    let spec = fixture_spec_with_criteria(
        vec![GoalCriterion {
            id: "patch".to_string(),
            description: "edit landed".to_string(),
            required: true,
            verifier_hint: None,
            requires_workspace_change: true,
        }],
        GoalEvidencePolicy::Standard,
    );
    let claim = GoalCompletionClaim {
        summary: "shipped".to_string(),
        completed_criteria: vec!["patch".to_string()],
        evidence_refs: vec![GoalEvidenceRef {
            criterion_id: Some("patch".to_string()),
            target: GoalEvidenceTarget::File {
                path: "src/feature.rs".to_string(),
                sha256: "deadbeef".to_string(),
            },
            description: "edit".to_string(),
        }],
        verification: vec![GoalVerificationRecord {
            criterion_id: "patch".to_string(),
            method: "cargo check".to_string(),
            passed: true,
            output_summary: Some("clean".to_string()),
        }],
        residual_risks: Vec::new(),
    };
    let ctx = CtxFixture::new().with_file_change("src/feature.rs", "deadbeef");
    let outcome =
        DeterministicGoalVerifier.verify(&ctx, &spec, &claim, fixture_claim_envelope_id());
    let GoalVerifyOutcome::Accept(report) = outcome else {
        panic!("expected Accept, got {outcome:?}");
    };
    assert_eq!(report.claim_envelope_id, fixture_claim_envelope_id());
    assert_eq!(report.completed_criteria, vec!["patch".to_string()]);
    assert_eq!(report.changed_files, vec!["src/feature.rs".to_string()]);
    assert_eq!(report.total_spent_micro_usd, 250_000);
    assert_eq!(report.elapsed_wall_clock_seconds, 600);
    assert_eq!(report.continuation_loops_used, 2);
    assert_eq!(report.verification.len(), 1);
}

/// Rule 1: a required criterion missing from the claim → Reject
/// with the missing id called out.
#[test]
fn rejects_when_required_criterion_missing_from_claim() {
    let spec = fixture_spec_with_criteria(
        vec![
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
        ],
        GoalEvidencePolicy::Standard,
    );
    let claim = GoalCompletionClaim {
        summary: "partial".to_string(),
        completed_criteria: vec!["compiles".to_string()],
        evidence_refs: vec![GoalEvidenceRef {
            criterion_id: Some("compiles".to_string()),
            target: GoalEvidenceTarget::ToolCall {
                call_id: "tc-1".to_string(),
            },
            description: "ran cargo check".to_string(),
        }],
        verification: vec![GoalVerificationRecord {
            criterion_id: "compiles".to_string(),
            method: "cargo check".to_string(),
            passed: true,
            output_summary: None,
        }],
        residual_risks: Vec::new(),
    };
    let ctx = CtxFixture::new();
    let outcome =
        DeterministicGoalVerifier.verify(&ctx, &spec, &claim, fixture_claim_envelope_id());
    let GoalVerifyOutcome::Reject { missing, .. } = outcome else {
        panic!("expected Reject, got {outcome:?}");
    };
    assert_eq!(missing, vec!["tests".to_string()]);
}

/// Rule 5: a recent tool call with `Failed` status blocks
/// completion. The reason names the failing call_id so the
/// continuation prompt can surface it.
#[test]
fn rejects_when_recent_tool_result_failed() {
    let spec = fixture_spec_with_criteria(
        vec![GoalCriterion {
            id: "compiles".to_string(),
            description: "x".to_string(),
            required: true,
            verifier_hint: None,
            requires_workspace_change: false,
        }],
        GoalEvidencePolicy::Standard,
    );
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
    let ctx = CtxFixture::new().with_tool_result("tc-failed", ToolResultStatus::Failed);
    let outcome =
        DeterministicGoalVerifier.verify(&ctx, &spec, &claim, fixture_claim_envelope_id());
    let GoalVerifyOutcome::Reject { reason, .. } = outcome else {
        panic!("expected Reject, got {outcome:?}");
    };
    assert!(
        reason.contains("tc-failed"),
        "reason should name failing call: {reason}"
    );
}

/// Rule 3: a workspace-change criterion whose evidence sha256 does
/// not match the on-disk hash → Reject. The schema floor cannot
/// enforce this (it doesn't read disk); the verifier is the only
/// gate that can.
#[test]
fn rejects_workspace_change_with_stale_file_sha256() {
    let spec = fixture_spec_with_criteria(
        vec![GoalCriterion {
            id: "patch".to_string(),
            description: "x".to_string(),
            required: true,
            verifier_hint: None,
            requires_workspace_change: true,
        }],
        GoalEvidencePolicy::Standard,
    );
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
    // Disk hash differs.
    let ctx = CtxFixture::new().with_file_change("src/feature.rs", "fresh-hash");
    let outcome =
        DeterministicGoalVerifier.verify(&ctx, &spec, &claim, fixture_claim_envelope_id());
    let GoalVerifyOutcome::Reject { missing, .. } = outcome else {
        panic!("expected Reject, got {outcome:?}");
    };
    assert_eq!(missing, vec!["patch".to_string()]);
}

/// Rule 6: workspace-change criterion satisfied by an explicit
/// `NoChangesNeeded` rationale (research-only acceptance).
#[test]
fn accepts_workspace_change_via_no_changes_needed_evidence() {
    let spec = fixture_spec_with_criteria(
        vec![GoalCriterion {
            id: "investigation".to_string(),
            description: "verify spec correctness".to_string(),
            required: true,
            verifier_hint: None,
            requires_workspace_change: true,
        }],
        GoalEvidencePolicy::Standard,
    );
    let claim = GoalCompletionClaim {
        summary: "no change required".to_string(),
        completed_criteria: vec!["investigation".to_string()],
        evidence_refs: vec![GoalEvidenceRef {
            criterion_id: Some("investigation".to_string()),
            target: GoalEvidenceTarget::NoChangesNeeded {
                rationale: "spec already says what was claimed".to_string(),
            },
            description: "research-only".to_string(),
        }],
        verification: vec![GoalVerificationRecord {
            criterion_id: "investigation".to_string(),
            method: "manual review".to_string(),
            passed: true,
            output_summary: None,
        }],
        residual_risks: Vec::new(),
    };
    let ctx = CtxFixture::new();
    let outcome =
        DeterministicGoalVerifier.verify(&ctx, &spec, &claim, fixture_claim_envelope_id());
    assert!(matches!(outcome, GoalVerifyOutcome::Accept(_)));
}

/// DocumentationOnly policy relaxes Rule 4 (verification) and the
/// per-criterion evidence floor (Rule 2). The verifier accepts a
/// claim with no structured evidence + no verification when the
/// policy permits narrative acceptance.
#[test]
fn documentation_only_policy_accepts_narrative_claim() {
    let spec = fixture_spec_with_criteria(
        vec![GoalCriterion {
            id: "decision".to_string(),
            description: "ADR drafted".to_string(),
            required: true,
            verifier_hint: None,
            requires_workspace_change: false,
        }],
        GoalEvidencePolicy::DocumentationOnly,
    );
    let claim = GoalCompletionClaim {
        summary: "ADR captured the decision".to_string(),
        completed_criteria: vec!["decision".to_string()],
        evidence_refs: Vec::new(),
        verification: Vec::new(),
        residual_risks: Vec::new(),
    };
    let ctx = CtxFixture::new();
    let outcome =
        DeterministicGoalVerifier.verify(&ctx, &spec, &claim, fixture_claim_envelope_id());
    assert!(matches!(outcome, GoalVerifyOutcome::Accept(_)));
}
