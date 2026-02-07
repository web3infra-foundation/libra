//! Tool handler implementations.

pub mod apply_patch;
pub mod grep_files;
pub mod list_dir;
pub mod read_file;

pub use apply_patch::ApplyPatchHandler;
pub use grep_files::GrepFilesHandler;
pub use list_dir::ListDirHandler;
pub use read_file::ReadFileHandler;

/// Helper function to parse JSON arguments for tool handlers.
pub fn parse_arguments<T: serde::de::DeserializeOwned>(
    arguments: &str,
) -> crate::internal::ai::tools::ToolResult<T> {
    serde_json::from_str(arguments).map_err(|e| {
        crate::internal::ai::tools::error::ToolError::ParseError(format!(
            "Failed to parse arguments: {}",
            e
        ))
    })
}
