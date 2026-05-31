//! Implements `show-ref` to list all refs (branches, tags) with their object IDs.
//!
//! 实现 `show-ref` 以列出所有参考（分支、标记）及其对象 ID。

use std::io::Write;

use clap::Parser;
use serde::Serialize;

use crate::{
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
/// output, a pattern filter for substring search, and a JSON variant
/// for agents so users see all supported forms without reading the
/// design doc. Cross-cutting `--help` EXAMPLES rollout per
/// `docs/improvement/README.md` item B.
pub const SHOW_REF_EXAMPLES: &str = "\
EXAMPLES:
    libra show-ref                   List all local refs with their object hashes
    libra show-ref --heads           List only branches (refs/heads/)
    libra show-ref --tags            List only tags (refs/tags/)
    libra show-ref --head            Include HEAD in the output
    libra show-ref -s --heads        Print branch hashes only (one per line, scripting-friendly)
    libra show-ref main              Filter refs by substring match (e.g. only entries containing 'main')
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

    /// Filter refs by pattern (substring match on the ref name)
    pub pattern: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ShowRefEntry {
    hash: String,
    refname: String,
}

pub async fn execute(args: ShowRefArgs) -> Result<(), String> {
    execute_safe(args, &OutputConfig::default())
        .await
        .map_err(|err| err.render())
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting. Lists all refs (branches, tags) with their object IDs.
pub async fn execute_safe(args: ShowRefArgs, output: &OutputConfig) -> CliResult<()> {
    let hash_only = args.hash;
    let entries = collect_show_ref_entries(&args).await?;

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
        for entry in &entries {
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
    // When neither --heads nor --tags is specified, show both
    let show_heads = args.heads || !args.tags;
    let show_tags = args.tags || !args.heads;

    let mut entries: Vec<ShowRefEntry> = Vec::new();

    // Include HEAD if --head is specified
    if args.head
        && let Some(hash) = Head::current_commit_result()
            .await
            .map_err(|error| show_ref_branch_store_error("resolve HEAD", error))?
    {
        entries.push(ShowRefEntry {
            hash: hash.to_string(),
            refname: "HEAD".to_string(),
        });
    }

    // Collect local branches: refs/heads/<name>
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

    // Collect tags: refs/tags/<name>
    if show_tags {
        let tag_list = tag::list().await.map_err(show_ref_tag_list_error)?;
        for t in tag_list {
            // For annotated tags use the tag object hash; for lightweight use the commit hash.
            let hash = match &t.object {
                tag::TagObject::Commit(c) => c.id.to_string(),
                tag::TagObject::Tag(tg) => tg.id.to_string(),
                tag::TagObject::Blob(b) => b.id.to_string(),
                tag::TagObject::Tree(tr) => tr.id.to_string(),
            };
            entries.push(ShowRefEntry {
                hash,
                refname: format!("refs/tags/{}", t.name),
            });
        }
    }

    // Apply pattern filter if any patterns were given
    if !args.pattern.is_empty() {
        entries.retain(|entry| {
            entry.refname == "HEAD"
                || args
                    .pattern
                    .iter()
                    .any(|p| entry.refname.contains(p.as_str()))
        });
    }

    if entries.is_empty() {
        return Err(CliError::failure("no matching refs found")
            .with_stable_code(StableErrorCode::CliInvalidTarget));
    }

    Ok(entries)
}

fn remote_refname(remote: &str, branch_name: &str) -> String {
    if branch_name.starts_with("refs/remotes/") {
        branch_name.to_string()
    } else {
        format!("refs/remotes/{remote}/{branch_name}")
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::ShowRefArgs;

    #[test]
    fn test_show_ref_args_default() {
        let args = ShowRefArgs::try_parse_from(["show-ref"]).unwrap();
        assert!(!args.heads);
        assert!(!args.tags);
        assert!(!args.head);
        assert!(!args.hash);
        assert!(args.pattern.is_empty());
    }

    #[test]
    fn test_show_ref_args_heads_only() {
        let args = ShowRefArgs::try_parse_from(["show-ref", "--heads"]).unwrap();
        assert!(args.heads);
        assert!(!args.tags);
    }

    #[test]
    fn test_show_ref_args_tags_only() {
        let args = ShowRefArgs::try_parse_from(["show-ref", "--tags"]).unwrap();
        assert!(!args.heads);
        assert!(args.tags);
    }

    #[test]
    fn test_show_ref_args_with_pattern() {
        let args = ShowRefArgs::try_parse_from(["show-ref", "--heads", "main"]).unwrap();
        assert!(args.heads);
        assert_eq!(args.pattern, vec!["main".to_string()]);
    }

    #[test]
    fn test_show_ref_args_hash_flag() {
        let args = ShowRefArgs::try_parse_from(["show-ref", "--hash"]).unwrap();
        assert!(args.hash);
    }
}
