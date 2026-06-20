mod cleanup;
mod cli;
mod manifest;
mod plan;
mod registry;
mod runner;
mod scenarios;
mod support;

use std::{
    env,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use clap::Parser;
use cli::{Cli, Commands};

fn main() -> Result<()> {
    let cli = Cli::parse();
    let repo_root = repo_root()?;
    match cli.command {
        Commands::List => manifest::list(&repo_root),
        Commands::CheckPlan => plan::check_plan(&repo_root),
        Commands::Run {
            waves,
            only,
            binary,
            keep,
        } => runner::run(&repo_root, waves, only, binary, keep),
        Commands::RunLive { only, binary, keep } => {
            runner::run_live(&repo_root, only, binary, keep)
        }
    }
}

pub(crate) fn repo_root() -> Result<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .context("failed to resolve repository root from CARGO_MANIFEST_DIR")
}
