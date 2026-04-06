//! Implements `clean` to remove untracked files from the working tree.

use std::fs;

use clap::Parser;
use git_internal::internal::index::Index;
use serde::Serialize;

use crate::utils::{
    error::{CliError, CliResult, StableErrorCode},
    output::{OutputConfig, emit_json_data},
    path, util, worktree,
};

const CLEAN_EXAMPLES: &str = "\
EXAMPLES:
  libra clean -n
  libra clean -f
  libra clean -n --json
";

#[derive(Parser, Debug, Clone)]
#[command(after_help = CLEAN_EXAMPLES)]
pub struct CleanArgs {
    /// Show what would be removed without actually removing
    #[clap(short = 'n', long)]
    pub dry_run: bool,
    /// Force removal of untracked files
    #[clap(short, long)]
    pub force: bool,
}

#[derive(Debug, Clone, Serialize)]
struct CleanOutput {
    dry_run: bool,
    removed: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
enum CleanError {
    #[error("clean requires -f or -n (use -f to remove files, -n to dry-run)")]
    MissingMode,
    #[error("failed to load index: {0}")]
    LoadIndex(String),
    #[error("{0}")]
    ScanUntracked(String),
    #[error("failed to resolve working directory: {0}")]
    ResolveWorkdir(String),
    #[error("failed to resolve path {path}: {detail}")]
    ResolvePath { path: String, detail: String },
    #[error("refusing to remove path outside workdir: {0}")]
    OutsideWorkdir(String),
    #[error("failed to remove {path}: {detail}")]
    RemoveFile { path: String, detail: String },
}

pub async fn execute(args: CleanArgs) {
    if let Err(err) = execute_safe(args, &OutputConfig::default()).await {
        err.print_stderr();
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting. Removes untracked files from the working tree.
pub async fn execute_safe(args: CleanArgs, output: &OutputConfig) -> CliResult<()> {
    util::require_repo().map_err(|_| CliError::repo_not_found())?;
    let clean_output = run_clean(args).map_err(clean_cli_error)?;

    if output.is_json() {
        emit_json_data("clean", &clean_output, output)?;
    } else if !output.quiet {
        for path in &clean_output.removed {
            if clean_output.dry_run {
                println!("Would remove {path}");
            } else {
                println!("Removing {path}");
            }
        }
    }

    Ok(())
}

fn run_clean(args: CleanArgs) -> Result<CleanOutput, CleanError> {
    if !args.force && !args.dry_run {
        return Err(CleanError::MissingMode);
    }

    let index_path = path::index();
    let index = match Index::load(&index_path) {
        Ok(index) => index,
        Err(e) => {
            if !index_path.exists() {
                Index::new()
            } else {
                return Err(CleanError::LoadIndex(e.to_string()));
            }
        }
    };
    let untracked = worktree::untracked_workdir_paths(&index)
        .map_err(|e| CleanError::ScanUntracked(e.to_string()))?;

    if untracked.is_empty() {
        return Ok(CleanOutput {
            dry_run: args.dry_run,
            removed: Vec::new(),
        });
    }

    if args.dry_run {
        return Ok(CleanOutput {
            dry_run: true,
            removed: untracked
                .iter()
                .map(|path| path.display().to_string())
                .collect(),
        });
    }

    let workdir = fs::canonicalize(util::working_dir())
        .map_err(|e| CleanError::ResolveWorkdir(e.to_string()))?;
    let mut removed = Vec::new();
    for path in untracked {
        let abs_path = util::workdir_to_absolute(&path);
        if abs_path.exists() {
            let resolved = fs::canonicalize(&abs_path).map_err(|e| CleanError::ResolvePath {
                path: abs_path.display().to_string(),
                detail: e.to_string(),
            })?;
            if !resolved.starts_with(&workdir) {
                return Err(CleanError::OutsideWorkdir(abs_path.display().to_string()));
            }
            fs::remove_file(&abs_path).map_err(|e| CleanError::RemoveFile {
                path: abs_path.display().to_string(),
                detail: e.to_string(),
            })?;
            removed.push(path.display().to_string());
        }
    }
    Ok(CleanOutput {
        dry_run: false,
        removed,
    })
}

fn clean_cli_error(error: CleanError) -> CliError {
    match error {
        CleanError::MissingMode => CliError::fatal(error.to_string())
            .with_stable_code(StableErrorCode::CliInvalidArguments)
            .with_hint("use 'libra clean -n' to preview removals.")
            .with_hint("use 'libra clean -f' to remove untracked files."),
        CleanError::LoadIndex(message) => {
            CliError::fatal(format!("failed to load index: {message}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        }
        CleanError::ScanUntracked(message) | CleanError::ResolveWorkdir(message) => {
            CliError::fatal(message).with_stable_code(StableErrorCode::IoReadFailed)
        }
        CleanError::ResolvePath { path, detail } => {
            CliError::fatal(format!("failed to resolve path {path}: {detail}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        }
        CleanError::OutsideWorkdir(path) => {
            CliError::fatal(format!("refusing to remove path outside workdir: {path}"))
                .with_stable_code(StableErrorCode::ConflictOperationBlocked)
        }
        CleanError::RemoveFile { path, detail } => {
            CliError::fatal(format!("failed to remove {path}: {detail}"))
                .with_stable_code(StableErrorCode::IoWriteFailed)
        }
    }
}
