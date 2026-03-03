use std::sync::Arc;

use crate::internal::ai::{
    completion::{
        Chat, CompletionError, CompletionModel, CompletionRequest, Message, Prompt,
        message::{AssistantContent, OneOrMany, ToolResult, UserContent},
    },
    tools::{ToolDefinition, ToolSet},
};

pub mod builder;
pub mod tool_loop;
pub use builder::AgentBuilder;
pub use tool_loop::{
    ToolLoopConfig, ToolLoopObserver, run_tool_loop, run_tool_loop_with_history_and_observer,
};

pub mod chat;
pub use chat::ChatAgent;

/// An AI Agent that manages interactions with a CompletionModel.
///
/// This is a **stateless** agent (also known as a Simple Agent). It handles configuration
/// (preamble, tools, temperature) and requests, but it does not maintain conversation history
/// between calls.
///
/// For a stateful agent that remembers context, use [`ChatAgent`].
///
/// The Agent is responsible for:
/// - Maintaining configuration (temperature, preamble/system prompt).
/// - Managing tools (optional).
/// - Constructing requests to the underlying model.
///
/// It implements the `Prompt` and `Chat` traits for easy interaction.
#[derive(Clone)]
pub struct Agent<M: CompletionModel> {
    /// The underlying completion model (e.g., Gemini, OpenAI).
    model: Arc<M>,
    /// System prompt or preamble to set the agent's behavior context.
    preamble: Option<String>,
    /// Sampling temperature (0.0 to 2.0). Higher values mean more creativity.
    temperature: Option<f64>,
    /// Maximum number of steps for tool execution loops. `None` means unlimited (though currently enforced to 4 by default).
    max_steps: Option<usize>,
    /// Set of tools available to the agent.
    tools: ToolSet,
}

impl<M: CompletionModel> Agent<M> {
    /// Creates a new Agent with the given model.
    ///
    /// # Arguments
    /// * `model` - The completion model instance.
    pub fn new(model: M) -> Self {
        Self {
            model: Arc::new(model),
            preamble: None,
            temperature: None,
            max_steps: Some(4),
            tools: ToolSet::default(),
        }
    }

    pub(crate) async fn run_with_history(
        &self,
        mut chat_history: Vec<Message>,
    ) -> Result<String, CompletionError> {
        let tools: Vec<ToolDefinition> = self.tools.tools.iter().map(|t| t.definition()).collect();

        let mut steps = 0usize;

        loop {
            let request = CompletionRequest {
                preamble: self.preamble.clone(),
                chat_history: chat_history.clone(),
                temperature: self.temperature,
                tools: tools.clone(),
                ..Default::default()
            };

            let response = self.model.completion(request).await?;

            let mut tool_calls = Vec::new();
            for item in &response.content {
                if let AssistantContent::ToolCall(tc) = item {
                    tool_calls.push(tc.clone());
                }
            }

            if tool_calls.is_empty() {
                let text_response = response
                    .content
                    .iter()
                    .filter_map(|c| match c {
                        AssistantContent::Text(t) => Some(t.text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");

                if text_response.is_empty() && !response.content.is_empty() {
                    // Return a more user-friendly error instead of debug format
                    return Err(CompletionError::ResponseError(
                        "Model returned non-text response (likely only thought or unsupported content)".into()
                    ));
                }

                return Ok(text_response);
            }

            steps += 1;
            if let Some(limit) = self.max_steps
                && steps > limit
            {
                return Err(CompletionError::ResponseError(format!(
                    "Tool calling exceeded max steps ({limit})",
                )));
            }

            let assistant_content = match OneOrMany::many(response.content.clone()) {
                Some(content) => content,
                None => {
                    return Err(CompletionError::ResponseError(
                        "Empty assistant content in tool call response".into(),
                    ));
                }
            };

            chat_history.push(Message::Assistant {
                id: None,
                content: assistant_content,
            });

            let mut results = Vec::new();
            for tc in tool_calls {
                let tool = self
                    .tools
                    .tools
                    .iter()
                    .find(|t| t.name() == tc.function.name)
                    .ok_or_else(|| {
                        CompletionError::RequestError(
                            std::io::Error::new(
                                std::io::ErrorKind::NotFound,
                                format!("Tool not found: {}", tc.function.name),
                            )
                            .into(),
                        )
                    })?;

                let result = tool
                    .call(tc.function.arguments.clone())
                    .map_err(CompletionError::RequestError)?;

                results.push(UserContent::ToolResult(ToolResult {
                    id: tc.id.clone(),
                    name: tc.function.name.clone(),
                    result,
                }));
            }

            let tool_result_content = match OneOrMany::many(results) {
                Some(content) => content,
                None => {
                    return Err(CompletionError::ResponseError("Empty tool results".into()));
                }
            };

            chat_history.push(Message::User {
                content: tool_result_content,
            });
        }
    }
}

impl<M: CompletionModel> Prompt for Agent<M> {
    async fn prompt(&self, prompt: impl Into<Message> + Send) -> Result<String, CompletionError> {
        let msg = prompt.into();
        self.run_with_history(vec![msg]).await
    }
}

impl<M: CompletionModel> Chat for Agent<M> {
    async fn chat(
        &self,
        prompt: impl Into<Message> + Send,
        mut chat_history: Vec<Message>,
    ) -> Result<String, CompletionError> {
        let msg = prompt.into();
        chat_history.push(msg);
        self.run_with_history(chat_history).await
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::AgentBuilder;
    use crate::internal::ai::{
        completion::{
            CompletionError, CompletionModel, CompletionRequest, CompletionResponse, Message,
            Prompt,
            message::{AssistantContent, Function, Text, ToolCall, UserContent},
        },
        tools::{Tool, ToolDefinition, ToolSet},
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

    struct MockTool;

    impl Tool for MockTool {
        fn name(&self) -> String {
            "mock_tool".to_string()
        }

        fn description(&self) -> String {
            "Mock tool".to_string()
        }

        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: self.name(),
                description: self.description(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "value": { "type": "number" }
                    }
                }),
            }
        }

        fn call(
            &self,
            _args: serde_json::Value,
        ) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
            Ok(json!({"ok": true}))
        }
    }

    #[tokio::test]
    async fn test_tool_call_loop_executes_tool() {
        let mut tool_set = ToolSet::default();
        tool_set.tools.push(std::sync::Arc::new(MockTool));

        let agent = AgentBuilder::new(MockModel).tools(tool_set).build();
        let response = Prompt::prompt(&agent, "hi").await.unwrap();

        assert_eq!(response, "done");
    }

    #[tokio::test]
    async fn test_max_steps_allows_exact_tool_call_count() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };

        #[derive(Clone)]
        struct AlwaysToolCallModel {
            completions: Arc<AtomicUsize>,
        }

        impl CompletionModel for AlwaysToolCallModel {
            type Response = ();

            async fn completion(
                &self,
                _request: CompletionRequest,
            ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
                self.completions.fetch_add(1, Ordering::SeqCst);

                Ok(CompletionResponse {
                    content: vec![AssistantContent::ToolCall(ToolCall {
                        id: "call".to_string(),
                        name: "always_tool".to_string(),
                        function: Function {
                            name: "always_tool".to_string(),
                            arguments: json!({}),
                        },
                    })],
                    raw_response: (),
                })
            }
        }

        #[derive(Clone)]
        struct CountingTool {
            calls: Arc<AtomicUsize>,
        }

        impl Tool for CountingTool {
            fn name(&self) -> String {
                "always_tool".to_string()
            }

            fn description(&self) -> String {
                "Always returns the same mock value".to_string()
            }

            fn definition(&self) -> ToolDefinition {
                ToolDefinition {
                    name: self.name(),
                    description: self.description(),
                    parameters: json!({ "type": "object" }),
                }
            }

            fn call(
                &self,
                _args: serde_json::Value,
            ) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok(json!({"ok": true}))
            }
        }

        let completion_calls = Arc::new(AtomicUsize::new(0));
        let tool_calls = Arc::new(AtomicUsize::new(0));

        let mut tool_set = ToolSet::default();
        tool_set.tools.push(std::sync::Arc::new(CountingTool {
            calls: tool_calls.clone(),
        }));

        let model = AlwaysToolCallModel {
            completions: completion_calls.clone(),
        };
        let agent = AgentBuilder::new(model)
            .tools(tool_set)
            .max_steps(2)
            .build();

        let err = Prompt::prompt(&agent, "hi").await.unwrap_err().to_string();

        assert_eq!(tool_calls.load(Ordering::SeqCst), 2);
        assert_eq!(completion_calls.load(Ordering::SeqCst), 3);
        assert!(err.contains("max steps"));
    }
}
