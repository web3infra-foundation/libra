use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::super::super::{
    provider::ProviderInstallOptions,
    setup::{load_json_settings, resolve_project_root, write_json_settings},
};

const DEFAULT_HOOK_TIMEOUT_SECS: u64 = 10;
const CLAUDE_SETTINGS_DIR: &str = ".claude";
const CLAUDE_SETTINGS_FILE: &str = "settings.json";
const CLAUDE_HOOK_FORWARD_MAP: &[(&str, &str)] = &[
    ("SessionStart", "session-start"),
    ("UserPromptSubmit", "prompt"),
    ("PostToolUse", "tool-use"),
    ("Stop", "stop"),
    ("SessionEnd", "session-end"),
];

#[derive(Debug, Serialize, Deserialize, Default)]
struct ClaudeSettings {
    #[serde(default)]
    hooks: BTreeMap<String, Vec<ClaudeHookMatcher>>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
struct ClaudeHookMatcher {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    matcher: Option<String>,
    hooks: Vec<ClaudeHookEntry>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
struct ClaudeHookEntry {
    #[serde(rename = "type")]
    entry_type: String,
    command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    timeout: Option<u64>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

pub(super) fn install_claude_hooks(options: &ProviderInstallOptions) -> Result<()> {
    let command_prefix = options.command_prefix.trim();
    if command_prefix.is_empty() {
        bail!("invalid --command-prefix: value cannot be empty");
    }

    let timeout = options.timeout_secs.unwrap_or(DEFAULT_HOOK_TIMEOUT_SECS);
    if timeout == 0 {
        bail!("invalid --timeout: value must be greater than 0");
    }

    let settings_path = claude_settings_path()?;
    let mut settings = load_claude_settings(&settings_path)?;
    let changed = upsert_claude_hooks(&mut settings, command_prefix, timeout);

    if changed {
        write_json_settings(&settings_path, &settings, "Claude")?;
        println!(
            "Installed Claude hook forwarding at {}",
            settings_path.display()
        );
    } else {
        println!(
            "Claude hook forwarding is already up to date at {}",
            settings_path.display()
        );
    }

    Ok(())
}

pub(super) fn uninstall_claude_hooks() -> Result<()> {
    let settings_path = claude_settings_path()?;
    if !settings_path.exists() {
        println!(
            "Claude hook settings not found at {}",
            settings_path.display()
        );
        return Ok(());
    }

    let mut settings = load_claude_settings(&settings_path)?;
    let changed = remove_libra_claude_hooks(&mut settings);
    if changed {
        write_json_settings(&settings_path, &settings, "Claude")?;
        println!(
            "Removed Claude hook forwarding at {}",
            settings_path.display()
        );
    } else {
        println!(
            "No Libra-managed Claude hooks found at {}",
            settings_path.display()
        );
    }
    Ok(())
}

pub(super) fn claude_hooks_are_installed() -> Result<bool> {
    let settings_path = claude_settings_path()?;
    if !settings_path.exists() {
        return Ok(false);
    }
    let settings = load_claude_settings(&settings_path)?;
    Ok(all_claude_specs_installed(&settings))
}

fn claude_settings_path() -> Result<PathBuf> {
    Ok(resolve_project_root()?
        .join(CLAUDE_SETTINGS_DIR)
        .join(CLAUDE_SETTINGS_FILE))
}

fn load_claude_settings(path: &Path) -> Result<ClaudeSettings> {
    load_json_settings(path, "Claude")
}

fn upsert_claude_hooks(settings: &mut ClaudeSettings, command_prefix: &str, timeout: u64) -> bool {
    let mut changed = false;

    for (event_name, subcommand) in CLAUDE_HOOK_FORWARD_MAP {
        let desired_entry = ClaudeHookEntry {
            entry_type: "command".to_string(),
            command: format!("{command_prefix} hooks claude {subcommand}"),
            timeout: Some(timeout),
            extra: BTreeMap::new(),
        };

        let original_matchers = settings.hooks.remove(*event_name).unwrap_or_default();
        let mut rebuilt_matchers = Vec::with_capacity(original_matchers.len() + 1);
        let mut has_desired_entry = false;

        for mut matcher in original_matchers {
            if matcher.matcher.is_none() && matcher.hooks == vec![desired_entry.clone()] {
                has_desired_entry = true;
                rebuilt_matchers.push(matcher);
                continue;
            }

            let matcher_name = matcher.matcher.as_deref();
            let original_hook_count = matcher.hooks.len();
            matcher.hooks.retain(|hook| {
                !is_replaced_managed_claude_hook(
                    matcher_name,
                    hook,
                    &desired_entry.command,
                    subcommand,
                )
            });
            if matcher.hooks.len() != original_hook_count {
                changed = true;
            }
            if matcher.hooks.is_empty() {
                continue;
            }
            rebuilt_matchers.push(matcher);
        }

        if !has_desired_entry {
            rebuilt_matchers.push(ClaudeHookMatcher {
                matcher: None,
                hooks: vec![desired_entry],
                extra: BTreeMap::new(),
            });
            changed = true;
        }

        settings
            .hooks
            .insert((*event_name).to_string(), rebuilt_matchers);
    }

    changed
}

fn remove_libra_claude_hooks(settings: &mut ClaudeSettings) -> bool {
    let keys: Vec<String> = settings.hooks.keys().cloned().collect();
    let mut changed = false;

    for key in keys {
        let Some(mut matchers) = settings.hooks.remove(&key) else {
            continue;
        };
        let original = matchers.clone();

        for matcher in &mut matchers {
            let matcher_name = matcher.matcher.as_deref();
            matcher.hooks.retain(|hook| {
                !(is_managed_claude_command(&hook.command)
                    || (matcher_name == Some("libra") && hook.command.contains(" hooks claude ")))
            });
        }
        matchers.retain(|matcher| !matcher.hooks.is_empty());

        if matchers != original {
            changed = true;
        }
        if !matchers.is_empty() {
            settings.hooks.insert(key, matchers);
        }
    }

    changed
}

fn all_claude_specs_installed(settings: &ClaudeSettings) -> bool {
    CLAUDE_HOOK_FORWARD_MAP
        .iter()
        .all(|(event_name, subcommand)| {
            settings.hooks.get(*event_name).is_some_and(|matchers| {
                matchers.iter().any(|matcher| {
                    matcher.matcher.is_none()
                        && matcher.hooks.iter().any(|hook| {
                            hook.command
                                .ends_with(&format!(" hooks claude {subcommand}"))
                        })
                })
            })
        })
}

fn is_managed_claude_command(command: &str) -> bool {
    CLAUDE_HOOK_FORWARD_MAP
        .iter()
        .any(|(_, subcommand)| command.ends_with(&format!(" hooks claude {subcommand}")))
}

fn is_replaced_managed_claude_hook(
    matcher: Option<&str>,
    hook: &ClaudeHookEntry,
    desired_command: &str,
    subcommand: &str,
) -> bool {
    hook.command == desired_command
        || (matcher == Some("libra")
            && hook
                .command
                .ends_with(&format!(" hooks claude {subcommand}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_claude_hooks_is_idempotent() {
        let mut settings = ClaudeSettings::default();
        assert!(upsert_claude_hooks(&mut settings, "libra", 10));
        assert!(!upsert_claude_hooks(&mut settings, "libra", 10));
        assert!(all_claude_specs_installed(&settings));
    }

    #[test]
    fn remove_claude_hooks_preserves_non_libra_entries() {
        let mut settings = ClaudeSettings::default();
        settings.hooks.insert(
            "SessionStart".to_string(),
            vec![
                ClaudeHookMatcher {
                    matcher: None,
                    hooks: vec![ClaudeHookEntry {
                        entry_type: "command".to_string(),
                        command: "libra hooks claude session-start".to_string(),
                        timeout: Some(10),
                        extra: BTreeMap::new(),
                    }],
                    extra: BTreeMap::new(),
                },
                ClaudeHookMatcher {
                    matcher: Some("startup".to_string()),
                    hooks: vec![ClaudeHookEntry {
                        entry_type: "command".to_string(),
                        command: "echo keep".to_string(),
                        timeout: Some(3),
                        extra: BTreeMap::new(),
                    }],
                    extra: BTreeMap::new(),
                },
            ],
        );

        assert!(remove_libra_claude_hooks(&mut settings));
        let session_start = settings.hooks.get("SessionStart").expect("SessionStart");
        assert_eq!(session_start.len(), 1);
        assert_eq!(session_start[0].hooks[0].command, "echo keep");
    }
}
