//! Ollama completion model implementation.
//!
//! Libra sends requests to Ollama's native `/api/chat` endpoint and converts
//! the response into the shared OpenAI-compatible internal chat shape.

use std::time::Instant;

use chrono::DateTime;
use futures_util::StreamExt;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::time::{Duration, sleep};
use uuid::Uuid;

use crate::internal::ai::{
    client::{CompletionClient, Provider},
    completion::{
        AssistantContent, CompletionError, CompletionModel as CompletionModelTrait,
        CompletionStreamEvent, CompletionThinking, Function, Message, UserContent,
        request::{CompletionRequest, CompletionResponse},
    },
    providers::{
        ollama::client::Client,
        openai_compat::{
            ChatChoice, ChatErrorResponse, ChatFunctionCall, ChatMessage, ChatResponse,
            ChatToolCall, ChatToolDefinition, ChatUsage, parse_tools,
        },
    },
    tools::ToolDefinition,
};

/// Ollama completion model.
#[derive(Clone, Debug)]
pub struct Model {
    client: Client,
    model: String,
}

const OLLAMA_LOAD_RETRY_ATTEMPTS: u32 = 6;
const OLLAMA_LOAD_RETRY_BASE_DELAY_MS: u64 = 1_000;
const OLLAMA_LOAD_RETRY_MAX_DELAY_MS: u64 = 10_000;

impl Model {
    /// Creates a new Ollama completion model.
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
// Ollama-specific Request / ToolChoice Types
// ================================================================

#[derive(Debug, Serialize)]
struct OllamaRequest {
    model: String,
    messages: Vec<OllamaRequestMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    think: Option<OllamaThink>,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<OllamaOptions>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ChatToolDefinition>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(untagged)]
enum OllamaThink {
    Bool(bool),
    Level(OllamaThinkLevel),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum OllamaThinkLevel {
    Low,
    Medium,
    High,
}

#[derive(Debug, Serialize)]
struct OllamaOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "role", rename_all = "lowercase")]
enum OllamaRequestMessage {
    System {
        content: String,
    },
    User {
        content: String,
    },
    Assistant {
        content: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<OllamaRequestToolCall>,
    },
    Tool {
        content: String,
    },
}

#[derive(Debug, Serialize)]
struct OllamaRequestToolCall {
    function: OllamaRequestFunctionCall,
}

#[derive(Debug, Serialize)]
struct OllamaRequestFunctionCall {
    name: String,
    arguments: Value,
}

#[derive(Debug, Deserialize)]
struct OllamaResponse {
    model: String,
    created_at: String,
    message: OllamaMessage,
    #[serde(default)]
    prompt_eval_count: Option<usize>,
    #[serde(default)]
    eval_count: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct OllamaMessage {
    content: Option<String>,
    #[serde(default)]
    thinking: Option<String>,
    #[serde(default)]
    tool_calls: Vec<OllamaToolCall>,
}

#[derive(Debug, Deserialize)]
struct OllamaToolCall {
    #[serde(default)]
    id: Option<String>,
    function: OllamaFunctionCall,
}

#[derive(Debug, Deserialize)]
struct OllamaFunctionCall {
    name: String,
    arguments: Value,
}

#[derive(Debug, Deserialize)]
struct OllamaStreamChunk {
    model: String,
    created_at: String,
    message: OllamaMessage,
    #[serde(default)]
    done: bool,
    #[serde(default)]
    done_reason: Option<String>,
    #[serde(default)]
    prompt_eval_count: Option<usize>,
    #[serde(default)]
    eval_count: Option<usize>,
}

#[derive(Debug, Default)]
struct OllamaStreamAccumulator {
    model: Option<String>,
    created_at: Option<String>,
    content: String,
    tool_calls: Vec<OllamaToolCall>,
    prompt_eval_count: Option<usize>,
    eval_count: Option<usize>,
    done: bool,
    chunk_count: usize,
}

struct OllamaStreamReadContext<'a> {
    request_id: &'a str,
    model: &'a str,
    endpoint: &'a str,
    attempt: u32,
    total_attempts: u32,
    attempt_started: Instant,
    stream_events: Option<&'a tokio::sync::mpsc::UnboundedSender<CompletionStreamEvent>>,
}

impl OllamaStreamReadContext<'_> {
    fn elapsed_ms(&self) -> u64 {
        self.attempt_started
            .elapsed()
            .as_millis()
            .min(u128::from(u64::MAX)) as u64
    }
}

#[derive(Debug, Deserialize)]
struct OllamaNativeErrorResponse {
    error: OllamaNativeError,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum OllamaNativeError {
    Message(String),
    Object { message: Option<String> },
}

fn native_chat_endpoint(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    let root = trimmed.strip_suffix("/v1").unwrap_or(trimmed);
    format!("{root}/api/chat")
}

fn ollama_tools_from_definitions(
    tools: &[ToolDefinition],
    compact_tool_schema: bool,
) -> Vec<ChatToolDefinition> {
    if !compact_tool_schema {
        return parse_tools(tools);
    }

    tools
        .iter()
        .map(|tool| ChatToolDefinition {
            r#type: "function".to_string(),
            function: crate::internal::ai::providers::openai_compat::ChatFunctionDefinition {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: compact_tool_parameters(&tool.parameters),
            },
        })
        .collect()
}

fn compact_tool_parameters(parameters: &Value) -> Value {
    compact_schema_value(parameters, 3)
}

fn compact_schema_value(schema: &Value, depth: usize) -> Value {
    let Some(object) = schema.as_object() else {
        return json!({ "type": "object" });
    };

    if object.contains_key("$ref") {
        return json!({ "type": "object" });
    }

    match object.get("type").and_then(Value::as_str) {
        Some("object") => {
            let mut compact = Map::new();
            compact.insert("type".to_string(), json!("object"));

            if depth > 0 {
                if let Some(properties) = object.get("properties").and_then(Value::as_object) {
                    let mut compact_properties = Map::new();
                    for (name, property_schema) in properties {
                        compact_properties.insert(
                            name.clone(),
                            compact_schema_value(property_schema, depth.saturating_sub(1)),
                        );
                    }
                    if !compact_properties.is_empty() {
                        compact.insert("properties".to_string(), Value::Object(compact_properties));
                    }
                }

                if let Some(required) = object.get("required").and_then(Value::as_array) {
                    let required = required
                        .iter()
                        .filter_map(Value::as_str)
                        .map(|item| json!(item))
                        .collect::<Vec<_>>();
                    if !required.is_empty() {
                        compact.insert("required".to_string(), Value::Array(required));
                    }
                }
            }

            Value::Object(compact)
        }
        Some("array") => {
            let mut compact = Map::new();
            compact.insert("type".to_string(), json!("array"));
            if let Some(items) = object.get("items") {
                compact.insert(
                    "items".to_string(),
                    compact_schema_value(items, depth.saturating_sub(1)),
                );
            }
            Value::Object(compact)
        }
        Some(schema_type) => json!({ "type": schema_type }),
        None => json!({ "type": "object" }),
    }
}

fn format_ollama_provider_error(
    status: StatusCode,
    response_text: &str,
    model: &str,
    endpoint: &str,
) -> String {
    let detail = extract_ollama_error_message(response_text).unwrap_or_else(|| {
        "empty response body; Ollama may still be loading the model, the runner may have crashed, \
         or the model may be unavailable. Check `ollama ps` and the Ollama server logs."
            .to_string()
    });
    format!(
        "Ollama request failed: status {} from {} for model '{}': {}",
        status.as_u16(),
        endpoint,
        model,
        detail
    )
}

fn extract_ollama_error_message(response_text: &str) -> Option<String> {
    let trimmed = response_text.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Ok(error_response) = serde_json::from_str::<ChatErrorResponse>(trimmed)
        && !error_response.error.message.trim().is_empty()
    {
        return Some(error_response.error.message);
    }

    if let Ok(error_response) = serde_json::from_str::<OllamaNativeErrorResponse>(trimmed) {
        match error_response.error {
            OllamaNativeError::Message(message) if !message.trim().is_empty() => {
                return Some(message);
            }
            OllamaNativeError::Object {
                message: Some(message),
            } if !message.trim().is_empty() => return Some(message),
            _ => {}
        }
    }

    Some(trimmed.to_string())
}

fn truncate_for_log(value: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for ch in value.chars().take(max_chars) {
        out.push(ch);
    }
    if out.len() < value.len() {
        out.push_str("...");
    }
    out
}

fn should_retry_ollama_load_error(status: StatusCode, response_text: &str) -> bool {
    if status != StatusCode::SERVICE_UNAVAILABLE {
        return false;
    }

    let body = response_text.trim();
    if body.is_empty() {
        return true;
    }

    let message = extract_ollama_error_message(body)
        .unwrap_or_else(|| body.to_string())
        .to_ascii_lowercase();
    [
        "loading",
        "try again",
        "temporarily unavailable",
        "temporarily overloaded",
        "busy",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

fn ollama_load_retry_delay(attempt: u32) -> Duration {
    let exp = 2_u64.saturating_pow(attempt.saturating_sub(1));
    Duration::from_millis(
        OLLAMA_LOAD_RETRY_BASE_DELAY_MS
            .saturating_mul(exp)
            .min(OLLAMA_LOAD_RETRY_MAX_DELAY_MS),
    )
}

fn default_ollama_think_setting() -> Result<Option<OllamaThink>, CompletionError> {
    match std::env::var("OLLAMA_THINK") {
        Ok(value) => parse_ollama_think_setting(&value)
            .map_err(|message| CompletionError::ProviderError(message.to_string())),
        Err(std::env::VarError::NotPresent) => Ok(Some(OllamaThink::Bool(false))),
        Err(std::env::VarError::NotUnicode(_)) => Err(CompletionError::ProviderError(
            "invalid OLLAMA_THINK: value is not valid Unicode; expected true, false, low, medium, high, or auto"
                .to_string(),
        )),
    }
}

fn ollama_think_setting(
    thinking: Option<CompletionThinking>,
) -> Result<Option<OllamaThink>, CompletionError> {
    match thinking {
        Some(CompletionThinking::Auto) => Ok(None),
        Some(CompletionThinking::Disabled) => Ok(Some(OllamaThink::Bool(false))),
        Some(CompletionThinking::Enabled) => Ok(Some(OllamaThink::Bool(true))),
        Some(CompletionThinking::Low) => Ok(Some(OllamaThink::Level(OllamaThinkLevel::Low))),
        Some(CompletionThinking::Medium) => Ok(Some(OllamaThink::Level(OllamaThinkLevel::Medium))),
        Some(CompletionThinking::High) => Ok(Some(OllamaThink::Level(OllamaThinkLevel::High))),
        None => default_ollama_think_setting(),
    }
}

fn parse_ollama_think_setting(value: &str) -> Result<Option<OllamaThink>, &'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "auto" | "default" => Ok(None),
        "0" | "false" | "no" | "off" => Ok(Some(OllamaThink::Bool(false))),
        "1" | "true" | "yes" | "on" => Ok(Some(OllamaThink::Bool(true))),
        "low" => Ok(Some(OllamaThink::Level(OllamaThinkLevel::Low))),
        "medium" => Ok(Some(OllamaThink::Level(OllamaThinkLevel::Medium))),
        "high" => Ok(Some(OllamaThink::Level(OllamaThinkLevel::High))),
        _ => Err("invalid OLLAMA_THINK: expected true, false, low, medium, high, or auto"),
    }
}

impl OllamaStreamAccumulator {
    fn push_chunk(&mut self, chunk: OllamaStreamChunk) {
        self.chunk_count = self.chunk_count.saturating_add(1);
        self.model = Some(chunk.model);
        self.created_at = Some(chunk.created_at);
        if let Some(content) = chunk.message.content
            && !content.is_empty()
        {
            self.content.push_str(&content);
        }
        self.tool_calls.extend(chunk.message.tool_calls);
        if chunk.prompt_eval_count.is_some() {
            self.prompt_eval_count = chunk.prompt_eval_count;
        }
        if chunk.eval_count.is_some() {
            self.eval_count = chunk.eval_count;
        }
        if chunk.done {
            self.done = true;
        }
    }

    fn into_response(self) -> Result<OllamaResponse, CompletionError> {
        if self.chunk_count == 0 {
            return Err(CompletionError::ResponseError(
                "Ollama streaming response was empty".to_string(),
            ));
        }
        if !self.done {
            return Err(CompletionError::ResponseError(
                "Ollama streaming response ended before the final done chunk".to_string(),
            ));
        }

        Ok(OllamaResponse {
            model: self.model.unwrap_or_default(),
            created_at: self.created_at.unwrap_or_default(),
            message: OllamaMessage {
                content: (!self.content.is_empty()).then_some(self.content),
                thinking: None,
                tool_calls: self.tool_calls,
            },
            prompt_eval_count: self.prompt_eval_count,
            eval_count: self.eval_count,
        })
    }
}

fn parse_ollama_stream_line(line: &str) -> Result<Option<OllamaStreamChunk>, CompletionError> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    serde_json::from_str::<OllamaStreamChunk>(trimmed)
        .map(Some)
        .map_err(CompletionError::JsonError)
}

#[cfg(test)]
fn aggregate_ollama_stream_lines<'a>(
    lines: impl IntoIterator<Item = &'a str>,
) -> Result<OllamaResponse, CompletionError> {
    let mut accumulator = OllamaStreamAccumulator::default();
    for line in lines {
        if let Some(chunk) = parse_ollama_stream_line(line)? {
            accumulator.push_chunk(chunk);
        }
    }
    accumulator.into_response()
}

fn process_ollama_stream_line(
    line_bytes: &[u8],
    accumulator: &mut OllamaStreamAccumulator,
    ctx: &OllamaStreamReadContext<'_>,
) -> Result<(), CompletionError> {
    let line = line_bytes.strip_suffix(b"\n").unwrap_or(line_bytes);
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let line = std::str::from_utf8(line).map_err(|error| {
        CompletionError::ResponseError(format!("Ollama streaming response was not UTF-8: {error}"))
    })?;
    let Some(chunk) = parse_ollama_stream_line(line)? else {
        return Ok(());
    };

    let chunk_index = accumulator.chunk_count.saturating_add(1);
    let content = chunk.message.content.as_deref().unwrap_or_default();
    let thinking = chunk.message.thinking.as_deref().unwrap_or_default();
    tracing::debug!(
        provider = "ollama",
        request_id = ctx.request_id,
        model = ctx.model,
        endpoint = ctx.endpoint,
        attempt = ctx.attempt,
        chunk_index,
        done = chunk.done,
        done_reason = ?chunk.done_reason,
        content_bytes = content.len(),
        thinking_bytes = thinking.len(),
        tool_call_count = chunk.message.tool_calls.len(),
        elapsed_ms = ctx.elapsed_ms(),
        "received Ollama streaming chunk"
    );

    if !content.is_empty() {
        tracing::debug!(
            provider = "ollama",
            request_id = ctx.request_id,
            model = ctx.model,
            endpoint = ctx.endpoint,
            attempt = ctx.attempt,
            chunk_index,
            content = %truncate_for_log(content, 2048),
            "received Ollama streamed content"
        );
        if let Some(stream_events) = ctx.stream_events {
            let _ = stream_events.send(CompletionStreamEvent::TextDelta {
                request_id: Some(ctx.request_id.to_string()),
                delta: content.to_string(),
            });
        }
    }

    if !thinking.is_empty()
        && let Some(stream_events) = ctx.stream_events
    {
        let _ = stream_events.send(CompletionStreamEvent::ThinkingDelta {
            request_id: Some(ctx.request_id.to_string()),
            delta: thinking.to_string(),
        });
    }

    for (tool_index, tool_call) in chunk.message.tool_calls.iter().enumerate() {
        let tool_call_id = tool_call
            .id
            .clone()
            .unwrap_or_else(|| format!("call_{}", accumulator.tool_calls.len() + tool_index));
        tracing::debug!(
            provider = "ollama",
            request_id = ctx.request_id,
            model = ctx.model,
            endpoint = ctx.endpoint,
            attempt = ctx.attempt,
            chunk_index,
            tool_index,
            tool_call_id = %tool_call_id,
            tool_name = %tool_call.function.name,
            arguments = %truncate_for_log(&tool_arguments_json(&tool_call.function.arguments), 2048),
            "received Ollama streamed tool call"
        );
        if let Some(stream_events) = ctx.stream_events {
            let _ = stream_events.send(CompletionStreamEvent::ToolCallPreview {
                request_id: Some(ctx.request_id.to_string()),
                call_id: tool_call_id,
                tool_name: tool_call.function.name.clone(),
                arguments: tool_call.function.arguments.clone(),
            });
        }
    }

    accumulator.push_chunk(chunk);
    Ok(())
}

async fn read_ollama_stream_response(
    response: reqwest::Response,
    ctx: OllamaStreamReadContext<'_>,
) -> Result<OllamaResponse, CompletionError> {
    let mut accumulator = OllamaStreamAccumulator::default();
    let mut pending = Vec::<u8>::new();
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                tracing::warn!(
                    provider = "ollama",
                    request_id = ctx.request_id,
                    model = ctx.model,
                    endpoint = ctx.endpoint,
                    attempt = ctx.attempt,
                    total_attempts = ctx.total_attempts,
                    elapsed_ms = ctx.elapsed_ms(),
                    timeout = error.is_timeout(),
                    connect = error.is_connect(),
                    error = %error,
                    "failed to read Ollama streaming response chunk"
                );
                return Err(CompletionError::HttpError(error));
            }
        };

        pending.extend_from_slice(&chunk);
        while let Some(newline_index) = pending.iter().position(|byte| *byte == b'\n') {
            let line = pending.drain(..=newline_index).collect::<Vec<_>>();
            process_ollama_stream_line(&line, &mut accumulator, &ctx)?;
        }
    }

    if !pending.is_empty() {
        process_ollama_stream_line(&pending, &mut accumulator, &ctx)?;
    }

    let response = accumulator.into_response()?;
    tracing::debug!(
        provider = "ollama",
        request_id = ctx.request_id,
        model = ctx.model,
        endpoint = ctx.endpoint,
        attempt = ctx.attempt,
        total_attempts = ctx.total_attempts,
        response_model = %response.model,
        prompt_eval_count = ?response.prompt_eval_count,
        eval_count = ?response.eval_count,
        elapsed_ms = ctx.elapsed_ms(),
        "completed Ollama streaming response"
    );
    Ok(response)
}

fn ollama_response_to_chat_response(response: OllamaResponse, response_id: String) -> ChatResponse {
    let content = response
        .message
        .content
        .filter(|content| !content.is_empty());
    let tool_calls = response
        .message
        .tool_calls
        .into_iter()
        .enumerate()
        .map(|(index, tool_call)| ChatToolCall {
            id: tool_call.id.unwrap_or_else(|| format!("call_{index}")),
            r#type: "function".to_string(),
            function: ChatFunctionCall {
                name: tool_call.function.name,
                arguments: tool_arguments_json(&tool_call.function.arguments),
            },
        })
        .collect::<Vec<_>>();
    let finish_reason = if tool_calls.is_empty() {
        Some("stop".to_string())
    } else {
        Some("tool_calls".to_string())
    };
    let usage = match (response.prompt_eval_count, response.eval_count) {
        (Some(prompt_tokens), Some(completion_tokens)) => Some(ChatUsage {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens.saturating_add(completion_tokens),
        }),
        _ => None,
    };
    let created = DateTime::parse_from_rfc3339(&response.created_at)
        .ok()
        .and_then(|created| u64::try_from(created.timestamp()).ok())
        .unwrap_or_default();

    ChatResponse {
        id: response_id,
        object: "chat.completion".to_string(),
        created,
        model: response.model,
        choices: vec![ChatChoice {
            index: 0,
            message: ChatMessage::Assistant {
                content,
                reasoning_content: None,
                tool_calls,
            },
            finish_reason,
        }],
        usage,
    }
}

fn tool_arguments_json(arguments: &Value) -> String {
    match arguments {
        Value::String(value) => value.clone(),
        other => serde_json::to_string(other).unwrap_or_else(|_| other.to_string()),
    }
}

fn build_ollama_messages(
    request: &CompletionRequest,
) -> Result<Vec<OllamaRequestMessage>, CompletionError> {
    let mut messages = Vec::new();

    if let Some(preamble) = &request.preamble {
        messages.push(OllamaRequestMessage::System {
            content: preamble.clone(),
        });
    }

    for msg in &request.chat_history {
        match msg {
            Message::User { content } => {
                for item in content.iter() {
                    match item {
                        UserContent::Text(text) => messages.push(OllamaRequestMessage::User {
                            content: text.text.clone(),
                        }),
                        UserContent::ToolResult(tool_result) => {
                            let content = serde_json::to_string(&tool_result.result)
                                .unwrap_or_else(|_| tool_result.result.to_string());
                            messages.push(OllamaRequestMessage::Tool { content });
                        }
                        UserContent::Image(_) => {
                            return Err(CompletionError::NotImplemented(
                                "Image content is not supported by this provider".into(),
                            ));
                        }
                    }
                }
            }
            Message::System { content } => {
                for item in content.iter() {
                    match item {
                        UserContent::Text(text) => messages.push(OllamaRequestMessage::System {
                            content: text.text.clone(),
                        }),
                        UserContent::ToolResult(tool_result) => {
                            let content = serde_json::to_string(&tool_result.result)
                                .unwrap_or_else(|_| tool_result.result.to_string());
                            messages.push(OllamaRequestMessage::System { content });
                        }
                        UserContent::Image(_) => {
                            return Err(CompletionError::NotImplemented(
                                "Image content is not supported by this provider".into(),
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
                        AssistantContent::Text(text) => {
                            if !text.text.trim().is_empty() {
                                text_parts.push(text.text.clone());
                            }
                        }
                        AssistantContent::ToolCall(call) => {
                            tool_calls.push(OllamaRequestToolCall {
                                function: OllamaRequestFunctionCall {
                                    name: call.function.name.clone(),
                                    arguments: call.function.arguments.clone(),
                                },
                            });
                        }
                    }
                }

                let content = if text_parts.is_empty() {
                    None
                } else {
                    Some(text_parts.join("\n"))
                };
                messages.push(OllamaRequestMessage::Assistant {
                    content,
                    tool_calls,
                });
            }
        }
    }

    Ok(messages)
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
        let request_id = Uuid::now_v7().to_string();
        let response_id = format!("ollama-{request_id}");
        let compact_tool_schema = self.client.compact_tool_schema();
        let tools = ollama_tools_from_definitions(&request.tools, compact_tool_schema);
        let messages = build_ollama_messages(&request)?;
        let stream_events = request.stream_events.as_ref();

        let ollama_request = OllamaRequest {
            model: self.model.clone(),
            messages,
            stream: true,
            think: ollama_think_setting(request.thinking)?,
            options: if request.temperature.is_some() {
                Some(OllamaOptions {
                    temperature: request.temperature,
                })
            } else {
                None
            },
            tools,
        };

        let endpoint = native_chat_endpoint(&self.client.base_url);
        tracing::debug!(
            provider = "ollama",
            request_id = %request_id,
            response_id = %response_id,
            model = %self.model,
            endpoint = %endpoint,
            message_count = ollama_request.messages.len(),
            tool_count = ollama_request.tools.len(),
            think = ?ollama_request.think,
            compact_tool_schema,
            stream = ollama_request.stream,
            "sending Ollama streaming chat completion request"
        );

        let native_response = {
            let mut final_response = None;
            for attempt in 1..=OLLAMA_LOAD_RETRY_ATTEMPTS {
                let attempt_started = Instant::now();
                let mut req_builder = self
                    .client
                    .http_client
                    .post(&endpoint)
                    .header("x-request-id", request_id.as_str())
                    .json(&ollama_request);
                req_builder = self.client.provider.on_request(req_builder);

                let response = match req_builder.send().await {
                    Ok(response) => response,
                    Err(error) => {
                        tracing::warn!(
                            provider = "ollama",
                            request_id = %request_id,
                            model = %self.model,
                            endpoint = %endpoint,
                            attempt,
                            total_attempts = OLLAMA_LOAD_RETRY_ATTEMPTS,
                            elapsed_ms = attempt_started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                            timeout = error.is_timeout(),
                            connect = error.is_connect(),
                            error = %error,
                            "Ollama chat completion transport failed"
                        );
                        return Err(CompletionError::HttpError(error));
                    }
                };

                let status = response.status();
                tracing::debug!(
                    provider = "ollama",
                    request_id = %request_id,
                    model = %self.model,
                    endpoint = %endpoint,
                    attempt,
                    total_attempts = OLLAMA_LOAD_RETRY_ATTEMPTS,
                    status = status.as_u16(),
                    elapsed_ms = attempt_started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                    "received Ollama chat completion response headers"
                );

                if status.is_success() {
                    let response = read_ollama_stream_response(
                        response,
                        OllamaStreamReadContext {
                            request_id: &request_id,
                            model: &self.model,
                            endpoint: &endpoint,
                            attempt,
                            total_attempts: OLLAMA_LOAD_RETRY_ATTEMPTS,
                            attempt_started,
                            stream_events,
                        },
                    )
                    .await?;
                    final_response = Some(response);
                    break;
                }

                let response_text = match response.text().await {
                    Ok(response_text) => response_text,
                    Err(error) => {
                        tracing::warn!(
                            provider = "ollama",
                            request_id = %request_id,
                            model = %self.model,
                            endpoint = %endpoint,
                            attempt,
                            total_attempts = OLLAMA_LOAD_RETRY_ATTEMPTS,
                            elapsed_ms = attempt_started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                            timeout = error.is_timeout(),
                            connect = error.is_connect(),
                            error = %error,
                            "failed to read Ollama chat completion response body"
                        );
                        return Err(CompletionError::HttpError(error));
                    }
                };

                tracing::debug!(
                    provider = "ollama",
                    request_id = %request_id,
                    model = %self.model,
                    endpoint = %endpoint,
                    attempt,
                    total_attempts = OLLAMA_LOAD_RETRY_ATTEMPTS,
                    status = status.as_u16(),
                    elapsed_ms = attempt_started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                    response_body_bytes = response_text.len(),
                    "received Ollama chat completion response"
                );

                if should_retry_ollama_load_error(status, &response_text)
                    && attempt < OLLAMA_LOAD_RETRY_ATTEMPTS
                {
                    let delay = ollama_load_retry_delay(attempt);
                    tracing::warn!(
                        provider = "ollama",
                        request_id = %request_id,
                        model = %self.model,
                        endpoint = %endpoint,
                        attempt,
                        total_attempts = OLLAMA_LOAD_RETRY_ATTEMPTS,
                        status = status.as_u16(),
                        delay_ms = delay.as_millis().min(u128::from(u64::MAX)) as u64,
                        response_body = %truncate_for_log(&response_text, 2048),
                        "Ollama model appears to be loading; retrying request"
                    );
                    sleep(delay).await;
                    continue;
                }

                let error =
                    format_ollama_provider_error(status, &response_text, &self.model, &endpoint);
                tracing::warn!(
                    provider = "ollama",
                    request_id = %request_id,
                    model = %self.model,
                    endpoint = %endpoint,
                    status = status.as_u16(),
                    response_body = %truncate_for_log(&response_text, 2048),
                    error = %error,
                    "Ollama chat completion failed"
                );
                return Err(CompletionError::ProviderError(error));
            }

            final_response.ok_or_else(|| {
                CompletionError::ProviderError(format!(
                    "Ollama request failed: exhausted {} model-load retries for model '{}' at {}",
                    OLLAMA_LOAD_RETRY_ATTEMPTS, self.model, endpoint
                ))
            })?
        };

        let ollama_response = ollama_response_to_chat_response(native_response, response_id);
        tracing::debug!(
            provider = "ollama",
            request_id = %request_id,
            response_id = %ollama_response.id,
            model = %ollama_response.model,
            choice_count = ollama_response.choices.len(),
            "normalized Ollama chat completion response"
        );
        if let Some(choice) = ollama_response.choices.first()
            && let ChatMessage::Assistant {
                content,
                reasoning_content: _,
                tool_calls,
            } = &choice.message
        {
            if let Some(content) = content
                && !content.trim().is_empty()
            {
                tracing::debug!(
                    provider = "ollama",
                    request_id = %request_id,
                    response_id = %ollama_response.id,
                    model = %ollama_response.model,
                    content = %truncate_for_log(content, 4096),
                    "Ollama assistant response content"
                );
            }
            for (tool_index, tool_call) in tool_calls.iter().enumerate() {
                tracing::debug!(
                    provider = "ollama",
                    request_id = %request_id,
                    response_id = %ollama_response.id,
                    model = %ollama_response.model,
                    tool_index,
                    tool_call_id = %tool_call.id,
                    tool_name = %tool_call.function.name,
                    arguments = %truncate_for_log(&tool_call.function.arguments, 2048),
                    "Ollama assistant response tool call"
                );
            }
        }

        let choice = ollama_response
            .choices
            .first()
            .ok_or_else(|| CompletionError::ResponseError("No choices in response".to_string()))?;

        let content = match &choice.message {
            ChatMessage::Assistant {
                content,
                reasoning_content: _,
                tool_calls,
            } => {
                let mut parsed = Vec::new();
                if let Some(content) = content
                    && !content.trim().is_empty()
                {
                    parsed.push(AssistantContent::Text(
                        crate::internal::ai::completion::Text {
                            text: content.clone(),
                        },
                    ));
                }
                for call in tool_calls {
                    parsed.push(AssistantContent::ToolCall(
                        crate::internal::ai::completion::ToolCall {
                            id: call.id.clone(),
                            name: call.function.name.clone(),
                            function: Function {
                                name: call.function.name.clone(),
                                arguments: serde_json::from_str(&call.function.arguments)
                                    .unwrap_or(Value::String(call.function.arguments.clone())),
                            },
                        },
                    ));
                }
                parsed
            }
            _ => {
                return Err(CompletionError::ResponseError(
                    "Expected assistant message in Ollama response".to_string(),
                ));
            }
        };

        Ok(CompletionResponse {
            content,
            reasoning_content: None,
            raw_response: ollama_response,
        })
    }
}

impl CompletionClient for Client {
    type Model = Model;

    fn completion_model(&self, model: impl Into<String>) -> Self::Model {
        Model::new(self.clone(), model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::{
        completion::{AssistantContent, OneOrMany, ToolCall},
        providers::openai_compat::{ChatMessage, ChatResponse, parse_choice_content},
    };

    #[test]
    fn test_ollama_request_serialization() {
        let request = OllamaRequest {
            model: "llama3.2".to_string(),
            messages: vec![
                OllamaRequestMessage::System {
                    content: "You are a helpful assistant.".to_string(),
                },
                OllamaRequestMessage::User {
                    content: "Hello!".to_string(),
                },
            ],
            stream: true,
            think: Some(OllamaThink::Bool(false)),
            options: Some(OllamaOptions {
                temperature: Some(0.7),
            }),
            tools: Vec::new(),
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"model\":\"llama3.2\""));
        assert!(json.contains("\"stream\":true"));
        assert!(json.contains("\"think\":false"));
        assert!(json.contains("\"options\":{\"temperature\":0.7}"));
    }

    #[test]
    fn test_compact_tool_schema_removes_deep_definitions() {
        let tools = vec![ToolDefinition {
            name: "submit_intent_draft".to_string(),
            description: "Submit a structured IntentDraft".to_string(),
            parameters: json!({
                "type": "object",
                "required": ["draft"],
                "properties": {
                    "draft": {
                        "type": "object",
                        "required": ["intent", "acceptance", "risk"],
                        "properties": {
                            "intent": {
                                "type": "object",
                                "required": ["summary", "changeType"],
                                "properties": {
                                    "summary": {"type": "string", "description": "Summary"},
                                    "changeType": {
                                        "type": "string",
                                        "enum": ["bugfix", "feature", "refactor"]
                                    }
                                }
                            },
                            "acceptance": {
                                "type": "object",
                                "properties": {
                                    "fastChecks": {
                                        "type": "array",
                                        "items": {"$ref": "#/$defs/check"}
                                    }
                                }
                            },
                            "risk": {"type": "object"}
                        }
                    }
                },
                "$defs": {
                    "check": {
                        "type": "object",
                        "properties": {
                            "kind": {
                                "type": "string",
                                "enum": ["command", "testSuite", "policy"]
                            }
                        }
                    }
                }
            }),
        }];

        let compact = ollama_tools_from_definitions(&tools, true);
        let parameters = &compact[0].function.parameters;

        assert_eq!(parameters["type"], "object");
        assert_eq!(parameters["required"], json!(["draft"]));
        assert!(parameters.get("$defs").is_none());
        assert!(parameters.get("definitions").is_none());
        assert!(
            parameters
                .pointer("/properties/draft/properties/intent/properties/changeType/enum")
                .is_none()
        );
        assert_eq!(
            parameters
                .pointer("/properties/draft/properties/acceptance/properties/fastChecks/items")
                .unwrap(),
            &json!({"type": "object"})
        );
    }

    #[test]
    fn test_parse_ollama_think_setting() {
        assert_eq!(
            parse_ollama_think_setting("false").unwrap(),
            Some(OllamaThink::Bool(false))
        );
        assert_eq!(
            parse_ollama_think_setting("true").unwrap(),
            Some(OllamaThink::Bool(true))
        );
        assert_eq!(
            parse_ollama_think_setting("high").unwrap(),
            Some(OllamaThink::Level(OllamaThinkLevel::High))
        );
        assert_eq!(parse_ollama_think_setting("auto").unwrap(), None);
        assert!(parse_ollama_think_setting("invalid").is_err());
    }

    #[test]
    fn test_request_thinking_overrides_env_default() {
        assert_eq!(
            ollama_think_setting(Some(CompletionThinking::Auto)).unwrap(),
            None
        );
        assert_eq!(
            ollama_think_setting(Some(CompletionThinking::Disabled)).unwrap(),
            Some(OllamaThink::Bool(false))
        );
        assert_eq!(
            ollama_think_setting(Some(CompletionThinking::Enabled)).unwrap(),
            Some(OllamaThink::Bool(true))
        );
        assert_eq!(
            ollama_think_setting(Some(CompletionThinking::High)).unwrap(),
            Some(OllamaThink::Level(OllamaThinkLevel::High))
        );
    }

    #[test]
    fn test_aggregate_ollama_stream_text_chunks() {
        let response = aggregate_ollama_stream_lines([
            r#"{"model":"qwen3.6","created_at":"2026-04-17T13:04:26Z","message":{"role":"assistant","content":"hel"},"done":false}"#,
            r#"{"model":"qwen3.6","created_at":"2026-04-17T13:04:27Z","message":{"role":"assistant","content":"lo"},"done":false}"#,
            r#"{"model":"qwen3.6","created_at":"2026-04-17T13:04:28Z","message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","prompt_eval_count":3,"eval_count":2}"#,
        ])
        .unwrap();

        assert_eq!(response.model, "qwen3.6");
        assert_eq!(response.message.content.as_deref(), Some("hello"));
        assert_eq!(response.prompt_eval_count, Some(3));
        assert_eq!(response.eval_count, Some(2));
    }

    #[test]
    fn test_aggregate_ollama_stream_tool_call_preserves_id() {
        let response = aggregate_ollama_stream_lines([
            r#"{"model":"qwen3.6","created_at":"2026-04-17T13:04:30Z","message":{"role":"assistant","content":"","tool_calls":[{"id":"call_streamed","function":{"name":"echo","arguments":{"text":"hi"}}}]},"done":false}"#,
            r#"{"model":"qwen3.6","created_at":"2026-04-17T13:04:31Z","message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","prompt_eval_count":274,"eval_count":25}"#,
        ])
        .unwrap();
        let response = ollama_response_to_chat_response(response, "ollama-req".to_string());

        let ChatMessage::Assistant { tool_calls, .. } = &response.choices[0].message else {
            panic!("expected assistant message");
        };
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "call_streamed");
        assert_eq!(tool_calls[0].function.name, "echo");
        assert_eq!(tool_calls[0].function.arguments, r#"{"text":"hi"}"#);
    }

    #[test]
    fn test_streamed_tool_call_emits_preview_event() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let ctx = OllamaStreamReadContext {
            request_id: "req_preview",
            model: "qwen3.6",
            endpoint: "http://127.0.0.1:11434/api/chat",
            attempt: 1,
            total_attempts: 1,
            attempt_started: Instant::now(),
            stream_events: Some(&tx),
        };
        let mut accumulator = OllamaStreamAccumulator::default();

        process_ollama_stream_line(
            br#"{"model":"qwen3.6","created_at":"2026-04-17T13:04:30Z","message":{"role":"assistant","content":"","tool_calls":[{"id":"call_streamed","function":{"name":"echo","arguments":{"text":"hi"}}}]},"done":false}"#,
            &mut accumulator,
            &ctx,
        )
        .unwrap();

        let event = rx.try_recv().unwrap();
        match event {
            CompletionStreamEvent::ToolCallPreview {
                request_id,
                call_id,
                tool_name,
                arguments,
            } => {
                assert_eq!(request_id.as_deref(), Some("req_preview"));
                assert_eq!(call_id, "call_streamed");
                assert_eq!(tool_name, "echo");
                assert_eq!(arguments, serde_json::json!({"text": "hi"}));
            }
            other => panic!("expected tool preview event, got {other:?}"),
        }
    }

    #[test]
    fn test_streamed_thinking_emits_delta_event() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let ctx = OllamaStreamReadContext {
            request_id: "req_thinking",
            model: "glm-5.1",
            endpoint: "http://127.0.0.1:11434/api/chat",
            attempt: 1,
            total_attempts: 1,
            attempt_started: Instant::now(),
            stream_events: Some(&tx),
        };
        let mut accumulator = OllamaStreamAccumulator::default();

        process_ollama_stream_line(
            br#"{"model":"glm-5.1","created_at":"2026-04-17T13:04:30Z","message":{"role":"assistant","thinking":"checking repository state","content":""},"done":false}"#,
            &mut accumulator,
            &ctx,
        )
        .unwrap();

        let event = rx.try_recv().unwrap();
        match event {
            CompletionStreamEvent::ThinkingDelta { request_id, delta } => {
                assert_eq!(request_id.as_deref(), Some("req_thinking"));
                assert_eq!(delta, "checking repository state");
            }
            other => panic!("expected thinking delta event, got {other:?}"),
        }
    }

    #[test]
    fn test_ollama_tool_call_history_serializes_arguments_as_object() {
        let request = CompletionRequest {
            chat_history: vec![
                Message::user("Read Cargo.toml"),
                Message::Assistant {
                    id: None,
                    reasoning_content: None,
                    content: OneOrMany::One(AssistantContent::ToolCall(ToolCall {
                        id: "call_1".to_string(),
                        name: "read_file".to_string(),
                        function: Function {
                            name: "read_file".to_string(),
                            arguments: serde_json::json!({"file_path": "Cargo.toml"}),
                        },
                    })),
                },
                Message::User {
                    content: OneOrMany::One(UserContent::ToolResult(
                        crate::internal::ai::completion::ToolResult {
                            id: "call_1".to_string(),
                            name: "read_file".to_string(),
                            result: serde_json::json!("ok"),
                        },
                    )),
                },
            ],
            ..Default::default()
        };
        let messages = build_ollama_messages(&request).unwrap();
        let json = serde_json::to_value(&messages).unwrap();

        assert_eq!(
            json.pointer("/1/tool_calls/0/function/arguments/file_path")
                .and_then(serde_json::Value::as_str),
            Some("Cargo.toml")
        );
        assert!(
            json.pointer("/1/tool_calls/0/function/arguments")
                .is_some_and(serde_json::Value::is_object)
        );
    }

    #[test]
    fn test_ollama_response_deserialization() {
        let json = r#"
        {
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1677652288,
            "model": "llama3.2",
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
        assert_eq!(response.model, "llama3.2");
        assert_eq!(response.choices.len(), 1);
        assert_eq!(response.usage.unwrap().total_tokens, 21);
    }

    #[test]
    fn test_model_new() {
        let client = Client::new_local();
        let model = Model::new(client, "llama3.2");
        assert_eq!(model.model_name(), "llama3.2");
    }

    #[test]
    fn test_client_completion_model() {
        let client = Client::new_local();
        let model = client.completion_model("llama3.2");
        assert_eq!(model.model_name(), "llama3.2");
    }

    #[test]
    fn test_ollama_native_error_message_is_extracted() {
        let error = format_ollama_provider_error(
            StatusCode::SERVICE_UNAVAILABLE,
            r#"{"error":"model is loading, try again"}"#,
            "gemma4:31b",
            "http://127.0.0.1:11434/api/chat",
        );

        assert!(error.contains("status 503"));
        assert!(error.contains("gemma4:31b"));
        assert!(error.contains("model is loading, try again"));
        assert!(!error.contains(r#"{"error""#));
    }

    #[test]
    fn test_ollama_empty_503_error_is_actionable() {
        let error = format_ollama_provider_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "",
            "gemma4:31b",
            "http://127.0.0.1:11434/api/chat",
        );

        assert!(error.contains("status 503"));
        assert!(error.contains("empty response body"));
        assert!(error.contains("ollama ps"));
        assert!(error.contains("gemma4:31b"));
    }

    #[test]
    fn test_ollama_empty_503_is_load_retryable() {
        assert!(should_retry_ollama_load_error(
            StatusCode::SERVICE_UNAVAILABLE,
            ""
        ));
        assert!(should_retry_ollama_load_error(
            StatusCode::SERVICE_UNAVAILABLE,
            r#"{"error":"model is loading, try again"}"#
        ));
        assert!(!should_retry_ollama_load_error(
            StatusCode::BAD_REQUEST,
            r#"{"error":"bad request"}"#
        ));
        assert!(!should_retry_ollama_load_error(
            StatusCode::SERVICE_UNAVAILABLE,
            r#"{"error":"model requires more system memory"}"#
        ));
    }

    /// Verify that Ollama's real response format (with extra fields like
    /// `system_fingerprint` and `reasoning`) deserializes correctly.
    #[test]
    fn test_ollama_real_response_deserialization() {
        let json = r#"
        {
            "id": "chatcmpl-394",
            "object": "chat.completion",
            "created": 1772113825,
            "model": "llama3.2",
            "system_fingerprint": "fp_ollama",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hello! I hope you're having a wonderful day.",
                    "reasoning": "User asks to say hello."
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 73,
                "completion_tokens": 38,
                "total_tokens": 111
            }
        }
        "#;

        let response: ChatResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.model, "llama3.2");

        let content = parse_choice_content(&response.choices[0]).unwrap();
        assert_eq!(content.len(), 1);
        match &content[0] {
            AssistantContent::Text(t) => {
                assert!(t.text.contains("wonderful day"));
            }
            _ => panic!("Expected text content"),
        }
    }

    #[test]
    fn test_ollama_native_response_conversion() {
        let json = r#"
        {
            "model": "gemma4:31b",
            "created_at": "2026-04-15T17:24:23.159403Z",
            "message": {
                "role": "assistant",
                "content": "done",
                "tool_calls": [{
                    "id": "call_native",
                    "function": {
                        "name": "read_file",
                        "arguments": {"path": "Cargo.toml"}
                    }
                }]
            },
            "done": true,
            "prompt_eval_count": 12,
            "eval_count": 7
        }
        "#;

        let response: OllamaResponse = serde_json::from_str(json).unwrap();
        let response =
            ollama_response_to_chat_response(response, "ollama-test-request".to_string());

        assert_eq!(response.id, "ollama-test-request");
        assert_eq!(response.model, "gemma4:31b");
        assert_eq!(response.usage.unwrap().total_tokens, 19);
        let content = match &response.choices[0].message {
            ChatMessage::Assistant {
                content,
                reasoning_content: _,
                tool_calls,
            } => {
                assert_eq!(content.as_deref(), Some("done"));
                assert_eq!(tool_calls.len(), 1);
                assert_eq!(tool_calls[0].id, "call_native");
                assert_eq!(tool_calls[0].function.name, "read_file");
                assert_eq!(tool_calls[0].function.arguments, r#"{"path":"Cargo.toml"}"#);
                content
            }
            _ => panic!("Expected assistant message"),
        };
        assert_eq!(content.as_deref(), Some("done"));
    }

    /// Integration test: actually calls the local Ollama instance.
    /// Uses `OLLAMA_TEST_MODEL` env var or defaults to `llama3.2`.
    ///
    /// Run with: `OLLAMA_TEST_MODEL=llama3.2 cargo test -- --ignored`
    #[tokio::test]
    async fn test_ollama_live_completion() {
        if std::env::var("OLLAMA_TEST_MODEL").map_or(true, |v| v.is_empty()) {
            eprintln!("skipped (OLLAMA_TEST_MODEL not set)");
            return;
        }

        // Quick check if Ollama is available
        let client = reqwest::Client::new();
        if client
            .get("http://127.0.0.1:11434/v1/models")
            .send()
            .await
            .is_err()
        {
            eprintln!("skipped (Ollama not running on 127.0.0.1:11434)");
            return;
        }

        let model_name =
            std::env::var("OLLAMA_TEST_MODEL").unwrap_or_else(|_| "llama3.2".to_string());

        let ollama_client = Client::new_local();
        let model = ollama_client.completion_model(&model_name);

        let request = CompletionRequest {
            preamble: Some("Reply concisely.".to_string()),
            chat_history: vec![crate::internal::ai::completion::Message::user(
                "What is 2+3? Reply with just the number.",
            )],
            temperature: Some(0.0),
            tools: vec![],
            documents: vec![],
            thinking: None,
            reasoning_effort: None,
            stream: None,
            stream_events: None,
        };

        let response = model.completion(request).await;

        // Skip gracefully if the requested model isn't pulled locally.
        if let Err(ref e) = response {
            let msg = format!("{e:?}");
            if msg.contains("not found") {
                println!(
                    "Skipping: model '{model_name}' not found in local Ollama (pull it or set OLLAMA_TEST_MODEL)"
                );
                return;
            }
        }

        assert!(response.is_ok(), "Completion failed: {:?}", response);

        let response = response.unwrap();
        assert!(!response.content.is_empty(), "No content in response");

        match &response.content[0] {
            AssistantContent::Text(t) => {
                println!("Ollama response: {}", t.text);
                assert!(
                    t.text.contains('5'),
                    "Expected answer to contain '5': {}",
                    t.text
                );
            }
            _ => panic!("Expected text content"),
        }
    }

    #[tokio::test]
    async fn test_ollama_live_tool_call_history_completion() {
        if std::env::var("OLLAMA_TEST_MODEL").map_or(true, |v| v.is_empty()) {
            eprintln!("skipped (OLLAMA_TEST_MODEL not set)");
            return;
        }

        let client = reqwest::Client::new();
        if client
            .get("http://127.0.0.1:11434/v1/models")
            .send()
            .await
            .is_err()
        {
            eprintln!("skipped (Ollama not running on 127.0.0.1:11434)");
            return;
        }

        let model_name =
            std::env::var("OLLAMA_TEST_MODEL").unwrap_or_else(|_| "llama3.2".to_string());

        let ollama_client = Client::new_local();
        let model = ollama_client.completion_model(&model_name);

        let request = CompletionRequest {
            preamble: Some("Reply concisely.".to_string()),
            chat_history: vec![
                Message::user("Read Cargo.toml, then say OK."),
                Message::Assistant {
                    id: None,
                    reasoning_content: None,
                    content: OneOrMany::One(AssistantContent::ToolCall(ToolCall {
                        id: "call_1".to_string(),
                        name: "read_file".to_string(),
                        function: Function {
                            name: "read_file".to_string(),
                            arguments: serde_json::json!({"file_path": "Cargo.toml"}),
                        },
                    })),
                },
                Message::User {
                    content: OneOrMany::One(UserContent::ToolResult(
                        crate::internal::ai::completion::ToolResult {
                            id: "call_1".to_string(),
                            name: "read_file".to_string(),
                            result: serde_json::json!("ok"),
                        },
                    )),
                },
            ],
            temperature: Some(0.0),
            tools: vec![crate::internal::ai::tools::ToolDefinition {
                name: "read_file".to_string(),
                description: "Read file".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "file_path": {"type": "string"}
                    },
                    "required": ["file_path"]
                }),
            }],
            documents: vec![],
            thinking: None,
            reasoning_effort: None,
            stream: None,
            stream_events: None,
        };

        let response = model.completion(request).await;
        assert!(response.is_ok(), "Completion failed: {:?}", response);
    }
}
