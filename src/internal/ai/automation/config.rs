use std::{fs, path::Path};

use serde::{Deserialize, Serialize};

use crate::internal::ai::{automation::events::AutomationError, hooks::HookEvent};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AutomationConfig {
    #[serde(default)]
    pub rules: Vec<AutomationRule>,
}

impl AutomationConfig {
    pub fn from_toml_str(input: &str) -> Result<Self, AutomationError> {
        toml::from_str(input).map_err(|error| AutomationError::ConfigParse(error.to_string()))
    }

    pub fn load_from_working_dir(working_dir: &Path) -> Result<Self, AutomationError> {
        let path = working_dir.join(".libra").join("automations.toml");
        if !path.exists() {
            return Ok(Self::default());
        }
        let contents = fs::read_to_string(&path).map_err(|error| {
            AutomationError::ConfigParse(format!(
                "failed to read automation config {}: {error}",
                path.display()
            ))
        })?;
        Self::from_toml_str(&contents)
    }

    pub fn validate(&self) -> Result<(), AutomationError> {
        let mut seen = std::collections::HashSet::new();
        for rule in &self.rules {
            rule.validate()?;
            if !seen.insert(rule.id.clone()) {
                return Err(AutomationError::ConfigValidation(format!(
                    "duplicate automation rule id `{}`",
                    rule.id
                )));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AutomationRule {
    pub id: String,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    pub trigger: AutomationTrigger,
    pub action: AutomationAction,
}

impl AutomationRule {
    fn validate(&self) -> Result<(), AutomationError> {
        if self.id.trim().is_empty() {
            return Err(AutomationError::ConfigValidation(
                "automation rule id must not be empty".to_string(),
            ));
        }
        self.trigger.validate()?;
        self.action.validate()?;
        Ok(())
    }
}

fn enabled_by_default() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AutomationTrigger {
    Hook { event: HookEvent },
    Cron { schedule: String },
    Vcs { event: String },
}

impl AutomationTrigger {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Hook { .. } => "hook",
            Self::Cron { .. } => "cron",
            Self::Vcs { .. } => "vcs",
        }
    }

    fn validate(&self) -> Result<(), AutomationError> {
        match self {
            Self::Hook { .. } => Ok(()),
            Self::Cron { schedule } if schedule.trim().is_empty() => {
                Err(AutomationError::ConfigValidation(
                    "cron trigger schedule must not be empty".to_string(),
                ))
            }
            Self::Cron { .. } => Ok(()),
            Self::Vcs { event } if event.trim().is_empty() => {
                Err(AutomationError::ConfigValidation(
                    "vcs trigger event must not be empty".to_string(),
                ))
            }
            Self::Vcs { .. } => Ok(()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AutomationAction {
    Prompt {
        prompt: String,
    },
    Webhook {
        url: String,
        #[serde(default = "default_webhook_method")]
        method: String,
    },
    Shell {
        command: String,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
}

impl AutomationAction {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Prompt { .. } => "prompt",
            Self::Webhook { .. } => "webhook",
            Self::Shell { .. } => "shell",
        }
    }

    fn validate(&self) -> Result<(), AutomationError> {
        match self {
            Self::Prompt { prompt } if prompt.trim().is_empty() => {
                Err(AutomationError::ConfigValidation(
                    "prompt action prompt must not be empty".to_string(),
                ))
            }
            Self::Webhook { url, .. } if url.trim().is_empty() => {
                Err(AutomationError::ConfigValidation(
                    "webhook action url must not be empty".to_string(),
                ))
            }
            Self::Shell { command, .. } if command.trim().is_empty() => {
                Err(AutomationError::ConfigValidation(
                    "shell action command must not be empty".to_string(),
                ))
            }
            _ => Ok(()),
        }
    }
}

fn default_webhook_method() -> String {
    "POST".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::hooks::HookEvent;

    fn rule(id: &str) -> AutomationRule {
        AutomationRule {
            id: id.to_string(),
            enabled: true,
            trigger: AutomationTrigger::Hook {
                event: HookEvent::SessionEnd,
            },
            action: AutomationAction::Prompt {
                prompt: "ping".to_string(),
            },
        }
    }

    #[test]
    fn enabled_by_default_returns_true() {
        // INVARIANT: serde-derived defaults must enable rules unless the
        // user explicitly opts out; flipping this default would silently
        // disable every existing automation on upgrade.
        assert!(enabled_by_default());
    }

    #[test]
    fn default_webhook_method_returns_post() {
        // INVARIANT: omitted webhook `method` falls back to POST — the
        // canonical verb most receivers expect. A silent change here
        // would shift every legacy webhook to GET.
        assert_eq!(default_webhook_method(), "POST");
    }

    #[test]
    fn automation_rule_serde_defaults_enabled_when_field_omitted() {
        let toml_input = r#"
            [[rules]]
            id = "default_enabled"
            trigger = { kind = "hook", event = "session_end" }
            action = { kind = "prompt", prompt = "go" }
        "#;
        let config = AutomationConfig::from_toml_str(toml_input).expect("parse");
        assert_eq!(config.rules.len(), 1);
        assert!(config.rules[0].enabled);
    }

    #[test]
    fn from_toml_str_maps_invalid_toml_to_config_parse() {
        let err = AutomationConfig::from_toml_str("this is = not = valid")
            .expect_err("invalid TOML must fail");
        match err {
            AutomationError::ConfigParse(msg) => {
                assert!(!msg.is_empty(), "ConfigParse must carry the parser message")
            }
            other => panic!("expected ConfigParse, got {other:?}"),
        }
    }

    #[test]
    fn config_validate_accepts_unique_non_empty_ids() {
        let cfg = AutomationConfig {
            rules: vec![rule("a"), rule("b"), rule("c")],
        };
        cfg.validate().expect("unique ids must validate");
    }

    #[test]
    fn config_validate_rejects_duplicate_rule_ids() {
        let cfg = AutomationConfig {
            rules: vec![rule("dup"), rule("dup")],
        };
        let err = cfg.validate().expect_err("duplicate ids must fail");
        match err {
            AutomationError::ConfigValidation(msg) => {
                assert!(
                    msg.contains("duplicate automation rule id"),
                    "message must name the duplicate-id error: {msg}"
                );
                assert!(
                    msg.contains("dup"),
                    "message must include offending id: {msg}"
                );
            }
            other => panic!("expected ConfigValidation, got {other:?}"),
        }
    }

    #[test]
    fn config_validate_rejects_empty_rule_id() {
        let cfg = AutomationConfig {
            rules: vec![rule("")],
        };
        let err = cfg.validate().expect_err("empty id must fail");
        match err {
            AutomationError::ConfigValidation(msg) => assert!(
                msg.contains("rule id must not be empty"),
                "message must explain why: {msg}"
            ),
            other => panic!("expected ConfigValidation, got {other:?}"),
        }
    }

    #[test]
    fn config_validate_rejects_whitespace_only_rule_id() {
        let cfg = AutomationConfig {
            rules: vec![rule("   ")],
        };
        cfg.validate()
            .expect_err("whitespace-only id must fail like empty");
    }

    #[test]
    fn trigger_kind_string_is_stable() {
        // INVARIANT: scheduler / history persist the kind() string.
        // Renaming would silently drop history correlation for the
        // affected trigger family.
        assert_eq!(
            AutomationTrigger::Hook {
                event: HookEvent::SessionEnd
            }
            .kind(),
            "hook"
        );
        assert_eq!(
            AutomationTrigger::Cron {
                schedule: "*/5 * * * *".to_string()
            }
            .kind(),
            "cron"
        );
        assert_eq!(
            AutomationTrigger::Vcs {
                event: "post_commit".to_string()
            }
            .kind(),
            "vcs"
        );
    }

    #[test]
    fn trigger_validate_accepts_any_hook_event() {
        AutomationTrigger::Hook {
            event: HookEvent::SessionEnd,
        }
        .validate()
        .expect("hook trigger has no content gate");
    }

    #[test]
    fn trigger_validate_rejects_empty_cron_schedule() {
        let err = AutomationTrigger::Cron {
            schedule: "   ".to_string(),
        }
        .validate()
        .expect_err("blank cron schedule must fail");
        match err {
            AutomationError::ConfigValidation(msg) => assert!(
                msg.contains("cron trigger schedule"),
                "message must reference cron schedule: {msg}"
            ),
            other => panic!("expected ConfigValidation, got {other:?}"),
        }
    }

    #[test]
    fn trigger_validate_accepts_non_empty_cron_schedule() {
        AutomationTrigger::Cron {
            schedule: "@hourly".to_string(),
        }
        .validate()
        .expect("non-empty cron must validate");
    }

    #[test]
    fn trigger_validate_rejects_empty_vcs_event() {
        let err = AutomationTrigger::Vcs {
            event: "\t".to_string(),
        }
        .validate()
        .expect_err("blank vcs event must fail");
        match err {
            AutomationError::ConfigValidation(msg) => assert!(
                msg.contains("vcs trigger event"),
                "message must reference vcs event: {msg}"
            ),
            other => panic!("expected ConfigValidation, got {other:?}"),
        }
    }

    #[test]
    fn trigger_validate_accepts_non_empty_vcs_event() {
        AutomationTrigger::Vcs {
            event: "post_commit".to_string(),
        }
        .validate()
        .expect("non-empty vcs must validate");
    }

    #[test]
    fn action_kind_string_is_stable() {
        // INVARIANT: history + audit records persist the kind() string.
        assert_eq!(
            AutomationAction::Prompt {
                prompt: "x".to_string()
            }
            .kind(),
            "prompt"
        );
        assert_eq!(
            AutomationAction::Webhook {
                url: "https://example.test".to_string(),
                method: "POST".to_string(),
            }
            .kind(),
            "webhook"
        );
        assert_eq!(
            AutomationAction::Shell {
                command: "echo".to_string(),
                timeout_ms: None,
            }
            .kind(),
            "shell"
        );
    }

    #[test]
    fn action_validate_rejects_empty_prompt() {
        let err = AutomationAction::Prompt {
            prompt: "  ".to_string(),
        }
        .validate()
        .expect_err("blank prompt must fail");
        match err {
            AutomationError::ConfigValidation(msg) => assert!(
                msg.contains("prompt action prompt"),
                "message must reference prompt: {msg}"
            ),
            other => panic!("expected ConfigValidation, got {other:?}"),
        }
    }

    #[test]
    fn action_validate_rejects_empty_webhook_url() {
        let err = AutomationAction::Webhook {
            url: "".to_string(),
            method: "POST".to_string(),
        }
        .validate()
        .expect_err("blank url must fail");
        match err {
            AutomationError::ConfigValidation(msg) => assert!(
                msg.contains("webhook action url"),
                "message must reference webhook url: {msg}"
            ),
            other => panic!("expected ConfigValidation, got {other:?}"),
        }
    }

    #[test]
    fn action_validate_rejects_empty_shell_command() {
        let err = AutomationAction::Shell {
            command: "\n".to_string(),
            timeout_ms: Some(1000),
        }
        .validate()
        .expect_err("blank shell command must fail");
        match err {
            AutomationError::ConfigValidation(msg) => assert!(
                msg.contains("shell action command"),
                "message must reference shell command: {msg}"
            ),
            other => panic!("expected ConfigValidation, got {other:?}"),
        }
    }

    #[test]
    fn action_validate_accepts_all_filled_variants() {
        AutomationAction::Prompt {
            prompt: "summarize".to_string(),
        }
        .validate()
        .expect("filled prompt must pass");
        AutomationAction::Webhook {
            url: "https://example.test/hook".to_string(),
            method: "POST".to_string(),
        }
        .validate()
        .expect("filled webhook must pass");
        AutomationAction::Shell {
            command: "echo hello".to_string(),
            timeout_ms: None,
        }
        .validate()
        .expect("filled shell must pass");
    }

    #[test]
    fn config_validate_walks_rule_trigger_and_action_validators() {
        // INVARIANT: outer validate() must delegate to each rule's
        // validators, not just dedupe ids. A rule with a blank cron
        // schedule must surface as ConfigValidation, not silently
        // pass because its id is unique.
        let bad_trigger = AutomationRule {
            id: "bad".to_string(),
            enabled: true,
            trigger: AutomationTrigger::Cron {
                schedule: "".to_string(),
            },
            action: AutomationAction::Prompt {
                prompt: "x".to_string(),
            },
        };
        let cfg = AutomationConfig {
            rules: vec![bad_trigger],
        };
        let err = cfg.validate().expect_err("bad trigger must propagate");
        assert!(matches!(err, AutomationError::ConfigValidation(_)));
    }
}
