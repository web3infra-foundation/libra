//! Tool handler implementations.

pub mod apply_patch;
pub mod grep_files;
pub mod list_dir;
pub mod mcp_bridge;
pub mod plan;
pub mod read_file;
pub mod request_user_input;
pub mod shell;
pub mod submit_intent_draft;

pub use apply_patch::ApplyPatchHandler;
pub use grep_files::{GrepFilesHandler, SearchFilesHandler};
pub use list_dir::ListDirHandler;
pub use mcp_bridge::McpBridgeHandler;
pub use plan::PlanHandler;
pub use read_file::ReadFileHandler;
pub use request_user_input::RequestUserInputHandler;
use serde_json::Value;
pub use shell::ShellHandler;
pub use submit_intent_draft::SubmitIntentDraftHandler;

use crate::internal::ai::tools::{ToolResult, error::ToolError};

const MAX_JSON_STRING_UNWRAP_DEPTH: usize = 3;

/// Helper function to parse JSON arguments for tool handlers.
pub fn parse_arguments<T: serde::de::DeserializeOwned>(arguments: &str) -> ToolResult<T> {
    let value = parse_argument_value(arguments)?;
    serde_json::from_value(value)
        .map_err(|e| ToolError::ParseError(format!("Failed to parse arguments: {}", e)))
}

/// Parses a tool argument string and unwraps accidental JSON-string envelopes.
///
/// Some providers can return function-call arguments as a JSON string containing
/// the real JSON object, for example `"{\"dir_path\":\".\"}"`. Tool handlers
/// should see the inner object, not the transport artifact.
pub(crate) fn parse_argument_value(arguments: &str) -> ToolResult<Value> {
    let value: Value = serde_json::from_str(arguments)
        .map_err(|e| ToolError::ParseError(format!("Failed to parse arguments: {}", e)))?;
    unwrap_json_string_value(value)
}

pub(crate) fn unwrap_json_string_value(mut value: Value) -> ToolResult<Value> {
    for _ in 0..MAX_JSON_STRING_UNWRAP_DEPTH {
        let Value::String(raw) = &value else {
            return Ok(value);
        };

        let trimmed = raw.trim();
        if !looks_like_json_container(trimmed) {
            return Ok(value);
        }

        value = serde_json::from_str(trimmed).map_err(|e| {
            ToolError::ParseError(format!(
                "Failed to parse JSON encoded in string arguments: {}",
                e
            ))
        })?;
    }

    Ok(value)
}

fn looks_like_json_container(value: &str) -> bool {
    matches!(
        value.as_bytes().first(),
        Some(b'{') | Some(b'[') | Some(b'"')
    )
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Args {
        dir_path: String,
    }

    #[test]
    fn parse_arguments_accepts_normal_object_arguments() {
        let args: Args = parse_arguments(r#"{"dir_path":"."}"#).unwrap();

        assert_eq!(args.dir_path, ".");
    }

    #[test]
    fn parse_arguments_accepts_json_string_encoded_object_arguments() {
        let encoded = serde_json::to_string(r#"{"dir_path":"."}"#).unwrap();

        let args: Args = parse_arguments(&encoded).unwrap();

        assert_eq!(args.dir_path, ".");
    }

    #[test]
    fn parse_arguments_accepts_nested_json_string_encoded_object_arguments() {
        let once = serde_json::to_string(r#"{"dir_path":"."}"#).unwrap();
        let twice = serde_json::to_string(&once).unwrap();

        let args: Args = parse_arguments(&twice).unwrap();

        assert_eq!(args.dir_path, ".");
    }
}
