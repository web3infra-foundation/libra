//! Implements `ls-files` to list files in the index with various filters.

use clap::Parser;
use serde::Serialize;

use crate::utils::{
    error::CliResult,
    output::OutputConfig,
};

/// `--help` examples for ls-files
pub const LS_FILES_EXAMPLES: &str = "\
EXAMPLES:
    libra ls-files                      List all files in the index (cached)
    libra ls-files --cached             Show only files staged in the index
    libra ls-files --deleted            Show only deleted files
    libra ls-files --modified           Show only modified files
    libra ls-files --stage              Include stage information (for conflicts)
    libra ls-files --others             Show untracked files
    libra ls-files --exclude-standard   Exclude files matching .gitignore
    libra ls-files -s                   Short output with stage info";

#[derive(Parser, Debug)]
#[command(after_help = LS_FILES_EXAMPLES)]
pub struct LsFilesArgs {
    /// Show only staged (cached) files in the index
    #[clap(long)]
    pub cached: bool,

    /// Show only deleted files
    #[clap(long)]
    pub deleted: bool,

    /// Show only modified files
    #[clap(long)]
    pub modified: bool,

    /// Include stage information for conflict resolution
    #[clap(long)]
    pub stage: bool,

    /// Show untracked files (not in index)
    #[clap(long)]
    pub others: bool,

    /// Exclude files matching .gitignore patterns
    #[clap(long)]
    pub exclude_standard: bool,

    /// Short output format with mode and hash
    #[clap(short = 's')]
    pub short: bool,
}

#[derive(Debug, Serialize)]
pub struct FileEntry {
    path: String,
    hash: Option<String>,
    mode: Option<String>,
    stage: Option<u32>,
    status: String,
}

pub async fn execute(args: LsFilesArgs) -> CliResult<()> {
    let output = OutputConfig::default();
    let result = run_ls_files(&args)?;
    render_output(&result, &args, &output)?;
    Ok(())
}

pub async fn execute_safe(args: LsFilesArgs, output: &OutputConfig) -> CliResult<()> {
    let result = run_ls_files(&args)?;
    render_output(&result, &args, output)?;
    Ok(())
}

fn run_ls_files(_args: &LsFilesArgs) -> CliResult<Vec<FileEntry>> {
    // TODO: Implement full ls-files functionality
    // For now, return empty list as placeholder
    // Full implementation requires index enumeration and filtering

    Ok(Vec::new())
}

fn render_output(entries: &[FileEntry], args: &LsFilesArgs, _output: &OutputConfig) -> CliResult<()> {
    for entry in entries {
        if args.short || args.stage {
            print!("[{}] {}", entry.stage.unwrap_or(0), entry.path);
            if let Some(hash) = &entry.hash {
                print!(" ({})", hash);
            }
            println!();
        } else {
            println!("{}", entry.path);
        }
    }

    Ok(())
}
