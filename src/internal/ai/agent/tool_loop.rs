use serde_json::Value;

use crate::internal::ai::{
    completion::{
        AssistantContent, CompletionError, CompletionModel, CompletionRequest, Message, OneOrMany,
        ToolResult, UserContent,
    },
    tools::{FunctionParameters, ToolDefinition, ToolInvocation, ToolOutput, ToolPayload, ToolRegistry},
};

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
    if config.max_steps == 0 {
        return Err(CompletionError::RequestError(
            "max_steps must be greater than 0".into(),
        ));
    }

    let mut history = vec![Message::user(prompt.into())];
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
            let assistant_content = OneOrMany::many(response.content.clone()).ok_or_else(|| {
                CompletionError::ResponseError("Empty assistant content in tool call response".to_string())
            })?;
            history.push(Message::Assistant {
                id: None,
                content: assistant_content,
            });

            for call in tool_calls {
                let invocation = ToolInvocation::new(
                    call.id.clone(),
                    call.function.name.clone(),
                    ToolPayload::Function {
                        arguments: tool_arguments_json(&call.function.arguments),
                    },
                    registry.working_dir().to_path_buf(),
                );

                let result = match registry.dispatch(invocation).await {
                    Ok(output) => output.into_response(),
                    Err(err) => ToolOutput::failure(format!(
                        "Tool '{}' failed: {}",
                        call.function.name, err
                    ))
                    .into_response(),
                };

                history.push(Message::User {
                    content: OneOrMany::One(UserContent::ToolResult(ToolResult {
                        id: call.id,
                        name: call.function.name,
                        result,
                    })),
                });
            }

            continue;
        }

        let choice = text_parts.join("\n");
        if !choice.trim().is_empty() {
            return Ok(choice);
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
