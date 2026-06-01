//! Command-line surface for creating archives from committed tree snapshots.

use clap::Parser;

use crate::utils::{
    error::{CliError, CliResult, StableErrorCode},
    output::OutputConfig,
};

const ARCHIVE_EXAMPLES: &str = "\
EXAMPLES:
    libra archive -o project.tar HEAD
    libra archive --format=tar.gz --prefix=project-v1/ -o project-v1.tar.gz v1.0
    libra archive --format=zip -o feature.zip feature-branch";

/// Create an archive of files from a named tree.
#[derive(Parser, Debug)]
#[command(after_help = ARCHIVE_EXAMPLES)]
pub struct ArchiveArgs {
    /// Commit, branch, tag, or abbreviated commit hash to archive. Defaults to HEAD.
    #[arg(default_value = "HEAD", value_name = "TREEISH")]
    pub treeish: String,

    /// Archive format: tar, tar.gz, tar.bz2, or zip.
    #[arg(short = 'f', long, default_value = "tar", value_name = "FMT")]
    pub format: String,

    /// Write archive bytes to a file instead of stdout.
    #[arg(short = 'o', long, value_name = "FILE")]
    pub output: Option<String>,

    /// Prepend a relative directory prefix to each archived path.
    #[arg(long, value_name = "PREFIX")]
    pub prefix: Option<String>,
}

/// # Side Effects
///
/// None yet. This skeleton only reserves the CLI surface for later archive
/// implementation commits.
///
/// # Errors
///
/// Always returns `Unsupported` until archive creation is implemented.
pub async fn execute_safe(_args: ArchiveArgs, _output: &OutputConfig) -> CliResult<()> {
    Err(CliError::failure(
        "archive command is registered but archive creation is not implemented yet",
    )
    .with_stable_code(StableErrorCode::Unsupported))
}
