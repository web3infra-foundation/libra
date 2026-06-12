//! Implements `for-each-ref` to enumerate refs with filtering and formatting.

use clap::Parser;
use serde::Serialize;

use crate::{
    internal::{branch::Branch, config::ConfigKv, tag},
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
        util,
    },
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

    /// Refname patterns to match
    #[clap(value_name = "PATTERN")]
    pub patterns: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
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
    util::require_repo().map_err(|_| CliError::repo_not_found())?;

    let show_all = _args.all || (!_args.heads && !_args.tags && !_args.remotes);
    let mut entries = Vec::new();

    if show_all || _args.heads {
        let branches = Branch::list_branches_result(None)
            .await
            .map_err(branch_error)?;
        for branch in branches {
            entries.push(RefEntry {
                refname: format!("refs/heads/{}", branch.name),
                objectname: branch.commit.to_string(),
                objecttype: "commit".to_string(),
            });
        }
    }

    if show_all || _args.remotes {
        let remotes = ConfigKv::all_remote_configs().await.map_err(|source| {
            CliError::fatal(format!("failed to list remotes: {source}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        })?;
        for remote in remotes {
            let branches = Branch::list_branches_result(Some(&remote.name))
                .await
                .map_err(branch_error)?;
            for branch in branches {
                let refname = if branch.name.starts_with("refs/remotes/") {
                    branch.name
                } else {
                    format!("refs/remotes/{}/{}", remote.name, branch.name)
                };
                entries.push(RefEntry {
                    refname,
                    objectname: branch.commit.to_string(),
                    objecttype: "commit".to_string(),
                });
            }
        }
    }

    if show_all || _args.tags {
        let tags = tag::list().await.map_err(|source| {
            CliError::fatal(format!("failed to list tags: {source}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        })?;
        for t in tags {
            let (objectname, objecttype) = tag_object_info(&t.object);
            entries.push(RefEntry {
                refname: format!("refs/tags/{}", t.name),
                objectname,
                objecttype,
            });
        }
    }

    if !_args.patterns.is_empty() {
        entries.retain(|entry| {
            _args
                .patterns
                .iter()
                .any(|pattern| matches_ref_pattern(&entry.refname, pattern))
        });
    }

    sort_entries(&mut entries, _args.sort.as_deref())?;
    if let Some(count) = _args.count {
        entries.truncate(count);
    }
    Ok(entries)
}

fn tag_object_info(object: &tag::TagObject) -> (String, String) {
    match object {
        tag::TagObject::Commit(commit) => (commit.id.to_string(), "commit".to_string()),
        tag::TagObject::Tag(tag) => (tag.id.to_string(), "tag".to_string()),
        tag::TagObject::Tree(tree) => (tree.id.to_string(), "tree".to_string()),
        tag::TagObject::Blob(blob) => (blob.id.to_string(), "blob".to_string()),
    }
}

fn branch_error(source: crate::internal::branch::BranchStoreError) -> CliError {
    CliError::fatal(format!("failed to list branches: {source}"))
        .with_stable_code(StableErrorCode::IoReadFailed)
}

fn matches_ref_pattern(refname: &str, pattern: &str) -> bool {
    refname == pattern || refname.ends_with(pattern) || refname.contains(pattern)
}

fn sort_entries(entries: &mut [RefEntry], sort: Option<&str>) -> CliResult<()> {
    match sort.unwrap_or("refname") {
        "refname" => entries.sort_by(|a, b| a.refname.cmp(&b.refname)),
        "-refname" => entries.sort_by(|a, b| b.refname.cmp(&a.refname)),
        "objectname" => entries.sort_by(|a, b| a.objectname.cmp(&b.objectname)),
        "-objectname" => entries.sort_by(|a, b| b.objectname.cmp(&a.objectname)),
        other => {
            return Err(CliError::command_usage(format!(
                "unsupported for-each-ref sort key '{other}'"
            ))
            .with_stable_code(StableErrorCode::CliInvalidArguments));
        }
    }
    Ok(())
}

fn render_output(
    entries: &[RefEntry],
    args: &ForEachRefArgs,
    output: &OutputConfig,
) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("for-each-ref", &entries.to_vec(), output);
    }
    if output.quiet {
        return Ok(());
    }

    for entry in entries {
        if let Some(format) = &args.format {
            println!("{}", render_format(format, entry)?);
        } else {
            println!("{} {}", entry.objectname, entry.refname);
        }
    }
    Ok(())
}

fn render_format(format: &str, entry: &RefEntry) -> CliResult<String> {
    let mut out = format.to_string();
    for (atom, value) in [
        ("%(refname)", entry.refname.as_str()),
        ("%(objectname)", entry.objectname.as_str()),
        ("%(objecttype)", entry.objecttype.as_str()),
    ] {
        out = out.replace(atom, value);
    }
    if out.contains("%(") {
        return Err(
            CliError::command_usage("unsupported for-each-ref format atom")
                .with_stable_code(StableErrorCode::CliInvalidArguments),
        );
    }
    Ok(out)
}
