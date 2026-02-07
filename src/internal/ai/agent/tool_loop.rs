use serde_json::Value;

use crate::internal::ai::{
    completion::{
        AssistantContent, CompletionError, CompletionModel, CompletionRequest, Message, OneOrMany,
        ToolCall, ToolResult, UserContent,
    },
    tools::{ToolInvocation, ToolOutput, ToolPayload, ToolRegistry},
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

    for _ in 0..config.max_steps {
        let request = CompletionRequest {
            preamble: config.preamble.clone(),
            chat_history: history.clone(),
            temperature: config.temperature,
            tools: registry.tool_specs_json(),
            ..Default::default()
        };

        let response = model.completion(request).await?;

        if let Some(message) = response.message.clone() {
            history.push(message.clone());

            let tool_calls = extract_tool_calls(&message);
            if !tool_calls.is_empty() {
                for call in tool_calls {
                    let invocation = ToolInvocation::new(
                        call.id.clone(),
                        call.name.clone(),
                        ToolPayload::Function {
                            arguments: tool_arguments_json(&call.arguments),
                        },
                        registry.working_dir().to_path_buf(),
                    );

                    let result = match registry.dispatch(invocation).await {
                        Ok(output) => output.into_response(),
                        Err(err) => {
                            ToolOutput::failure(format!("Tool '{}' failed: {}", call.name, err))
                                .into_response()
                        }
                    };

                    history.push(Message::User {
                        content: OneOrMany::One(UserContent::ToolResult(ToolResult {
                            id: call.id,
                            name: Some(call.name),
                            result,
                        })),
                    });
                }

                continue;
            }
        }

        if !response.choice.trim().is_empty() {
            return Ok(response.choice);
        }
    }

    Err(CompletionError::ResponseError(format!(
        "Agent reached max_steps={} without producing a final text response",
        config.max_steps
    )))
}

fn extract_tool_calls(message: &Message) -> Vec<ToolCall> {
    let Message::Assistant { content, .. } = message else {
        return Vec::new();
    };

    content
        .iter()
        .filter_map(|item| {
            if let AssistantContent::ToolCall(call) = item {
                Some(call.clone())
            } else {
                None
            }
        })
        .collect()
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
