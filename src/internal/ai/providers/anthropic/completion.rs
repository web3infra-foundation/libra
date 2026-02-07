//! Anthropic completion model implementation.

use serde::{Deserialize, Serialize};

use crate::internal::ai::{
    client::{CompletionClient, Provider},
    completion::{
        AssistantContent, CompletionError, CompletionModel as CompletionModelTrait, Function,
        Message, Text, ToolCall, UserContent,
        request::{CompletionRequest, CompletionResponse},
    },
    providers::anthropic::client::Client,
    tools::ToolDefinition,
};

/// Anthropic completion model.
#[derive(Clone, Debug)]
pub struct Model {
    client: Client,
    model: String,
}

impl Model {
    /// Creates a new Anthropic completion model.
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
// Anthropic API Types
// ================================================================

/// Anthropic messages API request.
#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    messages: Vec<AnthropicMessage>,
    max_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<AnthropicToolDefinition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<AnthropicToolChoice>,
}

/// Anthropic message.
#[derive(Debug, Serialize, Deserialize)]
struct AnthropicMessage {
    role: String,
    content: AnthropicContent,
}

/// Anthropic content - can be a string or array of content blocks.
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum AnthropicContent {
    String(String),
    Array(Vec<AnthropicContentBlock>),
}

/// Anthropic content block.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicContentBlock {
    Text {
        text: String,
    },
    Image {
        source: AnthropicImageSource,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
}

/// Anthropic image source.
#[derive(Debug, Serialize, Deserialize)]
struct AnthropicImageSource {
    r#type: String,
    media_type: String,
    data: String,
}

/// Anthropic tool definition.
#[derive(Debug, Serialize, Deserialize)]
struct AnthropicToolDefinition {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

/// Anthropic tool choice.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicToolChoice {
    Auto,
    Any,
    None,
    Tool { name: String },
}

/// Anthropic usage.
#[derive(Debug, Serialize, Deserialize)]
struct AnthropicUsage {
    input_tokens: u64,
    output_tokens: u64,
}

/// Anthropic messages API response.
#[derive(Debug, Serialize, Deserialize)]
pub struct AnthropicResponse {
    pub id: String,
    pub r#type: String,
    pub role: String,
    content: Vec<AnthropicContentBlock>,
    pub model: String,
    pub stop_reason: Option<String>,
    usage: AnthropicUsage,
}

/// Anthropic API error response.
#[derive(Debug, Deserialize)]
struct AnthropicError {
    message: String,
}

/// Anthropic API error wrapper.
#[derive(Debug, Deserialize)]
struct AnthropicErrorResponse {
    error: AnthropicError,
}

// ================================================================
// CompletionModel Implementation
// ================================================================

impl CompletionModelTrait for Model {
    type Response = AnthropicResponse;

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
                return Err(CompletionError::ProviderError(error_response.error.message));
            }
            return Err(CompletionError::ProviderError(response_text));
        }

        let anthropic_response: AnthropicResponse =
            serde_json::from_str(&response_text).map_err(CompletionError::JsonError)?;

        let content = parse_response(&anthropic_response);

        Ok(CompletionResponse {
            content,
            raw_response: anthropic_response,
        })
    }
}

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
                let content = if content_blocks.len() == 1 {
                    // Single content block can be a string
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
                // Anthropic requires at least one content block
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

fn parse_response(response: &AnthropicResponse) -> Vec<AssistantContent> {
    let mut parts = Vec::new();

    for block in &response.content {
        match block {
            AnthropicContentBlock::Text { text } => {
                if !text.trim().is_empty() {
                    parts.push(AssistantContent::Text(Text { text: text.clone() }));
                }
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
            _ => {}
        }
    }
    parts
}

/// Calculate default max_tokens based on the model.
fn calculate_max_tokens(model: &str) -> u64 {
    if model.starts_with("claude-opus-4") {
        32000
    } else if model.starts_with("claude-sonnet-4") || model.starts_with("claude-3-7-sonnet") {
        64000
    } else if model.starts_with("claude-3-5-sonnet") || model.starts_with("claude-3-5-haiku") {
        8192
    } else {
        4096 // Default fallback
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

    #[test]
    fn test_model_new() {
        let client = Client::with_api_key("sk-ant-test-key".to_string());
        let model = Model::new(client, "claude-3-5-sonnet-latest");
        assert_eq!(model.model_name(), "claude-3-5-sonnet-latest");
    }

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

    #[test]
    fn test_client_completion_model() {
        let client = Client::with_api_key("sk-ant-test-key".to_string());
        let model = client.completion_model("claude-3-5-sonnet-latest");
        assert_eq!(model.model_name(), "claude-3-5-sonnet-latest");
    }

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
