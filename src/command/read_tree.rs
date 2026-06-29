//! `libra read-tree` — read a tree object into the index. Plumbing companion to
//! `write-tree`.
//!
//! First-version scope: it reads a single tree-ish into the index, **replacing**
//! the current index content. It does **not** touch the working tree, and the
//! Git options that would (`-u`, `-m`, `--prefix`, `--reset`) are not exposed —
//! so this command can never silently overwrite working-tree files. Document
//! those as deferred.

use std::str::FromStr;

use clap::Parser;
use git_internal::{
    hash::ObjectHash,
    internal::object::{commit::Commit, tree::Tree},
};
use serde::Serialize;

use crate::{
    command::load_object,
    internal::tree_plumbing,
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
        path, util,
    },
};

/// `--help` examples (cross-cutting EXAMPLES contract, `_general.md`).
pub const READ_TREE_EXAMPLES: &str = "\
EXAMPLES:
    libra read-tree HEAD          Replace the index with HEAD's tree
    libra read-tree <tree-id>     Read a specific tree object into the index
    libra --json read-tree HEAD   Structured JSON output for agents";

/// Read a tree object into the index (index-only; the working tree is untouched).
#[derive(Parser, Debug)]
#[command(after_help = READ_TREE_EXAMPLES)]
pub struct ReadTreeArgs {
    /// The tree-ish to read: a tree object id, a commit id/ref/tag (peeled to
    /// its tree), or a branch name / `HEAD`.
    #[clap(value_name = "TREE-ISH")]
    pub tree_ish: String,
}

#[derive(Debug, Serialize)]
struct ReadTreeOutput {
    tree: String,
    entries: usize,
}

pub async fn execute(args: ReadTreeArgs) {
    if let Err(err) = execute_safe(args, &OutputConfig::default()).await {
        err.print_stderr();
        std::process::exit(err.exit_code());
    }
}

/// Safe entry point. Resolves the tree-ish, reads it into a fresh index, and
/// saves it to `.libra/index` (replacing the previous index). The working tree
/// is not modified.
pub async fn execute_safe(args: ReadTreeArgs, output: &OutputConfig) -> CliResult<()> {
    util::require_repo().map_err(|_| CliError::repo_not_found())?;

    let tree_id = resolve_tree_ish(&args.tree_ish).await?;
    let index = tree_plumbing::read_tree_into_index(&tree_id).map_err(|error| {
        CliError::fatal(format!("failed to read tree '{}': {error}", args.tree_ish))
            .with_stable_code(StableErrorCode::RepoStateInvalid)
    })?;
    let entries = index.tracked_entries(0).len();
    index.save(path::index()).map_err(|error| {
        CliError::fatal(format!("failed to save index: {error}"))
            .with_stable_code(StableErrorCode::RepoStateInvalid)
    })?;

    if output.is_json() {
        emit_json_data(
            "read-tree",
            &ReadTreeOutput {
                tree: tree_id.to_string(),
                entries,
            },
            output,
        )
    } else {
        Ok(())
    }
}

/// Resolve a tree-ish to a concrete tree object id. Accepts a raw tree id, a
/// commit id (peeled to its tree), or any revision name `util::get_commit_base`
/// understands (branch, tag, `HEAD`, …, peeled to its tree).
async fn resolve_tree_ish(tree_ish: &str) -> CliResult<ObjectHash> {
    if let Ok(hash) = ObjectHash::from_str(tree_ish) {
        if let Ok(tree) = load_object::<Tree>(&hash) {
            return Ok(tree.id);
        }
        if let Ok(commit) = load_object::<Commit>(&hash) {
            return Ok(commit.tree_id);
        }
    }

    let commit_hash = util::get_commit_base(tree_ish).await.map_err(|error| {
        CliError::fatal(format!("not a valid tree-ish '{tree_ish}': {error}"))
            .with_exit_code(128)
            .with_stable_code(StableErrorCode::CliInvalidTarget)
    })?;
    let commit = load_object::<Commit>(&commit_hash).map_err(|error| {
        CliError::fatal(format!("failed to load commit for '{tree_ish}': {error}"))
            .with_stable_code(StableErrorCode::RepoStateInvalid)
    })?;
    Ok(commit.tree_id)
}
