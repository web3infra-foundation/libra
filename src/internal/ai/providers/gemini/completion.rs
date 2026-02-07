use bytes::Buf;
use uuid::Uuid;

use super::{
    client::Client,
    gemini_api_types::{
        Content, FunctionCallingMode, FunctionDeclaration, GenerateContentRequest,
        GenerateContentResponse, GenerationConfig, Part, PartKind, Schema, Tool, ToolConfig,
    },
};
use crate::internal::ai::{
    client::Provider,
    completion::{
        AssistantContent, CompletionError, CompletionModel as CompletionModelTrait,
        CompletionRequest, CompletionResponse, Message, UserContent,
    },
};

/// A completion model implementation for Google Gemini.
///
/// This struct handles the interaction with the Gemini API, including:
/// - Constructing requests from the generic `CompletionRequest`.
/// - Parsing responses from the Gemini API.
/// - Handling errors and status codes.
#[derive(Clone, Debug)]
pub struct CompletionModel {
    /// The client instance used to make HTTP requests.
    client: Client,
    /// The name of the Gemini model to use (e.g., "gemini-1.5-flash").
    model: String,
}

impl CompletionModel {
    /// Creates a new Gemini CompletionModel.
    ///
    /// # Arguments
    /// * `client` - The configured Gemini client.
    /// * `model` - The model identifier.
    pub fn new(client: Client, model: impl Into<String>) -> Self {
        Self {
            client,
            model: model.into(),
        }
    }
}

impl CompletionModelTrait for CompletionModel {
    type Response = GenerateContentResponse;

    async fn completion(
        &self,
        request: CompletionRequest,
    ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
        // Use generic Provider to customize request if needed.
        // Our Client structure puts auth in header via on_request.

        let url = format!(
            "{}/v1beta/models/{}:generateContent",
            self.client.base_url, self.model
        );

        // Convert messages
        let mut contents = Vec::new();
        for msg in request.chat_history {
            match msg {
                Message::User { content } => {
                    let mut parts = Vec::new();
                    for item in content.into_iter() {
                        match item {
                            UserContent::Text(t) => parts.push(Part {
                                part: PartKind::Text(t.text),
                                thought: None,
                                thought_signature: None,
                            }),
                            UserContent::Image(_) => {
                                // Reserved for future Image support
                                return Err(CompletionError::NotImplemented(
                                    "Image support not implemented yet for Gemini provider".into(),
                                ));
                            }
                            UserContent::ToolResult(r) => {
                                if r.name.is_empty() {
                                    return Err(CompletionError::RequestError(
                                        std::io::Error::new(
                                            std::io::ErrorKind::InvalidInput,
                                            "ToolResult name is required for Gemini",
                                        )
                                        .into(),
                                    ));
                                }
                                if !r
                                    .name
                                    .chars()
                                    .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
                                {
                                    return Err(CompletionError::RequestError(
                                        std::io::Error::new(
                                            std::io::ErrorKind::InvalidInput,
                                            format!("Invalid ToolResult name: {}", r.name),
                                        )
                                        .into(),
                                    ));
                                }
                                // Gemini requires tool response to have a 'name'
                                let function_response = super::gemini_api_types::FunctionResponse {
                                    name: r.name.clone(),
                                    response: Some(serde_json::json!({ "result": r.result })),
                                };
                                parts.push(Part {
                                    part: PartKind::FunctionResponse(function_response),
                                    thought: None,
                                    thought_signature: None,
                                });
                            }
                        }
                    }
                    contents.push(Content {
                        role: Some("user".to_string()),
                        parts,
                    });
                }
                Message::Assistant { content, .. } => {
                    let mut parts = Vec::new();
                    for item in content.into_iter() {
                        match item {
                            AssistantContent::Text(t) => parts.push(Part {
                                part: PartKind::Text(t.text),
                                thought: None,
                                thought_signature: None,
                            }),
                            AssistantContent::ToolCall(tc) => {
                                let function_call = super::gemini_api_types::FunctionCall {
                                    name: tc.function.name,
                                    args: tc.function.arguments,
                                };
                                parts.push(Part {
                                    part: PartKind::FunctionCall(function_call),
                                    thought: None,
                                    thought_signature: None,
                                });
                            }
                        }
                    }
                    contents.push(Content {
                        role: Some("model".to_string()),
                        parts,
                    });
                }
                Message::System { content } => {
                    // System messages in chat history treated as user text.
                    let mut parts = Vec::new();
                    for item in content.into_iter() {
                        if let UserContent::Text(t) = item {
                            parts.push(Part {
                                part: PartKind::Text(t.text),
                                thought: None,
                                thought_signature: None,
                            })
                        }
                    }
                    contents.push(Content {
                        role: Some("user".to_string()),
                        parts,
                    });
                }
            }
        }

        // Handle Preamble (System Prompt)
        let system_instruction = request.preamble.map(|preamble| Content {
            role: Some("user".to_string()),
            parts: vec![Part {
                part: PartKind::Text(preamble),
                thought: None,
                thought_signature: None,
            }],
        });

        // Handle Tools
        let tools = if request.tools.is_empty() {
            None
        } else {
            let mut function_declarations = Vec::new();
            for t in request.tools {
                let parameters = if t.parameters.as_object().is_some_and(|o| o.is_empty())
                    || t.parameters == serde_json::json!({"type": "object", "properties": {}})
                {
                    None
                } else {
                    Some(Schema::try_from(t.parameters)?)
                };

                function_declarations.push(FunctionDeclaration {
                    name: t.name,
                    description: t.description,
                    parameters,
                });
            }
            Some(vec![Tool {
                function_declarations,
            }])
        };

        // For now, we default to Auto if tools are present
        let tool_config = if tools.is_some() {
            Some(ToolConfig {
                function_calling_config: Some(FunctionCallingMode::Auto),
            })
        } else {
            None
        };

        let body = GenerateContentRequest {
            contents,
            system_instruction,
            tools,
            tool_config,
            generation_config: Some(GenerationConfig {
                temperature: request.temperature,
            }),
        };

        // Build request using generic client logic
        let mut req_builder = self.client.http_client.post(&url).json(&body);

        // Apply Provider customizations (Auth headers)
        req_builder = self.client.provider.on_request(req_builder);

        tracing::info!("Sending request to Gemini API: {}", url);
        let resp = req_builder
            .send()
            .await
            .map_err(CompletionError::HttpError)?;

        tracing::info!("Received response status: {}", resp.status());

        if !resp.status().is_success() {
            // Read up to 4KB of the error body to avoid memory issues with large responses
            // and handle potential non-UTF8 content safely.
            use std::io::Read;
            let mut buf = [0u8; 4096];
            let mut chunk = resp
                .bytes()
                .await
                .map_err(CompletionError::HttpError)?
                .reader();
            let n = chunk
                .read(&mut buf)
                .map_err(|e: std::io::Error| CompletionError::ResponseError(e.to_string()))?;
            let mut text = String::from_utf8_lossy(&buf[..n]).to_string();
            if n == 4096 {
                text.push_str("... (truncated)");
            }

            return Err(CompletionError::ProviderError(format!(
                "Gemini API Error: {}",
                text
            )));
        }

        let api_resp: GenerateContentResponse =
            resp.json().await.map_err(CompletionError::HttpError)?;

        // Extract content
        let candidate = api_resp
            .candidates
            .as_ref()
            .and_then(|c| c.first())
            .ok_or_else(|| {
                CompletionError::ResponseError("No response candidates in Gemini response".into())
            })?;

        let mut content = Vec::new();
        if let Some(c) = &candidate.content {
            for part in &c.parts {
                match &part.part {
                    PartKind::Text(text) => {
                        content.push(AssistantContent::Text(
                            crate::internal::ai::completion::message::Text { text: text.clone() },
                        ));
                    }
                    PartKind::FunctionCall(fc) => {
                        content.push(AssistantContent::ToolCall(
                            crate::internal::ai::completion::message::ToolCall {
                                id: format!("{}-{}", fc.name, Uuid::new_v4()), // Generate unique ID
                                name: fc.name.clone(),
                                function: crate::internal::ai::completion::message::Function {
                                    name: fc.name.clone(),
                                    arguments: fc.args.clone(),
                                },
                            },
                        ));
                    }
                    _ => {
                        tracing::warn!("Received unsupported part kind in response: {:?}", part);
                    }
                }
            }
        }

        if content.is_empty() {
            return Err(CompletionError::ResponseError(
                "No content in Gemini response".into(),
            ));
        }

        Ok(CompletionResponse {
            content,
            raw_response: api_resp,
        })
    }
}
