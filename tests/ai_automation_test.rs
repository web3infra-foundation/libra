//! CEX-15 automation MVP contract tests.

use std::fs;

use chrono::{TimeZone, Utc};
use libra::internal::{
    ai::{
        automation::{
            AutomationAction, AutomationConfig, AutomationExecutor, AutomationHistory,
            AutomationRunStatus, AutomationRuntimeEvent, AutomationScheduler, AutomationTrigger,
            VCS_EVENT_POST_COMMIT, dispatch_hook_lifecycle_event_to_history,
            dispatch_vcs_event_to_history,
        },
        hooks::{HookEvent, LifecycleEventKind},
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
async fn automation_scheduler_dispatches_hook_and_vcs_runtime_events() {
    let config = AutomationConfig::from_toml_str(
        r#"
        [[rules]]
        id = "session_end_hook"
        trigger = { kind = "hook", event = "session_end" }
        action = { kind = "prompt", prompt = "summarize session" }

        [[rules]]
        id = "post_commit_vcs"
        trigger = { kind = "vcs", event = "post_commit" }
        action = { kind = "prompt", prompt = "summarize commit" }

        [[rules]]
        id = "hourly"
        trigger = { kind = "cron", schedule = "@hourly" }
        action = { kind = "prompt", prompt = "cron only" }
    "#,
    )
    .expect("parse automations");

    let scheduler = AutomationScheduler::new(config);
    let hook_results = scheduler
        .run_event(
            AutomationRuntimeEvent::hook(HookEvent::SessionEnd),
            &AutomationExecutor::dry_run(),
        )
        .await
        .expect("dispatch hook event");
    assert_eq!(
        hook_results
            .iter()
            .map(|result| result.rule_id.as_str())
            .collect::<Vec<_>>(),
        vec!["session_end_hook"]
    );

    let vcs_results = scheduler
        .run_event(
            AutomationRuntimeEvent::vcs("post_commit"),
            &AutomationExecutor::dry_run(),
        )
        .await
        .expect("dispatch vcs event");
    assert_eq!(
        vcs_results
            .iter()
            .map(|result| result.rule_id.as_str())
            .collect::<Vec<_>>(),
        vec!["post_commit_vcs"]
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
async fn hook_lifecycle_event_dispatches_matching_rule_to_history() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let libra_dir = tmp.path().join(".libra");
    fs::create_dir(&libra_dir).expect("create .libra dir");
    fs::write(
        libra_dir.join("automations.toml"),
        r#"
        [[rules]]
        id = "session_end_summary"
        trigger = { kind = "hook", event = "session_end" }
        action = { kind = "prompt", prompt = "summarize this session" }

        [[rules]]
        id = "session_start_only"
        trigger = { kind = "hook", event = "session_start" }
        action = { kind = "prompt", prompt = "not this event" }
    "#,
    )
    .expect("write automations");

    let conn = Database::connect("sqlite::memory:")
        .await
        .expect("connect sqlite");
    run_builtin_migrations(&conn).await.expect("run migrations");

    let results =
        dispatch_hook_lifecycle_event_to_history(tmp.path(), &conn, LifecycleEventKind::SessionEnd)
            .await
            .expect("dispatch hook lifecycle event");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].rule_id, "session_end_summary");
    assert_eq!(results[0].trigger_kind, "hook");
    assert_eq!(results[0].status, AutomationRunStatus::Succeeded);

    let rows = AutomationHistory::list_recent(&conn, 10)
        .await
        .expect("list automation log");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].rule_id, "session_end_summary");
    assert_eq!(rows[0].details["prompt"], "summarize this session");
}

#[tokio::test]
async fn vcs_event_dispatches_matching_rule_to_history() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let libra_dir = tmp.path().join(".libra");
    fs::create_dir(&libra_dir).expect("create .libra dir");
    fs::write(
        libra_dir.join("automations.toml"),
        r#"
        [[rules]]
        id = "commit_summary"
        trigger = { kind = "vcs", event = "post_commit" }
        action = { kind = "prompt", prompt = "summarize this commit" }

        [[rules]]
        id = "add_only"
        trigger = { kind = "vcs", event = "post_add" }
        action = { kind = "prompt", prompt = "not this event" }
    "#,
    )
    .expect("write automations");

    let conn = Database::connect("sqlite::memory:")
        .await
        .expect("connect sqlite");
    run_builtin_migrations(&conn).await.expect("run migrations");

    let results = dispatch_vcs_event_to_history(tmp.path(), &conn, VCS_EVENT_POST_COMMIT)
        .await
        .expect("dispatch vcs event");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].rule_id, "commit_summary");
    assert_eq!(results[0].trigger_kind, "vcs");
    assert_eq!(results[0].status, AutomationRunStatus::Succeeded);

    let rows = AutomationHistory::list_recent(&conn, 10)
        .await
        .expect("list automation log");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].rule_id, "commit_summary");
    assert_eq!(rows[0].details["prompt"], "summarize this commit");
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

#[tokio::test]
async fn automation_history_prune_removes_only_older_rows_idempotently() {
    use libra::internal::ai::automation::AutomationRunResult;
    use sea_orm::Database;

    let conn = Database::connect("sqlite::memory:")
        .await
        .expect("connect sqlite");
    run_builtin_migrations(&conn).await.expect("run migrations");

    let old = AutomationRunResult {
        id: "row-old".to_string(),
        rule_id: "rule-old".to_string(),
        trigger_kind: "cron".to_string(),
        action_kind: "prompt".to_string(),
        status: AutomationRunStatus::Succeeded,
        message: "old run".to_string(),
        details: serde_json::json!({}),
        started_at: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        finished_at: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 1).unwrap(),
    };
    let recent = AutomationRunResult {
        id: "row-recent".to_string(),
        rule_id: "rule-recent".to_string(),
        trigger_kind: "cron".to_string(),
        action_kind: "prompt".to_string(),
        status: AutomationRunStatus::Succeeded,
        message: "recent run".to_string(),
        details: serde_json::json!({}),
        started_at: Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap(),
        finished_at: Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 1).unwrap(),
    };

    AutomationHistory::append(&conn, &old)
        .await
        .expect("append old row");
    AutomationHistory::append(&conn, &recent)
        .await
        .expect("append recent row");

    let cutoff = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
    let deleted = AutomationHistory::prune_before(&conn, &cutoff.to_rfc3339())
        .await
        .expect("prune older rows");
    assert_eq!(deleted, 1, "old row should be removed");

    let rows = AutomationHistory::list_recent(&conn, 10)
        .await
        .expect("list automation log");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].rule_id, "rule-recent");

    // Second prune at the same cutoff is a no-op (idempotent).
    let deleted_again = AutomationHistory::prune_before(&conn, &cutoff.to_rfc3339())
        .await
        .expect("repeat prune");
    assert_eq!(
        deleted_again, 0,
        "idempotent re-prune must delete zero rows"
    );
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
