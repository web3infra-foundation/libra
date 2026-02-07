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
        AssistantContent, CompletionError, CompletionModel as CompletionModelTrait, CompletionRequest,
        CompletionResponse, Function, Message, Text, ToolCall, UserContent,
    },
    tools::ToolDefinition,
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
                                parts.push(Part::function_response(
                                    tool_result.name,
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
                                parts.push(Part::function_call(
                                    tool_call.function.name,
                                    tool_call.function.arguments,
                                ));
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
        let system_instruction = request.preamble.map(Content::text);

        // Convert tools to Gemini format
        let tools = if !request.tools.is_empty() {
            Some(convert_tools_to_gemini(&request.tools))
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

        let content = parse_assistant_output(&api_resp)?;

        Ok(CompletionResponse {
            content,
            raw_response: api_resp,
        })
    }
}

fn parse_assistant_output(
    api_resp: &GenerateContentResponse,
) -> Result<Vec<AssistantContent>, CompletionError> {
    let parts = api_resp
        .candidates
        .as_ref()
        .and_then(|c| c.first())
        .and_then(|c| c.content.as_ref())
        .map(|c| c.parts.clone())
        .ok_or_else(|| CompletionError::ResponseError("No candidate content in response".into()))?;

    let mut assistant_parts = Vec::new();

    for (idx, part) in parts.iter().enumerate() {
        if let Some(text) = &part.text
            && !text.trim().is_empty()
        {
            assistant_parts.push(AssistantContent::Text(Text { text: text.clone() }));
        }

        if let Some(function_call) = &part.function_call {
            assistant_parts.push(AssistantContent::ToolCall(ToolCall {
                id: format!("call-{}-{}", function_call.name, idx + 1),
                name: function_call.name.clone(),
                function: Function {
                    name: function_call.name.clone(),
                    arguments: function_call.args.clone(),
                },
            }));
        }
    }

    if assistant_parts.is_empty() {
        return Err(CompletionError::ResponseError(
            "No text or tool-call content in Gemini response".into(),
        ));
    }

    Ok(assistant_parts)
}

/// Convert generic tool specs to Gemini's tool declaration format.
fn convert_tools_to_gemini(tools: &[ToolDefinition]) -> Vec<ToolDeclaration> {
    let mut function_declarations = Vec::new();

    for tool in tools {
        let parameters = tool.parameters.as_object().and_then(|params| {
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

        let mut function_decl =
            FunctionDeclaration::new(tool.name.clone(), tool.description.clone());
        if let Some(params) = parameters {
            function_decl = function_decl.with_parameters(params);
        }

        function_declarations.push(function_decl);
    }

    vec![ToolDeclaration::new(function_declarations)]
}
