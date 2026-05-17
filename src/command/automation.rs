//! `libra automation` command surface for CEX-15.

use std::{
    fs,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};

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

const DEFAULT_AUTOMATION_RETENTION_DAYS: u32 = 90;

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
    /// Delete automation history rows older than the retention window.
    Prune {
        /// Retention window in days. Rows whose `finished_at` is older than this
        /// are deleted. Defaults to the `automation.retention_days` key in
        /// `<repo>/.libra/config.toml` when present, otherwise 90 days.
        #[arg(long, value_parser = parse_positive_retention_days)]
        retention_days: Option<u32>,
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
        AutomationSubcommand::Prune { retention_days } => {
            prune_history(retention_days, output).await
        }
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

async fn prune_history(retention_days: Option<u32>, output: &OutputConfig) -> CliResult<()> {
    let retention_days = resolve_automation_retention_days(retention_days)?;
    let db = open_repo_db().await?;
    let cutoff = Utc::now() - chrono::Duration::days(i64::from(retention_days));
    let deleted = AutomationHistory::prune_before(&db, &cutoff.to_rfc3339())
        .await
        .map_err(|error| {
            CliError::failure(format!("failed to prune automation history: {error}"))
        })?;
    if output.is_json() {
        return emit_json_data(
            "automation.prune",
            &serde_json::json!({
                "retention_days": retention_days,
                "cutoff": cutoff.to_rfc3339(),
                "deleted": deleted,
            }),
            output,
        );
    }
    info_println!(
        output,
        "Deleted {deleted} automation history row(s) older than {} day(s).",
        retention_days
    );
    Ok(())
}

#[derive(Debug, Default, Deserialize)]
struct AutomationRetentionProjectConfig {
    #[serde(default)]
    automation: AutomationRetentionConfig,
}

#[derive(Debug, Default, Deserialize)]
struct AutomationRetentionConfig {
    retention_days: Option<u32>,
}

fn parse_positive_retention_days(raw: &str) -> Result<u32, String> {
    let days = raw
        .parse::<u32>()
        .map_err(|_| format!("'{raw}' is not a valid day count"))?;
    validate_positive_retention_days(days, "--retention-days").map_err(|error| error.to_string())
}

fn resolve_automation_retention_days(cli_retention_days: Option<u32>) -> CliResult<u32> {
    if let Some(days) = cli_retention_days {
        return validate_positive_retention_days(days, "--retention-days");
    }

    let storage = try_get_storage_path(None).map_err(|error| {
        CliError::repo_not_found()
            .with_hint(format!("failed to resolve repository storage: {error}"))
    })?;
    let Some(days) = read_automation_retention_days_config(&storage.join("config.toml"))? else {
        return Ok(DEFAULT_AUTOMATION_RETENTION_DAYS);
    };
    validate_positive_retention_days(days, "automation.retention_days")
}

fn read_automation_retention_days_config(path: &Path) -> CliResult<Option<u32>> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(CliError::io(format!(
                "failed to read automation retention config '{}': {error}",
                path.display()
            )));
        }
    };
    automation_retention_days_from_config_toml(&contents).map_err(|error| {
        CliError::failure(format!(
            "failed to parse automation retention config '{}': {error}",
            path.display()
        ))
    })
}

fn automation_retention_days_from_config_toml(
    contents: &str,
) -> Result<Option<u32>, toml::de::Error> {
    let config: AutomationRetentionProjectConfig = toml::from_str(contents)?;
    Ok(config.automation.retention_days)
}

fn validate_positive_retention_days(days: u32, source: &str) -> CliResult<u32> {
    if days == 0 {
        Err(CliError::command_usage(format!(
            "{source} must be greater than 0"
        )))
    } else {
        Ok(days)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_zero_retention_days() {
        let err = validate_positive_retention_days(0, "--retention-days").unwrap_err();
        assert!(err.message().contains("must be greater than 0"));
    }

    #[test]
    fn parse_positive_retention_days_accepts_positive() {
        assert_eq!(parse_positive_retention_days("7").unwrap(), 7);
    }

    #[test]
    fn parse_positive_retention_days_rejects_zero() {
        let err = parse_positive_retention_days("0").unwrap_err();
        assert!(err.contains("must be greater than 0"));
    }

    #[test]
    fn parse_positive_retention_days_rejects_non_numeric() {
        let err = parse_positive_retention_days("forever").unwrap_err();
        assert!(err.contains("not a valid day count"));
    }

    #[test]
    fn config_toml_extracts_automation_retention_days() {
        let toml_src = r#"
[automation]
retention_days = 30
"#;
        let days = automation_retention_days_from_config_toml(toml_src).unwrap();
        assert_eq!(days, Some(30));
    }

    #[test]
    fn config_toml_missing_section_returns_none() {
        let toml_src = "";
        let days = automation_retention_days_from_config_toml(toml_src).unwrap();
        assert_eq!(days, None);
    }
}
