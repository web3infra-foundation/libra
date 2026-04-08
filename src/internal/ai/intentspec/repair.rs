use std::collections::HashSet;

use super::{
    types::{
        ArtifactName, ArtifactReq, ArtifactStage, ChangeType, LifecycleStatus, QualityGates,
        RiskLevel, ToolRule,
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
        let quality = spec.acceptance.quality_gates.get_or_insert(QualityGates {
            require_new_tests_when_bugfix: None,
            max_allowed_regression: None,
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

    if spec.intent.in_scope.is_empty() {
        if let Some(touch_hints) = spec.intent.touch_hints.as_ref()
            && !touch_hints.files.is_empty()
        {
            spec.intent.in_scope = touch_hints.files.clone();
        } else {
            spec.intent.in_scope.push(".".to_string());
        }
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

    let mut to_add: Vec<(ArtifactStage, ArtifactName)> = Vec::new();

    {
        let plan = &mut spec.acceptance.verification_plan;
        for (stage, checks) in [
            (ArtifactStage::PerTask, &mut plan.fast_checks),
            (ArtifactStage::Integration, &mut plan.integration_checks),
            (ArtifactStage::Security, &mut plan.security_checks),
            (ArtifactStage::Release, &mut plan.release_checks),
        ] {
            for check in checks.iter_mut() {
                normalize_artifacts_produced(&mut check.artifacts_produced);
                for produced in &check.artifacts_produced {
                    if let Some(name) = parse_artifact_name(produced)
                        && !existing.contains(&name)
                    {
                        existing.insert(name.clone());
                        to_add.push((stage.clone(), name));
                    }
                }
            }
        }
    }

    for (stage, name) in to_add {
        spec.artifacts.required.push(ArtifactReq {
            name,
            stage,
            required: true,
            format: String::new(),
        });
    }
}

fn normalize_artifacts_produced(produced: &mut Vec<String>) {
    // `artifactsProduced` is an enum of well-known artifact names. In practice, agents sometimes
    // mistakenly put file paths here (e.g. `.../target/release/foo`). Normalize those to a
    // supported artifact type so validation+repair can converge.
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for raw in produced.drain(..) {
        let item = raw.trim();
        if item.is_empty() {
            continue;
        }

        let normalized = if parse_artifact_name(item).is_some() {
            item.to_string()
        } else if looks_like_cargo_build_output(item) {
            "build-log".to_string()
        } else {
            // Drop unknown entries; otherwise they will keep failing validation.
            continue;
        };

        if seen.insert(normalized.clone()) {
            out.push(normalized);
        }
    }

    *produced = out;
}

fn looks_like_cargo_build_output(s: &str) -> bool {
    // Heuristic: cargo outputs are under `target/{debug,release}/...`.
    let has_target = s.contains("/target/") || s.contains("\\target\\");
    let has_profile = s.contains("/release/")
        || s.contains("/debug/")
        || s.contains("\\release\\")
        || s.contains("\\debug\\");
    has_target && has_profile
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::intentspec::{
        ResolveContext,
        draft::{DraftAcceptance, DraftCheck, DraftIntent, DraftRisk, IntentDraft},
        resolve_intentspec,
        types::{ChangeType, CheckKind, Objective, ObjectiveKind, RiskLevel},
        validate_intentspec,
    };

    #[test]
    fn test_repair_normalizes_path_like_artifacts_produced_to_build_log() {
        let draft = IntentDraft {
            intent: DraftIntent {
                summary: "Compile hello world".to_string(),
                problem_statement: "Ensure hello-world compiles".to_string(),
                change_type: ChangeType::Feature,
                objectives: vec![Objective {
                    title: "hello-world builds".to_string(),
                    kind: ObjectiveKind::Implementation,
                }],
                in_scope: vec!["hello-world/".to_string()],
                out_of_scope: vec![],
                touch_hints: None,
            },
            acceptance: DraftAcceptance {
                success_criteria: vec!["cargo build succeeds".to_string()],
                fast_checks: vec![DraftCheck {
                    id: "hello-world-compiles".to_string(),
                    kind: CheckKind::Command,
                    command: Some("cargo build -p hello-world --release".to_string()),
                    timeout_seconds: Some(120),
                    expected_exit_code: Some(0),
                    required: true,
                    artifacts_produced: vec!["hello-world/target/release/hello-world".to_string()],
                }],
                integration_checks: vec![],
                security_checks: vec![],
                release_checks: vec![],
            },
            risk: DraftRisk {
                rationale: "low risk".to_string(),
                factors: vec![],
                level: Some(RiskLevel::Low),
            },
        };

        let mut spec = resolve_intentspec(
            draft,
            RiskLevel::Low,
            ResolveContext {
                working_dir: ".".to_string(),
                base_ref: "HEAD".to_string(),
                created_by_id: "tester".to_string(),
            },
        );

        let issues = validate_intentspec(&spec);
        assert!(
            issues
                .iter()
                .any(|i| i.message.contains("unknown artifact name")),
            "{issues:?}"
        );

        repair_intentspec(&mut spec, &issues);

        let issues = validate_intentspec(&spec);
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(
            spec.acceptance.verification_plan.fast_checks[0].artifacts_produced,
            vec!["build-log".to_string()]
        );
        assert!(
            spec.artifacts
                .required
                .iter()
                .any(|a| a.name == ArtifactName::BuildLog),
            "{:?}",
            spec.artifacts.required
        );
    }
}
