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
