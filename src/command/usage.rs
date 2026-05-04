//! `libra usage` command for provider/model usage aggregates.

use chrono::{DateTime, NaiveDate, Utc};
use clap::{Parser, Subcommand, ValueEnum};
use serde::Serialize;

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
        #[arg(long, default_value_t = 90)]
        retention_days: u32,
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
        let cost = row
            .cost_usd
            .map(|cost| format!(" ${cost:.4}"))
            .unwrap_or_default();
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

async fn prune_usage(retention_days: u32, output: &OutputConfig) -> CliResult<()> {
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
        "provider,model,requests,failed,prompt_tokens,completion_tokens,cached_tokens,reasoning_tokens,total_tokens,tool_calls,wall_ms,cost_usd"
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
        info_println!(
            output,
            "{},{},{},{},{},{},{},{},{},{},{},{}",
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
            cost
        );
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
