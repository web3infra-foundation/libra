//! Hook events, actions, and I/O types.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Lifecycle events that can trigger hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    /// Before a tool is executed. Can block execution.
    PreToolUse,
    /// After a tool has executed. Informational only.
    PostToolUse,
    /// When a session starts.
    SessionStart,
    /// When a session ends.
    SessionEnd,
}

impl std::fmt::Display for HookEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PreToolUse => write!(f, "pre_tool_use"),
            Self::PostToolUse => write!(f, "post_tool_use"),
            Self::SessionStart => write!(f, "session_start"),
            Self::SessionEnd => write!(f, "session_end"),
        }
    }
}

/// Result of evaluating a hook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookAction {
    /// Allow the operation to proceed.
    Allow,
    /// Block the operation with a reason.
    Block(String),
}

impl HookAction {
    /// Returns `true` if this action blocks the operation.
    pub fn is_blocked(&self) -> bool {
        matches!(self, Self::Block(_))
    }
}

/// Input payload sent to a hook command on stdin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookInput {
    /// The event that triggered this hook.
    pub event: HookEvent,
    /// Name of the tool being invoked (for tool events).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// Tool input arguments (for tool events).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_input: Option<Value>,
    /// Tool output (for PostToolUse only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_output: Option<Value>,
    /// Working directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
}

impl HookInput {
    /// Create input for a PreToolUse event.
    pub fn pre_tool_use(tool_name: &str, tool_input: Value, working_dir: &str) -> Self {
        Self {
            event: HookEvent::PreToolUse,
            tool_name: Some(tool_name.to_string()),
            tool_input: Some(tool_input),
            tool_output: None,
            working_dir: Some(working_dir.to_string()),
        }
    }

    /// Create input for a PostToolUse event.
    pub fn post_tool_use(
        tool_name: &str,
        tool_input: Value,
        tool_output: Value,
        working_dir: &str,
    ) -> Self {
        Self {
            event: HookEvent::PostToolUse,
            tool_name: Some(tool_name.to_string()),
            tool_input: Some(tool_input),
            tool_output: Some(tool_output),
            working_dir: Some(working_dir.to_string()),
        }
    }

    /// Create input for a session lifecycle event.
    pub fn session_event(event: HookEvent, working_dir: &str) -> Self {
        Self {
            event,
            tool_name: None,
            tool_input: None,
            tool_output: None,
            working_dir: Some(working_dir.to_string()),
        }
    }
}

/// Output from a hook command (parsed from stdout JSON).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HookOutput {
    /// Optional message from the hook.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}
