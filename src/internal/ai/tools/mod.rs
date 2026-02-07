use std::error::Error;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A definition of a tool that can be used by the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// The name of the tool.
    pub name: String,
    /// A description of what the tool does.
    pub description: String,
    /// The parameters of the tool in JSON Schema format.
    pub parameters: Value,
}

/// A trait representing a tool that can be invoked by an AI agent.
pub trait Tool: Send + Sync {
    /// Returns the name of the tool.
    fn name(&self) -> String {
        self.definition().name
    }

    /// Returns a description of what the tool does.
    fn description(&self) -> String {
        self.definition().description
    }

    /// Returns the definition of the tool.
    fn definition(&self) -> ToolDefinition;

    /// Executes the tool with the given arguments.
    ///
    /// # Arguments
    /// * `args` - A JSON Value containing the arguments for the tool execution.
    ///
    /// # Returns
    /// A Result containing the tool's output as a JSON Value or an error.
    fn call(&self, args: Value) -> Result<Value, Box<dyn Error + Send + Sync>>;
}

/// A collection of tools available to an AI agent.
#[derive(Default)]
pub struct ToolSet {
    /// The list of tools in the set.
    pub tools: Vec<Box<dyn Tool>>,
}
