//! Handler for the `request_user_input` tool.

use async_trait::async_trait;
use tokio::sync::mpsc::UnboundedSender;

use super::parse_arguments;
use crate::internal::ai::tools::{
    ToolResult,
    context::{
        RequestUserInputArgs, ToolInvocation, ToolKind, ToolOutput, ToolPayload, UserInputRequest,
        UserInputResponse,
    },
    error::ToolError,
    registry::ToolHandler,
    spec::ToolSpec,
};

/// Blocking handler that presents questions to the user and awaits their response.
///
/// Communication with the TUI happens through an unbounded channel:
///
/// 1. The handler creates a `oneshot` channel for the response.
/// 2. It sends a `UserInputRequest` (questions + response sender) to the TUI
///    via the `request_tx` channel.
/// 3. It awaits the response on the `oneshot` receiver.
/// 4. The user's answers are returned as JSON to the model.
pub struct RequestUserInputHandler {
    request_tx: UnboundedSender<UserInputRequest>,
}

impl RequestUserInputHandler {
    /// Create a new handler wired to the given request channel.
    pub fn new(request_tx: UnboundedSender<UserInputRequest>) -> Self {
        Self { request_tx }
    }
}

#[async_trait]
impl ToolHandler for RequestUserInputHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> ToolResult<ToolOutput> {
        let arguments = match invocation.payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(ToolError::IncompatiblePayload(
                    "request_user_input requires Function payload".into(),
                ));
            }
        };

        let args: RequestUserInputArgs = parse_arguments(&arguments)?;

        if args.questions.is_empty() {
            return Err(ToolError::InvalidArguments(
                "At least one question is required".into(),
            ));
        }

        // Create a oneshot channel for the response.
        let (response_tx, response_rx) = tokio::sync::oneshot::channel::<UserInputResponse>();

        let request = UserInputRequest {
            call_id: invocation.call_id.clone(),
            questions: args.questions,
            response_tx,
        };

        // Send the request to the TUI.
        self.request_tx
            .send(request)
            .map_err(|_| ToolError::ExecutionFailed("TUI is not available".into()))?;

        // Block until the user responds.
        let response = response_rx
            .await
            .map_err(|_| ToolError::ExecutionFailed("User input was cancelled".into()))?;

        // Serialize the response as JSON for the model.
        let json = serde_json::to_string(&response)
            .map_err(|e| ToolError::ExecutionFailed(format!("Failed to serialize response: {e}")))?;

        Ok(ToolOutput::success(json))
    }

    fn schema(&self) -> ToolSpec {
        ToolSpec::request_user_input()
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, path::PathBuf};

    use tokio::sync::mpsc;

    use super::*;

    fn make_invocation(json: &str) -> ToolInvocation {
        ToolInvocation::new(
            "call-input-1",
            "request_user_input",
            ToolPayload::Function {
                arguments: json.to_string(),
            },
            PathBuf::from("/tmp"),
        )
    }

    #[tokio::test]
    async fn test_valid_request() {
        let (tx, mut rx) = mpsc::unbounded_channel::<UserInputRequest>();
        let handler = RequestUserInputHandler::new(tx);

        let inv = make_invocation(
            r#"{
                "questions": [{
                    "id": "q1",
                    "header": "Auth",
                    "question": "Which auth method?",
                    "options": [
                        {"label": "JWT", "description": "JSON Web Tokens"},
                        {"label": "OAuth", "description": "OAuth 2.0"}
                    ]
                }]
            }"#,
        );

        // Spawn the handler so it blocks on the oneshot.
        let handle = tokio::spawn(async move { handler.handle(inv).await });

        // Simulate TUI answering.
        let request = rx.recv().await.expect("should receive request");
        assert_eq!(request.call_id, "call-input-1");
        assert_eq!(request.questions.len(), 1);
        assert_eq!(request.questions[0].id, "q1");

        let mut answers = HashMap::new();
        answers.insert("q1".to_string(), "JWT".to_string());
        request
            .response_tx
            .send(UserInputResponse { answers })
            .unwrap();

        let result = handle.await.unwrap();
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.is_success());
        let text = output.as_text().unwrap();
        assert!(text.contains("JWT"));
    }

    #[tokio::test]
    async fn test_empty_questions() {
        let (tx, _rx) = mpsc::unbounded_channel::<UserInputRequest>();
        let handler = RequestUserInputHandler::new(tx);

        let inv = make_invocation(r#"{"questions": []}"#);
        let result = handler.handle(inv).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_cancelled_response() {
        let (tx, mut rx) = mpsc::unbounded_channel::<UserInputRequest>();
        let handler = RequestUserInputHandler::new(tx);

        let inv = make_invocation(
            r#"{
                "questions": [{
                    "id": "q1",
                    "header": "Test",
                    "question": "Pick one",
                    "options": [{"label": "A", "description": "Option A"}]
                }]
            }"#,
        );

        let handle = tokio::spawn(async move { handler.handle(inv).await });

        // Drop the response_tx to simulate cancellation.
        let request = rx.recv().await.unwrap();
        drop(request.response_tx);

        let result = handle.await.unwrap();
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_schema() {
        let (tx, _rx) = mpsc::unbounded_channel::<UserInputRequest>();
        let handler = RequestUserInputHandler::new(tx);
        let spec = handler.schema();
        assert_eq!(spec.function.name, "request_user_input");
    }
}
