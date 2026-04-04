//! Applies commits onto the current branch by replaying their changes into the index/worktree and emitting new commits or conflict notices.

use std::{
    collections::{HashMap, HashSet},
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
            tree::{Tree, TreeItem, TreeItemMode},
        },
    },
};
use sea_orm::ConnectionTrait;
use serde::Serialize;

use crate::{
    command::{load_object, save_object},
    common_utils::format_commit_msg,
    internal::{
        branch::Branch,
        head::Head,
        reflog::{ReflogAction, ReflogContext, with_reflog},
    },
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        object_ext::{BlobExt, TreeExt},
        output::{OutputConfig, emit_json_data},
        path,
        text::short_display_hash,
        util, worktree,
    },
};

const CHERRY_PICK_EXAMPLES: &str = "\
EXAMPLES:
    libra cherry-pick abc1234              Apply a single commit
    libra cherry-pick abc1234 def5678      Apply multiple commits in order
    libra cherry-pick -n abc1234           Apply without auto-committing
    libra cherry-pick --json abc1234       Structured JSON output for agents";

// ── Typed error ──────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
enum CherryPickError {
    #[error("not a libra repository")]
    NotInRepo,

    #[error("cannot cherry-pick on detached HEAD")]
    DetachedHead,

    #[error("failed to resolve commit reference '{0}'")]
    InvalidCommit(String),

    #[error("cannot cherry-pick multiple commits with --no-commit")]
    MultipleWithNoCommit,

    #[error("failed to cherry-pick {commit}: {reason}")]
    Conflict { commit: String, reason: String },
}

impl CherryPickError {
    fn stable_code(&self) -> StableErrorCode {
        match self {
            Self::NotInRepo => StableErrorCode::RepoNotFound,
            Self::DetachedHead => StableErrorCode::RepoStateInvalid,
            Self::InvalidCommit(_) => StableErrorCode::CliInvalidTarget,
            Self::MultipleWithNoCommit => StableErrorCode::CliInvalidArguments,
            Self::Conflict { .. } => StableErrorCode::ConflictUnresolved,
        }
    }
}

impl From<CherryPickError> for CliError {
    fn from(error: CherryPickError) -> Self {
        let stable_code = error.stable_code();
        let message = error.to_string();
        match error {
            CherryPickError::NotInRepo => CliError::repo_not_found(),
            CherryPickError::DetachedHead => CliError::fatal(message)
                .with_stable_code(stable_code)
                .with_hint("switch to a branch first with 'libra switch <branch>'"),
            CherryPickError::InvalidCommit(_) => CliError::fatal(message)
                .with_stable_code(stable_code)
                .with_hint("use 'libra log' to find valid commit references"),
            CherryPickError::MultipleWithNoCommit => CliError::fatal(message)
                .with_stable_code(stable_code)
                .with_hint("use 'libra commit' to save the changes from the first cherry-pick"),
            CherryPickError::Conflict { .. } => CliError::failure(message)
                .with_stable_code(stable_code)
                .with_hint("resolve conflicts manually, then use 'libra commit'"),
        }
    }
}

// ─�� Structured output ────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct CherryPickOutput {
    pub picked: Vec<CherryPickEntry>,
    pub no_commit: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct CherryPickEntry {
    pub source_commit: String,
    pub short_source: String,
    pub new_commit: Option<String>,
    pub short_new: Option<String>,
}

// ── Entry points ─────────────────────────────────────────────────────

/// Arguments for the cherry-pick command
#[derive(Parser, Debug)]
#[command(about = "Apply the changes introduced by some existing commits")]
#[command(after_help = CHERRY_PICK_EXAMPLES)]
pub struct CherryPickArgs {
    /// Commits to cherry-pick
    #[clap(required = true)]
    pub commits: Vec<String>,

    /// Don't automatically commit the cherry-pick
    #[clap(short = 'n', long)]
    pub no_commit: bool,
}

pub async fn execute(args: CherryPickArgs) {
    if let Err(e) = execute_safe(args, &OutputConfig::default()).await {
        e.print_stderr();
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting. Replays one or more commit changes onto the current
/// branch, optionally creating new commits or leaving them staged.
pub async fn execute_safe(args: CherryPickArgs, output: &OutputConfig) -> CliResult<()> {
    let result = run_cherry_pick(args).await.map_err(CliError::from)?;
    render_cherry_pick_output(&result, output)
}

// ── Core execution ───────────────────────────────────────────────────

async fn run_cherry_pick(args: CherryPickArgs) -> Result<CherryPickOutput, CherryPickError> {
    util::require_repo().map_err(|_| CherryPickError::NotInRepo)?;

    if let Head::Detached(_) = Head::current().await {
        return Err(CherryPickError::DetachedHead);
    }

    if args.no_commit && args.commits.len() > 1 {
        return Err(CherryPickError::MultipleWithNoCommit);
    }

    let mut commit_ids = Vec::new();
    for commit_ref in &args.commits {
        let id = resolve_commit(commit_ref)
            .await
            .map_err(|_| CherryPickError::InvalidCommit(commit_ref.clone()))?;
        commit_ids.push(id);
    }

    let mut picked = Vec::new();
    for (i, commit_id) in commit_ids.iter().enumerate() {
        match cherry_pick_single_commit(commit_id, &args).await {
            Ok(new_commit_id) => {
                let source_str = commit_id.to_string();
                picked.push(CherryPickEntry {
                    source_commit: source_str.clone(),
                    short_source: short_display_hash(&source_str).to_string(),
                    new_commit: new_commit_id.as_ref().map(|id| id.to_string()),
                    short_new: new_commit_id
                        .as_ref()
                        .map(|id| short_display_hash(&id.to_string()).to_string()),
                });
            }
            Err(reason) => {
                return Err(CherryPickError::Conflict {
                    commit: args.commits[i].clone(),
                    reason,
                });
            }
        }
    }

    Ok(CherryPickOutput {
        picked,
        no_commit: args.no_commit,
    })
}

// ── Rendering ────────────────────────────────────────────────────────

fn render_cherry_pick_output(result: &CherryPickOutput, output: &OutputConfig) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("cherry-pick", result, output);
    }

    if output.quiet {
        return Ok(());
    }

    for entry in &result.picked {
        if let Some(short_new) = &entry.short_new {
            println!("[{}] cherry-picked from {}", short_new, entry.short_source,);
        } else {
            println!(
                "Changes from {} staged. Use 'libra commit' to finalize.",
                entry.short_source,
            );
        }
    }
    Ok(())
}

// ── Internal logic (unchanged algorithm) ─────────────────────────────

async fn cherry_pick_single_commit(
    commit_id: &ObjectHash,
    args: &CherryPickArgs,
) -> Result<Option<ObjectHash>, String> {
    let commit_to_pick: Commit =
        load_object(commit_id).map_err(|e| format!("failed to load commit: {e}"))?;

    if commit_to_pick.parent_commit_ids.len() > 1 {
        return Err("cherry-picking merge commits is not supported".to_string());
    }

    let parent_tree = if commit_to_pick.parent_commit_ids.is_empty() {
        Tree::from_tree_items(vec![]).unwrap()
    } else {
        let parent_commit: Commit = load_object(&commit_to_pick.parent_commit_ids[0])
            .map_err(|e| format!("failed to load parent commit: {e}"))?;
        load_object(&parent_commit.tree_id)
            .map_err(|e| format!("failed to load parent tree: {e}"))?
    };

    let their_tree: Tree = load_object(&commit_to_pick.tree_id)
        .map_err(|e| format!("failed to load commit tree: {e}"))?;

    let index_file = path::index();
    let current_index =
        Index::load(&index_file).map_err(|e| format!("failed to load current index: {e}"))?;
    let mut index =
        Index::load(&index_file).map_err(|e| format!("failed to load current index: {e}"))?;

    let diff = diff_trees(&their_tree, &parent_tree);

    for (path, their_hash, base_hash) in diff {
        match (their_hash, base_hash) {
            (Some(th), Some(_bh)) => {
                update_index_entry(&mut index, &path, th);
            }
            (Some(th), None) => {
                update_index_entry(&mut index, &path, th);
            }
            (None, Some(_bh)) => {
                index.remove(path.to_str().unwrap(), 0);
            }
            (None, None) => unreachable!(),
        }
    }

    index
        .save(&index_file)
        .map_err(|e| format!("failed to save index: {e}"))?;
    reset_workdir_tracked_only(&current_index, &index)?;

    if args.no_commit {
        Ok(None)
    } else {
        let current_head = Head::current_commit()
            .await
            .ok_or("failed to resolve current HEAD")?;
        let cherry_pick_commit_id =
            create_cherry_pick_commit(&commit_to_pick, &current_head).await?;
        Ok(Some(cherry_pick_commit_id))
    }
}

async fn create_cherry_pick_commit(
    original_commit: &Commit,
    parent_id: &ObjectHash,
) -> Result<ObjectHash, String> {
    let index = Index::load(path::index()).map_err(|e| format!("failed to load index: {e}"))?;
    let tree_id = create_tree_from_index(&index)?;

    let cherry_pick_message = format!(
        "{}\n\n(cherry picked from commit {})",
        original_commit.message.trim(),
        original_commit.id
    );

    let commit = Commit::from_tree_id(
        tree_id,
        vec![*parent_id],
        &format_commit_msg(&cherry_pick_message, None),
    );

    save_object(&commit, &commit.id).map_err(|e| format!("failed to save commit: {e}"))?;

    let action = ReflogAction::CherryPick {
        source_message: original_commit.message.clone(),
    };
    let context = ReflogContext {
        old_oid: parent_id.to_string(),
        new_oid: commit.id.to_string(),
        action,
    };

    with_reflog(
        context,
        move |txn| {
            Box::pin(async move {
                update_head(txn, &commit.id.to_string()).await?;
                Ok(())
            })
        },
        true,
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(commit.id)
}

fn diff_trees(
    theirs: &Tree,
    base: &Tree,
) -> Vec<(PathBuf, Option<ObjectHash>, Option<ObjectHash>)> {
    let mut diffs = Vec::new();
    let their_items: HashMap<_, _> = theirs.get_plain_items().into_iter().collect();
    let base_items: HashMap<_, _> = base.get_plain_items().into_iter().collect();

    let all_paths: HashSet<_> = their_items.keys().chain(base_items.keys()).collect();

    for path in all_paths {
        let their_hash = their_items.get(path).cloned();
        let base_hash = base_items.get(path).cloned();
        if their_hash != base_hash {
            diffs.push((path.clone(), their_hash, base_hash));
        }
    }
    diffs
}

fn update_index_entry(index: &mut Index, path: &Path, hash: ObjectHash) {
    let blob = git_internal::internal::object::blob::Blob::load(&hash);
    let entry = IndexEntry::new_from_blob(
        path.to_str().unwrap().to_string(),
        hash,
        blob.data.len() as u32,
    );
    index.add(entry);
}

fn create_tree_from_index(index: &Index) -> Result<ObjectHash, String> {
    let mut entries_map: HashMap<PathBuf, Vec<TreeItem>> = HashMap::new();
    for path_buf in index.tracked_files() {
        let path_str = path_buf.to_str().unwrap();
        if let Some(entry) = index.get(path_str, 0) {
            let item = TreeItem {
                mode: match entry.mode {
                    0o100644 => TreeItemMode::Blob,
                    0o100755 => TreeItemMode::BlobExecutable,
                    0o120000 => TreeItemMode::Link,
                    0o040000 => TreeItemMode::Tree,
                    _ => return Err(format!("unsupported file mode: {:#o}", entry.mode)),
                },
                name: path_buf.file_name().unwrap().to_str().unwrap().to_string(),
                id: entry.hash,
            };
            let parent_dir = path_buf
                .parent()
                .unwrap_or_else(|| Path::new(""))
                .to_path_buf();
            entries_map.entry(parent_dir).or_default().push(item);
        }
    }

    build_tree_recursively(Path::new(""), &mut entries_map)
}

fn build_tree_recursively(
    current_path: &Path,
    entries_map: &mut HashMap<PathBuf, Vec<TreeItem>>,
) -> Result<ObjectHash, String> {
    let mut current_items = entries_map.remove(current_path).unwrap_or_default();

    let subdirs: Vec<_> = entries_map
        .keys()
        .filter(|p| p.parent() == Some(current_path))
        .cloned()
        .collect();

    for subdir_path in subdirs {
        let subdir_name = subdir_path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let subtree_hash = build_tree_recursively(&subdir_path, entries_map)?;
        current_items.push(TreeItem {
            mode: TreeItemMode::Tree,
            name: subdir_name,
            id: subtree_hash,
        });
    }

    let tree =
        Tree::from_tree_items(current_items).map_err(|e| format!("failed to create tree: {e}"))?;
    save_object(&tree, &tree.id).map_err(|e| e.to_string())?;
    Ok(tree.id)
}

fn reset_workdir_tracked_only(current_index: &Index, new_index: &Index) -> Result<(), String> {
    let workdir = util::working_dir();
    let untracked_paths = worktree::untracked_workdir_paths(current_index)?;
    if let Some(conflict) = worktree::untracked_overwrite_path(&untracked_paths, new_index) {
        return Err(format!(
            "untracked working tree file would be overwritten: {}",
            conflict.display()
        ));
    }
    let new_tracked_paths: HashSet<_> = new_index.tracked_files().into_iter().collect();

    for path_buf in current_index.tracked_files() {
        if !new_tracked_paths.contains(&path_buf) {
            let full_path = workdir.join(path_buf);
            if full_path.exists() {
                fs::remove_file(&full_path).map_err(|e| e.to_string())?;
            }
        }
    }

    for path_buf in new_index.tracked_files() {
        let path_str = path_buf.to_str().unwrap();
        if let Some(entry) = new_index.get(path_str, 0) {
            let blob = git_internal::internal::object::blob::Blob::load(&entry.hash);
            let target_path = workdir.join(path_str);
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            fs::write(&target_path, &blob.data).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

async fn resolve_commit(reference: &str) -> Result<ObjectHash, String> {
    util::get_commit_base(reference).await
}

async fn update_head<C: ConnectionTrait>(db: &C, commit_id: &str) -> Result<(), sea_orm::DbErr> {
    if let Head::Branch(name) = Head::current_with_conn(db).await {
        Branch::update_branch_with_conn(db, &name, commit_id, None).await?;
    }
    Ok(())
}
