//! Hook configuration loading and per-tool matching.
//!
//! This module is the on-disk surface of the hook system. It deserialises the
//! `hooks.json` files described in [`super`] and exposes the merged set of hook
//! definitions that the runtime executes when lifecycle events fire.
//!
//! Two tiers are merged (not overridden) so a project may layer additional hooks on
//! top of a user-global default set. Both tiers are optional; missing files are
//! silently ignored.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::event::HookEvent;

/// A single hook definition as read from `hooks.json`.
///
/// Each definition binds a [`HookEvent`] (lifecycle trigger) to a shell `command` and,
/// for tool-scoped events, a `matcher` that filters which tool invocations fire it.
/// Default values for `timeout_ms` and `enabled` keep older configs forward-compatible.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookDefinition {
    /// Which lifecycle event triggers this hook.
    pub event: HookEvent,
    /// Tool name pattern to match (plain string or pipe-separated alternatives).
    /// Empty or `"*"` matches all tools. Ignored for session events.
    #[serde(default)]
    pub matcher: String,
    /// Shell command to execute. Receives JSON on stdin.
    pub command: String,
    /// Human-readable description of what this hook does.
    #[serde(default)]
    pub description: String,
    /// Timeout in milliseconds. Defaults to 10_000 (10s).
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    /// Whether this hook is enabled. Defaults to true.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

/// Default hook timeout in milliseconds. Chosen to be long enough for typical lint /
/// formatter invocations but short enough to keep a runaway script from stalling the
/// agent loop.
fn default_timeout() -> u64 {
    10_000
}

/// Hooks default to enabled when the `enabled` field is omitted from JSON.
fn default_enabled() -> bool {
    true
}

impl HookDefinition {
    /// Decide whether this hook should fire for a tool with the given name.
    ///
    /// Functional scope:
    /// - An empty matcher or `"*"` is treated as a wildcard and matches every tool.
    /// - Otherwise the matcher is split on `|` and trimmed, supporting compact
    ///   alternation like `"Edit|Write|apply_patch"`.
    ///
    /// Boundary conditions:
    /// - The match is exact; substring matches are intentionally rejected to avoid
    ///   accidentally enabling a hook for unrelated tools.
    /// - Whitespace inside each alternative is trimmed, but punctuation is not.
    pub fn matches_tool(&self, tool_name: &str) -> bool {
        if self.matcher.is_empty() || self.matcher == "*" {
            return true;
        }
        // Support pipe-separated alternatives: "Edit|Write|apply_patch"
        self.matcher
            .split('|')
            .any(|pattern| pattern.trim() == tool_name)
    }
}

/// Root document persisted in `hooks.json`.
///
/// The file is intentionally a single object so that future fields (e.g. metadata,
/// schema version) can be added without breaking older parsers.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HookConfig {
    /// List of hook definitions.
    #[serde(default)]
    pub hooks: Vec<HookDefinition>,
}

/// Load hook configuration from the project + user tiers and merge them.
///
/// Functional scope:
/// - Reads `<working_dir>/.libra/hooks.json` first, then the user-global file at
///   `<config_dir>/libra/hooks.json` (typically `~/.config/libra/hooks.json` on
///   Linux/macOS).
/// - Hooks from both files are concatenated; later tiers do not override earlier
///   ones — every matching hook fires.
///
/// Boundary conditions:
/// - Missing files are silently skipped — running without hooks is a valid state.
/// - Malformed JSON is logged at `warn` level and ignored, so a broken config never
///   blocks the rest of the agent.
/// - When `dirs::config_dir()` returns `None` (unusual sandboxed environments) only
///   the project-local tier is loaded.
pub fn load_hook_config(working_dir: &Path) -> HookConfig {
    let mut all_hooks = Vec::new();

    // 1. Project-local
    let project_config = working_dir.join(".libra").join("hooks.json");
    if let Some(config) = load_config_file(&project_config) {
        all_hooks.extend(config.hooks);
    }

    // 2. User-global
    if let Some(config_dir) = dirs::config_dir() {
        let user_config = config_dir.join("libra").join("hooks.json");
        if let Some(config) = load_config_file(&user_config) {
            all_hooks.extend(config.hooks);
        }
    }

    HookConfig { hooks: all_hooks }
}

/// Try to read and parse a single `hooks.json` from the given path.
///
/// Returns `None` when the file does not exist, cannot be read, or fails to parse.
/// Parse errors are surfaced via `tracing::warn` so operators can debug a broken file
/// without losing the rest of the agent session.
fn load_config_file(path: &Path) -> Option<HookConfig> {
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content)
        .map_err(|e| {
            tracing::warn!("Failed to parse hook config {}: {}", path.display(), e);
            e
        })
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Scenario: a hook listing alternatives matches each named tool but not others.
    #[test]
    fn test_hook_definition_matches_tool() {
        let hook = HookDefinition {
            event: HookEvent::PreToolUse,
            matcher: "read_file|list_dir".to_string(),
            command: "echo test".to_string(),
            description: String::new(),
            timeout_ms: 10_000,
            enabled: true,
        };

        assert!(hook.matches_tool("read_file"));
        assert!(hook.matches_tool("list_dir"));
        assert!(!hook.matches_tool("apply_patch"));
    }

    // Scenario: `"*"` is the explicit wildcard that fires on every tool name.
    #[test]
    fn test_hook_definition_wildcard() {
        let hook = HookDefinition {
            event: HookEvent::PreToolUse,
            matcher: "*".to_string(),
            command: "echo test".to_string(),
            description: String::new(),
            timeout_ms: 10_000,
            enabled: true,
        };

        assert!(hook.matches_tool("read_file"));
        assert!(hook.matches_tool("anything"));
    }

    // Scenario: an omitted `matcher` field is treated identically to `"*"`.
    #[test]
    fn test_hook_definition_empty_matcher() {
        let hook = HookDefinition {
            event: HookEvent::PreToolUse,
            matcher: String::new(),
            command: "echo test".to_string(),
            description: String::new(),
            timeout_ms: 10_000,
            enabled: true,
        };

        assert!(hook.matches_tool("anything"));
    }

    // Scenario: a fresh working directory with no hooks.json yields an empty config.
    #[test]
    fn test_load_hook_config_missing_files() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = load_hook_config(tmp.path());
        assert!(config.hooks.is_empty());
    }

    // Scenario: project-local hooks.json is loaded when present in `.libra/`.
    #[test]
    fn test_load_hook_config_from_project() {
        let tmp = tempfile::TempDir::new().unwrap();
        let hook_dir = tmp.path().join(".libra");
        std::fs::create_dir_all(&hook_dir).unwrap();
        std::fs::write(
            hook_dir.join("hooks.json"),
            r#"{"hooks": [{"event": "pre_tool_use", "matcher": "shell", "command": "echo blocked"}]}"#,
        )
        .unwrap();

        let config = load_hook_config(tmp.path());
        assert_eq!(config.hooks.len(), 1);
        assert_eq!(config.hooks[0].matcher, "shell");
    }

    // Scenario: full JSON round-trip with both explicit and default-filled fields.
    #[test]
    fn test_deserialize_hook_config() {
        let json = r#"{
            "hooks": [
                {
                    "event": "pre_tool_use",
                    "matcher": "shell",
                    "command": "node check.js",
                    "description": "Block dangerous shell commands",
                    "timeout_ms": 5000,
                    "enabled": true
                },
                {
                    "event": "post_tool_use",
                    "matcher": "apply_patch",
                    "command": "cargo fmt",
                    "description": "Format after edit"
                }
            ]
        }"#;

        let config: HookConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.hooks.len(), 2);
        assert_eq!(config.hooks[0].event, HookEvent::PreToolUse);
        assert_eq!(config.hooks[0].timeout_ms, 5000);
        assert_eq!(config.hooks[1].event, HookEvent::PostToolUse);
        assert_eq!(config.hooks[1].timeout_ms, 10_000); // default
        assert!(config.hooks[1].enabled); // default
    }
}
