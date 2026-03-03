//! Handler for the `submit_intent_draft` tool.

use async_trait::async_trait;

use super::parse_arguments;
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

        let _args: SubmitIntentDraftArgs = parse_arguments(&arguments)?;
        Ok(ToolOutput::success("Intent draft submitted"))
    }

    fn schema(&self) -> ToolSpec {
        ToolSpec::submit_intent_draft()
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
                        "objectives": ["fix it"],
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
    async fn test_invalid_draft_submission() {
        let handler = SubmitIntentDraftHandler;
        let inv = make_invocation(r#"{"draft": {"intent": {}}}"#);
        let result = handler.handle(inv).await;
        assert!(result.is_err());
    }
}
