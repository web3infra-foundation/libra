//! Context types for tool invocation and execution.

use std::{collections::HashMap, fmt, path::PathBuf};

use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

/// The kind of tool payload.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ToolKind {
    /// Standard function calling with JSON arguments.
    Function,
    /// Model Context Protocol tool call.
    Mcp,
    /// Custom/freeform tool call.
    Custom,
}

/// A tool invocation containing all context needed for execution.
#[derive(Clone)]
pub struct ToolInvocation {
    /// Unique identifier for this specific tool call.
    pub call_id: String,
    /// Name of the tool being invoked.
    pub tool_name: String,
    /// The payload containing arguments or input data.
    pub payload: ToolPayload,
    /// Working directory for file operations.
    pub working_dir: PathBuf,
}

impl ToolInvocation {
    /// Create a new tool invocation.
    pub fn new(
        call_id: impl Into<String>,
        tool_name: impl Into<String>,
        payload: ToolPayload,
        working_dir: PathBuf,
    ) -> Self {
        Self {
            call_id: call_id.into(),
            tool_name: tool_name.into(),
            payload,
            working_dir,
        }
    }

    /// Get the payload as a string for logging.
    pub fn log_payload(&self) -> String {
        match &self.payload {
            ToolPayload::Function { arguments } => arguments.clone(),
            ToolPayload::Custom { input } => input.clone(),
            ToolPayload::Mcp { raw_arguments, .. } => raw_arguments.clone(),
        }
    }
}

/// The payload of a tool invocation, containing the input data.
#[derive(Clone, Debug)]
pub enum ToolPayload {
    /// Function-style tool call with JSON arguments.
    Function {
        /// JSON string containing the tool arguments.
        arguments: String,
    },
    /// Custom/freeform tool call with plain text input.
    Custom {
        /// Plain text input for the tool.
        input: String,
    },
    /// Model Context Protocol tool call.
    Mcp {
        /// MCP server name.
        server: String,
        /// Tool name within the MCP server.
        tool: String,
        /// Raw arguments string.
        raw_arguments: String,
    },
}

impl ToolPayload {
    /// Check if this is a Function payload.
    pub fn is_function(&self) -> bool {
        matches!(self, ToolPayload::Function { .. })
    }

    /// Check if this is a Custom payload.
    pub fn is_custom(&self) -> bool {
        matches!(self, ToolPayload::Custom { .. })
    }

    /// Check if this is an Mcp payload.
    pub fn is_mcp(&self) -> bool {
        matches!(self, ToolPayload::Mcp { .. })
    }
}

/// Output from a tool execution.
#[derive(Clone, Debug)]
pub enum ToolOutput {
    /// Function-style output with structured data.
    Function {
        /// The output content as text.
        content: String,
        /// Whether the tool execution succeeded.
        success: Option<bool>,
    },
    /// MCP tool output.
    Mcp {
        /// The MCP result as JSON.
        result: serde_json::Value,
    },
}

impl ToolOutput {
    /// Create a successful function output.
    pub fn success(content: impl Into<String>) -> Self {
        Self::Function {
            content: content.into(),
            success: Some(true),
        }
    }

    /// Create a failed function output.
    pub fn failure(content: impl Into<String>) -> Self {
        Self::Function {
            content: content.into(),
            success: Some(false),
        }
    }

    /// Create a text-only function output (success assumed).
    pub fn text(content: impl Into<String>) -> Self {
        Self::Function {
            content: content.into(),
            success: None,
        }
    }

    /// Get the text content of this output.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            ToolOutput::Function { content, .. } => Some(content),
            ToolOutput::Mcp { .. } => None,
        }
    }

    /// Check if the output indicates success.
    pub fn is_success(&self) -> bool {
        match self {
            ToolOutput::Function { success, .. } => success.unwrap_or(true),
            ToolOutput::Mcp { .. } => true,
        }
    }

    /// Convert this output to a JSON value for sending to the model.
    pub fn into_response(self) -> serde_json::Value {
        match self {
            ToolOutput::Function { content, success } => {
                serde_json::json!({
                    "content": content,
                    "success": success
                })
            }
            ToolOutput::Mcp { result } => result,
        }
    }

    /// Get a preview of the output for logging (truncated if necessary).
    pub fn log_preview(&self) -> String {
        const MAX_PREVIEW_LENGTH: usize = 500;
        match self {
            ToolOutput::Function { content, .. } => {
                if content.len() <= MAX_PREVIEW_LENGTH {
                    content.clone()
                } else {
                    format!("{}... (truncated)", &content[..MAX_PREVIEW_LENGTH])
                }
            }
            ToolOutput::Mcp { result } => format!("{:?}", result),
        }
    }
}

/// Arguments for the read_file tool.
#[derive(Clone, Deserialize, Debug)]
pub struct ReadFileArgs {
    /// Absolute path to the file to read.
    pub file_path: String,
    /// 1-indexed line number to start reading from (default: 1).
    #[serde(default = "default_offset")]
    pub offset: usize,
    /// Maximum number of lines to return (default: 2000).
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_offset() -> usize {
    1
}

fn default_limit() -> usize {
    2000
}

/// Arguments for the list_dir tool.
#[derive(Clone, Deserialize, Debug)]
pub struct ListDirArgs {
    /// Absolute path to the directory to list.
    pub dir_path: String,
    /// 1-indexed entry number to start listing from (default: 1).
    #[serde(default = "default_dir_offset")]
    pub offset: usize,
    /// Maximum number of entries to return (default: 25).
    #[serde(default = "default_dir_limit")]
    pub limit: usize,
    /// Maximum directory depth to traverse (default: 2, must be >= 1).
    #[serde(default = "default_depth")]
    pub depth: usize,
}

fn default_dir_offset() -> usize {
    1
}

fn default_dir_limit() -> usize {
    25
}

fn default_depth() -> usize {
    2
}

/// Arguments for the shell tool.
#[derive(Clone, Deserialize, Debug)]
pub struct ShellArgs {
    /// Shell command or script to execute (runs in the user's default shell).
    pub command: String,
    /// Working directory for the command. Must be an absolute path within the
    /// sandbox working directory. Defaults to the registry's working directory.
    #[serde(default)]
    pub workdir: Option<String>,
    /// Timeout in milliseconds. Defaults to 10,000 ms (10 seconds).
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

/// Arguments for the grep_files tool.
#[derive(Clone, Deserialize, Debug)]
pub struct GrepFilesArgs {
    /// Regular expression pattern to search for.
    pub pattern: String,
    /// Optional glob limiting which files are searched (e.g. "*.rs" or "*.{ts,tsx}").
    #[serde(default)]
    pub include: Option<String>,
    /// Directory or file path to search. Defaults to the working directory.
    #[serde(default)]
    pub path: Option<String>,
    /// Maximum number of file paths to return (default: 100, max: 2000).
    #[serde(default = "default_grep_limit")]
    pub limit: usize,
}

fn default_grep_limit() -> usize {
    100
}

// ── update_plan types ──────────────────────────────────────────────────

/// Status of a single plan step.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    InProgress,
    Completed,
}

/// A single step in a plan.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlanStep {
    /// Human-readable description of the step.
    pub step: String,
    /// Current status of the step.
    pub status: StepStatus,
}

/// Arguments for the `update_plan` tool.
#[derive(Clone, Debug, Deserialize)]
pub struct UpdatePlanArgs {
    /// Optional explanation of what changed since the last plan update.
    #[serde(default)]
    pub explanation: Option<String>,
    /// The full plan, expressed as an ordered list of steps.
    pub plan: Vec<PlanStep>,
}

// ── request_user_input types ───────────────────────────────────────────

/// A selectable option presented to the user.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserInputOption {
    /// Short label for the option (1-5 words).
    pub label: String,
    /// Longer description of the impact/tradeoff.
    pub description: String,
}

/// A single question to present to the user.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserInputQuestion {
    /// Machine-readable identifier for the question (unique within a request).
    pub id: String,
    /// Short header displayed above the question (≤12 chars).
    pub header: String,
    /// The full question text (single sentence).
    pub question: String,
    /// Whether to auto-add a "None of the above" option.
    #[serde(default)]
    pub is_other: bool,
    /// Whether to mask user-typed text with '*' (for secrets).
    #[serde(default)]
    pub is_secret: bool,
    /// Predefined options the user can choose from.
    /// When `None` or empty, the question is freeform (text input only).
    #[serde(default)]
    pub options: Option<Vec<UserInputOption>>,
}

/// Arguments for the `request_user_input` tool.
#[derive(Clone, Debug, Deserialize)]
pub struct RequestUserInputArgs {
    /// Questions to present to the user (1-3, prefer 1).
    pub questions: Vec<UserInputQuestion>,
}

/// A single answer, potentially containing the selected option label
/// and/or user notes.
///
/// - If an option was selected: `answers[0]` = selected label
/// - If notes were added: last entry = `"user_note: {text}"`
/// - For freeform questions: `answers[0]` = user-typed text
/// - Empty `answers` means the user skipped the question
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserInputAnswer {
    pub answers: Vec<String>,
}

/// User's responses to a set of questions, keyed by question id.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserInputResponse {
    pub answers: HashMap<String, UserInputAnswer>,
}

/// A request sent from the tool handler to the TUI, carrying the questions
/// and a one-shot channel for the user's response.
pub struct UserInputRequest {
    /// The call_id of the originating tool invocation (for correlation).
    pub call_id: String,
    /// Questions to display.
    pub questions: Vec<UserInputQuestion>,
    /// Channel the TUI uses to send back the response.
    pub response_tx: oneshot::Sender<UserInputResponse>,
}

impl fmt::Debug for UserInputRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UserInputRequest")
            .field("call_id", &self.call_id)
            .field("questions", &self.questions)
            .field("response_tx", &"<oneshot::Sender>")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_invocation_creation() {
        let invocation = ToolInvocation::new(
            "call-123",
            "read_file",
            ToolPayload::Function {
                arguments: r#"{"file_path":"/tmp/test.txt"}"#.to_string(),
            },
            PathBuf::from("/work"),
        );
        assert_eq!(invocation.call_id, "call-123");
        assert_eq!(invocation.tool_name, "read_file");
        assert_eq!(invocation.working_dir, PathBuf::from("/work"));
    }

    #[test]
    fn test_tool_output_success() {
        let output = ToolOutput::success("File content here");
        assert!(output.is_success());
        assert_eq!(output.as_text(), Some("File content here"));
    }

    #[test]
    fn test_tool_output_failure() {
        let output = ToolOutput::failure("Error reading file");
        assert!(!output.is_success());
        assert_eq!(output.as_text(), Some("Error reading file"));
    }

    #[test]
    fn test_tool_payload_kinds() {
        let func = ToolPayload::Function {
            arguments: "{}".to_string(),
        };
        assert!(func.is_function());
        assert!(!func.is_custom());

        let custom = ToolPayload::Custom {
            input: "test".to_string(),
        };
        assert!(custom.is_custom());
        assert!(!custom.is_function());
    }

    #[test]
    fn test_log_preview_truncation() {
        let long_content = "x".repeat(1000);
        let output = ToolOutput::success(long_content.clone());
        let preview = output.log_preview();
        assert!(preview.len() < long_content.len());
        assert!(preview.contains("truncated"));
    }

    #[test]
    fn test_read_file_args_defaults() {
        let args: Result<ReadFileArgs, _> =
            serde_json::from_str(r#"{"file_path":"/tmp/test.txt"}"#);
        assert!(args.is_ok());
        let args = args.unwrap();
        assert_eq!(args.offset, 1);
        assert_eq!(args.limit, 2000);
    }
}
