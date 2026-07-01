//! `libra logfile info` — report the resolved tracing log-file configuration
//! (path, rolling strategy, filter, current size), mirroring Lore's `logfile`
//! command. Pure inspection of the process environment (`LIBRA_LOG_FILE`,
//! `LIBRA_LOG_ROTATION`, `LIBRA_LOG`/`RUST_LOG`); needs no repository (lore.md §0.7).

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use serde::Serialize;

use crate::utils::{
    error::CliResult,
    log_config::{LogConfig, LogRotation, resolve_log_config},
    output::{OutputConfig, emit_json_data},
};

pub const LOGFILE_EXAMPLES: &str = "\
EXAMPLES:
    libra logfile info                     Show the resolved log-file configuration
    LIBRA_LOG_FILE=/var/log/libra.log LIBRA_LOG_ROTATION=daily libra logfile info
    libra --json logfile info              Structured { enabled, file, rotation, filter, size_bytes, file_count }";

/// Inspect the tracing log-file configuration.
#[derive(Parser, Debug)]
#[command(after_help = LOGFILE_EXAMPLES)]
pub struct LogfileArgs {
    #[command(subcommand)]
    pub command: LogfileCommand,
}

#[derive(Subcommand, Debug)]
pub enum LogfileCommand {
    /// Show the resolved log-file configuration (path, rotation, filter, size).
    Info,
}

#[derive(Debug, Serialize)]
struct LogfileInfo {
    /// Whether tracing is enabled (a filter directive was resolved).
    enabled: bool,
    /// The `LIBRA_LOG_FILE` path, or `null` when logging to stderr.
    file: Option<String>,
    /// Rolling strategy: `never` / `minutely` / `hourly` / `daily`.
    rotation: String,
    /// The resolved env-filter directive, if any.
    filter: Option<String>,
    /// Total size in bytes of the log file(s) on disk — the single file for
    /// `never`, or the sum of all rolled `<file>.<suffix>` files otherwise.
    /// `null` when no log file exists yet.
    size_bytes: Option<u64>,
    /// Number of log files on disk (1 for `never`; the rolled-file count
    /// otherwise). `0` when none exist yet.
    file_count: usize,
}

/// Total size and count of the on-disk log file(s) for `config`: the exact file
/// under `never`, or every rolled `<name>.<suffix>` sibling under a rolling
/// strategy (`tracing-appender` writes the active file with a date suffix, so a
/// bare stat of the base path would miss it).
fn log_files_on_disk(config: &LogConfig) -> (Option<u64>, usize) {
    let Some(path) = config.file.as_ref() else {
        return (None, 0);
    };

    if config.rotation == LogRotation::Never {
        return match std::fs::metadata(path) {
            Ok(meta) => (Some(meta.len()), 1),
            Err(_) => (None, 0),
        };
    }

    // Rolling: sum all `<dir>/<name>.*` files.
    let directory = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return (None, 0);
    };
    let prefix = format!("{file_name}.");

    let Ok(entries) = std::fs::read_dir(&directory) else {
        return (None, 0);
    };
    let mut total = 0u64;
    let mut count = 0usize;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with(&prefix)
            && let Ok(meta) = entry.metadata()
            && meta.is_file()
        {
            total += meta.len();
            count += 1;
        }
    }
    if count == 0 {
        (None, 0)
    } else {
        (Some(total), count)
    }
}

pub async fn execute_safe(args: LogfileArgs, output: &OutputConfig) -> CliResult<()> {
    match args.command {
        LogfileCommand::Info => info(output),
    }
}

fn info(output: &OutputConfig) -> CliResult<()> {
    let config = resolve_log_config();
    let (size_bytes, file_count) = log_files_on_disk(&config);

    let report = LogfileInfo {
        enabled: config.is_enabled(),
        file: config.file.as_ref().map(|path| path.display().to_string()),
        rotation: config.rotation.as_str().to_string(),
        filter: config.filter.clone(),
        size_bytes,
        file_count,
    };

    if output.is_json() {
        return emit_json_data("logfile", &report, output);
    }

    println!(
        "logging: {}",
        if report.enabled {
            "enabled"
        } else {
            "disabled"
        }
    );
    match &report.file {
        Some(file) => {
            println!("file:     {file}");
            println!("rotation: {}", report.rotation);
            match report.size_bytes {
                Some(size) => {
                    println!(
                        "size:     {size} bytes across {} file(s)",
                        report.file_count
                    )
                }
                None => println!("size:     (no log file on disk yet)"),
            }
        }
        None => println!("file:     (stderr — set LIBRA_LOG_FILE to log to a file)"),
    }
    if let Some(filter) = &report.filter {
        println!("filter:   {filter}");
    }
    Ok(())
}
