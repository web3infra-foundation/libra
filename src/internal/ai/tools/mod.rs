use std::error::Error;

use serde_json::Value;

/// A trait representing a tool that can be invoked by an AI agent.
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

/// A collection of tools available to an AI agent.
#[derive(Default)]
pub struct ToolSet {
    /// The list of tools in the set.
    pub tools: Vec<Box<dyn Tool>>,
}
