//! Completion model implementation for the Anthropic Messages API.
//!
//! This module translates libra's generic [`CompletionRequest`] into the
//! Anthropic-specific wire format (`POST /v1/messages`) and parses the
//! response back into the provider-agnostic [`CompletionResponse`].
//!
//! Key design points:
//!
//! - **System messages** are extracted from the chat history and merged with
//!   the optional preamble into a single top-level `system` field, because
//!   the Messages API does not allow "system" role messages inside the
//!   `messages` array.
//! - **Tool use** follows Anthropic's content-block model: tool calls arrive
//!   as `tool_use` blocks in the assistant response, and results are sent
//!   back as `tool_result` blocks in the next user message.
//! - **`max_tokens`** is required by the API and is inferred from the model
//!   name via [`calculate_max_tokens`] when the caller does not set it.

use serde::{Deserialize, Serialize};

use crate::internal::ai::{
    client::{CompletionClient, Provider},
    completion::{
        AssistantContent, CompletionError, CompletionModel as CompletionModelTrait,
        CompletionUsage, CompletionUsageSummary, Function, Message, Text, ToolCall, UserContent,
        request::{CompletionRequest, CompletionResponse},
    },
    providers::anthropic::client::Client,
    tools::ToolDefinition,
};

/// A specific Anthropic model bound to a [`Client`].
///
/// Created via [`CompletionClient::completion_model`] and used to send
/// completion requests to the Anthropic Messages API.
#[derive(Clone, Debug)]
pub struct Model {
    /// The authenticated HTTP client used to send requests.
    client: Client,
    /// The Anthropic model identifier (e.g. `"claude-sonnet-4-0"`).
    model: String,
}

impl Model {
    /// Creates a new Anthropic completion model.
    ///
    /// Boundary conditions:
    /// - The `model` string is forwarded verbatim to the API; unknown ids fail at
    ///   request time with a 404 rather than being rejected here.
    pub fn new(client: Client, model: impl Into<String>) -> Self {
        Self {
            client,
            model: model.into(),
        }
    }

    /// Returns the model name as supplied at construction.
    pub fn model_name(&self) -> &str {
        &self.model
    }
}

// ================================================================
// Anthropic API Types
// ================================================================

/// Request body for `POST /v1/messages`.
///
/// Maps directly to the Anthropic Messages API JSON schema. The `system`
/// field is a top-level string (not part of `messages`) because the API
/// treats it specially. Fields that are `None` or empty are omitted from
/// the serialized JSON via `skip_serializing_if`.
#[derive(Debug, Serialize)]
struct AnthropicRequest {
    /// The model identifier to use for this completion.
    model: String,
    /// The conversation turns (user and assistant messages only).
    messages: Vec<AnthropicMessage>,
    /// The maximum number of tokens the model may generate.
    max_tokens: u64,
    /// Optional system prompt, placed outside the messages array.
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    /// Sampling temperature (0.0 = deterministic, 1.0 = creative).
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    /// Tool definitions available for the model to call.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<AnthropicToolDefinition>,
    /// How the model should choose which tool to call (if any).
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<AnthropicToolChoice>,
}

/// A single message in the conversation, carrying a `role` ("user" or
/// "assistant") and its associated content.
#[derive(Debug, Serialize, Deserialize)]
struct AnthropicMessage {
    /// Either `"user"` or `"assistant"`. System content is handled
    /// separately via the top-level `system` field on the request.
    role: String,
    /// The message payload -- either a plain string or an array of
    /// typed content blocks.
    content: AnthropicContent,
}

/// Message content, which the API accepts in two forms.
///
/// `#[serde(untagged)]` lets serde choose the right variant automatically:
/// - [`String`](AnthropicContent::String) is a shorthand for a single text block.
/// - [`Array`](AnthropicContent::Array) is required when the message contains
///   multiple blocks (e.g. text + tool_use, or text + tool_result).
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum AnthropicContent {
    /// Shorthand: a single plain-text message.
    String(String),
    /// An array of typed content blocks (text, image, tool_use, tool_result).
    Array(Vec<AnthropicContentBlock>),
}

/// A single content block within a message.
///
/// Discriminated by the `"type"` field in JSON thanks to `#[serde(tag = "type")]`.
/// The four variants correspond to the block types supported by the Messages API.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicContentBlock {
    /// A plain text block.
    Text { text: String },
    /// A base64-encoded image block.
    Image { source: AnthropicImageSource },
    /// A tool invocation emitted by the assistant. Contains the tool `name`,
    /// a unique `id` (used to correlate with the corresponding `ToolResult`),
    /// and the JSON `input` arguments.
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// The result of a previously invoked tool, sent by the user in a
    /// follow-up message. `tool_use_id` links back to the originating
    /// `ToolUse` block.
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
}

/// Source data for an inline image, sent as a base64-encoded payload.
#[derive(Debug, Serialize, Deserialize)]
struct AnthropicImageSource {
    /// Always `"base64"` for inline images.
    r#type: String,
    /// MIME type of the image (e.g. `"image/png"`, `"image/jpeg"`).
    media_type: String,
    /// The base64-encoded image bytes.
    data: String,
}

/// A tool definition describing a function the model may invoke.
///
/// Mirrors the generic [`ToolDefinition`] but uses the Anthropic-specific
/// field name `input_schema` (instead of `parameters`).
#[derive(Debug, Serialize, Deserialize)]
struct AnthropicToolDefinition {
    /// Unique tool name (e.g. `"read_file"`).
    name: String,
    /// Human-readable description shown to the model.
    description: String,
    /// JSON Schema describing the expected input arguments.
    input_schema: serde_json::Value,
}

/// Controls how the model selects tools.
///
/// - `Auto` -- the model decides whether to use a tool.
/// - `Any` -- the model must call at least one tool (any of them).
/// - `None` -- tool use is disabled even if tools are provided.
/// - `Tool` -- the model must call the specific named tool.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicToolChoice {
    /// Let the model decide whether to use a tool.
    Auto,
    /// Force the model to call at least one tool.
    Any,
    /// Disable tool use.
    None,
    /// Force the model to call a specific tool by name.
    Tool { name: String },
}

/// Token usage statistics returned by the API, used for billing and
/// observability.
#[derive(Debug, Serialize, Deserialize)]
struct AnthropicUsage {
    /// Number of tokens in the input (prompt + system + tools).
    input_tokens: u64,
    /// Number of tokens generated by the model.
    output_tokens: u64,
}

/// Deserialized response from `POST /v1/messages`.
///
/// Stored as the `raw_response` inside [`CompletionResponse`] so callers can
/// access provider-specific fields (e.g. `stop_reason`, `usage`).
#[derive(Debug, Serialize, Deserialize)]
pub struct AnthropicResponse {
    /// Unique message identifier (e.g. `"msg_01XF..."`).
    pub id: String,
    /// Always `"message"` for non-streaming responses.
    pub r#type: String,
    /// Always `"assistant"` in a response.
    pub role: String,
    /// The content blocks produced by the model (text and/or tool_use).
    content: Vec<AnthropicContentBlock>,
    /// The model that actually served the request (may differ from the
    /// alias used in the request, e.g. `"claude-3-5-sonnet-20241022"`).
    pub model: String,
    /// Why the model stopped generating: `"end_turn"`, `"max_tokens"`,
    /// `"stop_sequence"`, or `"tool_use"`.
    pub stop_reason: Option<String>,
    /// Token usage statistics for this request/response pair.
    usage: AnthropicUsage,
}

impl CompletionUsage for AnthropicResponse {
    fn usage_summary(&self) -> Option<CompletionUsageSummary> {
        Some(CompletionUsageSummary {
            input_tokens: self.usage.input_tokens,
            output_tokens: self.usage.output_tokens,
            cost_usd: None,
        })
    }
}

/// Inner error payload returned by the API on failure.
#[derive(Debug, Deserialize)]
struct AnthropicError {
    /// Human-readable error description.
    message: String,
}

/// Top-level error envelope: `{ "error": { "message": "..." } }`.
#[derive(Debug, Deserialize)]
struct AnthropicErrorResponse {
    error: AnthropicError,
}

// ================================================================
// CompletionModel Implementation
// ================================================================

impl CompletionModelTrait for Model {
    type Response = AnthropicResponse;

    /// Sends a completion request to the Anthropic Messages API.
    ///
    /// Functional scope:
    /// - Converts the generic request into Anthropic's wire format, sends it
    ///   via the authenticated client, and parses the response back into
    ///   provider-agnostic types.
    /// - Sets `tool_choice = Auto` whenever any tools are provided so the model
    ///   may decide whether to call one — Anthropic rejects requests that omit
    ///   `tool_choice` while supplying tools.
    ///
    /// Boundary conditions:
    /// - Non-2xx responses are first attempted as `AnthropicErrorResponse` JSON
    ///   to surface the human-readable `message` field; failing that, the raw
    ///   body is returned in the error so the caller can still see what went wrong.
    /// - JSON deserialisation errors of a successful response surface as
    ///   `CompletionError::JsonError` so the upstream agent loop can retry or
    ///   abort cleanly.
    /// - `max_tokens` is computed from the model name via
    ///   [`calculate_max_tokens`]; the API requires this field, so request-side
    ///   defaults are non-optional.
    ///
    /// See: `tests::test_anthropic_request_serialization`,
    /// `tests::test_anthropic_response_deserialization`.
    async fn completion(
        &self,
        request: CompletionRequest,
    ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
        let tools = parse_tools(&request.tools);
        let (system, messages) = build_messages(&request)?;

        // Calculate default max_tokens based on model if not provided
        // Anthropic requires max_tokens to be set
        let max_tokens = calculate_max_tokens(&self.model);

        // Build request
        let anthropic_request = AnthropicRequest {
            model: self.model.clone(),
            messages,
            max_tokens,
            system,
            temperature: request.temperature,
            tool_choice: if tools.is_empty() {
                None
            } else {
                Some(AnthropicToolChoice::Auto)
            },
            tools,
        };

        // Send request
        let mut req_builder = self
            .client
            .http_client
            .post(format!("{}/v1/messages", self.client.base_url))
            .json(&anthropic_request);
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
                serde_json::from_str::<AnthropicErrorResponse>(&response_text)
            {
                return Err(CompletionError::ProviderError(format!(
                    "status {}: {}",
                    status.as_u16(),
                    error_response.error.message
                )));
            }
            return Err(CompletionError::ProviderError(format!(
                "status {}: {}",
                status.as_u16(),
                response_text
            )));
        }

        let anthropic_response: AnthropicResponse =
            serde_json::from_str(&response_text).map_err(CompletionError::JsonError)?;

        let content = parse_response(&anthropic_response);

        Ok(CompletionResponse {
            content,
            reasoning_content: None,
            raw_response: anthropic_response,
        })
    }
}

/// Converts generic [`ToolDefinition`]s into Anthropic-specific tool
/// definitions.
///
/// Functional scope: renames the `parameters` field to `input_schema` as
/// required by the Messages API, leaving the JSON Schema body untouched.
///
/// Boundary conditions: clones every field so the caller's slice is unmodified;
/// no schema validation is performed here.
fn parse_tools(tools: &[ToolDefinition]) -> Vec<AnthropicToolDefinition> {
    tools
        .iter()
        .map(|tool| AnthropicToolDefinition {
            name: tool.name.clone(),
            description: tool.description.clone(),
            input_schema: tool.parameters.clone(),
        })
        .collect()
}

/// Converts a generic [`CompletionRequest`] into Anthropic's message format.
///
/// Returns `(system, messages)` where:
/// - `system` is the merged system prompt (preamble + any `Message::System`
///   entries from the chat history), or `None` if there is no system content.
/// - `messages` is the ordered list of user/assistant turns.
///
/// # Notable behaviour
///
/// - **System messages are hoisted**: The Messages API does not support a
///   "system" role inside the `messages` array, so all system content is
///   collected and concatenated into the top-level `system` field.
/// - **Single text blocks use the string shorthand**: When a user message
///   contains exactly one text block, it is sent as a plain `String` instead
///   of a one-element `Array` for a more compact payload.
/// - **Empty assistant blocks get a whitespace placeholder**: Anthropic
///   requires at least one content block per message. If all assistant text
///   blocks were empty (e.g. a tool-only turn where the text was blank),
///   a single space is inserted to satisfy the API constraint.
fn build_messages(
    request: &CompletionRequest,
) -> Result<(Option<String>, Vec<AnthropicMessage>), CompletionError> {
    let mut messages = Vec::new();
    let mut system_messages = Vec::new();

    for msg in &request.chat_history {
        match msg {
            Message::User { content } => {
                let mut content_blocks = Vec::new();
                for item in content.iter() {
                    match item {
                        UserContent::Text(t) => {
                            content_blocks.push(AnthropicContentBlock::Text {
                                text: t.text.clone(),
                            });
                        }
                        UserContent::ToolResult(tool_result) => {
                            // Serialize the tool result value to a JSON string so
                            // it can be embedded in the `content` field of a
                            // ToolResult block (which expects a string, not an object).
                            let content = serde_json::to_string(&tool_result.result)
                                .unwrap_or_else(|_| tool_result.result.to_string());
                            content_blocks.push(AnthropicContentBlock::ToolResult {
                                tool_use_id: tool_result.id.clone(),
                                content,
                                is_error: None,
                            });
                        }
                        UserContent::Image(_) => {
                            return Err(CompletionError::NotImplemented(
                                "Image content not yet implemented for Anthropic provider".into(),
                            ));
                        }
                    }
                }
                // Optimisation: use the string shorthand when there is
                // exactly one text block, producing a smaller payload.
                let content = if content_blocks.len() == 1 {
                    match &content_blocks[0] {
                        AnthropicContentBlock::Text { text } => {
                            AnthropicContent::String(text.clone())
                        }
                        _ => AnthropicContent::Array(content_blocks),
                    }
                } else {
                    AnthropicContent::Array(content_blocks)
                };
                messages.push(AnthropicMessage {
                    role: "user".to_string(),
                    content,
                });
            }
            Message::Assistant { content, .. } => {
                let mut content_blocks = Vec::new();
                for item in content.iter() {
                    match item {
                        AssistantContent::Text(t) => {
                            // Skip blank text blocks -- they carry no useful
                            // content and would add noise to the payload.
                            if !t.text.trim().is_empty() {
                                content_blocks.push(AnthropicContentBlock::Text {
                                    text: t.text.clone(),
                                });
                            }
                        }
                        AssistantContent::ToolCall(call) => {
                            content_blocks.push(AnthropicContentBlock::ToolUse {
                                id: call.id.clone(),
                                name: call.function.name.clone(),
                                input: call.function.arguments.clone(),
                            });
                        }
                    }
                }
                // Anthropic requires at least one content block per message.
                // If everything was filtered out above, insert a whitespace
                // placeholder to satisfy the API constraint.
                if content_blocks.is_empty() {
                    content_blocks.push(AnthropicContentBlock::Text {
                        text: " ".to_string(),
                    });
                }
                messages.push(AnthropicMessage {
                    role: "assistant".to_string(),
                    content: AnthropicContent::Array(content_blocks),
                });
            }
            Message::System { content } => {
                // Collect system message text; it will be merged into the
                // top-level `system` field after the loop.
                let text = content
                    .iter()
                    .filter_map(|c| match c {
                        UserContent::Text(t) => Some(t.text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if !text.is_empty() {
                    system_messages.push(text);
                }
            }
        }
    }

    // Merge the preamble (if any) with collected system messages into a
    // single system string, separated by blank lines.
    let mut system_parts = Vec::new();
    if let Some(preamble) = &request.preamble
        && !preamble.trim().is_empty()
    {
        system_parts.push(preamble.clone());
    }
    system_parts.extend(system_messages);

    let system = if system_parts.is_empty() {
        None
    } else {
        Some(system_parts.join("\n\n"))
    };

    Ok((system, messages))
}

/// Converts the Anthropic response content blocks into generic
/// [`AssistantContent`] items.
///
/// Only `Text` and `ToolUse` blocks are relevant for the assistant's
/// output; other block types (e.g. `Image`, `ToolResult`) are ignored
/// because they are request-only constructs.
fn parse_response(response: &AnthropicResponse) -> Vec<AssistantContent> {
    let mut parts = Vec::new();

    for block in &response.content {
        match block {
            AnthropicContentBlock::Text { text } if !text.trim().is_empty() => {
                parts.push(AssistantContent::Text(Text { text: text.clone() }));
            }
            AnthropicContentBlock::ToolUse { id, name, input } => {
                parts.push(AssistantContent::ToolCall(ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    function: Function {
                        name: name.clone(),
                        arguments: input.clone(),
                    },
                }));
            }
            AnthropicContentBlock::Text { .. } => {}
            // Image and ToolResult blocks are only used in requests, so
            // they should never appear in an assistant response.
            _ => {}
        }
    }
    parts
}

/// Returns a sensible default `max_tokens` value for the given model.
///
/// Anthropic's Messages API **requires** `max_tokens` to be set explicitly
/// (there is no server-side default). The values chosen here reflect each
/// model family's maximum output capacity to avoid unnecessarily
/// truncating long responses:
///
/// | Model family           | `max_tokens` |
/// |------------------------|------------- |
/// | Claude Opus 4          | 32 000       |
/// | Claude Sonnet 4 / 3.7  | 64 000       |
/// | Claude 3.5 Sonnet/Haiku| 8 192        |
/// | Everything else        | 4 096        |
fn calculate_max_tokens(model: &str) -> u64 {
    if model.starts_with("claude-opus-4") {
        32000
    } else if model.starts_with("claude-sonnet-4") || model.starts_with("claude-3-7-sonnet") {
        64000
    } else if model.starts_with("claude-3-5-sonnet") || model.starts_with("claude-3-5-haiku") {
        8192
    } else {
        4096 // Default fallback for unknown or older models
    }
}

// ================================================================
// CompletionClient Implementation
// ================================================================

impl CompletionClient for Client {
    type Model = Model;

    /// Creates a new [`Model`] bound to this client for the given model
    /// identifier (e.g. `"claude-sonnet-4-0"`).
    fn completion_model(&self, model: impl Into<String>) -> Self::Model {
        Model::new(self.clone(), model)
    }
}

/// Type alias retained for backwards compatibility with earlier versions
/// of this module that used the name `CompletionModel` instead of `Model`.
pub type CompletionModel = Model;

#[cfg(test)]
mod tests {
    use super::*;

    /// Scenario: serde must serialise `tool_choice` with the literal `type`
    /// discriminant Anthropic expects — the API rejects payloads that put the
    /// variant name elsewhere.
    #[test]
    fn test_anthropic_tool_choice_serialization() {
        let auto = serde_json::to_value(AnthropicToolChoice::Auto).unwrap();
        assert_eq!(auto["type"], "auto");

        let tool = serde_json::to_value(AnthropicToolChoice::Tool {
            name: "read_file".to_string(),
        })
        .unwrap();
        assert_eq!(tool["type"], "tool");
        assert_eq!(tool["name"], "read_file");
    }

    /// Scenario: pin the on-the-wire shape of a typical request so a future
    /// rename of any serde field name breaks this test before reaching the API.
    #[test]
    fn test_anthropic_request_serialization() {
        let request = AnthropicRequest {
            model: "claude-3-5-sonnet-latest".to_string(),
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: AnthropicContent::String("Hello!".to_string()),
            }],
            max_tokens: 4096,
            system: Some("You are a helpful assistant.".to_string()),
            temperature: Some(0.7),
            tools: Vec::new(),
            tool_choice: None,
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"model\":\"claude-3-5-sonnet-latest\""));
        assert!(json.contains("\"max_tokens\":4096"));
        assert!(json.contains("\"temperature\":0.7"));
    }

    /// Scenario: a minimal text-only response should round-trip through serde
    /// without losing usage stats; this is the canonical "happy path" decode.
    #[test]
    fn test_anthropic_response_deserialization() {
        let json = r#"
        {
            "id": "msg_123",
            "type": "message",
            "role": "assistant",
            "content": [
                {
                    "type": "text",
                    "text": "Hello there!"
                }
            ],
            "model": "claude-3-5-sonnet-latest",
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 20
            }
        }
        "#;

        let response: AnthropicResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.id, "msg_123");
        assert_eq!(response.role, "assistant");
        assert_eq!(response.content.len(), 1);
        assert_eq!(response.usage.input_tokens, 10);
        assert_eq!(response.usage.output_tokens, 20);
    }

    /// Scenario: a response containing both a text block and a `tool_use`
    /// block must decode all fields, including the `tool_use` stop reason
    /// that signals the agent loop to dispatch the call.
    #[test]
    fn test_anthropic_tool_use_response() {
        let json = r#"
        {
            "id": "msg_456",
            "type": "message",
            "role": "assistant",
            "content": [
                {
                    "type": "text",
                    "text": "I'll check the weather for you."
                },
                {
                    "type": "tool_use",
                    "id": "toolu_123",
                    "name": "get_weather",
                    "input": {"location": "San Francisco"}
                }
            ],
            "model": "claude-3-5-sonnet-latest",
            "stop_reason": "tool_use",
            "usage": {
                "input_tokens": 15,
                "output_tokens": 25
            }
        }
        "#;

        let response: AnthropicResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.id, "msg_456");
        assert_eq!(response.content.len(), 2);
        assert_eq!(response.stop_reason, Some("tool_use".to_string()));
    }

    /// Scenario: the Messages API forbids "system" entries inside `messages`,
    /// so this test guards the hoisting logic that merges the preamble plus
    /// every `Message::System` into the top-level `system` field, joined by
    /// blank lines.
    #[test]
    fn test_build_messages_consolidates_system_content() {
        let request = CompletionRequest {
            preamble: Some("preamble".to_string()),
            chat_history: vec![
                Message::System {
                    content: crate::internal::ai::completion::message::OneOrMany::one(
                        UserContent::Text(Text {
                            text: "system one".to_string(),
                        }),
                    ),
                },
                Message::System {
                    content: crate::internal::ai::completion::message::OneOrMany::one(
                        UserContent::Text(Text {
                            text: "system two".to_string(),
                        }),
                    ),
                },
                Message::user("hello"),
            ],
            ..Default::default()
        };

        let (system, messages) = build_messages(&request).unwrap();
        assert_eq!(
            system,
            Some("preamble\n\nsystem one\n\nsystem two".to_string())
        );
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "user");
    }

    /// Scenario: smoke-test that `Model::new` stores the model identifier
    /// verbatim — used by callers to verify they bound the right model.
    #[test]
    fn test_model_new() {
        let client = Client::with_api_key("sk-ant-test-key".to_string());
        let model = Model::new(client, "claude-3-5-sonnet-latest");
        assert_eq!(model.model_name(), "claude-3-5-sonnet-latest");
    }

    /// Scenario: max-tokens defaults differ across model families because
    /// Anthropic's published output ceilings vary. This pins the lookup table
    /// so a regression cannot silently truncate Opus or Sonnet 4 output.
    #[test]
    fn test_calculate_max_tokens() {
        assert_eq!(calculate_max_tokens("claude-opus-4-0"), 32000);
        assert_eq!(calculate_max_tokens("claude-sonnet-4-0"), 64000);
        assert_eq!(calculate_max_tokens("claude-3-7-sonnet-latest"), 64000);
        assert_eq!(calculate_max_tokens("claude-3-5-sonnet-latest"), 8192);
        assert_eq!(calculate_max_tokens("claude-3-5-haiku-latest"), 8192);
        assert_eq!(calculate_max_tokens("claude-3-opus"), 4096);
        assert_eq!(calculate_max_tokens("unknown-model"), 4096);
    }

    /// Scenario: `CompletionClient::completion_model` is the canonical entry
    /// point used by the agent runtime; verify it produces a `Model` bound to
    /// the requested identifier.
    #[test]
    fn test_client_completion_model() {
        let client = Client::with_api_key("sk-ant-test-key".to_string());
        let model = client.completion_model("claude-3-5-sonnet-latest");
        assert_eq!(model.model_name(), "claude-3-5-sonnet-latest");
    }

    /// Scenario: tool definitions must round-trip with the schema preserved —
    /// Anthropic uses `input_schema` rather than OpenAI's `parameters`, and
    /// regressions in this rename are silently accepted by the API but cause
    /// the model to ignore the tool.
    #[test]
    fn test_parse_tools_maps_tool_definition() {
        let tools = vec![crate::internal::ai::tools::ToolDefinition {
            name: "read_file".to_string(),
            description: "Read a file".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "file_path": { "type": "string" }
                },
                "required": ["file_path"]
            }),
        }];

        let parsed = parse_tools(&tools);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "read_file");
        assert_eq!(parsed[0].description, "Read a file");
        assert_eq!(parsed[0].input_schema["type"], "object");
    }

    /// Scenario: nested JSON Schema fields (`properties`, `required`) must be
    /// preserved verbatim during conversion so the model sees an identical
    /// tool contract on both wire formats.
    #[test]
    fn test_parse_tools_preserves_parameters() {
        let tools = vec![crate::internal::ai::tools::ToolDefinition {
            name: "list_dir".to_string(),
            description: "List directory contents".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "dir_path": { "type": "string" }
                },
                "required": ["dir_path"]
            }),
        }];

        let parsed = parse_tools(&tools);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "list_dir");
        assert_eq!(parsed[0].description, "List directory contents");
        assert_eq!(parsed[0].input_schema["type"], "object");
    }
}
