//! Ollama completion model implementation.
//!
//! Ollama exposes an OpenAI-compatible chat completions API. Common wire types
//! and helpers are imported from [`openai_compat`](super::super::openai_compat).

use serde::{Deserialize, Serialize};

use crate::internal::ai::{
    client::{CompletionClient, Provider},
    completion::{
        CompletionError, CompletionModel as CompletionModelTrait,
        request::{CompletionRequest, CompletionResponse},
    },
    providers::{
        ollama::client::Client,
        openai_compat::{
            ChatErrorResponse, ChatMessage, ChatResponse, ChatToolDefinition, build_messages,
            parse_choice_content, parse_tools,
        },
    },
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
// Ollama-specific Request / ToolChoice Types
// ================================================================

#[derive(Debug, Serialize)]
struct OllamaRequest {
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ChatToolDefinition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<OllamaToolChoiceMode>,
}

/// Ollama only supports `auto` and `none` tool-choice modes.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OllamaToolChoiceMode {
    Auto,
    None,
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

        let ollama_request = OllamaRequest {
            model: self.model.clone(),
            messages,
            temperature: request.temperature,
            tool_choice: if tools.is_empty() {
                None
            } else {
                Some(OllamaToolChoiceMode::Auto)
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
            if let Ok(error_response) = serde_json::from_str::<ChatErrorResponse>(&response_text) {
                return Err(CompletionError::ProviderError(error_response.error.message));
            }
            return Err(CompletionError::ProviderError(response_text));
        }

        let ollama_response: ChatResponse =
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
        completion::AssistantContent,
        providers::openai_compat::{ChatMessage, ChatResponse, parse_choice_content},
    };

    #[test]
    fn test_ollama_request_serialization() {
        let request = OllamaRequest {
            model: "llama3.2".to_string(),
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
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"model\":\"llama3.2\""));
        assert!(json.contains("\"temperature\":0.7"));
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

    /// Integration test: actually calls the local Ollama instance.
    /// Uses `OLLAMA_TEST_MODEL` env var or defaults to `llama3.2`.
    ///
    /// Run with: `OLLAMA_TEST_MODEL=llama3.2 cargo test -- --ignored`
    #[tokio::test]
    #[ignore = "requires a local Ollama instance"]
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
