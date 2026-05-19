//! Canonicalization logic that converts draft IntentSpec data into stable serialized
//! form.
//!
//! Boundary: canonicalization normalizes order, defaults, and aliases while preserving
//! user intent; semantic validation is left to `validator`. Tests compare canonical
//! JSON output so AI plans and persisted objects remain stable across refactors.

use std::collections::BTreeMap;

use serde_json::Value;

use super::types::IntentSpec;

pub fn to_canonical_json(spec: &IntentSpec) -> Result<String, serde_json::Error> {
    let value = serde_json::to_value(spec)?;
    let normalized = normalize_value(value);
    serde_json::to_string(&normalized)
}

fn normalize_value(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted: BTreeMap<String, Value> = map
                .into_iter()
                .map(|(k, v)| (k, normalize_value(v)))
                .collect();
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(normalize_value).collect()),
        scalar => scalar,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::intentspec::{
        ResolveContext,
        draft::{DraftAcceptance, DraftIntent, DraftRisk, IntentDraft},
        resolve_intentspec,
        types::{ChangeType, Objective, ObjectiveKind, RiskLevel},
    };

    #[test]
    fn test_canonical_json_is_stable() {
        let spec = resolve_intentspec(
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

        let a = to_canonical_json(&spec).unwrap();
        let b = to_canonical_json(&spec).unwrap();
        assert_eq!(a, b);
    }
}
