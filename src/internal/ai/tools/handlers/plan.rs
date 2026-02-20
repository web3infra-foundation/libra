//! Handler for the `update_plan` tool.

use async_trait::async_trait;

use super::parse_arguments;
use crate::internal::ai::tools::{
    ToolResult,
    context::{ToolInvocation, ToolKind, ToolOutput, ToolPayload, UpdatePlanArgs},
    error::ToolError,
    registry::ToolHandler,
    spec::ToolSpec,
};

/// Fire-and-forget handler for plan updates.
///
/// The handler itself simply validates the arguments and returns "Plan updated".
/// The TUI intercepts the `ToolCallBegin` event and renders a specialised
/// `PlanUpdateHistoryCell` with checkbox UI.
pub struct PlanHandler;

#[async_trait]
impl ToolHandler for PlanHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> ToolResult<ToolOutput> {
        let arguments = match invocation.payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(ToolError::IncompatiblePayload(
                    "update_plan requires Function payload".into(),
                ));
            }
        };

        // Validate that the arguments parse correctly.
        let _args: UpdatePlanArgs = parse_arguments(&arguments)?;

        Ok(ToolOutput::success("Plan updated"))
    }

    fn schema(&self) -> ToolSpec {
        ToolSpec::update_plan()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn make_invocation(json: &str) -> ToolInvocation {
        ToolInvocation::new(
            "call-plan-1",
            "update_plan",
            ToolPayload::Function {
                arguments: json.to_string(),
            },
            PathBuf::from("/tmp"),
        )
    }

    #[tokio::test]
    async fn test_valid_plan() {
        let handler = PlanHandler;
        let inv = make_invocation(
            r#"{
                "explanation": "Starting implementation",
                "plan": [
                    {"step": "Parse arguments", "status": "completed"},
                    {"step": "Update tests", "status": "in_progress"},
                    {"step": "Document changes", "status": "pending"}
                ]
            }"#,
        );
        let result = handler.handle(inv).await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.is_success());
        assert_eq!(output.as_text(), Some("Plan updated"));
    }

    #[tokio::test]
    async fn test_plan_without_explanation() {
        let handler = PlanHandler;
        let inv = make_invocation(
            r#"{
                "plan": [
                    {"step": "Do something", "status": "pending"}
                ]
            }"#,
        );
        let result = handler.handle(inv).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_invalid_json() {
        let handler = PlanHandler;
        let inv = make_invocation(r#"{"plan": "not an array"}"#);
        let result = handler.handle(inv).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_schema() {
        let handler = PlanHandler;
        let spec = handler.schema();
        assert_eq!(spec.function.name, "update_plan");
    }
}
