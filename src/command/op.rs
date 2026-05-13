//! Operation (op) command group for command-level operation history.

use clap::{Parser, Subcommand};
use serde::Serialize;

use crate::{
    internal::{
        config::ConfigKv,
        db::get_db_conn_instance,
        operation::{OperationLogListItem, OperationQueryPage, OperationService, OperationStatus},
    },
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
        util,
    },
};

#[derive(Parser, Debug)]
#[command(about = "View and restore command-level operation history")]
pub struct OpArgs {
    #[command(subcommand)]
    pub command: OpCommand,
}

#[derive(Subcommand, Debug)]
pub enum OpCommand {
    /// List operation history with pagination
    Log {
        /// Number of operations to show (default: 50)
        #[clap(short = 'n', long)]
        number: Option<u64>,

        /// Page number for pagination (default: 1)
        #[clap(long)]
        page: Option<u64>,

        /// Filter by command name (e.g., commit, merge)
        #[clap(long)]
        command: Option<String>,

        /// Show detailed metadata
        #[clap(long)]
        verbose: bool,
    },

    /// Show detailed operation information
    Show {
        /// Operation ID or index (e.g., @{0} for latest)
        #[arg(help = "Operation ID (UUID) or index like @{0}, @{1}")]
        op_ref: String,

        /// Show view snapshot details
        #[clap(long)]
        view: bool,
    },

    /// Restore repository to a previous operation's view state
    Restore {
        /// Operation ID or index to restore to
        #[arg(help = "Operation ID (UUID) or index like @{0}, @{1}")]
        op_ref: String,

        /// Force restoration even with uncommitted changes
        #[clap(long)]
        force: bool,

        /// Only show what would be done
        #[clap(long)]
        dry_run: bool,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "action")]
pub enum OpOutput {
    #[serde(rename = "log")]
    Log {
        operations: Vec<OpLogEntry>,
        page: u64,
        per_page: u64,
        total: u64,
    },
    #[serde(rename = "show")]
    Show {
        op_id: String,
        command_name: String,
        description: String,
        actor: String,
        status: String,
        start_ts: i64,
        end_ts: Option<i64>,
        view_id: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct OpLogEntry {
    pub op_id: String,
    pub command_name: String,
    pub description: String,
    pub actor: String,
    pub status: String,
    pub end_ts: Option<i64>,
}

pub async fn execute(args: OpArgs) {
    if let Err(err) = execute_safe(args, &OutputConfig::default()).await {
        err.print_stderr();
    }
}

pub async fn execute_safe(args: OpArgs, output: &OutputConfig) -> CliResult<()> {
    util::require_repo().map_err(|_| CliError::repo_not_found())?;

    match args.command {
        OpCommand::Log {
            number,
            page,
            command,
            verbose,
        } => handle_op_log(number, page, command, verbose, output).await,
        OpCommand::Show { op_ref, view } => handle_op_show(op_ref, view, output).await,
        OpCommand::Restore { .. } => Err(CliError::fatal("op restore is not implemented yet")),
    }
}

async fn handle_op_log(
    number: Option<u64>,
    page: Option<u64>,
    command_filter: Option<String>,
    verbose: bool,
    output: &OutputConfig,
) -> CliResult<()> {
    let db = get_db_conn_instance().await;
    let repo_id = current_repo_id().await?;
    let query_page = OperationQueryPage {
        page: page.unwrap_or(1),
        per_page: number.unwrap_or(OperationQueryPage::DEFAULT_PER_PAGE),
    };

    let result =
        OperationService::list_operations_by_repo_paginated_with_conn(&db, &repo_id, query_page)
            .await
            .map_err(|e| CliError::fatal(format!("failed to query operations: {e}")))?;

    let mut items = result.items;
    if let Some(filter) = command_filter
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        items.retain(|op| op.command_name == filter);
    }

    let entries: Vec<OpLogEntry> = items.iter().map(log_entry_from_item).collect();
    let op_output = OpOutput::Log {
        operations: entries.clone(),
        page: result.page,
        per_page: result.per_page,
        total: entries.len() as u64,
    };

    if output.is_json() {
        return emit_json_data("op", &op_output, output);
    }
    if output.quiet {
        return Ok(());
    }

    println!(
        "Operations (page {}, {} per page, shown {}):",
        result.page,
        result.per_page,
        entries.len()
    );
    println!();

    for (idx, op) in entries.iter().enumerate() {
        let short_id = &op.op_id[..8.min(op.op_id.len())];
        let timestamp = op
            .end_ts
            .map(format_timestamp)
            .unwrap_or_else(|| "running".to_string());

        if verbose {
            println!("{short_id}@{{{idx}}}");
            println!("  command: {}", op.command_name);
            println!("  description: {}", op.description);
            println!("  actor: {}", op.actor);
            println!("  status: {}", op.status);
            println!("  time: {timestamp}");
            println!();
        } else {
            println!(
                "{short_id}@{{{idx}}} {} {} - {} [{}]",
                op.command_name, op.description, timestamp, op.status
            );
        }
    }

    Ok(())
}

async fn handle_op_show(op_ref: String, show_view: bool, output: &OutputConfig) -> CliResult<()> {
    let db = get_db_conn_instance().await;
    let repo_id = current_repo_id().await?;
    let op_id = resolve_op_ref(&db, &repo_id, &op_ref).await?;

    let graph = OperationService::load_restore_view_by_operation_with_conn(&db, &op_id)
        .await
        .map_err(|e| CliError::fatal(format!("failed to load operation '{op_id}': {e}")))?
        .ok_or_else(|| {
            CliError::fatal(format!("operation '{op_id}' not found"))
                .with_stable_code(StableErrorCode::CliInvalidTarget)
                .with_hint("use 'libra op log' to list available operations")
        })?;
    let op_record = &graph.operation;
    let op_output = OpOutput::Show {
        op_id: op_record.op_id.clone(),
        command_name: op_record.command_name.clone(),
        description: op_record.description.clone(),
        actor: op_record.actor.clone(),
        status: status_label(op_record.status).to_string(),
        start_ts: op_record.start_ts,
        end_ts: op_record.end_ts,
        view_id: op_record.view_id.clone(),
    };

    if output.is_json() {
        return emit_json_data("op", &op_output, output);
    }

    let short_id = &op_record.op_id[..8.min(op_record.op_id.len())];
    println!("Operation: {short_id}");
    println!("Command: {}", op_record.command_name);
    println!("Description: {}", op_record.description);
    println!("Actor: {}", op_record.actor);
    println!("Status: {}", status_label(op_record.status));
    println!("Started: {}", format_timestamp(op_record.start_ts));
    if let Some(end_ts) = op_record.end_ts {
        println!("Completed: {}", format_timestamp(end_ts));
        println!(
            "Duration: {}ms",
            end_ts.saturating_sub(op_record.start_ts) * 1000
        );
    }
    println!("View ID: {}", op_record.view_id);

    if show_view {
        println!();
        println!("View Snapshot:");
        println!(
            "  HEAD: {} ({})",
            graph.view.head_target, graph.view.head_kind
        );
        println!("  Refs:");
        for ref_rec in &graph.refs {
            let ref_name = if let Some(remote) = &ref_rec.ref_remote {
                format!("{}/{}/{}", ref_rec.ref_kind, remote, ref_rec.ref_name)
            } else {
                format!("{} {}", ref_rec.ref_kind, ref_rec.ref_name)
            };
            println!(
                "    {}: {}",
                ref_name,
                &ref_rec.target_oid[..7.min(ref_rec.target_oid.len())]
            );
        }
    }

    Ok(())
}

async fn current_repo_id() -> CliResult<String> {
    ConfigKv::get("libra.repoid")
        .await
        .map_err(|e| CliError::fatal(format!("failed to read repository id: {e}")))?
        .map(|entry| entry.value)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            CliError::fatal("repository id is missing")
                .with_stable_code(StableErrorCode::RepoCorrupt)
                .with_hint("run 'libra init' to initialize repository metadata")
        })
}

async fn resolve_op_ref<C: sea_orm::ConnectionTrait>(
    db: &C,
    repo_id: &str,
    op_ref: &str,
) -> CliResult<String> {
    if let Some(index_str) = op_ref.strip_prefix("@{")
        && let Some(index_end) = index_str.find('}')
    {
        let index: usize = index_str[..index_end].parse().map_err(|_| {
            CliError::fatal(format!("invalid operation index: {op_ref}"))
                .with_stable_code(StableErrorCode::CliInvalidArguments)
        })?;
        let page = OperationQueryPage {
            page: 1,
            per_page: (index + 1) as u64,
        };
        let result = OperationService::list_operations_by_repo_paginated_with_conn(db, repo_id, page)
            .await
            .map_err(|e| CliError::fatal(format!("failed to query operations: {e}")))?;

        return result
            .items
            .into_iter()
            .nth(index)
            .map(|op| op.op_id)
            .ok_or_else(|| {
                CliError::fatal(format!("operation index {index} out of range"))
                    .with_stable_code(StableErrorCode::CliInvalidTarget)
                    .with_hint("use 'libra op log' to see available operations")
            });
    }

    Ok(op_ref.to_string())
}

fn log_entry_from_item(op: &OperationLogListItem) -> OpLogEntry {
    OpLogEntry {
        op_id: op.op_id.clone(),
        command_name: op.command_name.clone(),
        description: op.description.clone(),
        actor: op.actor.clone(),
        status: status_label(op.status).to_string(),
        end_ts: op.end_ts,
    }
}

fn status_label(status: OperationStatus) -> &'static str {
    match status {
        OperationStatus::Running => "running",
        OperationStatus::Succeeded => "succeeded",
        OperationStatus::Failed => "failed",
        OperationStatus::Canceled => "canceled",
    }
}

fn format_timestamp(ts: i64) -> String {
    use chrono::{TimeZone, Utc};
    Utc.timestamp_opt(ts, 0)
        .single()
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| ts.to_string())
}
