//! `libra automation` command surface for CEX-15.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};
use serde::Serialize;

use crate::{
    info_println,
    internal::{
        ai::automation::{
            AutomationConfig, AutomationExecutor, AutomationHistory, AutomationRunResult,
            AutomationScheduler,
        },
        db::get_db_conn_instance_for_path,
    },
    utils::{
        error::{CliError, CliResult},
        output::{OutputConfig, emit_json_data},
        util::{DATABASE, try_get_storage_path},
    },
};

#[derive(Parser, Debug)]
pub struct AutomationArgs {
    #[command(subcommand)]
    pub command: AutomationSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum AutomationSubcommand {
    /// List configured automation rules.
    List,
    /// Run due cron rules, or one named rule.
    Run {
        /// Run one rule regardless of whether its cron trigger is due.
        #[arg(long)]
        rule: Option<String>,
        /// Simulated current time as RFC3339. Defaults to now.
        #[arg(long)]
        now: Option<String>,
        /// Actually spawn shell actions that pass safety preflight.
        #[arg(long)]
        live: bool,
    },
    /// Show recent automation history rows.
    History {
        #[arg(long, default_value_t = 20)]
        limit: u64,
    },
}

#[derive(Serialize)]
struct AutomationListOutput<'a> {
    rules: &'a [crate::internal::ai::automation::AutomationRule],
}

#[derive(Serialize)]
struct AutomationRunOutput {
    results: Vec<AutomationRunResult>,
}

pub async fn execute_safe(args: AutomationArgs, output: &OutputConfig) -> CliResult<()> {
    match args.command {
        AutomationSubcommand::List => list_rules(output).await,
        AutomationSubcommand::Run { rule, now, live } => run_rules(rule, now, live, output).await,
        AutomationSubcommand::History { limit } => list_history(limit, output).await,
    }
}

async fn list_rules(output: &OutputConfig) -> CliResult<()> {
    let config = load_config()?;
    config
        .validate()
        .map_err(|error| CliError::failure(error.to_string()))?;
    if output.is_json() {
        return emit_json_data(
            "automation.list",
            &AutomationListOutput {
                rules: &config.rules,
            },
            output,
        );
    }

    if config.rules.is_empty() {
        info_println!(output, "No automation rules configured.");
        return Ok(());
    }
    for rule in &config.rules {
        info_println!(
            output,
            "{}\t{}\t{}",
            rule.id,
            rule.trigger.kind(),
            rule.action.kind()
        );
    }
    Ok(())
}

async fn run_rules(
    rule_id: Option<String>,
    now: Option<String>,
    live: bool,
    output: &OutputConfig,
) -> CliResult<()> {
    let config = load_config()?;
    config
        .validate()
        .map_err(|error| CliError::failure(error.to_string()))?;
    let now = parse_now(now)?;
    let executor = if live {
        AutomationExecutor::live(current_working_dir()?)
    } else {
        AutomationExecutor::dry_run()
    };
    let results = if let Some(rule_id) = rule_id {
        let rule = config
            .rules
            .iter()
            .find(|rule| rule.id == rule_id)
            .ok_or_else(|| {
                CliError::failure(format!("automation rule `{rule_id}` was not found"))
            })?;
        vec![executor.execute_rule(rule, rule.trigger.clone()).await]
    } else {
        AutomationScheduler::new(config)
            .run_due_at(now, &executor)
            .await
            .map_err(|error| CliError::failure(error.to_string()))?
    };

    let db = open_repo_db().await?;
    for result in &results {
        AutomationHistory::append(&db, result)
            .await
            .map_err(|error| CliError::failure(error.to_string()))?;
    }

    if output.is_json() {
        return emit_json_data("automation.run", &AutomationRunOutput { results }, output);
    }
    for result in &results {
        info_println!(
            output,
            "{}\t{}\t{}",
            result.rule_id,
            result.status.as_str(),
            result.message
        );
    }
    Ok(())
}

async fn list_history(limit: u64, output: &OutputConfig) -> CliResult<()> {
    let db = open_repo_db().await?;
    let rows = AutomationHistory::list_recent(&db, limit)
        .await
        .map_err(|error| CliError::failure(error.to_string()))?;
    if output.is_json() {
        return emit_json_data(
            "automation.history",
            &AutomationRunOutput { results: rows },
            output,
        );
    }
    if rows.is_empty() {
        info_println!(output, "No automation history.");
        return Ok(());
    }
    for row in &rows {
        info_println!(
            output,
            "{}\t{}\t{}\t{}",
            row.finished_at.to_rfc3339(),
            row.rule_id,
            row.status.as_str(),
            row.message
        );
    }
    Ok(())
}

fn load_config() -> CliResult<AutomationConfig> {
    let working_dir = current_working_dir()?;
    AutomationConfig::load_from_working_dir(&working_dir)
        .map_err(|error| CliError::failure(error.to_string()))
}

fn current_working_dir() -> CliResult<PathBuf> {
    std::env::current_dir()
        .map_err(|error| CliError::io(format!("failed to resolve current directory: {error}")))
}

fn parse_now(now: Option<String>) -> CliResult<DateTime<Utc>> {
    match now {
        Some(raw) => DateTime::parse_from_rfc3339(&raw)
            .map(|value| value.with_timezone(&Utc))
            .map_err(|error| CliError::command_usage(format!("invalid --now timestamp: {error}"))),
        None => Ok(Utc::now()),
    }
}

async fn open_repo_db() -> CliResult<sea_orm::DatabaseConnection> {
    let db_path = try_get_storage_path(None)
        .map(|storage| storage.join(DATABASE))
        .map_err(|error| {
            CliError::repo_not_found()
                .with_hint(format!("failed to resolve repository storage: {error}"))
        })?;
    get_db_conn_instance_for_path(&db_path)
        .await
        .map_err(|error| {
            CliError::failure(format!(
                "failed to open repository database {}: {error}",
                db_path.display()
            ))
        })
}
