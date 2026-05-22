//! `libra usage` command for provider/model usage aggregates.

use std::{fs, path::Path};

use chrono::{DateTime, NaiveDate, Utc};
use clap::{Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};

use crate::{
    info_println,
    internal::{
        ai::usage::{UsageQuery, UsageQueryFilter},
        db::get_db_conn_instance_for_path,
    },
    utils::{
        error::{CliError, CliResult},
        output::{OutputConfig, emit_json_data},
        util::{DATABASE, try_get_storage_path},
    },
};

#[derive(Parser, Debug)]
pub struct UsageArgs {
    #[command(subcommand)]
    pub command: UsageSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum UsageSubcommand {
    /// Report usage aggregates.
    Report {
        /// Aggregation dimension. Currently only `model` is supported.
        #[arg(long, default_value = "model", value_parser = ["model"])]
        by: String,
        /// Start time filter. Accepts RFC3339, YYYY-MM-DD, or relative values like 24h/7d.
        #[arg(long)]
        since: Option<String>,
        /// End time filter. Accepts RFC3339, YYYY-MM-DD, or relative values like 1h.
        #[arg(long)]
        until: Option<String>,
        /// Restrict report to a session id.
        #[arg(long)]
        session: Option<String>,
        /// Restrict report to a canonical thread id.
        #[arg(long)]
        thread: Option<String>,
        /// Include failed provider requests in request counts and wall-clock totals.
        #[arg(long)]
        include_failed: bool,
        /// Output format for this report. Global --json also forces JSON.
        #[arg(long, value_enum, default_value_t = UsageReportFormat::Human)]
        format: UsageReportFormat,
    },
    /// Delete usage rows older than the retention window.
    Prune {
        /// Retention window in days. Rows older than this are deleted.
        #[arg(long, value_parser = parse_positive_retention_days)]
        retention_days: Option<u32>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum UsageReportFormat {
    Human,
    Json,
    Csv,
}

#[derive(Serialize)]
struct UsageReportOutput {
    by: String,
    filter: UsageReportFilterOutput,
    rows: Vec<crate::internal::ai::usage::UsageAggregate>,
}

#[derive(Serialize)]
struct UsageReportFilterOutput {
    since: Option<String>,
    until: Option<String>,
    session: Option<String>,
    thread: Option<String>,
    include_failed: bool,
}

pub async fn execute_safe(args: UsageArgs, output: &OutputConfig) -> CliResult<()> {
    match args.command {
        UsageSubcommand::Report {
            by,
            since,
            until,
            session,
            thread,
            include_failed,
            format,
        } => {
            report_usage(
                UsageReportOptions {
                    by,
                    since,
                    until,
                    session,
                    thread,
                    include_failed,
                    format,
                },
                output,
            )
            .await
        }
        UsageSubcommand::Prune { retention_days } => prune_usage(retention_days, output).await,
    }
}

struct UsageReportOptions {
    by: String,
    since: Option<String>,
    until: Option<String>,
    session: Option<String>,
    thread: Option<String>,
    include_failed: bool,
    format: UsageReportFormat,
}

async fn report_usage(options: UsageReportOptions, output: &OutputConfig) -> CliResult<()> {
    let filter_output = UsageReportFilterOutput {
        since: options
            .since
            .as_deref()
            .map(|value| parse_usage_time_filter(value, "--since"))
            .transpose()?,
        until: options
            .until
            .as_deref()
            .map(|value| parse_usage_time_filter(value, "--until"))
            .transpose()?,
        session: options.session,
        thread: options.thread,
        include_failed: options.include_failed,
    };
    let filter = UsageQueryFilter {
        since: filter_output.since.clone(),
        until: filter_output.until.clone(),
        session_id: filter_output.session.clone(),
        thread_id: filter_output.thread.clone(),
        include_failed: filter_output.include_failed,
    };
    let db = open_repo_db().await?;
    let rows = UsageQuery::new(db)
        .aggregate_by_model_filtered(&filter)
        .await
        .map_err(|error| CliError::failure(format!("failed to query usage stats: {error}")))?;
    if output.is_json() || options.format == UsageReportFormat::Json {
        return emit_json_data(
            "usage.report",
            &UsageReportOutput {
                by: options.by,
                filter: filter_output,
                rows,
            },
            output,
        );
    }
    if options.format == UsageReportFormat::Csv {
        return emit_usage_csv(&rows, output);
    }
    if rows.is_empty() {
        info_println!(output, "No usage stats recorded.");
        return Ok(());
    }
    for row in &rows {
        let total = if row.total_tokens > 0 {
            row.total_tokens
        } else {
            row.prompt_tokens.saturating_add(row.completion_tokens)
        };
        let cost = usage_human_cost(row);
        info_println!(
            output,
            "{}\t{}\trequests={}\tfailed={}\ttokens={}\tcached={}\treasoning={}\ttool_calls={}\twall_ms={}{}",
            row.provider,
            row.model,
            row.request_count,
            row.failed_count,
            total,
            row.cached_tokens,
            row.reasoning_tokens,
            row.tool_call_count,
            row.wall_clock_ms,
            cost
        );
    }
    Ok(())
}

/// Session-bootstrap auto-prune (v0.17.791): if `[usage]
/// retention_days = N` is configured in the repo's
/// `config.toml`, prune usage rows older than `N` days. Called
/// once per `libra code` session start so long-running operators
/// don't accumulate unbounded usage history.
///
/// Soft-failure: every error path emits `tracing::warn!` and
/// returns without bubbling — a malformed config or DB error
/// must not block session startup. Returns the number of rows
/// pruned (or 0 if no retention is configured / on any error).
pub async fn auto_prune_at_session_start(storage_root: &Path) -> u64 {
    let config_path = storage_root.join("config.toml");
    let retention_days = match read_usage_retention_days_config(&config_path) {
        Ok(Some(days)) => days,
        Ok(None) => return 0,
        Err(err) => {
            tracing::warn!(
                %err,
                path = %config_path.display(),
                "failed to read usage retention config at session bootstrap; \
                 skipping auto-prune",
            );
            return 0;
        }
    };
    let conn = match open_repo_db_at(storage_root).await {
        Ok(conn) => conn,
        Err(err) => {
            tracing::warn!(
                %err,
                "failed to open repo DB for usage auto-prune; skipping",
            );
            return 0;
        }
    };
    let cutoff = Utc::now() - chrono::Duration::days(i64::from(retention_days));
    match crate::internal::ai::usage::UsageRecorder::new(conn)
        .prune_before(&cutoff.to_rfc3339())
        .await
    {
        Ok(deleted) => {
            if deleted > 0 {
                tracing::info!(
                    deleted,
                    retention_days,
                    cutoff = %cutoff.to_rfc3339(),
                    "session-bootstrap pruned old usage rows",
                );
            }
            deleted
        }
        Err(err) => {
            tracing::warn!(%err, "usage auto-prune failed; skipping");
            0
        }
    }
}

async fn prune_usage(retention_days: Option<u32>, output: &OutputConfig) -> CliResult<()> {
    let retention_days = resolve_usage_retention_days(retention_days)?;
    let db = open_repo_db().await?;
    let cutoff = Utc::now() - chrono::Duration::days(i64::from(retention_days));
    let deleted = crate::internal::ai::usage::UsageRecorder::new(db)
        .prune_before(&cutoff.to_rfc3339())
        .await
        .map_err(|error| CliError::failure(format!("failed to prune usage stats: {error}")))?;
    if output.is_json() {
        return emit_json_data(
            "usage.prune",
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
        "Deleted {deleted} usage row(s) older than {} day(s).",
        retention_days
    );
    Ok(())
}

fn emit_usage_csv(
    rows: &[crate::internal::ai::usage::UsageAggregate],
    output: &OutputConfig,
) -> CliResult<()> {
    info_println!(
        output,
        "provider,model,requests,failed,prompt_tokens,completion_tokens,cached_tokens,reasoning_tokens,total_tokens,tool_calls,wall_ms,cost_usd,cost_estimate_micro_dollars"
    );
    for row in rows {
        let total = if row.total_tokens > 0 {
            row.total_tokens
        } else {
            row.prompt_tokens.saturating_add(row.completion_tokens)
        };
        let cost = row
            .cost_usd
            .map(|cost| format!("{cost:.6}"))
            .unwrap_or_default();
        let cost_estimate = row
            .cost_estimate_micro_dollars
            .map(|cost| cost.to_string())
            .unwrap_or_default();
        info_println!(
            output,
            "{},{},{},{},{},{},{},{},{},{},{},{},{}",
            csv_escape(&row.provider),
            csv_escape(&row.model),
            row.request_count,
            row.failed_count,
            row.prompt_tokens,
            row.completion_tokens,
            row.cached_tokens,
            row.reasoning_tokens,
            total,
            row.tool_call_count,
            row.wall_clock_ms,
            cost,
            cost_estimate
        );
    }
    Ok(())
}

const DEFAULT_USAGE_RETENTION_DAYS: u32 = 90;

#[derive(Debug, Default, Deserialize)]
struct UsageRetentionProjectConfig {
    #[serde(default)]
    usage: UsageRetentionConfig,
}

#[derive(Debug, Default, Deserialize)]
struct UsageRetentionConfig {
    retention_days: Option<u32>,
}

fn parse_positive_retention_days(raw: &str) -> Result<u32, String> {
    let days = raw
        .parse::<u32>()
        .map_err(|_| format!("'{raw}' is not a valid day count"))?;
    validate_positive_retention_days(days, "--retention-days").map_err(|error| error.to_string())
}

fn resolve_usage_retention_days(cli_retention_days: Option<u32>) -> CliResult<u32> {
    if let Some(days) = cli_retention_days {
        return validate_positive_retention_days(days, "--retention-days");
    }

    let storage = try_get_storage_path(None).map_err(|error| {
        CliError::repo_not_found()
            .with_hint(format!("failed to resolve repository storage: {error}"))
    })?;
    let Some(days) = read_usage_retention_days_config(&storage.join("config.toml"))? else {
        return Ok(DEFAULT_USAGE_RETENTION_DAYS);
    };
    validate_positive_retention_days(days, "usage.retention_days")
}

fn read_usage_retention_days_config(path: &Path) -> CliResult<Option<u32>> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(CliError::io(format!(
                "failed to read usage retention config '{}': {error}",
                path.display()
            )));
        }
    };
    usage_retention_days_from_config_toml(&contents).map_err(|error| {
        CliError::failure(format!(
            "failed to parse usage retention config '{}': {error}",
            path.display()
        ))
    })
}

fn usage_retention_days_from_config_toml(contents: &str) -> Result<Option<u32>, toml::de::Error> {
    let config: UsageRetentionProjectConfig = toml::from_str(contents)?;
    Ok(config.usage.retention_days)
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

fn usage_human_cost(row: &crate::internal::ai::usage::UsageAggregate) -> String {
    if let Some(cost) = row.cost_usd {
        return format!(" ${cost:.4}");
    }
    row.cost_estimate_micro_dollars
        .map(|micro| format!(" ~${:.4}", micro as f64 / 1_000_000.0))
        .unwrap_or_default()
}

fn csv_escape(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn parse_usage_time_filter(value: &str, flag: &str) -> CliResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(CliError::command_usage(format!("{flag} cannot be empty")));
    }
    if let Some(relative) = parse_relative_usage_time(trimmed) {
        return Ok((Utc::now() - relative).to_rfc3339());
    }
    if let Ok(datetime) = DateTime::parse_from_rfc3339(trimmed) {
        return Ok(datetime.with_timezone(&Utc).to_rfc3339());
    }
    if let Ok(date) = NaiveDate::parse_from_str(trimmed, "%Y-%m-%d")
        && let Some(datetime) = date.and_hms_opt(0, 0, 0)
    {
        return Ok(datetime.and_utc().to_rfc3339());
    }
    Err(CliError::command_usage(format!(
        "invalid {flag} value '{value}'; expected RFC3339, YYYY-MM-DD, or relative duration like 24h/7d"
    )))
}

fn parse_relative_usage_time(value: &str) -> Option<chrono::Duration> {
    let (number, suffix) = value.split_at(value.len().checked_sub(1)?);
    let amount = number.parse::<i64>().ok()?;
    if amount < 0 {
        return None;
    }
    match suffix {
        "m" => Some(chrono::Duration::minutes(amount)),
        "h" => Some(chrono::Duration::hours(amount)),
        "d" => Some(chrono::Duration::days(amount)),
        _ => None,
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

/// Variant of [`open_repo_db`] that takes an explicit
/// `storage_root` instead of resolving via the working
/// directory. Used by the v0.17.791 session-bootstrap
/// auto-prune path which already has the resolved storage
/// root in scope.
async fn open_repo_db_at(storage_root: &Path) -> anyhow::Result<sea_orm::DatabaseConnection> {
    let db_path = storage_root.join(DATABASE);
    get_db_conn_instance_for_path(&db_path)
        .await
        .map_err(|err| anyhow::anyhow!("failed to open repository database {}: {err}", db_path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::usage::UsageAggregate;

    #[test]
    fn parses_usage_report_relative_duration() {
        let parsed = parse_usage_time_filter("24h", "--since").expect("parse relative duration");
        assert!(DateTime::parse_from_rfc3339(&parsed).is_ok());
    }

    #[test]
    fn parses_usage_report_date() {
        let parsed = parse_usage_time_filter("2026-05-03", "--since").expect("parse date");
        assert_eq!(parsed, "2026-05-03T00:00:00+00:00");
    }

    #[test]
    fn rejects_invalid_usage_report_time() {
        let error = parse_usage_time_filter("soonish", "--since").unwrap_err();
        assert!(error.to_string().contains("invalid --since value"));
    }

    #[test]
    fn parses_usage_retention_days_config() {
        let days = usage_retention_days_from_config_toml(
            r#"
            [usage]
            retention_days = 14
            "#,
        )
        .expect("usage retention config should parse");

        assert_eq!(days, Some(14));
    }

    #[test]
    fn explicit_retention_days_must_be_positive() {
        let error = parse_positive_retention_days("0").unwrap_err();
        assert!(error.contains("--retention-days must be greater than 0"));
    }

    #[test]
    fn invalid_usage_retention_config_reports_toml_error() {
        let error = usage_retention_days_from_config_toml("[usage]\nretention_days = \"soon\"")
            .unwrap_err();
        assert!(error.to_string().contains("retention_days"));
    }

    #[test]
    fn human_usage_cost_marks_estimates() {
        let actual = UsageAggregate {
            agent_name: None,
            provider: "openai".to_string(),
            model: "gpt-4o-mini".to_string(),
            request_count: 1,
            failed_count: 0,
            prompt_tokens: 1,
            completion_tokens: 1,
            cached_tokens: 0,
            reasoning_tokens: 0,
            total_tokens: 2,
            tool_call_count: 0,
            wall_clock_ms: 1,
            cost_usd: Some(0.25),
            cost_estimate_micro_dollars: Some(250_000),
        };
        assert_eq!(usage_human_cost(&actual), " $0.2500");

        let estimated = UsageAggregate {
            cost_usd: None,
            cost_estimate_micro_dollars: Some(1_350_000),
            ..actual
        };
        assert_eq!(usage_human_cost(&estimated), " ~$1.3500");
    }
}
