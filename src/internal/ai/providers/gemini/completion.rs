//! Gemini completion model implementation.
//!
//! Implements the [`CompletionModelTrait`] by calling the Gemini
//! `generateContent` endpoint:
//!
//! ```text
//! POST /v1beta/models/{model}:generateContent
//! ```
//!
//! # Role mapping
//!
//! Gemini uses `"model"` as the assistant role (not `"assistant"` as in
//! OpenAI-style APIs). System instructions are sent via the dedicated
//! `system_instruction` field rather than as a message in the conversation
//! history.

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
        CompletionRequest, CompletionResponse, CompletionUsage, CompletionUsageSummary, Function,
        Message, Text, ToolCall, UserContent,
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
    ///
    /// Boundary conditions: the model id is forwarded verbatim and not validated;
    /// unknown identifiers fail at request time with HTTP 404.
    pub fn new(client: Client, model: impl Into<String>) -> Self {
        Self {
            client,
            model: model.into(),
        }
    }

    /// Returns the model name as supplied at construction.
    pub fn model_name(&self) -> &str {
        &self.model
    }
}

impl CompletionModelTrait for CompletionModel {
    type Response = GenerateContentResponse;

    /// Drive a single chat completion against the Gemini API.
    ///
    /// Functional scope:
    /// - Translates each [`Message`] into Gemini's [`Content`] entries, mapping
    ///   the assistant role to `"model"` and merging `Message::System` entries
    ///   into `"user"` content (Gemini has no `"system"` role inside `contents`).
    /// - Hoists the optional preamble into the dedicated `system_instruction`
    ///   field — the only correct location per the Gemini wire format.
    /// - Converts tools through [`convert_tools_to_gemini`] which produces a
    ///   single `[ToolDeclaration]` containing every function declaration.
    ///
    /// Boundary conditions:
    /// - User images return [`CompletionError::NotImplemented`]; the Gemini
    ///   provider is currently text-only inside Libra.
    /// - On non-2xx responses, only the first 1KB of the body is read to bound
    ///   memory usage on pathological error pages and avoid blocking on a slow
    ///   error stream.
    /// - Successful responses are deserialised eagerly via
    ///   [`reqwest::Response::json`]; deserialisation failures surface as
    ///   [`CompletionError::HttpError`] (the type Gemini's `reqwest` JSON path
    ///   returns).
    async fn completion(
        &self,
        request: CompletionRequest,
    ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
        let url = format!(
            "{}/v1beta/models/{}:generateContent",
            self.client.base_url, self.model
        );

        // Convert generic messages into Gemini `Content` entries.
        // Gemini uses "user" and "model" roles (not "assistant").
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
                    // Gemini expects "model" as the role for assistant turns.
                    let mut parts = Vec::new();
                    for item in content.into_iter() {
                        match item {
                            AssistantContent::Text(t) => parts.push(Part::text(t.text)),
                            AssistantContent::ToolCall(tool_call) => {
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
                    // Gemini does not have a dedicated system role in the
                    // contents array (system instructions go via the separate
                    // `system_instruction` field). Inline system messages
                    // are mapped to the "user" role as a fallback.
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

        // The preamble maps to Gemini's `system_instruction` field, which is
        // sent outside the regular conversation contents.
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

        let mut req_builder = self.client.http_client.post(&url).json(&body);

        // Apply provider-level customisations (adds the x-goog-api-key header).
        req_builder = self.client.provider.on_request(req_builder);

        tracing::debug!("Sending request to Gemini API: {}", url);
        let resp = req_builder
            .send()
            .await
            .map_err(CompletionError::HttpError)?;
        let status = resp.status();

        tracing::debug!("Received response status: {}", status);

        if !status.is_success() {
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
                "status {}: Gemini API Error: {}",
                status.as_u16(),
                text
            )));
        }

        let api_resp: GenerateContentResponse =
            resp.json().await.map_err(CompletionError::HttpError)?;

        let content = parse_assistant_output(&api_resp)?;

        Ok(CompletionResponse {
            content,
            reasoning_content: None,
            raw_response: api_resp,
        })
    }
}

impl CompletionUsage for GenerateContentResponse {
    fn usage_summary(&self) -> Option<CompletionUsageSummary> {
        self.usage_metadata
            .as_ref()
            .map(|usage| CompletionUsageSummary {
                input_tokens: usage.prompt_token_count.unwrap_or(0),
                output_tokens: usage.candidates_token_count.unwrap_or(0),
                cached_tokens: usage.cached_content_token_count,
                reasoning_tokens: usage.thoughts_token_count,
                total_tokens: usage.total_token_count,
                cost_usd: None,
            })
    }
}

/// Extracts text segments and function calls from the first candidate in a
/// Gemini `GenerateContentResponse`.
///
/// Functional scope:
/// - Each [`Part`] in the candidate content is inspected: text parts become
///   [`AssistantContent::Text`] and function-call parts become
///   [`AssistantContent::ToolCall`].
/// - Because the Gemini API does not return tool-call IDs, a synthetic ID is
///   generated from the function name and the part index (e.g.,
///   `call-get_weather-1`). Down-stream code must use this ID when echoing the
///   tool result back, otherwise the model cannot correlate it.
///
/// Boundary conditions:
/// - Returns [`CompletionError::ResponseError`] if the response contains no
///   candidates (typical for safety-blocked outputs) or if no actionable content
///   (text or tool calls) is found.
/// - Whitespace-only text parts are dropped to keep downstream rendering clean.
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

/// Converts generic [`ToolDefinition`] values into Gemini's
/// [`ToolDeclaration`] / [`FunctionDeclaration`] wire format.
///
/// Functional scope:
/// - Each tool's JSON Schema `parameters` object is destructured into Gemini's
///   [`FunctionParameters`] (type, properties, required).
/// - All function declarations are grouped into a single [`ToolDeclaration`],
///   which matches the Gemini API expectation of a top-level `tools` array
///   containing objects with `function_declarations`.
///
/// Boundary conditions:
/// - Tools whose `parameters` are not a JSON object (or lack a `type` field)
///   produce a function declaration with no parameters — Gemini treats this as
///   a zero-argument tool rather than rejecting the request.
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
