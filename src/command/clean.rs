//! Implements `clean` to remove untracked files from the working tree.

use std::fs;

use clap::Parser;
use git_internal::internal::index::Index;

use crate::utils::{
    error::{CliError, CliResult},
    path, util, worktree,
};

#[derive(Parser, Debug, Clone)]
pub struct CleanArgs {
    /// Show what would be removed without actually removing
    #[clap(short = 'n', long)]
    pub dry_run: bool,
    /// Force removal of untracked files
    #[clap(short, long)]
    pub force: bool,
}

pub async fn execute(args: CleanArgs) {
    if let Err(err) = execute_safe(args).await {
        eprintln!("{}", err.render());
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting. Removes untracked files from the working tree.
pub async fn execute_safe(args: CleanArgs) -> CliResult<()> {
    util::require_repo().map_err(|_| CliError::repo_not_found())?;
    run_clean(args)
}

fn run_clean(args: CleanArgs) -> CliResult<()> {
    if !args.force && !args.dry_run {
        return Err(CliError::fatal(
            "clean requires -f or -n (use -f to remove files, -n to dry-run)",
        )
        .with_hint("use 'libra clean -n' to preview removals.")
        .with_hint("use 'libra clean -f' to remove untracked files."));
    }

    let index_path = path::index();
    let index = match Index::load(&index_path) {
        Ok(index) => index,
        Err(e) => {
            if !index_path.exists() {
                Index::new()
            } else {
                return Err(CliError::fatal(format!("failed to load index: {e}")));
            }
        }
    };
    let untracked =
        worktree::untracked_workdir_paths(&index).map_err(|e| CliError::fatal(e.to_string()))?;

    if untracked.is_empty() {
        return Ok(());
    }

    if args.dry_run {
        for path in untracked {
            println!("Would remove {}", path.display());
        }
        return Ok(());
    }

    let workdir = fs::canonicalize(util::working_dir())
        .map_err(|e| CliError::fatal(format!("failed to resolve working directory: {e}")))?;
    for path in untracked {
        let abs_path = util::workdir_to_absolute(&path);
        if abs_path.exists() {
            let resolved = fs::canonicalize(&abs_path).map_err(|e| {
                CliError::fatal(format!(
                    "failed to resolve path {}: {}",
                    abs_path.display(),
                    e
                ))
            })?;
            if !resolved.starts_with(&workdir) {
                return Err(CliError::fatal(format!(
                    "refusing to remove path outside workdir: {}",
                    abs_path.display()
                )));
            }
            fs::remove_file(&abs_path).map_err(|e| {
                CliError::fatal(format!("failed to remove {}: {e}", abs_path.display()))
            })?;
        }
    }
    Ok(())
}
