use std::{path::PathBuf, time::Duration};

use chrono::Utc;
use serde_json::json;
use tokio::{process::Command, time::timeout};

use crate::internal::ai::{
    automation::{
        config::{AutomationAction, AutomationRule, AutomationTrigger},
        events::{AutomationRunResult, AutomationRunStatus},
    },
    runtime::hardening::{CommandSafetySurface, SafetyDisposition},
    tools::utils::classify_ai_command_safety,
};

const DEFAULT_SHELL_TIMEOUT_MS: u64 = 30_000;

#[derive(Clone, Debug)]
pub struct AutomationExecutor {
    dry_run: bool,
    working_dir: PathBuf,
}

impl AutomationExecutor {
    pub fn dry_run() -> Self {
        Self {
            dry_run: true,
            working_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        }
    }

    pub fn live(working_dir: impl Into<PathBuf>) -> Self {
        Self {
            dry_run: false,
            working_dir: working_dir.into(),
        }
    }

    pub async fn execute_rule(
        &self,
        rule: &AutomationRule,
        trigger: AutomationTrigger,
    ) -> AutomationRunResult {
        let started_at = Utc::now();
        if !rule.enabled {
            return AutomationRunResult::new(
                rule.id.clone(),
                trigger.kind(),
                rule.action.kind(),
                AutomationRunStatus::Skipped,
                "automation rule is disabled",
                json!({}),
                started_at,
            );
        }

        match &rule.action {
            AutomationAction::Prompt { prompt } => AutomationRunResult::new(
                rule.id.clone(),
                trigger.kind(),
                rule.action.kind(),
                AutomationRunStatus::Succeeded,
                "automation prompt action prepared",
                json!({ "prompt": prompt }),
                started_at,
            ),
            AutomationAction::Webhook { url, method } => AutomationRunResult::new(
                rule.id.clone(),
                trigger.kind(),
                rule.action.kind(),
                AutomationRunStatus::Succeeded,
                "automation webhook action prepared",
                json!({ "url": url, "method": method, "dry_run": self.dry_run }),
                started_at,
            ),
            AutomationAction::Shell {
                command,
                timeout_ms,
            } => {
                self.execute_shell_action(rule, trigger, command, *timeout_ms, started_at)
                    .await
            }
        }
    }

    async fn execute_shell_action(
        &self,
        rule: &AutomationRule,
        trigger: AutomationTrigger,
        command: &str,
        timeout_ms: Option<u64>,
        started_at: chrono::DateTime<Utc>,
    ) -> AutomationRunResult {
        let decision = classify_ai_command_safety(CommandSafetySurface::Shell, command, &[]);
        let safety = serde_json::to_value(&decision).unwrap_or_else(|error| {
            json!({
                "serialization_error": error.to_string(),
                "rule_name": decision.rule_name,
                "reason": decision.reason,
            })
        });

        match decision.disposition {
            SafetyDisposition::Deny => AutomationRunResult::new(
                rule.id.clone(),
                trigger.kind(),
                rule.action.kind(),
                AutomationRunStatus::Failed,
                "automation shell action failed safety preflight",
                json!({ "command": command, "safety": safety }),
                started_at,
            ),
            SafetyDisposition::NeedsHuman => AutomationRunResult::new(
                rule.id.clone(),
                trigger.kind(),
                rule.action.kind(),
                AutomationRunStatus::ApprovalRequired,
                "automation shell action requires approval and was not spawned",
                json!({ "command": command, "safety": safety }),
                started_at,
            ),
            SafetyDisposition::Allow if self.dry_run => AutomationRunResult::new(
                rule.id.clone(),
                trigger.kind(),
                rule.action.kind(),
                AutomationRunStatus::Succeeded,
                "automation shell action passed safety preflight (dry-run)",
                json!({ "command": command, "safety": safety, "dry_run": true }),
                started_at,
            ),
            SafetyDisposition::Allow => {
                self.spawn_allowed_shell(rule, trigger, command, timeout_ms, safety, started_at)
                    .await
            }
        }
    }

    async fn spawn_allowed_shell(
        &self,
        rule: &AutomationRule,
        trigger: AutomationTrigger,
        command: &str,
        timeout_ms: Option<u64>,
        safety: serde_json::Value,
        started_at: chrono::DateTime<Utc>,
    ) -> AutomationRunResult {
        let mut process = shell_command(command);
        process.current_dir(&self.working_dir);
        let timeout_duration =
            Duration::from_millis(timeout_ms.unwrap_or(DEFAULT_SHELL_TIMEOUT_MS));
        let output = timeout(timeout_duration, process.output()).await;

        match output {
            Ok(Ok(output)) => {
                let success = output.status.success();
                AutomationRunResult::new(
                    rule.id.clone(),
                    trigger.kind(),
                    rule.action.kind(),
                    if success {
                        AutomationRunStatus::Succeeded
                    } else {
                        AutomationRunStatus::Failed
                    },
                    if success {
                        "automation shell action completed"
                    } else {
                        "automation shell action exited non-zero"
                    },
                    json!({
                        "command": command,
                        "safety": safety,
                        "exit_code": output.status.code(),
                        "stdout": String::from_utf8_lossy(&output.stdout),
                        "stderr": String::from_utf8_lossy(&output.stderr),
                    }),
                    started_at,
                )
            }
            Ok(Err(error)) => AutomationRunResult::new(
                rule.id.clone(),
                trigger.kind(),
                rule.action.kind(),
                AutomationRunStatus::Failed,
                "automation shell action failed to spawn",
                json!({ "command": command, "safety": safety, "error": error.to_string() }),
                started_at,
            ),
            Err(_) => AutomationRunResult::new(
                rule.id.clone(),
                trigger.kind(),
                rule.action.kind(),
                AutomationRunStatus::Failed,
                "automation shell action timed out",
                json!({ "command": command, "safety": safety, "timeout_ms": timeout_duration.as_millis() }),
                started_at,
            ),
        }
    }
}

#[cfg(unix)]
fn shell_command(command: &str) -> Command {
    let mut process = Command::new("sh");
    process.arg("-c").arg(command);
    process
}

#[cfg(windows)]
fn shell_command(command: &str) -> Command {
    let mut process = Command::new("cmd");
    process.arg("/C").arg(command);
    process
}
