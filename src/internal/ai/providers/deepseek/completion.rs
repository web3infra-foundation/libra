//! DeepSeek completion model implementation.
//!
//! DeepSeek exposes an OpenAI-compatible Chat Completions endpoint. Common wire
//! types and helpers are imported from [`openai_compat`](super::super::openai_compat).

use std::collections::{BTreeMap, HashSet};

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

use crate::internal::ai::{
    client::{CompletionClient, Provider},
    completion::{
        CompletionError, CompletionModel as CompletionModelTrait, CompletionReasoningEffort,
        CompletionStreamEvent, CompletionThinking,
        request::{CompletionRequest, CompletionResponse},
    },
    providers::{
        deepseek::client::Client,
        openai_compat::{
            ChatChoice, ChatErrorResponse, ChatFunctionCall, ChatMessage, ChatResponse,
            ChatToolCall, ChatToolDefinition, ChatUsage, build_messages_with_reasoning_content,
            choice_reasoning_content, parse_choice_content, parse_tools,
        },
    },
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

    async fn send_chat_completion_request(
        &self,
        request: &DeepSeekRequest,
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
                provider = "deepseek",
                status = status.as_u16(),
                "DeepSeek HTTP request succeeded"
            );
            return Ok(response);
        }

        let response_text = response.text().await.map_err(CompletionError::HttpError)?;
        tracing::debug!(
            provider = "deepseek",
            status = status.as_u16(),
            body_bytes = response_text.len(),
            "DeepSeek HTTP request failed"
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
// DeepSeek-specific Request / ToolChoice Types
// ================================================================

/// DeepSeek request body.
#[derive(Debug, Serialize)]
struct DeepSeekRequest {
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ChatToolDefinition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<DeepSeekToolChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<DeepSeekThinking>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<DeepSeekReasoningEffort>,
    stream: bool,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct DeepSeekThinking {
    #[serde(rename = "type")]
    r#type: DeepSeekThinkingType,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum DeepSeekThinkingType {
    Enabled,
    Disabled,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum DeepSeekReasoningEffort {
    Low,
    Medium,
    High,
    Max,
}

/// DeepSeek uses a tagged enum for tool choice (differs from OpenAI's untagged approach).
#[derive(Debug, Serialize, Deserialize)]
enum DeepSeekToolChoice {
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "none")]
    None,
    #[serde(rename = "required")]
    Required,
    Function(DeepSeekFunctionToolChoice),
}

#[derive(Debug, Serialize, Deserialize)]
struct DeepSeekFunctionToolChoice {
    #[serde(rename = "type")]
    r#type: String,
    function: DeepSeekToolChoiceFunction,
}

#[derive(Debug, Serialize, Deserialize)]
struct DeepSeekToolChoiceFunction {
    name: String,
}

fn deepseek_thinking(thinking: Option<CompletionThinking>) -> Option<DeepSeekThinking> {
    match thinking {
        Some(CompletionThinking::Disabled) => Some(DeepSeekThinking {
            r#type: DeepSeekThinkingType::Disabled,
        }),
        Some(
            CompletionThinking::Enabled
            | CompletionThinking::Low
            | CompletionThinking::Medium
            | CompletionThinking::High,
        ) => Some(DeepSeekThinking {
            r#type: DeepSeekThinkingType::Enabled,
        }),
        Some(CompletionThinking::Auto) | None => None,
    }
}

fn deepseek_reasoning_effort(
    reasoning_effort: Option<CompletionReasoningEffort>,
) -> Option<DeepSeekReasoningEffort> {
    match reasoning_effort {
        Some(CompletionReasoningEffort::Low) => Some(DeepSeekReasoningEffort::Low),
        Some(CompletionReasoningEffort::Medium) => Some(DeepSeekReasoningEffort::Medium),
        Some(CompletionReasoningEffort::High) => Some(DeepSeekReasoningEffort::High),
        Some(CompletionReasoningEffort::Max) => Some(DeepSeekReasoningEffort::Max),
        None => None,
    }
}

fn deepseek_thinking_enabled(thinking: &Option<DeepSeekThinking>) -> bool {
    matches!(
        thinking,
        Some(DeepSeekThinking {
            r#type: DeepSeekThinkingType::Enabled
        })
    )
}

fn normalize_messages_for_deepseek_thinking(
    messages: Vec<ChatMessage>,
) -> (Vec<ChatMessage>, usize) {
    let mut normalized = Vec::with_capacity(messages.len());
    let mut iter = messages.into_iter().peekable();
    let mut converted = 0usize;

    while let Some(message) = iter.next() {
        match message {
            ChatMessage::Assistant {
                content,
                reasoning_content: None,
                tool_calls,
            } => {
                let mut note_parts = Vec::new();
                if let Some(text) = content.filter(|text| !text.trim().is_empty()) {
                    note_parts.push(format!("Previous assistant message:\n{text}"));
                }

                if !tool_calls.is_empty() {
                    let tool_call_ids = tool_calls
                        .iter()
                        .map(|call| call.id.clone())
                        .collect::<HashSet<_>>();
                    let tool_call_summary = tool_calls
                        .iter()
                        .map(|call| {
                            format!(
                                "- {} ({}) arguments: {}",
                                call.function.name, call.id, call.function.arguments
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    note_parts.push(format!(
                        "Previous assistant tool calls:\n{tool_call_summary}"
                    ));

                    while matches!(
                        iter.peek(),
                        Some(ChatMessage::Tool { tool_call_id, .. })
                            if tool_call_ids.contains(tool_call_id)
                    ) {
                        if let Some(ChatMessage::Tool {
                            tool_call_id,
                            name,
                            content,
                        }) = iter.next()
                        {
                            note_parts.push(format!(
                                "Previous tool result for {name} ({tool_call_id}):\n{content}"
                            ));
                        }
                    }
                }

                if !note_parts.is_empty() {
                    normalized.push(ChatMessage::User {
                        content: note_parts.join("\n\n"),
                    });
                }
                converted += 1;
            }
            other => normalized.push(other),
        }
    }

    (normalized, converted)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeepSeekResponseKind {
    TextOrTool,
    ReasoningOnly,
    Empty,
}

fn classify_deepseek_choice(choice: &ChatChoice) -> DeepSeekResponseKind {
    let ChatMessage::Assistant {
        content,
        reasoning_content,
        tool_calls,
    } = &choice.message
    else {
        return DeepSeekResponseKind::Empty;
    };

    if content
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
        || !tool_calls.is_empty()
    {
        return DeepSeekResponseKind::TextOrTool;
    }

    if reasoning_content
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return DeepSeekResponseKind::ReasoningOnly;
    }

    DeepSeekResponseKind::Empty
}

#[derive(Debug, Deserialize)]
struct DeepSeekStreamChunk {
    id: String,
    choices: Vec<DeepSeekStreamChoice>,
    created: u64,
    model: String,
    #[serde(default)]
    usage: Option<ChatUsage>,
}

#[derive(Debug, Deserialize)]
struct DeepSeekStreamChoice {
    index: usize,
    delta: DeepSeekStreamDelta,
    finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct DeepSeekStreamDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<DeepSeekStreamToolCallDelta>,
}

#[derive(Debug, Deserialize)]
struct DeepSeekStreamToolCallDelta {
    #[serde(default)]
    index: Option<usize>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<DeepSeekStreamFunctionDelta>,
}

#[derive(Debug, Deserialize)]
struct DeepSeekStreamFunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Debug, Default)]
struct DeepSeekStreamAccumulator {
    id: Option<String>,
    created: Option<u64>,
    model: Option<String>,
    content: String,
    reasoning_content: String,
    tool_calls: BTreeMap<usize, DeepSeekStreamToolCallBuilder>,
    finish_reason: Option<String>,
    usage: Option<ChatUsage>,
}

#[derive(Debug, Default)]
struct DeepSeekStreamToolCallBuilder {
    id: Option<String>,
    name: String,
    arguments: String,
}

impl DeepSeekStreamToolCallBuilder {
    fn is_complete(&self) -> bool {
        !self.name.is_empty()
            && (self.arguments.trim().is_empty()
                || serde_json::from_str::<Value>(&self.arguments).is_ok())
    }
}

impl DeepSeekStreamAccumulator {
    fn has_salvageable_response(&self) -> bool {
        !self.content.trim().is_empty()
            || self
                .tool_calls
                .values()
                .any(DeepSeekStreamToolCallBuilder::is_complete)
    }

    fn has_partial_output(&self) -> bool {
        !self.content.is_empty()
            || !self.reasoning_content.is_empty()
            || !self.tool_calls.is_empty()
    }

    fn push_chunk(
        &mut self,
        chunk: DeepSeekStreamChunk,
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
        delta: DeepSeekStreamToolCallDelta,
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
                    "DeepSeek stream returned tool call {index} without a function name"
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
            id: self.id.unwrap_or_else(|| "deepseek-stream".to_string()),
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

fn process_deepseek_stream_line(
    line: &[u8],
    accumulator: &mut DeepSeekStreamAccumulator,
    stream_events: Option<&UnboundedSender<CompletionStreamEvent>>,
) -> Result<bool, CompletionError> {
    let line = std::str::from_utf8(line).map_err(|error| {
        CompletionError::ResponseError(format!("invalid DeepSeek stream UTF-8: {error}"))
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

    let chunk: DeepSeekStreamChunk = serde_json::from_str(data)?;
    accumulator.push_chunk(chunk, stream_events);
    Ok(false)
}

async fn read_deepseek_stream_response(
    response: reqwest::Response,
    fallback_model: &str,
    stream_events: Option<&UnboundedSender<CompletionStreamEvent>>,
) -> Result<ChatResponse, CompletionError> {
    let mut accumulator = DeepSeekStreamAccumulator::default();
    let mut pending = Vec::<u8>::new();
    let mut stream = response.bytes_stream();
    let mut done = false;

    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                if accumulator.has_salvageable_response() {
                    tracing::warn!(
                        provider = "deepseek",
                        error = %error,
                        content_bytes = accumulator.content.len(),
                        reasoning_bytes = accumulator.reasoning_content.len(),
                        tool_call_fragments = accumulator.tool_calls.len(),
                        "DeepSeek stream ended with a body read error after a usable response; using accumulated response"
                    );
                    break;
                }

                return Err(deepseek_stream_body_error(error, &accumulator));
            }
        };
        pending.extend_from_slice(&chunk);

        while let Some(newline_index) = pending.iter().position(|byte| *byte == b'\n') {
            let line = pending.drain(..=newline_index).collect::<Vec<_>>();
            done = process_deepseek_stream_line(&line, &mut accumulator, stream_events)? || done;
        }
    }

    if !pending.is_empty() && !done {
        process_deepseek_stream_line(&pending, &mut accumulator, stream_events)?;
    }

    tracing::debug!(
        provider = "deepseek",
        done,
        content_bytes = accumulator.content.len(),
        reasoning_bytes = accumulator.reasoning_content.len(),
        tool_call_fragments = accumulator.tool_calls.len(),
        finish_reason = accumulator.finish_reason.as_deref().unwrap_or(""),
        "DeepSeek stream accumulated response"
    );

    accumulator.into_response(fallback_model)
}

fn deepseek_stream_body_error(
    error: reqwest::Error,
    accumulator: &DeepSeekStreamAccumulator,
) -> CompletionError {
    if accumulator.has_partial_output() {
        return CompletionError::ResponseError(format!(
            "DeepSeek stream ended before a usable response: {error} \
             (received {} visible bytes, {} reasoning bytes, {} tool call fragments)",
            accumulator.content.len(),
            accumulator.reasoning_content.len(),
            accumulator.tool_calls.len()
        ));
    }

    CompletionError::HttpError(error)
}

async fn read_deepseek_json_response(
    response: reqwest::Response,
) -> Result<ChatResponse, CompletionError> {
    let response_text = response.text().await.map_err(CompletionError::HttpError)?;
    let response =
        serde_json::from_str::<ChatResponse>(&response_text).map_err(CompletionError::JsonError)?;
    tracing::debug!(
        provider = "deepseek",
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
        "DeepSeek JSON response decoded"
    );
    Ok(response)
}

fn should_retry_deepseek_stream_without_stream(error: &CompletionError) -> bool {
    match error {
        CompletionError::HttpError(error) => error.is_body() || error.is_timeout(),
        CompletionError::ResponseError(message) => {
            message.contains("DeepSeek stream ended before a usable response")
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
        let thinking = deepseek_thinking(request.thinking);
        let reasoning_effort = deepseek_reasoning_effort(request.reasoning_effort);
        let mut messages = build_messages_with_reasoning_content(&request)?;
        if deepseek_thinking_enabled(&thinking) {
            let (normalized, converted) = normalize_messages_for_deepseek_thinking(messages);
            if converted > 0 {
                tracing::debug!(
                    provider = "deepseek",
                    converted,
                    "converted assistant history entries without reasoning_content for DeepSeek thinking mode"
                );
            }
            messages = normalized;
        }
        let stream = request.stream.unwrap_or(false);
        let stream_events = request.stream_events.clone();

        let mut deepseek_request = DeepSeekRequest {
            model: self.model.clone(),
            messages,
            temperature: request.temperature,
            tool_choice: if tools.is_empty() {
                None
            } else {
                Some(DeepSeekToolChoice::Auto)
            },
            thinking,
            reasoning_effort,
            tools,
            stream,
        };

        tracing::debug!(
            provider = "deepseek",
            model = %deepseek_request.model,
            stream = deepseek_request.stream,
            messages = deepseek_request.messages.len(),
            tools = deepseek_request.tools.len(),
            has_temperature = deepseek_request.temperature.is_some(),
            thinking = ?deepseek_request.thinking.as_ref(),
            reasoning_effort = ?deepseek_request.reasoning_effort.as_ref(),
            "DeepSeek completion request started"
        );

        let response = self.send_chat_completion_request(&deepseek_request).await?;

        let deepseek_response = if stream {
            match read_deepseek_stream_response(response, &self.model, stream_events.as_ref()).await
            {
                Ok(response) => response,
                Err(error) if should_retry_deepseek_stream_without_stream(&error) => {
                    tracing::warn!(
                        provider = "deepseek",
                        error = %error,
                        "DeepSeek stream failed before a usable response; retrying once without stream"
                    );
                    deepseek_request.stream = false;
                    let response = self.send_chat_completion_request(&deepseek_request).await?;
                    read_deepseek_json_response(response).await?
                }
                Err(error) => return Err(error),
            }
        } else {
            read_deepseek_json_response(response).await?
        };

        let choice = deepseek_response
            .choices
            .first()
            .ok_or_else(|| CompletionError::ResponseError("No choices in response".to_string()))?;

        let response_kind = classify_deepseek_choice(choice);
        if matches!(
            response_kind,
            DeepSeekResponseKind::ReasoningOnly | DeepSeekResponseKind::Empty
        ) {
            tracing::debug!(
                provider = "deepseek",
                response_kind = ?response_kind,
                "DeepSeek returned no text or tool calls"
            );
        }
        let content = parse_choice_content(choice)?;
        let reasoning_content = choice_reasoning_content(choice);
        let text_parts = content
            .iter()
            .filter(|part| {
                matches!(
                    part,
                    crate::internal::ai::completion::AssistantContent::Text(_)
                )
            })
            .count();
        let tool_calls = content
            .iter()
            .filter(|part| {
                matches!(
                    part,
                    crate::internal::ai::completion::AssistantContent::ToolCall(_)
                )
            })
            .count();
        tracing::debug!(
            provider = "deepseek",
            model = %deepseek_response.model,
            response_id = %deepseek_response.id,
            response_kind = ?response_kind,
            finish_reason = choice.finish_reason.as_deref().unwrap_or(""),
            choices = deepseek_response.choices.len(),
            text_parts,
            tool_calls,
            reasoning_bytes = reasoning_content.as_deref().map(str::len).unwrap_or_default(),
            prompt_tokens = deepseek_response
                .usage
                .as_ref()
                .map(|usage| usage.prompt_tokens)
                .unwrap_or_default(),
            completion_tokens = deepseek_response
                .usage
                .as_ref()
                .map(|usage| usage.completion_tokens)
                .unwrap_or_default(),
            total_tokens = deepseek_response
                .usage
                .as_ref()
                .map(|usage| usage.total_tokens)
                .unwrap_or_default(),
            "DeepSeek completion response parsed"
        );

        Ok(CompletionResponse {
            content,
            reasoning_content,
            raw_response: deepseek_response,
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
pub type DeepSeekResponse = ChatResponse;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::providers::openai_compat::{ChatFunctionDefinition, ChatMessage};

    #[test]
    fn test_deepseek_request_serialization() {
        let request = DeepSeekRequest {
            model: "deepseek-chat".to_string(),
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
            reasoning_effort: None,
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
            tool_choice: Some(DeepSeekToolChoice::Auto),
            thinking: None,
            reasoning_effort: None,
            stream: false,
        };

        let json = serde_json::to_value(request).unwrap();
        assert_eq!(json["tool_choice"], "auto");
    }

    #[test]
    fn test_deepseek_reasoning_request_serialization() {
        let request = DeepSeekRequest {
            model: "deepseek-v4-pro".to_string(),
            messages: vec![ChatMessage::User {
                content: "Hello!".to_string(),
            }],
            temperature: None,
            tools: Vec::new(),
            tool_choice: None,
            thinking: deepseek_thinking(Some(CompletionThinking::Enabled)),
            reasoning_effort: deepseek_reasoning_effort(Some(CompletionReasoningEffort::High)),
            stream: false,
        };

        let json = serde_json::to_value(request).unwrap();
        assert_eq!(json["thinking"]["type"], "enabled");
        assert_eq!(json["reasoning_effort"], "high");
        assert_eq!(json["stream"], false);
    }

    #[test]
    fn deepseek_thinking_normalizes_synthetic_assistant_messages_without_reasoning() {
        let messages = vec![
            ChatMessage::User {
                content: "Build an intent".to_string(),
            },
            ChatMessage::Assistant {
                content: Some("IntentSpec ready for review.".to_string()),
                reasoning_content: None,
                tool_calls: Vec::new(),
            },
            ChatMessage::User {
                content: "Create the execution plan".to_string(),
            },
        ];

        let (normalized, converted) = normalize_messages_for_deepseek_thinking(messages);

        assert_eq!(converted, 1);
        assert_eq!(normalized.len(), 3);
        assert!(matches!(
            &normalized[1],
            ChatMessage::User { content }
                if content.contains("Previous assistant message")
                    && content.contains("IntentSpec ready for review.")
        ));
    }

    #[test]
    fn deepseek_thinking_keeps_assistant_messages_with_reasoning_content() {
        let messages = vec![ChatMessage::Assistant {
            content: None,
            reasoning_content: Some("Need to submit the draft.".to_string()),
            tool_calls: vec![ChatToolCall {
                id: "call_1".to_string(),
                r#type: "function".to_string(),
                function: ChatFunctionCall {
                    name: "submit_intent_draft".to_string(),
                    arguments: "{\"draft\":{}}".to_string(),
                },
            }],
        }];

        let (normalized, converted) = normalize_messages_for_deepseek_thinking(messages);

        assert_eq!(converted, 0);
        assert!(matches!(
            &normalized[0],
            ChatMessage::Assistant {
                reasoning_content: Some(reasoning),
                tool_calls,
                ..
            } if reasoning == "Need to submit the draft." && tool_calls.len() == 1
        ));
    }

    #[test]
    fn deepseek_thinking_collapses_unusable_tool_history_without_reasoning_content() {
        let messages = vec![
            ChatMessage::Assistant {
                content: Some("I will inspect the repo.".to_string()),
                reasoning_content: None,
                tool_calls: vec![ChatToolCall {
                    id: "call_1".to_string(),
                    r#type: "function".to_string(),
                    function: ChatFunctionCall {
                        name: "read_file".to_string(),
                        arguments: "{\"file_path\":\"Cargo.toml\"}".to_string(),
                    },
                }],
            },
            ChatMessage::Tool {
                tool_call_id: "call_1".to_string(),
                name: "read_file".to_string(),
                content: "{\"content\":\"[package]\"}".to_string(),
            },
            ChatMessage::User {
                content: "Continue".to_string(),
            },
        ];

        let (normalized, converted) = normalize_messages_for_deepseek_thinking(messages);

        assert_eq!(converted, 1);
        assert_eq!(normalized.len(), 2);
        assert!(matches!(
            &normalized[0],
            ChatMessage::User { content }
                if content.contains("Previous assistant tool calls")
                    && content.contains("read_file")
                    && content.contains("Previous tool result")
        ));
        assert!(matches!(&normalized[1], ChatMessage::User { content } if content == "Continue"));
    }

    #[test]
    fn test_deepseek_stream_request_serialization() {
        let request = DeepSeekRequest {
            model: "deepseek-v4-pro".to_string(),
            messages: vec![ChatMessage::User {
                content: "Hello!".to_string(),
            }],
            temperature: None,
            tools: Vec::new(),
            tool_choice: None,
            thinking: None,
            reasoning_effort: None,
            stream: true,
        };

        let json = serde_json::to_value(request).unwrap();
        assert_eq!(json["stream"], true);
    }

    #[test]
    fn test_deepseek_stream_accumulates_text_and_usage() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut accumulator = DeepSeekStreamAccumulator::default();

        let done = process_deepseek_stream_line(
            br#"data: {"id":"chunk_1","choices":[{"index":0,"delta":{"content":"Hello","role":"assistant"},"finish_reason":null}],"created":1718345013,"model":"deepseek-v4-pro","object":"chat.completion.chunk","usage":null}"#,
            &mut accumulator,
            Some(&tx),
        )
        .unwrap();
        assert!(!done);

        process_deepseek_stream_line(
            br#"data: {"id":"chunk_1","choices":[{"index":0,"delta":{"reasoning_content":"thinking"},"finish_reason":null}],"created":1718345013,"model":"deepseek-v4-pro","object":"chat.completion.chunk","usage":null}"#,
            &mut accumulator,
            Some(&tx),
        )
        .unwrap();

        process_deepseek_stream_line(
            br#"data: {"id":"chunk_1","choices":[{"index":0,"delta":{"content":"!"},"finish_reason":"stop"}],"created":1718345013,"model":"deepseek-v4-pro","object":"chat.completion.chunk","usage":{"completion_tokens":2,"prompt_tokens":3,"total_tokens":5}}"#,
            &mut accumulator,
            Some(&tx),
        )
        .unwrap();

        let done =
            process_deepseek_stream_line(b"data: [DONE]", &mut accumulator, Some(&tx)).unwrap();
        assert!(done);

        let response = accumulator.into_response("fallback-model").unwrap();
        assert_eq!(response.id, "chunk_1");
        assert_eq!(response.model, "deepseek-v4-pro");
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
            CompletionStreamEvent::TextDelta { delta, .. } if delta == "Hello"
        ));
        let second = rx.try_recv().unwrap();
        assert!(matches!(
            second,
            CompletionStreamEvent::ThinkingDelta { delta, .. } if delta == "thinking"
        ));
        let third = rx.try_recv().unwrap();
        assert!(matches!(
            third,
            CompletionStreamEvent::TextDelta { delta, .. } if delta == "!"
        ));
    }

    #[test]
    fn test_deepseek_stream_accumulates_tool_calls() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut accumulator = DeepSeekStreamAccumulator::default();

        process_deepseek_stream_line(
            br#"data: {"id":"chunk_2","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"read_file","arguments":"{\"file_path\":\"Cargo.toml\"}"}}]},"finish_reason":"tool_calls"}],"created":1718345014,"model":"deepseek-v4-pro","object":"chat.completion.chunk"}"#,
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

    #[test]
    fn test_deepseek_stream_salvageable_response_detection() {
        let mut reasoning_only = DeepSeekStreamAccumulator::default();
        process_deepseek_stream_line(
            br#"data: {"id":"chunk_3","choices":[{"index":0,"delta":{"reasoning_content":"thinking only"},"finish_reason":null}],"created":1718345015,"model":"deepseek-v4-pro","object":"chat.completion.chunk"}"#,
            &mut reasoning_only,
            None,
        )
        .unwrap();
        assert!(reasoning_only.has_partial_output());
        assert!(!reasoning_only.has_salvageable_response());

        let mut with_text = DeepSeekStreamAccumulator::default();
        process_deepseek_stream_line(
            br#"data: {"id":"chunk_4","choices":[{"index":0,"delta":{"content":"usable"},"finish_reason":null}],"created":1718345016,"model":"deepseek-v4-pro","object":"chat.completion.chunk"}"#,
            &mut with_text,
            None,
        )
        .unwrap();
        assert!(with_text.has_salvageable_response());

        let mut with_tool_call = DeepSeekStreamAccumulator::default();
        process_deepseek_stream_line(
            br#"data: {"id":"chunk_5","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"submit_plan_draft","arguments":"{\"steps\":[\"inspect\"]}"}}]},"finish_reason":"tool_calls"}],"created":1718345017,"model":"deepseek-v4-pro","object":"chat.completion.chunk"}"#,
            &mut with_tool_call,
            None,
        )
        .unwrap();
        assert!(with_tool_call.has_salvageable_response());
    }

    #[test]
    fn test_deepseek_stream_body_error_triggers_non_stream_fallback() {
        let error = CompletionError::ResponseError(
            "DeepSeek stream ended before a usable response: error decoding response body"
                .to_string(),
        );

        assert!(should_retry_deepseek_stream_without_stream(&error));
    }

    #[test]
    fn test_deepseek_max_reasoning_effort_serialization() {
        let effort = deepseek_reasoning_effort(Some(CompletionReasoningEffort::Max));

        let json = serde_json::to_value(effort).unwrap();
        assert_eq!(json, "max");
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

        let response: ChatResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.id, "chatcmpl-123");
        assert_eq!(response.model, "deepseek-chat");
        assert_eq!(response.choices.len(), 1);
        assert_eq!(response.usage.unwrap().total_tokens, 21);
    }

    #[test]
    fn test_deepseek_reasoning_content_deserialization() {
        let json = r#"
        {
            "id": "chatcmpl-456",
            "object": "chat.completion",
            "created": 1677652288,
            "model": "deepseek-v4-pro",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "",
                    "reasoning_content": "I need to inspect the repository before editing.",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": "{\"path\":\"Cargo.toml\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": null
        }
        "#;

        let response: ChatResponse = serde_json::from_str(json).unwrap();
        let choice = &response.choices[0];
        let content = parse_choice_content(choice).unwrap();

        assert_eq!(
            choice_reasoning_content(choice).as_deref(),
            Some("I need to inspect the repository before editing.")
        );
        assert!(matches!(
            &content[0],
            crate::internal::ai::completion::AssistantContent::ToolCall(tool_call)
                if tool_call.id == "call_1"
                    && tool_call.function.name == "read_file"
        ));
    }

    #[test]
    fn deepseek_classifies_text_tool_reasoning_only_and_empty_responses() {
        let text_choice = ChatChoice {
            index: 0,
            message: ChatMessage::Assistant {
                content: Some("hello".to_string()),
                reasoning_content: None,
                tool_calls: Vec::new(),
            },
            finish_reason: Some("stop".to_string()),
        };
        assert_eq!(
            classify_deepseek_choice(&text_choice),
            DeepSeekResponseKind::TextOrTool
        );

        let tool_choice = ChatChoice {
            index: 0,
            message: ChatMessage::Assistant {
                content: Some(String::new()),
                reasoning_content: Some("thinking".to_string()),
                tool_calls: vec![ChatToolCall {
                    id: "call_1".to_string(),
                    r#type: "function".to_string(),
                    function: ChatFunctionCall {
                        name: "read_file".to_string(),
                        arguments: "{\"file_path\":\"Cargo.toml\"}".to_string(),
                    },
                }],
            },
            finish_reason: Some("tool_calls".to_string()),
        };
        assert_eq!(
            classify_deepseek_choice(&tool_choice),
            DeepSeekResponseKind::TextOrTool
        );

        let reasoning_only_choice = ChatChoice {
            index: 0,
            message: ChatMessage::Assistant {
                content: Some(String::new()),
                reasoning_content: Some("thinking only".to_string()),
                tool_calls: Vec::new(),
            },
            finish_reason: Some("stop".to_string()),
        };
        assert_eq!(
            classify_deepseek_choice(&reasoning_only_choice),
            DeepSeekResponseKind::ReasoningOnly
        );

        let empty_choice = ChatChoice {
            index: 0,
            message: ChatMessage::Assistant {
                content: Some("   ".to_string()),
                reasoning_content: None,
                tool_calls: Vec::new(),
            },
            finish_reason: Some("stop".to_string()),
        };
        assert_eq!(
            classify_deepseek_choice(&empty_choice),
            DeepSeekResponseKind::Empty
        );
    }

    #[test]
    fn test_model_new() {
        let client = Client::with_api_key("test-key".to_string());
        let model = Model::new(client, "deepseek-chat");
        assert_eq!(model.model_name(), "deepseek-chat");
    }

    #[test]
    fn test_client_completion_model() {
        let client = Client::with_api_key("test-key".to_string());
        let model = client.completion_model("deepseek-chat");
        assert_eq!(model.model_name(), "deepseek-chat");
    }
}
