//! Ollama completion model implementation.
//!
//! Libra sends requests to Ollama's native `/api/chat` endpoint and converts
//! the response into the shared OpenAI-compatible internal chat shape.

use chrono::DateTime;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::time::{Duration, sleep};

use crate::internal::ai::{
    client::{CompletionClient, Provider},
    completion::{
        AssistantContent, CompletionError, CompletionModel as CompletionModelTrait, Function,
        Message, UserContent,
        request::{CompletionRequest, CompletionResponse},
    },
    providers::{
        ollama::client::Client,
        openai_compat::{
            ChatChoice, ChatErrorResponse, ChatFunctionCall, ChatMessage, ChatResponse,
            ChatToolCall, ChatToolDefinition, ChatUsage, parse_tools,
        },
    },
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
    options: Option<OllamaOptions>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ChatToolDefinition>,
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
    tool_calls: Vec<OllamaToolCall>,
}

#[derive(Debug, Deserialize)]
struct OllamaToolCall {
    function: OllamaFunctionCall,
}

#[derive(Debug, Deserialize)]
struct OllamaFunctionCall {
    name: String,
    arguments: Value,
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

fn ollama_response_to_chat_response(response: OllamaResponse) -> ChatResponse {
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
            id: format!("call_{index}"),
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
        id: format!("ollama-{}", response.created_at),
        object: "chat.completion".to_string(),
        created,
        model: response.model,
        choices: vec![ChatChoice {
            index: 0,
            message: ChatMessage::Assistant {
                content,
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
        let tools = parse_tools(&request.tools);
        let messages = build_ollama_messages(&request)?;

        let ollama_request = OllamaRequest {
            model: self.model.clone(),
            messages,
            stream: false,
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
            model = %self.model,
            endpoint = %endpoint,
            message_count = ollama_request.messages.len(),
            tool_count = ollama_request.tools.len(),
            "sending Ollama chat completion request"
        );

        let response_text = {
            let mut final_response = None;
            for attempt in 1..=OLLAMA_LOAD_RETRY_ATTEMPTS {
                let mut req_builder = self
                    .client
                    .http_client
                    .post(&endpoint)
                    .json(&ollama_request);
                req_builder = self.client.provider.on_request(req_builder);

                let response = req_builder
                    .send()
                    .await
                    .map_err(CompletionError::HttpError)?;

                let status = response.status();
                let response_text = response.text().await.map_err(CompletionError::HttpError)?;

                tracing::debug!(
                    provider = "ollama",
                    model = %self.model,
                    endpoint = %endpoint,
                    attempt,
                    total_attempts = OLLAMA_LOAD_RETRY_ATTEMPTS,
                    status = status.as_u16(),
                    response_body_bytes = response_text.len(),
                    "received Ollama chat completion response"
                );

                if status.is_success() {
                    final_response = Some(response_text);
                    break;
                }

                if should_retry_ollama_load_error(status, &response_text)
                    && attempt < OLLAMA_LOAD_RETRY_ATTEMPTS
                {
                    let delay = ollama_load_retry_delay(attempt);
                    tracing::warn!(
                        provider = "ollama",
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

        let ollama_response: OllamaResponse =
            serde_json::from_str(&response_text).map_err(CompletionError::JsonError)?;
        let ollama_response = ollama_response_to_chat_response(ollama_response);

        let choice = ollama_response
            .choices
            .first()
            .ok_or_else(|| CompletionError::ResponseError("No choices in response".to_string()))?;

        let content = match &choice.message {
            ChatMessage::Assistant {
                content,
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
            stream: false,
            options: Some(OllamaOptions {
                temperature: Some(0.7),
            }),
            tools: Vec::new(),
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"model\":\"llama3.2\""));
        assert!(json.contains("\"stream\":false"));
        assert!(json.contains("\"options\":{\"temperature\":0.7}"));
    }

    #[test]
    fn test_ollama_tool_call_history_serializes_arguments_as_object() {
        let request = CompletionRequest {
            chat_history: vec![
                Message::user("Read Cargo.toml"),
                Message::Assistant {
                    id: None,
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
        let response = ollama_response_to_chat_response(response);

        assert_eq!(response.model, "gemma4:31b");
        assert_eq!(response.usage.unwrap().total_tokens, 19);
        let content = match &response.choices[0].message {
            ChatMessage::Assistant {
                content,
                tool_calls,
            } => {
                assert_eq!(content.as_deref(), Some("done"));
                assert_eq!(tool_calls.len(), 1);
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
        };

        let response = model.completion(request).await;
        assert!(response.is_ok(), "Completion failed: {:?}", response);
    }
}
