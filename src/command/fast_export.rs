//! `libra fast-export` — emit the commits reachable from a revision as a
//! `git fast-import` stream, a focused subset of `git fast-export`.
//!
//! Read-only: it never writes objects or refs. Each commit is emitted with a
//! full `deleteall` + `M` file list reconstructed from its tree (rather than a
//! parent diff) — larger than Git's diff-based output but byte-for-byte correct
//! and simpler. Multi-ref / signed-tag / `--export-marks` are deferred.

use std::{
    collections::{HashMap, HashSet},
    io::{self, Write},
};

use clap::Parser;
use git_internal::{
    hash::ObjectHash,
    internal::object::{
        blob::Blob,
        commit::Commit,
        tree::{Tree, TreeItemMode},
    },
};

use crate::{
    command::load_object,
    internal::head::Head,
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::OutputConfig,
        util,
    },
};

pub const FAST_EXPORT_EXAMPLES: &str = "\
EXAMPLES:
    libra fast-export                 Export the current branch as a fast-import stream
    libra fast-export main            Export commits reachable from main
    libra fast-export HEAD > repo.fi  Save the stream to a file";

/// Emit the history reachable from `<rev>` as a fast-import stream.
#[derive(Parser, Debug)]
#[command(after_help = FAST_EXPORT_EXAMPLES)]
pub struct FastExportArgs {
    /// The revision to export (default `HEAD`). Its reachable commits are
    /// emitted under the corresponding branch ref.
    #[clap(value_name = "REV")]
    pub rev: Option<String>,
}

pub async fn execute(args: FastExportArgs) {
    if let Err(err) = execute_safe(args, &OutputConfig::default()).await {
        err.print_stderr();
        std::process::exit(err.exit_code());
    }
}

pub async fn execute_safe(args: FastExportArgs, _output: &OutputConfig) -> CliResult<()> {
    util::require_repo().map_err(|_| CliError::repo_not_found())?;

    let rev = args.rev.clone().unwrap_or_else(|| "HEAD".to_string());
    let tip = util::get_commit_base(&rev).await.map_err(|error| {
        CliError::fatal(format!("not a valid revision '{rev}': {error}"))
            .with_exit_code(128)
            .with_stable_code(StableErrorCode::CliInvalidTarget)
    })?;
    let ref_name = resolve_ref_name(&rev).await;

    let commits = crate::command::log::get_reachable_commits(tip.to_string(), None)
        .await
        .map_err(|error| error.with_exit_code(128))?;
    let by_id: HashMap<ObjectHash, Commit> = commits.into_iter().map(|c| (c.id, c)).collect();
    let order = topological_order(&tip, &by_id);

    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());

    // Marks are shared by blobs and commits; `:1`, `:2`, … in emission order.
    let mut marks: HashMap<ObjectHash, usize> = HashMap::new();
    let mut next_mark = 0usize;

    for oid in &order {
        let commit = &by_id[oid];
        let leaves = flatten_tree(&commit.tree_id)?;

        // Emit each not-yet-seen blob.
        for (_, mode, id) in &leaves {
            if *mode == TreeItemMode::Commit || marks.contains_key(id) {
                continue; // gitlink (no blob) or already emitted
            }
            let blob: Blob = load_object(id).map_err(|error| object_error(id, error))?;
            next_mark += 1;
            marks.insert(*id, next_mark);
            writeln!(out, "blob\nmark :{next_mark}\ndata {}", blob.data.len())
                .map_err(write_err)?;
            out.write_all(&blob.data).map_err(write_err)?;
            out.write_all(b"\n").map_err(write_err)?;
        }

        next_mark += 1;
        let commit_mark = next_mark;
        marks.insert(*oid, commit_mark);
        writeln!(out, "commit {ref_name}\nmark :{commit_mark}").map_err(write_err)?;
        writeln!(out, "author {}", format_ident(&commit.author)).map_err(write_err)?;
        writeln!(out, "committer {}", format_ident(&commit.committer)).map_err(write_err)?;
        writeln!(out, "data {}", commit.message.len()).map_err(write_err)?;
        out.write_all(commit.message.as_bytes())
            .map_err(write_err)?;
        // The optional LF after the `data` payload terminates the message so the
        // following directive is recognised even when the message has no
        // trailing newline.
        out.write_all(b"\n").map_err(write_err)?;

        // Link parents. Every parent is in the reachable set and was emitted
        // earlier (topological order); a missing mark means a broken traversal,
        // which is a hard error rather than a silently root-less commit.
        for (index, parent) in commit.parent_commit_ids.iter().enumerate() {
            let mark = marks.get(parent).ok_or_else(|| {
                CliError::fatal(format!(
                    "fast-export: parent {parent} of {oid} was not exported"
                ))
                .with_exit_code(128)
                .with_stable_code(StableErrorCode::InternalInvariant)
            })?;
            let keyword = if index == 0 { "from" } else { "merge" };
            writeln!(out, "{keyword} :{mark}").map_err(write_err)?;
        }

        // Reconstruct the whole tree (no parent diff needed).
        out.write_all(b"deleteall\n").map_err(write_err)?;
        for (path, mode, id) in &leaves {
            let mode_str = std::str::from_utf8(mode.to_bytes()).unwrap_or("100644");
            if *mode == TreeItemMode::Commit {
                // Submodule gitlink: reference the commit sha directly.
                writeln!(out, "M {mode_str} {id} {path}").map_err(write_err)?;
            } else {
                let mark = marks.get(id).copied().unwrap_or(0);
                writeln!(out, "M {mode_str} :{mark} {path}").map_err(write_err)?;
            }
        }
    }

    out.flush().map_err(write_err)?;
    Ok(())
}

/// Resolve the ref name a commit should be exported under.
async fn resolve_ref_name(rev: &str) -> String {
    if rev == "HEAD" {
        return match Head::current().await {
            Head::Branch(name) => format!("refs/heads/{name}"),
            Head::Detached(_) => "refs/heads/master".to_string(),
        };
    }
    if rev.starts_with("refs/") {
        rev.to_string()
    } else {
        format!("refs/heads/{rev}")
    }
}

/// Post-order (parents-before-children) traversal of the reachable set so every
/// `from`/`merge` mark is defined before it is referenced.
fn topological_order(tip: &ObjectHash, by_id: &HashMap<ObjectHash, Commit>) -> Vec<ObjectHash> {
    let mut order = Vec::with_capacity(by_id.len());
    let mut visited: HashSet<ObjectHash> = HashSet::new();
    // (id, children-expanded?)
    let mut stack: Vec<(ObjectHash, bool)> = vec![(*tip, false)];
    while let Some((id, expanded)) = stack.pop() {
        if expanded {
            order.push(id);
            continue;
        }
        if !visited.insert(id) {
            continue;
        }
        stack.push((id, true));
        if let Some(commit) = by_id.get(&id) {
            for parent in &commit.parent_commit_ids {
                if by_id.contains_key(parent) && !visited.contains(parent) {
                    stack.push((*parent, false));
                }
            }
        }
    }
    order
}

/// Flatten a tree into `(path, mode, object id)` leaves (recursing into
/// subtrees), in a deterministic order.
fn flatten_tree(tree_id: &ObjectHash) -> CliResult<Vec<(String, TreeItemMode, ObjectHash)>> {
    fn walk(
        tree_id: &ObjectHash,
        prefix: &str,
        out: &mut Vec<(String, TreeItemMode, ObjectHash)>,
    ) -> CliResult<()> {
        let tree: Tree = load_object(tree_id).map_err(|error| object_error(tree_id, error))?;
        for item in &tree.tree_items {
            let path = if prefix.is_empty() {
                item.name.clone()
            } else {
                format!("{prefix}/{}", item.name)
            };
            if item.mode == TreeItemMode::Tree {
                walk(&item.id, &path, out)?;
            } else {
                out.push((path, item.mode, item.id));
            }
        }
        Ok(())
    }
    let mut leaves = Vec::new();
    walk(tree_id, "", &mut leaves)?;
    Ok(leaves)
}

/// fast-import identity line body: `Name <email> <timestamp> <timezone>`.
fn format_ident(sig: &git_internal::internal::object::signature::Signature) -> String {
    format!(
        "{} <{}> {} {}",
        sig.name, sig.email, sig.timestamp, sig.timezone
    )
}

fn object_error(id: &ObjectHash, error: git_internal::errors::GitError) -> CliError {
    CliError::fatal(format!("failed to load object {id}: {error}"))
        .with_exit_code(128)
        .with_stable_code(StableErrorCode::RepoCorrupt)
}

fn write_err(error: io::Error) -> CliError {
    CliError::fatal(format!("failed to write fast-export stream: {error}"))
        .with_exit_code(128)
        .with_stable_code(StableErrorCode::IoWriteFailed)
}
