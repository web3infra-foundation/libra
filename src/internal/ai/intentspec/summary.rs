//! Human-readable summaries for IntentSpecs used in CLI, MCP, and review output.
//!
//! 用于 CLI、MCP 和审查输出的 IntentSpec 人类可读摘要。
//!
//! Boundary: summaries are presentation-only and must not feed back into canonical
//! storage. Intent-flow coverage checks that high-risk constraints and acceptance
//! criteria remain visible after normalization.

use super::types::IntentSpec;

pub fn render_summary(spec: &IntentSpec, intent_id: Option<&str>) -> String {
    let checks = &spec.acceptance.verification_plan;
    let check_count = checks.fast_checks.len()
        + checks.integration_checks.len()
        + checks.security_checks.len()
        + checks.release_checks.len();
    let intent_id_text = intent_id.unwrap_or("not-persisted");

    format!(
        "IntentSpec generated.\n\nIntent ID: {intent_id_text}\nRisk: {:?}\nObjectives: {}\nVerification checks: {}\nArtifacts required: {}",
        spec.risk.level,
        spec.intent.objectives.len(),
        check_count,
        spec.artifacts.required.len(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::intentspec::{
        ResolveContext,
        draft::{DraftAcceptance, DraftCheck, DraftIntent, DraftRisk, IntentDraft},
        resolve_intentspec,
        types::{ChangeType, CheckKind, Objective, ObjectiveKind, RiskLevel},
    };

    fn build_spec(
        risk: RiskLevel,
        objectives: Vec<&str>,
        fast_checks: Vec<&str>,
        integration_checks: Vec<&str>,
        security_checks: Vec<&str>,
        release_checks: Vec<&str>,
    ) -> IntentSpec {
        let mk_check = |id: &str| DraftCheck {
            id: id.to_string(),
            kind: CheckKind::Command,
            command: Some("echo ok".to_string()),
            timeout_seconds: Some(60),
            expected_exit_code: Some(0),
            required: true,
            artifacts_produced: vec![],
        };
        resolve_intentspec(
            IntentDraft {
                intent: DraftIntent {
                    summary: "summary".to_string(),
                    problem_statement: "problem".to_string(),
                    change_type: ChangeType::Chore,
                    objectives: objectives
                        .iter()
                        .map(|title| Objective {
                            title: (*title).to_string(),
                            kind: ObjectiveKind::Implementation,
                        })
                        .collect(),
                    in_scope: vec!["src".to_string()],
                    out_of_scope: vec![],
                    touch_hints: None,
                },
                acceptance: DraftAcceptance {
                    success_criteria: vec!["ok".to_string()],
                    fast_checks: fast_checks.iter().map(|id| mk_check(id)).collect(),
                    integration_checks: integration_checks.iter().map(|id| mk_check(id)).collect(),
                    security_checks: security_checks.iter().map(|id| mk_check(id)).collect(),
                    release_checks: release_checks.iter().map(|id| mk_check(id)).collect(),
                },
                risk: DraftRisk {
                    rationale: "rationale".to_string(),
                    factors: vec![],
                    level: Some(risk.clone()),
                },
            },
            risk,
            ResolveContext {
                working_dir: ".".to_string(),
                base_ref: "HEAD".to_string(),
                created_by_id: "tester".to_string(),
            },
        )
    }

    #[test]
    fn test_summary_contains_key_fields() {
        let spec = build_spec(RiskLevel::Low, vec!["fix"], vec![], vec![], vec![], vec![]);
        let summary = render_summary(&spec, Some("intent-123"));
        assert!(summary.contains("Intent ID: intent-123"));
        assert!(summary.contains("Objectives: 1"));
    }

    /// `intent_id = None` must render as `not-persisted` in the
    /// Intent ID line — the same sentinel used by `review.rs`.
    #[test]
    fn render_summary_uses_not_persisted_when_intent_id_is_none() {
        let spec = build_spec(RiskLevel::Low, vec!["fix"], vec![], vec![], vec![], vec![]);
        let summary = render_summary(&spec, None);
        assert!(
            summary.contains("Intent ID: not-persisted"),
            "got:\n{summary}",
        );
    }

    /// The `Risk:` line must use Rust's `{:?}` Debug formatter, which
    /// renders `RiskLevel` variants with their PascalCase names (not
    /// the lowercase serde tag). Pin this so a hand-rolled rendering
    /// refactor doesn't silently switch to `variant_name()`.
    #[test]
    fn render_summary_risk_line_uses_debug_format_pascal_case() {
        for (level, expected) in [
            (RiskLevel::Low, "Risk: Low"),
            (RiskLevel::Medium, "Risk: Medium"),
            (RiskLevel::High, "Risk: High"),
        ] {
            let spec = build_spec(level, vec!["obj"], vec![], vec![], vec![], vec![]);
            let summary = render_summary(&spec, Some("intent-1"));
            assert!(summary.contains(expected), "got:\n{summary}");
        }
    }

    /// Verification check count must sum across all four lists
    /// (fast/integration/security/release). Pin the count to verify
    /// no list is silently dropped from the sum.
    #[test]
    fn render_summary_verification_check_count_sums_all_four_lists() {
        // 1 fast + 2 integration + 1 security + 3 release = 7
        let spec = build_spec(
            RiskLevel::Low,
            vec!["obj"],
            vec!["fast-1"],
            vec!["integ-1", "integ-2"],
            vec!["sec-1"],
            vec!["rel-1", "rel-2", "rel-3"],
        );
        let summary = render_summary(&spec, Some("intent-1"));
        assert!(
            summary.contains("Verification checks: 7"),
            "expected count 7; got:\n{summary}",
        );
    }

    /// `Objectives` count reflects the length of the `objectives` vec,
    /// including the zero-objective edge case (which the validator
    /// later rejects, but render_summary itself must still compute
    /// the count without panicking).
    #[test]
    fn render_summary_objectives_count_handles_multiple_and_zero() {
        let three_obj = build_spec(
            RiskLevel::Low,
            vec!["a", "b", "c"],
            vec![],
            vec![],
            vec![],
            vec![],
        );
        let summary = render_summary(&three_obj, Some("intent-1"));
        assert!(summary.contains("Objectives: 3"), "got:\n{summary}");

        let zero_obj = build_spec(RiskLevel::Low, vec![], vec![], vec![], vec![], vec![]);
        let summary = render_summary(&zero_obj, Some("intent-1"));
        assert!(summary.contains("Objectives: 0"), "got:\n{summary}");
    }

    /// The summary must start with `IntentSpec generated.` — this
    /// header is the contract the TUI greps for to distinguish a
    /// successful spec rendering from any other CLI output.
    #[test]
    fn render_summary_starts_with_generated_header() {
        let spec = build_spec(RiskLevel::Low, vec!["obj"], vec![], vec![], vec![], vec![]);
        let summary = render_summary(&spec, Some("intent-1"));
        assert!(
            summary.starts_with("IntentSpec generated.\n\n"),
            "header drift; got:\n{summary}",
        );
    }
}
