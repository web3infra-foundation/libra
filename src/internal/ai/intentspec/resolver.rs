use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
};

use chrono::Utc;
use uuid::Uuid;

use super::{
    draft::{DraftCheck, IntentDraft},
    profiles,
    types::{
        Acceptance, ArtifactName, ArtifactReq, ArtifactStage, Artifacts, ChangeType, Check,
        Constraints, CreatedBy, CreatorType, Intent, IntentSpec, Lifecycle, LifecycleStatus,
        Metadata, QualityGates, RepoTarget, RepoType, RiskLevel, Target, VerificationPlan,
    },
};

#[derive(Clone, Debug)]
pub struct ResolveContext {
    pub working_dir: String,
    pub base_ref: String,
    pub created_by_id: String,
}

/// Resolves an IntentDraft into a full IntentSpec by applying risk-based defaults and metadata.
///
/// This is the final stage of "Plan" generation before the IntentSpec is presented to the user
/// and persisted via MCP (see [`crate::internal::ai::intentspec::persist_intentspec`]).
pub fn resolve_intentspec(
    draft: IntentDraft,
    risk_level: RiskLevel,
    ctx: ResolveContext,
) -> IntentSpec {
    let constraints = profiles::default_constraints(risk_level.clone());
    let mut artifacts = profiles::default_artifacts(risk_level.clone());

    let verification_plan = VerificationPlan {
        fast_checks: draft
            .acceptance
            .fast_checks
            .into_iter()
            .map(convert_draft_check)
            .collect(),
        integration_checks: draft
            .acceptance
            .integration_checks
            .into_iter()
            .map(convert_draft_check)
            .collect(),
        security_checks: draft
            .acceptance
            .security_checks
            .into_iter()
            .map(convert_draft_check)
            .collect(),
        release_checks: draft
            .acceptance
            .release_checks
            .into_iter()
            .map(convert_draft_check)
            .collect(),
    };

    merge_artifacts_from_checks(&verification_plan, &mut artifacts);
    harmonize_retention(&constraints, &mut artifacts);

    let quality_gates = QualityGates {
        require_new_tests_when_bugfix: if matches!(draft.intent.change_type, ChangeType::Bugfix) {
            Some(true)
        } else {
            None
        },
        max_allowed_regression: None,
    };

    IntentSpec {
        api_version: "intentspec.io/v1alpha1".to_string(),
        kind: "IntentSpec".to_string(),
        metadata: Metadata {
            id: Uuid::now_v7().to_string(),
            created_at: Utc::now().to_rfc3339(),
            created_by: CreatedBy {
                creator_type: CreatorType::User,
                id: ctx.created_by_id,
                display_name: None,
            },
            target: Target {
                repo: RepoTarget {
                    repo_type: RepoType::Local,
                    locator: normalize_working_dir(&ctx.working_dir),
                },
                base_ref: if ctx.base_ref.trim().is_empty() {
                    "HEAD".to_string()
                } else {
                    ctx.base_ref
                },
                workspace_id: None,
                labels: BTreeMap::new(),
            },
        },
        intent: Intent {
            summary: draft.intent.summary,
            problem_statement: draft.intent.problem_statement,
            change_type: draft.intent.change_type,
            objectives: draft.intent.objectives,
            in_scope: draft.intent.in_scope,
            out_of_scope: draft.intent.out_of_scope,
            touch_hints: draft.intent.touch_hints,
        },
        acceptance: Acceptance {
            success_criteria: draft.acceptance.success_criteria,
            verification_plan,
            quality_gates: Some(quality_gates),
        },
        constraints,
        risk: profiles::default_risk(risk_level.clone(), draft.risk.rationale, draft.risk.factors),
        evidence: profiles::default_evidence(risk_level.clone()),
        security: profiles::default_security(),
        execution: profiles::default_execution(risk_level.clone()),
        artifacts,
        provenance: profiles::default_provenance(risk_level),
        lifecycle: Lifecycle {
            schema_version: "1.0.0".to_string(),
            status: LifecycleStatus::Active,
            change_log: Vec::new(),
        },
        libra: None,
        extensions: BTreeMap::new(),
    }
}

fn convert_draft_check(c: DraftCheck) -> Check {
    Check {
        id: c.id,
        kind: c.kind,
        command: c.command,
        timeout_seconds: c.timeout_seconds,
        expected_exit_code: c.expected_exit_code,
        required: c.required,
        artifacts_produced: c.artifacts_produced,
    }
}

fn normalize_working_dir(raw: &str) -> String {
    let p = Path::new(raw);
    p.canonicalize()
        .map(|v| v.display().to_string())
        .unwrap_or_else(|_| raw.to_string())
}

fn merge_artifacts_from_checks(plan: &VerificationPlan, artifacts: &mut Artifacts) {
    let mut existing: HashSet<ArtifactName> =
        artifacts.required.iter().map(|a| a.name.clone()).collect();

    for (stage, checks) in [
        (ArtifactStage::PerTask, &plan.fast_checks),
        (ArtifactStage::Integration, &plan.integration_checks),
        (ArtifactStage::Security, &plan.security_checks),
        (ArtifactStage::Release, &plan.release_checks),
    ] {
        for check in checks {
            for name in &check.artifacts_produced {
                if let Some(parsed_name) = parse_artifact_name(name)
                    && !existing.contains(&parsed_name)
                {
                    artifacts.required.push(ArtifactReq {
                        name: parsed_name.clone(),
                        stage: stage.clone(),
                        required: true,
                        format: String::new(),
                    });
                    existing.insert(parsed_name);
                }
            }
        }
    }
}

fn parse_artifact_name(name: &str) -> Option<ArtifactName> {
    match name {
        "patchset" => Some(ArtifactName::Patchset),
        "test-log" => Some(ArtifactName::TestLog),
        "build-log" => Some(ArtifactName::BuildLog),
        "sast-report" => Some(ArtifactName::SastReport),
        "sca-report" => Some(ArtifactName::ScaReport),
        "sbom" => Some(ArtifactName::Sbom),
        "provenance-attestation" => Some(ArtifactName::ProvenanceAttestation),
        "transparency-proof" => Some(ArtifactName::TransparencyProof),
        "release-notes" => Some(ArtifactName::ReleaseNotes),
        _ => None,
    }
}

fn harmonize_retention(constraints: &Constraints, artifacts: &mut Artifacts) {
    let effective = constraints
        .privacy
        .retention_days
        .min(artifacts.retention.days);
    artifacts.retention.days = effective;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::intentspec::draft::{DraftAcceptance, DraftIntent, DraftRisk};

    #[test]
    fn test_resolve_intentspec_low_profile() {
        let draft = IntentDraft {
            intent: DraftIntent {
                summary: "Fix bug".to_string(),
                problem_statement: "Bug details".to_string(),
                change_type: ChangeType::Bugfix,
                objectives: vec!["fix".to_string()],
                in_scope: vec!["src".to_string()],
                out_of_scope: vec![],
                touch_hints: None,
            },
            acceptance: DraftAcceptance {
                success_criteria: vec!["tests pass".to_string()],
                fast_checks: vec![],
                integration_checks: vec![],
                security_checks: vec![],
                release_checks: vec![],
            },
            risk: DraftRisk {
                rationale: "safe".to_string(),
                factors: vec![],
                level: None,
            },
        };

        let spec = resolve_intentspec(
            draft,
            RiskLevel::Low,
            ResolveContext {
                working_dir: ".".to_string(),
                base_ref: "HEAD".to_string(),
                created_by_id: "tester".to_string(),
            },
        );

        assert_eq!(spec.risk.level, RiskLevel::Low);
        assert_eq!(spec.kind, "IntentSpec");
        assert!(
            spec.acceptance
                .quality_gates
                .as_ref()
                .and_then(|q| q.require_new_tests_when_bugfix)
                .unwrap_or(false)
        );
    }
}
