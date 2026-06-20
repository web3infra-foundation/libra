//! Provider-neutral completion request envelope.
//!
//! Boundary: requests carry normalized model, message, tool, and reasoning settings
//! but avoid provider-specific HTTP payload details. Provider tests cover optional
//! reasoning fields, absent tools, and streaming flags.

use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

use super::message::{AssistantContent, Message};
use crate::internal::ai::tools::ToolDefinition;

/// Incremental output from a provider while a completion request is still in flight.
#[derive(Debug, Clone)]
pub enum CompletionStreamEvent {
    TextDelta {
        request_id: Option<String>,
        delta: String,
    },
    ThinkingDelta {
        request_id: Option<String>,
        delta: String,
    },
    ToolCallPreview {
        request_id: Option<String>,
        call_id: String,
        tool_name: String,
        arguments: Value,
    },
}

/// Provider-neutral thinking control for models that expose reasoning knobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionThinking {
    Auto,
    Disabled,
    Enabled,
    Low,
    Medium,
    High,
}

/// Provider-neutral reasoning effort for models that expose a separate depth knob.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionReasoningEffort {
    Low,
    Medium,
    High,
    Max,
}

/// Represents a request for AI completion, including chat history and optional parameters.
#[derive(Debug, Clone, Default)]
pub struct CompletionRequest {
    pub preamble: Option<String>,   // Future-proof: Preamble support
    pub chat_history: Vec<Message>, // Conversation messages
    pub temperature: Option<f64>,   // Sampling temperature
    // Future-proof: Tools support
    pub tools: Vec<ToolDefinition>, // Tools available to the model
    // Future-proof: RAG support
    pub documents: Vec<Value>, // Placeholder for Document
    /// Optional thinking/reasoning mode for providers that support it.
    pub thinking: Option<CompletionThinking>,
    /// Optional reasoning effort for providers that expose a separate effort field.
    pub reasoning_effort: Option<CompletionReasoningEffort>,
    /// Optional provider request streaming flag.
    pub stream: Option<bool>,
    /// Optional sink for providers that can stream partial response events.
    pub stream_events: Option<UnboundedSender<CompletionStreamEvent>>,
}

/// Represents a response from the AI completion service.
#[derive(Debug)]
pub struct CompletionResponse<T> {
    pub content: Vec<AssistantContent>, // The content of the response (text, tool calls, etc.)
    /// Provider-specific reasoning text that must be preserved while continuing
    /// the same assistant tool-call turn.
    pub reasoning_content: Option<String>,
    pub raw_response: T, // Raw response from the AI service
}

impl CompletionRequest {
    /// Create a new CompletionRequest with the given chat history.
    pub fn new(messages: Vec<Message>) -> Self {
        Self {
            chat_history: messages,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `CompletionRequest::default()` must produce a fully-empty
    /// envelope: no preamble, empty chat history, no temperature, no
    /// tools, no documents, no thinking/reasoning, no streaming. Pin
    /// every field so provider adapters that branch on `is_none()` /
    /// `is_empty()` don't inherit surprising state from new defaults.
    #[test]
    fn completion_request_default_is_fully_empty_envelope() {
        let req = CompletionRequest::default();
        assert!(req.preamble.is_none());
        assert!(req.chat_history.is_empty());
        assert!(req.temperature.is_none());
        assert!(req.tools.is_empty());
        assert!(req.documents.is_empty());
        assert!(req.thinking.is_none());
        assert!(req.reasoning_effort.is_none());
        assert!(req.stream.is_none());
        assert!(req.stream_events.is_none());
    }

    /// `CompletionRequest::new(messages)` must thread the messages into
    /// `chat_history` and leave every other field at the default. This
    /// is the canonical "single-shot from a fresh chat" entry point.
    #[test]
    fn completion_request_new_threads_history_and_keeps_other_fields_default() {
        let messages = vec![Message::user("hi"), Message::assistant("hello")];
        let req = CompletionRequest::new(messages.clone());

        assert_eq!(req.chat_history, messages);
        // Every other field must still be at the default.
        assert!(req.preamble.is_none());
        assert!(req.temperature.is_none());
        assert!(req.tools.is_empty());
        assert!(req.documents.is_empty());
        assert!(req.thinking.is_none());
        assert!(req.reasoning_effort.is_none());
        assert!(req.stream.is_none());
        assert!(req.stream_events.is_none());
    }

    /// `CompletionRequest::new(vec![])` must produce the same shape as
    /// `default()` — the empty-history path is the only branch that
    /// makes this trivially observable.
    #[test]
    fn completion_request_new_with_empty_history_matches_default() {
        let req = CompletionRequest::new(vec![]);
        let default = CompletionRequest::default();
        // Compare every observable field (no PartialEq derived because
        // `UnboundedSender` isn't comparable).
        assert_eq!(req.preamble, default.preamble);
        assert_eq!(req.chat_history, default.chat_history);
        assert_eq!(req.temperature, default.temperature);
        assert_eq!(req.tools.len(), default.tools.len());
        assert_eq!(req.documents.len(), default.documents.len());
        assert_eq!(req.thinking, default.thinking);
        assert_eq!(req.reasoning_effort, default.reasoning_effort);
        assert_eq!(req.stream, default.stream);
        assert!(req.stream_events.is_none());
        assert!(default.stream_events.is_none());
    }

    /// `CompletionThinking` is `Copy` + `Eq`: comparing two variants
    /// must work without dereference, and all 6 variants must be
    /// distinct.
    #[test]
    fn completion_thinking_variants_are_distinct_copy_values() {
        let variants = [
            CompletionThinking::Auto,
            CompletionThinking::Disabled,
            CompletionThinking::Enabled,
            CompletionThinking::Low,
            CompletionThinking::Medium,
            CompletionThinking::High,
        ];
        for (i, a) in variants.iter().enumerate() {
            for (j, b) in variants.iter().enumerate() {
                if i == j {
                    assert_eq!(*a, *b);
                } else {
                    assert_ne!(*a, *b, "{a:?} must not equal {b:?}");
                }
            }
        }
    }

    /// `CompletionReasoningEffort` is `Copy` + `Eq`: all 4 variants
    /// distinct.
    #[test]
    fn completion_reasoning_effort_variants_are_distinct_copy_values() {
        let variants = [
            CompletionReasoningEffort::Low,
            CompletionReasoningEffort::Medium,
            CompletionReasoningEffort::High,
            CompletionReasoningEffort::Max,
        ];
        for (i, a) in variants.iter().enumerate() {
            for (j, b) in variants.iter().enumerate() {
                if i == j {
                    assert_eq!(*a, *b);
                } else {
                    assert_ne!(*a, *b, "{a:?} must not equal {b:?}");
                }
            }
        }
    }

    /// `CompletionStreamEvent` must derive `Clone` so observer code
    /// can fan out events to multiple sinks without consuming the
    /// original. Pin the three variants by constructing and cloning
    /// each.
    #[test]
    fn completion_stream_event_clone_preserves_variant_shape() {
        let text = CompletionStreamEvent::TextDelta {
            request_id: Some("req-1".to_string()),
            delta: "hi".to_string(),
        };
        let thinking = CompletionStreamEvent::ThinkingDelta {
            request_id: None,
            delta: "...".to_string(),
        };
        let tool = CompletionStreamEvent::ToolCallPreview {
            request_id: Some("req-2".to_string()),
            call_id: "call-1".to_string(),
            tool_name: "shell".to_string(),
            arguments: serde_json::json!({"cmd": "ls"}),
        };

        let _ = text.clone();
        let _ = thinking.clone();
        let _ = tool.clone();

        // Variant shape pin: a Debug snapshot must contain the variant
        // discriminator so audit log emission can grep on it.
        assert!(format!("{text:?}").contains("TextDelta"));
        assert!(format!("{thinking:?}").contains("ThinkingDelta"));
        assert!(format!("{tool:?}").contains("ToolCallPreview"));
    }
}
