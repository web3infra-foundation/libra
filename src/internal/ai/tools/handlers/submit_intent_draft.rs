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
    let mut value = match value {
        Value::Object(map)
            if !map.contains_key("draft")
                && (map.contains_key("intent")
                    || map.contains_key("acceptance")
                    || map.contains_key("risk")
                    || looks_like_flat_intent_draft_object(&map)) =>
        {
            serde_json::json!({ "draft": Value::Object(map) })
        }
        other => other,
    };

    normalize_submit_intent_draft_shape(&mut value);
    value
}

fn looks_like_flat_intent_draft_object(map: &Map<String, Value>) -> bool {
    [
        "summary",
        "problemStatement",
        "changeType",
        "objectives",
        "inScope",
        "outOfScope",
        "touchHints",
        "riskProfile",
        "title",
        "description",
    ]
    .into_iter()
    .any(|key| map.contains_key(key))
}

fn normalize_submit_intent_draft_shape(value: &mut Value) {
    let Some(draft) = value.pointer_mut("/draft").and_then(Value::as_object_mut) else {
        return;
    };

    if !draft.contains_key("intent") {
        let intent = extract_kimi_intent_object(draft);
        if !intent.is_empty() {
            draft.insert("intent".to_string(), Value::Object(intent));
        }
    }

    let draft_summary = draft.get("summary").cloned();
    let draft_problem_statement = draft.get("problemStatement").cloned();
    let draft_in_scope = draft.get("inScope").cloned();
    let draft_out_of_scope = draft.get("outOfScope").cloned();
    let draft_touch_hints = draft.get("touchHints").cloned();
    let draft_risk_profile = draft.get("riskProfile").cloned();

    if let Some(intent) = draft.get_mut("intent").and_then(Value::as_object_mut) {
        fill_missing_object_field(intent, "summary", draft_summary);
        fill_missing_object_field(intent, "problemStatement", draft_problem_statement);
        fill_missing_object_field(intent, "inScope", draft_in_scope);
        fill_missing_object_field(intent, "outOfScope", draft_out_of_scope);
        fill_missing_object_field(intent, "touchHints", draft_touch_hints);
        fill_missing_object_field(intent, "riskProfile", draft_risk_profile);

        synthesize_intent_fields(intent);
    }

    synthesize_acceptance(draft);
    synthesize_risk(draft);
}

fn extract_kimi_intent_object(draft: &Map<String, Value>) -> Map<String, Value> {
    let mut intent = Map::new();
    for key in [
        "summary",
        "problemStatement",
        "changeType",
        "objectives",
        "inScope",
        "outOfScope",
        "touchHints",
        "riskProfile",
        "title",
        "description",
    ] {
        if let Some(value) = draft.get(key) {
            intent.insert(key.to_string(), value.clone());
        }
    }
    intent
}

fn fill_missing_object_field(map: &mut Map<String, Value>, key: &str, value: Option<Value>) {
    if map.contains_key(key) {
        return;
    }

    if let Some(value) = value
        && !value.is_null()
    {
        map.insert(key.to_string(), value);
    }
}

fn synthesize_intent_fields(intent: &mut Map<String, Value>) {
    if !intent.contains_key("summary")
        && let Some(summary) = first_non_empty_string(intent, ["summary", "title", "description"])
    {
        intent.insert("summary".to_string(), Value::String(summary));
    }

    if !intent.contains_key("problemStatement")
        && let Some(problem_statement) = first_non_empty_string(
            intent,
            ["problemStatement", "description", "summary", "title"],
        )
    {
        intent.insert(
            "problemStatement".to_string(),
            Value::String(problem_statement),
        );
    }

    if !intent.contains_key("inScope") {
        let in_scope = intent
            .get("touchHints")
            .and_then(Value::as_object)
            .and_then(|touch_hints| touch_hints.get("files"))
            .cloned()
            .filter(|value| value.as_array().is_some_and(|items| !items.is_empty()))
            .unwrap_or_else(|| serde_json::json!(["."]));
        intent.insert("inScope".to_string(), in_scope);
    }

    if !intent.contains_key("outOfScope") {
        intent.insert("outOfScope".to_string(), Value::Array(Vec::new()));
    }
}

fn synthesize_acceptance(draft: &mut Map<String, Value>) {
    let (objective_criteria, objective_title_fallbacks, summary_fallback) = draft
        .get("intent")
        .and_then(Value::as_object)
        .map(|intent| {
            (
                collect_objective_acceptance_criteria(intent),
                collect_objective_title_fallbacks(intent),
                intent
                    .get("summary")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|summary| !summary.is_empty())
                    .map(str::to_string),
            )
        })
        .unwrap_or_default();
    let plan_criteria = collect_plan_verification_criteria(draft);

    let acceptance = draft
        .entry("acceptance".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !acceptance.is_object() {
        *acceptance = Value::Object(Map::new());
    }

    let Some(acceptance) = acceptance.as_object_mut() else {
        return;
    };

    let mut success_criteria = acceptance
        .get("successCriteria")
        .map(value_to_string_list)
        .unwrap_or_default();
    for criterion in objective_criteria {
        push_success_criterion(&mut success_criteria, criterion);
    }
    for criterion in plan_criteria {
        push_success_criterion(&mut success_criteria, criterion);
    }
    if success_criteria.is_empty() {
        for criterion in objective_title_fallbacks {
            push_success_criterion(&mut success_criteria, criterion);
        }
    }
    if success_criteria.is_empty()
        && let Some(summary) = summary_fallback
    {
        push_success_criterion(
            &mut success_criteria,
            format!("Deliver requested scope: {summary}"),
        );
    }
    acceptance.insert(
        "successCriteria".to_string(),
        Value::Array(
            success_criteria
                .into_iter()
                .map(Value::String)
                .collect::<Vec<_>>(),
        ),
    );

    for field in [
        "fastChecks",
        "integrationChecks",
        "securityChecks",
        "releaseChecks",
    ] {
        if !acceptance.contains_key(field) {
            acceptance.insert(field.to_string(), Value::Array(Vec::new()));
        }
    }
}

fn collect_objective_acceptance_criteria(intent: &Map<String, Value>) -> Vec<String> {
    intent
        .get("objectives")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .flat_map(|objective| {
            objective
                .get("acceptanceCriteria")
                .map(value_to_string_list)
                .unwrap_or_default()
        })
        .collect()
}

fn collect_objective_title_fallbacks(intent: &Map<String, Value>) -> Vec<String> {
    intent
        .get("objectives")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .filter_map(|objective| objective.get("title"))
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(|title| format!("Complete objective: {title}"))
        .collect()
}

fn collect_plan_verification_criteria(draft: &Map<String, Value>) -> Vec<String> {
    draft
        .get("plan")
        .and_then(Value::as_object)
        .and_then(|plan| plan.get("steps"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .flat_map(|step| {
            let verification = step.get("verification").and_then(Value::as_object);
            let mut criteria = Vec::new();

            if let Some(check) = verification
                .and_then(|verification| verification.get("check"))
                .and_then(Value::as_str)
            {
                push_success_criterion(&mut criteria, check.to_string());
            }

            if criteria.is_empty()
                && let Some(command) = verification
                    .and_then(|verification| verification.get("command"))
                    .and_then(Value::as_str)
            {
                push_success_criterion(
                    &mut criteria,
                    format!("Verification command succeeds: {command}"),
                );
            }

            criteria
        })
        .collect()
}

fn synthesize_risk(draft: &mut Map<String, Value>) {
    let risk_profile = draft
        .get("riskProfile")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|profile| !profile.is_empty())
        .map(str::to_lowercase)
        .or_else(|| {
            draft
                .get("intent")
                .and_then(Value::as_object)
                .and_then(|intent| intent.get("riskProfile"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|profile| !profile.is_empty())
                .map(str::to_lowercase)
        });

    let risk = draft
        .entry("risk".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !risk.is_object() {
        *risk = Value::Object(Map::new());
    }

    let Some(risk) = risk.as_object_mut() else {
        return;
    };

    if !risk.contains_key("rationale") {
        let rationale = risk_profile
            .as_deref()
            .map(|profile| format!("Planner-estimated risk profile: {profile}"))
            .unwrap_or_else(|| "Planner did not provide a risk rationale.".to_string());
        risk.insert("rationale".to_string(), Value::String(rationale));
    }

    if !risk.contains_key("factors") {
        risk.insert("factors".to_string(), Value::Array(Vec::new()));
    }

    if !risk.contains_key("level")
        && let Some(profile) = risk_profile
            .as_deref()
            .filter(|profile| matches!(*profile, "low" | "medium" | "high"))
    {
        risk.insert("level".to_string(), Value::String(profile.to_string()));
    }
}

fn first_non_empty_string<const N: usize>(
    map: &Map<String, Value>,
    keys: [&str; N],
) -> Option<String> {
    keys.into_iter()
        .filter_map(|key| map.get(key))
        .find_map(|value| match value {
            Value::String(text) => {
                let text = text.trim();
                (!text.is_empty()).then(|| text.to_string())
            }
            _ => None,
        })
}

fn value_to_string_list(value: &Value) -> Vec<String> {
    match value {
        Value::String(text) => {
            let text = text.trim();
            if text.is_empty() {
                Vec::new()
            } else {
                vec![text.to_string()]
            }
        }
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
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
    fn test_risk_level_is_case_insensitive() {
        let args = parse_submit_intent_draft_arguments(
            &serde_json::json!({
                "draft": {
                    "intent": {
                        "summary": "Inspect repository",
                        "problemStatement": "Need a small read-only diagnosis.",
                        "changeType": "unknown",
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
                        "rationale": "read-only",
                        "level": "Low"
                    }
                }
            })
            .to_string(),
        )
        .expect("PascalCase risk level should be accepted");

        assert_eq!(
            args.draft.risk.level,
            Some(crate::internal::ai::intentspec::types::RiskLevel::Low)
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

    #[test]
    fn test_kimi_style_intent_derives_missing_fields_from_title_description_and_objectives() {
        let args = parse_submit_intent_draft_arguments(
            &serde_json::json!({
                "draft": {
                    "intent": {
                        "changeType": "feature",
                        "description": "Create a new Rust project named libra using cargo without VCS support. Implement three CLI subcommands (code, cloud, backup) that echo their names. Ensure the codebase passes nightly formatting, clippy with zero warnings, and all tests.",
                        "objectives": [
                            {
                                "title": "Initialize cargo project named libra without VCS support",
                                "kind": "implementation",
                                "acceptanceCriteria": [
                                    "Cargo.toml exists with package.name = 'libra'",
                                    "src/main.rs exists"
                                ]
                            },
                            {
                                "title": "Verify formatting, clippy, and tests pass",
                                "kind": "implementation",
                                "acceptanceCriteria": [
                                    "cargo +nightly fmt --all --check exits with code 0",
                                    "cargo test --all exits with code 0"
                                ]
                            }
                        ],
                        "riskProfile": "low",
                        "title": "Initialize libra CLI project with code, cloud, backup subcommands"
                    }
                }
            })
            .to_string(),
        )
        .expect("kimi-style draft should be normalized");

        assert_eq!(
            args.draft.intent.summary,
            "Initialize libra CLI project with code, cloud, backup subcommands"
        );
        assert_eq!(
            args.draft.intent.problem_statement,
            "Create a new Rust project named libra using cargo without VCS support. Implement three CLI subcommands (code, cloud, backup) that echo their names. Ensure the codebase passes nightly formatting, clippy with zero warnings, and all tests."
        );
        assert_eq!(args.draft.intent.in_scope, vec![".".to_string()]);
        assert!(args.draft.intent.out_of_scope.is_empty());
        assert_eq!(
            args.draft.risk.level,
            Some(crate::internal::ai::intentspec::types::RiskLevel::Low)
        );
        assert!(
            args.draft.risk.rationale.contains("low"),
            "{}",
            args.draft.risk.rationale
        );
        assert!(
            args.draft
                .acceptance
                .success_criteria
                .contains(&"Cargo.toml exists with package.name = 'libra'".to_string())
        );
        assert!(
            args.draft
                .acceptance
                .success_criteria
                .contains(&"cargo +nightly fmt --all --check exits with code 0".to_string())
        );
    }

    #[test]
    fn test_kimi_style_root_fields_are_hoisted_into_intent() {
        let args = parse_submit_intent_draft_arguments(
            &serde_json::json!({
                "draft": {
                    "intent": {
                        "changeType": "feature",
                        "objectives": [
                            {
                                "title": "Implement echo stubs for code, cloud, and backup commands",
                                "kind": "implementation"
                            }
                        ],
                        "riskProfile": "medium",
                        "title": "Initialize libra CLI project with code, cloud, backup subcommands"
                    },
                    "summary": "Initialize a new Rust CLI project named libra without VCS, add three subcommands (code, cloud, backup) with echo stubs, and ensure the project passes nightly formatting, clippy with zero warnings, and all tests.",
                    "problemStatement": "The workspace is empty and needs a new Rust CLI project named libra that supports three subcommands (code, cloud, backup) with minimal echo implementations, passing all formatting, clippy, and test checks.",
                    "inScope": ["."],
                    "outOfScope": ["vendor/"]
                }
            })
            .to_string(),
        )
        .expect("misplaced root fields should be normalized");

        assert_eq!(
            args.draft.intent.summary,
            "Initialize a new Rust CLI project named libra without VCS, add three subcommands (code, cloud, backup) with echo stubs, and ensure the project passes nightly formatting, clippy with zero warnings, and all tests."
        );
        assert_eq!(
            args.draft.intent.problem_statement,
            "The workspace is empty and needs a new Rust CLI project named libra that supports three subcommands (code, cloud, backup) with minimal echo implementations, passing all formatting, clippy, and test checks."
        );
        assert_eq!(args.draft.intent.in_scope, vec![".".to_string()]);
        assert_eq!(args.draft.intent.out_of_scope, vec!["vendor/".to_string()]);
    }

    #[test]
    fn test_kimi_style_flattened_draft_without_intent_is_accepted() {
        let args = parse_submit_intent_draft_arguments(
            &serde_json::json!({
                "draft": {
                    "changeType": "feature",
                    "summary": "Initialize a new Rust CLI project named libra without VCS support.",
                    "problemStatement": "The workspace is empty and needs a new Rust CLI project.",
                    "objectives": [
                        {
                            "title": "Initialize cargo project named libra without VCS support",
                            "kind": "implementation",
                            "acceptanceCriteria": ["Cargo.toml exists"]
                        }
                    ],
                    "inScope": ["."],
                    "riskProfile": "high"
                }
            })
            .to_string(),
        )
        .expect("flattened kimi draft should be normalized");

        assert_eq!(
            args.draft.intent.summary,
            "Initialize a new Rust CLI project named libra without VCS support."
        );
        assert_eq!(
            args.draft.intent.problem_statement,
            "The workspace is empty and needs a new Rust CLI project."
        );
        assert_eq!(
            args.draft.intent.change_type,
            crate::internal::ai::intentspec::types::ChangeType::Feature
        );
        assert_eq!(args.draft.intent.in_scope, vec![".".to_string()]);
        assert_eq!(
            args.draft.risk.level,
            Some(crate::internal::ai::intentspec::types::RiskLevel::High)
        );
        assert!(
            args.draft
                .acceptance
                .success_criteria
                .contains(&"Cargo.toml exists".to_string())
        );
    }

    #[test]
    fn test_kimi_style_plan_verification_checks_become_success_criteria() {
        let args = parse_submit_intent_draft_arguments(
            &serde_json::json!({
                "draft": {
                    "intent": {
                        "changeType": "feature",
                        "description": "Initialize a Rust project named libra using cargo without VCS support and implement three echo subcommands.",
                        "objectives": [
                            {
                                "kind": "implementation",
                                "title": "Initialize cargo project 'libra' with --no-vcs flag"
                            }
                        ],
                        "title": "Initialize libra project with code, cloud, and backup echo subcommands"
                    },
                    "plan": {
                        "name": "libra-init",
                        "steps": [
                            {
                                "title": "Initialize libra project with cargo",
                                "verification": {
                                    "check": "Directory exists and contains Cargo.toml with name = libra"
                                }
                            },
                            {
                                "title": "Implement echo subcommands in main.rs",
                                "verification": {
                                    "check": "The program compiles and each subcommand echoes provided arguments."
                                }
                            }
                        ]
                    },
                    "riskProfile": "low"
                }
            })
            .to_string(),
        )
        .expect("plan verification checks should be normalized into success criteria");

        assert!(
            args.draft.acceptance.success_criteria.contains(
                &"Directory exists and contains Cargo.toml with name = libra".to_string()
            )
        );
        assert!(args.draft.acceptance.success_criteria.contains(
            &"The program compiles and each subcommand echoes provided arguments.".to_string()
        ));
    }

    #[test]
    fn test_objective_titles_fallback_when_model_omits_all_acceptance_criteria() {
        let args = parse_submit_intent_draft_arguments(
            &serde_json::json!({
                "draft": {
                    "intent": {
                        "summary": "Add CLI echo commands",
                        "problemStatement": "Need basic CLI commands for code, cloud, and backup.",
                        "changeType": "feature",
                        "objectives": [
                            {
                                "title": "Implement code command",
                                "kind": "implementation"
                            },
                            {
                                "title": "Implement cloud command",
                                "kind": "implementation"
                            }
                        ],
                        "inScope": ["src/"]
                    },
                    "risk": {
                        "rationale": "small CLI scaffold"
                    }
                }
            })
            .to_string(),
        )
        .expect("objective titles should provide a final success criteria fallback");

        assert!(
            args.draft
                .acceptance
                .success_criteria
                .contains(&"Complete objective: Implement code command".to_string())
        );
        assert!(
            args.draft
                .acceptance
                .success_criteria
                .contains(&"Complete objective: Implement cloud command".to_string())
        );
    }
}
