//! OpenAI Chat Completions API implementation.
//!
//! This module translates libra's provider-agnostic [`CompletionRequest`] into the
//! OpenAI-specific wire format and sends it as a `POST /chat/completions` request.
//! The response is then mapped back into the generic [`CompletionResponse`].
//!
//! ## Wire format overview
//!
//! OpenAI expects a JSON body with `model`, `messages`, optional `temperature`,
//! and optional `tools` / `tool_choice` fields. Messages are tagged by `role`
//! (`system`, `user`, `assistant`, `tool`). Tool calls are returned inside the
//! assistant message and subsequent tool results must carry the matching
//! `tool_call_id`.
//!
//! The private helper functions in this module handle the conversions:
//! - [`parse_tools`] -- generic tool definitions to OpenAI function-calling format.
//! - [`build_messages`] -- [`CompletionRequest`] to the OpenAI message list.
//! - [`parse_choice_content`] -- response choice to [`AssistantContent`] items.
//! - [`tool_arguments_json`] -- ensures tool arguments are a JSON string.

use serde::{Deserialize, Serialize};

use crate::internal::ai::{
    client::{CompletionClient, Provider},
    completion::{
        AssistantContent, CompletionError, CompletionModel as CompletionModelTrait, Function,
        Message, Text, ToolCall, UserContent,
        request::{CompletionRequest, CompletionResponse},
    },
    providers::openai::client::Client,
    tools::ToolDefinition,
};

/// OpenAI completion model.
#[derive(Clone, Debug)]
pub struct Model {
    client: Client,
    model: String,
}

impl Model {
    /// Creates a new OpenAI completion model.
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
// OpenAI API Types
// ================================================================

/// Request body for the OpenAI `POST /chat/completions` endpoint.
///
/// Serialized directly to JSON and sent in the HTTP request body.
/// Optional fields (`temperature`, `tools`, `tool_choice`) are omitted from
/// the payload when empty or `None` via `skip_serializing_if`.
#[derive(Debug, Serialize)]
struct OpenAIRequest {
    /// The model ID to use (e.g. `"gpt-4o"`, `"gpt-4o-mini"`).
    model: String,
    /// Ordered list of conversation messages (system, user, assistant, tool).
    messages: Vec<OpenAIMessage>,
    /// Sampling temperature (0.0 -- 2.0). Omitted to use the server default.
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    /// Tool (function) definitions available to the model. Omitted when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OpenAIToolDefinition>,
    /// Controls how the model selects tools. Set to `auto` when tools are
    /// present, omitted otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<OpenAIToolChoice>,
}

/// A message in the OpenAI chat conversation, tagged by `role`.
///
/// Serde serializes each variant with a `"role"` discriminator (`"system"`,
/// `"user"`, `"assistant"`, or `"tool"`) matching the OpenAI API schema.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
enum OpenAIMessage {
    /// A system-level instruction (preamble / system prompt).
    System { content: String },
    /// A user message containing plain text.
    User { content: String },
    /// An assistant response, which may include both text and tool calls.
    Assistant {
        content: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<OpenAIToolCall>,
    },
    /// The result of a tool invocation, linked back to the assistant's
    /// `tool_call_id`.
    Tool {
        tool_call_id: String,
        name: String,
        content: String,
    },
}

/// Specifies how the model should choose tools.
///
/// Uses `#[serde(untagged)]` so that the simple string modes (`"auto"`,
/// `"none"`, `"required"`) serialize as bare strings, while a forced-function
/// choice serializes as a `{ "type": "function", "function": { "name": ... } }`
/// object.
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum OpenAIToolChoice {
    /// One of the predefined string modes (`auto`, `none`, `required`).
    Mode(OpenAIToolChoiceMode),
    /// Forces the model to call a specific function by name.
    Function(OpenAIFunctionToolChoice),
}

/// Simple string-based tool choice modes.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OpenAIToolChoiceMode {
    /// Let the model decide whether to call a tool.
    Auto,
    /// Prevent the model from calling any tools.
    None,
    /// Require the model to call at least one tool.
    Required,
}

/// Forces the model to call a specific function.
#[derive(Debug, Serialize, Deserialize)]
struct OpenAIFunctionToolChoice {
    /// Always `"function"`.
    #[serde(rename = "type")]
    r#type: String,
    function: OpenAIToolChoiceFunction,
}

/// Identifies the specific function to force-call.
#[derive(Debug, Serialize, Deserialize)]
struct OpenAIToolChoiceFunction {
    name: String,
}

/// A tool definition sent in the request body, describing a callable function.
///
/// The `type` field is always `"function"` in the current API version.
#[derive(Debug, Serialize, Deserialize)]
struct OpenAIToolDefinition {
    /// Always `"function"`.
    r#type: String,
    /// The function metadata (name, description, JSON Schema parameters).
    function: OpenAIFunctionDefinition,
}

/// Metadata for a single callable function exposed as a tool.
#[derive(Debug, Serialize, Deserialize)]
struct OpenAIFunctionDefinition {
    /// Unique function name the model can reference when making a tool call.
    name: String,
    /// Human-readable description shown to the model for tool selection.
    description: String,
    /// JSON Schema describing the function's expected arguments.
    parameters: serde_json::Value,
}

/// A tool call emitted by the assistant in a response message.
#[derive(Debug, Serialize, Deserialize)]
struct OpenAIToolCall {
    /// Unique identifier for this tool call, used to match tool results.
    id: String,
    /// Always `"function"`.
    r#type: String,
    /// The function name and its stringified JSON arguments.
    function: OpenAIFunctionCall,
}

/// The function invocation details within a tool call.
#[derive(Debug, Serialize, Deserialize)]
struct OpenAIFunctionCall {
    /// Name of the function to invoke.
    name: String,
    /// Arguments as a JSON-encoded string (OpenAI always returns stringified JSON).
    arguments: String,
}

/// A single completion choice from the response.
///
/// The API may return multiple choices when `n > 1`; we always use the first.
#[derive(Debug, Serialize, Deserialize)]
struct OpenAIChoice {
    /// Zero-based index of this choice in the `choices` array.
    index: usize,
    /// The assistant message for this choice.
    message: OpenAIMessage,
    /// Reason the model stopped generating (`"stop"`, `"tool_calls"`, etc.).
    finish_reason: Option<String>,
}

/// Token usage statistics returned alongside the completion.
#[derive(Debug, Serialize, Deserialize)]
struct OpenAIUsage {
    /// Number of tokens in the prompt (system + user + history).
    prompt_tokens: usize,
    /// Number of tokens generated by the model.
    completion_tokens: usize,
    /// Sum of `prompt_tokens` and `completion_tokens`.
    total_tokens: usize,
}

/// Top-level response from the `POST /chat/completions` endpoint.
#[derive(Debug, Serialize, Deserialize)]
pub struct OpenAIResponse {
    /// Unique identifier for this completion (e.g. `"chatcmpl-abc123"`).
    pub id: String,
    /// Object type, always `"chat.completion"`.
    pub object: String,
    /// Unix timestamp (seconds) when the completion was created.
    pub created: u64,
    /// The model that produced this completion.
    pub model: String,
    /// List of generated choices (typically one).
    choices: Vec<OpenAIChoice>,
    /// Token usage statistics, if available.
    usage: Option<OpenAIUsage>,
}

/// Inner error object from the OpenAI API.
#[derive(Debug, Deserialize)]
struct OpenAIError {
    /// Human-readable error message from the API.
    message: String,
}

/// Wrapper for the `{ "error": { ... } }` JSON shape returned on API errors.
#[derive(Debug, Deserialize)]
struct OpenAIErrorResponse {
    error: OpenAIError,
}

// ================================================================
// Conversions
// ================================================================

/// Simple one-to-one conversion from a generic [`Message`] to [`OpenAIMessage`].
///
/// This is a convenience conversion used when individual messages need to be
/// mapped without the richer per-content-item handling that [`build_messages`]
/// provides. Note: only the **first** content item is extracted; multi-part
/// content is not fully supported here.
impl From<&Message> for OpenAIMessage {
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
                OpenAIMessage::User { content: text }
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
                OpenAIMessage::Assistant {
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
                OpenAIMessage::System { content: text }
            }
        }
    }
}

// ================================================================
// CompletionModel Implementation
// ================================================================

/// Core implementation that translates a generic [`CompletionRequest`] into an
/// OpenAI-specific HTTP request, sends it, and maps the response back.
impl CompletionModelTrait for Model {
    type Response = OpenAIResponse;

    async fn completion(
        &self,
        request: CompletionRequest,
    ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
        let tools = parse_tools(&request.tools);
        let messages = build_messages(&request)?;

        // Build request
        let openai_request = OpenAIRequest {
            model: self.model.clone(),
            messages,
            temperature: request.temperature,
            tool_choice: if tools.is_empty() {
                None
            } else {
                Some(OpenAIToolChoice::Mode(OpenAIToolChoiceMode::Auto))
            },
            tools,
        };

        // Send request
        let mut req_builder = self
            .client
            .http_client
            .post(format!("{}/chat/completions", self.client.base_url))
            .json(&openai_request);
        req_builder = self.client.provider.on_request(req_builder);

        let response = req_builder
            .send()
            .await
            .map_err(CompletionError::HttpError)?;

        let status = response.status();
        let response_text = response.text().await.map_err(CompletionError::HttpError)?;

        if !status.is_success() {
            // Try to parse error
            if let Ok(error_response) = serde_json::from_str::<OpenAIErrorResponse>(&response_text)
            {
                return Err(CompletionError::ProviderError(error_response.error.message));
            }
            return Err(CompletionError::ProviderError(response_text));
        }

        let openai_response: OpenAIResponse =
            serde_json::from_str(&response_text).map_err(CompletionError::JsonError)?;

        // Extract choice
        let choice = openai_response
            .choices
            .first()
            .ok_or_else(|| CompletionError::ResponseError("No choices in response".to_string()))?;

        let content = parse_choice_content(choice)?;

        Ok(CompletionResponse {
            content,
            raw_response: openai_response,
        })
    }
}

/// Converts generic [`ToolDefinition`]s into the OpenAI function-calling format.
///
/// Each tool is wrapped in an [`OpenAIToolDefinition`] with `type: "function"` and
/// its name, description, and JSON Schema parameters copied over verbatim.
fn parse_tools(tools: &[ToolDefinition]) -> Vec<OpenAIToolDefinition> {
    tools
        .iter()
        .map(|tool| OpenAIToolDefinition {
            r#type: "function".to_string(),
            function: OpenAIFunctionDefinition {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: tool.parameters.clone(),
            },
        })
        .collect()
}

/// Builds the ordered list of [`OpenAIMessage`]s from a [`CompletionRequest`].
///
/// The conversion works as follows:
/// 1. If a `preamble` is present it becomes the leading `system` message.
/// 2. Each message in `chat_history` is expanded into one or more OpenAI
///    messages. User messages may contain interleaved text and tool results,
///    which are split into separate `user` and `tool` messages respectively.
///    Assistant messages collect text parts and tool calls into a single
///    `assistant` message. System messages concatenate all text content.
/// 3. Image content is not yet supported and returns an error.
fn build_messages(request: &CompletionRequest) -> Result<Vec<OpenAIMessage>, CompletionError> {
    let mut messages = Vec::new();

    if let Some(preamble) = &request.preamble {
        messages.push(OpenAIMessage::System {
            content: preamble.clone(),
        });
    }

    for msg in &request.chat_history {
        match msg {
            Message::User { content } => {
                // User content items are expanded individually: plain text
                // becomes a `user` message, while tool results become `tool`
                // messages that carry the `tool_call_id` so the API can match
                // them to the assistant's prior tool call.
                for item in content.iter() {
                    match item {
                        UserContent::Text(t) => messages.push(OpenAIMessage::User {
                            content: t.text.clone(),
                        }),
                        UserContent::ToolResult(tool_result) => {
                            // Serialize the tool result value to a JSON string;
                            // fall back to Display if serialization fails.
                            let content = serde_json::to_string(&tool_result.result)
                                .unwrap_or_else(|_| tool_result.result.to_string());
                            messages.push(OpenAIMessage::Tool {
                                tool_call_id: tool_result.id.clone(),
                                name: tool_result.name.clone(),
                                content,
                            });
                        }
                        UserContent::Image(_) => {
                            return Err(CompletionError::NotImplemented(
                                "Image content not implemented for OpenAI provider".into(),
                            ));
                        }
                    }
                }
            }
            Message::Assistant { content, .. } => {
                // Collect all text and tool call items into a single OpenAI
                // assistant message. Multiple text parts are joined with
                // newlines; empty/whitespace-only text is discarded.
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
                            tool_calls.push(OpenAIToolCall {
                                id: call.id.clone(),
                                r#type: "function".to_string(),
                                function: OpenAIFunctionCall {
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

                messages.push(OpenAIMessage::Assistant {
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
                messages.push(OpenAIMessage::System { content: text });
            }
        }
    }

    Ok(messages)
}

/// Extracts [`AssistantContent`] items (text and tool calls) from a response choice.
///
/// The function expects the choice's message to be an `Assistant` variant.
/// Non-empty text is emitted as [`AssistantContent::Text`], and each tool call
/// is parsed from its stringified JSON arguments back into a
/// [`serde_json::Value`] before being wrapped in [`AssistantContent::ToolCall`].
/// Returns an error if the message is not an assistant message.
fn parse_choice_content(choice: &OpenAIChoice) -> Result<Vec<AssistantContent>, CompletionError> {
    match &choice.message {
        OpenAIMessage::Assistant {
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
                // OpenAI returns tool call arguments as a JSON-encoded string.
                // Parse it back into a Value so downstream consumers get
                // structured data. If parsing fails (e.g. malformed JSON from
                // the model), fall back to wrapping the raw string as a Value.
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
            "Unexpected non-assistant message in OpenAI response".to_string(),
        )),
    }
}

/// Ensures tool arguments are serialized as a JSON string, which is the format
/// the OpenAI API expects for the `arguments` field of a function call.
///
/// - If `arguments` is already a [`serde_json::Value::String`] that contains
///   valid JSON, it is returned as-is (it is already stringified JSON).
/// - If it is a `String` containing non-JSON text, it is re-serialized (quoted)
///   via `to_string()` to produce valid JSON.
/// - For any other `Value` variant (object, array, etc.), `to_string()` converts
///   it into a JSON string representation.
fn tool_arguments_json(arguments: &serde_json::Value) -> String {
    match arguments {
        serde_json::Value::String(raw) => {
            // If the string is already valid JSON, pass it through directly.
            // Otherwise, serialize the Value (which wraps it in quotes).
            if serde_json::from_str::<serde_json::Value>(raw).is_ok() {
                raw.clone()
            } else {
                arguments.to_string()
            }
        }
        // Non-string values (objects, arrays, numbers, etc.) are serialized to
        // their JSON text representation.
        _ => arguments.to_string(),
    }
}

// ================================================================
// CompletionClient Implementation
// ================================================================

/// Allows an OpenAI [`Client`] to produce [`Model`] instances for any
/// supported model name (e.g. `"gpt-4o"`, `"o1-mini"`).
impl CompletionClient for Client {
    type Model = Model;

    fn completion_model(&self, model: impl Into<String>) -> Self::Model {
        Model::new(self.clone(), model)
    }
}

/// Type alias retained for backwards compatibility with older call sites.
pub type CompletionModel = Model;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_openai_request_serialization() {
        let request = OpenAIRequest {
            model: "gpt-4o".to_string(),
            messages: vec![
                OpenAIMessage::System {
                    content: "You are a helpful assistant.".to_string(),
                },
                OpenAIMessage::User {
                    content: "Hello!".to_string(),
                },
            ],
            temperature: Some(0.7),
            tools: Vec::new(),
            tool_choice: None,
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"model\":\"gpt-4o\""));
        assert!(json.contains("\"temperature\":0.7"));
    }

    #[test]
    fn test_openai_tool_choice_serialization() {
        let request = OpenAIRequest {
            model: "gpt-4o-mini".to_string(),
            messages: vec![OpenAIMessage::User {
                content: "hi".to_string(),
            }],
            temperature: None,
            tools: vec![OpenAIToolDefinition {
                r#type: "function".to_string(),
                function: OpenAIFunctionDefinition {
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
            tool_choice: Some(OpenAIToolChoice::Mode(OpenAIToolChoiceMode::Auto)),
        };

        let json = serde_json::to_value(request).unwrap();
        assert_eq!(json["tool_choice"], "auto");
    }

    #[test]
    fn test_openai_response_deserialization() {
        let json = r#"
        {
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1677652288,
            "model": "gpt-4o",
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

        let response: OpenAIResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.id, "chatcmpl-123");
        assert_eq!(response.model, "gpt-4o");
        assert_eq!(response.choices.len(), 1);
        assert_eq!(response.usage.unwrap().total_tokens, 21);
    }

    #[test]
    fn test_model_new() {
        let client = Client::with_api_key("test-key".to_string());
        let model = Model::new(client, "gpt-4o");
        assert_eq!(model.model_name(), "gpt-4o");
    }

    #[test]
    fn test_message_to_openai_message() {
        let user_msg = Message::user("Hello");
        let openai_msg: OpenAIMessage = (&user_msg).into();
        assert!(matches!(openai_msg, OpenAIMessage::User { .. }));

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
        let openai_msg: OpenAIMessage = (&assistant_msg).into();
        assert!(matches!(openai_msg, OpenAIMessage::Assistant { .. }));

        let system_msg = Message::System {
            content: crate::internal::ai::completion::message::OneOrMany::one(
                crate::internal::ai::completion::message::UserContent::Text(
                    crate::internal::ai::completion::message::Text {
                        text: "System prompt".to_string(),
                    },
                ),
            ),
        };
        let openai_msg: OpenAIMessage = (&system_msg).into();
        assert!(matches!(openai_msg, OpenAIMessage::System { .. }));
    }

    #[test]
    fn test_client_completion_model() {
        let client = Client::with_api_key("test-key".to_string());
        let model = client.completion_model("gpt-4o");
        assert_eq!(model.model_name(), "gpt-4o");
    }
}
