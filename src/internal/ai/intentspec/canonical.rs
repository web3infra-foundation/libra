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

/// Recursively sort all object keys in `value`, leaving arrays in
/// their original order and scalars unchanged.
///
/// Exposed at `pub(crate)` so call sites that already have a
/// `serde_json::Value` (e.g., tests, custom canonicalisation paths)
/// can reuse the same normalisation rules without going through
/// `to_canonical_json`'s `IntentSpec → Value` conversion.
pub(crate) fn normalize_value(value: Value) -> Value {
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

    /// `normalize_value` must reorder object keys alphabetically at
    /// every depth while leaving scalars and array element order
    /// untouched.
    #[test]
    fn normalize_value_sorts_object_keys_recursively() {
        let input: Value =
            serde_json::from_str(r#"{ "z": 1, "a": { "y": 2, "b": [3, 1, 2] }, "m": "stay" }"#)
                .unwrap();
        let out = normalize_value(input);
        let serialized = serde_json::to_string(&out).unwrap();

        // Top-level keys: a, m, z. Nested under "a": b, y.
        assert_eq!(serialized, r#"{"a":{"b":[3,1,2],"y":2},"m":"stay","z":1}"#,);
    }

    /// `normalize_value` must not reorder array elements (arrays are
    /// ordered by index, not key). This protects against accidental
    /// "let's sort arrays too" refactors that would silently corrupt
    /// `objectives` ordering in an IntentSpec.
    #[test]
    fn normalize_value_preserves_array_element_order() {
        let input: Value = serde_json::from_str(r#"[3, 1, 2]"#).unwrap();
        let out = normalize_value(input);
        assert_eq!(serde_json::to_string(&out).unwrap(), "[3,1,2]");
    }

    /// Scalars (numbers, strings, booleans, null) must pass through
    /// `normalize_value` unchanged.
    #[test]
    fn normalize_value_passes_scalars_through_unchanged() {
        for raw in ["42", "\"hello\"", "true", "false", "null"] {
            let value: Value = serde_json::from_str(raw).unwrap();
            let out = normalize_value(value.clone());
            assert_eq!(out, value, "scalar {raw} must be unchanged");
        }
    }

    /// Round-trip: parsing a canonical JSON back into an IntentSpec
    /// and re-canonicalising must yield the same string. This catches
    /// drift in `#[serde(skip_serializing_if)]` defaults or
    /// canonicalisation rules that don't survive a deserialise.
    #[test]
    fn canonical_json_round_trips_through_intentspec() {
        let spec = resolve_intentspec(
            IntentDraft {
                intent: DraftIntent {
                    summary: "Round trip".to_string(),
                    problem_statement: "Ensure stability".to_string(),
                    change_type: ChangeType::Chore,
                    objectives: vec![Objective {
                        title: "verify".to_string(),
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

        let canonical_a = to_canonical_json(&spec).unwrap();
        let parsed: IntentSpec = serde_json::from_str(&canonical_a).unwrap();
        let canonical_b = to_canonical_json(&parsed).unwrap();
        assert_eq!(
            canonical_a, canonical_b,
            "canonical JSON must round-trip via IntentSpec",
        );
    }

    /// The serialised top-level keys must be in alphabetical order,
    /// reflecting the BTreeMap re-ordering applied by
    /// `normalize_value`. This is what makes Libra's hash-based
    /// content addressing stable across language clients.
    #[test]
    fn canonical_json_top_level_keys_are_alphabetical() {
        let spec = resolve_intentspec(
            IntentDraft {
                intent: DraftIntent {
                    summary: "Order check".to_string(),
                    problem_statement: "Ensure order".to_string(),
                    change_type: ChangeType::Chore,
                    objectives: vec![Objective {
                        title: "verify".to_string(),
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

        let canonical = to_canonical_json(&spec).unwrap();
        let parsed: serde_json::Map<String, Value> = serde_json::from_str(&canonical).unwrap();
        let keys: Vec<&String> = parsed.keys().collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(
            keys, sorted,
            "canonical JSON top-level keys must be alphabetical",
        );
    }
}
