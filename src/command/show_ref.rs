//! Implements `show-ref` to list all refs (branches, tags) with their object IDs.

use std::io::Write;

use clap::Parser;
use serde::Serialize;

use crate::{
    command::show_ref_deref,
    internal::{
        branch::{Branch, BranchStoreError},
        config::ConfigKv,
        head::Head,
        tag::{self, ListTagError},
    },
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
    },
};

/// `--help` examples shown in `libra show-ref --help` output.
///
/// `show-ref` lists local references with their object hashes. The
/// banner pins the all-refs default, `--heads` / `--tags` scope
/// filters, the `--head` opt-in for including HEAD, `-s` for hash-only
/// output, a Git-style path-segment pattern filter, and a JSON variant
/// for agents so users see all supported forms without reading the
/// design doc. Cross-cutting `--help` EXAMPLES rollout per
/// `docs/development/commands/_general.md` item B.
pub const SHOW_REF_EXAMPLES: &str = "\
EXAMPLES:
    libra show-ref                   List all local refs with their object hashes
    libra show-ref --heads           List only branches (refs/heads/)
    libra show-ref --tags            List only tags (refs/tags/)
    libra show-ref --head            Include HEAD in the output
    libra show-ref -s --heads        Print branch hashes only (one per line, scripting-friendly)
    libra show-ref -d --tags         Peel annotated tags and show refs/tags/<name>^{} lines
    libra show-ref --verify refs/heads/main
                                     Verify an exact refname and print it
    libra show-ref --exists refs/heads/main
                                     Check whether an exact refname exists
    libra show-ref main              Filter refs ending in the path segment 'main'
    libra show-ref --json --heads    Structured JSON output for agents";

#[derive(Parser, Debug)]
#[command(after_help = SHOW_REF_EXAMPLES)]
pub struct ShowRefArgs {
    /// Show only branches (refs/heads/)
    #[clap(long)]
    pub heads: bool,

    /// Show only tags (refs/tags/)
    #[clap(long)]
    pub tags: bool,

    /// Include HEAD in the output
    #[clap(long = "head")]
    pub head: bool,

    /// Only show the object hash, not the reference name
    #[clap(short = 's', long = "hash")]
    pub hash: bool,

    /// Dereference annotated tags and include peeled refs/tags/<name>^{} entries
    #[clap(short = 'd', long = "dereference")]
    pub dereference: bool,

    /// Verify exact refnames instead of pattern filtering
    #[clap(long, conflicts_with = "exists")]
    pub verify: bool,

    /// Check whether exactly one ref exists without printing it
    #[clap(long, conflicts_with = "verify")]
    pub exists: bool,

    /// Filter refs by path-segment suffix pattern
    pub pattern: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ShowRefEntry {
    pub(crate) hash: String,
    pub(crate) refname: String,
}

pub async fn execute(args: ShowRefArgs) -> Result<(), String> {
    execute_safe(args, &OutputConfig::default())
        .await
        .map_err(|err| err.render())
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting. Lists all refs (branches, tags) with their object IDs.
pub async fn execute_safe(args: ShowRefArgs, output: &OutputConfig) -> CliResult<()> {
    if args.exists {
        return execute_exists(&args, output).await;
    }
    if args.verify {
        return execute_verify(&args, output).await;
    }

    let entries = collect_show_ref_entries(&args).await?;
    render_show_ref_entries(&entries, args.hash, output)
}

fn render_show_ref_entries(
    entries: &[ShowRefEntry],
    hash_only: bool,
    output: &OutputConfig,
) -> CliResult<()> {
    if output.is_json() {
        emit_json_data(
            "show-ref",
            &serde_json::json!({
                "hash_only": hash_only,
                "entries": entries,
            }),
            output,
        )
    } else if output.quiet {
        Ok(())
    } else {
        let stdout = std::io::stdout();
        let mut writer = stdout.lock();
        for entry in entries {
            if hash_only {
                writeln!(writer, "{}", entry.hash)
                    .map_err(|e| CliError::io(format!("failed to write show-ref output: {e}")))?;
            } else {
                writeln!(writer, "{} {}", entry.hash, entry.refname)
                    .map_err(|e| CliError::io(format!("failed to write show-ref output: {e}")))?;
            }
        }
        Ok(())
    }
}

fn show_ref_branch_store_error(context: &str, error: BranchStoreError) -> CliError {
    match error {
        BranchStoreError::Query(detail) => {
            CliError::fatal(format!("failed to {context}: {detail}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        }
        other => CliError::fatal(format!("failed to {context}: {other}"))
            .with_stable_code(StableErrorCode::RepoCorrupt),
    }
}

fn show_ref_tag_list_error(error: ListTagError) -> CliError {
    let stable_code = match error {
        ListTagError::Query(_) => StableErrorCode::IoReadFailed,
        ListTagError::MissingCommit { .. }
        | ListTagError::InvalidObjectHash { .. }
        | ListTagError::MissingName
        | ListTagError::LoadObject { .. } => StableErrorCode::RepoCorrupt,
    };

    CliError::fatal(format!("failed to list tags: {error}")).with_stable_code(stable_code)
}

async fn collect_show_ref_entries(args: &ShowRefArgs) -> CliResult<Vec<ShowRefEntry>> {
    let show_heads = args.heads || !args.tags;
    let show_tags = args.tags || !args.heads;
    let mut entries =
        collect_raw_show_ref_entries(args.head, show_heads, show_tags, args.dereference).await?;
    if !args.pattern.is_empty() {
        entries.retain(|entry| {
            entry.refname == "HEAD"
                || args
                    .pattern
                    .iter()
                    .any(|p| show_ref_pattern_matches(&entry.refname, p))
        });
    }

    if entries.is_empty() {
        return Err(CliError::failure("no matching refs found")
            .with_stable_code(StableErrorCode::CliInvalidTarget));
    }

    Ok(entries)
}

async fn collect_raw_show_ref_entries(
    include_head: bool,
    show_heads: bool,
    show_tags: bool,
    dereference_tags: bool,
) -> CliResult<Vec<ShowRefEntry>> {
    let mut entries = Vec::new();

    if include_head
        && let Some(hash) = Head::current_commit_result()
            .await
            .map_err(|error| show_ref_branch_store_error("resolve HEAD", error))?
    {
        entries.push(ShowRefEntry {
            hash: hash.to_string(),
            refname: String::from("HEAD"),
        });
    }

    if show_heads {
        let branches = Branch::list_branches_result(None)
            .await
            .map_err(|error| show_ref_branch_store_error("list branches", error))?;
        for branch in branches {
            entries.push(ShowRefEntry {
                hash: branch.commit.to_string(),
                refname: format!("refs/heads/{}", branch.name),
            });
        }

        let remotes = ConfigKv::all_remote_configs().await.map_err(|error| {
            CliError::fatal(format!("failed to list remotes: {error}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        })?;
        for remote in remotes {
            let branches = Branch::list_branches_result(Some(&remote.name))
                .await
                .map_err(|error| {
                    show_ref_branch_store_error(
                        &format!("list remote-tracking branches for '{}'", remote.name),
                        error,
                    )
                })?;
            for branch in branches {
                entries.push(ShowRefEntry {
                    hash: branch.commit.to_string(),
                    refname: remote_refname(&remote.name, &branch.name),
                });
            }
        }
    }

    if show_tags {
        let tag_list = tag::list().await.map_err(show_ref_tag_list_error)?;
        for t in tag_list {
            entries.extend(show_ref_deref::tag_entries(t, dereference_tags).await?);
        }
    }

    Ok(entries)
}

fn show_ref_pattern_matches(refname: &str, pattern: &str) -> bool {
    let base_refname = refname.strip_suffix("^{}").unwrap_or(refname);
    base_refname == pattern
        || base_refname
            .strip_suffix(pattern)
            .is_some_and(|prefix| prefix.ends_with('/'))
}

async fn execute_verify(args: &ShowRefArgs, output: &OutputConfig) -> CliResult<()> {
    if args.pattern.is_empty() {
        return Err(CliError::command_usage("--verify requires a reference").with_exit_code(128));
    }

    let entries = collect_raw_show_ref_entries(true, true, true, args.dereference).await?;
    let mut verified = Vec::new();
    for refname in &args.pattern {
        let Some(entry) = entries.iter().find(|entry| entry.refname == *refname) else {
            let exit_code = if output.quiet { 1 } else { 128 };
            return Err(CliError::failure(format!("'{refname}' - not a valid ref"))
                .with_stable_code(StableErrorCode::CliInvalidTarget)
                .with_exit_code(exit_code));
        };
        verified.push(entry.clone());
    }

    render_show_ref_entries(&verified, args.hash, output)
}

async fn execute_exists(args: &ShowRefArgs, output: &OutputConfig) -> CliResult<()> {
    if args.pattern.len() != 1 {
        let message = if args.pattern.is_empty() {
            "--exists requires a reference"
        } else {
            "--exists requires exactly one reference"
        };
        return Err(CliError::command_usage(message).with_exit_code(128));
    }

    let refname = &args.pattern[0];
    let entries = collect_raw_show_ref_entries(true, true, true, false).await?;
    if !entries.iter().any(|entry| entry.refname == *refname) {
        return Err(
            CliError::failure(format!("reference does not exist: {refname}"))
                .with_stable_code(StableErrorCode::CliInvalidTarget)
                .with_exit_code(2),
        );
    }

    if !output.is_json() {
        return Ok(());
    }
    emit_json_data(
        "show-ref",
        &serde_json::json!({ "exists": true, "refname": refname }),
        output,
    )
}

fn remote_refname(remote: &str, branch_name: &str) -> String {
    if branch_name.starts_with("refs/remotes/") {
        return branch_name.to_string();
    }
    format!("refs/remotes/{remote}/{branch_name}")
}
