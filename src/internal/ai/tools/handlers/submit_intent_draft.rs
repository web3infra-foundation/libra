//! Handler for the `submit_intent_draft` tool.

use async_trait::async_trait;
use serde_json::{Map, Value};

use super::{parse_argument_value, unwrap_json_string_value};
use crate::internal::ai::tools::{
    ToolResult,
    context::{SubmitIntentDraftArgs, ToolInvocation, ToolKind, ToolOutput, ToolPayload},
    error::ToolError,
    registry::ToolHandler,
    spec::ToolSpec,
};

/// Validates and acknowledges a structured IntentDraft submission.
///
/// The final draft payload is captured by the `/plan` observer.
pub struct SubmitIntentDraftHandler;

#[async_trait]
impl ToolHandler for SubmitIntentDraftHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> ToolResult<ToolOutput> {
        let arguments = match invocation.payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(ToolError::IncompatiblePayload(
                    "submit_intent_draft requires Function payload".into(),
                ));
            }
        };

        let _args = parse_submit_intent_draft_arguments(&arguments)?;
        Ok(ToolOutput::success("Intent draft submitted"))
    }

    fn schema(&self) -> ToolSpec {
        ToolSpec::submit_intent_draft()
    }
}

pub(crate) fn parse_submit_intent_draft_arguments(
    arguments: &str,
) -> ToolResult<SubmitIntentDraftArgs> {
    let value = parse_argument_value(arguments)?;
    parse_submit_intent_draft_value(&value)
}

pub(crate) fn parse_submit_intent_draft_value(value: &Value) -> ToolResult<SubmitIntentDraftArgs> {
    let mut value = unwrap_json_string_value(value.clone())?;
    unwrap_json_string_draft_field(&mut value)?;
    let mut value = normalize_submit_intent_draft_value(value);
    normalize_string_check_entries(&mut value)?;

    if value
        .pointer("/draft/intent/changeType")
        .and_then(Value::as_str)
        == Some("analysis")
    {
        return Err(ToolError::ParseError(
            "intent.changeType cannot be 'analysis'; use intent.objectives[*].kind='analysis' and set changeType='unknown' for read-only plans".into(),
        ));
    }

    serde_json::from_value(value)
        .map_err(|e| ToolError::ParseError(format!("Failed to parse arguments: {e}")))
}

fn unwrap_json_string_draft_field(value: &mut Value) -> ToolResult<()> {
    let Value::Object(map) = value else {
        return Ok(());
    };
    let Some(draft) = map.get_mut("draft") else {
        return Ok(());
    };

    *draft = unwrap_json_string_value(draft.clone())?;
    Ok(())
}

fn normalize_submit_intent_draft_value(value: Value) -> Value {
    match &value {
        Value::Object(map)
            if !map.contains_key("draft")
                && map.contains_key("intent")
                && map.contains_key("acceptance")
                && map.contains_key("risk") =>
        {
            serde_json::json!({ "draft": value })
        }
        _ => value,
    }
}

fn normalize_string_check_entries(value: &mut Value) -> ToolResult<()> {
    let Some(acceptance) = value
        .pointer_mut("/draft/acceptance")
        .and_then(Value::as_object_mut)
    else {
        return Ok(());
    };

    let mut extra_success_criteria = Vec::new();
    for field in [
        "fastChecks",
        "integrationChecks",
        "securityChecks",
        "releaseChecks",
    ] {
        if let Some(checks) = acceptance.get_mut(field) {
            normalize_check_entries(field, checks, &mut extra_success_criteria)?;
        }
    }

    append_success_criteria(acceptance, extra_success_criteria)
}

fn normalize_check_entries(
    field: &str,
    checks: &mut Value,
    extra_success_criteria: &mut Vec<String>,
) -> ToolResult<()> {
    let value = unwrap_json_string_value(checks.take())?;
    match value {
        Value::Array(items) => {
            let mut normalized = Vec::with_capacity(items.len());
            for item in items {
                match unwrap_json_string_value(item)? {
                    object @ Value::Object(_) => normalized.push(object),
                    Value::String(text) => push_success_criterion(extra_success_criteria, text),
                    Value::Null => {}
                    other => {
                        return Err(ToolError::ParseError(format!(
                            "acceptance.{field} entries must be check objects or strings, got {}",
                            json_type_name(&other)
                        )));
                    }
                }
            }
            *checks = Value::Array(normalized);
        }
        object @ Value::Object(_) => {
            *checks = Value::Array(vec![object]);
        }
        Value::String(text) => {
            push_success_criterion(extra_success_criteria, text);
            *checks = Value::Array(Vec::new());
        }
        Value::Null => {
            *checks = Value::Array(Vec::new());
        }
        other => {
            return Err(ToolError::ParseError(format!(
                "acceptance.{field} must be an array of check objects, got {}",
                json_type_name(&other)
            )));
        }
    }
    Ok(())
}

fn append_success_criteria(
    acceptance: &mut Map<String, Value>,
    extra_success_criteria: Vec<String>,
) -> ToolResult<()> {
    if extra_success_criteria.is_empty() {
        return Ok(());
    }

    let criteria = acceptance
        .entry("successCriteria")
        .or_insert_with(|| Value::Array(Vec::new()));
    if criteria.is_string() || criteria.is_null() {
        let initial = criteria.as_str().map(str::to_string);
        let mut values = Vec::new();
        if let Some(initial) = initial {
            push_success_criterion_value(&mut values, initial);
        }
        *criteria = Value::Array(values);
    }

    let Value::Array(criteria) = criteria else {
        return Err(ToolError::ParseError(
            "acceptance.successCriteria must be an array of strings".into(),
        ));
    };

    for text in extra_success_criteria {
        push_success_criterion_value(criteria, text);
    }
    Ok(())
}

fn push_success_criterion(criteria: &mut Vec<String>, text: String) {
    let text = text.trim();
    if !text.is_empty() && !criteria.iter().any(|existing| existing == text) {
        criteria.push(text.to_string());
    }
}

fn push_success_criterion_value(criteria: &mut Vec<Value>, text: String) {
    let text = text.trim();
    if !text.is_empty()
        && !criteria
            .iter()
            .any(|existing| existing.as_str() == Some(text))
    {
        criteria.push(Value::String(text.to_string()));
    }
}

fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn make_invocation(json: &str) -> ToolInvocation {
        ToolInvocation::new(
            "call-intent-draft-1",
            "submit_intent_draft",
            ToolPayload::Function {
                arguments: json.to_string(),
            },
            PathBuf::from("/tmp"),
        )
    }

    fn valid_draft_value() -> Value {
        serde_json::json!({
            "intent": {
                "summary": "Initialize cargo project",
                "problemStatement": "The project needs a cargo-based Rust layout",
                "changeType": "feature",
                "objectives": [{"title": "create cargo project", "kind": "implementation"}],
                "inScope": ["."],
                "outOfScope": []
            },
            "acceptance": {
                "successCriteria": ["cargo check succeeds"],
                "fastChecks": [],
                "integrationChecks": [],
                "securityChecks": [],
                "releaseChecks": []
            },
            "risk": {
                "rationale": "new project scaffold"
            }
        })
    }

    #[tokio::test]
    async fn test_valid_draft_submission() {
        let handler = SubmitIntentDraftHandler;
        let inv = make_invocation(
            r#"{
                "draft": {
                    "intent": {
                        "summary": "Fix bug",
                        "problemStatement": "A bug exists",
                        "changeType": "bugfix",
                        "objectives": [{"title": "fix it", "kind": "implementation"}],
                        "inScope": ["src/main.rs"],
                        "outOfScope": []
                    },
                    "acceptance": {
                        "successCriteria": ["tests pass"],
                        "fastChecks": [],
                        "integrationChecks": [],
                        "securityChecks": [],
                        "releaseChecks": []
                    },
                    "risk": {
                        "rationale": "limited blast radius"
                    }
                }
            }"#,
        );
        let result = handler.handle(inv).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_text(), Some("Intent draft submitted"));
    }

    #[tokio::test]
    async fn test_direct_draft_submission_is_accepted() {
        let handler = SubmitIntentDraftHandler;
        let inv = make_invocation(&valid_draft_value().to_string());

        let result = handler.handle(inv).await;

        assert!(result.is_ok(), "{result:?}");
    }

    #[tokio::test]
    async fn test_json_string_encoded_draft_submission_is_accepted() {
        let handler = SubmitIntentDraftHandler;
        let encoded = serde_json::to_string(&valid_draft_value().to_string()).unwrap();
        let inv = make_invocation(&encoded);

        let result = handler.handle(inv).await;

        assert!(result.is_ok(), "{result:?}");
    }

    #[tokio::test]
    async fn test_json_string_encoded_draft_field_is_accepted() {
        let handler = SubmitIntentDraftHandler;
        let inv = make_invocation(
            &serde_json::json!({
                "draft": valid_draft_value().to_string()
            })
            .to_string(),
        );

        let result = handler.handle(inv).await;

        assert!(result.is_ok(), "{result:?}");
    }

    #[tokio::test]
    async fn test_json_string_encoded_draft_field_with_extra_closing_brace_is_accepted() {
        let handler = SubmitIntentDraftHandler;
        let encoded_with_extra_closing = format!("{}}}", valid_draft_value());
        let inv = make_invocation(
            &serde_json::json!({
                "draft": encoded_with_extra_closing
            })
            .to_string(),
        );

        let result = handler.handle(inv).await;

        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn test_string_check_entries_are_moved_to_success_criteria() {
        let args = parse_submit_intent_draft_arguments(
            r#"{
                "draft": {
                    "intent": {
                        "summary": "Create hello file",
                        "problemStatement": "Need a hello.txt file",
                        "changeType": "feature",
                        "objectives": [{"title": "create hello.txt", "kind": "implementation"}],
                        "inScope": ["hello.txt"],
                        "outOfScope": []
                    },
                    "acceptance": {
                        "successCriteria": ["File created successfully"],
                        "fastChecks": [
                            "File hello.txt exists at project root",
                            "File contains exactly 'hello from libra'"
                        ],
                        "integrationChecks": [],
                        "securityChecks": [],
                        "releaseChecks": []
                    },
                    "risk": {
                        "rationale": "single-file change"
                    }
                }
            }"#,
        )
        .expect("string check entries should be accepted");

        assert!(args.draft.acceptance.fast_checks.is_empty());
        assert!(
            args.draft
                .acceptance
                .success_criteria
                .contains(&"File hello.txt exists at project root".to_string())
        );
        assert!(
            args.draft
                .acceptance
                .success_criteria
                .contains(&"File contains exactly 'hello from libra'".to_string())
        );
    }

    #[test]
    fn test_json_string_encoded_check_object_is_accepted() {
        let command_check = serde_json::json!({
            "command": "test -f hello.txt",
            "expectedExitCode": 0
        })
        .to_string();
        let args = parse_submit_intent_draft_arguments(
            &serde_json::json!({
                "draft": {
                    "intent": {
                        "summary": "Create hello file",
                        "problemStatement": "Need a hello.txt file",
                        "changeType": "feature",
                        "objectives": [{"title": "create hello.txt", "kind": "implementation"}],
                        "inScope": ["hello.txt"],
                        "outOfScope": []
                    },
                    "acceptance": {
                        "successCriteria": ["File created successfully"],
                        "fastChecks": [command_check],
                        "integrationChecks": [],
                        "securityChecks": [],
                        "releaseChecks": []
                    },
                    "risk": {
                        "rationale": "single-file change"
                    }
                }
            })
            .to_string(),
        )
        .expect("JSON string encoded check object should be accepted");

        assert_eq!(args.draft.acceptance.fast_checks.len(), 1);
        assert_eq!(
            args.draft.acceptance.fast_checks[0].command.as_deref(),
            Some("test -f hello.txt")
        );
    }

    #[tokio::test]
    async fn test_invalid_draft_submission() {
        let handler = SubmitIntentDraftHandler;
        let inv = make_invocation(r#"{"draft": {"intent": {}}}"#);
        let result = handler.handle(inv).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_change_type_analysis_returns_actionable_error() {
        let handler = SubmitIntentDraftHandler;
        let inv = make_invocation(
            r#"{
                "draft": {
                    "intent": {
                        "summary": "Analyze repo",
                        "problemStatement": "Need a read-only diagnosis",
                        "changeType": "analysis",
                        "objectives": [{"title": "inspect", "kind": "analysis"}],
                        "inScope": ["src"],
                        "outOfScope": []
                    },
                    "acceptance": {
                        "successCriteria": ["report findings"],
                        "fastChecks": [],
                        "integrationChecks": [],
                        "securityChecks": [],
                        "releaseChecks": []
                    },
                    "risk": {
                        "rationale": "read-only"
                    }
                }
            }"#,
        );
        let result = handler.handle(inv).await;
        let err = result.expect_err("changeType=analysis should be rejected");
        assert!(
            err.to_string()
                .contains("intent.changeType cannot be 'analysis'")
        );
    }

    #[tokio::test]
    async fn test_draft_check_missing_id_is_accepted() {
        let handler = SubmitIntentDraftHandler;
        let inv = make_invocation(
            r#"{
                "draft": {
                    "intent": {
                        "summary": "Initialize cargo project",
                        "problemStatement": "The project needs a cargo-based Rust layout",
                        "changeType": "feature",
                        "objectives": [{"title": "create cargo project", "kind": "implementation"}],
                        "inScope": ["."],
                        "outOfScope": []
                    },
                    "acceptance": {
                        "successCriteria": ["cargo check succeeds"],
                        "fastChecks": [{
                            "kind": "command",
                            "command": "cargo check",
                            "timeoutSeconds": 120,
                            "expectedExitCode": 0,
                            "required": true,
                            "artifactsProduced": []
                        }],
                        "integrationChecks": [],
                        "securityChecks": [],
                        "releaseChecks": []
                    },
                    "risk": {
                        "rationale": "new project scaffold"
                    }
                }
            }"#,
        );

        let result = handler.handle(inv).await;

        assert!(result.is_ok(), "{result:?}");
    }

    #[tokio::test]
    async fn test_draft_check_missing_kind_is_accepted_when_command_is_present() {
        let handler = SubmitIntentDraftHandler;
        let inv = make_invocation(
            r#"{
                "draft": {
                    "intent": {
                        "summary": "Initialize cargo project",
                        "problemStatement": "The project needs a cargo-based Rust layout without VCS",
                        "changeType": "feature",
                        "objectives": [{"title": "create cargo project", "kind": "implementation"}],
                        "inScope": ["."],
                        "outOfScope": []
                    },
                    "acceptance": {
                        "successCriteria": ["cargo check succeeds"],
                        "fastChecks": [{
                            "command": "cargo check",
                            "timeoutSeconds": 120,
                            "expectedExitCode": 0,
                            "required": true,
                            "artifactsProduced": []
                        }],
                        "integrationChecks": [],
                        "securityChecks": [],
                        "releaseChecks": []
                    },
                    "risk": {
                        "rationale": "new project scaffold"
                    }
                }
            }"#,
        );

        let result = handler.handle(inv).await;

        assert!(result.is_ok(), "{result:?}");
    }
}
