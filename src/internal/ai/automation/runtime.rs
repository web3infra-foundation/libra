use std::path::Path;

use sea_orm::DatabaseConnection;

use crate::{
    internal::{
        ai::{
            automation::{
                config::AutomationConfig,
                events::{AutomationError, AutomationRunResult, AutomationRuntimeEvent},
                executor::AutomationExecutor,
                history::AutomationHistory,
                scheduler::AutomationScheduler,
            },
            hooks::{HookEvent, LifecycleEventKind},
        },
        db,
    },
    utils::util,
};

/// Dispatch a normalized hook lifecycle event through automation rules and
/// persist every matched rule result.
pub async fn dispatch_hook_lifecycle_event_to_history(
    working_dir: &Path,
    conn: &DatabaseConnection,
    event_kind: LifecycleEventKind,
) -> Result<Vec<AutomationRunResult>, AutomationError> {
    let Some(hook_event) = automation_hook_event(event_kind) else {
        return Ok(Vec::new());
    };

    let config = AutomationConfig::load_from_working_dir(working_dir)?;
    config.validate()?;
    dispatch_hook_event_with_config_to_history(working_dir, conn, config, hook_event).await
}

/// Repository-oriented hook bridge used by provider hook ingestion. It avoids
/// touching the database when there is no matching automation work to do.
pub async fn dispatch_repo_hook_lifecycle_event_to_history(
    working_dir: &Path,
    storage_path: &Path,
    event_kind: LifecycleEventKind,
) -> Result<Vec<AutomationRunResult>, AutomationError> {
    let Some(hook_event) = automation_hook_event(event_kind) else {
        return Ok(Vec::new());
    };

    let config = AutomationConfig::load_from_working_dir(working_dir)?;
    config.validate()?;
    if !has_matching_hook_rule(&config, hook_event) {
        return Ok(Vec::new());
    }

    let conn = db::get_db_conn_instance_for_path(&storage_path.join(util::DATABASE))
        .await
        .map_err(|error| AutomationError::Database(error.to_string()))?;
    dispatch_hook_event_with_config_to_history(working_dir, &conn, config, hook_event).await
}

/// Best-effort VCS event bridge for top-level Libra VCS commands.
///
/// Automation must never make a successful VCS command fail, so this helper logs
/// dispatch problems and returns `()`.
pub async fn dispatch_current_repo_vcs_event_to_history(event: &'static str) {
    let working_dir = match util::try_working_dir() {
        Ok(path) => path,
        Err(error) => {
            tracing::warn!(
                target: "libra::ai::automation",
                event,
                error = %error,
                "failed to resolve working directory for automation VCS event"
            );
            return;
        }
    };
    let storage_path = match util::try_get_storage_path(Some(working_dir.clone())) {
        Ok(path) => path,
        Err(error) => {
            tracing::warn!(
                target: "libra::ai::automation",
                event,
                working_dir = %working_dir.display(),
                error = %error,
                "failed to resolve storage path for automation VCS event"
            );
            return;
        }
    };

    if let Err(error) = dispatch_repo_vcs_event_to_history(&working_dir, &storage_path, event).await
    {
        tracing::warn!(
            target: "libra::ai::automation",
            event,
            working_dir = %working_dir.display(),
            error = %error,
            "failed to dispatch automation VCS event"
        );
    }
}

pub async fn dispatch_vcs_event_to_history(
    working_dir: &Path,
    conn: &DatabaseConnection,
    event: &str,
) -> Result<Vec<AutomationRunResult>, AutomationError> {
    let config = AutomationConfig::load_from_working_dir(working_dir)?;
    config.validate()?;
    dispatch_vcs_event_with_config_to_history(working_dir, conn, config, event).await
}

pub async fn dispatch_repo_vcs_event_to_history(
    working_dir: &Path,
    storage_path: &Path,
    event: &str,
) -> Result<Vec<AutomationRunResult>, AutomationError> {
    let config = AutomationConfig::load_from_working_dir(working_dir)?;
    config.validate()?;
    if !has_matching_vcs_rule(&config, event) {
        return Ok(Vec::new());
    }

    let conn = db::get_db_conn_instance_for_path(&storage_path.join(util::DATABASE))
        .await
        .map_err(|error| AutomationError::Database(error.to_string()))?;
    dispatch_vcs_event_with_config_to_history(working_dir, &conn, config, event).await
}

async fn dispatch_hook_event_with_config_to_history(
    working_dir: &Path,
    conn: &DatabaseConnection,
    config: AutomationConfig,
    hook_event: HookEvent,
) -> Result<Vec<AutomationRunResult>, AutomationError> {
    if !has_matching_hook_rule(&config, hook_event) {
        return Ok(Vec::new());
    }

    let scheduler = AutomationScheduler::new(config);
    let executor = AutomationExecutor::live(working_dir);
    let results = scheduler
        .run_event(AutomationRuntimeEvent::hook(hook_event), &executor)
        .await?;
    for result in &results {
        AutomationHistory::append(conn, result).await?;
    }
    Ok(results)
}

async fn dispatch_vcs_event_with_config_to_history(
    working_dir: &Path,
    conn: &DatabaseConnection,
    config: AutomationConfig,
    event: &str,
) -> Result<Vec<AutomationRunResult>, AutomationError> {
    if !has_matching_vcs_rule(&config, event) {
        return Ok(Vec::new());
    }

    let scheduler = AutomationScheduler::new(config);
    let executor = AutomationExecutor::live(working_dir);
    let results = scheduler
        .run_event(AutomationRuntimeEvent::vcs(event), &executor)
        .await?;
    for result in &results {
        AutomationHistory::append(conn, result).await?;
    }
    Ok(results)
}

fn has_matching_hook_rule(config: &AutomationConfig, hook_event: HookEvent) -> bool {
    config.rules.iter().any(|rule| {
        rule.enabled
            && matches!(
                &rule.trigger,
                crate::internal::ai::automation::config::AutomationTrigger::Hook { event }
                    if *event == hook_event
            )
    })
}

fn has_matching_vcs_rule(config: &AutomationConfig, vcs_event: &str) -> bool {
    config.rules.iter().any(|rule| {
        rule.enabled
            && matches!(
                &rule.trigger,
                crate::internal::ai::automation::config::AutomationTrigger::Vcs { event }
                    if event == vcs_event
            )
    })
}

fn automation_hook_event(event_kind: LifecycleEventKind) -> Option<HookEvent> {
    match event_kind {
        LifecycleEventKind::SessionStart => Some(HookEvent::SessionStart),
        LifecycleEventKind::SessionEnd => Some(HookEvent::SessionEnd),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::automation::config::{
        AutomationAction, AutomationConfig, AutomationRule, AutomationTrigger,
    };

    fn hook_rule(id: &str, event: HookEvent, enabled: bool) -> AutomationRule {
        AutomationRule {
            id: id.to_string(),
            enabled,
            trigger: AutomationTrigger::Hook { event },
            action: AutomationAction::Prompt {
                prompt: "noop".to_string(),
            },
        }
    }

    fn vcs_rule(id: &str, event: &str, enabled: bool) -> AutomationRule {
        AutomationRule {
            id: id.to_string(),
            enabled,
            trigger: AutomationTrigger::Vcs {
                event: event.to_string(),
            },
            action: AutomationAction::Prompt {
                prompt: "noop".to_string(),
            },
        }
    }

    fn cron_rule(id: &str, enabled: bool) -> AutomationRule {
        AutomationRule {
            id: id.to_string(),
            enabled,
            trigger: AutomationTrigger::Cron {
                schedule: "@hourly".to_string(),
            },
            action: AutomationAction::Prompt {
                prompt: "noop".to_string(),
            },
        }
    }

    #[test]
    fn automation_hook_event_maps_session_lifecycle_kinds() {
        // INVARIANT: only session-start/end currently project onto
        // user-visible automation `HookEvent` variants. The repo-hook
        // bridge silently drops every other lifecycle kind.
        assert_eq!(
            automation_hook_event(LifecycleEventKind::SessionStart),
            Some(HookEvent::SessionStart)
        );
        assert_eq!(
            automation_hook_event(LifecycleEventKind::SessionEnd),
            Some(HookEvent::SessionEnd)
        );
    }

    #[test]
    fn automation_hook_event_returns_none_for_non_session_kinds() {
        // INVARIANT: any future `LifecycleEventKind` variant must be
        // explicitly mapped — defaulting to `Some(_)` would push
        // unexpected events through the dispatcher.
        for kind in [
            LifecycleEventKind::TurnStart,
            LifecycleEventKind::ToolUse,
            LifecycleEventKind::ModelUpdate,
            LifecycleEventKind::Compaction,
            LifecycleEventKind::CompactionCompleted,
            LifecycleEventKind::PermissionRequest,
            LifecycleEventKind::SourceEnabled,
            LifecycleEventKind::SourceDisabled,
            LifecycleEventKind::TurnEnd,
        ] {
            assert!(
                automation_hook_event(kind).is_none(),
                "lifecycle kind {kind:?} must not map to a HookEvent without an explicit branch"
            );
        }
    }

    #[test]
    fn has_matching_hook_rule_requires_enabled_and_event_equality() {
        let config = AutomationConfig {
            rules: vec![
                hook_rule("disabled_end", HookEvent::SessionEnd, false),
                hook_rule("start", HookEvent::SessionStart, true),
                hook_rule("end", HookEvent::SessionEnd, true),
            ],
        };
        assert!(has_matching_hook_rule(&config, HookEvent::SessionEnd));
        assert!(has_matching_hook_rule(&config, HookEvent::SessionStart));
    }

    #[test]
    fn has_matching_hook_rule_returns_false_when_only_disabled_rules_match() {
        let config = AutomationConfig {
            rules: vec![hook_rule("disabled_end", HookEvent::SessionEnd, false)],
        };
        assert!(!has_matching_hook_rule(&config, HookEvent::SessionEnd));
    }

    #[test]
    fn has_matching_hook_rule_returns_false_when_only_other_trigger_kinds_present() {
        let config = AutomationConfig {
            rules: vec![cron_rule("c", true), vcs_rule("v", "post_commit", true)],
        };
        assert!(!has_matching_hook_rule(&config, HookEvent::SessionEnd));
    }

    #[test]
    fn has_matching_hook_rule_returns_false_for_empty_config() {
        let config = AutomationConfig::default();
        assert!(!has_matching_hook_rule(&config, HookEvent::SessionEnd));
        assert!(!has_matching_hook_rule(&config, HookEvent::SessionStart));
    }

    #[test]
    fn has_matching_vcs_rule_requires_exact_event_string() {
        // INVARIANT: VCS event names are compared as raw strings, not
        // normalised. `post_commit` and `post-commit` are different
        // events; a silent change to case-insensitive matching would
        // re-fire unrelated rules.
        let config = AutomationConfig {
            rules: vec![
                vcs_rule("post_commit", "post_commit", true),
                vcs_rule("post_push", "post_push", true),
            ],
        };
        assert!(has_matching_vcs_rule(&config, "post_commit"));
        assert!(has_matching_vcs_rule(&config, "post_push"));
        assert!(!has_matching_vcs_rule(&config, "post-commit"));
        assert!(!has_matching_vcs_rule(&config, "Post_Commit"));
        assert!(!has_matching_vcs_rule(&config, "post_commit "));
    }

    #[test]
    fn has_matching_vcs_rule_skips_disabled_rules() {
        let config = AutomationConfig {
            rules: vec![vcs_rule("off", "post_commit", false)],
        };
        assert!(!has_matching_vcs_rule(&config, "post_commit"));
    }

    #[test]
    fn has_matching_vcs_rule_returns_false_when_only_other_trigger_kinds_present() {
        let config = AutomationConfig {
            rules: vec![hook_rule("h", HookEvent::SessionEnd, true)],
        };
        assert!(!has_matching_vcs_rule(&config, "post_commit"));
    }

    #[test]
    fn has_matching_vcs_rule_returns_false_for_empty_config() {
        let config = AutomationConfig::default();
        assert!(!has_matching_vcs_rule(&config, "post_commit"));
        assert!(!has_matching_vcs_rule(&config, ""));
    }
}
