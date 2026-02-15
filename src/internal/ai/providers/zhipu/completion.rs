//! Zhipu completion model implementation.

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

/// Zhipu chat completion request.
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

/// Zhipu message format.
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

/// Zhipu tool choice.
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum ZhipuToolChoice {
    Mode(ZhipuToolChoiceMode),
    Function(ZhipuFunctionToolChoice),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ZhipuToolChoiceMode {
    Auto,
    None,
    Required,
}

#[derive(Debug, Serialize, Deserialize)]
struct ZhipuFunctionToolChoice {
    #[serde(rename = "type")]
    r#type: String,
    function: ZhipuToolChoiceFunction,
}

#[derive(Debug, Serialize, Deserialize)]
struct ZhipuToolChoiceFunction {
    name: String,
}

/// Zhipu tool definition.
#[derive(Debug, Serialize, Deserialize)]
struct ZhipuToolDefinition {
    r#type: String,
    function: ZhipuFunctionDefinition,
}

/// Zhipu function definition.
#[derive(Debug, Serialize, Deserialize)]
struct ZhipuFunctionDefinition {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

/// Zhipu tool call.
#[derive(Debug, Serialize, Deserialize)]
struct ZhipuToolCall {
    id: String,
    r#type: String,
    function: ZhipuFunctionCall,
}

/// Zhipu function call.
#[derive(Debug, Serialize, Deserialize)]
struct ZhipuFunctionCall {
    name: String,
    arguments: String,
}

/// Zhipu choice.
#[derive(Debug, Serialize, Deserialize)]
struct ZhipuChoice {
    index: usize,
    message: ZhipuMessage,
    finish_reason: Option<String>,
}

/// Zhipu usage.
#[derive(Debug, Serialize, Deserialize)]
struct ZhipuUsage {
    prompt_tokens: usize,
    completion_tokens: usize,
    total_tokens: usize,
}

/// Zhipu chat completion response.
#[derive(Debug, Serialize, Deserialize)]
pub struct ZhipuResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    choices: Vec<ZhipuChoice>,
    usage: Option<ZhipuUsage>,
}

/// Zhipu API error response.
#[derive(Debug, Deserialize)]
struct ZhipuError {
    message: String,
}

/// Zhipu API error wrapper.
#[derive(Debug, Deserialize)]
struct ZhipuErrorResponse {
    error: ZhipuError,
}

// ================================================================
// Conversions
// ================================================================

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
