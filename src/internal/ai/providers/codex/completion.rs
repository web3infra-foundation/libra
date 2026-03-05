//! Codex completion model implementation using WebSocket.

use serde::Serialize;

use crate::internal::ai::{
    client::CompletionClient,
    completion::{
        message::{AssistantContent, Message, Text},
        CompletionError, CompletionModel as CompletionModelTrait,
        request::{CompletionRequest, CompletionResponse},
    },
    providers::codex::client::CodexWebSocket,
};

pub const CODEX_01: &str = "codex-01";

#[derive(Clone, Debug)]
pub struct Model {
    client: CodexWebSocket,
    model: String,
}

impl Model {
    pub fn new(client: CodexWebSocket, model: impl Into<String>) -> Self {
        Self {
            client,
            model: model.into(),
        }
    }

    pub fn model_name(&self) -> &str {
        &self.model
    }

    pub async fn connect(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.client.connect().await
    }
}

#[derive(Debug, Serialize)]
struct CodexRequest {
    model: String,
    messages: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
}

impl CompletionModelTrait for Model {
    type Response = serde_json::Value;

    async fn completion(
        &self,
        request: CompletionRequest,
    ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
        let user_message = request
            .chat_history
            .last()
            .and_then(|msg| {
                use crate::internal::ai::completion::message::Message;
                if let Message::User { content } = msg {
                    let text = match content {
                        crate::internal::ai::completion::message::OneOrMany::One(c) => {
                            match c {
                                crate::internal::ai::completion::message::UserContent::Text(t) => t.text.clone(),
                                _ => format!("{:?}", c),
                            }
                        }
                        crate::internal::ai::completion::message::OneOrMany::Many(vec) => {
                            vec.iter()
                                .map(|c| match c {
                                    crate::internal::ai::completion::message::UserContent::Text(t) => t.text.clone(),
                                    _ => format!("{:?}", c),
                                })
                                .collect::<Vec<_>>()
                                .join("\n")
                        }
                    };
                    Some(text)
                } else {
                    None
                }
            })
            .unwrap_or_default();

        let params = serde_json::json!({
            "input": [{
                "type": "text",
                "text": user_message
            }]
        });

        let result = self
            .client
            .clone()
            .send_request_with_thread("turn/start", params)
            .await
            .map_err(|e| CompletionError::ProviderError(e.to_string()))?;

        let messages = self.client.get_agent_messages().await;
        eprintln!("[Codex] Collected {} messages", messages.len());
        let content = if messages.is_empty() {
            extract_content_from_response(&result)
        } else {
            let text = messages.join("");
            vec![AssistantContent::Text(Text { text })]
        };

        self.client.clear_agent_messages().await;

        Ok(CompletionResponse {
            content,
            raw_response: result,
        })
    }
}

fn extract_content_from_response(response: &serde_json::Value) -> Vec<AssistantContent> {
    let mut content = Vec::new();

    if let Some(turn) = response.get("result").and_then(|r| r.get("turn")) {
        if let Some(items) = turn.get("items").and_then(|i| i.as_array()) {
            for item in items {
                if let Some(item_type) = item.get("type").and_then(|t| t.as_str()) {
                    if item_type == "agentMessage" {
                        if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                            content.push(AssistantContent::Text(Text {
                                text: text.to_string(),
                            }));
                        }
                    }
                }
            }
        }
    }

    if content.is_empty() {
        content.push(AssistantContent::Text(Text {
            text: "Codex is processing your request...".to_string(),
        }));
    }

    content
}

impl CompletionClient for CodexWebSocket {
    type Model = Model;

    fn completion_model(&self, model: impl Into<String>) -> Self::Model {
        Model::new(self.clone(), model)
    }
}

pub type CodexModel = Model;
