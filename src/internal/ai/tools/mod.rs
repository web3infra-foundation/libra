//! Tool calling infrastructure for AI agents.

use std::{error::Error, sync::Arc};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod apply_patch;
pub mod context;
pub mod error;
pub mod handlers;
pub mod registry;
pub mod spec;
pub mod utils;

pub use context::{
    GrepFilesArgs, ListDirArgs, ReadFileArgs, ToolInvocation, ToolKind, ToolOutput, ToolPayload,
};
pub use error::{ToolError, ToolResult};
pub use registry::{ToolHandler, ToolRegistry, ToolRegistryBuilder};
pub use spec::{FunctionDefinition, FunctionParameters, ToolSpec, ToolSpecBuilder};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

pub trait Tool: Send + Sync {
    fn name(&self) -> String {
        self.definition().name
    }

    fn description(&self) -> String {
        self.definition().description
    }

    fn definition(&self) -> ToolDefinition;

    fn call(&self, args: Value) -> Result<Value, Box<dyn Error + Send + Sync>>;
}

#[derive(Default, Clone)]
pub struct ToolSet {
    pub tools: Vec<Arc<dyn Tool>>,
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
