//! Implements `for-each-ref` to enumerate refs with filtering and formatting.

use clap::Parser;
use serde::Serialize;

use crate::utils::{
    error::CliResult,
    output::OutputConfig,
};

/// `--help` examples for for-each-ref
pub const FOR_EACH_REF_EXAMPLES: &str = "\
EXAMPLES:
    libra for-each-ref                  List all refs with commit info
    libra for-each-ref --heads          List only branches (refs/heads/)
    libra for-each-ref --tags           List only tags (refs/tags/)
    libra for-each-ref --remotes        List only remote-tracking refs
    libra for-each-ref --all            List all refs (default)
    libra for-each-ref --format='%(refname) %(objectname)'  Custom format
    libra for-each-ref --sort=refname   Sort by ref name
    libra for-each-ref --count=10       Limit output to 10 refs";

#[derive(Parser, Debug)]
#[command(after_help = FOR_EACH_REF_EXAMPLES)]
pub struct ForEachRefArgs {
    /// Show only branches (refs/heads/)
    #[clap(long)]
    pub heads: bool,

    /// Show only tags (refs/tags/)
    #[clap(long)]
    pub tags: bool,

    /// Show only remote-tracking refs (refs/remotes/)
    #[clap(long)]
    pub remotes: bool,

    /// Show all refs (default)
    #[clap(long)]
    pub all: bool,

    /// Custom output format with %(atoms)
    #[clap(long, value_name = "FORMAT")]
    pub format: Option<String>,

    /// Sort output by key (refname, objectname, taggerdate, etc.)
    #[clap(long, value_name = "KEY")]
    pub sort: Option<String>,

    /// Limit output to N refs
    #[clap(long, value_name = "COUNT")]
    pub count: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct RefEntry {
    refname: String,
    objectname: String,
    objecttype: String,
}

pub async fn execute(args: ForEachRefArgs) -> CliResult<()> {
    let output = OutputConfig::default();
    let result = run_for_each_ref(&args).await?;
    render_output(&result, &args, &output)?;
    Ok(())
}

pub async fn execute_safe(args: ForEachRefArgs, output: &OutputConfig) -> CliResult<()> {
    let result = run_for_each_ref(&args).await?;
    render_output(&result, &args, output)?;
    Ok(())
}

async fn run_for_each_ref(_args: &ForEachRefArgs) -> CliResult<Vec<RefEntry>> {
    // TODO: Implement full for-each-ref functionality
    // For now, return empty list as placeholder
    // Full implementation requires ref enumeration from branch/tag/remote listings
    Ok(Vec::new())
}

fn render_output(entries: &[RefEntry], _args: &ForEachRefArgs, _output: &OutputConfig) -> CliResult<()> {
    for entry in entries {
        println!("{} {} {}", entry.refname, entry.objectname, entry.objecttype);
    }
    Ok(())
}
