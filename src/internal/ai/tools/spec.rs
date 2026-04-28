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
            "Read the contents of a file. Returns the file content with each line prefixed as 'L{n}: content'. Blank lines appear as 'L{n}: ' (nothing after the space).",
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
            "Lists entries in a local directory with 1-indexed entry numbers and type labels (/ for dirs, @ for symlinks).",
        )
        .with_parameters(FunctionParameters::object(
            [
                ("dir_path", "string", "Absolute path to the directory to list"),
                ("offset", "integer", "1-indexed entry number to start listing from (default: 1)"),
                ("limit", "integer", "Maximum number of entries to return (default: 25)"),
                ("depth", "integer", "Maximum directory depth to traverse (default: 2, must be >= 1)"),
            ],
            [("dir_path", true)],
        ))
    }

    /// Create a ToolSpec for grep_files.
    pub fn grep_files() -> Self {
        Self::new(
            "grep_files",
            "Finds files whose contents match the pattern and lists them sorted by modification time.",
        )
        .with_parameters(FunctionParameters::object(
            [
                ("pattern", "string", "Regular expression pattern to search for"),
                ("include", "string", "Optional glob limiting which files are searched (e.g. \"*.rs\" or \"*.{ts,tsx}\")"),
                ("path", "string", "Directory or file path to search (defaults to the working directory)"),
                ("limit", "integer", "Maximum number of file paths to return (default: 100, max: 2000)"),
            ],
            [("pattern", true)],
        ))
    }

    /// Create a ToolSpec for web_search.
    pub fn web_search() -> Self {
        Self::new(
            "web_search",
            "Search the public web for current facts and return result titles, URLs, and snippets. Use this before making claims about changing external facts such as language/toolchain support, package versions, APIs, standards, or vendor behavior.",
        )
        .with_parameters(FunctionParameters::object(
            [
                ("query", "string", "Search query"),
                ("limit", "integer", "Maximum number of results to return (default: 5, max: 10)"),
            ],
            [("query", true)],
        ))
    }

    /// Create a ToolSpec for shell.
    pub fn shell() -> Self {
        Self::new(
            "shell",
            "Execute a shell command or script in the user's default shell (e.g., bash, zsh). \
             Returns the exit code and captured stdout/stderr. \
             Use for running build commands, tests, scripts, and other shell operations.",
        )
        .with_parameters(FunctionParameters::object(
            [
                ("command", "string", "Shell command or script to execute"),
                (
                    "workdir",
                    "string",
                    "Working directory (absolute or sandbox-relative, and within the sandbox)",
                ),
                (
                    "timeout_ms",
                    "number",
                    "Timeout in milliseconds (default: 60000)",
                ),
                (
                    "sandbox_permissions",
                    "string",
                    "Sandbox override: use_default (default) or require_escalated",
                ),
                (
                    "justification",
                    "string",
                    "Reason for requesting escalated execution outside sandbox",
                ),
            ],
            [
                ("command", true),
                ("workdir", false),
                ("timeout_ms", false),
                ("sandbox_permissions", false),
                ("justification", false),
            ],
        ))
    }

    /// Create a ToolSpec for update_plan.
    pub fn update_plan() -> Self {
        Self {
            spec_type: "function".to_string(),
            function: FunctionDefinition {
                name: "update_plan".to_string(),
                description: "Update the current plan with a list of steps and their status. \
                    Use this tool to track progress on multi-step tasks."
                    .to_string(),
                parameters: FunctionParameters::Object {
                    param_type: "object".to_string(),
                    properties: {
                        let mut props = Map::new();
                        props.insert(
                            "explanation".to_string(),
                            json!({
                                "type": "string",
                                "description": "Optional explanation of what changed since the last plan update"
                            }),
                        );
                        props.insert(
                            "plan".to_string(),
                            json!({
                                "type": "array",
                                "description": "The full plan, expressed as an ordered list of steps",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "step": {
                                            "type": "string",
                                            "description": "Human-readable description of the step"
                                        },
                                        "status": {
                                            "type": "string",
                                            "enum": ["pending", "in_progress", "completed"],
                                            "description": "Current status of the step"
                                        }
                                    },
                                    "required": ["step", "status"]
                                }
                            }),
                        );
                        props
                    },
                    required: vec!["plan".to_string()],
                    definitions: None,
                },
            },
        }
    }

    /// Create a ToolSpec for submit_plan_draft.
    pub fn submit_plan_draft() -> Self {
        Self {
            spec_type: "function".to_string(),
            function: FunctionDefinition {
                name: "submit_plan_draft".to_string(),
                description: "Submit an ordered provider draft for Phase 1 execution planning. \
                    Use this only to propose plan step titles before the local planner compiles formal tasks."
                    .to_string(),
                parameters: FunctionParameters::Object {
                    param_type: "object".to_string(),
                    properties: {
                        let mut props = Map::new();
                        props.insert(
                            "explanation".to_string(),
                            json!({
                                "type": "string",
                                "description": "Optional rationale for the proposed draft"
                            }),
                        );
                        props.insert(
                            "steps".to_string(),
                            json!({
                                "type": "array",
                                "minItems": 1,
                                "description": "Ordered draft step titles. Do not include runtime status.",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "title": {
                                            "type": "string",
                                            "description": "Human-readable draft step title"
                                        }
                                    },
                                    "required": ["title"]
                                }
                            }),
                        );
                        props
                    },
                    required: vec!["steps".to_string()],
                    definitions: None,
                },
            },
        }
    }

    /// Create a ToolSpec for submit_task_complete.
    pub fn submit_task_complete() -> Self {
        Self {
            spec_type: "function".to_string(),
            function: FunctionDefinition {
                name: "submit_task_complete".to_string(),
                description:
                    "Declare this task complete with a structured outcome. Calling this tool \
                    successfully ends the task immediately — the loop will not invoke any \
                    further tools. Use it once you have collected enough evidence for the \
                    task's acceptance criteria; do not re-run shell commands you have \
                    already executed in this task."
                        .to_string(),
                parameters: FunctionParameters::Object {
                    param_type: "object".to_string(),
                    properties: {
                        let mut props = Map::new();
                        props.insert(
                            "result".to_string(),
                            json!({
                                "type": "string",
                                "enum": ["pass", "fail", "no_changes_needed"],
                                "description": "Final outcome of the task. 'pass' when all acceptance criteria are verified; 'fail' when the task is blocked or a criterion is not met; 'no_changes_needed' when current workspace state already satisfies the criteria without further edits."
                            }),
                        );
                        props.insert(
                            "summary".to_string(),
                            json!({
                                "type": "string",
                                "minLength": 1,
                                "description": "One-paragraph summary of what was done (or why nothing was needed) and what evidence supports the result."
                            }),
                        );
                        props.insert(
                            "evidence".to_string(),
                            json!({
                                "type": "array",
                                "description": "Optional list of acceptance-check evidence. Provide one entry per shell command you ran to verify the task. Empty array is acceptable for 'no_changes_needed' or analysis-only tasks.",
                                "items": {
                                    "type": "object",
                                    "required": ["command", "exit_code"],
                                    "properties": {
                                        "command": {
                                            "type": "string",
                                            "description": "The exact shell command that was executed."
                                        },
                                        "exit_code": {
                                            "type": "integer",
                                            "description": "Exit code of the command."
                                        },
                                        "output_excerpt": {
                                            "type": "string",
                                            "description": "Short excerpt of stdout/stderr that supports the result. Truncate to a few hundred characters."
                                        }
                                    }
                                }
                            }),
                        );
                        props
                    },
                    required: vec!["result".to_string(), "summary".to_string()],
                    definitions: None,
                },
            },
        }
    }

    /// Create a ToolSpec for submit_intent_draft.
    pub fn submit_intent_draft() -> Self {
        Self {
            spec_type: "function".to_string(),
            function: FunctionDefinition {
                name: "submit_intent_draft".to_string(),
                description: "Submit a structured IntentDraft for the /plan pipeline. \
                    Use this exactly once after gathering enough context."
                    .to_string(),
                parameters: FunctionParameters::Object {
                    param_type: "object".to_string(),
                    properties: {
                        let mut props = Map::new();
                        props.insert(
                            "draft".to_string(),
                            json!({
                                "type": "object",
                                "required": ["intent", "acceptance", "risk"],
                                "properties": {
                                    "intent": {
                                        "type": "object",
                                        "required": ["summary", "problemStatement", "changeType", "objectives", "inScope", "outOfScope"],
                                        "properties": {
                                            "summary": {"type": "string"},
                                            "problemStatement": {"type": "string"},
                                            "changeType": {
                                                "type": "string",
                                                "description": "High-level code-change category. Do not use 'analysis' here. For read-only plans, use 'unknown' and set objectives[*].kind='analysis'.",
                                                "enum": ["bugfix","feature","refactor","performance","security","docs","chore","unknown"]
                                            },
                                            "objectives": {
                                                "type": "array",
                                                "items": {
                                                    "type": "object",
                                                    "required": ["title", "kind"],
                                                    "properties": {
                                                        "title": {"type": "string"},
                                                        "kind": {
                                                            "type": "string",
                                                            "description": "Use 'analysis' for read-only work and 'implementation' for code-changing work.",
                                                            "enum": ["implementation", "analysis"]
                                                        }
                                                    }
                                                }
                                            },
                                            "inScope": {"type": "array", "items": {"type": "string"}},
                                            "outOfScope": {"type": "array", "items": {"type": "string"}},
                                            "touchHints": {
                                                "type": "object",
                                                "properties": {
                                                    "files": {"type": "array", "items": {"type": "string"}},
                                                    "symbols": {"type": "array", "items": {"type": "string"}},
                                                    "apis": {"type": "array", "items": {"type": "string"}}
                                                }
                                            }
                                        }
                                    },
                                    "acceptance": {
                                        "type": "object",
                                        "required": ["successCriteria"],
                                        "properties": {
                                            "successCriteria": {"type": "array", "items": {"type": "string"}},
                                            "fastChecks": {"type": "array", "items": {"$ref": "#/$defs/check"}},
                                            "integrationChecks": {"type": "array", "items": {"$ref": "#/$defs/check"}},
                                            "securityChecks": {"type": "array", "items": {"$ref": "#/$defs/check"}},
                                            "releaseChecks": {"type": "array", "items": {"$ref": "#/$defs/check"}}
                                        }
                                    },
                                    "risk": {
                                        "type": "object",
                                        "required": ["rationale"],
                                        "properties": {
                                            "rationale": {"type": "string"},
                                            "factors": {"type": "array", "items": {"type": "string"}},
                                            "level": {"type": "string", "enum": ["low", "medium", "high"]}
                                        }
                                    }
                                }
                            }),
                        );
                        props
                    },
                    required: vec!["draft".to_string()],
                    definitions: Some({
                        let mut defs = Map::new();
                        defs.insert(
                            "check".to_string(),
                            json!({
                                "type": "object",
                                "properties": {
                                    "id": {
                                        "type": "string",
                                        "description": "Optional stable check id. If omitted, Libra derives one from command or kind."
                                    },
                                    "kind": {
                                        "type": "string",
                                        "description": "Check type. Optional when command is present; omitted command checks default to command.",
                                        "enum": ["command", "testSuite", "policy"]
                                    },
                                    "command": {"type": "string"},
                                    "timeoutSeconds": {"type": "integer"},
                                    "expectedExitCode": {"type": "integer"},
                                    "required": {"type": "boolean"},
                                    "artifactsProduced": {
                                        "type": "array",
                                        "description": "Names of produced evidence artifacts. Must be one of the supported artifact names (not file paths).",
                                        "items": {
                                            "type": "string",
                                            "enum": [
                                                "patchset",
                                                "test-log",
                                                "build-log",
                                                "sast-report",
                                                "sca-report",
                                                "sbom",
                                                "provenance-attestation",
                                                "transparency-proof",
                                                "release-notes"
                                            ]
                                        }
                                    }
                                }
                            }),
                        );
                        defs
                    }),
                },
            },
        }
    }

    /// Create a ToolSpec for request_user_input.
    pub fn request_user_input() -> Self {
        Self {
            spec_type: "function".to_string(),
            function: FunctionDefinition {
                name: "request_user_input".to_string(),
                description: "Request user input for one to three short questions and wait \
                    for the response. Each question can have 2-3 predefined options (the \
                    client auto-adds 'None of the above'). Do NOT include an 'Other' option \
                    yourself. The first option should be marked '(Recommended)'. Prefer \
                    sending only 1 question at a time."
                    .to_string(),
                parameters: FunctionParameters::Object {
                    param_type: "object".to_string(),
                    properties: {
                        let mut props = Map::new();
                        props.insert(
                            "questions".to_string(),
                            json!({
                                "type": "array",
                                "description": "Questions to present to the user (1-3, prefer 1)",
                                "minItems": 1,
                                "maxItems": 3,
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "id": {
                                            "type": "string",
                                            "description": "Stable snake_case identifier for the question"
                                        },
                                        "header": {
                                            "type": "string",
                                            "description": "Short header displayed above the question (max 12 chars)"
                                        },
                                        "question": {
                                            "type": "string",
                                            "description": "Single-sentence question prompt"
                                        },
                                        "is_other": {
                                            "type": "boolean",
                                            "description": "Whether to auto-add a 'None of the above' option (default: true)"
                                        },
                                        "is_secret": {
                                            "type": "boolean",
                                            "description": "Whether to mask typed input with '*' (default: false)"
                                        },
                                        "options": {
                                            "type": "array",
                                            "description": "2-3 mutually exclusive options. Omit for freeform text input.",
                                            "minItems": 2,
                                            "maxItems": 3,
                                            "items": {
                                                "type": "object",
                                                "properties": {
                                                    "label": {
                                                        "type": "string",
                                                        "description": "User-facing label (1-5 words)"
                                                    },
                                                    "description": {
                                                        "type": "string",
                                                        "description": "Impact/tradeoff explanation"
                                                    }
                                                },
                                                "required": ["label", "description"]
                                            }
                                        }
                                    },
                                    "required": ["id", "header", "question"]
                                }
                            }),
                        );
                        props
                    },
                    required: vec!["questions".to_string()],
                    definitions: None,
                },
            },
        }
    }

    /// Create a ToolSpec for apply_patch.
    pub fn apply_patch() -> Self {
        Self::new(
            "apply_patch",
            r#"Use the `apply_patch` tool to edit files.
IMPORTANT:
- File references can only be relative, NEVER ABSOLUTE.
- When deleting or changing a line, do NOT also include that same line as an unchanged context line.
- When writing context/removed/added lines from `read_file` output, strip the `L{n}: ` line-number prefix (preserve indentation after the prefix).
- Blank lines MUST be included as context lines. Represent a blank context line as a single space ` ` on its own line.

Your patch language is a stripped-down, file-oriented diff format designed to be easy to parse and safe to apply. You can think of it as a high-level envelope:

*** Begin Patch
[ one or more file sections ]
*** End Patch

Within that envelope, you get a sequence of file operations.
You MUST include a header to specify the action you are taking.
Each operation starts with one of three headers:

*** Add File: <path> - create a new file. Every following line is a + line (the initial contents).
*** Delete File: <path> - remove an existing file. Nothing follows.
*** Update File: <path> - patch an existing file in place (optionally with a rename).

May be immediately followed by *** Move to: <new path> if you want to rename the file.
Then one or more "hunks", each introduced by @@ (optionally followed by a hunk header).
Within a hunk each line starts with:
' ' (space) - unchanged context line (line already exists in the file, kept as-is)
'-'         - removed line (line exists in the file and will be deleted)
'+'         - added line (new line to be inserted)

For instructions on [context_before] and [context_after]:
- By default, show 3 lines of code immediately above and 3 lines immediately below each change. If a change is within 3 lines of a previous change, do NOT duplicate the first change's [context_after] lines in the second change's [context_before] lines.
- If 3 lines of context is insufficient to uniquely identify the snippet of code within the file, use the @@ operator to indicate the class or function to which the snippet belongs. For instance, we might have:
@@ class BaseClass
[3 lines of pre-context]
- [old_code]
+ [new_code]
[3 lines of post-context]

- If a code block is repeated so many times in a class or function such that even a single `@@` statement and 3 lines of context cannot uniquely identify the snippet of code, you can use multiple `@@` statements to jump to the right context. For instance:

@@ class BaseClass
@@ 	 def method():
[3 lines of pre-context]
- [old_code]
+ [new_code]
[3 lines of post-context]

The full grammar definition is below:
Patch := Begin { FileOp } End
Begin := "*** Begin Patch" NEWLINE
End := "*** End Patch" NEWLINE
FileOp := AddFile | DeleteFile | UpdateFile
AddFile := "*** Add File: " path NEWLINE { "+" line NEWLINE }
DeleteFile := "*** Delete File: " path NEWLINE
UpdateFile := "*** Update File: " path NEWLINE [ MoveTo ] { Hunk }
MoveTo := "*** Move to: " newPath NEWLINE
Hunk := "@@" [ header ] NEWLINE { HunkLine } [ "*** End of File" NEWLINE ]
HunkLine := (" " | "-" | "+") text NEWLINE

A full patch can combine several operations:

*** Begin Patch
*** Add File: hello.txt
+Hello world
*** Update File: src/app.py
*** Move to: src/main.py
@@ def greet():
-print("Hi")
+print("Hello, world!")
*** Delete File: obsolete.txt
*** End Patch

It is important to remember:

- You must include a header with your intended action (Add/Delete/Update)
- You must prefix new lines with `+` even when creating a new file
- File references can only be relative, NEVER ABSOLUTE.
- IMPORTANT: When writing context or removed lines from `read_file` output, strip the `L{n}: ` line-number prefix. For example, `L3:     my_func():` becomes `    my_func():` (preserve any indentation that follows the prefix).
- IMPORTANT: Blank lines MUST be included as context lines. In `read_file` output a blank line appears as `L{n}: ` (nothing after the space). Represent it in the patch as a single space ` ` on its own line. Do NOT skip blank lines -- omitting them will cause the patch to fail to locate the target region.
"#,
        )
        .with_parameters(FunctionParameters::object(
            [("input", "string", "The entire contents of the apply_patch command")],
            [("input", true)],
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
        /// JSON Schema definitions for $ref resolution.
        #[serde(rename = "$defs", skip_serializing_if = "Option::is_none")]
        definitions: Option<Map<String, Value>>,
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
            definitions: None,
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
                        definitions: None,
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
                definitions: _,
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

    #[test]
    fn test_submit_intent_draft_definitions_at_root() {
        let spec = ToolSpec::submit_intent_draft();
        let json_str = serde_json::to_string(&spec).unwrap();
        let parsed: Value = serde_json::from_str(&json_str).unwrap();

        // $defs should be at root level of parameters, not nested inside acceptance
        let params = &parsed["function"]["parameters"];
        assert!(
            params.get("$defs").is_some(),
            "$defs should be at root level"
        );
        assert!(
            params["$defs"]["check"].is_object(),
            "check definition should exist"
        );

        // acceptance should NOT have nested $defs
        let acceptance = &params["properties"]["draft"]["properties"]["acceptance"];
        assert!(
            acceptance.get("$defs").is_none(),
            "$defs should not be nested in acceptance"
        );
    }

    #[test]
    fn test_submit_intent_draft_check_definition_allows_inferred_check_fields() {
        let spec = ToolSpec::submit_intent_draft();
        let json_str = serde_json::to_string(&spec).unwrap();
        let parsed: Value = serde_json::from_str(&json_str).unwrap();
        let check = &parsed["function"]["parameters"]["$defs"]["check"];

        assert!(
            check.get("required").is_none(),
            "check id/kind can be inferred from command in IntentDraft submissions"
        );
        assert_eq!(
            check["properties"]["kind"]["description"],
            "Check type. Optional when command is present; omitted command checks default to command."
        );
    }
}
