use std::collections::HashSet;

use super::{
    types::{
        ArtifactName, ArtifactReq, ArtifactStage, ChangeType, LifecycleStatus, RiskLevel, ToolRule,
    },
    validator::ValidationIssue,
};
use crate::internal::ai::intentspec::types::IntentSpec;

pub fn repair_intentspec(spec: &mut IntentSpec, _issues: &[ValidationIssue]) {
    if spec.risk.level == RiskLevel::High {
        spec.risk.human_in_loop.required = true;
        if spec.risk.human_in_loop.min_approvers < 2 {
            spec.risk.human_in_loop.min_approvers = 2;
        }
    }

    if spec.intent.change_type == ChangeType::Bugfix {
        let quality = spec.acceptance.quality_gates.get_or_insert_with(|| {
            crate::internal::ai::intentspec::types::QualityGates {
                require_new_tests_when_bugfix: None,
                max_allowed_regression: None,
            }
        });
        quality.require_new_tests_when_bugfix = Some(true);
    }

    if spec.security.tool_acl.allow.is_empty() {
        spec.security.tool_acl.allow.push(ToolRule {
            tool: "workspace.fs".to_string(),
            actions: vec!["read".to_string(), "write".to_string()],
            constraints: Default::default(),
        });
    }

    spec.lifecycle.status = LifecycleStatus::Active;
    spec.lifecycle.change_log.clear();

    let effective = spec
        .constraints
        .privacy
        .retention_days
        .min(spec.artifacts.retention.days);
    spec.artifacts.retention.days = effective;

    ensure_artifacts_from_checks(spec);
}

fn ensure_artifacts_from_checks(spec: &mut IntentSpec) {
    let mut existing: HashSet<ArtifactName> = spec
        .artifacts
        .required
        .iter()
        .map(|a| a.name.clone())
        .collect();

    for (stage, checks) in [
        (
            ArtifactStage::PerTask,
            &spec.acceptance.verification_plan.fast_checks,
        ),
        (
            ArtifactStage::Integration,
            &spec.acceptance.verification_plan.integration_checks,
        ),
        (
            ArtifactStage::Security,
            &spec.acceptance.verification_plan.security_checks,
        ),
        (
            ArtifactStage::Release,
            &spec.acceptance.verification_plan.release_checks,
        ),
    ] {
        for check in checks {
            for produced in &check.artifacts_produced {
                if let Some(name) = parse_artifact_name(produced)
                    && !existing.contains(&name)
                {
                    spec.artifacts.required.push(ArtifactReq {
                        name: name.clone(),
                        stage: stage.clone(),
                        required: true,
                        format: String::new(),
                    });
                    existing.insert(name);
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
