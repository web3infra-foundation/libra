//! Wire-level helpers shared across provider builders (OC-Phase 4 P4.2).
//!
//! Each provider's `completion.rs` translates the canonical
//! [`CompletionRequest`](crate::internal::ai::completion::CompletionRequest)
//! into a vendor-specific wire payload. A handful of formatting decisions —
//! how to stringify a `ToolResult.result` value, how to coerce arbitrary
//! JSON into the `arguments` string OpenAI's tool-call schema requires —
//! are duplicated across providers and silently drift. This module
//! centralises those decisions so the provider builders stay short and a
//! quirk fix lands in one place instead of N.
//!
//! Helpers here are intentionally tiny (no provider-specific knowledge)
//! and infallible (every input shape produces *some* string output) so
//! the wire builder never crashes on a syntactically-valid canonical
//! input. Tests for the helpers live next to the helpers; quirk tests
//! that prove each provider's `build_messages` wires through correctly
//! live next to each provider's wire builder.

use serde_json::Value;

/// Render a `ToolResult.result` value as the string the OpenAI-compatible
/// `tool` role and the Anthropic `tool_result` block both expect.
///
/// Both provider families serialise the canonical `serde_json::Value` to
/// a JSON string and embed it in a single content field. The fallback
/// path uses `Value::to_string()` so a value that fails to serialise via
/// `serde_json::to_string` (vanishingly rare in practice — only happens
/// for floats containing NaN or Infinity, which the canonical schema does
/// not permit) still produces a non-empty payload rather than disappearing
/// from the wire.
///
/// Idempotent: applying it twice (e.g. through a retry middleware) on the
/// same input produces identical output.
pub fn serialize_tool_result_content(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn serialize_tool_result_content_handles_object_value() {
        // Object keys serialise in alphabetical order (serde_json's
        // default), but the receiver only cares about structural
        // equivalence, so assert via round-trip rather than literal
        // string equality.
        let value = json!({"path": "Cargo.toml", "ok": true});
        let out = serialize_tool_result_content(&value);
        let reparsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(reparsed, value);
    }

    #[test]
    fn serialize_tool_result_content_handles_string_value() {
        // A bare string is valid JSON; the helper produces a quoted
        // representation so the receiver can `JSON.parse(...)` if needed.
        let value = json!("hello");
        let out = serialize_tool_result_content(&value);
        assert_eq!(out, "\"hello\"");
    }

    #[test]
    fn serialize_tool_result_content_handles_array_value() {
        let value = json!(["a", 1, true, null]);
        let out = serialize_tool_result_content(&value);
        assert_eq!(out, "[\"a\",1,true,null]");
    }

    #[test]
    fn serialize_tool_result_content_handles_null_value() {
        // `null` is a valid JSON value; the helper preserves it so the
        // receiving provider sees `"null"` (a parseable JSON literal)
        // rather than an empty string that some APIs treat as missing.
        let value = json!(null);
        let out = serialize_tool_result_content(&value);
        assert_eq!(out, "null");
    }

    #[test]
    fn serialize_tool_result_content_handles_nested_value() {
        let value = json!({"outer": {"inner": [1, 2, 3]}});
        let out = serialize_tool_result_content(&value);
        let reparsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(reparsed, value);
    }

    #[test]
    fn serialize_tool_result_content_is_idempotent_via_double_parse() {
        let value = json!({"path": "Cargo.toml"});
        let first = serialize_tool_result_content(&value);
        let reparsed: Value = serde_json::from_str(&first).unwrap();
        let second = serialize_tool_result_content(&reparsed);
        assert_eq!(first, second);
    }
}
