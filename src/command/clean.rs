//! Implements `clean` to remove untracked files from the working tree.

use std::fs;

use clap::Parser;
use colored::Colorize;
use git_internal::internal::index::Index;

use crate::utils::{path, util, worktree};

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
    if !util::check_repo_exist() {
        return;
    }
    if let Err(e) = run_clean(args).await {
        eprintln!("{}", format!("fatal: {}", e).red());
    }
}

async fn run_clean(args: CleanArgs) -> Result<(), String> {
    if !args.force && !args.dry_run {
        return Err("clean requires -f or -n".to_string());
    }

    let index_path = path::index();
    let index = Index::load(&index_path).unwrap_or_else(|_| Index::new());
    let untracked = worktree::untracked_workdir_paths(&index)?;

    if untracked.is_empty() {
        return Ok(());
    }

    if args.dry_run {
        for path in untracked {
            println!("Would remove {}", path.display());
        }
        return Ok(());
    }

    for path in untracked {
        let abs_path = util::workdir_to_absolute(&path);
        if abs_path.exists() {
            fs::remove_file(&abs_path).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}
