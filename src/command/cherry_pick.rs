//! Applies commits onto the current branch by replaying their changes into the index/worktree and emitting new commits or conflict notices.

use std::{
    collections::{HashMap, HashSet},
    fs,
    io::IsTerminal,
    path::{Path, PathBuf},
};

use clap::Parser;
use git_internal::{
    hash::ObjectHash,
    internal::{
        index::{Index, IndexEntry},
        object::{
            ObjectTrait,
            commit::Commit,
            tree::{Tree, TreeItem, TreeItemMode},
            types::ObjectType,
        },
    },
};
use sea_orm::ConnectionTrait;
use serde::Serialize;

use crate::{
    command::{load_object, merge, save_object},
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

    #[error("cherry-picking merge commits is not supported")]
    MergeCommitUnsupported,

    #[error("unsupported cherry-pick option: {0}")]
    Unsupported(String),

    #[error("commit {0} is empty (its change set is empty)")]
    EmptyCommit(String),

    #[error("commit {0} became redundant after replay (no changes to apply)")]
    RedundantCommit(String),

    #[error("commit {0} has an empty commit message")]
    EmptyMessage(String),

    #[error("failed to cherry-pick {commit}: {reason}")]
    Conflict { commit: String, reason: String },

    #[error("failed to load cherry-pick state: {0}")]
    LoadObject(String),

    #[error("failed to update cherry-pick state: {0}")]
    SaveFailed(String),
}

impl CherryPickError {
    fn stable_code(&self) -> StableErrorCode {
        match self {
            Self::NotInRepo => StableErrorCode::RepoNotFound,
            Self::DetachedHead => StableErrorCode::RepoStateInvalid,
            Self::InvalidCommit(_) => StableErrorCode::CliInvalidTarget,
            Self::MergeCommitUnsupported => StableErrorCode::CliInvalidArguments,
            Self::Unsupported(_) => StableErrorCode::Unsupported,
            Self::EmptyCommit(_) => StableErrorCode::CliInvalidArguments,
            Self::RedundantCommit(_) => StableErrorCode::CliInvalidArguments,
            Self::EmptyMessage(_) => StableErrorCode::CliInvalidArguments,
            Self::Conflict { .. } => StableErrorCode::ConflictUnresolved,
            Self::LoadObject(_) => StableErrorCode::IoReadFailed,
            Self::SaveFailed(_) => StableErrorCode::IoWriteFailed,
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
            CherryPickError::MergeCommitUnsupported => CliError::fatal(message)
                .with_stable_code(stable_code)
                .with_hint("specify -m <parent-number> to cherry-pick a merge commit"),
            CherryPickError::Unsupported(_) => CliError::fatal(message)
                .with_stable_code(stable_code)
                .with_hint("this Git option is not supported by libra cherry-pick"),
            CherryPickError::EmptyCommit(_) => CliError::fatal(message)
                .with_stable_code(stable_code)
                .with_hint("use --allow-empty to cherry-pick an empty commit"),
            CherryPickError::RedundantCommit(_) => CliError::fatal(message)
                .with_stable_code(stable_code)
                .with_hint("use --keep-redundant-commits to keep the redundant commit"),
            CherryPickError::EmptyMessage(_) => CliError::fatal(message)
                .with_stable_code(stable_code)
                .with_hint("use --allow-empty-message to cherry-pick with an empty message"),
            CherryPickError::Conflict { .. } => CliError::failure(message)
                .with_stable_code(stable_code)
                .with_hint("resolve conflicts manually, then use 'libra commit'"),
            CherryPickError::LoadObject(_) => CliError::fatal(message)
                .with_stable_code(stable_code)
                .with_hint("check repository integrity and retry"),
            CherryPickError::SaveFailed(_) => CliError::fatal(message)
                .with_stable_code(stable_code)
                .with_hint("check filesystem permissions and repository writability"),
        }
    }
}

#[derive(Debug)]
enum CherryPickSingleError {
    MergeCommitUnsupported,
    EmptyCommit(String),
    RedundantCommit(String),
    EmptyMessage(String),
    Conflict(String),
    LoadObject(String),
    SaveFailed(String),
}

// ── Structured output ────────────────────────────────────────────────

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
#[derive(Parser, Debug, Default)]
#[command(about = "Apply the changes introduced by some existing commits")]
#[command(after_help = CHERRY_PICK_EXAMPLES)]
pub struct CherryPickArgs {
    /// Commits to cherry-pick
    #[clap(required = true)]
    pub commits: Vec<String>,

    /// Don't automatically commit the cherry-pick
    #[clap(short = 'n', long)]
    pub no_commit: bool,

    /// Append a "(cherry picked from commit <oid>)" line to the commit message
    #[clap(short = 'x')]
    pub append_source: bool,

    /// Add a Signed-off-by trailer to the commit message
    #[clap(short = 's', long = "signoff", overrides_with = "no_signoff")]
    pub signoff: bool,

    /// Edit the commit message before committing
    #[clap(short = 'e', long = "edit", overrides_with = "no_edit")]
    pub edit: bool,

    /// Cherry-pick a commit even if its own change set is empty
    #[clap(long = "allow-empty", overrides_with = "no_allow_empty")]
    pub allow_empty: bool,

    /// Cherry-pick a commit even if its message is empty
    #[clap(
        long = "allow-empty-message",
        overrides_with = "no_allow_empty_message"
    )]
    pub allow_empty_message: bool,

    /// Keep commits that become redundant (empty) after being replayed
    #[clap(
        long = "keep-redundant-commits",
        overrides_with = "no_keep_redundant_commits"
    )]
    pub keep_redundant_commits: bool,

    // ── Negative (reset-to-default) forms; last flag wins, never an error ──
    #[clap(long = "no-signoff", overrides_with = "signoff", hide = true)]
    pub no_signoff: bool,
    #[clap(long = "no-edit", overrides_with = "edit", hide = true)]
    pub no_edit: bool,
    #[clap(long = "no-allow-empty", overrides_with = "allow_empty", hide = true)]
    pub no_allow_empty: bool,
    #[clap(
        long = "no-allow-empty-message",
        overrides_with = "allow_empty_message",
        hide = true
    )]
    pub no_allow_empty_message: bool,
    #[clap(
        long = "no-keep-redundant-commits",
        overrides_with = "keep_redundant_commits",
        hide = true
    )]
    pub no_keep_redundant_commits: bool,

    // ── Unsupported Git options captured for explicit rejection ──
    #[clap(long = "empty", value_name = "mode", hide = true)]
    pub empty: Option<String>,
    #[clap(long = "cleanup", value_name = "mode", hide = true)]
    pub cleanup: Option<String>,
    #[clap(
        long = "rerere-autoupdate",
        overrides_with = "no_rerere_autoupdate",
        hide = true
    )]
    pub rerere_autoupdate: bool,
    #[clap(
        long = "no-rerere-autoupdate",
        overrides_with = "rerere_autoupdate",
        hide = true
    )]
    pub no_rerere_autoupdate: bool,
    #[clap(long = "commit", hide = true)]
    pub commit: bool,
    #[clap(
        short = 'X',
        long = "strategy-option",
        value_name = "option",
        hide = true
    )]
    pub strategy_option: Option<String>,
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
    let result = run_cherry_pick(args, output)
        .await
        .map_err(CliError::from)?;
    render_cherry_pick_output(&result, output)
}

// ── Core execution ───────────────────────────────────────────────────

/// Reject Git options libra cherry-pick does not implement. Returns the first
/// offending flag (so the error names a concrete option) or `None`.
fn reject_unsupported_options(args: &CherryPickArgs) -> Option<&'static str> {
    if args.empty.is_some() {
        return Some("--empty (use --allow-empty / --keep-redundant-commits)");
    }
    if args.cleanup.is_some() {
        return Some("--cleanup");
    }
    if args.rerere_autoupdate {
        return Some("--rerere-autoupdate");
    }
    if args.commit {
        return Some("--commit (auto-commit is the default; use -n to stage only)");
    }
    if args.strategy_option.is_some() {
        return Some("-X / --strategy-option");
    }
    None
}

async fn run_cherry_pick(
    args: CherryPickArgs,
    output: &OutputConfig,
) -> Result<CherryPickOutput, CherryPickError> {
    util::require_repo().map_err(|_| CherryPickError::NotInRepo)?;

    if let Some(flag) = reject_unsupported_options(&args) {
        return Err(CherryPickError::Unsupported(flag.to_string()));
    }

    if let Head::Detached(_) = Head::current().await {
        return Err(CherryPickError::DetachedHead);
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
        match cherry_pick_single_commit(commit_id, &args, output).await {
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
            Err(CherryPickSingleError::MergeCommitUnsupported) => {
                return Err(CherryPickError::MergeCommitUnsupported);
            }
            Err(CherryPickSingleError::EmptyCommit(commit)) => {
                return Err(CherryPickError::EmptyCommit(commit));
            }
            Err(CherryPickSingleError::RedundantCommit(commit)) => {
                return Err(CherryPickError::RedundantCommit(commit));
            }
            Err(CherryPickSingleError::EmptyMessage(commit)) => {
                return Err(CherryPickError::EmptyMessage(commit));
            }
            Err(CherryPickSingleError::Conflict(reason)) => {
                return Err(CherryPickError::Conflict {
                    commit: args.commits[i].clone(),
                    reason,
                });
            }
            Err(CherryPickSingleError::LoadObject(reason)) => {
                return Err(CherryPickError::LoadObject(reason));
            }
            Err(CherryPickSingleError::SaveFailed(reason)) => {
                return Err(CherryPickError::SaveFailed(reason));
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
    output: &OutputConfig,
) -> Result<Option<ObjectHash>, CherryPickSingleError> {
    let commit_to_pick: Commit =
        load_object(commit_id).map_err(|e| CherryPickSingleError::LoadObject(e.to_string()))?;

    if commit_to_pick.parent_commit_ids.len() > 1 {
        return Err(CherryPickSingleError::MergeCommitUnsupported);
    }

    let parent_tree = if commit_to_pick.parent_commit_ids.is_empty() {
        let empty_id = ObjectHash::from_type_and_data(ObjectType::Tree, &[]);
        Tree::from_bytes(&[], empty_id).map_err(|e| {
            CherryPickSingleError::SaveFailed(format!(
                "failed to create empty tree for root commit: {e}",
            ))
        })?
    } else {
        let parent_commit: Commit =
            load_object(&commit_to_pick.parent_commit_ids[0]).map_err(|e| {
                CherryPickSingleError::LoadObject(format!("failed to load parent commit: {e}"))
            })?;
        load_object(&parent_commit.tree_id).map_err(|e| {
            CherryPickSingleError::LoadObject(format!("failed to load parent tree: {e}"))
        })?
    };

    let their_tree: Tree = load_object(&commit_to_pick.tree_id).map_err(|e| {
        CherryPickSingleError::LoadObject(format!("failed to load commit tree: {e}"))
    })?;

    // (A) "Empty" class 1: the picked commit's own change set is empty (its tree
    // equals its parent tree). Git blocks this unless `--allow-empty`. Checked
    // before any index/worktree mutation so a blocked pick leaves state intact.
    let originally_empty = commit_to_pick.tree_id == parent_tree.id;
    if originally_empty && !args.allow_empty {
        return Err(CherryPickSingleError::EmptyCommit(commit_id.to_string()));
    }

    let index_file = path::index();
    let current_index = Index::load(&index_file).map_err(|e| {
        CherryPickSingleError::LoadObject(format!("failed to load current index: {e}"))
    })?;
    let mut index = Index::load(&index_file).map_err(|e| {
        CherryPickSingleError::LoadObject(format!("failed to load current index: {e}"))
    })?;

    let diff = diff_trees(&their_tree, &parent_tree);

    for (path, their_hash, base_hash) in diff {
        match (their_hash, base_hash) {
            (Some(th), Some(_bh)) => {
                update_index_entry(&mut index, &path, th)?;
            }
            (Some(th), None) => {
                update_index_entry(&mut index, &path, th)?;
            }
            (None, Some(_bh)) => {
                index.remove(path_to_utf8(&path)?, 0);
            }
            (None, None) => continue,
        }
    }

    // Build the candidate tree first (saves tree objects, but does NOT touch the
    // on-disk index or worktree yet) so the "redundant after replay" check can
    // bail out before mutating any state.
    let tree_id = create_tree_from_index(&index)?;

    if args.no_commit {
        index
            .save(&index_file)
            .map_err(|e| CherryPickSingleError::SaveFailed(format!("failed to save index: {e}")))?;
        reset_workdir_tracked_only(&current_index, &index)?;
        return Ok(None);
    }

    let current_head = Head::current_commit().await.ok_or_else(|| {
        CherryPickSingleError::LoadObject("failed to resolve current HEAD".to_string())
    })?;

    // (B) "Empty" class 2: the replayed change is redundant against the current
    // HEAD (resulting tree is identical). Git stops unless `--keep-redundant-commits`.
    // An originally-empty commit that reached here has already passed `--allow-empty`,
    // so it is allowed through even though its tree is unchanged.
    let head_commit: Commit = load_object(&current_head).map_err(|e| {
        CherryPickSingleError::LoadObject(format!("failed to load current HEAD commit: {e}"))
    })?;
    if tree_id == head_commit.tree_id && !originally_empty && !args.keep_redundant_commits {
        return Err(CherryPickSingleError::RedundantCommit(
            commit_id.to_string(),
        ));
    }

    index
        .save(&index_file)
        .map_err(|e| CherryPickSingleError::SaveFailed(format!("failed to save index: {e}")))?;
    reset_workdir_tracked_only(&current_index, &index)?;

    let cherry_pick_commit_id =
        create_cherry_pick_commit(&commit_to_pick, &current_head, tree_id, args, output).await?;
    Ok(Some(cherry_pick_commit_id))
}

/// Assemble the cherry-pick commit message, honoring `-x` (append source line),
/// `-s` (Signed-off-by trailer, in that order), and `-e` (interactive edit).
async fn build_cherry_pick_message(
    original_commit: &Commit,
    args: &CherryPickArgs,
    output: &OutputConfig,
) -> Result<String, CherryPickSingleError> {
    let mut message = original_commit.message.trim().to_string();

    // Trailer block: `-x` line first, `Signed-off-by` last (matches Git).
    let mut trailers: Vec<String> = Vec::new();
    if args.append_source {
        let line = format!("(cherry picked from commit {})", original_commit.id);
        if !message.contains(&line) {
            trailers.push(line);
        }
    }
    if args.signoff {
        let (name, email) = merge::resolve_signoff_identity().await.map_err(|e| {
            CherryPickSingleError::SaveFailed(format!("failed to resolve sign-off identity: {e}"))
        })?;
        let line = format!("Signed-off-by: {name} <{email}>");
        if !message.contains(&line) {
            trailers.push(line);
        }
    }
    if !trailers.is_empty() {
        message.push_str("\n\n");
        message.push_str(&trailers.join("\n"));
    }

    // `-e`: only launch an editor on an interactive TTY and never in machine/JSON
    // mode (`--machine`/`--json`); otherwise degrade to the assembled message.
    if args.edit && !output.is_json() && std::io::stdin().is_terminal() {
        message = maybe_edit_cherry_pick_message(&message).await?;
    }

    Ok(message)
}

/// Launch the resolved editor on a scratch message file, mirroring merge's
/// `maybe_edit_message`. A missing or failing editor leaves the message intact.
async fn maybe_edit_cherry_pick_message(message: &str) -> Result<String, CherryPickSingleError> {
    let Some(editor) = merge::resolve_editor().await else {
        return Ok(message.to_string());
    };
    let path = util::storage_path().join("CHERRY_PICK_MSG");
    fs::write(&path, message).map_err(|e| {
        CherryPickSingleError::SaveFailed(format!(
            "failed to write edit buffer {}: {e}",
            path.display()
        ))
    })?;
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"{}\"", path.display()))
        .status();
    match status {
        Ok(code) if code.success() => fs::read_to_string(&path).map_err(|e| {
            CherryPickSingleError::LoadObject(format!("failed to read edited message: {e}"))
        }),
        _ => Ok(message.to_string()),
    }
}

async fn create_cherry_pick_commit(
    original_commit: &Commit,
    parent_id: &ObjectHash,
    tree_id: ObjectHash,
    args: &CherryPickArgs,
    output: &OutputConfig,
) -> Result<ObjectHash, CherryPickSingleError> {
    let message = build_cherry_pick_message(original_commit, args, output).await?;

    if message.trim().is_empty() && !args.allow_empty_message {
        return Err(CherryPickSingleError::EmptyMessage(
            original_commit.id.to_string(),
        ));
    }

    let commit = Commit::from_tree_id(
        tree_id,
        vec![*parent_id],
        &format_commit_msg(&message, None),
    );

    save_object(&commit, &commit.id)
        .map_err(|e| CherryPickSingleError::SaveFailed(format!("failed to save commit: {e}")))?;

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
    .map_err(|e| {
        CherryPickSingleError::SaveFailed(format!("failed to update branch and reflog: {e}"))
    })?;
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

fn update_index_entry(
    index: &mut Index,
    path: &Path,
    hash: ObjectHash,
) -> Result<(), CherryPickSingleError> {
    let blob = git_internal::internal::object::blob::Blob::load(&hash);
    let entry = IndexEntry::new_from_blob(
        path_to_utf8(path)?.to_string(),
        hash,
        blob.data.len() as u32,
    );
    index.add(entry);
    Ok(())
}

fn create_tree_from_index(index: &Index) -> Result<ObjectHash, CherryPickSingleError> {
    let mut entries_map: HashMap<PathBuf, Vec<TreeItem>> = HashMap::new();
    for path_buf in index.tracked_files() {
        let path_str = path_to_utf8(&path_buf)?;
        if let Some(entry) = index.get(path_str, 0) {
            let item = TreeItem {
                mode: match entry.mode {
                    0o100644 => TreeItemMode::Blob,
                    0o100755 => TreeItemMode::BlobExecutable,
                    0o120000 => TreeItemMode::Link,
                    0o040000 => TreeItemMode::Tree,
                    _ => {
                        return Err(CherryPickSingleError::SaveFailed(format!(
                            "unsupported file mode: {:#o}",
                            entry.mode
                        )));
                    }
                },
                name: file_name_to_utf8(&path_buf)?,
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
) -> Result<ObjectHash, CherryPickSingleError> {
    let mut current_items = entries_map.remove(current_path).unwrap_or_default();

    let subdirs: Vec<_> = entries_map
        .keys()
        .filter(|p| p.parent() == Some(current_path))
        .cloned()
        .collect();

    for subdir_path in subdirs {
        let subdir_name = file_name_to_utf8(&subdir_path)?;
        let subtree_hash = build_tree_recursively(&subdir_path, entries_map)?;
        current_items.push(TreeItem {
            mode: TreeItemMode::Tree,
            name: subdir_name,
            id: subtree_hash,
        });
    }

    crate::utils::tree::sort_tree_items_for_git(&mut current_items);
    let tree = Tree::from_tree_items(current_items)
        .map_err(|e| CherryPickSingleError::SaveFailed(format!("failed to create tree: {e}")))?;
    save_object(&tree, &tree.id).map_err(|e| CherryPickSingleError::SaveFailed(e.to_string()))?;
    Ok(tree.id)
}

fn reset_workdir_tracked_only(
    current_index: &Index,
    new_index: &Index,
) -> Result<(), CherryPickSingleError> {
    let workdir = util::working_dir();
    let untracked_paths = worktree::untracked_workdir_paths(current_index).map_err(|e| {
        CherryPickSingleError::LoadObject(format!("failed to inspect untracked files: {e}"))
    })?;
    if let Some(conflict) = worktree::untracked_overwrite_path(&untracked_paths, new_index) {
        return Err(CherryPickSingleError::Conflict(format!(
            "untracked working tree file would be overwritten: {}",
            conflict.display()
        )));
    }
    let new_tracked_paths: HashSet<_> = new_index.tracked_files().into_iter().collect();

    for path_buf in current_index.tracked_files() {
        if !new_tracked_paths.contains(&path_buf) {
            let full_path = workdir.join(path_buf);
            if full_path.exists() {
                fs::remove_file(&full_path).map_err(|e| {
                    CherryPickSingleError::SaveFailed(format!(
                        "failed to remove file '{}': {e}",
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
                    CherryPickSingleError::SaveFailed(format!(
                        "failed to create parent directory '{}': {e}",
                        parent.display()
                    ))
                })?;
            }
            fs::write(&target_path, &blob.data).map_err(|e| {
                CherryPickSingleError::SaveFailed(format!(
                    "failed to write file '{}': {e}",
                    target_path.display()
                ))
            })?;
        }
    }
    Ok(())
}

fn path_to_utf8(path: &Path) -> Result<&str, CherryPickSingleError> {
    path.to_str().ok_or_else(|| {
        CherryPickSingleError::LoadObject(format!("invalid path encoding: {}", path.display()))
    })
}

fn file_name_to_utf8(path: &Path) -> Result<String, CherryPickSingleError> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            CherryPickSingleError::LoadObject(format!(
                "invalid file name encoding: {}",
                path.display()
            ))
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Pin the `Display` format for every variant of [`CherryPickError`].
    /// These strings are used as the `CliError` message via
    /// `From<CherryPickError> for CliError` and surface in both human
    /// and `--json` envelopes for the `cherry-pick` subcommand.
    ///
    /// All variants are pinned because every variant carries either a
    /// static message or an explicit `{0}` field interpolation; none
    /// wrap an upstream source error directly.
    #[test]
    fn cherry_pick_error_display_pins_each_variant() {
        assert_eq!(
            CherryPickError::NotInRepo.to_string(),
            "not a libra repository",
        );
        assert_eq!(
            CherryPickError::DetachedHead.to_string(),
            "cannot cherry-pick on detached HEAD",
        );
        assert_eq!(
            CherryPickError::InvalidCommit("deadbeef".to_string()).to_string(),
            "failed to resolve commit reference 'deadbeef'",
        );
        assert_eq!(
            CherryPickError::MergeCommitUnsupported.to_string(),
            "cherry-picking merge commits is not supported",
        );
        assert_eq!(
            CherryPickError::Unsupported("--cleanup".to_string()).to_string(),
            "unsupported cherry-pick option: --cleanup",
        );
        assert_eq!(
            CherryPickError::EmptyCommit("abc123".to_string()).to_string(),
            "commit abc123 is empty (its change set is empty)",
        );
        assert_eq!(
            CherryPickError::RedundantCommit("abc123".to_string()).to_string(),
            "commit abc123 became redundant after replay (no changes to apply)",
        );
        assert_eq!(
            CherryPickError::EmptyMessage("abc123".to_string()).to_string(),
            "commit abc123 has an empty commit message",
        );
        assert_eq!(
            CherryPickError::Conflict {
                commit: "abc123".to_string(),
                reason: "untracked file would be overwritten".to_string(),
            }
            .to_string(),
            "failed to cherry-pick abc123: untracked file would be overwritten",
        );
        assert_eq!(
            CherryPickError::LoadObject("object not found".to_string()).to_string(),
            "failed to load cherry-pick state: object not found",
        );
        assert_eq!(
            CherryPickError::SaveFailed("disk full".to_string()).to_string(),
            "failed to update cherry-pick state: disk full",
        );
    }

    /// Pin the `stable_code()` mapping for every variant of
    /// [`CherryPickError`]. This is the second public surface contract:
    /// the [`StableErrorCode`] value is what `--json` consumers read
    /// from the `code` field of the error envelope and branch on
    /// (e.g. retry on `IoReadFailed`, surface a typed hint on
    /// `ConflictUnresolved`). A future refactor that re-routes a
    /// variant — for example flipping `MultipleWithNoCommit` from
    /// `CliInvalidArguments` to `CliInvalidTarget` — silently changes
    /// the wire surface unless every variant has its own guard.
    ///
    /// Enumerate every variant explicitly so adding a new variant
    /// trips the exhaustive match below (the compiler enforces it
    /// alongside the `stable_code()` match in the impl), and silently
    /// changing an existing variant's code trips the assertion.
    #[test]
    fn cherry_pick_error_stable_code_pins_each_variant() {
        assert_eq!(
            CherryPickError::NotInRepo.stable_code(),
            StableErrorCode::RepoNotFound,
        );
        assert_eq!(
            CherryPickError::DetachedHead.stable_code(),
            StableErrorCode::RepoStateInvalid,
        );
        assert_eq!(
            CherryPickError::InvalidCommit("deadbeef".to_string()).stable_code(),
            StableErrorCode::CliInvalidTarget,
        );
        assert_eq!(
            CherryPickError::MergeCommitUnsupported.stable_code(),
            StableErrorCode::CliInvalidArguments,
        );
        assert_eq!(
            CherryPickError::Unsupported("--cleanup".to_string()).stable_code(),
            StableErrorCode::Unsupported,
        );
        assert_eq!(
            CherryPickError::EmptyCommit("abc123".to_string()).stable_code(),
            StableErrorCode::CliInvalidArguments,
        );
        assert_eq!(
            CherryPickError::RedundantCommit("abc123".to_string()).stable_code(),
            StableErrorCode::CliInvalidArguments,
        );
        assert_eq!(
            CherryPickError::EmptyMessage("abc123".to_string()).stable_code(),
            StableErrorCode::CliInvalidArguments,
        );
        assert_eq!(
            CherryPickError::Conflict {
                commit: "abc123".to_string(),
                reason: "ignored".to_string(),
            }
            .stable_code(),
            StableErrorCode::ConflictUnresolved,
        );
        assert_eq!(
            CherryPickError::LoadObject("ignored".to_string()).stable_code(),
            StableErrorCode::IoReadFailed,
        );
        assert_eq!(
            CherryPickError::SaveFailed("ignored".to_string()).stable_code(),
            StableErrorCode::IoWriteFailed,
        );
    }
}
