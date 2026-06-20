//! Implements `for-each-ref` to enumerate refs with filtering and formatting.

use std::str::FromStr;

use clap::Parser;
use git_internal::hash::ObjectHash;
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
    libra for-each-ref --points-at HEAD List refs that point at HEAD
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

    /// Show only refs that point at OBJECT
    #[clap(long, value_name = "OBJECT")]
    pub points_at: Option<String>,

    /// Show only refs whose commit contains COMMIT (i.e. COMMIT is an ancestor).
    #[clap(long, value_name = "COMMIT")]
    pub contains: Option<String>,

    /// Show only refs whose commit does NOT contain COMMIT.
    #[clap(long = "no-contains", value_name = "COMMIT")]
    pub no_contains: Option<String>,

    /// Refname patterns to match
    #[clap(value_name = "PATTERN")]
    pub patterns: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RefEntry {
    refname: String,
    objectname: String,
    objecttype: String,
    #[serde(skip_serializing)]
    points_at: Vec<String>,
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
            entries.push(direct_ref_entry(
                format!("refs/heads/{}", branch.name),
                branch.commit.to_string(),
                "commit",
            ));
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
                entries.push(direct_ref_entry(
                    refname,
                    branch.commit.to_string(),
                    "commit",
                ));
            }
        }
    }

    if show_all || _args.tags {
        let tags = tag::list().await.map_err(|source| {
            CliError::fatal(format!("failed to list tags: {source}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        })?;
        for t in tags {
            entries.push(tag_ref_entry(&t));
        }
    }

    if let Some(object_ref) = _args.points_at.as_deref() {
        let target = resolve_points_at_target(object_ref).await?;
        entries.retain(|entry| entry.points_at.iter().any(|hash| hash == &target));
    }

    if let Some(commit_ref) = _args.contains.as_deref() {
        let target = resolve_points_at_target(commit_ref).await?;
        entries = retain_refs_containing(entries, &target, true).await?;
    }
    if let Some(commit_ref) = _args.no_contains.as_deref() {
        let target = resolve_points_at_target(commit_ref).await?;
        entries = retain_refs_containing(entries, &target, false).await?;
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

/// Keep (or, when `want` is false, drop) refs whose commit has `target` as an
/// ancestor — i.e. the ref "contains" `target` (`--contains`/`--no-contains`).
/// A ref's commit is its first peeled object id; reachability reuses
/// `log::get_reachable_commits`, so this walks history once per ref.
async fn retain_refs_containing(
    entries: Vec<RefEntry>,
    target: &str,
    want: bool,
) -> CliResult<Vec<RefEntry>> {
    let mut kept = Vec::with_capacity(entries.len());
    for entry in entries {
        let contains = match entry.points_at.first() {
            Some(commit) => crate::command::log::get_reachable_commits(commit.clone(), None)
                .await?
                .iter()
                .any(|reachable| reachable.id.to_string().as_str() == target),
            None => false,
        };
        if contains == want {
            kept.push(entry);
        }
    }
    Ok(kept)
}

fn direct_ref_entry(refname: String, objectname: String, objecttype: &str) -> RefEntry {
    RefEntry {
        refname,
        points_at: vec![objectname.clone()],
        objectname,
        objecttype: objecttype.to_string(),
    }
}

fn tag_ref_entry(tag: &tag::Tag) -> RefEntry {
    let (objectname, objecttype, points_at) = tag_object_info(&tag.object);
    RefEntry {
        refname: format!("refs/tags/{}", tag.name),
        objectname,
        objecttype,
        points_at,
    }
}

fn tag_object_info(object: &tag::TagObject) -> (String, String, Vec<String>) {
    match object {
        tag::TagObject::Commit(commit) => {
            let id = commit.id.to_string();
            (id.clone(), "commit".to_string(), vec![id])
        }
        tag::TagObject::Tag(tag) => (
            tag.id.to_string(),
            "tag".to_string(),
            vec![tag.id.to_string(), tag.object_hash.to_string()],
        ),
        tag::TagObject::Tree(tree) => {
            let id = tree.id.to_string();
            (id.clone(), "tree".to_string(), vec![id])
        }
        tag::TagObject::Blob(blob) => {
            let id = blob.id.to_string();
            (id.clone(), "blob".to_string(), vec![id])
        }
    }
}

async fn resolve_points_at_target(object_ref: &str) -> CliResult<String> {
    let tag_name = object_ref.strip_prefix("refs/tags/").unwrap_or(object_ref);
    if let Some(tag_ref) = tag::find_tag_ref(tag_name).await.map_err(|source| {
        CliError::fatal(format!("failed to resolve tag '{object_ref}': {source}"))
            .with_stable_code(StableErrorCode::IoReadFailed)
    })? {
        let target = tag_ref.target.ok_or_else(|| {
            CliError::fatal(format!("tag '{object_ref}' is missing target object"))
                .with_stable_code(StableErrorCode::RepoCorrupt)
        })?;
        ObjectHash::from_str(&target).map_err(|source| {
            CliError::fatal(format!(
                "tag '{object_ref}' has invalid target object '{target}': {source}"
            ))
            .with_stable_code(StableErrorCode::RepoCorrupt)
        })?;
        return Ok(target);
    }

    if let Ok(hash) = util::get_commit_base(object_ref).await {
        return Ok(hash.to_string());
    }
    if let Ok(hash) = ObjectHash::from_str(object_ref) {
        return Ok(hash.to_string());
    }

    Err(
        CliError::fatal(format!("Not a valid object name {object_ref}"))
            .with_stable_code(StableErrorCode::CliInvalidTarget),
    )
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
