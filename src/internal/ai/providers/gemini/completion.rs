use bytes::Buf;

use super::{
    client::Client,
    gemini_api_types::{
        Content, FunctionDeclaration, FunctionParameters, GenerateContentRequest,
        GenerateContentResponse, GenerationConfig, Part, ToolDeclaration,
    },
};
use crate::internal::ai::{
    client::Provider,
    completion::{
        AssistantContent, CompletionError, CompletionModel as CompletionModelTrait,
        CompletionRequest, CompletionResponse, Message, OneOrMany, Text, ToolCall, UserContent,
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
        // Use generic Provider to customize request if needed, though Gemini usually uses query param or header.
        // Our new Client structure puts auth in header via on_request.

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
                            UserContent::Text(t) => parts.push(Part::text(t.text)),
                            UserContent::Image(_) => {
                                // Reserved for future Image support
                                return Err(CompletionError::NotImplemented(
                                    "Image support not implemented yet for Gemini provider".into(),
                                ));
                            }
                            UserContent::ToolResult(tool_result) => {
                                // Convert tool result to function response
                                let function_name = tool_result.name.unwrap_or(tool_result.id);
                                parts.push(Part::function_response(
                                    function_name,
                                    tool_result.result,
                                ));
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
                            AssistantContent::Text(t) => parts.push(Part::text(t.text)),
                            AssistantContent::ToolCall(tool_call) => {
                                // Convert tool call to function call
                                parts
                                    .push(Part::function_call(tool_call.name, tool_call.arguments));
                            }
                        }
                    }
                    contents.push(Content {
                        role: Some("model".to_string()),
                        parts,
                    });
                }
                Message::System { content } => {
                    // System messages in chat history might need to be merged or handled as 'user' role for Gemini
                    // or ignored if preamble is preferred.
                    // For now, treat as user text.
                    let mut parts = Vec::new();
                    for item in content.into_iter() {
                        if let UserContent::Text(t) = item {
                            parts.push(Part::text(t.text))
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
        let system_instruction = request.preamble.map(|preamble| Content::text(preamble));

        // Convert tools to Gemini format
        let tools = if !request.tools.is_empty() {
            Some(convert_tools_to_gemini(&request.tools)?)
        } else {
            None
        };

        let body = GenerateContentRequest {
            contents,
            system_instruction,
            generation_config: Some(GenerationConfig {
                temperature: request.temperature,
            }),
            tools,
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
            // Read only the first 1KB of the error body to avoid memory issues with large responses
            // and handle potential non-UTF8 content safely.
            use std::io::Read;
            let mut buf = [0u8; 1024];
            let mut chunk = resp
                .bytes()
                .await
                .map_err(CompletionError::HttpError)?
                .reader();
            let n = chunk
                .read(&mut buf)
                .map_err(|e: std::io::Error| CompletionError::ResponseError(e.to_string()))?;
            let text = String::from_utf8_lossy(&buf[..n]);

            return Err(CompletionError::ProviderError(format!(
                "Gemini API Error: {}",
                text
            )));
        }

        let api_resp: GenerateContentResponse =
            resp.json().await.map_err(CompletionError::HttpError)?;

        let (text, message) = parse_assistant_output(&api_resp)?;

        Ok(CompletionResponse {
            choice: text,
            message,
            raw_response: api_resp,
        })
    }
}

fn parse_assistant_output(
    api_resp: &GenerateContentResponse,
) -> Result<(String, Option<Message>), CompletionError> {
    let parts = api_resp
        .candidates
        .as_ref()
        .and_then(|c| c.first())
        .and_then(|c| c.content.as_ref())
        .map(|c| c.parts.clone())
        .ok_or_else(|| CompletionError::ResponseError("No candidate content in response".into()))?;

    let mut text_parts = Vec::new();
    let mut assistant_parts = Vec::new();

    for (idx, part) in parts.iter().enumerate() {
        if let Some(text) = &part.text {
            if !text.trim().is_empty() {
                text_parts.push(text.clone());
                assistant_parts.push(AssistantContent::Text(Text { text: text.clone() }));
            }
        }

        if let Some(function_call) = &part.function_call {
            assistant_parts.push(AssistantContent::ToolCall(ToolCall {
                id: format!("call-{}-{}", function_call.name, idx + 1),
                name: function_call.name.clone(),
                arguments: function_call.args.clone(),
            }));
        }
    }

    let choice = text_parts.join("\n");
    let message = assistant_message_from_parts(assistant_parts);

    if choice.trim().is_empty() && message.is_none() {
        return Err(CompletionError::ResponseError(
            "No text or tool-call content in Gemini response".into(),
        ));
    }

    Ok((choice, message))
}

fn assistant_message_from_parts(parts: Vec<AssistantContent>) -> Option<Message> {
    if parts.is_empty() {
        return None;
    }

    let content = if parts.len() == 1 {
        OneOrMany::One(parts.into_iter().next().unwrap())
    } else {
        OneOrMany::Many(parts)
    };

    Some(Message::Assistant { id: None, content })
}

/// Convert generic tool specs to Gemini's tool declaration format.
fn convert_tools_to_gemini(
    tools: &[serde_json::Value],
) -> Result<Vec<ToolDeclaration>, CompletionError> {
    let mut function_declarations = Vec::new();

    for tool in tools {
        // Parse the tool spec
        let tool_spec: serde_json::Value = tool.clone();

        // Extract function info
        let function = tool_spec.get("function").ok_or_else(|| {
            CompletionError::RequestError("Tool spec missing 'function' field".into())
        })?;

        let name = function
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                CompletionError::RequestError("Tool function missing 'name' field".into())
            })?
            .to_string();

        let description = function
            .get("description")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                CompletionError::RequestError("Tool function missing 'description' field".into())
            })?
            .to_string();

        // Extract parameters
        let parameters = function.get("parameters").and_then(|params| {
            let param_type = params.get("type").and_then(|t| t.as_str())?;
            let properties = params.get("properties").cloned()?;
            let required = params.get("required").and_then(|r| {
                r.as_array().map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
            });

            Some(FunctionParameters {
                param_type: param_type.to_string(),
                properties,
                required,
            })
        });

        let mut function_decl = FunctionDeclaration::new(name, description);
        if let Some(params) = parameters {
            function_decl = function_decl.with_parameters(params);
        }

        function_declarations.push(function_decl);
    }

    Ok(vec![ToolDeclaration::new(function_declarations)])
}
