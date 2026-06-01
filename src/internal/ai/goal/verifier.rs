//! Goal verifier — deterministic completion gate.
//!
//! Per `docs/improvement/opencode.md` lines 670-681, the first-version
//! verifier is **deterministic + evidence-based**: it does not call a
//! model to make the final accept/reject decision. The supervisor
//! (P6.3) calls [`GoalVerifier::verify`] every time the model emits
//! `submit_goal_complete` (which the runtime turns into a
//! `GoalEvent::CompletionClaimed` envelope). On accept the verifier
//! produces a [`GoalCompletionReport`] the supervisor wraps in a
//! `GoalEvent::Completed`; on reject it returns missing items + a
//! reason that flow into `GoalEvent::CompletionRejected`.
//!
//! # Rules (opencode.md:670-681)
//!
//! 1. Every `required = true` criterion in the spec appears in
//!    `claim.completed_criteria`.
//! 2. Each required criterion has at least one matching evidence ref
//!    (same `criterion_id`, target other than the unknown
//!    `Future` catch-all).
//! 3. For criteria with
//!    [`super::spec::GoalCriterion::requires_workspace_change`], at
//!    least one matching evidence ref carries a workspace-bound
//!    target — `GoalEvidenceTarget::File` whose `sha256` matches the
//!    file's current on-disk hash, OR a `NoChangesNeeded` ref with
//!    a rationale (research-only acceptance).
//! 4. `claim.verification` is non-empty under
//!    [`super::spec::GoalEvidencePolicy::Standard`]. The
//!    `DocumentationOnly` policy relaxes this so narrative
//!    `verification` records can stand in for structured evidence.
//! 5. The most recent tool results known to the supervisor (passed
//!    via [`GoalVerifierContext::recent_tool_results`]) must not
//!    contain a `Failed`, `Denied`, or `TimedOut` entry — otherwise
//!    the verifier rejects with a reason that points at the failing
//!    call. The supervisor decides how many "recent" entries to
//!    surface; the verifier examines all of them.
//! 6. For workspace-change criteria: either the workspace shows a
//!    change (`ctx.changed_files()` non-empty for that criterion's
//!    declared `File` evidence path) or a `NoChangesNeeded` ref
//!    explains the absence.
//!
//! Schema floors (already enforced by
//! [`super::event::validate_completion_claim_shape`] /
//! [`super::event::validate_completion_report_shape`]) are still
//! re-applied here so the verifier rejects malformed claims with the
//! same rejection reason the schema would have surfaced. This is
//! defense-in-depth: a bug in the supervisor that hands the verifier
//! a payload that bypassed the apply seam still gets caught.
//!
//! # Determinism contract
//!
//! `verify` is a pure function of `(spec, claim, claim_envelope_id,
//! ctx)`. Same inputs → same outcome on the same machine + workspace
//! state. The non-determinism boundary is `GoalVerifierContext`: it
//! reads disk and tool-call history, both of which can change
//! between calls. The supervisor (P6.3) is responsible for locking
//! the workspace + tool history between `submit_goal_complete` and
//! the verifier call so the determinism contract holds.
//!
//! # Why a trait
//!
//! The default implementation [`DeterministicGoalVerifier`] is
//! sufficient for OC-Phase 6. The trait shape lets P6.7 E2E tests
//! plug a fake verifier (record/replay) and lets a future LLM
//! reviewer (per opencode.md:681) layer in as advisory evidence
//! while the deterministic verifier remains the final gate.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::{
    event::{
        GoalCompletionClaim, GoalCompletionReport, GoalCompletionShapeError,
        validate_completion_claim_shape, validate_completion_report_shape,
    },
    spec::{GoalActor, GoalCriterion, GoalEvidencePolicy, GoalSpec},
    state::{GoalEvidenceRef, GoalEvidenceTarget},
};

/// Outcome of a single tool call as far as the verifier cares.
///
/// The supervisor maps its richer tool-result enum down to this set
/// before passing to [`GoalVerifierContext::recent_tool_results`].
/// The verifier rejects on any non-`Succeeded` entry — see the
/// module-level Rule 5.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolResultStatus {
    Succeeded,
    Failed,
    Denied,
    TimedOut,
}

/// Per-call summary the verifier surfaces in its rejection reason
/// when Rule 5 fires. Carries enough detail that the continuation
/// prompt (P6.3) can name the tool that blocked completion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecentToolCall {
    pub call_id: String,
    pub tool_name: String,
    pub status: ToolResultStatus,
}

/// Side-channel the verifier uses to read mutable world state
/// (workspace files, tool results, wall-clock, accumulated spend).
///
/// Implementations live in the supervisor (P6.3) and the test
/// harness; the verifier itself never touches `std::fs` or the
/// session store directly.
pub trait GoalVerifierContext {
    /// Hex sha256 of the file at `path`, or `None` if the file
    /// does not exist or cannot be read. Used by Rule 3 to confirm
    /// that a `GoalEvidenceTarget::File` evidence ref still
    /// describes the actual on-disk content. Implementations are
    /// expected to read the workspace root that the Goal was
    /// created against — relative paths in evidence refs resolve
    /// against that root.
    fn file_sha256(&self, path: &str) -> Option<String>;

    /// Tool calls the supervisor wants the verifier to consider for
    /// Rule 5, ordered most-recent-first. The verifier rejects on
    /// any non-`Succeeded` entry. The supervisor decides the depth;
    /// an empty list means "no recent tool calls" (e.g. a Goal
    /// that completed without invoking any tool).
    fn recent_tool_results(&self) -> Vec<RecentToolCall>;

    /// Workspace files the supervisor reports as changed since
    /// `spec.created_at` (e.g. derived from `git status --short`).
    /// Used by Rule 6 to confirm that a workspace-change criterion
    /// without a `NoChangesNeeded` ref actually mutated something.
    fn changed_files(&self) -> Vec<String>;

    /// Wall-clock instant the verifier should stamp into the
    /// generated report's `finalised_at`. The supervisor passes
    /// `Utc::now()` in production and a fixture timestamp in tests.
    fn now(&self) -> DateTime<Utc>;

    /// Actor on whose behalf the verifier is finalising. The
    /// supervisor sets this to `GoalActor::System { reason }` for
    /// the deterministic verifier so audit logs do not attribute
    /// the report to the human user.
    fn finalised_by(&self) -> GoalActor;

    /// Accumulated spend for this Goal so far, in micro-USD. Stamped
    /// into the report's `total_spent_micro_usd` budget summary.
    fn total_spent_micro_usd(&self) -> u64;

    /// Wall-clock seconds elapsed since `spec.created_at`. Stamped
    /// into the report's `elapsed_wall_clock_seconds`.
    fn elapsed_wall_clock_seconds(&self) -> u64;

    /// Continuation loops the supervisor consumed before this
    /// verification call. Stamped into the report's
    /// `continuation_loops_used`.
    fn continuation_loops_used(&self) -> u32;
}

/// Outcome of a single verifier call. `Accept` carries the report
/// the supervisor will emit in `GoalEvent::Completed`; `Reject`
/// carries the missing items + reason the supervisor will emit in
/// `GoalEvent::CompletionRejected` and surface in the next
/// continuation prompt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GoalVerifyOutcome {
    Accept(GoalCompletionReport),
    Reject {
        missing: Vec<String>,
        reason: String,
    },
}

/// Trait the supervisor implements (or substitutes in tests) so the
/// verifier can be swapped without touching the supervisor loop. The
/// default deterministic implementation lives below as
/// [`DeterministicGoalVerifier`]; P6.7 E2E tests will plug a fake
/// implementation for record/replay scenarios.
pub trait GoalVerifier {
    fn verify(
        &self,
        ctx: &dyn GoalVerifierContext,
        spec: &GoalSpec,
        claim: &GoalCompletionClaim,
        claim_envelope_id: Uuid,
    ) -> GoalVerifyOutcome;
}

/// Default verifier — applies the six rules from opencode.md:670-681
/// against `(spec, claim, ctx)`. The struct is unit because the
/// verifier holds no per-instance state; the trait shape is what
/// matters for swap-out.
#[derive(Clone, Copy, Debug, Default)]
pub struct DeterministicGoalVerifier;

impl GoalVerifier for DeterministicGoalVerifier {
    fn verify(
        &self,
        ctx: &dyn GoalVerifierContext,
        spec: &GoalSpec,
        claim: &GoalCompletionClaim,
        claim_envelope_id: Uuid,
    ) -> GoalVerifyOutcome {
        // Schema floor first — defense-in-depth against a supervisor
        // bug that bypassed the apply seam. The floor catches
        // duplicate / unknown ids and unknown verification refs.
        if let Err(source) = validate_completion_claim_shape(spec, claim) {
            return reject_for_shape(source);
        }

        // Rule 1: every required criterion in the spec must be
        // claimed. The schema floor doesn't enforce required-coverage
        // on claims (claims are attempts), so the verifier checks it
        // here as the precondition for accepting.
        let claimed_set: std::collections::BTreeSet<&str> = claim
            .completed_criteria
            .iter()
            .map(String::as_str)
            .collect();
        let mut missing_required: Vec<String> = Vec::new();
        for criterion in &spec.acceptance_criteria {
            if criterion.required && !claimed_set.contains(criterion.id.as_str()) {
                missing_required.push(criterion.id.clone());
            }
        }
        if !missing_required.is_empty() {
            return GoalVerifyOutcome::Reject {
                reason: format!("claim omits {} required criteria", missing_required.len()),
                missing: missing_required,
            };
        }

        // Rule 4: `verification` non-empty under Standard policy. The
        // doc allows DocumentationOnly to ship a report with no
        // structured verification records — the supervisor's
        // narrative `verification` text is the audit artefact.
        if matches!(spec.evidence_policy, GoalEvidencePolicy::Standard)
            && claim.verification.is_empty()
        {
            return GoalVerifyOutcome::Reject {
                missing: claim.completed_criteria.clone(),
                reason: "claim has no verification records under Standard policy".to_string(),
            };
        }

        // Rule 5: any non-success in recent tool results blocks
        // acceptance. The supervisor decides the depth; the verifier
        // surfaces the first offending call so the continuation
        // prompt can name it.
        let recent = ctx.recent_tool_results();
        if let Some(failed) = recent
            .iter()
            .find(|r| r.status != ToolResultStatus::Succeeded)
        {
            return GoalVerifyOutcome::Reject {
                missing: claim.completed_criteria.clone(),
                reason: format!(
                    "recent tool call `{}` ({}) returned {:?} — completion blocked until \
                     the failure is addressed or claim adds residual_risk explaining it",
                    failed.tool_name, failed.call_id, failed.status,
                ),
            };
        }

        // Rules 2, 3, 6: per-criterion evidence floors. Iterate
        // claimed criteria (required + optional) so an optional
        // workspace-change criterion still demands matching evidence
        // — symmetric to the schema floor's claimed-criterion
        // iteration (Codex pass-8 P2).
        let spec_by_id: std::collections::BTreeMap<&str, &GoalCriterion> = spec
            .acceptance_criteria
            .iter()
            .map(|c| (c.id.as_str(), c))
            .collect();
        let changed_files: std::collections::BTreeSet<String> =
            ctx.changed_files().into_iter().collect();
        for claimed_id in &claim.completed_criteria {
            let Some(criterion) = spec_by_id.get(claimed_id.as_str()) else {
                // Already caught by validate_completion_claim_shape —
                // we re-iterate here for the per-criterion checks
                // below; an unknown id surfaced earlier would have
                // returned Reject already.
                continue;
            };
            let matching_refs: Vec<&GoalEvidenceRef> = claim
                .evidence_refs
                .iter()
                .filter(|r| r.criterion_id.as_deref() == Some(criterion.id.as_str()))
                .filter(|r| !matches!(r.target, GoalEvidenceTarget::Future))
                .collect();
            // Standard policy demands ≥1 matching evidence ref
            // (Rule 2). DocumentationOnly relaxes this so the
            // supervisor can attest acceptance through narrative
            // `verification` records.
            if matching_refs.is_empty()
                && matches!(spec.evidence_policy, GoalEvidencePolicy::Standard)
            {
                return GoalVerifyOutcome::Reject {
                    missing: vec![criterion.id.clone()],
                    reason: format!(
                        "criterion `{}` has no matching evidence under Standard policy",
                        criterion.id,
                    ),
                };
            }
            if criterion.requires_workspace_change {
                let satisfied = matching_refs.iter().any(|r| match &r.target {
                    GoalEvidenceTarget::File { path, sha256 } => {
                        // Rule 3: File evidence must match the
                        // file's current on-disk hash. A stale
                        // hash means the verifier cannot deterministically
                        // attest the change.
                        ctx.file_sha256(path).as_deref() == Some(sha256.as_str())
                            // Rule 6: workspace must show the path
                            // among changed files (e.g. via
                            // `git status --short`). A workspace
                            // that has no diff cannot demonstrate
                            // the claimed change.
                            && changed_files.contains(path)
                    }
                    GoalEvidenceTarget::NoChangesNeeded { .. } => {
                        // Documented escape hatch: the criterion
                        // declares "no change is needed" with a
                        // rationale. The verifier accepts this in
                        // lieu of a File ref.
                        true
                    }
                    _ => false,
                });
                if !satisfied {
                    return GoalVerifyOutcome::Reject {
                        missing: vec![criterion.id.clone()],
                        reason: format!(
                            "criterion `{}` requires workspace evidence — needs a `File` \
                             evidence ref whose sha256 matches the file on disk and whose \
                             path appears in `changed_files`, or a `NoChangesNeeded` ref \
                             with a rationale",
                            criterion.id,
                        ),
                    };
                }
            }
        }

        // All rules passed — build the report. The verifier copies
        // claim-level fields verbatim (the schema floor + per-criterion
        // checks above already validated each one) and stamps the
        // supervisor-provided trio (timestamp, actor, budget) on top.
        let report = GoalCompletionReport {
            summary: claim.summary.clone(),
            completed_criteria: claim.completed_criteria.clone(),
            evidence_refs: claim.evidence_refs.clone(),
            verification: claim.verification.clone(),
            residual_risks: claim.residual_risks.clone(),
            changed_files: ctx.changed_files(),
            claim_envelope_id,
            total_spent_micro_usd: ctx.total_spent_micro_usd(),
            elapsed_wall_clock_seconds: ctx.elapsed_wall_clock_seconds(),
            continuation_loops_used: ctx.continuation_loops_used(),
            finalised_at: ctx.now(),
            finalised_by: ctx.finalised_by(),
        };
        // Final defense-in-depth: re-run the report shape gate before
        // handing off. A bug in the verifier that emitted a malformed
        // report would otherwise surface only when the supervisor
        // tried to apply the Completed envelope downstream.
        if let Err(source) = validate_completion_report_shape(spec, &report) {
            return reject_for_shape(source);
        }
        GoalVerifyOutcome::Accept(report)
    }
}

/// Fold a schema-shape error into a verifier-facing rejection. The
/// `missing` list is empty for shape errors that aren't id-coverage
/// failures; the supervisor's continuation prompt should still be
/// able to render the reason.
fn reject_for_shape(source: GoalCompletionShapeError) -> GoalVerifyOutcome {
    let missing = match &source {
        GoalCompletionShapeError::MissingRequiredCriterion { id } => vec![id.clone()],
        GoalCompletionShapeError::MissingEvidenceForRequiredCriterion { id } => {
            vec![id.clone()]
        }
        GoalCompletionShapeError::MissingWorkspaceEvidenceForCriterion { id } => {
            vec![id.clone()]
        }
        _ => Vec::new(),
    };
    GoalVerifyOutcome::Reject {
        missing,
        reason: source.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use chrono::TimeZone;

    use super::*;
    use crate::internal::ai::goal::{
        GoalBudget, GoalCompletionClaim, GoalCriterion, GoalEvidencePolicy, GoalEvidenceRef,
        GoalEvidenceTarget, GoalSpec, GoalVerificationRecord,
    };

    fn fixture_now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 5, 8, 13, 0, 0).unwrap()
    }

    fn fixture_spec(criteria: Vec<GoalCriterion>, policy: GoalEvidencePolicy) -> GoalSpec {
        GoalSpec::new(
            Uuid::parse_str("00000000-0000-0000-0000-0000000000a1").unwrap(),
            "thread-1",
            "session-1",
            "deliver feature X",
            criteria,
            Vec::new(),
            policy,
            GoalBudget::default(),
            fixture_now(),
            GoalActor::User { id: None },
        )
        .expect("happy-path spec must construct")
    }

    /// Sticky ctx for tests — fields are RefCells so individual
    /// tests can adjust file hashes / tool results without rebuilding
    /// the whole struct.
    struct FixtureCtx {
        file_hashes: RefCell<std::collections::BTreeMap<String, String>>,
        tool_results: RefCell<Vec<RecentToolCall>>,
        changed_files: RefCell<Vec<String>>,
    }

    impl FixtureCtx {
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

    impl GoalVerifierContext for FixtureCtx {
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
                reason: "deterministic verifier".to_string(),
            }
        }

        fn total_spent_micro_usd(&self) -> u64 {
            1_000_000
        }

        fn elapsed_wall_clock_seconds(&self) -> u64 {
            900
        }

        fn continuation_loops_used(&self) -> u32 {
            3
        }
    }

    fn fixture_claim_envelope_id() -> Uuid {
        Uuid::parse_str("00000000-0000-0000-0000-0000c1a10042").unwrap()
    }

    /// Happy path: required criterion claimed with matching File
    /// evidence, verification record, no failed tool calls — accept.
    #[test]
    fn accepts_well_formed_workspace_change_claim() {
        let spec = fixture_spec(
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
        let ctx = FixtureCtx::new().with_file("src/feature.rs", "deadbeef");
        let outcome =
            DeterministicGoalVerifier.verify(&ctx, &spec, &claim, fixture_claim_envelope_id());
        match outcome {
            GoalVerifyOutcome::Accept(report) => {
                assert_eq!(report.completed_criteria, vec!["patch".to_string()]);
                assert_eq!(report.claim_envelope_id, fixture_claim_envelope_id());
                assert_eq!(report.total_spent_micro_usd, 1_000_000);
                assert_eq!(report.elapsed_wall_clock_seconds, 900);
                assert_eq!(report.continuation_loops_used, 3);
                assert_eq!(report.changed_files, vec!["src/feature.rs".to_string()]);
            }
            GoalVerifyOutcome::Reject { missing, reason } => {
                panic!("expected Accept, got Reject({missing:?}, {reason})")
            }
        }
    }

    /// Rule 1: a required criterion missing from the claim → reject.
    #[test]
    fn rejects_claim_missing_required_criterion() {
        let spec = fixture_spec(
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
            evidence_refs: Vec::new(),
            verification: vec![GoalVerificationRecord {
                criterion_id: "compiles".to_string(),
                method: "x".to_string(),
                passed: true,
                output_summary: None,
            }],
            residual_risks: Vec::new(),
        };
        let ctx = FixtureCtx::new();
        let outcome =
            DeterministicGoalVerifier.verify(&ctx, &spec, &claim, fixture_claim_envelope_id());
        match outcome {
            GoalVerifyOutcome::Reject { missing, .. } => {
                assert_eq!(missing, vec!["tests".to_string()]);
            }
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    /// Rule 4: empty verification under Standard → reject.
    #[test]
    fn rejects_empty_verification_under_standard_policy() {
        let spec = fixture_spec(
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
        let ctx = FixtureCtx::new();
        let outcome =
            DeterministicGoalVerifier.verify(&ctx, &spec, &claim, fixture_claim_envelope_id());
        assert!(
            matches!(outcome, GoalVerifyOutcome::Reject { ref reason, .. }
                if reason.contains("verification records")),
            "expected verification-empty rejection, got {outcome:?}",
        );
    }

    /// DocumentationOnly relaxes Rule 4 — empty verification is OK
    /// when the policy permits narrative-only acceptance.
    #[test]
    fn documentation_only_policy_accepts_empty_verification() {
        let spec = fixture_spec(
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
            summary: "research note attached".to_string(),
            completed_criteria: vec!["decision".to_string()],
            evidence_refs: Vec::new(),
            verification: Vec::new(),
            residual_risks: Vec::new(),
        };
        let ctx = FixtureCtx::new();
        let outcome =
            DeterministicGoalVerifier.verify(&ctx, &spec, &claim, fixture_claim_envelope_id());
        assert!(matches!(outcome, GoalVerifyOutcome::Accept(_)));
    }

    /// Rule 5: a recent failed tool call → reject. The reason names
    /// the call so the continuation prompt can surface it.
    #[test]
    fn rejects_when_recent_tool_call_failed() {
        let spec = fixture_spec(
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
            summary: "x".to_string(),
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
                method: "x".to_string(),
                passed: true,
                output_summary: None,
            }],
            residual_risks: Vec::new(),
        };
        let ctx = FixtureCtx::new().with_tool_result("tc-99", ToolResultStatus::Failed);
        let outcome =
            DeterministicGoalVerifier.verify(&ctx, &spec, &claim, fixture_claim_envelope_id());
        match outcome {
            GoalVerifyOutcome::Reject { reason, .. } => {
                assert!(
                    reason.contains("tc-99"),
                    "reason should name failing call: {reason}"
                );
            }
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    /// Rule 3: workspace-change criterion with stale File sha256 →
    /// reject. The verifier's hash check refuses an evidence ref
    /// whose recorded sha256 no longer matches disk.
    #[test]
    fn rejects_workspace_change_criterion_with_stale_file_hash() {
        let spec = fixture_spec(
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
            summary: "x".to_string(),
            completed_criteria: vec!["patch".to_string()],
            evidence_refs: vec![GoalEvidenceRef {
                criterion_id: Some("patch".to_string()),
                target: GoalEvidenceTarget::File {
                    path: "src/feature.rs".to_string(),
                    sha256: "stale".to_string(),
                },
                description: "x".to_string(),
            }],
            verification: vec![GoalVerificationRecord {
                criterion_id: "patch".to_string(),
                method: "x".to_string(),
                passed: true,
                output_summary: None,
            }],
            residual_risks: Vec::new(),
        };
        // Disk hash differs from claim's hash.
        let ctx = FixtureCtx::new().with_file("src/feature.rs", "fresh");
        let outcome =
            DeterministicGoalVerifier.verify(&ctx, &spec, &claim, fixture_claim_envelope_id());
        match outcome {
            GoalVerifyOutcome::Reject { missing, .. } => {
                assert_eq!(missing, vec!["patch".to_string()]);
            }
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    /// Rule 6: workspace-change criterion with `NoChangesNeeded`
    /// rationale is accepted even without a File ref.
    #[test]
    fn accepts_workspace_change_criterion_with_no_changes_needed_evidence() {
        let spec = fixture_spec(
            vec![GoalCriterion {
                id: "investigation".to_string(),
                description: "research-only criterion that may not require code".to_string(),
                required: true,
                verifier_hint: None,
                requires_workspace_change: true,
            }],
            GoalEvidencePolicy::Standard,
        );
        let claim = GoalCompletionClaim {
            summary: "no change needed".to_string(),
            completed_criteria: vec!["investigation".to_string()],
            evidence_refs: vec![GoalEvidenceRef {
                criterion_id: Some("investigation".to_string()),
                target: GoalEvidenceTarget::NoChangesNeeded {
                    rationale: "spec already correct".to_string(),
                },
                description: "no change needed".to_string(),
            }],
            verification: vec![GoalVerificationRecord {
                criterion_id: "investigation".to_string(),
                method: "manual review".to_string(),
                passed: true,
                output_summary: None,
            }],
            residual_risks: Vec::new(),
        };
        let ctx = FixtureCtx::new();
        let outcome =
            DeterministicGoalVerifier.verify(&ctx, &spec, &claim, fixture_claim_envelope_id());
        assert!(matches!(outcome, GoalVerifyOutcome::Accept(_)));
    }

    /// Schema-floor defense-in-depth: a malformed claim
    /// (duplicate criterion id) reaches the verifier — the trait
    /// implementation runs `validate_completion_claim_shape` first
    /// and rejects with the same shape error.
    #[test]
    fn rejects_via_schema_floor_when_claim_is_malformed() {
        let spec = fixture_spec(
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
            summary: "x".to_string(),
            // Duplicate id triggers DuplicateClaimedCriterion in
            // the schema floor before any verifier rule runs.
            completed_criteria: vec!["compiles".to_string(), "compiles".to_string()],
            evidence_refs: Vec::new(),
            verification: Vec::new(),
            residual_risks: Vec::new(),
        };
        let ctx = FixtureCtx::new();
        let outcome =
            DeterministicGoalVerifier.verify(&ctx, &spec, &claim, fixture_claim_envelope_id());
        assert!(matches!(outcome, GoalVerifyOutcome::Reject { .. }));
    }
}
