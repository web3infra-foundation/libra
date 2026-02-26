//! Zhipu completion model implementation.
//!
//! Zhipu exposes an OpenAI-compatible Chat Completions API, so the request and
//! response payloads (`ZhipuRequest`, `ZhipuResponse`, etc.) closely mirror the
//! OpenAI format. Messages are serialized with a `role` tag (`system`, `user`,
//! `assistant`, `tool`) and tool calling follows the same `function`-type
//! convention. This module converts between the internal libra `CompletionRequest`
//! / `CompletionResponse` types and the Zhipu-specific wire types.

use serde::{Deserialize, Serialize};

use crate::internal::ai::{
    client::{CompletionClient, Provider},
    completion::{
        AssistantContent, CompletionError, Function, Message, Text, ToolCall, UserContent,
        request::{CompletionRequest, CompletionResponse},
    },
    providers::zhipu::client::Client,
    tools::ToolDefinition,
};

/// Zhipu completion model.
#[derive(Clone, Debug)]
pub struct Model {
    client: Client,
    model: String,
}

impl Model {
    /// Creates a new Zhipu completion model.
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
// Zhipu API Types
// ================================================================

/// Zhipu chat completion request body sent to `POST /chat/completions`.
///
/// Fields mirror the OpenAI-compatible API. Optional fields are skipped during
/// serialization when they are `None` or empty so that only relevant keys
/// appear in the JSON payload.
#[derive(Debug, Serialize)]
struct ZhipuRequest {
    model: String,
    messages: Vec<ZhipuMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ZhipuToolDefinition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<ZhipuToolChoice>,
}

/// A single message in the Zhipu chat conversation.
///
/// Serialized with an internally-tagged `role` field (`system`, `user`,
/// `assistant`, `tool`). The `Assistant` variant carries an optional text
/// body and zero-or-more tool calls; the `Tool` variant represents the
/// result returned for a specific tool invocation.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
enum ZhipuMessage {
    System {
        content: String,
    },
    User {
        content: String,
    },
    Assistant {
        content: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<ZhipuToolCall>,
    },
    Tool {
        tool_call_id: String,
        name: String,
        content: String,
    },
}

/// Controls how the model selects tools.
///
/// Uses `#[serde(untagged)]` so that `Mode` variants serialize as bare strings
/// (e.g. `"auto"`) while `Function` serializes as an object with a `type` and
/// `function` key, allowing the caller to force a specific function.
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum ZhipuToolChoice {
    /// One of the predefined selection strategies (`auto`, `none`, `required`).
    Mode(ZhipuToolChoiceMode),
    /// Force the model to call a specific function by name.
    Function(ZhipuFunctionToolChoice),
}

/// Predefined tool-selection strategies understood by the Zhipu API.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ZhipuToolChoiceMode {
    Auto,
    None,
    Required,
}

/// Requests a specific function to be called by the model.
#[derive(Debug, Serialize, Deserialize)]
struct ZhipuFunctionToolChoice {
    #[serde(rename = "type")]
    r#type: String,
    function: ZhipuToolChoiceFunction,
}

/// Identifies the function to force-call by name.
#[derive(Debug, Serialize, Deserialize)]
struct ZhipuToolChoiceFunction {
    name: String,
}

/// A tool the model may invoke, currently always of type `"function"`.
#[derive(Debug, Serialize, Deserialize)]
struct ZhipuToolDefinition {
    /// Always `"function"` for now.
    r#type: String,
    function: ZhipuFunctionDefinition,
}

/// Describes a callable function: its name, human-readable description, and
/// JSON Schema for the expected parameters.
#[derive(Debug, Serialize, Deserialize)]
struct ZhipuFunctionDefinition {
    name: String,
    description: String,
    /// JSON Schema object describing the function's parameters.
    parameters: serde_json::Value,
}

/// A tool invocation emitted by the assistant in a response.
#[derive(Debug, Serialize, Deserialize)]
struct ZhipuToolCall {
    /// Provider-assigned identifier used to correlate tool results.
    id: String,
    /// Always `"function"`.
    r#type: String,
    function: ZhipuFunctionCall,
}

/// The function name and its serialized JSON arguments string as returned by the model.
#[derive(Debug, Serialize, Deserialize)]
struct ZhipuFunctionCall {
    name: String,
    /// Raw JSON string of the function arguments.
    arguments: String,
}

/// A single completion candidate returned by the API.
#[derive(Debug, Serialize, Deserialize)]
struct ZhipuChoice {
    /// Zero-based index of this choice in the response array.
    index: usize,
    /// The assistant message generated for this choice.
    message: ZhipuMessage,
    /// Reason the model stopped generating (e.g. `"stop"`, `"tool_calls"`).
    finish_reason: Option<String>,
}

/// Token usage statistics returned alongside a completion response.
#[derive(Debug, Serialize, Deserialize)]
struct ZhipuUsage {
    prompt_tokens: usize,
    completion_tokens: usize,
    total_tokens: usize,
}

/// Top-level chat completion response returned by the Zhipu API.
///
/// Mirrors the OpenAI-compatible response shape. The `choices` vector
/// typically contains a single element; only the first choice is used
/// when converting into libra's internal `CompletionResponse`.
#[derive(Debug, Serialize, Deserialize)]
pub struct ZhipuResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    choices: Vec<ZhipuChoice>,
    usage: Option<ZhipuUsage>,
}

/// Inner error body returned when the Zhipu API reports an error.
#[derive(Debug, Deserialize)]
struct ZhipuError {
    message: String,
}

/// Wrapper for a Zhipu API error response (`{ "error": { "message": "..." } }`).
#[derive(Debug, Deserialize)]
struct ZhipuErrorResponse {
    error: ZhipuError,
}

// ================================================================
// Conversions
// ================================================================

/// Converts a libra [`Message`] into a [`ZhipuMessage`].
///
/// Because the Zhipu API accepts only a single text string per message (not an
/// array of content parts), this implementation takes only the **first** content
/// item from the message's content list. Non-text content items in the first
/// position are mapped to an empty string.
///
/// Note: this `From` impl is a simplified path used by callers that only need
/// a quick single-item conversion. The full [`build_messages`] function handles
/// multi-item content, tool calls, and tool results more thoroughly.
impl From<&Message> for ZhipuMessage {
    fn from(msg: &Message) -> Self {
        match msg {
            Message::User { content } => {
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
                ZhipuMessage::User { content: text }
            }
            Message::Assistant { content, .. } => {
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
                ZhipuMessage::Assistant {
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
                ZhipuMessage::System { content: text }
            }
        }
    }
}

// ================================================================
// CompletionModel Implementation
// ================================================================

impl crate::internal::ai::completion::CompletionModel for Model {
    type Response = ZhipuResponse;

    async fn completion(
        &self,
        request: CompletionRequest,
    ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
        let tools = parse_tools(&request.tools);
        let messages = build_messages(&request)?;

        // Build request
        let zhipu_request = ZhipuRequest {
            model: self.model.clone(),
            messages,
            temperature: request.temperature,
            tool_choice: if tools.is_empty() {
                None
            } else {
                Some(ZhipuToolChoice::Mode(ZhipuToolChoiceMode::Auto))
            },
            tools,
        };

        // Send request
        let mut req_builder = self
            .client
            .http_client
            .post(format!("{}/chat/completions", self.client.base_url))
            .json(&zhipu_request);
        req_builder = self.client.provider.on_request(req_builder);

        let response = req_builder
            .send()
            .await
            .map_err(CompletionError::HttpError)?;

        let status = response.status();
        let response_text = response.text().await.map_err(CompletionError::HttpError)?;

        if !status.is_success() {
            // Try to parse error
            if let Ok(error_response) = serde_json::from_str::<ZhipuErrorResponse>(&response_text) {
                return Err(CompletionError::ProviderError(error_response.error.message));
            }
            return Err(CompletionError::ProviderError(response_text));
        }

        let zhipu_response: ZhipuResponse =
            serde_json::from_str(&response_text).map_err(CompletionError::JsonError)?;

        // Extract choice
        let choice = zhipu_response
            .choices
            .first()
            .ok_or_else(|| CompletionError::ResponseError("No choices in response".to_string()))?;

        let content = parse_choice_content(choice)?;

        Ok(CompletionResponse {
            content,
            raw_response: zhipu_response,
        })
    }
}

/// Converts libra [`ToolDefinition`]s into the Zhipu-specific
/// [`ZhipuToolDefinition`] format, setting the type to `"function"` for each.
fn parse_tools(tools: &[ToolDefinition]) -> Vec<ZhipuToolDefinition> {
    tools
        .iter()
        .map(|tool| ZhipuToolDefinition {
            r#type: "function".to_string(),
            function: ZhipuFunctionDefinition {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: tool.parameters.clone(),
            },
        })
        .collect()
}

/// Builds the full message list for a Zhipu chat completion request.
///
/// The optional preamble is prepended as a `System` message, followed by the
/// chat history. Each libra `Message` is expanded into one or more
/// `ZhipuMessage` entries:
///
/// - `User` content items are split so that text becomes `User` messages and
///   tool results become `Tool` messages (Zhipu requires them at the top level).
/// - `Assistant` messages aggregate text parts into a single optional string
///   and collect any tool calls.
/// - `System` messages concatenate all text content items with newlines.
///
/// Returns an error if an unsupported content type (e.g. images) is encountered.
fn build_messages(request: &CompletionRequest) -> Result<Vec<ZhipuMessage>, CompletionError> {
    let mut messages = Vec::new();

    if let Some(preamble) = &request.preamble {
        messages.push(ZhipuMessage::System {
            content: preamble.clone(),
        });
    }

    for msg in &request.chat_history {
        match msg {
            Message::User { content } => {
                for item in content.iter() {
                    match item {
                        UserContent::Text(t) => messages.push(ZhipuMessage::User {
                            content: t.text.clone(),
                        }),
                        UserContent::ToolResult(tool_result) => {
                            let content = serde_json::to_string(&tool_result.result)
                                .unwrap_or_else(|_| tool_result.result.to_string());
                            messages.push(ZhipuMessage::Tool {
                                tool_call_id: tool_result.id.clone(),
                                name: tool_result.name.clone(),
                                content,
                            });
                        }
                        UserContent::Image(_) => {
                            return Err(CompletionError::NotImplemented(
                                "Image content not implemented for Zhipu provider".into(),
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
                            tool_calls.push(ZhipuToolCall {
                                id: call.id.clone(),
                                r#type: "function".to_string(),
                                function: ZhipuFunctionCall {
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

                messages.push(ZhipuMessage::Assistant {
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
                messages.push(ZhipuMessage::System { content: text });
            }
        }
    }

    Ok(messages)
}

/// Extracts [`AssistantContent`] items from a single [`ZhipuChoice`].
///
/// The assistant message may contain a text body, zero or more tool calls, or
/// both. Tool call arguments arrive as a JSON string from the API and are
/// parsed back into a `serde_json::Value`; if parsing fails the raw string is
/// preserved as a JSON string value.
///
/// Returns an error if the choice's message is not the `Assistant` variant.
fn parse_choice_content(choice: &ZhipuChoice) -> Result<Vec<AssistantContent>, CompletionError> {
    match &choice.message {
        ZhipuMessage::Assistant {
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
                // Zhipu tool call arguments are a JSON string; parse if possible.
                let arguments: serde_json::Value = serde_json::from_str(&call.function.arguments)
                    .unwrap_or_else(|_| serde_json::Value::String(call.function.arguments.clone()));
                parts.push(AssistantContent::ToolCall(ToolCall {
                    id: call.id.clone(),
                    name: call.function.name.clone(),
                    function: Function {
                        name: call.function.name.clone(),
                        arguments,
                    },
                }));
            }

            Ok(parts)
        }
        _ => Err(CompletionError::ResponseError(
            "Unexpected non-assistant message in Zhipu response".to_string(),
        )),
    }
}

/// Serializes tool-call arguments into a JSON string suitable for the Zhipu API.
///
/// If the value is already a `String` that contains valid JSON, it is returned
/// as-is (avoiding double-encoding). Otherwise the value is serialized with
/// `serde_json::to_string` / `Value::to_string`.
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

impl CompletionClient for Client {
    type Model = Model;

    fn completion_model(&self, model: impl Into<String>) -> Self::Model {
        Model::new(self.clone(), model)
    }
}

// Type alias for backwards compatibility
pub type CompletionModel = Model;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zhipu_request_serialization() {
        let request = ZhipuRequest {
            model: "glm-5".to_string(),
            messages: vec![
                ZhipuMessage::System {
                    content: "You are a helpful assistant.".to_string(),
                },
                ZhipuMessage::User {
                    content: "Hello!".to_string(),
                },
            ],
            temperature: Some(0.7),
            tools: Vec::new(),
            tool_choice: None,
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"model\":\"glm-5\""));
        assert!(json.contains("\"temperature\":0.7"));
    }

    #[test]
    fn test_zhipu_tool_choice_serialization() {
        let request = ZhipuRequest {
            model: "glm-5".to_string(),
            messages: vec![ZhipuMessage::User {
                content: "hi".to_string(),
            }],
            temperature: None,
            tools: vec![ZhipuToolDefinition {
                r#type: "function".to_string(),
                function: ZhipuFunctionDefinition {
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
            tool_choice: Some(ZhipuToolChoice::Mode(ZhipuToolChoiceMode::Auto)),
        };

        let json = serde_json::to_value(request).unwrap();
        assert_eq!(json["tool_choice"], "auto");
    }

    #[test]
    fn test_zhipu_response_deserialization() {
        let json = r#"
        {
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1677652288,
            "model": "glm-5",
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

        let response: ZhipuResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.id, "chatcmpl-123");
        assert_eq!(response.model, "glm-5");
        assert_eq!(response.choices.len(), 1);
        assert_eq!(response.usage.unwrap().total_tokens, 21);
    }

    #[test]
    fn test_model_new() {
        let client = Client::with_api_key("test-key".to_string());
        let model = Model::new(client, "glm-5");
        assert_eq!(model.model_name(), "glm-5");
    }

    #[test]
    fn test_message_to_zhipu_message() {
        let user_msg = Message::user("Hello");
        let zhipu_msg: ZhipuMessage = (&user_msg).into();
        assert!(matches!(zhipu_msg, ZhipuMessage::User { .. }));

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
        let zhipu_msg: ZhipuMessage = (&assistant_msg).into();
        assert!(matches!(zhipu_msg, ZhipuMessage::Assistant { .. }));

        let system_msg = Message::System {
            content: crate::internal::ai::completion::message::OneOrMany::one(
                crate::internal::ai::completion::message::UserContent::Text(
                    crate::internal::ai::completion::message::Text {
                        text: "System prompt".to_string(),
                    },
                ),
            ),
        };
        let zhipu_msg: ZhipuMessage = (&system_msg).into();
        assert!(matches!(zhipu_msg, ZhipuMessage::System { .. }));
    }

    #[test]
    fn test_client_completion_model() {
        let client = Client::with_api_key("test-key".to_string());
        let model = client.completion_model("glm-5");
        assert_eq!(model.model_name(), "glm-5");
    }
}
