//! Error types for tool operations.

use std::path::PathBuf;
use thiserror::Error;

/// Errors that can occur during tool execution.
#[derive(Debug, Error)]
pub enum ToolError {
    /// Invalid arguments provided to the tool.
    #[error("Invalid arguments: {0}")]
    InvalidArguments(String),

    /// Path is not absolute.
    #[error("Path must be absolute: {0}")]
    PathNotAbsolute(PathBuf),

    /// Path is outside the allowed working directory.
    #[error("Path outside working directory: {0}")]
    PathOutsideWorkingDir(PathBuf),

    /// IO error during file operation.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Failed to parse tool arguments.
    #[error("Failed to parse arguments: {0}")]
    ParseError(String),

    /// Tool execution failed.
    #[error("Tool execution failed: {0}")]
    ExecutionFailed(String),

    /// Tool not found in registry.
    #[error("Tool not found: {0}")]
    ToolNotFound(String),

    /// Incompatible payload type for the tool.
    #[error("Incompatible payload type for tool: {0}")]
    IncompatiblePayload(String),

    /// Generic tool error.
    #[error("Tool error: {0}")]
    Other(String),
}

/// Result type for tool operations.
pub type ToolResult<T> = Result<T, ToolError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_error_display() {
        let err = ToolError::InvalidArguments("offset must be positive".to_string());
        assert_eq!(
            err.to_string(),
            "Invalid arguments: offset must be positive"
        );
    }

    #[test]
    fn test_tool_error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let tool_err: ToolError = io_err.into();
        assert!(matches!(tool_err, ToolError::Io(_)));
        assert_eq!(tool_err.to_string(), "IO error: file not found");
    }
}
