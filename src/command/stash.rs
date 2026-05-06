//! Implements stash push/pop/show/drop/apply by saving worktree/index states as commits and restoring them on demand.

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    str::FromStr,
};

use git_internal::{
    errors::GitError,
    hash::ObjectHash,
    internal::{
        index::{Index, Time},
        object::{
            ObjectTrait,
            commit::Commit,
            signature::Signature,
            tree::{Tree, TreeItem, TreeItemMode},
            types::ObjectType,
        },
    },
};
use serde::Serialize;

use crate::{
    cli::Stash,
    command::{
        load_object,
        reset::{
            rebuild_index_from_tree, remove_empty_directories, reset_index_to_commit,
            restore_working_directory_from_tree,
        },
    },
    internal::head::Head,
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        object,
        object_ext::TreeExt,
        output::{OutputConfig, emit_json_data},
        tree, util,
    },
};

// ── Typed error ──────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
enum StashError {
    #[error("not a libra repository")]
    NotInRepo,

    #[error("you do not have the initial commit yet")]
    NoInitialCommit,

    #[error("no stash found")]
    NoStashFound,

    #[error("'{0}' is not a valid stash reference")]
    InvalidStashRef(String),

    #[error("stash@{{{0}}}: stash does not exist")]
    StashNotExist(usize),

    #[error("merge conflict during stash apply:\n  {0}")]
    MergeConflict(String),

    #[error("a branch named '{0}' already exists")]
    BranchExists(String),

    #[error("clearing all stash entries requires --force in interactive mode")]
    ClearRequiresForce,

    #[error("failed to read object: {0}")]
    ReadObject(String),

    #[error("failed to write object: {0}")]
    WriteObject(String),

    #[error("failed to save index: {0}")]
    IndexSave(String),

    #[error("failed to reset working directory: {0}")]
    ResetFailed(String),

    #[error("{0}")]
    Other(String),
}

impl StashError {
    fn stable_code(&self) -> StableErrorCode {
        match self {
            Self::NotInRepo => StableErrorCode::RepoNotFound,
            Self::NoInitialCommit => StableErrorCode::RepoStateInvalid,
            Self::NoStashFound => StableErrorCode::CliInvalidTarget,
            Self::InvalidStashRef(_) => StableErrorCode::CliInvalidArguments,
            Self::StashNotExist(_) => StableErrorCode::CliInvalidTarget,
            Self::MergeConflict(_) => StableErrorCode::ConflictUnresolved,
            Self::BranchExists(_) => StableErrorCode::ConflictOperationBlocked,
            Self::ClearRequiresForce => StableErrorCode::CliInvalidArguments,
            Self::ReadObject(_) => StableErrorCode::IoReadFailed,
            Self::WriteObject(_) => StableErrorCode::IoWriteFailed,
            Self::IndexSave(_) => StableErrorCode::IoWriteFailed,
            Self::ResetFailed(_) => StableErrorCode::IoWriteFailed,
            Self::Other(_) => StableErrorCode::InternalInvariant,
        }
    }
}

impl From<StashError> for CliError {
    fn from(error: StashError) -> Self {
        let stable_code = error.stable_code();
        let message = error.to_string();
        match error {
            StashError::NotInRepo => CliError::repo_not_found(),
            StashError::NoInitialCommit => CliError::fatal(message)
                .with_stable_code(stable_code)
                .with_hint("create an initial commit first"),
            StashError::NoStashFound => CliError::fatal(message)
                .with_stable_code(stable_code)
                .with_hint("use 'libra stash push' to create a stash first"),
            StashError::InvalidStashRef(_) => CliError::fatal(message)
                .with_stable_code(stable_code)
                .with_hint("use stash@{N} syntax, e.g. stash@{0}"),
            StashError::StashNotExist(_) => CliError::fatal(message)
                .with_stable_code(stable_code)
                .with_hint("use 'libra stash list' to see available stashes"),
            StashError::MergeConflict(_) => CliError::failure(message)
                .with_stable_code(stable_code)
                .with_hint("resolve conflicts manually, then use 'libra add'"),
            StashError::BranchExists(_) => CliError::fatal(message)
                .with_stable_code(stable_code)
                .with_hint("use a different branch name or delete the existing branch first"),
            StashError::ClearRequiresForce => CliError::fatal(message)
                .with_stable_code(stable_code)
                .with_hint("re-run with --force, or use --json / --machine for scripted use"),
            _ => CliError::fatal(message).with_stable_code(stable_code),
        }
    }
}

// ── Structured output ────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "action")]
pub enum StashOutput {
    #[serde(rename = "noop")]
    Noop { message: String },
    #[serde(rename = "push")]
    Push { message: String, stash_id: String },
    #[serde(rename = "pop")]
    Pop {
        index: usize,
        stash_id: String,
        branch: String,
    },
    #[serde(rename = "apply")]
    Apply {
        index: usize,
        stash_id: String,
        branch: String,
    },
    #[serde(rename = "drop")]
    Drop { index: usize, stash_id: String },
    #[serde(rename = "list")]
    List { entries: Vec<StashListEntry> },
    #[serde(rename = "show")]
    Show {
        stash: String,
        stash_id: String,
        files: Vec<StashFileChange>,
        files_changed: StashFilesChangedStats,
        // Human-render hints. Skipped in JSON because the structured output
        // always carries the full file list with status.
        #[serde(skip)]
        name_only: bool,
        #[serde(skip)]
        name_status: bool,
    },
    #[serde(rename = "branch")]
    Branch {
        branch: String,
        stash: String,
        stash_id: String,
        applied: bool,
        dropped: bool,
    },
    #[serde(rename = "clear")]
    Clear { cleared_count: usize },
}

#[derive(Debug, Clone, Serialize)]
pub struct StashListEntry {
    pub index: usize,
    pub message: String,
    pub stash_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct StashFileChange {
    pub path: String,
    pub status: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct StashFilesChangedStats {
    pub total: usize,
    pub added: usize,
    pub modified: usize,
    pub deleted: usize,
}

/// `--help` examples shown in `libra stash --help` output.
pub const STASH_EXAMPLES: &str = "\
EXAMPLES:
    libra stash push -m 'WIP'         Save current changes
    libra stash list                  Show all stash entries
    libra stash show                  File-level summary of stash@{0}
    libra stash show stash@{1}        Inspect a specific stash entry
    libra stash branch hotfix         Branch off the latest stash and drop it
    libra stash apply                 Re-apply stash@{0} without dropping
    libra stash pop                   Apply stash@{0} and drop it
    libra stash clear --force         Remove every stash entry";

// ── Entry points ─────────────────────────────────────────────────────

pub async fn execute(stash_cmd: Stash) {
    if let Err(e) = execute_safe(stash_cmd, &OutputConfig::default()).await {
        e.print_stderr();
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting. Dispatches to stash sub-commands (push, pop, list,
/// apply, drop, show, branch, clear).
pub async fn execute_safe(stash_cmd: Stash, output: &OutputConfig) -> CliResult<()> {
    let result = run_stash(stash_cmd, output).await.map_err(CliError::from)?;
    render_stash_output(&result, output)
}

// ── Core execution ───────────────────────────────────────────────────

async fn run_stash(stash_cmd: Stash, output: &OutputConfig) -> Result<StashOutput, StashError> {
    util::require_repo().map_err(|_| StashError::NotInRepo)?;

    match stash_cmd {
        Stash::Push { message } => run_push(message).await,
        Stash::Pop { stash } => run_pop(stash).await,
        Stash::List => run_list().await,
        Stash::Apply { stash } => run_apply(stash).await,
        Stash::Drop { stash } => run_drop(stash).await,
        Stash::Show {
            stash,
            name_only,
            name_status,
        } => run_show(stash, name_only, name_status).await,
        Stash::Branch { branch, stash } => run_branch(branch, stash).await,
        Stash::Clear { force } => run_clear(force, output).await,
    }
}

async fn run_push(message: Option<String>) -> Result<StashOutput, StashError> {
    if !has_changes().await {
        return Ok(StashOutput::Noop {
            message: "No local changes to save".to_string(),
        });
    }

    let git_dir =
        util::try_get_storage_path(None).map_err(|e| StashError::ReadObject(e.to_string()))?;
    let index_path = git_dir.join("index");
    let index = Index::load(&index_path).unwrap_or_else(|_| Index::new());

    let head_commit_hash = Head::current_commit()
        .await
        .ok_or(StashError::NoInitialCommit)?;
    let head_commit_hash_str = head_commit_hash.to_string();

    let index_tree =
        tree::create_tree_from_index(&index).map_err(|e| StashError::WriteObject(e.to_string()))?;
    let index_tree_hash = index_tree.id;

    let (author, committer) = util::create_signatures().await;
    let (current_branch_name, head_commit_summary) = match Head::current().await {
        Head::Branch(name) => {
            let data = object::read_git_object(&git_dir, &head_commit_hash)
                .map_err(|e| StashError::ReadObject(e.to_string()))?;
            let c = Commit::from_bytes(&data, head_commit_hash)
                .map_err(|e| StashError::ReadObject(e.to_string()))?;
            let summary = c.message.lines().next().unwrap_or("").to_string();
            (name, summary)
        }
        Head::Detached(_) => {
            let data = object::read_git_object(&git_dir, &head_commit_hash)
                .map_err(|e| StashError::ReadObject(e.to_string()))?;
            let c = Commit::from_bytes(&data, head_commit_hash)
                .map_err(|e| StashError::ReadObject(e.to_string()))?;
            let summary = c.message.lines().next().unwrap_or("").to_string();
            ("(no branch)".to_string(), summary)
        }
    };

    let wip_message = format!(
        "WIP on {}: {} {}",
        current_branch_name,
        &head_commit_hash_str[..7],
        head_commit_summary
    );
    let final_message = message.unwrap_or(wip_message);

    let index_commit = Commit::new(
        author.clone(),
        committer.clone(),
        index_tree_hash,
        vec![head_commit_hash],
        &final_message,
    );
    let data = index_commit
        .to_data()
        .map_err(|e| StashError::WriteObject(e.to_string()))?;
    let index_commit_hash = object::write_git_object(&git_dir, "commit", &data)
        .map_err(|e| StashError::WriteObject(e.to_string()))?;

    let workdir = git_dir
        .parent()
        .ok_or_else(|| StashError::Other("cannot find workdir".into()))?;
    let worktree_tree =
        create_tree_from_workdir(workdir, &git_dir, &index).map_err(StashError::WriteObject)?;
    let worktree_tree_data = worktree_tree
        .to_data()
        .map_err(|e| StashError::WriteObject(e.to_string()))?;
    let worktree_tree_hash = object::write_git_object(&git_dir, "tree", &worktree_tree_data)
        .map_err(|e| StashError::WriteObject(e.to_string()))?;

    let stash_commit = Commit::new(
        author,
        committer.clone(),
        worktree_tree_hash,
        vec![head_commit_hash, index_commit_hash],
        &final_message,
    );
    let stash_commit_data = stash_commit
        .to_data()
        .map_err(|e| StashError::WriteObject(e.to_string()))?;
    let stash_commit_hash = object::write_git_object(&git_dir, "commit", &stash_commit_data)
        .map_err(|e| StashError::WriteObject(e.to_string()))?;

    update_stash_ref(&git_dir, &stash_commit_hash, &committer, &final_message)
        .map_err(|e| StashError::WriteObject(e.to_string()))?;

    perform_hard_reset(&head_commit_hash)
        .await
        .map_err(StashError::ResetFailed)?;

    Ok(StashOutput::Push {
        message: final_message,
        stash_id: stash_commit_hash.to_string(),
    })
}

async fn run_pop(stash: Option<String>) -> Result<StashOutput, StashError> {
    let apply_result = do_apply(stash.clone()).await?;
    let (index, stash_id, branch) = match apply_result {
        StashOutput::Apply {
            index,
            stash_id,
            branch,
        } => (index, stash_id, branch),
        other => {
            return Err(StashError::Other(format!(
                "internal error: expected stash apply to return StashOutput::Apply, got {other:?}",
            )));
        }
    };

    // Drop after successful apply
    do_drop(stash)?;

    Ok(StashOutput::Pop {
        index,
        stash_id,
        branch,
    })
}

async fn run_list() -> Result<StashOutput, StashError> {
    if !has_stash() {
        return Ok(StashOutput::List {
            entries: Vec::new(),
        });
    }

    let git_dir =
        util::try_get_storage_path(None).map_err(|e| StashError::ReadObject(e.to_string()))?;
    let stash_log_path = git_dir.join("logs/refs/stash");
    if !stash_log_path.exists() {
        return Ok(StashOutput::List {
            entries: Vec::new(),
        });
    }
    let entries = parse_stash_log_entries(read_stash_log_lines(&stash_log_path)?)?
        .into_iter()
        .enumerate()
        .map(|(index, entry)| StashListEntry {
            index,
            message: entry.message,
            stash_id: entry.stash_id,
        })
        .collect();

    Ok(StashOutput::List { entries })
}

async fn run_apply(stash: Option<String>) -> Result<StashOutput, StashError> {
    do_apply(stash).await
}

async fn run_drop(stash: Option<String>) -> Result<StashOutput, StashError> {
    do_drop(stash)
}

async fn run_show(
    stash: Option<String>,
    name_only: bool,
    name_status: bool,
) -> Result<StashOutput, StashError> {
    let (index, stash_id_str) = resolve_stash_to_commit_hash(stash)?;
    let git_dir =
        util::try_get_storage_path(None).map_err(|e| StashError::ReadObject(e.to_string()))?;

    let stash_hash =
        ObjectHash::from_str(&stash_id_str).map_err(|e| StashError::ReadObject(e.to_string()))?;
    let stash_commit_data = object::read_git_object(&git_dir, &stash_hash)
        .map_err(|e| StashError::ReadObject(e.to_string()))?;
    let stash_commit = Commit::from_bytes(&stash_commit_data, stash_hash)
        .map_err(|e| StashError::ReadObject(e.to_string()))?;

    let base_hash = *stash_commit
        .parent_commit_ids
        .first()
        .ok_or_else(|| StashError::ReadObject("stash commit is malformed".into()))?;
    let base_commit_data = object::read_git_object(&git_dir, &base_hash)
        .map_err(|e| StashError::ReadObject(e.to_string()))?;
    let base_commit = Commit::from_bytes(&base_commit_data, base_hash)
        .map_err(|e| StashError::ReadObject(e.to_string()))?;

    let base_tree_data = object::read_git_object(&git_dir, &base_commit.tree_id)
        .map_err(|e| StashError::ReadObject(e.to_string()))?;
    let base_tree = Tree::from_bytes(&base_tree_data, base_commit.tree_id)
        .map_err(|e| StashError::ReadObject(e.to_string()))?;
    let stash_tree_data = object::read_git_object(&git_dir, &stash_commit.tree_id)
        .map_err(|e| StashError::ReadObject(e.to_string()))?;
    let stash_tree = Tree::from_bytes(&stash_tree_data, stash_commit.tree_id)
        .map_err(|e| StashError::ReadObject(e.to_string()))?;

    let base_files = tree::get_tree_files_recursive(&base_tree, &git_dir, &PathBuf::new())
        .map_err(|e| StashError::ReadObject(e.to_string()))?;
    let stash_files = tree::get_tree_files_recursive(&stash_tree, &git_dir, &PathBuf::new())
        .map_err(|e| StashError::ReadObject(e.to_string()))?;

    let mut files: Vec<StashFileChange> = Vec::new();
    let mut stats = StashFilesChangedStats::default();
    let mut seen = HashSet::new();

    for (path, stash_item) in stash_files.iter() {
        seen.insert(path.clone());
        match base_files.get(path) {
            Some(base_item) => {
                if base_item.id != stash_item.id {
                    files.push(StashFileChange {
                        path: path.clone(),
                        status: "modified".to_string(),
                    });
                    stats.modified += 1;
                }
            }
            None => {
                files.push(StashFileChange {
                    path: path.clone(),
                    status: "added".to_string(),
                });
                stats.added += 1;
            }
        }
    }
    for (path, _) in base_files.iter() {
        if !seen.contains(path) {
            files.push(StashFileChange {
                path: path.clone(),
                status: "deleted".to_string(),
            });
            stats.deleted += 1;
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    stats.total = files.len();

    Ok(StashOutput::Show {
        stash: format!("stash@{{{index}}}"),
        stash_id: stash_id_str,
        files,
        files_changed: stats,
        name_only,
        name_status,
    })
}

async fn run_branch(branch_name: String, stash: Option<String>) -> Result<StashOutput, StashError> {
    use crate::internal::branch::Branch as InternalBranch;

    if InternalBranch::exists(&branch_name).await {
        return Err(StashError::BranchExists(branch_name));
    }

    // Resolve stash & metadata for the new branch base.
    let (index, stash_id_str) = resolve_stash_to_commit_hash(stash.clone())?;
    let stash_hash =
        ObjectHash::from_str(&stash_id_str).map_err(|e| StashError::ReadObject(e.to_string()))?;
    let git_dir =
        util::try_get_storage_path(None).map_err(|e| StashError::ReadObject(e.to_string()))?;
    let stash_commit_data = object::read_git_object(&git_dir, &stash_hash)
        .map_err(|e| StashError::ReadObject(e.to_string()))?;
    let stash_commit = Commit::from_bytes(&stash_commit_data, stash_hash)
        .map_err(|e| StashError::ReadObject(e.to_string()))?;
    let base_hash = *stash_commit
        .parent_commit_ids
        .first()
        .ok_or_else(|| StashError::ReadObject("stash commit is malformed".into()))?;

    InternalBranch::update_branch(&branch_name, &base_hash.to_string(), None)
        .await
        .map_err(|e| StashError::Other(format!("failed to create branch '{branch_name}': {e}")))?;

    // Switch HEAD to the new branch so apply runs on the right tip.
    Head::update(Head::Branch(branch_name.clone()), None).await;

    let apply_result = do_apply(stash.clone()).await?;
    let applied = matches!(apply_result, StashOutput::Apply { .. });
    let dropped = if applied {
        do_drop(stash).is_ok()
    } else {
        false
    };

    Ok(StashOutput::Branch {
        branch: branch_name,
        stash: format!("stash@{{{index}}}"),
        stash_id: stash_id_str,
        applied,
        dropped,
    })
}

async fn run_clear(force: bool, output: &OutputConfig) -> Result<StashOutput, StashError> {
    if !force && !output.is_json() {
        return Err(StashError::ClearRequiresForce);
    }

    if !has_stash() {
        return Ok(StashOutput::Clear { cleared_count: 0 });
    }

    let git_dir =
        util::try_get_storage_path(None).map_err(|e| StashError::ReadObject(e.to_string()))?;
    let stash_ref_path = git_dir.join("refs/stash");
    let stash_log_path = git_dir.join("logs/refs/stash");

    let cleared = if stash_log_path.exists() {
        let entries = parse_stash_log_entries(read_stash_log_lines(&stash_log_path)?)?;
        entries.len()
    } else {
        0
    };

    if stash_log_path.exists() {
        std::fs::remove_file(&stash_log_path)
            .map_err(|e| StashError::WriteObject(e.to_string()))?;
    }
    if stash_ref_path.exists() {
        std::fs::remove_file(&stash_ref_path)
            .map_err(|e| StashError::WriteObject(e.to_string()))?;
    }

    Ok(StashOutput::Clear {
        cleared_count: cleared,
    })
}

// ── Rendering ────────────────────────────────────────────────────────

fn render_stash_output(result: &StashOutput, output: &OutputConfig) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("stash", result, output);
    }

    if output.quiet {
        return Ok(());
    }

    match result {
        StashOutput::Noop { message } => {
            println!("{message}");
        }
        StashOutput::Push { message, .. } => {
            println!("Saved working directory and index state {message}");
        }
        StashOutput::Pop {
            index,
            stash_id,
            branch,
        } => {
            println!("On branch {branch}");
            println!(
                "Dropped stash@{{{index}}} ({})",
                &stash_id[..stash_id.len().min(7)]
            );
        }
        StashOutput::Apply { index, branch, .. } => {
            println!("On branch {branch}");
            println!("Applied stash@{{{index}}}");
        }
        StashOutput::Drop { index, stash_id } => {
            println!(
                "Dropped stash@{{{index}}} ({})",
                &stash_id[..stash_id.len().min(7)]
            );
        }
        StashOutput::List { entries } => {
            for entry in entries {
                println!("stash@{{{}}}: {}", entry.index, entry.message);
            }
        }
        StashOutput::Show {
            stash,
            files,
            files_changed,
            name_only,
            name_status,
            ..
        } => {
            if *name_only {
                for change in files {
                    println!("{}", change.path);
                }
            } else {
                println!("Files changed in {stash}:");
                let prefix_len = if *name_status { 0 } else { 9 };
                for change in files {
                    if *name_status {
                        println!("{}\t{}", change.status, change.path);
                    } else {
                        println!(
                            "  {:<prefix_len$}{}",
                            format!("{}:", change.status),
                            change.path
                        );
                    }
                }
                println!(
                    "{} files changed, {} insertions(+), {} deletions(-)",
                    files_changed.total, files_changed.added, files_changed.deleted
                );
            }
        }
        StashOutput::Branch {
            branch,
            stash,
            applied,
            dropped,
            ..
        } => {
            println!("Switched to a new branch '{branch}'");
            if *applied {
                println!("Applied {stash}");
            }
            if *dropped {
                println!("Dropped {stash}");
            }
        }
        StashOutput::Clear { cleared_count } => {
            if *cleared_count == 0 {
                println!("No stash entries to clear.");
            } else if *cleared_count == 1 {
                println!("Cleared 1 stash entry.");
            } else {
                println!("Cleared {cleared_count} stash entries.");
            }
        }
    }
    Ok(())
}

// ── Internal helpers ─────────────────────────────────────────────────

async fn do_apply(stash: Option<String>) -> Result<StashOutput, StashError> {
    let (index, hash_str) = resolve_stash_to_commit_hash(stash)?;
    let stash_commit_hash =
        ObjectHash::from_str(&hash_str).map_err(|e| StashError::ReadObject(e.to_string()))?;
    let git_dir =
        util::try_get_storage_path(None).map_err(|e| StashError::ReadObject(e.to_string()))?;

    let stash_commit_data = object::read_git_object(&git_dir, &stash_commit_hash)
        .map_err(|e| StashError::ReadObject(e.to_string()))?;
    let stash_commit = Commit::from_bytes(&stash_commit_data, stash_commit_hash)
        .map_err(|e| StashError::ReadObject(e.to_string()))?;

    let base_commit_hash = *stash_commit
        .parent_commit_ids
        .first()
        .ok_or_else(|| StashError::ReadObject("stash commit is malformed".into()))?;
    let head_commit_hash = Head::current_commit()
        .await
        .ok_or_else(|| StashError::ReadObject("could not get HEAD commit hash".into()))?;

    let base_commit_data = object::read_git_object(&git_dir, &base_commit_hash)
        .map_err(|e| StashError::ReadObject(e.to_string()))?;
    let base_commit = Commit::from_bytes(&base_commit_data, base_commit_hash)
        .map_err(|e| StashError::ReadObject(e.to_string()))?;
    let base_tree_data = object::read_git_object(&git_dir, &base_commit.tree_id)
        .map_err(|e| StashError::ReadObject(e.to_string()))?;
    let base_tree = Tree::from_bytes(&base_tree_data, base_commit.tree_id)
        .map_err(|e| StashError::ReadObject(e.to_string()))?;

    let head_commit_data = object::read_git_object(&git_dir, &head_commit_hash)
        .map_err(|e| StashError::ReadObject(e.to_string()))?;
    let head_commit = Commit::from_bytes(&head_commit_data, head_commit_hash)
        .map_err(|e| StashError::ReadObject(e.to_string()))?;
    let head_tree_data = object::read_git_object(&git_dir, &head_commit.tree_id)
        .map_err(|e| StashError::ReadObject(e.to_string()))?;
    let head_tree = Tree::from_bytes(&head_tree_data, head_commit.tree_id)
        .map_err(|e| StashError::ReadObject(e.to_string()))?;

    let stash_tree_data = object::read_git_object(&git_dir, &stash_commit.tree_id)
        .map_err(|e| StashError::ReadObject(e.to_string()))?;
    let stash_tree = Tree::from_bytes(&stash_tree_data, stash_commit.tree_id)
        .map_err(|e| StashError::ReadObject(e.to_string()))?;

    let merged_tree = merge_trees(&base_tree, &head_tree, &stash_tree, &git_dir)
        .map_err(StashError::MergeConflict)?;

    let workdir = git_dir.parent().ok_or_else(|| {
        StashError::Other(format!(
            "internal error: storage path '{}' has no parent",
            git_dir.display()
        ))
    })?;
    let index_path = git_dir.join("index");
    let mut new_index = Index::new();

    let head_files = tree::get_tree_files_recursive(&head_tree, &git_dir, &PathBuf::new())
        .map_err(|e| StashError::ReadObject(e.to_string()))?;
    let merged_files = tree::get_tree_files_recursive(&merged_tree, &git_dir, &PathBuf::new())
        .map_err(|e| StashError::ReadObject(e.to_string()))?;

    for (path, _) in head_files.iter() {
        if !merged_files.contains_key(path) {
            let full_path = workdir.join(path);
            if full_path.exists() {
                fs::remove_file(full_path).map_err(|e| StashError::WriteObject(e.to_string()))?;
            }
        }
    }

    restore_working_directory_from_tree(&merged_tree, workdir, "")
        .map_err(StashError::WriteObject)?;
    rebuild_index_from_tree(&merged_tree, &mut new_index, "").map_err(StashError::IndexSave)?;

    new_index
        .save(&index_path)
        .map_err(|e| StashError::IndexSave(e.to_string()))?;

    let branch = match Head::current().await {
        Head::Branch(name) => name,
        Head::Detached(_) => "(no branch)".to_string(),
    };

    Ok(StashOutput::Apply {
        index,
        stash_id: hash_str,
        branch,
    })
}

fn do_drop(stash: Option<String>) -> Result<StashOutput, StashError> {
    if !has_stash() {
        return Err(StashError::NoStashFound);
    }

    let git_dir =
        util::try_get_storage_path(None).map_err(|e| StashError::ReadObject(e.to_string()))?;
    let stash_ref_path = git_dir.join("refs/stash");
    let stash_log_path = git_dir.join("logs/refs/stash");
    if !stash_log_path.exists() {
        return Err(StashError::NoStashFound);
    }

    let mut entries = parse_stash_log_entries(read_stash_log_lines(&stash_log_path)?)?;
    if entries.is_empty() {
        return Err(StashError::NoStashFound);
    }

    let index_to_drop = match stash {
        None => 0,
        Some(s) => parse_stash_index(&s)?,
    };

    if index_to_drop >= entries.len() {
        return Err(StashError::StashNotExist(index_to_drop));
    }
    let removed_entry = entries.remove(index_to_drop);
    let stash_commit_hash = removed_entry.stash_id;

    if entries.is_empty() {
        std::fs::remove_file(&stash_log_path)
            .map_err(|e| StashError::WriteObject(e.to_string()))?;
        if stash_ref_path.exists() {
            std::fs::remove_file(&stash_ref_path)
                .map_err(|e| StashError::WriteObject(e.to_string()))?;
        }
    } else {
        let new_content = entries
            .iter()
            .map(|entry| entry.raw_line.as_str())
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        std::fs::write(&stash_log_path, new_content)
            .map_err(|e| StashError::WriteObject(e.to_string()))?;

        if index_to_drop == 0
            && let Some(new_top_entry) = entries.first()
        {
            std::fs::write(&stash_ref_path, format!("{}\n", new_top_entry.stash_id))
                .map_err(|e| StashError::WriteObject(e.to_string()))?;
        }
    }

    Ok(StashOutput::Drop {
        index: index_to_drop,
        stash_id: stash_commit_hash,
    })
}

fn parse_stash_index(s: &str) -> Result<usize, StashError> {
    if s.starts_with("stash@{") && s.ends_with('}') {
        s[7..s.len() - 1]
            .parse::<usize>()
            .map_err(|_| StashError::InvalidStashRef(s.to_string()))
    } else {
        Err(StashError::InvalidStashRef(s.to_string()))
    }
}

// ── Unchanged helpers ────────────────────────────────────────────────

async fn has_changes() -> bool {
    let Some(git_dir) = util::try_get_storage_path(None).ok() else {
        return false;
    };

    let head_tree_hash = match Head::current_commit().await {
        Some(hash) => {
            let Ok(commit_data) = object::read_git_object(&git_dir, &hash) else {
                return false;
            };
            let Ok(commit) = Commit::from_bytes(&commit_data, hash) else {
                return false;
            };
            commit.tree_id
        }
        None => ObjectHash::from_type_and_data(ObjectType::Tree, &[]),
    };

    let index_path = git_dir.join("index");
    let Ok(index) = Index::load(&index_path) else {
        return false;
    };
    let Ok(index_tree) = tree::create_tree_from_index(&index) else {
        return false;
    };
    let index_tree_hash = index_tree.id;

    if head_tree_hash != index_tree_hash {
        return true;
    }

    let Some(workdir) = git_dir.parent() else {
        return false;
    };
    for entry in index.tracked_entries(0) {
        let file_path = workdir.join(&entry.name);

        let Ok(metadata) = fs::metadata(&file_path) else {
            return true;
        };

        let mtime =
            Time::from_system_time(metadata.modified().unwrap_or(std::time::SystemTime::now()));
        if metadata.len() == entry.size as u64 && mtime == entry.mtime {
            continue;
        }

        if let Ok(content) = fs::read(&file_path) {
            let header = format!("blob {}\0", content.len());
            let mut full_content = header.into_bytes();
            full_content.extend_from_slice(&content);
            let current_hash = ObjectHash::new(&full_content);

            if current_hash != entry.hash {
                return true;
            }
        } else {
            return true;
        }
    }

    false
}

fn has_stash() -> bool {
    util::try_get_storage_path(None)
        .ok()
        .map(|p| p.join("refs/stash").is_file())
        .unwrap_or(false)
}

fn empty_tree() -> Result<Tree, String> {
    let empty_id = ObjectHash::from_type_and_data(ObjectType::Tree, &[]);
    Tree::from_bytes(&[], empty_id).map_err(|e| e.to_string())
}

fn read_stash_log_lines(stash_log_path: &Path) -> Result<Vec<String>, StashError> {
    let file = std::fs::File::open(stash_log_path).map_err(|e| {
        StashError::ReadObject(format!(
            "failed to open stash log '{}': {}",
            stash_log_path.display(),
            e
        ))
    })?;
    let reader = BufReader::new(file);
    reader.lines().collect::<Result<Vec<_>, _>>().map_err(|e| {
        StashError::ReadObject(format!(
            "failed to read stash log '{}': {}",
            stash_log_path.display(),
            e
        ))
    })
}

#[derive(Debug, Clone)]
struct StashLogEntry {
    raw_line: String,
    stash_id: String,
    message: String,
}

fn parse_stash_log_entries(lines: Vec<String>) -> Result<Vec<StashLogEntry>, StashError> {
    let mut entries = Vec::new();

    for (line_index, line) in lines.into_iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }

        let stash_id = line.split_whitespace().nth(1).ok_or_else(|| {
            StashError::ReadObject(format!(
                "corrupted stash log entry at line {}: missing stash commit hash",
                line_index + 1
            ))
        })?;
        let stash_id = ObjectHash::from_str(stash_id).map_err(|_| {
            StashError::ReadObject(format!(
                "corrupted stash log entry at line {}: invalid stash commit hash '{}'",
                line_index + 1,
                stash_id
            ))
        })?;
        let message = line
            .split_once('\t')
            .map(|(_, message)| message.to_string())
            .unwrap_or_default();

        entries.push(StashLogEntry {
            raw_line: line,
            stash_id: stash_id.to_string(),
            message,
        });
    }

    Ok(entries)
}

fn resolve_stash_to_commit_hash(stash_ref: Option<String>) -> Result<(usize, String), StashError> {
    if !has_stash() {
        return Err(StashError::NoStashFound);
    }

    let git_dir =
        util::try_get_storage_path(None).map_err(|e| StashError::ReadObject(e.to_string()))?;
    let stash_log_path = git_dir.join("logs/refs/stash");
    if !stash_log_path.exists() {
        return Err(StashError::NoStashFound);
    }

    let entries = parse_stash_log_entries(read_stash_log_lines(&stash_log_path)?)?;

    let index_to_resolve = match stash_ref {
        None => 0,
        Some(s) => parse_stash_index(&s)?,
    };

    if index_to_resolve >= entries.len() {
        return Err(StashError::StashNotExist(index_to_resolve));
    }

    Ok((index_to_resolve, entries[index_to_resolve].stash_id.clone()))
}

fn update_stash_ref(
    git_dir: &Path,
    stash_hash: &ObjectHash,
    committer: &Signature,
    message: &str,
) -> Result<(), GitError> {
    let stash_ref_path = git_dir.join("refs/stash");
    let stash_log_path = git_dir.join("logs/refs/stash");

    let old_hash = if stash_ref_path.exists() {
        let content = fs::read_to_string(&stash_ref_path)?;
        ObjectHash::from_str(content.trim())
            .map_err(|_| GitError::InvalidHashValue(content.trim().to_string()))?
    } else {
        ObjectHash::default()
    };

    if let Some(parent) = stash_ref_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&stash_ref_path, format!("{stash_hash}\n"))?;

    if let Some(parent) = stash_log_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let reflog_entry = format!(
        "{} {} {} <{}> {} {}\t{}",
        old_hash,
        stash_hash,
        committer.name,
        committer.email,
        committer.timestamp,
        committer.timezone,
        message
    );

    let mut lines = if stash_log_path.exists() {
        let content = fs::read_to_string(&stash_log_path)?;
        content.lines().map(String::from).collect()
    } else {
        Vec::new()
    };

    lines.insert(0, reflog_entry);
    let new_content = lines.join("\n") + "\n";
    fs::write(stash_log_path, new_content)?;

    Ok(())
}

async fn perform_hard_reset(target_commit_id: &ObjectHash) -> Result<(), String> {
    let git_dir = util::try_get_storage_path(None).map_err(|e| e.to_string())?;
    let workdir = git_dir
        .parent()
        .ok_or_else(|| "cannot find workdir".to_string())?;
    let index_path = git_dir.join("index");

    let index_before_reset = Index::load(&index_path).unwrap_or_else(|_| Index::new());
    let all_tracked_paths: Vec<PathBuf> = index_before_reset
        .tracked_entries(0)
        .into_iter()
        .map(|e| PathBuf::from(&e.name))
        .collect();

    let target_commit: Commit =
        load_object(target_commit_id).map_err(|e| format!("failed to load target commit: {e}"))?;
    let target_tree: Tree = load_object(&target_commit.tree_id)
        .map_err(|e| format!("failed to load target tree: {e}"))?;
    let files_in_target_tree: HashSet<PathBuf> = target_tree
        .get_plain_items()
        .into_iter()
        .map(|(p, _)| p)
        .collect();

    reset_index_to_commit(target_commit_id)?;

    for path in &all_tracked_paths {
        if !files_in_target_tree.contains(path) {
            let full_path = workdir.join(path);
            if full_path.exists() {
                fs::remove_file(full_path).map_err(|e| format!("failed to remove file: {e}"))?;
            }
        }
    }

    restore_working_directory_from_tree(&target_tree, workdir, "")?;
    remove_empty_directories(workdir)?;

    Ok(())
}

fn create_tree_from_workdir(workdir: &Path, git_dir: &Path, index: &Index) -> Result<Tree, String> {
    fn build_tree_recursive(
        dir: &Path,
        git_dir: &Path,
        index: &Index,
        workdir: &Path,
    ) -> Result<Tree, String> {
        let mut items = Vec::new();
        let entries = fs::read_dir(dir).map_err(|e| e.to_string())?;

        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            let file_name = path
                .file_name()
                .ok_or_else(|| format!("entry has no file name: {}", path.display()))?
                .to_str()
                .ok_or_else(|| format!("invalid path encoding: {}", path.display()))?
                .to_string();

            // Skip only Libra's metadata directory. User-managed dotfiles such
            // as `.gitignore`, `.env`, or `.config/*` must remain stashed.
            if path == git_dir {
                continue;
            }

            if path.is_dir() {
                let subtree = build_tree_recursive(&path, git_dir, index, workdir)?;
                // Skip empty subtrees to avoid Tree serialisation errors
                if subtree.tree_items.is_empty() {
                    continue;
                }
                let subtree_data = subtree.to_data().map_err(|e| e.to_string())?;
                let subtree_hash = object::write_git_object(git_dir, "tree", &subtree_data)
                    .map_err(|e| e.to_string())?;
                items.push(TreeItem::new(TreeItemMode::Tree, subtree_hash, file_name));
            } else if path.is_file() {
                let metadata = fs::metadata(&path).map_err(|e| e.to_string())?;
                let relative_path = path.strip_prefix(workdir).map_err(|e| {
                    format!(
                        "failed to strip workdir prefix from {}: {e}",
                        path.display()
                    )
                })?;
                let relative_path_str = relative_path
                    .to_str()
                    .ok_or_else(|| format!("invalid path encoding: {}", relative_path.display()))?;

                if let Some(entry) = index.get(relative_path_str, 0) {
                    let mtime = Time::from_system_time(
                        metadata.modified().unwrap_or(std::time::SystemTime::now()),
                    );
                    let size = metadata.len() as u32;

                    if entry.mtime == mtime && entry.size == size {
                        #[cfg(unix)]
                        let mode = if metadata.permissions().mode() & 0o111 != 0 {
                            TreeItemMode::BlobExecutable
                        } else {
                            TreeItemMode::Blob
                        };
                        #[cfg(not(unix))]
                        let mode = TreeItemMode::Blob;
                        items.push(TreeItem::new(mode, entry.hash, file_name));
                        continue;
                    }
                }

                let content = fs::read(&path).map_err(|e| e.to_string())?;
                let blob_hash = object::write_git_object(git_dir, "blob", &content)
                    .map_err(|e| e.to_string())?;

                #[cfg(unix)]
                let mode = if metadata.permissions().mode() & 0o111 != 0 {
                    TreeItemMode::BlobExecutable
                } else {
                    TreeItemMode::Blob
                };
                #[cfg(not(unix))]
                let mode = TreeItemMode::Blob;

                items.push(TreeItem::new(mode, blob_hash, file_name));
            }
        }

        items.sort_by(|a, b| a.name.cmp(&b.name));
        if items.is_empty() {
            empty_tree()
        } else {
            Tree::from_tree_items(items).map_err(|e| e.to_string())
        }
    }

    build_tree_recursive(workdir, git_dir, index, workdir)
}

fn build_tree_from_flat_items(
    files: &HashMap<String, TreeItem>,
    git_dir: &Path,
) -> Result<Tree, String> {
    #[derive(Default)]
    struct DirectoryEntries {
        files: Vec<TreeItem>,
        subdirs: HashSet<String>,
    }

    fn build_dir(
        current_dir: &Path,
        directories: &mut HashMap<PathBuf, DirectoryEntries>,
        git_dir: &Path,
    ) -> Result<Tree, String> {
        let mut directory = directories.remove(current_dir).unwrap_or_default();
        let mut subdirs: Vec<String> = directory.subdirs.into_iter().collect();
        subdirs.sort();

        for subdir_name in subdirs {
            let subdir_path = if current_dir.as_os_str().is_empty() {
                PathBuf::from(&subdir_name)
            } else {
                current_dir.join(&subdir_name)
            };
            let subtree = build_dir(&subdir_path, directories, git_dir)?;
            if subtree.tree_items.is_empty() {
                continue;
            }
            let subtree_data = subtree.to_data().map_err(|e| e.to_string())?;
            let subtree_hash = object::write_git_object(git_dir, "tree", &subtree_data)
                .map_err(|e| e.to_string())?;
            directory
                .files
                .push(TreeItem::new(TreeItemMode::Tree, subtree_hash, subdir_name));
        }

        directory.files.sort_by(|a, b| a.name.cmp(&b.name));
        if directory.files.is_empty() {
            empty_tree()
        } else {
            Tree::from_tree_items(directory.files).map_err(|e| e.to_string())
        }
    }

    let mut directories: HashMap<PathBuf, DirectoryEntries> = HashMap::new();
    directories.entry(PathBuf::new()).or_default();

    for (path_str, item) in files {
        let path_buf = PathBuf::from(path_str);
        let file_name = path_buf
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("invalid merged stash path: {}", path_buf.display()))?
            .to_string();
        let parent_dir = path_buf
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .to_path_buf();

        let mut tree_item = item.clone();
        tree_item.name = file_name;
        directories
            .entry(parent_dir.clone())
            .or_default()
            .files
            .push(tree_item);

        let mut current_dir = PathBuf::new();
        for component in parent_dir.components() {
            let subdir_name = component
                .as_os_str()
                .to_str()
                .ok_or_else(|| format!("invalid merged stash path: {}", path_buf.display()))?
                .to_string();
            if subdir_name.is_empty() {
                continue;
            }
            directories
                .entry(current_dir.clone())
                .or_default()
                .subdirs
                .insert(subdir_name.clone());
            current_dir.push(&subdir_name);
            directories.entry(current_dir.clone()).or_default();
        }
    }

    build_dir(Path::new(""), &mut directories, git_dir)
}

/// Performs a three-way merge of tree objects.
/// This is a simplified implementation that prefers the stash version in case of conflicts.
fn merge_trees(base: &Tree, head: &Tree, stash: &Tree, git_dir: &Path) -> Result<Tree, String> {
    let base_items = tree::get_tree_files_recursive(base, git_dir, &PathBuf::new())?;
    let mut head_items = tree::get_tree_files_recursive(head, git_dir, &PathBuf::new())?;
    let stash_items = tree::get_tree_files_recursive(stash, git_dir, &PathBuf::new())?;
    let mut conflicts = Vec::new();

    // Replay only paths changed by the stash snapshot. If HEAD diverged from
    // the stash base in a different way, stop instead of overwriting newer work.
    for (path, stash_item) in stash_items.iter() {
        let base_item = base_items.get(path);
        let head_item = head_items.get(path);

        match (base_item, head_item) {
            (Some(b), Some(h)) => {
                if b.id != h.id && b.id != stash_item.id && h.id != stash_item.id {
                    conflicts.push(path.clone());
                    continue;
                }

                // Stash version differs from base: apply stash change
                if b.id != stash_item.id {
                    head_items.insert(path.clone(), stash_item.clone());
                }
            }
            (Some(_), None) => {
                head_items.insert(path.clone(), stash_item.clone());
            }
            (None, Some(_)) => {
                head_items.insert(path.clone(), stash_item.clone());
            }
            (None, None) => {
                head_items.insert(path.clone(), stash_item.clone());
            }
        }
    }

    for (path, base_item) in base_items.iter() {
        if !stash_items.contains_key(path) {
            if let Some(head_item) = head_items.get(path)
                && head_item.id != base_item.id
            {
                conflicts.push(path.clone());
                continue;
            }
            head_items.remove(path);
        }
    }

    if !conflicts.is_empty() {
        let error_message = format!(
            "Your local changes to the following files would be overwritten by merge:\n  {}\n\
             Please commit your changes or stash them before you merge.",
            conflicts.join("\n  ")
        );
        return Err(error_message);
    }

    build_tree_from_flat_items(&head_items, git_dir)
}

/// Get the number of stashes
pub(crate) fn get_stash_num() -> Result<usize, String> {
    if !has_stash() {
        return Ok(0);
    }

    let git_dir = util::try_get_storage_path(None).map_err(|e| e.to_string())?;
    let stash_log_path = git_dir.join("logs/refs/stash");
    if !stash_log_path.exists() {
        return Ok(0);
    }
    let count =
        parse_stash_log_entries(read_stash_log_lines(&stash_log_path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?
            .len();

    Ok(count)
}
