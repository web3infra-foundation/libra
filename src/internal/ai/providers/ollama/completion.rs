//! Ollama completion model implementation.
//!
//! Ollama exposes an OpenAI-compatible chat completions API, so the request and
//! response types mirror the OpenAI format.

use serde::{Deserialize, Serialize};

use crate::internal::ai::{
    client::{CompletionClient, Provider},
    completion::{
        AssistantContent, CompletionError, CompletionModel as CompletionModelTrait, Function,
        Message, Text, ToolCall, UserContent,
        request::{CompletionRequest, CompletionResponse},
    },
    providers::ollama::client::Client,
    tools::ToolDefinition,
};

/// Ollama completion model.
#[derive(Clone, Debug)]
pub struct Model {
    client: Client,
    model: String,
}

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
// OpenAI-compatible API Types (used by Ollama)
// ================================================================

/// Top-level request body sent to the Ollama `/v1/chat/completions` endpoint.
#[derive(Debug, Serialize)]
struct OllamaRequest {
    model: String,
    messages: Vec<OllamaMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OllamaToolDefinition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<OllamaToolChoice>,
}

/// A single message in the chat conversation, tagged by its role.
///
/// Serialized with `#[serde(tag = "role")]` so the JSON representation
/// includes a `"role"` field (e.g. `"system"`, `"user"`, `"assistant"`,
/// `"tool"`) matching the OpenAI chat format.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
enum OllamaMessage {
    System {
        content: String,
    },
    User {
        content: String,
    },
    Assistant {
        content: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<OllamaToolCall>,
    },
    Tool {
        tool_call_id: String,
        name: String,
        content: String,
    },
}

/// Controls how the model selects tool calls.
///
/// Serialized as an untagged enum so that the mode string (e.g. `"auto"`) is
/// emitted directly rather than wrapped in an object.
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum OllamaToolChoice {
    Mode(OllamaToolChoiceMode),
}

/// Available tool-choice modes: `auto` lets the model decide, `none` disables
/// tool calling entirely.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OllamaToolChoiceMode {
    Auto,
    None,
}

/// An OpenAI-compatible tool definition (always `type: "function"`).
#[derive(Debug, Serialize, Deserialize)]
struct OllamaToolDefinition {
    r#type: String,
    function: OllamaFunctionDefinition,
}

/// Schema describing a callable function: its name, description, and JSON
/// Schema parameters.
#[derive(Debug, Serialize, Deserialize)]
struct OllamaFunctionDefinition {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

/// A tool call emitted by the assistant in a response message.
#[derive(Debug, Serialize, Deserialize)]
struct OllamaToolCall {
    id: String,
    r#type: String,
    function: OllamaFunctionCall,
}

/// The function name and its JSON-encoded arguments string within a tool call.
#[derive(Debug, Serialize, Deserialize)]
struct OllamaFunctionCall {
    name: String,
    arguments: String,
}

/// A single completion choice returned by the API. Ollama typically returns
/// exactly one choice.
#[derive(Debug, Serialize, Deserialize)]
struct OllamaChoice {
    index: usize,
    message: OllamaMessage,
    finish_reason: Option<String>,
}

/// Token usage statistics returned alongside the completion.
#[derive(Debug, Serialize, Deserialize)]
struct OllamaUsage {
    prompt_tokens: usize,
    completion_tokens: usize,
    total_tokens: usize,
}

/// Ollama chat completion response (OpenAI-compatible format).
#[derive(Debug, Serialize, Deserialize)]
pub struct OllamaResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    choices: Vec<OllamaChoice>,
    usage: Option<OllamaUsage>,
}

/// Inner error payload containing the human-readable error message.
#[derive(Debug, Deserialize)]
struct OllamaError {
    message: String,
}

/// Wrapper for error responses returned by the Ollama API on non-2xx status codes.
#[derive(Debug, Deserialize)]
struct OllamaErrorResponse {
    error: OllamaError,
}

// ================================================================
// CompletionModel Implementation
// ================================================================

impl CompletionModelTrait for Model {
    type Response = OllamaResponse;

    async fn completion(
        &self,
        request: CompletionRequest,
    ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
        let tools = parse_tools(&request.tools);
        let messages = build_messages(&request)?;

        let ollama_request = OllamaRequest {
            model: self.model.clone(),
            messages,
            temperature: request.temperature,
            // Only set tool_choice when tools are present; omitting it entirely
            // avoids confusing models that do not support function calling.
            tool_choice: if tools.is_empty() {
                None
            } else {
                Some(OllamaToolChoice::Mode(OllamaToolChoiceMode::Auto))
            },
            tools,
        };

        let mut req_builder = self
            .client
            .http_client
            .post(format!("{}/chat/completions", self.client.base_url))
            .json(&ollama_request);
        req_builder = self.client.provider.on_request(req_builder);

        let response = req_builder
            .send()
            .await
            .map_err(CompletionError::HttpError)?;

        let status = response.status();
        let response_text = response.text().await.map_err(CompletionError::HttpError)?;

        if !status.is_success() {
            if let Ok(error_response) = serde_json::from_str::<OllamaErrorResponse>(&response_text)
            {
                return Err(CompletionError::ProviderError(error_response.error.message));
            }
            return Err(CompletionError::ProviderError(response_text));
        }

        let ollama_response: OllamaResponse =
            serde_json::from_str(&response_text).map_err(CompletionError::JsonError)?;

        let choice = ollama_response
            .choices
            .first()
            .ok_or_else(|| CompletionError::ResponseError("No choices in response".to_string()))?;

        let content = parse_choice_content(choice)?;

        Ok(CompletionResponse {
            content,
            raw_response: ollama_response,
        })
    }
}

/// Converts generic [`ToolDefinition`] items into the OpenAI-compatible
/// function-calling format expected by Ollama.
///
/// Each tool is wrapped in an [`OllamaToolDefinition`] with `type: "function"`,
/// copying its name, description, and JSON Schema parameters verbatim.
fn parse_tools(tools: &[ToolDefinition]) -> Vec<OllamaToolDefinition> {
    tools
        .iter()
        .map(|tool| OllamaToolDefinition {
            r#type: "function".to_string(),
            function: OllamaFunctionDefinition {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: tool.parameters.clone(),
            },
        })
        .collect()
}

/// Converts a [`CompletionRequest`] into the flat list of [`OllamaMessage`]s
/// required by the OpenAI-compatible chat API.
///
/// The conversion handles:
/// - An optional system preamble, emitted as the first `System` message.
/// - `User` messages: text items become `User` messages, tool results become
///   `Tool` messages (with their call ID and name), and image content is
///   rejected with [`CompletionError::NotImplemented`].
/// - `Assistant` messages: text parts are joined and tool calls are mapped to
///   [`OllamaToolCall`] entries.
/// - `System` messages from the chat history: text items are concatenated
///   into a single `System` message.
fn build_messages(request: &CompletionRequest) -> Result<Vec<OllamaMessage>, CompletionError> {
    let mut messages = Vec::new();

    if let Some(preamble) = &request.preamble {
        messages.push(OllamaMessage::System {
            content: preamble.clone(),
        });
    }

    for msg in &request.chat_history {
        match msg {
            Message::User { content } => {
                for item in content.iter() {
                    match item {
                        UserContent::Text(t) => messages.push(OllamaMessage::User {
                            content: t.text.clone(),
                        }),
                        UserContent::ToolResult(tool_result) => {
                            let content = serde_json::to_string(&tool_result.result)
                                .unwrap_or_else(|_| tool_result.result.to_string());
                            messages.push(OllamaMessage::Tool {
                                tool_call_id: tool_result.id.clone(),
                                name: tool_result.name.clone(),
                                content,
                            });
                        }
                        UserContent::Image(_) => {
                            return Err(CompletionError::NotImplemented(
                                "Image content not implemented for Ollama provider".into(),
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
                            tool_calls.push(OllamaToolCall {
                                id: call.id.clone(),
                                r#type: "function".to_string(),
                                function: OllamaFunctionCall {
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

                messages.push(OllamaMessage::Assistant {
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
                messages.push(OllamaMessage::System { content: text });
            }
        }
    }

    Ok(messages)
}

/// Extracts [`AssistantContent`] items (text and tool calls) from a single
/// [`OllamaChoice`].
///
/// Non-empty text is emitted as [`AssistantContent::Text`]. Each tool call is
/// deserialized from its JSON arguments string back into a [`serde_json::Value`]
/// and emitted as [`AssistantContent::ToolCall`]. Returns an error if the
/// choice message is not an assistant message.
fn parse_choice_content(choice: &OllamaChoice) -> Result<Vec<AssistantContent>, CompletionError> {
    match &choice.message {
        OllamaMessage::Assistant {
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
                // Parse the JSON-encoded arguments string back into a Value.
                // If parsing fails (e.g. malformed JSON from the model), fall
                // back to wrapping the raw string as a JSON string value.
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
            "Unexpected non-assistant message in Ollama response".to_string(),
        )),
    }
}

/// Ensures tool-call arguments are serialized as a JSON string.
///
/// The OpenAI chat format requires `arguments` to be a JSON-encoded string,
/// but internally libra stores them as a [`serde_json::Value`]. This function
/// handles two cases:
/// - If the value is already a `String` that parses as valid JSON, it is
///   returned as-is (avoiding double-encoding).
/// - Otherwise the value is serialized with [`serde_json::Value::to_string`].
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
    fn test_ollama_request_serialization() {
        let request = OllamaRequest {
            model: "gpt-oss:120b".to_string(),
            messages: vec![
                OllamaMessage::System {
                    content: "You are a helpful assistant.".to_string(),
                },
                OllamaMessage::User {
                    content: "Hello!".to_string(),
                },
            ],
            temperature: Some(0.7),
            tools: Vec::new(),
            tool_choice: None,
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"model\":\"gpt-oss:120b\""));
        assert!(json.contains("\"temperature\":0.7"));
    }

    #[test]
    fn test_ollama_response_deserialization() {
        let json = r#"
        {
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1677652288,
            "model": "gpt-oss:120b",
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

        let response: OllamaResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.id, "chatcmpl-123");
        assert_eq!(response.model, "gpt-oss:120b");
        assert_eq!(response.choices.len(), 1);
        assert_eq!(response.usage.unwrap().total_tokens, 21);
    }

    #[test]
    fn test_model_new() {
        let client = Client::new_local();
        let model = Model::new(client, "gpt-oss:120b");
        assert_eq!(model.model_name(), "gpt-oss:120b");
    }

    #[test]
    fn test_client_completion_model() {
        let client = Client::new_local();
        let model = client.completion_model("gpt-oss:120b");
        assert_eq!(model.model_name(), "gpt-oss:120b");
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
            "model": "gpt-oss:120b",
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

        let response: OllamaResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.model, "gpt-oss:120b");

        let content = parse_choice_content(&response.choices[0]).unwrap();
        assert_eq!(content.len(), 1);
        match &content[0] {
            AssistantContent::Text(t) => {
                assert!(t.text.contains("wonderful day"));
            }
            _ => panic!("Expected text content"),
        }
    }

    /// Integration test: actually calls the local Ollama instance.
    /// Skipped if Ollama is not running.
    #[tokio::test]
    async fn test_ollama_live_completion() {
        // Quick check if Ollama is available
        let client = reqwest::Client::new();
        if client
            .get("http://localhost:11434/v1/models")
            .send()
            .await
            .is_err()
        {
            println!("Skipping: Ollama not running on localhost:11434");
            return;
        }

        let ollama_client = Client::new_local();
        let model = ollama_client.completion_model("gpt-oss:120b");

        let request = CompletionRequest {
            preamble: Some("Reply concisely.".to_string()),
            chat_history: vec![Message::user("What is 2+3? Reply with just the number.")],
            temperature: Some(0.0),
            tools: vec![],
            documents: vec![],
        };

        let response = model.completion(request).await;
        assert!(response.is_ok(), "Completion failed: {:?}", response.err());

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
}
