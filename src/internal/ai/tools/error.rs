//! Error types for tool operations.

use std::path::PathBuf;

use thiserror::Error;

use super::apply_patch::ApplyPatchError;

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

    /// Path is reserved for repository metadata and cannot be accessed by AI tools.
    #[error("Path reserved for repository metadata: {0}")]
    PathReserved(PathBuf),

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

impl From<ApplyPatchError> for ToolError {
    fn from(err: ApplyPatchError) -> Self {
        ToolError::ExecutionFailed(err.to_string())
    }
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

    #[test]
    fn tool_error_display_pins_each_variant() {
        assert_eq!(
            ToolError::InvalidArguments("missing offset".to_string()).to_string(),
            "Invalid arguments: missing offset",
        );
        assert_eq!(
            ToolError::PathNotAbsolute(PathBuf::from("relative/file")).to_string(),
            "Path must be absolute: relative/file",
        );
        assert_eq!(
            ToolError::PathOutsideWorkingDir(PathBuf::from("/etc/passwd")).to_string(),
            "Path outside working directory: /etc/passwd",
        );
        assert_eq!(
            ToolError::PathReserved(PathBuf::from(".libra/HEAD")).to_string(),
            "Path reserved for repository metadata: .libra/HEAD",
        );
        // The Io variant deserves a pin here even though
        // `test_tool_error_from_io` also formats it — the test name
        // promises every variant, and Io is the only one whose inner
        // is not a String or PathBuf, so a Display template change
        // (e.g. dropping `IO ` or swapping the colon for a dash)
        // could break callers that grep stdout for this exact string.
        assert_eq!(
            ToolError::Io(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "denied",
            ))
            .to_string(),
            "IO error: denied",
        );
        assert_eq!(
            ToolError::ParseError("expected object".to_string()).to_string(),
            "Failed to parse arguments: expected object",
        );
        assert_eq!(
            ToolError::ExecutionFailed("shell exited 1".to_string()).to_string(),
            "Tool execution failed: shell exited 1",
        );
        assert_eq!(
            ToolError::ToolNotFound("apply_patch".to_string()).to_string(),
            "Tool not found: apply_patch",
        );
        assert_eq!(
            ToolError::IncompatiblePayload("text".to_string()).to_string(),
            "Incompatible payload type for tool: text",
        );
        assert_eq!(
            ToolError::Other("unexpected".to_string()).to_string(),
            "Tool error: unexpected",
        );
    }
}
