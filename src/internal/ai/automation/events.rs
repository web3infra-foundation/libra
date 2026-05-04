use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;
use uuid::Uuid;

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
