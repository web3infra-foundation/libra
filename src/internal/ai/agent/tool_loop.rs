use serde_json::Value;

use crate::internal::ai::{
    completion::{
        AssistantContent, CompletionError, CompletionModel, CompletionRequest, Message, OneOrMany,
        ToolResult, UserContent,
    },
    tools::{
        FunctionParameters, ToolDefinition, ToolInvocation, ToolOutput, ToolPayload, ToolRegistry,
    },
};

/// A single complete tool-loop turn result.
#[derive(Clone, Debug)]
pub struct ToolLoopTurn {
    pub final_text: String,
    pub history: Vec<Message>,
}

/// Observer hooks for tool-loop execution.
///
/// All callbacks are best-effort and must be non-panicking.
pub trait ToolLoopObserver: Send {
    fn on_assistant_step_text(&mut self, _text: &str) {}

    fn on_tool_call_begin(&mut self, _call_id: &str, _tool_name: &str, _arguments: &Value) {}

    fn on_tool_call_end(
        &mut self,
        _call_id: &str,
        _tool_name: &str,
        _result: &Result<ToolOutput, String>,
    ) {
    }
}

struct NoopObserver;

impl ToolLoopObserver for NoopObserver {}

/// Runtime configuration for iterative tool-calling execution.
#[derive(Clone, Debug)]
pub struct ToolLoopConfig {
    pub preamble: Option<String>,
    pub temperature: Option<f64>,
    pub max_steps: usize,
}

impl Default for ToolLoopConfig {
    fn default() -> Self {
        Self {
            preamble: None,
            temperature: Some(0.0),
            max_steps: 8,
        }
    }
}

/// Run a prompt through a completion model, allowing iterative tool calls.
pub async fn run_tool_loop<M: CompletionModel>(
    model: &M,
    prompt: impl Into<String>,
    registry: &ToolRegistry,
    config: ToolLoopConfig,
) -> Result<String, CompletionError> {
    let mut observer = NoopObserver;
    let turn = run_tool_loop_with_history_and_observer(
        model,
        Vec::new(),
        prompt,
        registry,
        config,
        &mut observer,
    )
    .await?;
    Ok(turn.final_text)
}

/// Run a prompt through a completion model with an existing conversation history,
/// allowing iterative tool calls and emitting observer callbacks.
pub async fn run_tool_loop_with_history_and_observer<M: CompletionModel, O: ToolLoopObserver>(
    model: &M,
    mut existing_history: Vec<Message>,
    prompt: impl Into<String>,
    registry: &ToolRegistry,
    config: ToolLoopConfig,
    observer: &mut O,
) -> Result<ToolLoopTurn, CompletionError> {
    if config.max_steps == 0 {
        return Err(CompletionError::RequestError(
            "max_steps must be greater than 0".into(),
        ));
    }

    existing_history.push(Message::user(prompt.into()));
    let mut history = existing_history;

    let tools = registry_tool_definitions(registry);

    for _ in 0..config.max_steps {
        let request = CompletionRequest {
            preamble: config.preamble.clone(),
            chat_history: history.clone(),
            temperature: config.temperature,
            tools: tools.clone(),
            ..Default::default()
        };

        let response = model.completion(request).await?;

        let mut tool_calls = Vec::new();
        let mut text_parts = Vec::new();
        for content in &response.content {
            match content {
                AssistantContent::ToolCall(call) => tool_calls.push(call.clone()),
                AssistantContent::Text(text) => {
                    if !text.text.trim().is_empty() {
                        text_parts.push(text.text.clone());
                    }
                }
            }
        }

        if !tool_calls.is_empty() {
            if !text_parts.is_empty() {
                observer.on_assistant_step_text(&text_parts.join("\n"));
            }

            let assistant_content = OneOrMany::many(response.content.clone()).ok_or_else(|| {
                CompletionError::ResponseError(
                    "Empty assistant content in tool call response".to_string(),
                )
            })?;
            history.push(Message::Assistant {
                id: None,
                content: assistant_content,
            });

            for call in tool_calls {
                observer.on_tool_call_begin(
                    &call.id,
                    &call.function.name,
                    &call.function.arguments,
                );

                let invocation = ToolInvocation::new(
                    call.id.clone(),
                    call.function.name.clone(),
                    ToolPayload::Function {
                        arguments: tool_arguments_json(&call.function.arguments),
                    },
                    registry.working_dir().to_path_buf(),
                );

                let tool_result: Result<ToolOutput, String> =
                    match registry.dispatch(invocation).await {
                        Ok(output) => Ok(output),
                        Err(err) => Err(format!("Tool '{}' failed: {}", call.function.name, err)),
                    };

                observer.on_tool_call_end(&call.id, &call.function.name, &tool_result);

                let result_json = match &tool_result {
                    Ok(output) => output.clone().into_response(),
                    Err(message) => ToolOutput::failure(message.clone()).into_response(),
                };

                history.push(Message::User {
                    content: OneOrMany::One(UserContent::ToolResult(ToolResult {
                        id: call.id,
                        name: call.function.name,
                        result: result_json,
                    })),
                });
            }

            continue;
        }

        let final_text = text_parts.join("\n");
        if !final_text.trim().is_empty() {
            let assistant_content = OneOrMany::many(response.content.clone()).ok_or_else(|| {
                CompletionError::ResponseError("Empty assistant text response".to_string())
            })?;
            history.push(Message::Assistant {
                id: None,
                content: assistant_content,
            });
            return Ok(ToolLoopTurn {
                final_text,
                history,
            });
        }

        if !response.content.is_empty() {
            return Err(CompletionError::ResponseError(
                "Model returned non-text response (likely only thought or unsupported content)"
                    .to_string(),
            ));
        }
    }

    Err(CompletionError::ResponseError(format!(
        "Agent reached max_steps={} without producing a final text response",
        config.max_steps
    )))
}

fn tool_arguments_json(arguments: &Value) -> String {
    match arguments {
        Value::String(raw) => {
            if serde_json::from_str::<Value>(raw).is_ok() {
                raw.clone()
            } else {
                arguments.to_string()
            }
        }
        _ => arguments.to_string(),
    }
}

fn registry_tool_definitions(registry: &ToolRegistry) -> Vec<ToolDefinition> {
    registry
        .tool_specs()
        .into_iter()
        .map(|spec| {
            let parameters = match spec.function.parameters {
                FunctionParameters::Empty => serde_json::json!({
                    "type": "object",
                    "properties": {}
                }),
                params => serde_json::to_value(params).unwrap_or_else(|_| {
                    serde_json::json!({
                        "type": "object",
                        "properties": {}
                    })
                }),
            };
            ToolDefinition {
                name: spec.function.name,
                description: spec.function.description,
                parameters,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use serde_json::json;
    use tempfile::TempDir;

    use super::*;
    use crate::internal::ai::{
        completion::{
            CompletionResponse,
            message::{Function, Text, ToolCall},
        },
        tools::{ToolHandler, ToolKind, ToolSpec},
    };

    #[derive(Clone)]
    struct MockModel;

    impl CompletionModel for MockModel {
        type Response = ();

        async fn completion(
            &self,
            request: CompletionRequest,
        ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
            let has_tool_result = request.chat_history.iter().any(|msg| match msg {
                Message::User { content } => content
                    .iter()
                    .any(|c| matches!(c, UserContent::ToolResult(_))),
                _ => false,
            });

            if !has_tool_result {
                return Ok(CompletionResponse {
                    content: vec![AssistantContent::ToolCall(ToolCall {
                        id: "call_1".to_string(),
                        name: "mock_tool".to_string(),
                        function: Function {
                            name: "mock_tool".to_string(),
                            arguments: json!({"value": 1}),
                        },
                    })],
                    raw_response: (),
                });
            }

            Ok(CompletionResponse {
                content: vec![AssistantContent::Text(Text {
                    text: "done".to_string(),
                })],
                raw_response: (),
            })
        }
    }

    struct MockHandler;

    #[async_trait]
    impl ToolHandler for MockHandler {
        fn kind(&self) -> ToolKind {
            ToolKind::Function
        }

        async fn handle(
            &self,
            _invocation: ToolInvocation,
        ) -> crate::internal::ai::tools::ToolResult<ToolOutput> {
            Ok(ToolOutput::success("ok"))
        }

        fn schema(&self) -> ToolSpec {
            ToolSpec::new("mock_tool", "mock tool")
        }
    }

    #[derive(Default)]
    struct RecordingObserver {
        begins: Vec<(String, String)>,
        ends: Vec<(String, String, bool)>,
    }

    impl ToolLoopObserver for RecordingObserver {
        fn on_tool_call_begin(&mut self, call_id: &str, tool_name: &str, _arguments: &Value) {
            self.begins
                .push((call_id.to_string(), tool_name.to_string()));
        }

        fn on_tool_call_end(
            &mut self,
            call_id: &str,
            tool_name: &str,
            result: &Result<ToolOutput, String>,
        ) {
            self.ends.push((
                call_id.to_string(),
                tool_name.to_string(),
                result.as_ref().is_ok_and(|o| o.is_success()),
            ));
        }
    }

    #[tokio::test]
    async fn tool_loop_emits_tool_events_and_updates_history() {
        let temp_dir = TempDir::new().unwrap();
        let mut registry = ToolRegistry::with_working_dir(temp_dir.path().to_path_buf());
        registry.register("mock_tool", Arc::new(MockHandler));

        let mut observer = RecordingObserver::default();
        let turn = run_tool_loop_with_history_and_observer(
            &MockModel,
            Vec::new(),
            "hello",
            &registry,
            ToolLoopConfig {
                preamble: None,
                temperature: Some(0.0),
                max_steps: 4,
            },
            &mut observer,
        )
        .await
        .unwrap();

        assert_eq!(turn.final_text, "done");
        assert_eq!(
            observer.begins,
            vec![("call_1".to_string(), "mock_tool".to_string())]
        );
        assert_eq!(
            observer.ends,
            vec![("call_1".to_string(), "mock_tool".to_string(), true)]
        );

        // User(prompt) + Assistant(toolcall) + User(toolresult) + Assistant(text)
        assert_eq!(turn.history.len(), 4);
        assert!(matches!(&turn.history[0], Message::User { .. }));
        assert!(matches!(&turn.history[1], Message::Assistant { .. }));
        assert!(matches!(&turn.history[2], Message::User { .. }));
        assert!(matches!(&turn.history[3], Message::Assistant { .. }));
    }
}
