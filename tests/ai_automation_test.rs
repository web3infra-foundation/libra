//! CEX-15 automation MVP contract tests.

use chrono::{TimeZone, Utc};
use libra::internal::{
    ai::{
        automation::{
            AutomationAction, AutomationConfig, AutomationExecutor, AutomationHistory,
            AutomationRunStatus, AutomationScheduler, AutomationTrigger,
        },
        hooks::LifecycleEventKind,
    },
    db::migration::run_builtin_migrations,
};
use sea_orm::Database;

#[test]
fn automation_config_parses_prompt_webhook_and_shell_rules() {
    let toml = r#"
        [[rules]]
        id = "daily_prompt"
        trigger = { kind = "cron", schedule = "*/15 * * * *" }
        action = { kind = "prompt", prompt = "summarize status" }

        [[rules]]
        id = "notify"
        trigger = { kind = "hook", event = "session_end" }
        action = { kind = "webhook", url = "https://example.test/hook", method = "POST" }

        [[rules]]
        id = "safe_shell"
        trigger = { kind = "cron", schedule = "@hourly" }
        action = { kind = "shell", command = "pwd" }
    "#;

    let config = AutomationConfig::from_toml_str(toml).expect("parse automations");
    config.validate().expect("validate automations");

    assert_eq!(config.rules.len(), 3);
    assert!(matches!(
        config.rules[0].action,
        AutomationAction::Prompt { .. }
    ));
    assert!(matches!(
        config.rules[1].action,
        AutomationAction::Webhook { .. }
    ));
    assert!(matches!(
        config.rules[2].action,
        AutomationAction::Shell { .. }
    ));
}

#[test]
fn automation_cron_simulation_selects_due_rules() {
    let config = AutomationConfig::from_toml_str(
        r#"
        [[rules]]
        id = "quarter_hour"
        trigger = { kind = "cron", schedule = "*/15 * * * *" }
        action = { kind = "prompt", prompt = "run" }

        [[rules]]
        id = "hourly"
        trigger = { kind = "cron", schedule = "@hourly" }
        action = { kind = "prompt", prompt = "run hourly" }
    "#,
    )
    .expect("parse automations");

    let scheduler = AutomationScheduler::new(config);
    let due = scheduler
        .due_rules_at(Utc.with_ymd_and_hms(2026, 5, 3, 10, 30, 0).unwrap())
        .expect("compute due rules");

    assert_eq!(
        due.iter().map(|rule| rule.id.as_str()).collect::<Vec<_>>(),
        vec!["quarter_hour"]
    );
}

#[tokio::test]
async fn automation_shell_action_reuses_safety_classifier_before_spawning() {
    let rule = AutomationConfig::from_toml_str(
        r#"
        [[rules]]
        id = "dangerous"
        trigger = { kind = "cron", schedule = "@hourly" }
        action = { kind = "shell", command = "rm -rf /" }
    "#,
    )
    .expect("parse automations")
    .rules
    .remove(0);

    let executor = AutomationExecutor::dry_run();
    let result = executor
        .execute_rule(
            &rule,
            AutomationTrigger::Cron {
                schedule: "@hourly".to_string(),
            },
        )
        .await;

    assert_eq!(result.status, AutomationRunStatus::Failed);
    assert_eq!(result.details["safety"]["disposition"], "deny");
    assert!(
        result
            .message
            .contains("automation shell action failed safety preflight")
    );
}

#[tokio::test]
async fn automation_scheduler_continues_after_rule_failure() {
    let config = AutomationConfig::from_toml_str(
        r#"
        [[rules]]
        id = "bad_shell"
        trigger = { kind = "cron", schedule = "@hourly" }
        action = { kind = "shell", command = "rm -rf /" }

        [[rules]]
        id = "prompt_after_failure"
        trigger = { kind = "cron", schedule = "@hourly" }
        action = { kind = "prompt", prompt = "still run" }
    "#,
    )
    .expect("parse automations");

    let scheduler = AutomationScheduler::new(config);
    let results = scheduler
        .run_due_at(
            Utc.with_ymd_and_hms(2026, 5, 3, 11, 0, 0).unwrap(),
            &AutomationExecutor::dry_run(),
        )
        .await
        .expect("run due automations");

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].rule_id, "bad_shell");
    assert_eq!(results[0].status, AutomationRunStatus::Failed);
    assert_eq!(results[1].rule_id, "prompt_after_failure");
    assert_eq!(results[1].status, AutomationRunStatus::Succeeded);
}

#[tokio::test]
async fn automation_history_uses_builtin_migration() {
    let conn = Database::connect("sqlite::memory:")
        .await
        .expect("connect sqlite");
    run_builtin_migrations(&conn).await.expect("run migrations");

    let result = AutomationExecutor::dry_run()
        .execute_rule(
            &AutomationConfig::from_toml_str(
                r#"
                [[rules]]
                id = "prompt"
                trigger = { kind = "cron", schedule = "@hourly" }
                action = { kind = "prompt", prompt = "hello" }
            "#,
            )
            .expect("parse automations")
            .rules
            .remove(0),
            AutomationTrigger::Cron {
                schedule: "@hourly".to_string(),
            },
        )
        .await;

    AutomationHistory::append(&conn, &result)
        .await
        .expect("append automation log");
    let rows = AutomationHistory::list_recent(&conn, 10)
        .await
        .expect("list automation log");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].rule_id, "prompt");
    assert_eq!(rows[0].status, AutomationRunStatus::Succeeded);
}

#[test]
fn automation_lifecycle_event_kinds_are_available_for_step_two() {
    assert_eq!(
        LifecycleEventKind::PermissionRequest.to_string(),
        "permission_request"
    );
    assert_eq!(
        LifecycleEventKind::SourceEnabled.to_string(),
        "source_enabled"
    );
    assert_eq!(
        LifecycleEventKind::SourceDisabled.to_string(),
        "source_disabled"
    );
    assert_eq!(
        LifecycleEventKind::CompactionCompleted.to_string(),
        "compaction_completed"
    );
}
