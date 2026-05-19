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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::{automation::config::AutomationAction, hooks::HookEvent};

    fn rule(id: &str, enabled: bool, action: AutomationAction) -> AutomationRule {
        AutomationRule {
            id: id.to_string(),
            enabled,
            trigger: AutomationTrigger::Hook {
                event: HookEvent::SessionEnd,
            },
            action,
        }
    }

    #[test]
    fn default_shell_timeout_ms_is_thirty_seconds() {
        // INVARIANT: a silent change here would lengthen or shorten
        // every shell-action timeout. 30s is the documented default
        // and the integration tests rely on it implicitly.
        assert_eq!(DEFAULT_SHELL_TIMEOUT_MS, 30_000);
    }

    #[test]
    fn dry_run_constructor_sets_dry_run_flag_true() {
        let executor = AutomationExecutor::dry_run();
        assert!(
            executor.dry_run,
            "dry_run() must produce a dry-run executor"
        );
        // working_dir is whatever the test process happens to be in;
        // we only require it's a non-empty path, never the literal
        // empty string.
        assert!(
            !executor.working_dir.as_os_str().is_empty(),
            "dry_run() must populate working_dir from current_dir() or fallback"
        );
    }

    #[test]
    fn live_constructor_sets_dry_run_flag_false_and_stores_working_dir() {
        let executor = AutomationExecutor::live("/tmp/libra-automation-test");
        assert!(!executor.dry_run, "live() must produce a live executor");
        assert_eq!(
            executor.working_dir,
            PathBuf::from("/tmp/libra-automation-test")
        );
    }

    #[tokio::test]
    async fn execute_rule_skips_disabled_rule_with_kind_strings() {
        let trigger = AutomationTrigger::Cron {
            schedule: "@hourly".to_string(),
        };
        let prompt_rule = rule(
            "off",
            false,
            AutomationAction::Prompt {
                prompt: "won't run".to_string(),
            },
        );
        let executor = AutomationExecutor::dry_run();
        let result = executor.execute_rule(&prompt_rule, trigger.clone()).await;
        assert_eq!(result.rule_id, "off");
        assert_eq!(result.status, AutomationRunStatus::Skipped);
        assert_eq!(result.message, "automation rule is disabled");
        // INVARIANT: the trigger_kind / action_kind columns must echo
        // the *call-time* trigger kind, not the rule's stored trigger
        // (callers pass the runtime trigger, which may be different
        // from the rule's configured trigger for dispatch).
        assert_eq!(result.trigger_kind, "cron");
        assert_eq!(result.action_kind, "prompt");
        assert_eq!(result.details, serde_json::json!({}));
    }

    #[tokio::test]
    async fn execute_rule_prompt_action_succeeds_with_prompt_in_details() {
        let r = rule(
            "p",
            true,
            AutomationAction::Prompt {
                prompt: "summarise status".to_string(),
            },
        );
        let trigger = AutomationTrigger::Hook {
            event: HookEvent::SessionEnd,
        };
        let result = AutomationExecutor::dry_run()
            .execute_rule(&r, trigger)
            .await;
        assert_eq!(result.status, AutomationRunStatus::Succeeded);
        assert_eq!(result.message, "automation prompt action prepared");
        assert_eq!(result.trigger_kind, "hook");
        assert_eq!(result.action_kind, "prompt");
        // INVARIANT: the persisted detail key is exactly `prompt`;
        // downstream consumers (rerun UI, audit search) key on it.
        assert_eq!(
            result.details.get("prompt").and_then(|v| v.as_str()),
            Some("summarise status")
        );
    }

    #[tokio::test]
    async fn execute_rule_webhook_action_succeeds_and_records_dry_run_flag() {
        let r = rule(
            "w",
            true,
            AutomationAction::Webhook {
                url: "https://example.test/hook".to_string(),
                method: "POST".to_string(),
            },
        );
        let trigger = AutomationTrigger::Vcs {
            event: "post_commit".to_string(),
        };
        let result = AutomationExecutor::dry_run()
            .execute_rule(&r, trigger)
            .await;
        assert_eq!(result.status, AutomationRunStatus::Succeeded);
        assert_eq!(result.message, "automation webhook action prepared");
        assert_eq!(result.trigger_kind, "vcs");
        assert_eq!(result.action_kind, "webhook");
        assert_eq!(
            result.details.get("url").and_then(|v| v.as_str()),
            Some("https://example.test/hook")
        );
        assert_eq!(
            result.details.get("method").and_then(|v| v.as_str()),
            Some("POST")
        );
        // INVARIANT: webhook details must record `dry_run: true` when
        // executed under a dry-run executor — without it, replay
        // tooling cannot distinguish prepared vs. delivered webhooks.
        assert_eq!(
            result.details.get("dry_run").and_then(|v| v.as_bool()),
            Some(true)
        );
    }

    #[tokio::test]
    async fn execute_rule_webhook_action_records_dry_run_false_when_live() {
        let r = rule(
            "wl",
            true,
            AutomationAction::Webhook {
                url: "https://example.test/hook".to_string(),
                method: "PUT".to_string(),
            },
        );
        let trigger = AutomationTrigger::Hook {
            event: HookEvent::SessionEnd,
        };
        let result = AutomationExecutor::live("/tmp")
            .execute_rule(&r, trigger)
            .await;
        assert_eq!(result.status, AutomationRunStatus::Succeeded);
        assert_eq!(
            result.details.get("dry_run").and_then(|v| v.as_bool()),
            Some(false),
            "live executor must surface dry_run: false so downstream replay tooling knows the row reflects a real prepare"
        );
        assert_eq!(
            result.details.get("method").and_then(|v| v.as_str()),
            Some("PUT")
        );
    }
}
