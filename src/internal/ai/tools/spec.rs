//! Tool specification types for OpenAI-compatible function calling.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// A tool specification compatible with OpenAI's function calling format.
///
/// This struct defines the interface for a tool that can be called by an LLM.
/// It follows the JSON Schema format for function definitions.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolSpec {
    /// The type of tool (always "function" for function calling).
    #[serde(rename = "type")]
    pub spec_type: String,

    /// The function definition.
    pub function: FunctionDefinition,
}

impl ToolSpec {
    /// Create a new ToolSpec.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            spec_type: "function".to_string(),
            function: FunctionDefinition {
                name: name.into(),
                description: description.into(),
                parameters: FunctionParameters::Empty,
            },
        }
    }

    /// Set the parameters for this tool.
    pub fn with_parameters(mut self, parameters: FunctionParameters) -> Self {
        self.function.parameters = parameters;
        self
    }

    /// Create a ToolSpec for read_file.
    pub fn read_file() -> Self {
        Self::new(
            "read_file",
            "Read the contents of a file. Returns the file content with line numbers.",
        )
        .with_parameters(FunctionParameters::object(
            [("file_path", "string", "Absolute path to the file to read")],
            [("file_path", true)],
        ))
    }

    /// Create a ToolSpec for list_dir.
    pub fn list_dir() -> Self {
        Self::new(
            "list_dir",
            "List the contents of a directory. Can list recursively with depth control.",
        )
        .with_parameters(FunctionParameters::object(
            [(
                "dir_path",
                "string",
                "Absolute path to the directory to list",
            )],
            [("dir_path", true)],
        ))
    }

    /// Create a ToolSpec for grep_files.
    pub fn grep_files() -> Self {
        Self::new(
            "grep_files",
            "Search for a pattern in files using ripgrep. Supports regex patterns.",
        )
        .with_parameters(FunctionParameters::object(
            [
                ("pattern", "string", "Search pattern (supports regex)"),
                ("path", "string", "Absolute path to search in"),
            ],
            [("pattern", true), ("path", true)],
        ))
    }

    /// Create a ToolSpec for apply_patch.
    pub fn apply_patch() -> Self {
        Self::new(
            "apply_patch",
            "Apply a patch to files using Codex-style format. \
             Format: *** Begin Patch, followed by hunks (*** Add File:/Delete File:/Update File:), \
             then *** End Patch. Supports adding, deleting, updating, and moving files. \
             Paths are relative to the working directory.",
        )
        .with_parameters(FunctionParameters::object(
            [("patch", "string", "The patch in Codex format")],
            [("patch", true)],
        ))
    }

    /// Convert to a JSON value for API requests.
    pub fn to_json(&self) -> Value {
        json!(self)
    }
}

/// Function definition containing name, description, and parameters.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FunctionDefinition {
    /// The name of the function to be called.
    pub name: String,

    /// A description of what the function does.
    pub description: String,

    /// The parameters the function accepts.
    pub parameters: FunctionParameters,
}

/// Function parameters definition (JSON Schema format).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FunctionParameters {
    /// No parameters (empty object).
    Empty,
    /// Object with properties.
    Object {
        /// Type is always "object".
        #[serde(rename = "type")]
        param_type: String,
        /// Property definitions.
        properties: Map<String, Value>,
        /// Required properties.
        required: Vec<String>,
    },
}

impl FunctionParameters {
    /// Create empty parameters (no arguments).
    pub fn empty() -> Self {
        Self::Empty
    }

    /// Create object parameters from property definitions.
    ///
    /// # Arguments
    /// * `properties` - Array of (name, type, description) tuples
    /// * `required` - Array of (name, is_required) tuples
    pub fn object<const N: usize, const M: usize>(
        properties: [(&str, &str, &str); N],
        required: [(&str, bool); M],
    ) -> Self {
        let mut props = Map::new();
        let mut req = Vec::new();

        for (name, param_type, description) in properties {
            props.insert(
                name.to_string(),
                json!({
                    "type": param_type,
                    "description": description
                }),
            );
        }

        for (name, is_required) in required {
            if is_required {
                req.push(name.to_string());
            }
        }

        Self::Object {
            param_type: "object".to_string(),
            properties: props,
            required: req,
        }
    }

    /// Add a property to the parameters.
    pub fn add_property(mut self, name: &str, param_type: &str, description: &str) -> Self {
        if let Self::Object {
            ref mut properties, ..
        } = self
        {
            properties.insert(
                name.to_string(),
                json!({
                    "type": param_type,
                    "description": description
                }),
            );
        }
        self
    }

    /// Add a required property.
    pub fn add_required(mut self, name: &str) -> Self {
        if let Self::Object {
            ref mut required, ..
        } = self
        {
            required.push(name.to_string());
        }
        self
    }
}

/// Helper to create a basic tool spec builder.
pub struct ToolSpecBuilder {
    name: String,
    description: String,
    parameters: Map<String, Value>,
    required: Vec<String>,
}

impl ToolSpecBuilder {
    /// Create a new tool spec builder.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters: Map::new(),
            required: Vec::new(),
        }
    }

    /// Add a parameter to the tool spec.
    pub fn param(mut self, name: &str, param_type: &str, description: &str) -> Self {
        self.parameters.insert(
            name.to_string(),
            json!({
                "type": param_type,
                "description": description
            }),
        );
        self
    }

    /// Mark a parameter as required.
    pub fn required(mut self, name: &str) -> Self {
        if !self.required.contains(&name.to_string()) {
            self.required.push(name.to_string());
        }
        self
    }

    /// Build the ToolSpec.
    pub fn build(self) -> ToolSpec {
        ToolSpec {
            spec_type: "function".to_string(),
            function: FunctionDefinition {
                name: self.name,
                description: self.description,
                parameters: if self.parameters.is_empty() {
                    FunctionParameters::Empty
                } else {
                    FunctionParameters::Object {
                        param_type: "object".to_string(),
                        properties: self.parameters,
                        required: self.required,
                    }
                },
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_spec_creation() {
        let spec = ToolSpec::new("test_tool", "A test tool");
        assert_eq!(spec.spec_type, "function");
        assert_eq!(spec.function.name, "test_tool");
        assert_eq!(spec.function.description, "A test tool");
    }

    #[test]
    fn test_tool_spec_read_file() {
        let spec = ToolSpec::read_file();
        assert_eq!(spec.function.name, "read_file");
        assert!(spec.function.description.contains("file"));

        // Check it serializes correctly
        let json = spec.to_json();
        assert_eq!(json["type"], "function");
        assert_eq!(json["function"]["name"], "read_file");
    }

    #[test]
    fn test_function_parameters_empty() {
        let params = FunctionParameters::empty();
        match params {
            FunctionParameters::Empty => {}
            _ => panic!("Expected Empty"),
        }
    }

    #[test]
    fn test_function_parameters_object() {
        let params =
            FunctionParameters::object([("test", "string", "A test parameter")], [("test", true)]);

        match params {
            FunctionParameters::Object {
                param_type,
                properties,
                required,
            } => {
                assert_eq!(param_type, "object");
                assert!(properties.contains_key("test"));
                assert_eq!(required, vec!["test"]);
            }
            _ => panic!("Expected Object"),
        }
    }

    #[test]
    fn test_tool_spec_builder() {
        let spec = ToolSpecBuilder::new("my_tool", "My custom tool")
            .param("input", "string", "The input value")
            .required("input")
            .build();

        assert_eq!(spec.function.name, "my_tool");
        match spec.function.parameters {
            FunctionParameters::Object {
                properties,
                required,
                ..
            } => {
                assert!(properties.contains_key("input"));
                assert_eq!(required, vec!["input"]);
            }
            _ => panic!("Expected Object parameters"),
        }
    }

    #[test]
    fn test_tool_spec_serialization() {
        let spec = ToolSpec::read_file();
        let json_str = serde_json::to_string(&spec).unwrap();
        let parsed: Value = serde_json::from_str(&json_str).unwrap();

        assert_eq!(parsed["type"], "function");
        assert_eq!(parsed["function"]["name"], "read_file");
    }
}
