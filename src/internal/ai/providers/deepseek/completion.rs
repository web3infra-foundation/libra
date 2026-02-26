//! DeepSeek completion model implementation.
//!
//! DeepSeek exposes an OpenAI-compatible Chat Completions endpoint
//! (`/chat/completions`), so the request and response wire types in this
//! module closely mirror those of the OpenAI provider. One notable
//! difference is that requests always set `stream: false` explicitly,
//! because the generic completion interface expects a single, complete
//! response rather than a stream of server-sent events.
//!
//! The main entry point is [`Model`], which implements the
//! [`CompletionModelTrait`] trait. A [`CompletionModel`] type alias is
//! also exported for backwards compatibility.

use serde::{Deserialize, Serialize};

use crate::internal::ai::{
    client::{CompletionClient, Provider},
    completion::{
        AssistantContent, CompletionError, CompletionModel as CompletionModelTrait, Message, Text,
        ToolCall, UserContent,
        request::{CompletionRequest, CompletionResponse},
    },
    providers::deepseek::client::Client,
    tools::ToolDefinition,
};

/// DeepSeek completion model.
#[derive(Clone, Debug)]
pub struct Model {
    client: Client,
    model: String,
}

impl Model {
    /// Creates a new DeepSeek completion model.
    pub fn new(client: Client, model: impl Into<String>) -> Self {
        Self {
            client,
            model: model.into(),
        }
    }

    /// Returns the model name.
    pub fn model_name(&self) -> &str {
        &self.model
    }
}

// ================================================================
// DeepSeek API Types
// ================================================================

/// Request body for the DeepSeek `/chat/completions` endpoint.
///
/// Mirrors the OpenAI chat completion request format. The `stream` field
/// is always set to `false` because this implementation uses synchronous
/// (non-streaming) completions.
#[derive(Debug, Serialize)]
struct DeepSeekRequest {
    model: String,
    messages: Vec<DeepSeekMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<DeepSeekToolDefinition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<DeepSeekToolChoice>,
    /// Always `false` -- streaming is not used by this provider.
    stream: bool,
}

/// A message in the DeepSeek chat completion conversation.
///
/// Serialized with a `role` tag (`system`, `user`, `assistant`, or `tool`)
/// to match the OpenAI-compatible wire format.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
enum DeepSeekMessage {
    /// A system-level instruction that sets the overall behaviour.
    System { content: String },
    /// A user turn containing plain text.
    User { content: String },
    /// An assistant turn that may contain text and/or tool calls.
    Assistant {
        content: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<DeepSeekToolCall>,
    },
    /// The result of a tool invocation, identified by `tool_call_id`.
    Tool {
        tool_call_id: String,
        name: String,
        content: String,
    },
}

/// Controls how the model selects which tool (if any) to call.
///
/// `Auto` lets the model decide, `None` disables tool use, `Required`
/// forces a tool call, and `Function` targets a specific function.
#[derive(Debug, Serialize, Deserialize)]
enum DeepSeekToolChoice {
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "none")]
    None,
    #[serde(rename = "required")]
    Required,
    /// Forces the model to call a specific named function.
    Function(DeepSeekFunctionToolChoice),
}

/// Wrapper used when `DeepSeekToolChoice::Function` is selected.
/// Contains `type: "function"` and the target function name.
#[derive(Debug, Serialize, Deserialize)]
struct DeepSeekFunctionToolChoice {
    #[serde(rename = "type")]
    r#type: String,
    function: DeepSeekToolChoiceFunction,
}

/// Identifies a specific function by name inside a forced tool-choice.
#[derive(Debug, Serialize, Deserialize)]
struct DeepSeekToolChoiceFunction {
    name: String,
}

/// A tool the model may invoke, always typed as `"function"`.
#[derive(Debug, Serialize, Deserialize)]
struct DeepSeekToolDefinition {
    /// Always `"function"`.
    r#type: String,
    function: DeepSeekFunctionDefinition,
}

/// Schema for a callable function exposed to the model.
#[derive(Debug, Serialize, Deserialize)]
struct DeepSeekFunctionDefinition {
    name: String,
    description: String,
    /// JSON Schema describing the function's parameters.
    parameters: serde_json::Value,
}

/// A tool call emitted by the assistant, referencing a specific function.
#[derive(Debug, Serialize, Deserialize)]
struct DeepSeekToolCall {
    /// Unique identifier for this tool call, used to correlate the result.
    id: String,
    /// Always `"function"`.
    r#type: String,
    function: DeepSeekFunctionCall,
}

/// The function name and its JSON-encoded arguments string.
#[derive(Debug, Serialize, Deserialize)]
struct DeepSeekFunctionCall {
    name: String,
    /// Arguments serialized as a JSON string (not a parsed object).
    arguments: String,
}

/// A single completion choice returned by the API.
///
/// Non-streaming responses typically contain exactly one choice at index 0.
#[derive(Debug, Serialize, Deserialize)]
struct DeepSeekChoice {
    index: usize,
    message: DeepSeekMessage,
    /// Reason the model stopped generating, e.g. `"stop"` or `"tool_calls"`.
    finish_reason: Option<String>,
}

/// Token usage statistics returned alongside the completion.
#[derive(Debug, Serialize, Deserialize)]
struct DeepSeekUsage {
    prompt_tokens: usize,
    completion_tokens: usize,
    total_tokens: usize,
}

/// Top-level response from the DeepSeek `/chat/completions` endpoint.
///
/// Public because it is exposed as the `raw_response` field on
/// [`CompletionResponse`] so callers can inspect provider-specific metadata
/// (e.g. `id`, `model`, `usage`).
#[derive(Debug, Serialize, Deserialize)]
pub struct DeepSeekResponse {
    /// Unique identifier for this completion.
    pub id: String,
    /// Object type, typically `"chat.completion"`.
    pub object: String,
    /// Unix timestamp (seconds) when the response was created.
    pub created: u64,
    /// The model that generated this response.
    pub model: String,
    choices: Vec<DeepSeekChoice>,
    usage: Option<DeepSeekUsage>,
}

/// Inner error payload returned by the DeepSeek API on failure.
#[derive(Debug, Deserialize)]
struct DeepSeekError {
    message: String,
}

/// Wrapper for the DeepSeek error response format: `{ "error": { "message": "..." } }`.
#[derive(Debug, Deserialize)]
struct DeepSeekErrorResponse {
    error: DeepSeekError,
}

// ================================================================
// Conversions
// ================================================================

/// Converts a generic [`Message`] to a [`DeepSeekMessage`].
///
/// Only the **first** content item is taken from the message; additional
/// items are silently ignored. Non-text content variants (e.g. images)
/// are converted to an empty string because the simple `From` conversion
/// cannot return an error. For full multi-item support, see
/// [`build_messages`], which is used by the `CompletionModel` implementation.
impl From<&Message> for DeepSeekMessage {
    fn from(msg: &Message) -> Self {
        match msg {
            Message::User { content } => {
                // Take only the first content item; non-text variants yield "".
                let text = content
                    .iter()
                    .next()
                    .map(|c| match c {
                        crate::internal::ai::completion::message::UserContent::Text(t) => {
                            t.text.clone()
                        }
                        _ => String::new(),
                    })
                    .unwrap_or_default();
                DeepSeekMessage::User { content: text }
            }
            Message::Assistant { content, .. } => {
                // Take only the first content item; tool calls are not carried over.
                let text = content
                    .iter()
                    .next()
                    .map(|c| match c {
                        crate::internal::ai::completion::message::AssistantContent::Text(t) => {
                            t.text.clone()
                        }
                        _ => String::new(),
                    })
                    .unwrap_or_default();
                DeepSeekMessage::Assistant {
                    // DeepSeek expects `null` (None) when the assistant only made tool calls.
                    content: if text.is_empty() { None } else { Some(text) },
                    tool_calls: Vec::new(),
                }
            }
            Message::System { content } => {
                let text = content
                    .iter()
                    .next()
                    .map(|c| match c {
                        crate::internal::ai::completion::message::UserContent::Text(t) => {
                            t.text.clone()
                        }
                        _ => String::new(),
                    })
                    .unwrap_or_default();
                DeepSeekMessage::System { content: text }
            }
        }
    }
}

// ================================================================
// CompletionModel Implementation
// ================================================================

/// Sends a non-streaming (`stream: false`) POST to the DeepSeek
/// `/chat/completions` endpoint and maps the response back to the
/// generic [`CompletionResponse`] type.
impl CompletionModelTrait for Model {
    type Response = DeepSeekResponse;

    async fn completion(
        &self,
        request: CompletionRequest,
    ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
        let tools = parse_tools(&request.tools);
        let messages = build_messages(&request)?;

        // Build the request body. `stream` is always false; see module docs.
        let deepseek_request = DeepSeekRequest {
            model: self.model.clone(),
            messages,
            temperature: request.temperature,
            // Only include `tool_choice` when tools are provided; otherwise
            // omit it entirely so the API does not complain.
            tool_choice: if tools.is_empty() {
                None
            } else {
                Some(DeepSeekToolChoice::Auto)
            },
            tools,
            stream: false,
        };

        // Send request
        let mut req_builder = self
            .client
            .http_client
            .post(format!("{}/chat/completions", self.client.base_url))
            .json(&deepseek_request);
        req_builder = self.client.provider.on_request(req_builder);

        let response = req_builder
            .send()
            .await
            .map_err(CompletionError::HttpError)?;

        let status = response.status();
        let response_text = response.text().await.map_err(CompletionError::HttpError)?;

        if !status.is_success() {
            // Try to parse error
            if let Ok(error_response) =
                serde_json::from_str::<DeepSeekErrorResponse>(&response_text)
            {
                return Err(CompletionError::ProviderError(error_response.error.message));
            }
            return Err(CompletionError::ProviderError(response_text));
        }

        let deepseek_response: DeepSeekResponse =
            serde_json::from_str(&response_text).map_err(CompletionError::JsonError)?;

        // Extract choice
        let choice = deepseek_response
            .choices
            .first()
            .ok_or_else(|| CompletionError::ResponseError("No choices in response".to_string()))?;

        let content = parse_choice_content(choice)?;

        Ok(CompletionResponse {
            content,
            raw_response: deepseek_response,
        })
    }
}

/// Converts the generic [`ToolDefinition`] slice into DeepSeek-specific
/// tool definitions. Each tool is wrapped as `type: "function"`.
fn parse_tools(tools: &[ToolDefinition]) -> Vec<DeepSeekToolDefinition> {
    tools
        .iter()
        .map(|tool| DeepSeekToolDefinition {
            r#type: "function".to_string(),
            function: DeepSeekFunctionDefinition {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: tool.parameters.clone(),
            },
        })
        .collect()
}

/// Builds the full message array for the DeepSeek API from a [`CompletionRequest`].
///
/// This function handles multi-item content correctly (unlike the simpler
/// `From<&Message>` conversion): user messages may contain interleaved text
/// and tool results, and assistant messages may contain text alongside tool
/// calls. The optional `preamble` is prepended as a system message.
///
/// # Errors
///
/// Returns [`CompletionError::NotImplemented`] if an image content item is
/// encountered, since the DeepSeek text-only API does not support images.
fn build_messages(request: &CompletionRequest) -> Result<Vec<DeepSeekMessage>, CompletionError> {
    let mut messages = Vec::new();

    if let Some(preamble) = &request.preamble {
        messages.push(DeepSeekMessage::System {
            content: preamble.clone(),
        });
    }

    for msg in &request.chat_history {
        match msg {
            Message::User { content } => {
                for item in content.iter() {
                    match item {
                        UserContent::Text(t) => messages.push(DeepSeekMessage::User {
                            content: t.text.clone(),
                        }),
                        UserContent::ToolResult(tool_result) => {
                            // Serialize the tool result value to a JSON string
                            // so it can be placed inside the `content` field.
                            let content = serde_json::to_string(&tool_result.result)
                                .unwrap_or_else(|_| tool_result.result.to_string());
                            messages.push(DeepSeekMessage::Tool {
                                tool_call_id: tool_result.id.clone(),
                                name: tool_result.name.clone(),
                                content,
                            });
                        }
                        UserContent::Image(_) => {
                            return Err(CompletionError::NotImplemented(
                                "Image content not implemented for DeepSeek provider".into(),
                            ));
                        }
                    }
                }
            }
            Message::Assistant { content, .. } => {
                let mut text_parts = Vec::new();
                let mut tool_calls = Vec::new();

                for item in content.iter() {
                    match item {
                        AssistantContent::Text(t) => {
                            if !t.text.trim().is_empty() {
                                text_parts.push(t.text.clone());
                            }
                        }
                        AssistantContent::ToolCall(call) => {
                            tool_calls.push(DeepSeekToolCall {
                                id: call.id.clone(),
                                r#type: "function".to_string(),
                                function: DeepSeekFunctionCall {
                                    name: call.function.name.clone(),
                                    arguments: tool_arguments_json(&call.function.arguments),
                                },
                            });
                        }
                    }
                }

                let text = if text_parts.is_empty() {
                    None
                } else {
                    Some(text_parts.join("\n"))
                };

                messages.push(DeepSeekMessage::Assistant {
                    content: text,
                    tool_calls,
                });
            }
            Message::System { content } => {
                let text = content
                    .iter()
                    .filter_map(|c| match c {
                        UserContent::Text(t) => Some(t.text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                messages.push(DeepSeekMessage::System { content: text });
            }
        }
    }

    Ok(messages)
}

/// Extracts [`AssistantContent`] items from a single [`DeepSeekChoice`].
///
/// The choice must contain an `Assistant` message; any other role is treated
/// as an error. Text and tool-call parts are both collected. Tool-call
/// arguments arrive as a JSON string and are parsed back into a
/// [`serde_json::Value`] so the rest of the system can work with structured
/// data.
fn parse_choice_content(choice: &DeepSeekChoice) -> Result<Vec<AssistantContent>, CompletionError> {
    match &choice.message {
        DeepSeekMessage::Assistant {
            content,
            tool_calls,
        } => {
            let mut parts = Vec::new();

            if let Some(text) = content
                && !text.trim().is_empty()
            {
                parts.push(AssistantContent::Text(Text { text: text.clone() }));
            }

            for call in tool_calls {
                // DeepSeek tool call arguments are a JSON string; parse if possible.
                let arguments: serde_json::Value = serde_json::from_str(&call.function.arguments)
                    .unwrap_or_else(|_| serde_json::Value::String(call.function.arguments.clone()));
                parts.push(AssistantContent::ToolCall(ToolCall {
                    id: call.id.clone(),
                    name: call.function.name.clone(),
                    function: crate::internal::ai::completion::Function {
                        name: call.function.name.clone(),
                        arguments,
                    },
                }));
            }

            Ok(parts)
        }
        _ => Err(CompletionError::ResponseError(
            "Unexpected non-assistant message in DeepSeek response".to_string(),
        )),
    }
}

/// Converts tool-call arguments from a [`serde_json::Value`] into the JSON
/// string format expected by the DeepSeek API.
///
/// If the value is already a `String` that contains valid JSON, it is
/// returned as-is (avoiding double-encoding). Otherwise the value is
/// serialized with `to_string()`.
fn tool_arguments_json(arguments: &serde_json::Value) -> String {
    match arguments {
        serde_json::Value::String(raw) => {
            if serde_json::from_str::<serde_json::Value>(raw).is_ok() {
                raw.clone()
            } else {
                arguments.to_string()
            }
        }
        _ => arguments.to_string(),
    }
}

// ================================================================
// CompletionClient Implementation
// ================================================================

/// Allows a [`Client`] to produce [`Model`] instances for any model name
/// string (e.g. `"deepseek-chat"`, `"deepseek-coder"`).
impl CompletionClient for Client {
    type Model = Model;

    fn completion_model(&self, model: impl Into<String>) -> Self::Model {
        Model::new(self.clone(), model)
    }
}

/// Backwards-compatible type alias so that existing code using
/// `deepseek::CompletionModel` continues to compile.
pub type CompletionModel = Model;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deepseek_request_serialization() {
        let request = DeepSeekRequest {
            model: "deepseek-chat".to_string(),
            messages: vec![
                DeepSeekMessage::System {
                    content: "You are a helpful assistant.".to_string(),
                },
                DeepSeekMessage::User {
                    content: "Hello!".to_string(),
                },
            ],
            temperature: Some(0.7),
            tools: Vec::new(),
            tool_choice: None,
            stream: false,
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"model\":\"deepseek-chat\""));
        assert!(json.contains("\"temperature\":0.7"));
        assert!(json.contains("\"stream\":false"));
    }

    #[test]
    fn test_deepseek_tool_choice_serialization() {
        let request = DeepSeekRequest {
            model: "deepseek-chat".to_string(),
            messages: vec![DeepSeekMessage::User {
                content: "hi".to_string(),
            }],
            temperature: None,
            tools: vec![DeepSeekToolDefinition {
                r#type: "function".to_string(),
                function: DeepSeekFunctionDefinition {
                    name: "read_file".to_string(),
                    description: "Read file".to_string(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "file_path": {"type": "string"}
                        },
                        "required": ["file_path"]
                    }),
                },
            }],
            tool_choice: Some(DeepSeekToolChoice::Auto),
            stream: false,
        };

        let json = serde_json::to_value(request).unwrap();
        assert_eq!(json["tool_choice"], "auto");
    }

    #[test]
    fn test_deepseek_response_deserialization() {
        let json = r#"
        {
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1677652288,
            "model": "deepseek-chat",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hello there!"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 9,
                "completion_tokens": 12,
                "total_tokens": 21
            }
        }
        "#;

        let response: DeepSeekResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.id, "chatcmpl-123");
        assert_eq!(response.model, "deepseek-chat");
        assert_eq!(response.choices.len(), 1);
        assert_eq!(response.usage.unwrap().total_tokens, 21);
    }

    #[test]
    fn test_model_new() {
        let client = Client::with_api_key("test-key".to_string());
        let model = Model::new(client, "deepseek-chat");
        assert_eq!(model.model_name(), "deepseek-chat");
    }

    #[test]
    fn test_message_to_deepseek_message() {
        let user_msg = Message::user("Hello");
        let deepseek_msg: DeepSeekMessage = (&user_msg).into();
        assert!(matches!(deepseek_msg, DeepSeekMessage::User { .. }));

        let assistant_msg = Message::Assistant {
            id: None,
            content: crate::internal::ai::completion::message::OneOrMany::one(
                crate::internal::ai::completion::message::AssistantContent::Text(
                    crate::internal::ai::completion::message::Text {
                        text: "Hi there".to_string(),
                    },
                ),
            ),
        };
        let deepseek_msg: DeepSeekMessage = (&assistant_msg).into();
        assert!(matches!(deepseek_msg, DeepSeekMessage::Assistant { .. }));

        let system_msg = Message::System {
            content: crate::internal::ai::completion::message::OneOrMany::one(
                crate::internal::ai::completion::message::UserContent::Text(
                    crate::internal::ai::completion::message::Text {
                        text: "System prompt".to_string(),
                    },
                ),
            ),
        };
        let deepseek_msg: DeepSeekMessage = (&system_msg).into();
        assert!(matches!(deepseek_msg, DeepSeekMessage::System { .. }));
    }

    #[test]
    fn test_client_completion_model() {
        let client = Client::with_api_key("test-key".to_string());
        let model = client.completion_model("deepseek-chat");
        assert_eq!(model.model_name(), "deepseek-chat");
    }
}
