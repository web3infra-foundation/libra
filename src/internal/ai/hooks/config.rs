//! Hook configuration: loading and matching.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::event::HookEvent;

/// A single hook definition from configuration.
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

fn default_timeout() -> u64 {
    10_000
}

fn default_enabled() -> bool {
    true
}

impl HookDefinition {
    /// Check if this hook matches a given tool name.
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

/// Top-level hook configuration file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HookConfig {
    /// List of hook definitions.
    #[serde(default)]
    pub hooks: Vec<HookDefinition>,
}

/// Load hook configuration from the three-tier hierarchy.
///
/// 1. `{working_dir}/.libra/hooks.json` (project-local)
/// 2. `~/.config/libra/hooks.json` (user-global)
///
/// Configs are merged (not overridden): all hooks from all tiers are collected.
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

    #[test]
    fn test_load_hook_config_missing_files() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = load_hook_config(tmp.path());
        assert!(config.hooks.is_empty());
    }

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
