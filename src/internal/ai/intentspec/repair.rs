//! Deterministic repair passes for incomplete or loosely-formed IntentSpec drafts.
//!
//! Boundary: repair may add defaults and normalize fields, but it must not invent
//! hidden goals or widen user scope. Edge cases around absent checks, duplicated
//! artifacts, and missing acceptance criteria are exercised by the intent-flow tests.

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
    ensure_tool_rule(
        &mut spec.security.tool_acl.allow,
        "libra.vcs",
        &["read", "write"],
    );

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

fn ensure_tool_rule(allow: &mut Vec<ToolRule>, tool: &str, actions: &[&str]) {
    if let Some(rule) = allow.iter_mut().find(|rule| rule.tool == tool) {
        for action in actions {
            if !rule.actions.iter().any(|existing| existing == action) {
                rule.actions.push((*action).to_string());
            }
        }
        return;
    }

    allow.push(ToolRule {
        tool: tool.to_string(),
        actions: actions.iter().map(|action| (*action).to_string()).collect(),
        constraints: Default::default(),
    });
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

        spec.security
            .tool_acl
            .allow
            .retain(|rule| rule.tool == "workspace.fs");
        repair_intentspec(&mut spec, &issues);

        let issues = validate_intentspec(&spec);
        assert!(issues.is_empty(), "{issues:?}");
        let libra_vcs_rule = spec
            .security
            .tool_acl
            .allow
            .iter()
            .find(|rule| rule.tool == "libra.vcs")
            .expect("repair should add libra.vcs ACL");
        assert!(libra_vcs_rule.actions.contains(&"read".to_string()));
        assert!(libra_vcs_rule.actions.contains(&"write".to_string()));
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

    fn minimal_spec(risk_level: RiskLevel, change_type: ChangeType) -> IntentSpec {
        resolve_intentspec(
            IntentDraft {
                intent: DraftIntent {
                    summary: "summary".to_string(),
                    problem_statement: "problem".to_string(),
                    change_type,
                    objectives: vec![Objective {
                        title: "do work".to_string(),
                        kind: ObjectiveKind::Implementation,
                    }],
                    in_scope: vec!["src".to_string()],
                    out_of_scope: vec![],
                    touch_hints: None,
                },
                acceptance: DraftAcceptance {
                    success_criteria: vec!["pass".to_string()],
                    fast_checks: vec![],
                    integration_checks: vec![],
                    security_checks: vec![],
                    release_checks: vec![],
                },
                risk: DraftRisk {
                    rationale: "rationale".to_string(),
                    factors: vec![],
                    level: Some(risk_level.clone()),
                },
            },
            risk_level,
            ResolveContext {
                working_dir: ".".to_string(),
                base_ref: "HEAD".to_string(),
                created_by_id: "tester".to_string(),
            },
        )
    }

    /// `repair_intentspec` must enforce the high-risk human-loop rule:
    /// `required = true` AND `min_approvers >= 2`. Pin both branches.
    #[test]
    fn repair_high_risk_forces_human_loop_required_and_min_approvers() {
        let mut spec = minimal_spec(RiskLevel::High, ChangeType::Chore);
        spec.risk.human_in_loop.required = false;
        spec.risk.human_in_loop.min_approvers = 0;

        repair_intentspec(&mut spec, &[]);

        assert!(spec.risk.human_in_loop.required);
        assert!(
            spec.risk.human_in_loop.min_approvers >= 2,
            "got min_approvers={}",
            spec.risk.human_in_loop.min_approvers,
        );
    }

    /// Low-risk specs MUST NOT have their human-loop values changed by
    /// repair — the high-risk gate is conditioned on `RiskLevel::High`.
    #[test]
    fn repair_low_risk_leaves_human_loop_untouched() {
        let mut spec = minimal_spec(RiskLevel::Low, ChangeType::Chore);
        spec.risk.human_in_loop.required = false;
        spec.risk.human_in_loop.min_approvers = 0;

        repair_intentspec(&mut spec, &[]);

        assert!(!spec.risk.human_in_loop.required);
        assert_eq!(spec.risk.human_in_loop.min_approvers, 0);
    }

    /// `repair_intentspec` must force
    /// `acceptance.qualityGates.requireNewTestsWhenBugfix = Some(true)`
    /// when `intent.changeType == Bugfix`, including when the
    /// `quality_gates` slot is `None` (must be initialized).
    #[test]
    fn repair_bugfix_initializes_quality_gates_with_new_tests_flag() {
        let mut spec = minimal_spec(RiskLevel::Low, ChangeType::Bugfix);
        spec.acceptance.quality_gates = None;

        repair_intentspec(&mut spec, &[]);

        let quality = spec
            .acceptance
            .quality_gates
            .expect("repair should initialize quality_gates for Bugfix");
        assert_eq!(quality.require_new_tests_when_bugfix, Some(true));
    }

    /// `repair_intentspec` must inject a default `workspace.fs` ACL rule
    /// when `security.tool_acl.allow` is empty, AND also ensure the
    /// `libra.vcs` rule exists with read/write actions (unconditional).
    #[test]
    fn repair_empty_security_acl_injects_workspace_fs_and_libra_vcs() {
        let mut spec = minimal_spec(RiskLevel::Low, ChangeType::Chore);
        spec.security.tool_acl.allow.clear();

        repair_intentspec(&mut spec, &[]);

        let workspace_fs = spec
            .security
            .tool_acl
            .allow
            .iter()
            .find(|rule| rule.tool == "workspace.fs")
            .expect("workspace.fs rule must be injected");
        assert!(workspace_fs.actions.iter().any(|a| a == "read"));
        assert!(workspace_fs.actions.iter().any(|a| a == "write"));

        let libra_vcs = spec
            .security
            .tool_acl
            .allow
            .iter()
            .find(|rule| rule.tool == "libra.vcs")
            .expect("libra.vcs rule must be injected");
        assert!(libra_vcs.actions.iter().any(|a| a == "read"));
        assert!(libra_vcs.actions.iter().any(|a| a == "write"));
    }

    /// `repair_intentspec` must reset `lifecycle.status` to `Active`
    /// and clear `change_log` (initial-state invariant for newly
    /// repaired specs).
    #[test]
    fn repair_resets_lifecycle_status_and_clears_change_log() {
        use crate::internal::ai::intentspec::types::ChangeLogEntry;
        let mut spec = minimal_spec(RiskLevel::Low, ChangeType::Chore);
        spec.lifecycle.status = LifecycleStatus::Deprecated;
        spec.lifecycle.change_log.push(ChangeLogEntry {
            at: "2026-01-01T00:00:00Z".to_string(),
            by: "tester".to_string(),
            reason: "manual entry".to_string(),
            diff_summary: "no-op".to_string(),
        });

        repair_intentspec(&mut spec, &[]);

        assert_eq!(spec.lifecycle.status, LifecycleStatus::Active);
        assert!(spec.lifecycle.change_log.is_empty());
    }

    /// `repair_intentspec` must clamp `artifacts.retention.days` to
    /// `min(constraints.privacy.retentionDays, artifacts.retention.days)`.
    /// Pin both directions of the clamp.
    #[test]
    fn repair_clamps_retention_days_to_minimum() {
        // artifacts.retention.days higher than constraint → clamped down.
        let mut spec = minimal_spec(RiskLevel::Low, ChangeType::Chore);
        spec.constraints.privacy.retention_days = 30;
        spec.artifacts.retention.days = 100;
        repair_intentspec(&mut spec, &[]);
        assert_eq!(spec.artifacts.retention.days, 30);

        // artifacts.retention.days lower than constraint → unchanged.
        let mut spec = minimal_spec(RiskLevel::Low, ChangeType::Chore);
        spec.constraints.privacy.retention_days = 100;
        spec.artifacts.retention.days = 30;
        repair_intentspec(&mut spec, &[]);
        assert_eq!(spec.artifacts.retention.days, 30);
    }

    /// `looks_like_cargo_build_output` heuristic must accept paths
    /// containing both `/target/` AND `/{release,debug}/` segments,
    /// in both forward-slash and backslash forms.
    #[test]
    fn cargo_build_output_heuristic_accepts_target_and_profile_segments() {
        assert!(looks_like_cargo_build_output("foo/target/release/bar"));
        assert!(looks_like_cargo_build_output("foo/target/debug/bar"));
        assert!(looks_like_cargo_build_output("foo\\target\\release\\bar"));
        assert!(looks_like_cargo_build_output("foo\\target\\debug\\bar"));
    }

    /// `looks_like_cargo_build_output` must reject paths that have only
    /// one of the two required segments.
    #[test]
    fn cargo_build_output_heuristic_rejects_partial_match() {
        // Only target, no profile.
        assert!(!looks_like_cargo_build_output("foo/target/bar"));
        // Only profile, no target.
        assert!(!looks_like_cargo_build_output("foo/release/bar"));
        // Plain unrelated path.
        assert!(!looks_like_cargo_build_output("src/main.rs"));
        // Empty.
        assert!(!looks_like_cargo_build_output(""));
    }

    /// `parse_artifact_name` must round-trip all 9 known artifact
    /// names and return `None` for everything else. Drift here would
    /// silently widen / narrow the artifact-name allowlist.
    #[test]
    fn parse_artifact_name_handles_all_known_and_unknown_values() {
        let known = [
            ("patchset", ArtifactName::Patchset),
            ("test-log", ArtifactName::TestLog),
            ("build-log", ArtifactName::BuildLog),
            ("sast-report", ArtifactName::SastReport),
            ("sca-report", ArtifactName::ScaReport),
            ("sbom", ArtifactName::Sbom),
            (
                "provenance-attestation",
                ArtifactName::ProvenanceAttestation,
            ),
            ("transparency-proof", ArtifactName::TransparencyProof),
            ("release-notes", ArtifactName::ReleaseNotes),
        ];
        for (raw, expected) in known {
            assert_eq!(parse_artifact_name(raw), Some(expected), "raw = {raw}");
        }

        for unknown in ["", "patchset ", "PATCHSET", "build_log", "garbage"] {
            assert_eq!(
                parse_artifact_name(unknown),
                None,
                "unknown {unknown:?} must return None",
            );
        }
    }
}
