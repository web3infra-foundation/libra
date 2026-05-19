//! Repository database maintenance commands.

use clap::{Parser, Subcommand};
use serde::Serialize;

use crate::{
    info_println,
    internal::db::{self, SchemaCompatibility, SchemaUpgradeReport},
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
        util::{DATABASE, try_get_storage_path},
    },
};

#[derive(Parser, Debug)]
pub struct DbArgs {
    #[command(subcommand)]
    pub command: DbSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum DbSubcommand {
    /// Upgrade the repository database schema to this Libra version.
    Upgrade,
    /// Show the repository database schema version without modifying it.
    Status,
}

#[derive(Serialize)]
struct DbUpgradeOutput {
    previous_version: Option<i64>,
    current_version: Option<i64>,
    latest_version: Option<i64>,
    applied_versions: Vec<i64>,
    upgraded: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum DbSchemaState {
    Compatible,
    UpgradeRequired,
    UnsupportedFuture,
}

#[derive(Serialize)]
struct DbStatusOutput {
    state: DbSchemaState,
    current_version: Option<i64>,
    latest_version: Option<i64>,
}

pub async fn execute_safe(args: DbArgs, output: &OutputConfig) -> CliResult<()> {
    match args.command {
        DbSubcommand::Upgrade => upgrade(output).await,
        DbSubcommand::Status => status(output).await,
    }
}

fn repo_db_path() -> CliResult<std::path::PathBuf> {
    try_get_storage_path(None)
        .map(|storage| storage.join(DATABASE))
        .map_err(|error| {
            CliError::repo_not_found()
                .with_hint(format!("failed to resolve repository storage: {error}"))
        })
}

async fn upgrade(output: &OutputConfig) -> CliResult<()> {
    let db_path = repo_db_path()?;
    let report = db::upgrade_database_schema(&db_path)
        .await
        .map_err(|error| {
            CliError::fatal(format!(
                "failed to upgrade repository database '{}': {error}",
                db_path.display()
            ))
            .with_stable_code(StableErrorCode::RepoCorrupt)
        })?;
    render_upgrade(report, output)
}

fn render_upgrade(report: SchemaUpgradeReport, output: &OutputConfig) -> CliResult<()> {
    let upgraded = !report.applied_versions.is_empty();
    let data = DbUpgradeOutput {
        previous_version: report.previous_version,
        current_version: report.current_version,
        latest_version: report.latest_version,
        applied_versions: report.applied_versions,
        upgraded,
    };

    if output.is_json() {
        return emit_json_data("db.upgrade", &data, output);
    }

    if upgraded {
        let applied = data
            .applied_versions
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        info_println!(
            output,
            "Upgraded repository database schema from {} to {} (applied: {}).",
            format_version(data.previous_version),
            format_version(data.current_version),
            applied
        );
    } else {
        info_println!(
            output,
            "Repository database schema is up to date (version {}).",
            format_version(data.current_version)
        );
    }
    Ok(())
}

async fn status(output: &OutputConfig) -> CliResult<()> {
    let db_path = repo_db_path()?;
    let compatibility = db::inspect_database_schema(&db_path)
        .await
        .map_err(|error| {
            CliError::fatal(format!(
                "failed to inspect repository database '{}': {error}",
                db_path.display()
            ))
            .with_stable_code(StableErrorCode::RepoCorrupt)
        })?;

    let data = match compatibility {
        SchemaCompatibility::Compatible {
            current_version,
            latest_version,
        } => DbStatusOutput {
            state: DbSchemaState::Compatible,
            current_version,
            latest_version,
        },
        SchemaCompatibility::UpgradeRequired {
            current_version,
            latest_version,
        } => DbStatusOutput {
            state: DbSchemaState::UpgradeRequired,
            current_version,
            latest_version: Some(latest_version),
        },
        SchemaCompatibility::UnsupportedFuture {
            current_version,
            latest_version,
        } => DbStatusOutput {
            state: DbSchemaState::UnsupportedFuture,
            current_version: Some(current_version),
            latest_version,
        },
    };

    if output.is_json() {
        return emit_json_data("db.status", &data, output);
    }

    match data.state {
        DbSchemaState::Compatible => info_println!(
            output,
            "Repository database schema is compatible (current: {}, latest: {}).",
            format_version(data.current_version),
            format_version(data.latest_version)
        ),
        DbSchemaState::UpgradeRequired => info_println!(
            output,
            "Repository database schema requires upgrade (current: {}, latest: {}). Run 'libra db upgrade'.",
            format_version(data.current_version),
            format_version(data.latest_version)
        ),
        DbSchemaState::UnsupportedFuture => info_println!(
            output,
            "Repository database schema is newer than this Libra binary (current: {}, latest supported: {}).",
            format_version(data.current_version),
            format_version(data.latest_version)
        ),
    }
    Ok(())
}

fn format_version(version: Option<i64>) -> String {
    version
        .map(|value| value.to_string())
        .unwrap_or_else(|| "none".to_string())
}
