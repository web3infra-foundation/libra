//! Kimi (Moonshot AI) completion model implementation.
//!
//! Kimi exposes an OpenAI-compatible Chat Completions endpoint, so common wire
//! types and helpers are imported from
//! [`openai_compat`](super::super::openai_compat). This file only defines the
//! provider-specific request body and tool-choice shape.

use serde::{Deserialize, Serialize};

use crate::internal::ai::{
    client::{CompletionClient, Provider},
    completion::{
        CompletionError, CompletionModel as CompletionModelTrait, CompletionThinking,
        request::{CompletionRequest, CompletionResponse},
    },
    providers::{
        kimi::client::Client,
        openai_compat::{
            ChatErrorResponse, ChatMessage, ChatResponse, ChatToolDefinition,
            build_messages_with_reasoning_content, choice_reasoning_content, parse_choice_content,
            parse_tools,
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
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct KimiThinking {
    #[serde(rename = "type")]
    r#type: KimiThinkingType,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum KimiThinkingType {
    Enabled,
    Disabled,
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
        }),
        Some(
            CompletionThinking::Enabled
            | CompletionThinking::Low
            | CompletionThinking::Medium
            | CompletionThinking::High,
        ) => Some(KimiThinking {
            r#type: KimiThinkingType::Enabled,
        }),
        Some(CompletionThinking::Auto) | None => None,
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

        let kimi_request = KimiRequest {
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
        };

        tracing::debug!(
            provider = "kimi",
            model = %kimi_request.model,
            messages = kimi_request.messages.len(),
            tools = kimi_request.tools.len(),
            has_temperature = kimi_request.temperature.is_some(),
            thinking = ?kimi_request.thinking.as_ref(),
            "Kimi completion request started"
        );

        let mut req_builder = self
            .client
            .http_client
            .post(format!("{}/chat/completions", self.client.base_url))
            .json(&kimi_request);
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

        let kimi_response: ChatResponse =
            serde_json::from_str(&response_text).map_err(CompletionError::JsonError)?;

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
        };

        let json = serde_json::to_value(request).unwrap();
        assert_eq!(json["thinking"]["type"], "disabled");
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
        }
    }

    /// Scenario: `Auto` and `None` both omit the `thinking` field so Kimi
    /// applies its server-side default. The skip-serializing rule on the
    /// request struct depends on `thinking` being `None` here.
    #[test]
    fn test_kimi_thinking_auto_and_none_omit_field() {
        assert!(kimi_thinking(Some(CompletionThinking::Auto)).is_none());
        assert!(kimi_thinking(None).is_none());

        let request = KimiRequest {
            model: "kimi-k2.6".to_string(),
            messages: vec![ChatMessage::User {
                content: "hi".to_string(),
            }],
            temperature: None,
            tools: Vec::new(),
            tool_choice: None,
            thinking: kimi_thinking(Some(CompletionThinking::Auto)),
        };
        let json = serde_json::to_value(request).unwrap();
        assert!(
            json.get("thinking").is_none(),
            "thinking should be skipped for Auto: {json}"
        );
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
        };

        let json = serde_json::to_value(request).unwrap();
        assert_eq!(json["model"], "kimi-k2.6");
        assert_eq!(json["thinking"]["type"], "enabled");
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
