//! Kimi (Moonshot AI) completion model implementation.
//!
//! Kimi exposes an OpenAI-compatible Chat Completions endpoint, so common wire
//! types and helpers are imported from
//! [`openai_compat`](super::super::openai_compat). This file defines the
//! provider-specific request body, tool-choice shape, thinking preservation,
//! and streaming response aggregation.

use std::collections::BTreeMap;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

use crate::internal::ai::{
    client::{CompletionClient, Provider},
    completion::{
        CompletionError, CompletionModel as CompletionModelTrait, CompletionStreamEvent,
        CompletionThinking,
        request::{CompletionRequest, CompletionResponse},
    },
    providers::{
        kimi::client::Client,
        openai_compat::{
            ChatChoice, ChatErrorResponse, ChatFunctionCall, ChatMessage, ChatResponse,
            ChatToolCall, ChatToolDefinition, ChatUsage, build_messages_with_reasoning_content,
            choice_reasoning_content, parse_choice_content, parse_tools,
        },
    },
};

/// Kimi completion model bound to a specific model identifier and HTTP
/// [`Client`].
///
/// Construct via [`CompletionClient::completion_model`] (preferred) or
/// [`Model::new`].
#[derive(Clone, Debug)]
pub struct Model {
    client: Client,
    model: String,
}

impl Model {
    /// Creates a new Kimi completion model.
    ///
    /// Boundary conditions:
    /// - The `model` string is forwarded verbatim; unknown ids fail at request
    ///   time with a 404.
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

    async fn send_chat_completion_request(
        &self,
        request: &KimiRequest,
    ) -> Result<reqwest::Response, CompletionError> {
        let mut req_builder = self
            .client
            .http_client
            .post(format!("{}/chat/completions", self.client.base_url))
            .json(request);
        req_builder = self.client.provider.on_request(req_builder);

        let response = req_builder
            .send()
            .await
            .map_err(CompletionError::HttpError)?;

        let status = response.status();
        if status.is_success() {
            tracing::debug!(
                provider = "kimi",
                status = status.as_u16(),
                "Kimi HTTP request succeeded"
            );
            return Ok(response);
        }

        let response_text = response.text().await.map_err(CompletionError::HttpError)?;
        tracing::debug!(
            provider = "kimi",
            status = status.as_u16(),
            body_bytes = response_text.len(),
            "Kimi HTTP request failed"
        );
        if let Ok(error_response) = serde_json::from_str::<ChatErrorResponse>(&response_text) {
            return Err(CompletionError::ProviderError(format!(
                "status {}: {}",
                status.as_u16(),
                error_response.error.message
            )));
        }
        Err(CompletionError::ProviderError(format!(
            "status {}: {}",
            status.as_u16(),
            response_text
        )))
    }
}

// ================================================================
// Kimi-specific Request / ToolChoice Types
// ================================================================

#[derive(Debug, Serialize)]
struct KimiRequest {
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ChatToolDefinition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<KimiToolChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<KimiThinking>,
    stream: bool,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct KimiThinking {
    #[serde(rename = "type")]
    r#type: KimiThinkingType,
    #[serde(skip_serializing_if = "Option::is_none")]
    keep: Option<KimiThinkingKeep>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum KimiThinkingType {
    Enabled,
    Disabled,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum KimiThinkingKeep {
    All,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum KimiToolChoice {
    Mode(KimiToolChoiceMode),
    Function(KimiFunctionToolChoice),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum KimiToolChoiceMode {
    Auto,
    None,
    Required,
}

#[derive(Debug, Serialize, Deserialize)]
struct KimiFunctionToolChoice {
    #[serde(rename = "type")]
    r#type: String,
    function: KimiToolChoiceFunction,
}

#[derive(Debug, Serialize, Deserialize)]
struct KimiToolChoiceFunction {
    name: String,
}

fn kimi_thinking(thinking: Option<CompletionThinking>) -> Option<KimiThinking> {
    match thinking {
        Some(CompletionThinking::Disabled) => Some(KimiThinking {
            r#type: KimiThinkingType::Disabled,
            keep: None,
        }),
        Some(
            CompletionThinking::Enabled
            | CompletionThinking::Low
            | CompletionThinking::Medium
            | CompletionThinking::High
            | CompletionThinking::Auto,
        ) => Some(KimiThinking {
            r#type: KimiThinkingType::Enabled,
            keep: Some(KimiThinkingKeep::All),
        }),
        None => Some(KimiThinking {
            r#type: KimiThinkingType::Enabled,
            keep: Some(KimiThinkingKeep::All),
        }),
    }
}

#[derive(Debug, Deserialize)]
struct KimiStreamChunk {
    id: String,
    choices: Vec<KimiStreamChoice>,
    created: u64,
    model: String,
    #[serde(default)]
    usage: Option<ChatUsage>,
}

#[derive(Debug, Deserialize)]
struct KimiStreamChoice {
    index: usize,
    delta: KimiStreamDelta,
    finish_reason: Option<String>,
    #[serde(default)]
    usage: Option<ChatUsage>,
}

#[derive(Debug, Default, Deserialize)]
struct KimiStreamDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<KimiStreamToolCallDelta>,
}

#[derive(Debug, Deserialize)]
struct KimiStreamToolCallDelta {
    #[serde(default)]
    index: Option<usize>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<KimiStreamFunctionDelta>,
}

#[derive(Debug, Deserialize)]
struct KimiStreamFunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Debug, Default)]
struct KimiStreamAccumulator {
    id: Option<String>,
    created: Option<u64>,
    model: Option<String>,
    content: String,
    reasoning_content: String,
    tool_calls: BTreeMap<usize, KimiStreamToolCallBuilder>,
    finish_reason: Option<String>,
    usage: Option<ChatUsage>,
}

#[derive(Debug, Default)]
struct KimiStreamToolCallBuilder {
    id: Option<String>,
    name: String,
    arguments: String,
}

impl KimiStreamToolCallBuilder {
    fn is_complete(&self) -> bool {
        !self.name.is_empty()
            && !self.arguments.trim().is_empty()
            && serde_json::from_str::<Value>(&self.arguments).is_ok()
    }
}

impl KimiStreamAccumulator {
    fn has_salvageable_response(&self) -> bool {
        !self.has_incomplete_tool_calls()
            && (!self.content.trim().is_empty() || !self.tool_calls.is_empty())
    }

    fn has_incomplete_tool_calls(&self) -> bool {
        self.tool_calls
            .values()
            .any(|builder| !builder.is_complete())
    }

    fn has_partial_output(&self) -> bool {
        !self.content.is_empty()
            || !self.reasoning_content.is_empty()
            || !self.tool_calls.is_empty()
    }

    fn push_chunk(
        &mut self,
        chunk: KimiStreamChunk,
        stream_events: Option<&UnboundedSender<CompletionStreamEvent>>,
    ) {
        let request_id = Some(chunk.id.clone());
        self.id.get_or_insert_with(|| chunk.id.clone());
        self.created.get_or_insert(chunk.created);
        self.model.get_or_insert_with(|| chunk.model.clone());
        if chunk.usage.is_some() {
            self.usage = chunk.usage;
        }

        for choice in chunk.choices {
            if choice.finish_reason.is_some() {
                self.finish_reason = choice.finish_reason;
            }
            if choice.usage.is_some() {
                self.usage = choice.usage;
            }

            if let Some(delta) = choice.delta.content.filter(|delta| !delta.is_empty()) {
                self.content.push_str(&delta);
                if let Some(stream_events) = stream_events {
                    let _ = stream_events.send(CompletionStreamEvent::TextDelta {
                        request_id: request_id.clone(),
                        delta,
                    });
                }
            }

            if let Some(delta) = choice
                .delta
                .reasoning_content
                .filter(|delta| !delta.is_empty())
            {
                self.reasoning_content.push_str(&delta);
                if let Some(stream_events) = stream_events {
                    let _ = stream_events.send(CompletionStreamEvent::ThinkingDelta {
                        request_id: request_id.clone(),
                        delta,
                    });
                }
            }

            for tool_call in choice.delta.tool_calls {
                self.push_tool_call_delta(choice.index, tool_call, stream_events, &request_id);
            }
        }
    }

    fn push_tool_call_delta(
        &mut self,
        fallback_index: usize,
        delta: KimiStreamToolCallDelta,
        stream_events: Option<&UnboundedSender<CompletionStreamEvent>>,
        request_id: &Option<String>,
    ) {
        let index = delta.index.unwrap_or(fallback_index);
        let builder = self.tool_calls.entry(index).or_default();
        if let Some(id) = delta.id {
            builder.id = Some(id);
        }
        if let Some(function) = delta.function {
            if let Some(name) = function.name {
                builder.name.push_str(&name);
            }
            if let Some(arguments) = function.arguments {
                builder.arguments.push_str(&arguments);
            }
        }

        if builder.name.is_empty() {
            return;
        }
        let Ok(arguments) = serde_json::from_str::<Value>(&builder.arguments) else {
            return;
        };
        if let Some(stream_events) = stream_events {
            let call_id = builder
                .id
                .clone()
                .unwrap_or_else(|| format!("call_{index}"));
            let _ = stream_events.send(CompletionStreamEvent::ToolCallPreview {
                request_id: request_id.clone(),
                call_id,
                tool_name: builder.name.clone(),
                arguments,
            });
        }
    }

    fn into_response(self, fallback_model: &str) -> Result<ChatResponse, CompletionError> {
        let mut tool_calls = Vec::new();
        for (index, builder) in self.tool_calls {
            if builder.name.is_empty() {
                return Err(CompletionError::ResponseError(format!(
                    "Kimi stream returned tool call {index} without a function name"
                )));
            }

            let arguments = if builder.arguments.trim().is_empty() {
                "{}".to_string()
            } else {
                builder.arguments
            };
            tool_calls.push(ChatToolCall {
                id: builder.id.unwrap_or_else(|| format!("call_{index}")),
                r#type: "function".to_string(),
                function: ChatFunctionCall {
                    name: builder.name,
                    arguments,
                },
            });
        }

        let content = if self.content.trim().is_empty() {
            None
        } else {
            Some(self.content)
        };
        let finish_reason = self.finish_reason.or_else(|| {
            if tool_calls.is_empty() {
                Some("stop".to_string())
            } else {
                Some("tool_calls".to_string())
            }
        });

        Ok(ChatResponse {
            id: self.id.unwrap_or_else(|| "kimi-stream".to_string()),
            object: "chat.completion".to_string(),
            created: self.created.unwrap_or_default(),
            model: self.model.unwrap_or_else(|| fallback_model.to_string()),
            choices: vec![ChatChoice {
                index: 0,
                message: ChatMessage::Assistant {
                    content,
                    reasoning_content: if self.reasoning_content.trim().is_empty() {
                        None
                    } else {
                        Some(self.reasoning_content)
                    },
                    tool_calls,
                },
                finish_reason,
            }],
            usage: self.usage,
        })
    }
}

fn process_kimi_stream_line(
    line: &[u8],
    accumulator: &mut KimiStreamAccumulator,
    stream_events: Option<&UnboundedSender<CompletionStreamEvent>>,
) -> Result<bool, CompletionError> {
    let line = std::str::from_utf8(line).map_err(|error| {
        CompletionError::ResponseError(format!("invalid Kimi stream UTF-8: {error}"))
    })?;
    let line = line.trim();
    if line.is_empty() || line.starts_with(':') {
        return Ok(false);
    }
    let Some(data) = line.strip_prefix("data:") else {
        return Ok(false);
    };
    let data = data.trim();
    if data == "[DONE]" {
        return Ok(true);
    }

    let chunk: KimiStreamChunk = serde_json::from_str(data)?;
    accumulator.push_chunk(chunk, stream_events);
    Ok(false)
}

async fn read_kimi_stream_response(
    response: reqwest::Response,
    fallback_model: &str,
    stream_events: Option<&UnboundedSender<CompletionStreamEvent>>,
) -> Result<ChatResponse, CompletionError> {
    let mut accumulator = KimiStreamAccumulator::default();
    let mut pending = Vec::<u8>::new();
    let mut stream = response.bytes_stream();
    let mut done = false;

    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                if accumulator.has_salvageable_response() {
                    tracing::warn!(
                        provider = "kimi",
                        error = %error,
                        content_bytes = accumulator.content.len(),
                        reasoning_bytes = accumulator.reasoning_content.len(),
                        tool_call_fragments = accumulator.tool_calls.len(),
                        "Kimi stream ended with a body read error after a usable response; using accumulated response"
                    );
                    break;
                }

                return Err(kimi_stream_body_error(error, &accumulator));
            }
        };
        pending.extend_from_slice(&chunk);

        while let Some(newline_index) = pending.iter().position(|byte| *byte == b'\n') {
            let line = pending.drain(..=newline_index).collect::<Vec<_>>();
            done = process_kimi_stream_line(&line, &mut accumulator, stream_events)? || done;
        }
    }

    if !pending.is_empty() && !done {
        process_kimi_stream_line(&pending, &mut accumulator, stream_events)?;
    }

    tracing::debug!(
        provider = "kimi",
        done,
        content_bytes = accumulator.content.len(),
        reasoning_bytes = accumulator.reasoning_content.len(),
        tool_call_fragments = accumulator.tool_calls.len(),
        finish_reason = accumulator.finish_reason.as_deref().unwrap_or(""),
        "Kimi stream accumulated response"
    );

    accumulator.into_response(fallback_model)
}

fn kimi_stream_body_error(
    error: reqwest::Error,
    accumulator: &KimiStreamAccumulator,
) -> CompletionError {
    if accumulator.has_partial_output() {
        return CompletionError::ResponseError(format!(
            "Kimi stream ended before a usable response: {error} \
             (received {} visible bytes, {} reasoning bytes, {} tool call fragments)",
            accumulator.content.len(),
            accumulator.reasoning_content.len(),
            accumulator.tool_calls.len()
        ));
    }

    CompletionError::HttpError(error)
}

async fn read_kimi_json_response(
    response: reqwest::Response,
) -> Result<ChatResponse, CompletionError> {
    let response_text = response.text().await.map_err(CompletionError::HttpError)?;
    let response =
        serde_json::from_str::<ChatResponse>(&response_text).map_err(CompletionError::JsonError)?;
    tracing::debug!(
        provider = "kimi",
        body_bytes = response_text.len(),
        choices = response.choices.len(),
        prompt_tokens = response
            .usage
            .as_ref()
            .map(|usage| usage.prompt_tokens)
            .unwrap_or_default(),
        completion_tokens = response
            .usage
            .as_ref()
            .map(|usage| usage.completion_tokens)
            .unwrap_or_default(),
        total_tokens = response
            .usage
            .as_ref()
            .map(|usage| usage.total_tokens)
            .unwrap_or_default(),
        "Kimi JSON response decoded"
    );
    Ok(response)
}

fn should_retry_kimi_stream_without_stream(error: &CompletionError) -> bool {
    match error {
        CompletionError::HttpError(error) => error.is_body() || error.is_timeout(),
        CompletionError::ResponseError(message) => {
            message.contains("Kimi stream ended before a usable response")
        }
        CompletionError::ProviderError(_)
        | CompletionError::JsonError(_)
        | CompletionError::RequestError(_)
        | CompletionError::NotImplemented(_) => false,
    }
}

// ================================================================
// CompletionModel Implementation
// ================================================================

impl CompletionModelTrait for Model {
    type Response = ChatResponse;

    async fn completion(
        &self,
        request: CompletionRequest,
    ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
        let tools = parse_tools(&request.tools);
        let messages = build_messages_with_reasoning_content(&request)?;
        let thinking = kimi_thinking(request.thinking);
        let stream = request.stream.unwrap_or(true);
        let stream_events = request.stream_events.clone();

        let mut kimi_request = KimiRequest {
            model: self.model.clone(),
            messages,
            temperature: request.temperature,
            tool_choice: if tools.is_empty() {
                None
            } else {
                Some(KimiToolChoice::Mode(KimiToolChoiceMode::Auto))
            },
            tools,
            thinking,
            stream,
        };

        tracing::debug!(
            provider = "kimi",
            model = %kimi_request.model,
            stream = kimi_request.stream,
            messages = kimi_request.messages.len(),
            tools = kimi_request.tools.len(),
            has_temperature = kimi_request.temperature.is_some(),
            thinking = ?kimi_request.thinking.as_ref(),
            "Kimi completion request started"
        );

        let response = self.send_chat_completion_request(&kimi_request).await?;

        let kimi_response = if stream {
            match read_kimi_stream_response(response, &self.model, stream_events.as_ref()).await {
                Ok(response) => response,
                Err(error) if should_retry_kimi_stream_without_stream(&error) => {
                    tracing::warn!(
                        provider = "kimi",
                        error = %error,
                        "Kimi stream failed before a usable response; retrying once without stream"
                    );
                    kimi_request.stream = false;
                    let response = self.send_chat_completion_request(&kimi_request).await?;
                    read_kimi_json_response(response).await?
                }
                Err(error) => return Err(error),
            }
        } else {
            read_kimi_json_response(response).await?
        };

        let choice = kimi_response
            .choices
            .first()
            .ok_or_else(|| CompletionError::ResponseError("No choices in response".to_string()))?;

        let reasoning_content = choice_reasoning_content(choice);
        let content = parse_choice_content(choice)?;

        Ok(CompletionResponse {
            content,
            reasoning_content,
            raw_response: kimi_response,
        })
    }
}

impl CompletionClient for Client {
    type Model = Model;

    fn completion_model(&self, model: impl Into<String>) -> Self::Model {
        Model::new(self.clone(), model)
    }
}

/// Backwards-compatible type alias.
pub type CompletionModel = Model;

/// Type alias for the raw response type.
pub type KimiResponse = ChatResponse;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::providers::openai_compat::{ChatFunctionDefinition, ChatMessage};

    /// Scenario: pin the on-the-wire request shape for a basic chat with
    /// temperature, ensuring we serialise the model id verbatim.
    #[test]
    fn test_kimi_request_serialization() {
        let request = KimiRequest {
            model: "kimi-k2-0905-preview".to_string(),
            messages: vec![
                ChatMessage::System {
                    content: "You are a helpful assistant.".to_string(),
                },
                ChatMessage::User {
                    content: "Hello!".to_string(),
                },
            ],
            temperature: Some(0.7),
            tools: Vec::new(),
            tool_choice: None,
            thinking: None,
            stream: false,
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"model\":\"kimi-k2-0905-preview\""));
        assert!(json.contains("\"temperature\":0.7"));
    }

    /// Scenario: when tools are sent, `tool_choice = Auto` must serialise as
    /// the bare string `"auto"` to match the OpenAI-compatible wire shape.
    #[test]
    fn test_kimi_tool_choice_serialization() {
        let request = KimiRequest {
            model: "kimi-k2-0905-preview".to_string(),
            messages: vec![ChatMessage::User {
                content: "hi".to_string(),
            }],
            temperature: None,
            tools: vec![ChatToolDefinition {
                r#type: "function".to_string(),
                function: ChatFunctionDefinition {
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
            tool_choice: Some(KimiToolChoice::Mode(KimiToolChoiceMode::Auto)),
            thinking: None,
            stream: false,
        };

        let json = serde_json::to_value(request).unwrap();
        assert_eq!(json["tool_choice"], "auto");
    }

    /// Scenario: Kimi K2.6/K2.5 support a provider-specific `thinking` body
    /// field. Disabling thinking must serialize exactly as documented.
    #[test]
    fn test_kimi_thinking_serialization() {
        let request = KimiRequest {
            model: "kimi-k2.6".to_string(),
            messages: vec![ChatMessage::User {
                content: "hi".to_string(),
            }],
            temperature: None,
            tools: Vec::new(),
            tool_choice: None,
            thinking: kimi_thinking(Some(CompletionThinking::Disabled)),
            stream: false,
        };

        let json = serde_json::to_value(request).unwrap();
        assert_eq!(json["thinking"]["type"], "disabled");
        assert!(json["thinking"].get("keep").is_none());
    }

    /// Scenario: `Enabled`/`Low`/`Medium`/`High` all collapse to Kimi's
    /// `thinking.type = "enabled"` because the Moonshot API does not yet
    /// surface a depth knob. The wire payload must therefore look identical
    /// for all four variants — pin that mapping here so a future depth knob
    /// does not silently bypass the other three branches.
    #[test]
    fn test_kimi_thinking_enabled_variants_serialize_as_enabled() {
        for variant in [
            CompletionThinking::Enabled,
            CompletionThinking::Low,
            CompletionThinking::Medium,
            CompletionThinking::High,
        ] {
            let thinking = kimi_thinking(Some(variant));
            let json = serde_json::to_value(thinking).unwrap();
            assert_eq!(
                json["type"], "enabled",
                "variant {variant:?} should map to thinking.type=enabled"
            );
            assert_eq!(
                json["keep"], "all",
                "variant {variant:?} should preserve historical reasoning"
            );
        }
    }

    /// Scenario: `Auto` and `None` both enable Kimi thinking with
    /// `keep = "all"` so historical `reasoning_content` is preserved by
    /// default across multi-turn coding sessions.
    #[test]
    fn test_kimi_thinking_auto_and_none_preserve_history() {
        for thinking in [
            kimi_thinking(Some(CompletionThinking::Auto)),
            kimi_thinking(None),
        ] {
            let json = serde_json::to_value(thinking).unwrap();
            assert_eq!(json["type"], "enabled");
            assert_eq!(json["keep"], "all");
        }

        let request = KimiRequest {
            model: "kimi-k2.6".to_string(),
            messages: vec![ChatMessage::User {
                content: "hi".to_string(),
            }],
            temperature: None,
            tools: Vec::new(),
            tool_choice: None,
            thinking: kimi_thinking(Some(CompletionThinking::Auto)),
            stream: false,
        };
        let json = serde_json::to_value(request).unwrap();
        assert_eq!(json["thinking"]["type"], "enabled");
        assert_eq!(json["thinking"]["keep"], "all");
    }

    /// Scenario: when thinking is enabled the request must include
    /// `thinking.type = "enabled"` exactly, alongside the model and
    /// messages — pin the full envelope so a partial regression is caught.
    #[test]
    fn test_kimi_thinking_enabled_request_envelope() {
        let request = KimiRequest {
            model: "kimi-k2.6".to_string(),
            messages: vec![ChatMessage::User {
                content: "hi".to_string(),
            }],
            temperature: None,
            tools: Vec::new(),
            tool_choice: None,
            thinking: kimi_thinking(Some(CompletionThinking::Enabled)),
            stream: false,
        };

        let json = serde_json::to_value(request).unwrap();
        assert_eq!(json["model"], "kimi-k2.6");
        assert_eq!(json["thinking"]["type"], "enabled");
        assert_eq!(json["thinking"]["keep"], "all");
    }

    /// Scenario: Kimi defaults to streaming in Libra so thinking tokens can be
    /// rendered incrementally in the TUI.
    #[test]
    fn test_kimi_stream_request_serialization() {
        let request = KimiRequest {
            model: "kimi-k2.6".to_string(),
            messages: vec![ChatMessage::User {
                content: "Hello!".to_string(),
            }],
            temperature: None,
            tools: Vec::new(),
            tool_choice: None,
            thinking: kimi_thinking(None),
            stream: true,
        };

        let json = serde_json::to_value(request).unwrap();
        assert_eq!(json["stream"], true);
        assert_eq!(json["thinking"]["keep"], "all");
    }

    /// Scenario: a canonical text-only response should round-trip through
    /// serde with usage stats intact.
    #[test]
    fn test_kimi_response_deserialization() {
        let json = r#"
        {
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1677652288,
            "model": "kimi-k2-0905-preview",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "reasoning_content": "I should answer briefly.",
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

        let response: ChatResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.id, "chatcmpl-123");
        assert_eq!(response.model, "kimi-k2-0905-preview");
        assert_eq!(response.choices.len(), 1);
        assert_eq!(
            choice_reasoning_content(response.choices.first().unwrap()).as_deref(),
            Some("I should answer briefly.")
        );
        assert_eq!(response.usage.unwrap().total_tokens, 21);
    }

    /// Scenario: Kimi streaming chunks expose `reasoning_content` before visible
    /// content. The provider must preserve the final reasoning text and forward
    /// per-token thinking events to the UI.
    #[test]
    fn test_kimi_stream_accumulates_thinking_and_text() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut accumulator = KimiStreamAccumulator::default();

        let done = process_kimi_stream_line(
            br#"data: {"id":"cmpl_1","object":"chat.completion.chunk","created":1718345013,"model":"kimi-k2.6","choices":[{"index":0,"delta":{"role":"assistant","reasoning_content":"thinking"},"finish_reason":null}]}"#,
            &mut accumulator,
            Some(&tx),
        )
        .unwrap();
        assert!(!done);

        process_kimi_stream_line(
            br#"data: {"id":"cmpl_1","object":"chat.completion.chunk","created":1718345013,"model":"kimi-k2.6","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}"#,
            &mut accumulator,
            Some(&tx),
        )
        .unwrap();

        process_kimi_stream_line(
            br#"data: {"id":"cmpl_1","object":"chat.completion.chunk","created":1718345013,"model":"kimi-k2.6","choices":[{"index":0,"delta":{"content":"!"},"finish_reason":"stop","usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}}]}"#,
            &mut accumulator,
            Some(&tx),
        )
        .unwrap();

        let done = process_kimi_stream_line(b"data: [DONE]", &mut accumulator, Some(&tx)).unwrap();
        assert!(done);

        let response = accumulator.into_response("fallback-model").unwrap();
        assert_eq!(response.id, "cmpl_1");
        assert_eq!(response.model, "kimi-k2.6");
        assert_eq!(
            choice_reasoning_content(&response.choices[0]).as_deref(),
            Some("thinking")
        );
        assert_eq!(
            response.usage.as_ref().map(|usage| usage.total_tokens),
            Some(5)
        );
        let content = parse_choice_content(&response.choices[0]).unwrap();
        assert!(matches!(
            &content[0],
            crate::internal::ai::completion::AssistantContent::Text(text) if text.text == "Hello!"
        ));

        let first = rx.try_recv().unwrap();
        assert!(matches!(
            first,
            CompletionStreamEvent::ThinkingDelta { delta, .. } if delta == "thinking"
        ));
        let second = rx.try_recv().unwrap();
        assert!(matches!(
            second,
            CompletionStreamEvent::TextDelta { delta, .. } if delta == "Hello"
        ));
        let third = rx.try_recv().unwrap();
        assert!(matches!(
            third,
            CompletionStreamEvent::TextDelta { delta, .. } if delta == "!"
        ));
    }

    /// Scenario: Kimi can stream function-call fragments. A completed call must
    /// reconstruct into the same response shape as non-streaming tool calls and
    /// emit a preview event.
    #[test]
    fn test_kimi_stream_accumulates_tool_calls() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut accumulator = KimiStreamAccumulator::default();

        process_kimi_stream_line(
            br#"data: {"id":"cmpl_2","object":"chat.completion.chunk","created":1718345014,"model":"kimi-k2.6","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"read_file","arguments":"{\"file_path\":\"Cargo.toml\"}"}}]},"finish_reason":"tool_calls"}]}"#,
            &mut accumulator,
            Some(&tx),
        )
        .unwrap();

        let response = accumulator.into_response("fallback-model").unwrap();
        let content = parse_choice_content(&response.choices[0]).unwrap();
        assert!(matches!(
            &content[0],
            crate::internal::ai::completion::AssistantContent::ToolCall(tool_call)
                if tool_call.id == "call_1"
                    && tool_call.function.name == "read_file"
                    && tool_call.function.arguments == serde_json::json!({"file_path": "Cargo.toml"})
        ));

        let event = rx.try_recv().unwrap();
        assert!(matches!(
            event,
            CompletionStreamEvent::ToolCallPreview { tool_name, .. } if tool_name == "read_file"
        ));
    }

    /// Scenario: smoke-test that `Model::new` stores the model identifier
    /// verbatim.
    #[test]
    fn test_model_new() {
        let client = Client::with_api_key("test-key".to_string());
        let model = Model::new(client, "kimi-k2-0905-preview");
        assert_eq!(model.model_name(), "kimi-k2-0905-preview");
    }

    /// Scenario: `CompletionClient::completion_model` is the canonical entry
    /// point used by the agent runtime; verify it produces a `Model` bound to
    /// the requested identifier.
    #[test]
    fn test_client_completion_model() {
        let client = Client::with_api_key("test-key".to_string());
        let model = client.completion_model("kimi-latest");
        assert_eq!(model.model_name(), "kimi-latest");
    }
}
