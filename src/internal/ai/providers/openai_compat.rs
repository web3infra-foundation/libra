//! Shared wire types and helpers for OpenAI-compatible chat completion providers.
//!
//! Several AI providers (OpenAI, Ollama, DeepSeek, Zhipu) expose an
//! OpenAI-compatible `/chat/completions` endpoint. This module centralizes the
//! common request/response types and conversion helpers so that each provider
//! only needs to define its own `Request` struct (to accommodate minor API
//! differences like DeepSeek's `stream` field) and `ToolChoice` enum (which
//! varies in serialization strategy and available modes across providers).

use serde::{Deserialize, Serialize};

use crate::internal::ai::{
    completion::{
        AssistantContent, CompletionError, CompletionUsage, CompletionUsageSummary, Function,
        Message, Text, ToolCall, UserContent, request::CompletionRequest,
    },
    tools::ToolDefinition,
};

// ================================================================
// Shared Wire Types
// ================================================================

/// A message in an OpenAI-compatible chat conversation, tagged by `role`.
///
/// Used by all OpenAI-compatible providers (OpenAI, Ollama, DeepSeek, Zhipu).
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum ChatMessage {
    /// A system-level instruction (preamble / system prompt).
    System { content: String },
    /// A user message containing plain text.
    User { content: String },
    /// An assistant response, which may include both text and tool calls.
    Assistant {
        content: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_content: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<ChatToolCall>,
    },
    /// The result of a tool invocation, linked back by `tool_call_id`.
    Tool {
        tool_call_id: String,
        name: String,
        content: String,
    },
}

/// A tool definition in the OpenAI function-calling format.
#[derive(Debug, Serialize, Deserialize)]
pub struct ChatToolDefinition {
    pub r#type: String,
    pub function: ChatFunctionDefinition,
}

/// Metadata for a callable function exposed as a tool.
#[derive(Debug, Serialize, Deserialize)]
pub struct ChatFunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// A tool call emitted by the assistant in a response message.
#[derive(Debug, Serialize, Deserialize)]
pub struct ChatToolCall {
    pub id: String,
    pub r#type: String,
    pub function: ChatFunctionCall,
}

/// The function name and its JSON-encoded arguments within a tool call.
#[derive(Debug, Serialize, Deserialize)]
pub struct ChatFunctionCall {
    pub name: String,
    pub arguments: String,
}

/// A single completion choice from the response.
#[derive(Debug, Serialize, Deserialize)]
pub struct ChatChoice {
    pub index: usize,
    pub message: ChatMessage,
    pub finish_reason: Option<String>,
}

/// Token usage statistics returned alongside the completion.
#[derive(Debug, Serialize, Deserialize)]
pub struct ChatUsage {
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
    pub total_tokens: usize,
}

/// Top-level response from an OpenAI-compatible `/chat/completions` endpoint.
#[derive(Debug, Serialize, Deserialize)]
pub struct ChatResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChatChoice>,
    pub usage: Option<ChatUsage>,
}

impl CompletionUsage for ChatResponse {
    fn usage_summary(&self) -> Option<CompletionUsageSummary> {
        self.usage.as_ref().map(|usage| CompletionUsageSummary {
            input_tokens: usage.prompt_tokens as u64,
            output_tokens: usage.completion_tokens as u64,
            cost_usd: None,
        })
    }
}

/// Inner error object from an OpenAI-compatible API.
#[derive(Debug, Deserialize)]
pub struct ChatError {
    pub message: String,
}

/// Wrapper for the `{ "error": { ... } }` JSON error shape.
#[derive(Debug, Deserialize)]
pub struct ChatErrorResponse {
    pub error: ChatError,
}

// ================================================================
// Shared Helper Functions
// ================================================================

/// Converts generic [`ToolDefinition`]s into the OpenAI function-calling format.
pub fn parse_tools(tools: &[ToolDefinition]) -> Vec<ChatToolDefinition> {
    tools
        .iter()
        .map(|tool| ChatToolDefinition {
            r#type: "function".to_string(),
            function: ChatFunctionDefinition {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: tool.parameters.clone(),
            },
        })
        .collect()
}

/// Builds the ordered list of [`ChatMessage`]s from a [`CompletionRequest`].
///
/// The optional `preamble` becomes a leading `System` message. User messages
/// are expanded per-item (text -> `User`, tool results -> `Tool`). Assistant
/// messages collect text and tool calls into a single message. Image content
/// returns a [`CompletionError::NotImplemented`] error.
pub fn build_messages(request: &CompletionRequest) -> Result<Vec<ChatMessage>, CompletionError> {
    build_messages_internal(request, false)
}

/// Builds messages while preserving assistant `reasoning_content`.
///
/// This is provider-specific for DeepSeek thinking mode. Other OpenAI-compatible
/// providers should use [`build_messages`] so they do not receive unsupported
/// message fields.
pub fn build_messages_with_reasoning_content(
    request: &CompletionRequest,
) -> Result<Vec<ChatMessage>, CompletionError> {
    build_messages_internal(request, true)
}

fn build_messages_internal(
    request: &CompletionRequest,
    include_reasoning_content: bool,
) -> Result<Vec<ChatMessage>, CompletionError> {
    let mut messages = Vec::new();

    if let Some(preamble) = &request.preamble {
        messages.push(ChatMessage::System {
            content: preamble.clone(),
        });
    }

    for msg in &request.chat_history {
        match msg {
            Message::User { content } => {
                for item in content.iter() {
                    match item {
                        UserContent::Text(t) => messages.push(ChatMessage::User {
                            content: t.text.clone(),
                        }),
                        UserContent::ToolResult(tool_result) => {
                            let content = serde_json::to_string(&tool_result.result)
                                .unwrap_or_else(|_| tool_result.result.to_string());
                            messages.push(ChatMessage::Tool {
                                tool_call_id: tool_result.id.clone(),
                                name: tool_result.name.clone(),
                                content,
                            });
                        }
                        UserContent::Image(_) => {
                            return Err(CompletionError::NotImplemented(
                                "Image content is not supported by this provider".into(),
                            ));
                        }
                    }
                }
            }
            Message::Assistant {
                content,
                reasoning_content,
                ..
            } => {
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
                            tool_calls.push(ChatToolCall {
                                id: call.id.clone(),
                                r#type: "function".to_string(),
                                function: ChatFunctionCall {
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

                messages.push(ChatMessage::Assistant {
                    content: text,
                    reasoning_content: if include_reasoning_content {
                        reasoning_content.clone()
                    } else {
                        None
                    },
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
                messages.push(ChatMessage::System { content: text });
            }
        }
    }

    Ok(messages)
}

/// Extracts [`AssistantContent`] items from a response [`ChatChoice`].
///
/// Non-empty text is emitted as [`AssistantContent::Text`]; each tool call
/// has its JSON arguments string parsed back into a [`serde_json::Value`].
/// Returns an error if the message is not an `Assistant` variant.
pub fn parse_choice_content(choice: &ChatChoice) -> Result<Vec<AssistantContent>, CompletionError> {
    match &choice.message {
        ChatMessage::Assistant {
            content,
            reasoning_content: _,
            tool_calls,
        } => {
            let mut parts = Vec::new();

            if let Some(text) = content
                && !text.trim().is_empty()
            {
                parts.push(AssistantContent::Text(Text { text: text.clone() }));
            }

            for call in tool_calls {
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
            "Unexpected non-assistant message in response".to_string(),
        )),
    }
}

/// Extracts provider-specific reasoning content from an assistant choice.
pub fn choice_reasoning_content(choice: &ChatChoice) -> Option<String> {
    match &choice.message {
        ChatMessage::Assistant {
            reasoning_content, ..
        } => reasoning_content
            .as_ref()
            .filter(|content| !content.trim().is_empty())
            .cloned(),
        _ => None,
    }
}

/// Ensures tool arguments are serialized as a JSON string.
///
/// If `arguments` is a [`serde_json::Value::String`] containing valid JSON,
/// it is returned as-is (avoiding double-encoding). Otherwise the value is
/// serialized via `to_string()`.
pub fn tool_arguments_json(arguments: &serde_json::Value) -> String {
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
// Test-only Helpers
// ================================================================

/// Simplified lossy conversion from [`Message`] to [`ChatMessage`].
///
/// Only extracts the first content item per message — **not** suitable for
/// production use where messages may carry multiple content items, tool calls,
/// or tool results. Use [`build_messages`] instead.
#[cfg(test)]
impl From<&Message> for ChatMessage {
    fn from(msg: &Message) -> Self {
        match msg {
            Message::User { content } => {
                let text = content
                    .iter()
                    .next()
                    .map(|c| match c {
                        UserContent::Text(t) => t.text.clone(),
                        _ => String::new(),
                    })
                    .unwrap_or_default();
                ChatMessage::User { content: text }
            }
            Message::Assistant { content, .. } => {
                let text = content
                    .iter()
                    .next()
                    .map(|c| match c {
                        AssistantContent::Text(t) => t.text.clone(),
                        _ => String::new(),
                    })
                    .unwrap_or_default();
                ChatMessage::Assistant {
                    content: if text.is_empty() { None } else { Some(text) },
                    reasoning_content: None,
                    tool_calls: Vec::new(),
                }
            }
            Message::System { content } => {
                let text = content
                    .iter()
                    .next()
                    .map(|c| match c {
                        UserContent::Text(t) => t.text.clone(),
                        _ => String::new(),
                    })
                    .unwrap_or_default();
                ChatMessage::System { content: text }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::completion::Message;

    #[test]
    fn test_message_to_chat_message() {
        let user_msg = Message::user("Hello");
        let chat_msg: ChatMessage = (&user_msg).into();
        assert!(matches!(chat_msg, ChatMessage::User { .. }));

        let assistant_msg = Message::Assistant {
            id: None,
            reasoning_content: None,
            content: crate::internal::ai::completion::message::OneOrMany::one(
                AssistantContent::Text(Text {
                    text: "Hi there".to_string(),
                }),
            ),
        };
        let chat_msg: ChatMessage = (&assistant_msg).into();
        assert!(matches!(chat_msg, ChatMessage::Assistant { .. }));

        let system_msg = Message::System {
            content: crate::internal::ai::completion::message::OneOrMany::one(UserContent::Text(
                Text {
                    text: "System prompt".to_string(),
                },
            )),
        };
        let chat_msg: ChatMessage = (&system_msg).into();
        assert!(matches!(chat_msg, ChatMessage::System { .. }));
    }

    #[test]
    fn build_messages_with_reasoning_content_preserves_assistant_reasoning_content() {
        let request = CompletionRequest {
            chat_history: vec![Message::Assistant {
                id: None,
                reasoning_content: Some("I should call the tool first.".to_string()),
                content: crate::internal::ai::completion::message::OneOrMany::one(
                    AssistantContent::ToolCall(ToolCall {
                        id: "call_1".to_string(),
                        name: "read_file".to_string(),
                        function: Function {
                            name: "read_file".to_string(),
                            arguments: serde_json::json!({"path": "Cargo.toml"}),
                        },
                    }),
                ),
            }],
            ..Default::default()
        };

        let messages = build_messages_with_reasoning_content(&request).unwrap();
        let json = serde_json::to_value(&messages).unwrap();

        assert_eq!(
            json[0]["reasoning_content"],
            "I should call the tool first."
        );
        assert_eq!(json[0]["tool_calls"][0]["id"], "call_1");
    }

    #[test]
    fn build_messages_omits_assistant_reasoning_content_by_default() {
        let request = CompletionRequest {
            chat_history: vec![Message::Assistant {
                id: None,
                reasoning_content: Some("DeepSeek-only reasoning".to_string()),
                content: crate::internal::ai::completion::message::OneOrMany::one(
                    AssistantContent::Text(Text {
                        text: "visible answer".to_string(),
                    }),
                ),
            }],
            ..Default::default()
        };

        let messages = build_messages(&request).unwrap();
        let json = serde_json::to_value(&messages).unwrap();

        assert!(json[0].get("reasoning_content").is_none());
        assert_eq!(json[0]["content"], "visible answer");
    }
}
