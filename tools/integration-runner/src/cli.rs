use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(about = "Black-box integration runner for the Libra CLI")]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Commands,
}

#[derive(Subcommand)]
pub(crate) enum Commands {
    /// List scenarios from docs/development/integration-scenarios.yaml.
    List,
    /// Check yaml, markdown, matrix, and implemented runner registry alignment.
    CheckPlan,
    /// Run Wave 0 and implemented black-box CLI scenarios.
    Run {
        /// Comma-separated waves, e.g. 0,1,2.
        #[arg(long)]
        waves: Option<String>,
        /// Comma-separated scenario ids. Wave 0 can be selected via --waves 0.
        #[arg(long)]
        only: Option<String>,
        /// Existing libra binary path. Defaults to target/debug/libra, building if absent.
        #[arg(long)]
        binary: Option<PathBuf>,
        /// Keep the run root after successful runs.
        #[arg(long)]
        keep: bool,
    },
    /// Run Wave 3 GitHub live scenarios (requires `gh` CLI + auth + repo create/delete scope).
    /// Uses host gh auth (no token in logs) + Rust cleanup guard + full isolation for `libra`.
    RunLive {
        /// Comma-separated live ids (e.g. live.github-create-push-clone-fetch).
        #[arg(long)]
        only: Option<String>,
        /// Existing libra binary path. Defaults to target/debug/libra, building if absent.
        #[arg(long)]
        binary: Option<PathBuf>,
        /// Keep the run root after successful runs.
        #[arg(long)]
        keep: bool,
    },
}
