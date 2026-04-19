//! Handler for the `submit_intent_draft` tool.

use async_trait::async_trait;
use serde_json::Value;

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
    let value = unwrap_json_string_value(value.clone())?;
    let value = normalize_submit_intent_draft_value(value);

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
