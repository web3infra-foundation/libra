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
        draft::{DraftAcceptance, DraftIntent, DraftRisk, IntentDraft},
        resolve_intentspec,
        types::{ChangeType, RiskLevel},
    };

    #[test]
    fn test_summary_contains_key_fields() {
        let spec = resolve_intentspec(
            IntentDraft {
                intent: DraftIntent {
                    summary: "Fix bug".to_string(),
                    problem_statement: "Bug".to_string(),
                    change_type: ChangeType::Bugfix,
                    objectives: vec!["fix".to_string()],
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
                    rationale: "safe".to_string(),
                    factors: vec![],
                    level: Some(RiskLevel::Low),
                },
            },
            RiskLevel::Low,
            ResolveContext {
                working_dir: ".".to_string(),
                base_ref: "HEAD".to_string(),
                created_by_id: "tester".to_string(),
            },
        );
        let summary = render_summary(&spec, Some("intent-123"));
        assert!(summary.contains("Intent ID: intent-123"));
        assert!(summary.contains("Objectives: 1"));
    }
}
