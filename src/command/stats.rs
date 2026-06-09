use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use clap::Parser;
use serde::Serialize;

use crate::utils::{
    error::{CliError, CliResult},
    output::OutputConfig,
};

const STATS_EXAMPLES: &str = "\
EXAMPLES:
    libra stats                     Count working-tree files grouped by extension
    libra stats --json              Structured JSON output for agents";

#[derive(Parser, Debug)]
#[command(after_help = STATS_EXAMPLES)]
pub struct StatsArgs {}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct StatsResult {
    pub total: usize,
    pub extensions: BTreeMap<String, usize>,
}

/// Execute `libra stats`.
///
/// This command is read-only. It scans the current working directory,
/// counts regular files, groups them by file extension, and ignores
/// `.libra/` and `target/`.
pub async fn execute_safe(_args: StatsArgs, output: &OutputConfig) -> CliResult<()> {
    let current_dir = std::env::current_dir()
        .map_err(|e| io_error("failed to get current working directory", e))?;

    let result = collect_stats(&current_dir)?;

    if output.json_format.is_some() {
        let json = serde_json::to_string_pretty(&result)
            .map_err(|e| CliError::fatal(format!("failed to serialize stats as JSON: {e}")))?;
        println!("{json}");
    } else if !output.quiet {
        print_text(&result);
    }

    Ok(())
}

pub fn collect_stats(root: &Path) -> CliResult<StatsResult> {
    let mut result = StatsResult {
        total: 0,
        extensions: BTreeMap::new(),
    };

    visit_dir(root, &mut result)?;

    Ok(result)
}

fn visit_dir(current: &Path, result: &mut StatsResult) -> CliResult<()> {
    for entry in fs::read_dir(current).map_err(|e| {
        io_error(
            format!("failed to read directory '{}'", current.display()),
            e,
        )
    })? {
        let entry = entry.map_err(|e| {
            io_error(
                format!("failed to read entry in directory '{}'", current.display()),
                e,
            )
        })?;

        let path = entry.path();

        if should_ignore(&path) {
            continue;
        }

        if path.is_dir() {
            visit_dir(&path, result)?;
        } else if path.is_file() {
            result.total += 1;

            let ext = path
                .extension()
                .and_then(|s| s.to_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("no_extension")
                .to_string();

            *result.extensions.entry(ext).or_insert(0) += 1;
        }
    }

    Ok(())
}

fn should_ignore(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name == ".libra" || name == "target")
        .unwrap_or(false)
}

fn print_text(result: &StatsResult) {
    println!("File statistics:");
    println!("total: {}", result.total);

    for (ext, count) in &result.extensions {
        println!("{}: {}", ext, count);
    }
}

fn io_error(context: impl Into<String>, error: std::io::Error) -> CliError {
    CliError::fatal(format!("{}: {}", context.into(), error))
}
