//! DeepSeek completion model implementation.
//!
//! DeepSeek exposes an OpenAI-compatible Chat Completions endpoint. One notable
//! difference is that requests always set `stream: false` explicitly. Common
//! wire types and helpers are imported from [`openai_compat`](super::super::openai_compat).

use serde::{Deserialize, Serialize};

use crate::internal::ai::{
    client::{CompletionClient, Provider},
    completion::{
        CompletionError, CompletionModel as CompletionModelTrait,
        request::{CompletionRequest, CompletionResponse},
    },
    providers::{
        deepseek::client::Client,
        openai_compat::{
            ChatErrorResponse, ChatMessage, ChatResponse, ChatToolDefinition, build_messages,
            parse_choice_content, parse_tools,
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
}

// ================================================================
// DeepSeek-specific Request / ToolChoice Types
// ================================================================

/// DeepSeek request body. Identical to OpenAI except for the `stream` field
/// which is always set to `false`.
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
    /// Always `false` -- streaming is not used by this provider.
    stream: bool,
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
        let messages = build_messages(&request)?;

        let deepseek_request = DeepSeekRequest {
            model: self.model.clone(),
            messages,
            temperature: request.temperature,
            tool_choice: if tools.is_empty() {
                None
            } else {
                Some(DeepSeekToolChoice::Auto)
            },
            tools,
            stream: false,
        };

        let mut req_builder = self
            .client
            .http_client
            .post(format!("{}/chat/completions", self.client.base_url))
            .json(&deepseek_request);
        req_builder = self.client.provider.on_request(req_builder);

        let response = req_builder
            .send()
            .await
            .map_err(CompletionError::HttpError)?;

        let status = response.status();
        let response_text = response.text().await.map_err(CompletionError::HttpError)?;

        if !status.is_success() {
            if let Ok(error_response) = serde_json::from_str::<ChatErrorResponse>(&response_text) {
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

        let deepseek_response: ChatResponse =
            serde_json::from_str(&response_text).map_err(CompletionError::JsonError)?;

        let choice = deepseek_response
            .choices
            .first()
            .ok_or_else(|| CompletionError::ResponseError("No choices in response".to_string()))?;

        let content = parse_choice_content(choice)?;

        Ok(CompletionResponse {
            content,
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
            stream: false,
        };

        let json = serde_json::to_value(request).unwrap();
        assert_eq!(json["tool_choice"], "auto");
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
