//! Provider-neutral chat message primitives for completion requests.
//!
//! Boundary: message roles and parts must round-trip across all configured providers;
//! provider adapters are responsible for translating unsupported fields. Mock provider
//! tests cover empty content, tool messages, and multi-part messages.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::CompletionError;

/// Represents a message in the conversation, which can be from a user, an assistant, or the system.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum Message {
    // The message is from a user.
    User {
        content: OneOrMany<UserContent>,
    },
    // The message is from an assistant.
    Assistant {
        id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_content: Option<String>,
        content: OneOrMany<AssistantContent>,
    },
    // Future-proof: Explicit System message support
    System {
        content: OneOrMany<UserContent>, // System usually takes text
    },
}

/// Content types for User messages.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum UserContent {
    Text(Text),
    // Future-proof: Image support
    Image(Image),
    // Future-proof: Tool Result support
    ToolResult(ToolResult),
}

/// Content types for Assistant messages.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
pub enum AssistantContent {
    Text(Text),
    // Future-proof: Tool Call support
    ToolCall(ToolCall),
}

/// Text content.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Text {
    pub text: String,
}

/// Image content.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Image {
    pub data: String, // Base64 or URL
    pub mime_type: Option<String>,
}

/// Tool Call content.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String, // Ensure name is present for Gemini mapping
    pub function: Function,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Function {
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Tool Result content.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ToolResult {
    pub id: String,
    /// The name of the tool function that was called.
    /// This is required for some providers (e.g. Gemini).
    pub name: String,
    pub result: serde_json::Value,
}

/// Implementations for Text
impl Text {
    pub fn text(&self) -> &str {
        &self.text
    }
}

impl std::fmt::Display for Text {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.text)
    }
}

// ================================================================
// Helper Types
// ================================================================

/// A type that can represent either a single item or multiple items.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
pub enum OneOrMany<T> {
    One(T),
    Many(Vec<T>),
}

impl<T> OneOrMany<T> {
    // Create a OneOrMany with a single item.
    pub fn one(item: T) -> Self {
        Self::One(item)
    }

    // Create a OneOrMany with multiple items, or None if the vector is empty.
    pub fn many(items: Vec<T>) -> Option<Self> {
        if items.is_empty() {
            None
        } else {
            Some(Self::Many(items))
        }
    }

    // Returns an iterator over the items.
    pub fn iter(&self) -> OneOrManyIter<'_, T> {
        match self {
            OneOrMany::One(item) => OneOrManyIter::One(Some(item)),
            OneOrMany::Many(items) => OneOrManyIter::Many(items.iter()),
        }
    }
}

impl<T> IntoIterator for OneOrMany<T> {
    type Item = T;
    type IntoIter = OneOrManyIntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        match self {
            OneOrMany::One(item) => OneOrManyIntoIter::One(Some(item)),
            OneOrMany::Many(items) => OneOrManyIntoIter::Many(items.into_iter()),
        }
    }
}

/// Iterator for OneOrMany
pub enum OneOrManyIter<'a, T> {
    One(Option<&'a T>),
    Many(std::slice::Iter<'a, T>),
}

impl<'a, T> Iterator for OneOrManyIter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            OneOrManyIter::One(item) => item.take(),
            OneOrManyIter::Many(iter) => iter.next(),
        }
    }
}

/// IntoIterator for OneOrMany
pub enum OneOrManyIntoIter<T> {
    One(Option<T>),
    Many(std::vec::IntoIter<T>),
}

impl<T> Iterator for OneOrManyIntoIter<T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            OneOrManyIntoIter::One(item) => item.take(),
            OneOrManyIntoIter::Many(iter) => iter.next(),
        }
    }
}

impl Message {
    /// Create a user message with text content.
    pub fn user(text: impl Into<String>) -> Self {
        Message::User {
            content: OneOrMany::One(UserContent::Text(Text { text: text.into() })),
        }
    }

    /// Create an assistant message with text content.
    pub fn assistant(text: impl Into<String>) -> Self {
        Message::Assistant {
            id: None,
            reasoning_content: None,
            content: OneOrMany::One(AssistantContent::Text(Text { text: text.into() })),
        }
    }
}

impl From<String> for Message {
    fn from(text: String) -> Self {
        Message::user(text)
    }
}

impl From<&str> for Message {
    fn from(text: &str) -> Self {
        Message::user(text)
    }
}

/// Errors related to Message operations.
#[derive(Debug, Error)]
pub enum MessageError {
    #[error("Message conversion error: {0}")]
    ConversionError(String),
}

impl From<MessageError> for CompletionError {
    fn from(error: MessageError) -> Self {
        CompletionError::RequestError(error.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_error_display_pins_conversion_error_template() {
        assert_eq!(
            MessageError::ConversionError("unsupported role".to_string()).to_string(),
            "Message conversion error: unsupported role",
        );
    }

    /// `Message::user` must wrap text in a `User { content: One(Text { ... }) }`
    /// shape. Audit log emitters and provider adapters rely on this exact
    /// shape rather than the `Many` variant for single-message turns.
    #[test]
    fn message_user_constructor_produces_one_text_shape() {
        let msg = Message::user("hello");
        match msg {
            Message::User { content } => match content {
                OneOrMany::One(UserContent::Text(Text { text })) => {
                    assert_eq!(text, "hello");
                }
                other => panic!("expected One(Text), got {other:?}"),
            },
            other => panic!("expected User, got {other:?}"),
        }
    }

    /// `Message::assistant` produces an `Assistant` message with `id =
    /// None`, `reasoning_content = None`, and a single Text content
    /// item. The id/reasoning fields are filled in by provider adapters,
    /// not the constructor.
    #[test]
    fn message_assistant_constructor_leaves_id_and_reasoning_none() {
        let msg = Message::assistant("response");
        match msg {
            Message::Assistant {
                id,
                reasoning_content,
                content,
            } => {
                assert!(id.is_none(), "constructor must leave id None");
                assert!(
                    reasoning_content.is_none(),
                    "constructor must leave reasoning_content None",
                );
                match content {
                    OneOrMany::One(AssistantContent::Text(Text { text })) => {
                        assert_eq!(text, "response");
                    }
                    other => panic!("expected One(Text), got {other:?}"),
                }
            }
            other => panic!("expected Assistant, got {other:?}"),
        }
    }

    /// `From<String>` and `From<&str>` for `Message` must route to the
    /// `user()` constructor. This is the contract chat callers rely on
    /// when threading raw strings into the history vector.
    #[test]
    fn message_from_string_and_str_route_to_user() {
        let from_string: Message = "hi".to_string().into();
        let from_str: Message = "hi".into();
        assert!(matches!(from_string, Message::User { .. }));
        assert!(matches!(from_str, Message::User { .. }));
        assert_eq!(
            from_string, from_str,
            "both paths must produce same message"
        );
    }

    /// `OneOrMany::many(vec![])` returns `None` (not `Some(Many(vec![]))`),
    /// while a non-empty vec returns `Some(Many(...))`. This is the
    /// contract that lets callers branch on "is there any content"
    /// without inspecting the inner Vec length.
    #[test]
    fn one_or_many_many_constructor_collapses_empty_to_none() {
        assert!(OneOrMany::<i32>::many(vec![]).is_none());
        let some_two = OneOrMany::many(vec![1, 2]).expect("non-empty must be Some");
        match some_two {
            OneOrMany::Many(items) => assert_eq!(items, vec![1, 2]),
            other => panic!("expected Many, got {other:?}"),
        }
    }

    /// `OneOrMany::one(...)` constructs the `One` variant. Combined with
    /// `many()`, callers have two unambiguous entry points: single-item
    /// → `one()`, possibly-empty list → `many()`.
    #[test]
    fn one_or_many_one_constructor_produces_one_variant() {
        match OneOrMany::one(42_i32) {
            OneOrMany::One(item) => assert_eq!(item, 42),
            other => panic!("expected One, got {other:?}"),
        }
    }

    /// `OneOrMany::iter()` must yield exactly one item for `One` and
    /// the full sequence for `Many`. Pins the iterator-shape contract
    /// the provider adapters rely on for shared loop code.
    #[test]
    fn one_or_many_iter_yields_single_or_sequence() {
        let single = OneOrMany::one(7_i32);
        let collected: Vec<i32> = single.iter().copied().collect();
        assert_eq!(collected, vec![7]);

        let multi = OneOrMany::Many(vec![1, 2, 3]);
        let collected: Vec<i32> = multi.iter().copied().collect();
        assert_eq!(collected, vec![1, 2, 3]);
    }

    /// `IntoIterator` for `OneOrMany` must mirror `iter()` — one item
    /// for `One`, all items for `Many`. This is the by-value consume
    /// path used by adapters that don't need to retain the original
    /// `OneOrMany`.
    #[test]
    fn one_or_many_into_iter_consumes_in_order() {
        let single = OneOrMany::one("x".to_string());
        let collected: Vec<String> = single.into_iter().collect();
        assert_eq!(collected, vec!["x".to_string()]);

        let multi = OneOrMany::Many(vec!["a".to_string(), "b".to_string()]);
        let collected: Vec<String> = multi.into_iter().collect();
        assert_eq!(collected, vec!["a".to_string(), "b".to_string()]);
    }

    /// `Message::User` must serde-round-trip with the `role = "user"`
    /// tag at the top level. This is the provider-neutral wire
    /// contract every adapter assumes.
    #[test]
    fn message_user_serde_round_trip_preserves_role_tag() {
        let msg = Message::user("hello world");
        let json = serde_json::to_string(&msg).unwrap();
        assert!(
            json.contains("\"role\":\"user\""),
            "user role tag must be in the JSON; got {json}",
        );
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, back);
    }

    /// `Message::Assistant` round-trips with `role = "assistant"`.
    /// Pin so a serde tag rename gets caught at this boundary.
    #[test]
    fn message_assistant_serde_round_trip_preserves_role_tag() {
        let msg = Message::assistant("response text");
        let json = serde_json::to_string(&msg).unwrap();
        assert!(
            json.contains("\"role\":\"assistant\""),
            "assistant role tag must be in the JSON; got {json}",
        );
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, back);
    }

    /// `Text::text()` accessor returns the inner string slice without
    /// allocation. Used by provider adapters that thread Text content
    /// into rendering pipelines.
    #[test]
    fn text_accessor_returns_inner_slice() {
        let t = Text {
            text: "abc".to_string(),
        };
        assert_eq!(t.text(), "abc");
        // Display impl too.
        assert_eq!(format!("{t}"), "abc");
    }

    /// `MessageError` -> `CompletionError::RequestError` conversion
    /// must preserve the underlying error message (anyhow chain).
    #[test]
    fn message_error_into_completion_error_preserves_message() {
        let err = MessageError::ConversionError("foo".to_string());
        let completion: CompletionError = err.into();
        let rendered = format!("{completion:#}");
        assert!(
            rendered.contains("foo"),
            "completion error must surface the inner message; got {rendered}",
        );
    }
}
