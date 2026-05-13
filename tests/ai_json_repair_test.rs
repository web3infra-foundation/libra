use libra::internal::ai::completion::{JsonRepairFixKind, parse_json_repaired};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Deserialize)]
struct JsonRepairFixture {
    name: String,
    input: String,
    #[serde(default)]
    expected: Option<Value>,
    #[serde(default)]
    repaired: Option<bool>,
    #[serde(default)]
    error_kind: Option<String>,
}

fn fixtures() -> Vec<JsonRepairFixture> {
    include_str!("data/ai_json_repair/json_repair.jsonl")
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            serde_json::from_str::<JsonRepairFixture>(line)
                .unwrap_or_else(|error| panic!("invalid fixture on line {}: {error}", index + 1))
        })
        .collect()
}

#[test]
fn ai_json_repair_fixture_corpus_has_required_coverage() {
    let fixtures = fixtures();
    let repairable = fixtures
        .iter()
        .filter(|fixture| fixture.expected.is_some())
        .count();
    let unrepairable = fixtures
        .iter()
        .filter(|fixture| fixture.error_kind.is_some())
        .count();

    assert!(
        repairable >= 20,
        "CEX-04 requires at least 20 malformed/valid repair fixtures"
    );
    assert!(
        unrepairable >= 3,
        "CEX-04 requires structured-error fixtures for unrecoverable input"
    );
}

#[test]
fn ai_json_repair_fixtures_match_core_contract() {
    for fixture in fixtures() {
        match (&fixture.expected, &fixture.error_kind) {
            (Some(expected), None) => {
                let outcome = parse_json_repaired(&fixture.input)
                    .unwrap_or_else(|error| panic!("{} should repair: {error}", fixture.name));
                assert_eq!(
                    &outcome.value, expected,
                    "{} repaired to the wrong JSON value",
                    fixture.name
                );
                assert_eq!(
                    outcome.repaired,
                    fixture.repaired.unwrap_or(true),
                    "{} repaired flag changed",
                    fixture.name
                );
                assert!(
                    serde_json::from_str::<Value>(&outcome.repaired_source).is_ok(),
                    "{} repaired_source must be valid JSON",
                    fixture.name
                );
            }
            (None, Some(kind)) => {
                let error = parse_json_repaired(&fixture.input)
                    .expect_err("unrepairable fixture unexpectedly repaired");
                assert_eq!(
                    error.kind.as_str(),
                    kind,
                    "{} error kind changed",
                    fixture.name
                );
            }
            _ => panic!(
                "{} must define exactly one of expected or error_kind",
                fixture.name
            ),
        }
    }
}

#[test]
fn valid_json_is_not_marked_repaired() {
    let outcome = parse_json_repaired(r#"{"file_path":"Cargo.toml"}"#).unwrap();

    assert_eq!(outcome.value, json!({"file_path": "Cargo.toml"}));
    assert!(!outcome.repaired);
    assert!(outcome.fixes.is_empty());
}

#[test]
fn repair_outcome_records_applied_fix_kinds() {
    let outcome =
        parse_json_repaired("```json\n{file_path: 'Cargo.toml', enabled: True,}\n```").unwrap();
    let fix_kinds = outcome.fixes.iter().map(|fix| fix.kind).collect::<Vec<_>>();

    assert!(fix_kinds.contains(&JsonRepairFixKind::StrippedCodeFence));
    assert!(fix_kinds.contains(&JsonRepairFixKind::ConvertedSingleQuotedStrings));
    assert!(fix_kinds.contains(&JsonRepairFixKind::QuotedObjectKeys));
    assert!(fix_kinds.contains(&JsonRepairFixKind::NormalizedPythonLiterals));
    assert!(fix_kinds.contains(&JsonRepairFixKind::RemovedTrailingCommas));
}
