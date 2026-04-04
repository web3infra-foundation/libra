//! Implements the revert command by parsing targets, reversing commit changes into the index/worktree, and optionally creating a new commit.

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use clap::Parser;
use git_internal::{
    hash::ObjectHash,
    internal::{
        index::{Index, IndexEntry},
        object::{
            commit::Commit,
            tree::{Tree, TreeItemMode},
        },
    },
};
use serde::Serialize;

use crate::{
    command::{load_object, save_object},
    common_utils::format_commit_msg,
    internal::{branch::Branch, head::Head},
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        object_ext::{BlobExt, TreeExt},
        output::{OutputConfig, emit_json_data},
        path,
        text::short_display_hash,
        util,
    },
};

const REVERT_EXAMPLES: &str = "\
EXAMPLES:
    libra revert HEAD                     Revert the most recent commit
    libra revert abc1234                  Revert a specific commit
    libra revert -n HEAD                  Revert without auto-committing
    libra revert --json HEAD              Structured JSON output for agents";

// ── Typed error ──────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
enum RevertError {
    #[error("not a libra repository")]
    NotInRepo,

    #[error("you are in a 'detached HEAD' state; reverting is not allowed")]
    DetachedHead,

    #[error("failed to resolve commit reference '{0}'")]
    InvalidCommit(String),

    #[error("reverting merge commits is not yet supported")]
    MergeCommitUnsupported,

    #[error("conflict: file '{path}' was modified in a later commit")]
    Conflict { path: String },

    #[error("failed to load object: {0}")]
    LoadObject(String),

    #[error("failed to save object: {0}")]
    SaveObject(String),

    #[error("failed to write worktree: {0}")]
    WriteWorktree(String),

    #[error("failed to save index: {0}")]
    IndexSave(String),

    #[error("failed to update HEAD: {0}")]
    UpdateHead(String),
}

impl RevertError {
    fn stable_code(&self) -> StableErrorCode {
        match self {
            Self::NotInRepo => StableErrorCode::RepoNotFound,
            Self::DetachedHead => StableErrorCode::RepoStateInvalid,
            Self::InvalidCommit(_) => StableErrorCode::CliInvalidTarget,
            Self::MergeCommitUnsupported => StableErrorCode::CliInvalidArguments,
            Self::Conflict { .. } => StableErrorCode::ConflictUnresolved,
            Self::LoadObject(_) => StableErrorCode::IoReadFailed,
            Self::SaveObject(_) => StableErrorCode::IoWriteFailed,
            Self::WriteWorktree(_) => StableErrorCode::IoWriteFailed,
            Self::IndexSave(_) => StableErrorCode::IoWriteFailed,
            Self::UpdateHead(_) => StableErrorCode::IoWriteFailed,
        }
    }
}

impl From<RevertError> for CliError {
    fn from(error: RevertError) -> Self {
        let stable_code = error.stable_code();
        let message = error.to_string();
        match error {
            RevertError::NotInRepo => CliError::repo_not_found(),
            RevertError::DetachedHead => CliError::fatal(message)
                .with_stable_code(stable_code)
                .with_hint("switch to a branch first with 'libra switch <branch>'"),
            RevertError::InvalidCommit(_) => CliError::fatal(message)
                .with_stable_code(stable_code)
                .with_hint("use 'libra log' to find valid commit references"),
            RevertError::MergeCommitUnsupported => CliError::fatal(message)
                .with_stable_code(stable_code)
                .with_hint("merge commit revert support is planned for a future release"),
            RevertError::Conflict { .. } => CliError::failure(message)
                .with_stable_code(stable_code)
                .with_hint("resolve conflicts manually, then use 'libra commit'"),
            _ => CliError::fatal(message).with_stable_code(stable_code),
        }
    }
}

// ── Structured output ────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct RevertOutput {
    pub reverted_commit: String,
    pub short_reverted: String,
    pub new_commit: Option<String>,
    pub short_new: Option<String>,
    pub no_commit: bool,
    pub files_changed: usize,
}

// ── Entry points ─────────────────────────────────────────────────────

/// Arguments for the revert command.
/// Reverts the specified commit by creating a new commit that undoes the changes.
#[derive(Parser, Debug)]
#[command(about = "Revert some existing commits")]
#[command(after_help = REVERT_EXAMPLES)]
pub struct RevertArgs {
    /// Commit to revert (can be commit hash, branch name, or HEAD)
    #[clap(required = true)]
    pub commit: String,

    /// Don't automatically commit the revert, just stage the changes
    #[clap(short = 'n', long)]
    pub no_commit: bool,
}

pub async fn execute(args: RevertArgs) {
    if let Err(e) = execute_safe(args, &OutputConfig::default()).await {
        e.print_stderr();
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting. Reverses one or more commits by replaying their inverse
/// changes into the index/worktree and optionally creating new commits.
pub async fn execute_safe(args: RevertArgs, output: &OutputConfig) -> CliResult<()> {
    let result = run_revert(args).await.map_err(CliError::from)?;
    render_revert_output(&result, output)
}

// ── Core execution ───────────────────────────────────────────────────

async fn run_revert(args: RevertArgs) -> Result<RevertOutput, RevertError> {
    util::require_repo().map_err(|_| RevertError::NotInRepo)?;

    if let Head::Detached(_) = Head::current().await {
        return Err(RevertError::DetachedHead);
    }

    let commit_id = resolve_commit(&args.commit)
        .await
        .map_err(|_| RevertError::InvalidCommit(args.commit.clone()))?;

    let (revert_commit_id, files_changed) = revert_single_commit(&commit_id, &args).await?;

    let commit_str = commit_id.to_string();
    Ok(RevertOutput {
        reverted_commit: commit_str.clone(),
        short_reverted: short_display_hash(&commit_str).to_string(),
        new_commit: revert_commit_id.as_ref().map(|id| id.to_string()),
        short_new: revert_commit_id
            .as_ref()
            .map(|id| short_display_hash(&id.to_string()).to_string()),
        no_commit: args.no_commit,
        files_changed,
    })
}

// ── Rendering ────────────────────────────────────────────────────────

fn render_revert_output(result: &RevertOutput, output: &OutputConfig) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("revert", result, output);
    }

    if output.quiet {
        return Ok(());
    }

    if let Some(short_new) = &result.short_new {
        println!("[{}] Revert commit {}", short_new, result.short_reverted,);
    } else {
        println!("Changes staged for revert. Use 'libra commit' to finalize.");
    }
    Ok(())
}

// ── Internal logic (unchanged algorithm) ─────────────────────────────

async fn revert_single_commit(
    commit_id: &ObjectHash,
    args: &RevertArgs,
) -> Result<(Option<ObjectHash>, usize), RevertError> {
    let reverted_commit: Commit =
        load_object(commit_id).map_err(|e| RevertError::LoadObject(e.to_string()))?;

    if reverted_commit.parent_commit_ids.len() > 1 {
        return Err(RevertError::MergeCommitUnsupported);
    }

    let parent_commit_id = if let Some(id) = reverted_commit.parent_commit_ids.first() {
        *id
    } else {
        return revert_root_commit(args).await;
    };

    let parent_commit: Commit =
        load_object(&parent_commit_id).map_err(|e| RevertError::LoadObject(e.to_string()))?;

    let current_head_commit_id = Head::current_commit()
        .await
        .ok_or_else(|| RevertError::LoadObject("could not get current HEAD commit".into()))?;
    let current_commit: Commit =
        load_object(&current_head_commit_id).map_err(|e| RevertError::LoadObject(e.to_string()))?;

    let current_tree: Tree =
        load_object(&current_commit.tree_id).map_err(|e| RevertError::LoadObject(e.to_string()))?;
    let reverted_tree: Tree = load_object(&reverted_commit.tree_id)
        .map_err(|e| RevertError::LoadObject(e.to_string()))?;
    let parent_tree: Tree =
        load_object(&parent_commit.tree_id).map_err(|e| RevertError::LoadObject(e.to_string()))?;

    let mut current_files: std::collections::HashMap<_, _> =
        current_tree.get_plain_items().into_iter().collect();
    let reverted_files: std::collections::HashMap<_, _> =
        reverted_tree.get_plain_items().into_iter().collect();
    let parent_files: std::collections::HashMap<_, _> =
        parent_tree.get_plain_items().into_iter().collect();

    let mut files_changed: usize = 0;

    for (path, &reverted_hash) in &reverted_files {
        let parent_hash = parent_files.get(path);

        if Some(&reverted_hash) == parent_hash {
            continue;
        }

        // Only revert paths that still match the commit being reverted; later
        // edits would be clobbered otherwise, so surface them as conflicts.
        if current_files.get(path) != Some(&reverted_hash) && current_files.contains_key(path) {
            return Err(RevertError::Conflict {
                path: path.display().to_string(),
            });
        }

        if let Some(parent_hash) = parent_hash {
            current_files.insert(path.clone(), *parent_hash);
        } else {
            current_files.remove(path);
        }
        files_changed += 1;
    }

    for (path, &parent_hash) in &parent_files {
        if !reverted_files.contains_key(path) {
            current_files.insert(path.clone(), parent_hash);
            files_changed += 1;
        }
    }

    let final_tree_id = build_tree_from_map(current_files).await?;
    let final_tree: Tree =
        load_object(&final_tree_id).map_err(|e| RevertError::LoadObject(e.to_string()))?;

    let mut new_index = Index::new();
    rebuild_index_from_tree(&final_tree, &mut new_index, "")?;
    let current_index = Index::load(path::index()).unwrap_or_else(|_| Index::new());
    reset_workdir_safely(&current_index, &new_index)?;
    new_index
        .save(path::index())
        .map_err(|e| RevertError::IndexSave(e.to_string()))?;

    if args.no_commit {
        Ok((None, files_changed))
    } else {
        let revert_commit_id =
            create_revert_commit(commit_id, &current_head_commit_id, &final_tree_id).await?;
        Ok((Some(revert_commit_id), files_changed))
    }
}

async fn build_tree_from_map(
    files: std::collections::HashMap<PathBuf, ObjectHash>,
) -> Result<ObjectHash, RevertError> {
    fn build_subtree(
        paths: &std::collections::HashMap<PathBuf, ObjectHash>,
        current_dir: &PathBuf,
    ) -> Result<Tree, RevertError> {
        let mut tree_items = Vec::new();
        let mut subdirs = std::collections::HashMap::new();
        for (path, hash) in paths {
            if let Ok(relative_path) = path.strip_prefix(current_dir) {
                if relative_path.components().count() == 1 {
                    tree_items.push(git_internal::internal::object::tree::TreeItem {
                        mode: git_internal::internal::object::tree::TreeItemMode::Blob,
                        name: path_to_utf8(relative_path)?.to_string(),
                        id: *hash,
                    });
                } else {
                    let subdir_component = relative_path.components().next().ok_or_else(|| {
                        RevertError::LoadObject(format!(
                            "missing path component for {}",
                            path.display()
                        ))
                    })?;
                    let subdir = current_dir.join(subdir_component);
                    subdirs
                        .entry(subdir)
                        .or_insert_with(Vec::new)
                        .push((path.clone(), *hash));
                }
            }
        }
        for (subdir, subdir_files) in subdirs {
            let subdir_tree = build_subtree(&subdir_files.into_iter().collect(), &subdir)?;
            tree_items.push(git_internal::internal::object::tree::TreeItem {
                mode: git_internal::internal::object::tree::TreeItemMode::Tree,
                name: file_name_to_utf8(&subdir)?,
                id: subdir_tree.id,
            });
        }
        Tree::from_tree_items(tree_items).map_err(|e| RevertError::SaveObject(e.to_string()))
    }

    let root_dir = PathBuf::new();
    let root_tree = build_subtree(&files, &root_dir)?;
    save_object(&root_tree, &root_tree.id).map_err(|e| RevertError::SaveObject(e.to_string()))?;
    Ok(root_tree.id)
}

async fn revert_root_commit(args: &RevertArgs) -> Result<(Option<ObjectHash>, usize), RevertError> {
    let new_index = Index::new();
    let current_index = Index::load(path::index()).unwrap_or_else(|_| Index::new());
    let files_changed = current_index.tracked_files().len();
    reset_workdir_safely(&current_index, &new_index)?;

    new_index
        .save(path::index())
        .map_err(|e| RevertError::IndexSave(e.to_string()))?;

    if args.no_commit {
        Ok((None, files_changed))
    } else {
        let current_head = Head::current_commit()
            .await
            .ok_or_else(|| RevertError::LoadObject("failed to resolve current HEAD".into()))?;
        let revert_commit_id = create_empty_revert_commit(&current_head).await?;
        Ok((Some(revert_commit_id), files_changed))
    }
}

fn rebuild_index_from_tree(
    tree: &Tree,
    index: &mut Index,
    prefix: &str,
) -> Result<(), RevertError> {
    for item in &tree.tree_items {
        let full_path = if prefix.is_empty() {
            PathBuf::from(&item.name)
        } else {
            PathBuf::from(prefix).join(&item.name)
        };

        if let TreeItemMode::Tree = item.mode {
            let subtree: Tree =
                load_object(&item.id).map_err(|e| RevertError::LoadObject(e.to_string()))?;
            let full_path_str = full_path.to_str().ok_or_else(|| {
                RevertError::LoadObject(format!("failed to convert path to UTF-8: {full_path:?}"))
            })?;
            rebuild_index_from_tree(&subtree, index, full_path_str)?;
        } else {
            let blob = git_internal::internal::object::blob::Blob::load(&item.id);
            let entry = IndexEntry::new_from_blob(
                full_path
                    .to_str()
                    .ok_or_else(|| {
                        RevertError::LoadObject(format!(
                            "failed to convert path to UTF-8: {full_path:?}"
                        ))
                    })?
                    .to_string(),
                item.id,
                blob.data.len() as u32,
            );
            index.add(entry);
        }
    }
    Ok(())
}

fn reset_workdir_safely(current_index: &Index, new_index: &Index) -> Result<(), RevertError> {
    let workdir = util::working_dir();
    let new_tracked_paths: HashSet<_> = new_index.tracked_files().into_iter().collect();

    for path_buf in current_index.tracked_files() {
        if !new_tracked_paths.contains(&path_buf) {
            let full_path = workdir.join(path_buf);
            if full_path.exists() {
                fs::remove_file(&full_path).map_err(|e| {
                    RevertError::WriteWorktree(format!(
                        "failed to remove '{}': {e}",
                        full_path.display()
                    ))
                })?;
            }
        }
    }

    for path_buf in new_index.tracked_files() {
        let path_str = path_to_utf8(&path_buf)?;
        if let Some(entry) = new_index.get(path_str, 0) {
            let blob = git_internal::internal::object::blob::Blob::load(&entry.hash);
            let target_path = workdir.join(path_str);
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent).map_err(|e| {
                    RevertError::WriteWorktree(format!(
                        "failed to create directory '{}': {e}",
                        parent.display()
                    ))
                })?;
            }
            fs::write(&target_path, &blob.data).map_err(|e| {
                RevertError::WriteWorktree(format!(
                    "failed to write '{}': {e}",
                    target_path.display()
                ))
            })?;
        }
    }

    Ok(())
}

fn path_to_utf8(path: &Path) -> Result<&str, RevertError> {
    path.to_str().ok_or_else(|| {
        RevertError::LoadObject(format!("invalid path encoding: {}", path.display()))
    })
}

fn file_name_to_utf8(path: &Path) -> Result<String, RevertError> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            RevertError::LoadObject(format!("invalid file name encoding: {}", path.display()))
        })
}

async fn create_revert_commit(
    reverted_commit_id: &ObjectHash,
    parent_id: &ObjectHash,
    tree_id: &ObjectHash,
) -> Result<ObjectHash, RevertError> {
    let reverted_commit: Commit =
        load_object(reverted_commit_id).map_err(|e| RevertError::LoadObject(e.to_string()))?;

    let revert_message = format!(
        "Revert \"{}\"\n\nThis reverts commit {}.",
        reverted_commit.message.lines().next().unwrap_or(""),
        reverted_commit_id
    );

    let commit = Commit::from_tree_id(
        *tree_id,
        vec![*parent_id],
        &format_commit_msg(&revert_message, None),
    );

    save_object(&commit, &commit.id).map_err(|e| RevertError::SaveObject(e.to_string()))?;
    update_head(&commit.id.to_string()).await?;
    Ok(commit.id)
}

async fn create_empty_revert_commit(parent_id: &ObjectHash) -> Result<ObjectHash, RevertError> {
    let empty_tree =
        Tree::from_tree_items(Vec::new()).map_err(|e| RevertError::SaveObject(e.to_string()))?;
    save_object(&empty_tree, &empty_tree.id).map_err(|e| RevertError::SaveObject(e.to_string()))?;

    let revert_message = "Revert root commit\n\nThis reverts the initial commit.";
    let commit = Commit::from_tree_id(
        empty_tree.id,
        vec![*parent_id],
        &format_commit_msg(revert_message, None),
    );

    save_object(&commit, &commit.id).map_err(|e| RevertError::SaveObject(e.to_string()))?;
    update_head(&commit.id.to_string()).await?;
    Ok(commit.id)
}

async fn resolve_commit(reference: &str) -> Result<ObjectHash, String> {
    util::get_commit_base(reference).await
}

async fn update_head(commit_id: &str) -> Result<(), RevertError> {
    if let Head::Branch(name) = Head::current().await {
        Branch::update_branch(&name, commit_id, None)
            .await
            .map_err(|e| RevertError::UpdateHead(e.to_string()))?;
    }
    Ok(())
}
