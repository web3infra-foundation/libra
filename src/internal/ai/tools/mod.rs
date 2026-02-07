//! Tool calling infrastructure for AI agents.
//!
//! This module provides the foundational components for implementing
//! function calling with LLMs. It includes:
//!
//! - **Registry**: Tool registration and dispatch
//! - **Handlers**: File system tools (read_file, list_dir, grep_files, apply_patch)
//! - **Context**: Invocation and execution context
//! - **Spec**: OpenAI-compatible tool definitions

// Core modules
pub mod context;
pub mod error;
pub mod registry;
pub mod spec;
pub mod utils;

// Tool handler implementations
pub mod handlers;

// Re-exports
pub use context::{
    ApplyPatchArgs, GrepFilesArgs, ListDirArgs, ReadFileArgs, ToolInvocation, ToolKind, ToolOutput,
    ToolPayload,
};
pub use error::{ToolError, ToolResult};
pub use registry::{ToolHandler, ToolRegistry, ToolRegistryBuilder};
pub use spec::{FunctionDefinition, FunctionParameters, ToolSpec, ToolSpecBuilder};

// Legacy support - keep the old Tool trait for backward compatibility
use serde_json::Value;
use std::error::Error;

/// A trait representing a tool that can be invoked by an AI agent (legacy).
///
/// This is kept for backward compatibility. New code should use the
/// ToolHandler trait from the registry module instead.
pub trait Tool: Send + Sync {
    /// Returns the name of the tool.
    fn name(&self) -> String;

    /// Returns a description of what the tool does.
    fn description(&self) -> String;

    /// Executes the tool with the given arguments.
    ///
    /// # Arguments
    /// * `args` - A JSON Value containing the arguments for the tool execution.
    ///
    /// # Returns
    /// A Result containing the tool's output as a JSON Value or an error.
    fn call(&self, args: Value) -> Result<Value, Box<dyn Error + Send + Sync>>;
}

/// A collection of tools available to an AI agent (legacy).
///
/// This is kept for backward compatibility. New code should use ToolRegistry.
#[derive(Default)]
pub struct ToolSet {
    /// The list of tools in the set.
    pub tools: Vec<Box<dyn Tool>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_specs_creation() {
        let read_file_spec = ToolSpec::read_file();
        assert_eq!(read_file_spec.function.name, "read_file");

        let list_dir_spec = ToolSpec::list_dir();
        assert_eq!(list_dir_spec.function.name, "list_dir");

        let grep_spec = ToolSpec::grep_files();
        assert_eq!(grep_spec.function.name, "grep_files");

        let patch_spec = ToolSpec::apply_patch();
        assert_eq!(patch_spec.function.name, "apply_patch");
    }
}
