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
    let has_implementation_work = draft.intent.has_implementation_objectives();
    let mut artifacts = profiles::default_artifacts(risk_level.clone(), has_implementation_work);

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
        provenance: profiles::default_provenance(risk_level, has_implementation_work),
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
    let mut existing: HashSet<(ArtifactName, ArtifactStage)> = artifacts
        .required
        .iter()
        .map(|a| (a.name.clone(), a.stage.clone()))
        .collect();

    for (stage, checks) in [
        (ArtifactStage::PerTask, &plan.fast_checks),
        (ArtifactStage::Integration, &plan.integration_checks),
        (ArtifactStage::Security, &plan.security_checks),
        (ArtifactStage::Release, &plan.release_checks),
    ] {
        for check in checks {
            for name in &check.artifacts_produced {
                if let Some(parsed_name) = parse_artifact_name(name) {
                    let key = (parsed_name.clone(), stage.clone());
                    if !existing.contains(&key) {
                        artifacts.required.push(ArtifactReq {
                            name: parsed_name.clone(),
                            stage: stage.clone(),
                            required: true,
                            format: String::new(),
                        });
                        existing.insert(key);
                    }
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
    use crate::internal::ai::intentspec::{
        draft::{DraftAcceptance, DraftIntent, DraftRisk},
        types::{CheckKind, Objective, ObjectiveKind},
    };

    #[test]
    fn test_resolve_intentspec_low_profile() {
        let draft = IntentDraft {
            intent: DraftIntent {
                summary: "Fix bug".to_string(),
                problem_statement: "Bug details".to_string(),
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

    #[test]
    fn test_merge_artifacts_preserves_stage_for_same_name() {
        let plan = VerificationPlan {
            fast_checks: vec![],
            integration_checks: vec![Check {
                id: "integration".into(),
                kind: CheckKind::Command,
                command: Some("cargo test".into()),
                timeout_seconds: None,
                expected_exit_code: None,
                required: true,
                artifacts_produced: vec!["test-log".into()],
            }],
            security_checks: vec![],
            release_checks: vec![Check {
                id: "release".into(),
                kind: CheckKind::Command,
                command: Some("cargo test --release".into()),
                timeout_seconds: None,
                expected_exit_code: None,
                required: true,
                artifacts_produced: vec!["test-log".into()],
            }],
        };
        let mut artifacts = Artifacts {
            required: vec![],
            retention: crate::internal::ai::intentspec::types::ArtifactRetention::default(),
        };

        merge_artifacts_from_checks(&plan, &mut artifacts);

        assert!(artifacts.required.iter().any(|req| {
            req.name == ArtifactName::TestLog && req.stage == ArtifactStage::Integration
        }));
        assert!(artifacts.required.iter().any(|req| {
            req.name == ArtifactName::TestLog && req.stage == ArtifactStage::Release
        }));
    }

    #[test]
    fn test_resolve_analysis_only_does_not_require_patchset_by_default() {
        let draft = IntentDraft {
            intent: DraftIntent {
                summary: "Diagnose repository".to_string(),
                problem_statement: "Summarize technical debt hotspots".to_string(),
                change_type: ChangeType::Unknown,
                objectives: vec![Objective {
                    title: "Inventory key issues".to_string(),
                    kind: ObjectiveKind::Analysis,
                }],
                in_scope: vec!["src".to_string()],
                out_of_scope: vec![],
                touch_hints: None,
            },
            acceptance: DraftAcceptance {
                success_criteria: vec!["Findings are summarized".to_string()],
                fast_checks: vec![],
                integration_checks: vec![],
                security_checks: vec![],
                release_checks: vec![],
            },
            risk: DraftRisk {
                rationale: "read-only analysis".to_string(),
                factors: vec![],
                level: Some(RiskLevel::Low),
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

        assert!(
            !spec
                .artifacts
                .required
                .iter()
                .any(|req| req.name == ArtifactName::Patchset),
            "{:?}",
            spec.artifacts.required
        );
    }

    #[test]
    fn test_resolve_analysis_only_medium_risk_has_no_default_security_or_release_artifacts() {
        let draft = IntentDraft {
            intent: DraftIntent {
                summary: "Diagnose repository".to_string(),
                problem_statement: "Summarize technical debt hotspots".to_string(),
                change_type: ChangeType::Unknown,
                objectives: vec![Objective {
                    title: "Inventory key issues".to_string(),
                    kind: ObjectiveKind::Analysis,
                }],
                in_scope: vec!["src".to_string()],
                out_of_scope: vec![],
                touch_hints: None,
            },
            acceptance: DraftAcceptance {
                success_criteria: vec!["Findings are summarized".to_string()],
                fast_checks: vec![],
                integration_checks: vec![],
                security_checks: vec![],
                release_checks: vec![],
            },
            risk: DraftRisk {
                rationale: "read-only analysis".to_string(),
                factors: vec![],
                level: Some(RiskLevel::Medium),
            },
        };

        let spec = resolve_intentspec(
            draft,
            RiskLevel::Medium,
            ResolveContext {
                working_dir: ".".to_string(),
                base_ref: "HEAD".to_string(),
                created_by_id: "tester".to_string(),
            },
        );

        assert!(
            spec.artifacts.required.is_empty(),
            "{:?}",
            spec.artifacts.required
        );
        assert!(!spec.provenance.require_slsa_provenance);
        assert!(!spec.provenance.require_sbom);
    }

    #[test]
    fn test_resolve_implementation_only_does_not_require_test_log_without_checks() {
        let draft = IntentDraft {
            intent: DraftIntent {
                summary: "Implement feature".to_string(),
                problem_statement: "Add the requested behavior".to_string(),
                change_type: ChangeType::Feature,
                objectives: vec![Objective {
                    title: "Ship feature".to_string(),
                    kind: ObjectiveKind::Implementation,
                }],
                in_scope: vec!["src".to_string()],
                out_of_scope: vec![],
                touch_hints: None,
            },
            acceptance: DraftAcceptance {
                success_criteria: vec!["feature works".to_string()],
                fast_checks: vec![],
                integration_checks: vec![],
                security_checks: vec![],
                release_checks: vec![],
            },
            risk: DraftRisk {
                rationale: "normal feature".to_string(),
                factors: vec![],
                level: Some(RiskLevel::Low),
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

        assert!(
            spec.artifacts
                .required
                .iter()
                .any(|req| req.name == ArtifactName::Patchset),
            "{:?}",
            spec.artifacts.required
        );
        assert!(
            !spec
                .artifacts
                .required
                .iter()
                .any(|req| req.name == ArtifactName::TestLog
                    && req.stage == ArtifactStage::PerTask),
            "{:?}",
            spec.artifacts.required
        );
    }
}
