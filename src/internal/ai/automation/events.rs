use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;
use uuid::Uuid;

use crate::internal::ai::{automation::config::AutomationTrigger, hooks::HookEvent};

pub const VCS_EVENT_POST_ADD: &str = "post_add";
pub const VCS_EVENT_POST_BRANCH: &str = "post_branch";
pub const VCS_EVENT_POST_COMMIT: &str = "post_commit";
pub const VCS_EVENT_POST_PUSH: &str = "post_push";
pub const VCS_EVENT_POST_SWITCH: &str = "post_switch";

#[derive(Debug, Error)]
pub enum AutomationError {
    #[error("failed to parse automation config: {0}")]
    ConfigParse(String),
    #[error("invalid automation config: {0}")]
    ConfigValidation(String),
    #[error("unsupported cron schedule `{0}`")]
    UnsupportedCron(String),
    #[error("automation database error: {0}")]
    Database(String),
    #[error("automation action failed: {0}")]
    Action(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationRunStatus {
    Succeeded,
    Failed,
    ApprovalRequired,
    Skipped,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AutomationRuntimeEvent {
    Hook { event: HookEvent },
    Vcs { event: String },
}

impl AutomationRuntimeEvent {
    pub fn hook(event: HookEvent) -> Self {
        Self::Hook { event }
    }

    pub fn vcs(event: impl Into<String>) -> Self {
        Self::Vcs {
            event: event.into(),
        }
    }

    pub fn matches_trigger(&self, trigger: &AutomationTrigger) -> bool {
        match (self, trigger) {
            (
                Self::Hook { event },
                AutomationTrigger::Hook {
                    event: trigger_event,
                },
            ) => event == trigger_event,
            (
                Self::Vcs { event },
                AutomationTrigger::Vcs {
                    event: trigger_event,
                },
            ) => event == trigger_event,
            _ => false,
        }
    }
}

impl AutomationRunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::ApprovalRequired => "approval_required",
            Self::Skipped => "skipped",
        }
    }

    pub fn parse(value: &str) -> Result<Self, AutomationError> {
        match value {
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "approval_required" => Ok(Self::ApprovalRequired),
            "skipped" => Ok(Self::Skipped),
            other => Err(AutomationError::Database(format!(
                "unknown automation status `{other}`"
            ))),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AutomationRunResult {
    pub id: String,
    pub rule_id: String,
    pub trigger_kind: String,
    pub action_kind: String,
    pub status: AutomationRunStatus,
    pub message: String,
    pub details: Value,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
}

impl AutomationRunResult {
    pub fn new(
        rule_id: impl Into<String>,
        trigger_kind: impl Into<String>,
        action_kind: impl Into<String>,
        status: AutomationRunStatus,
        message: impl Into<String>,
        details: Value,
        started_at: DateTime<Utc>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            rule_id: rule_id.into(),
            trigger_kind: trigger_kind.into(),
            action_kind: action_kind.into(),
            status,
            message: message.into(),
            details,
            started_at,
            finished_at: Utc::now(),
        }
    }

    pub fn skipped(rule_id: impl Into<String>, reason: impl Into<String>) -> Self {
        let now = Utc::now();
        Self::new(
            rule_id,
            "manual",
            "none",
            AutomationRunStatus::Skipped,
            reason,
            json!({}),
            now,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::AutomationError;

    #[test]
    fn automation_error_display_pins_each_variant() {
        assert_eq!(
            AutomationError::ConfigParse("invalid toml".to_string()).to_string(),
            "failed to parse automation config: invalid toml",
        );
        assert_eq!(
            AutomationError::ConfigValidation("missing action".to_string()).to_string(),
            "invalid automation config: missing action",
        );
        assert_eq!(
            AutomationError::UnsupportedCron("@hourly".to_string()).to_string(),
            "unsupported cron schedule `@hourly`",
        );
        assert_eq!(
            AutomationError::Database("connection lost".to_string()).to_string(),
            "automation database error: connection lost",
        );
        assert_eq!(
            AutomationError::Action("shell exit 1".to_string()).to_string(),
            "automation action failed: shell exit 1",
        );
    }
}
