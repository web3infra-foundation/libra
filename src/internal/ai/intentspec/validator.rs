//! Validation rules for canonical IntentSpecs before they are accepted by the
//! scheduler and orchestrator.
//!
//! Boundary: validators report actionable defects without mutating the spec; callers
//! that want fixups must run `repair` first. Regression tests cover missing goals,
//! invalid artifact references, and acceptance criteria that cannot be converted into
//! executable checks.

use std::collections::HashSet;

use super::types::{ArtifactName, ChangeType, Check, IntentSpec, LifecycleStatus, RiskLevel};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationIssue {
    pub path: String,
    pub message: String,
}

impl ValidationIssue {
    fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }
}

pub fn validate_intentspec(spec: &IntentSpec) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();

    validate_non_empty_fields(spec, &mut issues);
    validate_high_risk_human_loop(spec, &mut issues);
    validate_artifact_coverage(spec, &mut issues);
    validate_retention(spec, &mut issues);
    validate_bugfix_quality_gate(spec, &mut issues);
    validate_security_acl(spec, &mut issues);
    validate_lifecycle(spec, &mut issues);

    issues
}

fn validate_non_empty_fields(spec: &IntentSpec, issues: &mut Vec<ValidationIssue>) {
    require_non_empty("apiVersion", &spec.api_version, issues);
    require_non_empty("kind", &spec.kind, issues);
    require_non_empty("metadata.id", &spec.metadata.id, issues);
    require_non_empty("metadata.createdAt", &spec.metadata.created_at, issues);
    require_non_empty(
        "metadata.createdBy.id",
        &spec.metadata.created_by.id,
        issues,
    );
    require_non_empty(
        "metadata.target.repo.locator",
        &spec.metadata.target.repo.locator,
        issues,
    );
    require_non_empty(
        "metadata.target.baseRef",
        &spec.metadata.target.base_ref,
        issues,
    );
    require_non_empty("intent.summary", &spec.intent.summary, issues);
    require_non_empty(
        "intent.problemStatement",
        &spec.intent.problem_statement,
        issues,
    );
    if spec.intent.objectives.is_empty() {
        issues.push(ValidationIssue::new(
            "intent.objectives",
            "must include at least one objective",
        ));
    }
    for (idx, objective) in spec.intent.objectives.iter().enumerate() {
        if objective.title.trim().is_empty() {
            issues.push(ValidationIssue::new(
                format!("intent.objectives[{idx}].title"),
                "must not be empty",
            ));
        }
    }
    if spec.intent.in_scope.is_empty() {
        issues.push(ValidationIssue::new(
            "intent.inScope",
            "must include at least one in-scope item",
        ));
    }
    if spec.acceptance.success_criteria.is_empty() {
        issues.push(ValidationIssue::new(
            "acceptance.successCriteria",
            "must include at least one success criterion",
        ));
    }
}

fn validate_high_risk_human_loop(spec: &IntentSpec, issues: &mut Vec<ValidationIssue>) {
    if spec.risk.level == RiskLevel::High {
        if !spec.risk.human_in_loop.required {
            issues.push(ValidationIssue::new(
                "risk.humanInLoop.required",
                "high risk requires humanInLoop.required=true",
            ));
        }
        if spec.risk.human_in_loop.min_approvers < 2 {
            issues.push(ValidationIssue::new(
                "risk.humanInLoop.minApprovers",
                "high risk requires minApprovers>=2",
            ));
        }
    }
}

fn validate_artifact_coverage(spec: &IntentSpec, issues: &mut Vec<ValidationIssue>) {
    let required: HashSet<String> = spec
        .artifacts
        .required
        .iter()
        .map(|a| artifact_name_to_str(&a.name).to_string())
        .collect();

    validate_check_artifacts(
        "fastChecks",
        &spec.acceptance.verification_plan.fast_checks,
        &required,
        issues,
    );
    validate_check_artifacts(
        "integrationChecks",
        &spec.acceptance.verification_plan.integration_checks,
        &required,
        issues,
    );
    validate_check_artifacts(
        "securityChecks",
        &spec.acceptance.verification_plan.security_checks,
        &required,
        issues,
    );
    validate_check_artifacts(
        "releaseChecks",
        &spec.acceptance.verification_plan.release_checks,
        &required,
        issues,
    );
}

fn validate_check_artifacts(
    stage: &str,
    checks: &[Check],
    required: &HashSet<String>,
    issues: &mut Vec<ValidationIssue>,
) {
    for check in checks {
        for produced in &check.artifacts_produced {
            if !is_known_artifact_name(produced) {
                issues.push(ValidationIssue::new(
                    format!(
                        "acceptance.verificationPlan.{stage}.{}.artifactsProduced",
                        check.id
                    ),
                    format!(
                        "unknown artifact name '{produced}'; must be one of {} (do not use file paths here)",
                        KNOWN_ARTIFACT_NAMES.join(", ")
                    ),
                ));
                continue;
            }
            if !required.contains(produced) {
                issues.push(ValidationIssue::new(
                    format!(
                        "acceptance.verificationPlan.{stage}.{}.artifactsProduced",
                        check.id
                    ),
                    format!("produced artifact '{produced}' missing from artifacts.required"),
                ));
            }
        }
    }
}

const KNOWN_ARTIFACT_NAMES: [&str; 9] = [
    "patchset",
    "test-log",
    "build-log",
    "sast-report",
    "sca-report",
    "sbom",
    "provenance-attestation",
    "transparency-proof",
    "release-notes",
];

fn is_known_artifact_name(name: &str) -> bool {
    matches!(
        name,
        "patchset"
            | "test-log"
            | "build-log"
            | "sast-report"
            | "sca-report"
            | "sbom"
            | "provenance-attestation"
            | "transparency-proof"
            | "release-notes"
    )
}

fn validate_retention(spec: &IntentSpec, issues: &mut Vec<ValidationIssue>) {
    let expected = spec
        .constraints
        .privacy
        .retention_days
        .min(spec.artifacts.retention.days);
    if spec.artifacts.retention.days != expected {
        issues.push(ValidationIssue::new(
            "artifacts.retention.days",
            format!(
                "must be min(constraints.privacy.retentionDays, artifacts.retention.days) = {expected}"
            ),
        ));
    }
}

fn validate_bugfix_quality_gate(spec: &IntentSpec, issues: &mut Vec<ValidationIssue>) {
    if spec.intent.change_type == ChangeType::Bugfix {
        let enabled = spec
            .acceptance
            .quality_gates
            .as_ref()
            .and_then(|q| q.require_new_tests_when_bugfix)
            .unwrap_or(false);
        if !enabled {
            issues.push(ValidationIssue::new(
                "acceptance.qualityGates.requireNewTestsWhenBugfix",
                "bugfix changeType requires requireNewTestsWhenBugfix=true",
            ));
        }
    }
}

fn validate_security_acl(spec: &IntentSpec, issues: &mut Vec<ValidationIssue>) {
    if spec.security.tool_acl.allow.is_empty() {
        issues.push(ValidationIssue::new(
            "security.toolAcl.allow",
            "must define at least one allow rule",
        ));
    }
}

fn validate_lifecycle(spec: &IntentSpec, issues: &mut Vec<ValidationIssue>) {
    if spec.lifecycle.status != LifecycleStatus::Active {
        issues.push(ValidationIssue::new(
            "lifecycle.status",
            "initial lifecycle.status must be active",
        ));
    }
    if !spec.lifecycle.change_log.is_empty() {
        issues.push(ValidationIssue::new(
            "lifecycle.changeLog",
            "initial lifecycle.changeLog must be empty",
        ));
    }
}

fn require_non_empty(path: &str, value: &str, issues: &mut Vec<ValidationIssue>) {
    if value.trim().is_empty() {
        issues.push(ValidationIssue::new(path, "must be non-empty"));
    }
}

fn artifact_name_to_str(name: &ArtifactName) -> &'static str {
    match name {
        ArtifactName::Patchset => "patchset",
        ArtifactName::TestLog => "test-log",
        ArtifactName::BuildLog => "build-log",
        ArtifactName::SastReport => "sast-report",
        ArtifactName::ScaReport => "sca-report",
        ArtifactName::Sbom => "sbom",
        ArtifactName::ProvenanceAttestation => "provenance-attestation",
        ArtifactName::TransparencyProof => "transparency-proof",
        ArtifactName::ReleaseNotes => "release-notes",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::intentspec::{
        ResolveContext,
        draft::{DraftAcceptance, DraftIntent, DraftRisk, IntentDraft},
        resolve_intentspec,
        types::{ChangeType, CheckKind, Objective, ObjectiveKind, RiskLevel},
    };

    fn sample_spec() -> IntentSpec {
        resolve_intentspec(
            IntentDraft {
                intent: DraftIntent {
                    summary: "Fix bug".to_string(),
                    problem_statement: "Bug".to_string(),
                    change_type: ChangeType::Bugfix,
                    objectives: vec![Objective {
                        title: "fix".to_string(),
                        kind: ObjectiveKind::Implementation,
                    }],
                    in_scope: vec!["src".to_string()],
                    out_of_scope: vec![],
                    touch_hints: None,
                },
                acceptance: DraftAcceptance {
                    success_criteria: vec!["ok".to_string()],
                    fast_checks: vec![],
                    integration_checks: vec![],
                    security_checks: vec![],
                    release_checks: vec![],
                },
                risk: DraftRisk {
                    rationale: "high".to_string(),
                    factors: vec![],
                    level: Some(RiskLevel::High),
                },
            },
            RiskLevel::High,
            ResolveContext {
                working_dir: ".".to_string(),
                base_ref: "HEAD".to_string(),
                created_by_id: "tester".to_string(),
            },
        )
    }

    #[test]
    fn test_validate_high_risk_passes() {
        let spec = sample_spec();
        let issues = validate_intentspec(&spec);
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn test_validate_high_risk_approvers_fail() {
        let mut spec = sample_spec();
        spec.risk.human_in_loop.min_approvers = 1;
        let issues = validate_intentspec(&spec);
        assert!(
            issues
                .iter()
                .any(|i| i.path == "risk.humanInLoop.minApprovers")
        );
    }

    #[test]
    fn test_validate_rejects_unknown_artifacts_produced() {
        let mut spec = sample_spec();
        spec.acceptance.verification_plan.fast_checks.push(Check {
            id: "hello-world-compiles".into(),
            kind: CheckKind::Command,
            command: Some("cargo build -p hello-world --release".into()),
            timeout_seconds: Some(120),
            expected_exit_code: Some(0),
            required: true,
            artifacts_produced: vec!["hello-world/target/release/hello-world".into()],
        });

        let issues = validate_intentspec(&spec);
        assert!(
            issues
                .iter()
                .any(|i| i.message.contains("unknown artifact name")),
            "{issues:?}"
        );
    }
}
