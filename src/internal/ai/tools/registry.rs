//! Tool registry for managing and dispatching tool handlers.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use super::{
    context::{ToolInvocation, ToolKind, ToolOutput, ToolPayload},
    error::{ToolError, ToolResult},
    spec::ToolSpec,
};

/// Handler trait that all tools must implement.
///
/// This trait defines the interface for tools that can be invoked by an AI agent.
/// Tools are registered in the ToolRegistry and dispatched based on their name.
#[async_trait]
pub trait ToolHandler: Send + Sync {
    /// Returns the kind of tool (Function, Mcp, or Custom).
    fn kind(&self) -> ToolKind;

    /// Check if this handler matches the given payload kind.
    fn matches_kind(&self, payload: &ToolPayload) -> bool {
        matches!(
            (self.kind(), payload),
            (ToolKind::Function, ToolPayload::Function { .. })
                | (ToolKind::Custom, ToolPayload::Custom { .. })
                | (ToolKind::Mcp, ToolPayload::Mcp { .. })
        )
    }

    /// Returns `true` if the tool invocation *might* mutate the environment.
    /// This function should be defensive and return `true` if there's any doubt.
    async fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        false
    }

    /// Execute the tool with the given invocation context.
    ///
    /// Returns a ToolOutput containing the result to send back to the model.
    async fn handle(&self, invocation: ToolInvocation) -> ToolResult<ToolOutput>;

    /// Returns the JSON Schema for this tool's parameters.
    fn schema(&self) -> ToolSpec;
}

/// Registry for managing tool handlers and dispatching tool calls.
///
/// The ToolRegistry maintains a mapping of tool names to their handlers
/// and provides methods to register, retrieve, and dispatch tools.
#[derive(Clone)]
pub struct ToolRegistry {
    /// Map of tool name to handler implementation.
    handlers: HashMap<String, Arc<dyn ToolHandler>>,
    /// Working directory for file operations.
    working_dir: std::path::PathBuf,
}

impl ToolRegistry {
    /// Create a new empty ToolRegistry.
    pub fn new() -> Self {
        Self::try_new().unwrap_or_else(|err| {
            panic!(
                "failed to resolve current working directory for ToolRegistry::new(): {err}"
            )
        })
    }

    /// Try to create a new empty ToolRegistry from the current working directory.
    pub fn try_new() -> std::io::Result<Self> {
        Ok(Self {
            handlers: HashMap::new(),
            working_dir: std::env::current_dir()?,
        })
    }

    /// Create a new ToolRegistry with a specific working directory.
    pub fn with_working_dir(working_dir: std::path::PathBuf) -> Self {
        Self {
            handlers: HashMap::new(),
            working_dir,
        }
    }

    /// Register a tool handler with the given name.
    pub fn register(&mut self, name: impl Into<String>, handler: Arc<dyn ToolHandler>) {
        let name = name.into();
        if self.handlers.insert(name.clone(), handler).is_some() {
            tracing::warn!("Overwriting handler for tool: {name}");
        }
    }

    /// Register multiple tool handlers from a map.
    pub fn register_all(&mut self, handlers: HashMap<String, Arc<dyn ToolHandler>>) {
        for (name, handler) in handlers {
            self.register(name, handler);
        }
    }

    /// Get a handler by name.
    pub fn handler(&self, name: &str) -> Option<Arc<dyn ToolHandler>> {
        self.handlers.get(name).cloned()
    }

    /// Get all registered tool names.
    pub fn tool_names(&self) -> Vec<String> {
        self.handlers.keys().cloned().collect()
    }

    /// Get all tool specs as a vector of JSON values.
    pub fn tool_specs(&self) -> Vec<ToolSpec> {
        self.handlers
            .values()
            .map(|handler| handler.schema())
            .collect()
    }

    /// Get all tool specs as JSON values for API requests.
    pub fn tool_specs_json(&self) -> Vec<serde_json::Value> {
        self.tool_specs()
            .into_iter()
            .map(|spec| spec.to_json())
            .collect()
    }

    /// Dispatch a tool invocation to the appropriate handler.
    ///
    /// This method validates the tool name, checks payload compatibility,
    /// and executes the tool.
    pub async fn dispatch(&self, mut invocation: ToolInvocation) -> ToolResult<ToolOutput> {
        let tool_name = invocation.tool_name.clone();

        let handler = self
            .handler(&tool_name)
            .ok_or_else(|| ToolError::ToolNotFound(tool_name.clone()))?;

        if !handler.matches_kind(&invocation.payload) {
            return Err(ToolError::IncompatiblePayload(format!(
                "Tool {tool_name} received incompatible payload type"
            )));
        }

        // The registry working directory is the single source of truth for sandboxing.
        // Ignore any caller-provided working_dir to prevent sandbox bypass.
        invocation.working_dir = self.working_dir.clone();

        handler.handle(invocation).await
    }

    /// Get the current working directory.
    pub fn working_dir(&self) -> &std::path::Path {
        &self.working_dir
    }

    /// Set the working directory.
    pub fn set_working_dir(&mut self, dir: std::path::PathBuf) {
        self.working_dir = dir;
    }

    /// Check if a tool is registered.
    pub fn contains_tool(&self, name: &str) -> bool {
        self.handlers.contains_key(name)
    }

    /// Get the number of registered tools.
    pub fn len(&self) -> usize {
        self.handlers.len()
    }

    /// Check if the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for constructing a ToolRegistry with multiple handlers.
pub struct ToolRegistryBuilder {
    registry: ToolRegistry,
}

impl ToolRegistryBuilder {
    /// Create a new ToolRegistryBuilder.
    pub fn new() -> Self {
        Self::try_new().unwrap_or_else(|err| {
            panic!(
                "failed to resolve current working directory for ToolRegistryBuilder::new(): {err}"
            )
        })
    }

    /// Try to create a new ToolRegistryBuilder.
    pub fn try_new() -> std::io::Result<Self> {
        Ok(Self {
            registry: ToolRegistry::try_new()?,
        })
    }

    /// Create a new ToolRegistryBuilder with a specific working directory.
    pub fn with_working_dir(working_dir: std::path::PathBuf) -> Self {
        Self {
            registry: ToolRegistry::with_working_dir(working_dir),
        }
    }

    /// Register a tool handler.
    pub fn register(mut self, name: impl Into<String>, handler: Arc<dyn ToolHandler>) -> Self {
        self.registry.register(name, handler);
        self
    }

    /// Set the working directory.
    pub fn working_dir(mut self, dir: std::path::PathBuf) -> Self {
        self.registry.set_working_dir(dir);
        self
    }

    /// Build the ToolRegistry.
    pub fn build(self) -> ToolRegistry {
        self.registry
    }
}

impl Default for ToolRegistryBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::tools::handlers::{ListDirHandler, ReadFileHandler};
    use crate::internal::ai::tools::context::ToolPayload;
    use tempfile::TempDir;

    // Mock handler for testing
    struct MockHandler;

    #[async_trait]
    impl ToolHandler for MockHandler {
        fn kind(&self) -> ToolKind {
            ToolKind::Function
        }

        async fn handle(&self, _invocation: ToolInvocation) -> ToolResult<ToolOutput> {
            Ok(ToolOutput::success("mock result"))
        }

        fn schema(&self) -> ToolSpec {
            ToolSpec::new("mock", "A mock tool")
        }
    }

    #[tokio::test]
    async fn test_registry_registration() {
        let mut registry = ToolRegistry::new();
        registry.register("mock", Arc::new(MockHandler));

        assert!(registry.contains_tool("mock"));
        assert!(!registry.contains_tool("nonexistent"));
        assert_eq!(registry.len(), 1);
    }

    #[tokio::test]
    async fn test_registry_dispatch() {
        let mut registry = ToolRegistry::new();
        registry.register("mock", Arc::new(MockHandler));

        let invocation = ToolInvocation::new(
            "call-1",
            "mock",
            ToolPayload::Function {
                arguments: "{}".to_string(),
            },
            std::path::PathBuf::from("/tmp"),
        );

        let result = registry.dispatch(invocation).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_text(), Some("mock result"));
    }

    #[tokio::test]
    async fn test_registry_tool_not_found() {
        let registry = ToolRegistry::new();

        let invocation = ToolInvocation::new(
            "call-1",
            "nonexistent",
            ToolPayload::Function {
                arguments: "{}".to_string(),
            },
            std::path::PathBuf::from("/tmp"),
        );

        let result = registry.dispatch(invocation).await;
        assert!(matches!(result, Err(ToolError::ToolNotFound(_))));
    }

    #[tokio::test]
    async fn test_registry_incompatible_payload() {
        let mut registry = ToolRegistry::new();
        registry.register("mock", Arc::new(MockHandler));

        // MockHandler is Function kind, but we send Custom payload
        let invocation = ToolInvocation::new(
            "call-1",
            "mock",
            ToolPayload::Custom {
                input: "test".to_string(),
            },
            std::path::PathBuf::from("/tmp"),
        );

        let result = registry.dispatch(invocation).await;
        assert!(matches!(result, Err(ToolError::IncompatiblePayload(_))));
    }

    #[test]
    fn test_tool_specs() {
        let mut registry = ToolRegistry::new();
        registry.register("mock", Arc::new(MockHandler));

        let specs = registry.tool_specs();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].function.name, "mock");

        let specs_json = registry.tool_specs_json();
        assert_eq!(specs_json.len(), 1);
        assert_eq!(specs_json[0]["function"]["name"], "mock");
    }

    #[test]
    fn test_registry_builder() {
        let registry = ToolRegistryBuilder::new()
            .register("mock", Arc::new(MockHandler))
            .working_dir("/tmp".into())
            .build();

        assert!(registry.contains_tool("mock"));
        assert_eq!(registry.working_dir(), std::path::Path::new("/tmp"));
    }

    #[tokio::test]
    async fn test_registry_dispatch_real_handlers() {
        let temp_dir = TempDir::new().unwrap();
        let working_dir = temp_dir.path().to_path_buf();
        let file_path = working_dir.join("test.txt");
        tokio::fs::write(&file_path, "hello").await.unwrap();

        let mut registry = ToolRegistry::with_working_dir(working_dir.clone());
        registry.register("read_file", Arc::new(ReadFileHandler));
        registry.register("list_dir", Arc::new(ListDirHandler));

        let read_invocation = ToolInvocation::new(
            "call-1",
            "read_file",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "file_path": file_path,
                    "offset": 1,
                    "limit": 10
                })
                .to_string(),
            },
            working_dir.clone(),
        );

        let read_result = registry.dispatch(read_invocation).await;
        assert!(read_result.is_ok());
        let read_text = read_result.unwrap().as_text().unwrap().to_string();
        assert!(read_text.contains("hello"));

        let list_invocation = ToolInvocation::new(
            "call-2",
            "list_dir",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "dir_path": working_dir,
                    "max_depth": 1
                })
                .to_string(),
            },
            temp_dir.path().to_path_buf(),
        );

        let list_result = registry.dispatch(list_invocation).await;
        assert!(list_result.is_ok());
    }
}
