use std::path::PathBuf;

use serde_json::Value;

const AI_OBJECT_FIXTURES: &[&str] = &[
    "ai-object.json",
    "ai-object-strict.json",
    "ai-object-event.json",
    "ai-object-projection.json",
];

const SENSITIVE_PAYLOAD_KEYS: &[&str] = &[
    "absoluteworkspacepath",
    "prompttext",
    "providerrawresponse",
    "providerrawtranscript",
    "toolpayload",
];

#[test]
fn publish_redaction_contract_test_public_fixtures_do_not_leak_sensitive_payloads() {
    for fixture in AI_OBJECT_FIXTURES {
        let raw = load_publish_fixture(fixture);
        let payload = raw
            .get("payload")
            .unwrap_or_else(|| panic!("{fixture} must have payload"));
        let mut leaks = Vec::new();
        collect_payload_leaks(format!("{fixture}.payload"), payload, &mut leaks);
        assert!(
            leaks.is_empty(),
            "{fixture} has unredacted public payload fields or values: {leaks:?}",
        );

        let redaction = raw
            .get("redaction")
            .and_then(Value::as_object)
            .unwrap_or_else(|| panic!("{fixture} must have redaction object"));
        assert!(
            redaction
                .get("rulesVersion")
                .and_then(Value::as_str)
                .is_some_and(|rules| !rules.trim().is_empty()),
            "{fixture} must pin redaction.rulesVersion",
        );
    }

    let default_object = load_publish_fixture("ai-object.json");
    assert_removed_fields_cover(
        &default_object,
        &[
            "payload.providerRawResponse",
            "payload.absoluteWorkspacePath",
        ],
    );

    let strict_object = load_publish_fixture("ai-object-strict.json");
    assert_removed_fields_cover(
        &strict_object,
        &[
            "payload.providerRawResponse",
            "payload.absoluteWorkspacePath",
            "payload.toolPayload",
            "payload.promptText",
            "payload.providerRawTranscript",
        ],
    );

    let bundle = load_publish_fixture("ai-bundle.json");
    let redaction = bundle
        .get("redaction")
        .and_then(Value::as_object)
        .expect("ai-bundle.json must have redaction object");
    let removed_by_type = redaction
        .get("removedFieldsByType")
        .and_then(Value::as_object)
        .expect("bundle redaction must include removedFieldsByType");
    assert!(
        removed_by_type
            .get("Run")
            .and_then(Value::as_array)
            .is_some_and(|fields| {
                fields.contains(&Value::String("payload.providerRawResponse".to_string()))
                    && fields.contains(&Value::String("payload.absoluteWorkspacePath".to_string()))
            }),
        "bundle Run redaction summary must include removed raw provider/path fields",
    );
    assert!(
        redaction
            .get("objectCountsByType")
            .and_then(Value::as_object)
            .is_some_and(|counts| !counts.is_empty()),
        "bundle redaction must carry objectCountsByType",
    );
}

fn assert_removed_fields_cover(raw: &Value, expected: &[&str]) {
    let removed = raw
        .get("removedFields")
        .and_then(Value::as_array)
        .expect("AI object must include removedFields array");
    for field in expected {
        assert!(
            removed.contains(&Value::String((*field).to_string())),
            "removedFields must include {field}",
        );
    }
}

fn collect_payload_leaks(path: String, value: &Value, leaks: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let child_path = format!("{path}.{key}");
                if is_sensitive_payload_key(key) {
                    leaks.push(child_path);
                    continue;
                }
                collect_payload_leaks(child_path, child, leaks);
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                collect_payload_leaks(format!("{path}[{index}]"), item, leaks);
            }
        }
        Value::String(text) if is_sensitive_string(text) => {
            leaks.push(format!("{path}={text:?}"));
        }
        _ => {}
    }
}

fn is_sensitive_payload_key(key: &str) -> bool {
    let normalized = key.replace(['_', '-'], "").to_lowercase();
    SENSITIVE_PAYLOAD_KEYS.contains(&normalized.as_str())
}

fn is_sensitive_string(text: &str) -> bool {
    text.contains("sk-")
        || text.contains("ghp_")
        || text.contains("AKIA")
        || text.contains("token=")
        || text.contains("secret=")
        || text.contains("/Users/")
        || text.contains("/Volumes/")
        || text.contains("/home/")
}

fn load_publish_fixture(name: &str) -> Value {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests/data/publish");
    path.push(name);
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
    serde_json::from_str(&raw).unwrap_or_else(|err| panic!("parse {}: {err}", path.display()))
}
