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
