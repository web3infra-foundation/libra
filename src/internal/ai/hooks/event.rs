//! Strongly-typed events, actions, and JSON I/O contracts shared by hook clients.
//!
//! These types form the wire format between Libra and external hook scripts. They are
//! intentionally `serde`-friendly so a hook implemented in any language can read the
//! same JSON shape on stdin without having to track Rust internals.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Lifecycle events that can trigger hooks.
///
/// `PreToolUse` is the only variant that allows a hook to veto further execution.
/// All other variants are informational and never alter control flow regardless of
/// the hook's exit status.
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

/// Outcome produced by the hook runner after evaluating one hook.
///
/// `Block` carries a reason string surfaced to the user so the agent can explain why
/// a tool call was rejected. The reason is a human-readable diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookAction {
    /// Allow the operation to proceed.
    Allow,
    /// Block the operation with a reason.
    Block(String),
}

impl HookAction {
    /// Convenience predicate equivalent to `matches!(self, Block(_))`.
    pub fn is_blocked(&self) -> bool {
        matches!(self, Self::Block(_))
    }
}

/// Input payload sent to a hook command on stdin as a single JSON object.
///
/// Tool fields (`tool_name`, `tool_input`, `tool_output`) are populated only for
/// tool-scoped events. `Option<...>` plus `skip_serializing_if` keeps the serialised
/// envelope minimal — session events emit a JSON object with just `event` and
/// `working_dir`.
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
    /// Build the stdin payload for a `PreToolUse` event.
    ///
    /// Functional scope: captures the tool's name, raw input arguments, and the
    /// working directory. `tool_output` is intentionally `None` because the tool has
    /// not yet been executed.
    pub fn pre_tool_use(tool_name: &str, tool_input: Value, working_dir: &str) -> Self {
        Self {
            event: HookEvent::PreToolUse,
            tool_name: Some(tool_name.to_string()),
            tool_input: Some(tool_input),
            tool_output: None,
            working_dir: Some(working_dir.to_string()),
        }
    }

    /// Build the stdin payload for a `PostToolUse` event.
    ///
    /// Functional scope: includes both `tool_input` and `tool_output` so observer
    /// hooks (formatters, log shippers, etc.) can see the full request/response pair.
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

    /// Build the stdin payload for a session-scoped event.
    ///
    /// Functional scope: tool fields are left empty since session events fire once
    /// per session and have no associated tool invocation.
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

/// Optional structured response a hook may print to stdout as JSON.
///
/// Hooks usually communicate their decision via the exit code (0 = allow, 129 =
/// block); the optional `message` field is rendered verbatim when present so a
/// blocking hook can explain itself.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HookOutput {
    /// Optional message from the hook.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}
