//! Rebase implementation that parses onto/branch arguments, replays commits onto a new base, handles conflicts, and updates branch refs.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    str::FromStr,
};

use clap::Parser;
use git_internal::{
    hash::ObjectHash,
    internal::object::{blob::Blob, commit::Commit, tree::Tree},
};
use sea_orm::TransactionTrait;

use crate::{
    command::{load_object, save_object},
    internal::{
        branch::Branch,
        db::get_db_conn_instance,
        head::Head,
        reflog,
        reflog::{ReflogAction, ReflogContext, ReflogError, with_reflog},
    },
    utils::{
        object_ext::{BlobExt, TreeExt},
        path, util,
    },
};

/// Rebase state stored in .libra/rebase-merge/
#[derive(Debug, Clone)]
pub struct RebaseState {
    /// Original branch name being rebased
    pub head_name: String,
    /// Commit hash being rebased onto
    pub onto: ObjectHash,
    /// Original HEAD commit before rebase started
    pub orig_head: ObjectHash,
    /// Remaining commits to replay (in order)
    pub todo: VecDeque<ObjectHash>,
    /// Commits already replayed
    pub done: Vec<ObjectHash>,
    /// Current commit being applied (stopped due to conflict)
    pub stopped_sha: Option<ObjectHash>,
    /// Current new base (HEAD of rebased commits so far)
    pub current_head: ObjectHash,
}

impl RebaseState {
    /// Get the path to the rebase-merge directory
    pub fn rebase_dir() -> PathBuf {
        util::storage_path().join("rebase-merge")
    }

    /// Check if a rebase is in progress
    pub fn is_in_progress() -> bool {
        Self::rebase_dir().exists()
    }

    /// Save rebase state to .libra/rebase-merge/
    pub fn save(&self) -> Result<(), String> {
        let dir = Self::rebase_dir();
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

        // Save head-name
        fs::write(
            dir.join("head-name"),
            format!("refs/heads/{}", self.head_name),
        )
        .map_err(|e| e.to_string())?;

        // Save onto
        fs::write(dir.join("onto"), self.onto.to_string()).map_err(|e| e.to_string())?;

        // Save orig-head
        fs::write(dir.join("orig-head"), self.orig_head.to_string()).map_err(|e| e.to_string())?;

        // Save current-head
        fs::write(dir.join("current-head"), self.current_head.to_string())
            .map_err(|e| e.to_string())?;

        // Save todo (one commit per line)
        let todo_content: String = self.todo.iter().map(|h| h.to_string() + "\n").collect();
        fs::write(dir.join("todo"), todo_content).map_err(|e| e.to_string())?;

        // Save done (one commit per line)
        let done_content: String = self.done.iter().map(|h| h.to_string() + "\n").collect();
        fs::write(dir.join("done"), done_content).map_err(|e| e.to_string())?;

        // Save stopped-sha if present
        if let Some(stopped) = &self.stopped_sha {
            fs::write(dir.join("stopped-sha"), stopped.to_string()).map_err(|e| e.to_string())?;
        } else {
            let stopped_path = dir.join("stopped-sha");
            if stopped_path.exists() {
                fs::remove_file(stopped_path).map_err(|e| e.to_string())?;
            }
        }

        Ok(())
    }

    /// Load rebase state from .libra/rebase-merge/
    pub fn load() -> Result<Self, String> {
        let dir = Self::rebase_dir();
        if !dir.exists() {
            return Err("No rebase in progress".to_string());
        }

        // Load head-name
        let head_name_raw = fs::read_to_string(dir.join("head-name"))
            .map_err(|e| format!("Failed to read head-name: {}", e))?;
        let head_name = head_name_raw
            .trim()
            .strip_prefix("refs/heads/")
            .unwrap_or(head_name_raw.trim())
            .to_string();

        // Load onto
        let onto_str = fs::read_to_string(dir.join("onto"))
            .map_err(|e| format!("Failed to read onto: {}", e))?;
        let onto = ObjectHash::from_str(onto_str.trim())
            .map_err(|e| format!("Invalid onto hash: {}", e))?;

        // Load orig-head
        let orig_head_str = fs::read_to_string(dir.join("orig-head"))
            .map_err(|e| format!("Failed to read orig-head: {}", e))?;
        let orig_head = ObjectHash::from_str(orig_head_str.trim())
            .map_err(|e| format!("Invalid orig-head hash: {}", e))?;

        // Load current-head
        let current_head_str = fs::read_to_string(dir.join("current-head"))
            .map_err(|e| format!("Failed to read current-head: {}", e))?;
        let current_head = ObjectHash::from_str(current_head_str.trim())
            .map_err(|e| format!("Invalid current-head hash: {}", e))?;

        // Load todo
        let todo = VecDeque::from(Self::load_commit_list(&dir.join("todo"))?);

        // Load done
        let done = Self::load_commit_list(&dir.join("done"))?;

        // Load stopped-sha if present
        let stopped_sha = if dir.join("stopped-sha").exists() {
            let stopped_str = fs::read_to_string(dir.join("stopped-sha"))
                .map_err(|e| format!("Failed to read stopped-sha: {}", e))?;
            Some(
                ObjectHash::from_str(stopped_str.trim())
                    .map_err(|e| format!("Invalid stopped-sha hash: {}", e))?,
            )
        } else {
            None
        };

        Ok(RebaseState {
            head_name,
            onto,
            orig_head,
            todo,
            done,
            stopped_sha,
            current_head,
        })
    }

    /// Remove the rebase state directory
    pub fn cleanup() -> Result<(), String> {
        let dir = Self::rebase_dir();
        if dir.exists() {
            fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn load_commit_list(path: &Path) -> Result<Vec<ObjectHash>, String> {
        if !path.exists() {
            return Ok(Vec::new());
        }
        let file = fs::File::open(path).map_err(|e| e.to_string())?;
        let reader = BufReader::new(file);
        let mut commits = Vec::new();
        for line in reader.lines() {
            let line = line.map_err(|e| e.to_string())?;
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                let hash = ObjectHash::from_str(trimmed)
                    .map_err(|e| format!("Invalid commit hash '{}': {}", trimmed, e))?;
                commits.push(hash);
            }
        }
        Ok(commits)
    }
}

/// Result of attempting to replay a commit
pub enum ReplayResult {
    /// Commit was successfully replayed, contains the new commit hash
    Success(ObjectHash),
    /// Conflict occurred, contains list of conflicting file paths and an optional error message
    Conflict {
        paths: Vec<PathBuf>,
        message: Option<String>,
    },
}

impl ReplayResult {
    fn conflict(paths: Vec<PathBuf>) -> Self {
        ReplayResult::Conflict {
            paths,
            message: None,
        }
    }

    fn error(message: impl Into<String>) -> Self {
        ReplayResult::Conflict {
            paths: Vec::new(),
            message: Some(message.into()),
        }
    }
}

/// Command-line arguments for the rebase operation
#[derive(Parser, Debug)]
pub struct RebaseArgs {
    /// The upstream branch to rebase the current branch onto.
    /// This can be a branch name, commit hash, or other Git reference.
    #[clap(required_unless_present_any = ["continue_rebase", "abort", "skip"])]
    pub upstream: Option<String>,

    /// Continue an in-progress rebase after resolving conflicts
    #[clap(long = "continue", conflicts_with_all = ["abort", "skip", "upstream"])]
    pub continue_rebase: bool,

    /// Abort the current rebase and restore the original branch
    #[clap(long, conflicts_with_all = ["continue_rebase", "skip", "upstream"])]
    pub abort: bool,

    /// Skip the current commit and continue with the next
    #[clap(long, conflicts_with_all = ["continue_rebase", "abort", "upstream"])]
    pub skip: bool,
}

/// Execute the rebase command
///
/// Rebase moves or combines a sequence of commits to a new base commit.
/// This implementation performs a linear rebase by:
/// 1. Finding the common ancestor between current branch and upstream
/// 2. Collecting all commits from the common ancestor to current HEAD
/// 3. Replaying each commit on top of the upstream branch
/// 4. Updating the current branch reference to point to the final commit
///
/// The process maintains commit order but changes their parent relationships,
/// effectively "moving" the branch to start from the upstream commit.
pub async fn execute(args: RebaseArgs) {
    if !util::check_repo_exist() {
        return;
    }

    // Handle --continue, --abort, --skip
    if args.continue_rebase {
        rebase_continue().await;
        return;
    }
    if args.abort {
        rebase_abort().await;
        return;
    }
    if args.skip {
        rebase_skip().await;
        return;
    }

    // Check if rebase is already in progress
    if RebaseState::is_in_progress() {
        eprintln!("fatal: rebase already in progress");
        eprintln!("hint: use 'libra rebase --continue' to continue rebasing");
        eprintln!("hint: use 'libra rebase --abort' to abort and restore the original branch");
        eprintln!("hint: use 'libra rebase --skip' to skip this commit");
        return;
    }

    let upstream = match args.upstream {
        Some(u) => u,
        None => {
            eprintln!("fatal: no upstream specified");
            return;
        }
    };

    start_rebase(&upstream).await;
}

/// Start a new rebase operation
async fn start_rebase(upstream: &str) {
    let db = get_db_conn_instance().await;

    // Get the current branch that will be moved to the new base
    let current_branch_name = match Head::current().await {
        Head::Branch(name) if !name.is_empty() => name,
        _ => {
            eprintln!("fatal: not on a branch or in detached HEAD state, cannot rebase");
            return;
        }
    };

    // Get the current HEAD commit that represents the tip of the branch to rebase
    let head_to_rebase_id = match Head::current_commit().await {
        Some(id) => id,
        None => {
            eprintln!("fatal: current branch '{current_branch_name}' has no commits");
            return;
        }
    };

    // Resolve the upstream reference to a concrete commit ID
    let upstream_id = match resolve_branch_or_commit(upstream).await {
        Ok(id) => id,
        Err(e) => {
            eprintln!("fatal: {e}");
            return;
        }
    };

    // Find the merge base (common ancestor) between current branch and upstream
    // This determines which commits need to be replayed
    let base_id = match find_merge_base(&head_to_rebase_id, &upstream_id).await {
        Ok(Some(id)) => id,
        _ => {
            eprintln!("fatal: no common ancestor found");
            return;
        }
    };

    // Check if rebase is actually needed
    if base_id == head_to_rebase_id {
        let fast_forward_action = ReflogAction::Rebase {
            state: "fast-forward".to_string(),
            details: format!("moving {} to {}", current_branch_name, upstream),
        };
        let fast_forward_context = ReflogContext {
            old_oid: head_to_rebase_id.to_string(),
            new_oid: upstream_id.to_string(),
            action: fast_forward_action,
        };

        let branch_name_cloned = current_branch_name.clone();
        let upstream_id_str = upstream_id.to_string();
        if let Err(e) = with_reflog(
            fast_forward_context,
            move |txn: &sea_orm::DatabaseTransaction| {
                Box::pin(async move {
                    Branch::update_branch_with_conn(
                        txn,
                        &branch_name_cloned,
                        &upstream_id_str,
                        None,
                    )
                    .await;
                    Head::update_with_conn(txn, Head::Branch(branch_name_cloned), None).await;
                    Ok(())
                })
            },
            true,
        )
        .await
        {
            eprintln!("fatal: failed to fast-forward: {e}");
            return;
        }

        let upstream_commit: Commit = match load_object(&upstream_id) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("fatal: failed to load upstream commit: {:?}", e);
                return;
            }
        };
        let upstream_tree: Tree = match load_object(&upstream_commit.tree_id) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("fatal: failed to load upstream tree: {:?}", e);
                return;
            }
        };

        let index_file = path::index();
        let mut index = git_internal::internal::index::Index::new();
        if let Err(e) = rebuild_index_from_tree(&upstream_tree, &mut index, "") {
            eprintln!("fatal: failed to rebuild index: {}", e);
            return;
        }
        if let Err(e) = index.save(&index_file) {
            eprintln!("fatal: failed to save index: {:?}", e);
            return;
        }
        if let Err(e) = reset_workdir_to_index(&index) {
            eprintln!("fatal: failed to reset working directory: {}", e);
            return;
        }

        println!(
            "Fast-forwarded branch '{}' to '{}'.",
            current_branch_name, upstream
        );
        return;
    }
    if base_id == upstream_id {
        println!("Current branch is ahead of upstream. No rebase needed.");
        return;
    }

    // Collect all commits that need to be replayed from base to current HEAD
    let commits_to_replay = match collect_commits_to_replay(&base_id, &head_to_rebase_id).await {
        Ok(commits) if !commits.is_empty() => commits,
        _ => {
            println!("No commits to rebase on branch '{current_branch_name}'.",);
            return;
        }
    };
    println!("Found common ancestor: {}", &base_id.to_string()[..7]);
    println!(
        "Rebasing {} commits from '{}' onto '{}'...",
        commits_to_replay.len(),
        current_branch_name,
        upstream
    );

    let start_action = ReflogAction::Rebase {
        state: "start".to_string(),
        details: format!("checkout {}", upstream),
    };
    let start_context = ReflogContext {
        old_oid: head_to_rebase_id.to_string(),
        new_oid: upstream_id.to_string(),
        action: start_action,
    };
    let transaction_result = db
        .transaction(|txn| {
            Box::pin(async move {
                reflog::Reflog::insert_single_entry(txn, &start_context, "HEAD").await?;
                Head::update_with_conn(txn, Head::Detached(upstream_id), None).await;
                Ok::<_, ReflogError>(())
            })
        })
        .await;

    if let Err(e) = transaction_result {
        eprintln!("fatal: failed to start rebase: {}", e);
        return;
    }

    // Save rebase state
    let mut state = RebaseState {
        head_name: current_branch_name.clone(),
        onto: upstream_id,
        orig_head: head_to_rebase_id,
        todo: VecDeque::from(commits_to_replay.clone()),
        done: Vec::new(),
        stopped_sha: None,
        current_head: upstream_id,
    };

    if let Err(e) = state.save() {
        eprintln!("fatal: failed to save rebase state: {}", e);
        return;
    }

    // This mimics Git's behavior.
    Head::update_with_conn(db, Head::Detached(upstream_id), None).await;

    // Continue replaying commits
    continue_replay(&mut state, &current_branch_name, upstream).await;
}

/// Continue replaying commits from the current state
async fn continue_replay(state: &mut RebaseState, branch_name: &str, upstream_display: &str) {
    let db = get_db_conn_instance().await;
    let commit_subject = |commit_id: &ObjectHash| -> String {
        match load_object::<Commit>(commit_id) {
            Ok(commit) => commit.message.lines().next().unwrap_or("").to_string(),
            Err(e) => {
                eprintln!(
                    "warning: failed to load commit {}: {:?}",
                    &commit_id.to_string()[..7],
                    e
                );
                "unknown".to_string()
            }
        }
    };

    println!(
        "Rebasing {} commits from `{}` onto `{}`...",
        state.todo.len(),
        branch_name,
        upstream_display
    );

    while let Some(commit_id) = state.todo.front().cloned() {
        match replay_commit_with_conflict_detection(&commit_id, &state.current_head).await {
            ReplayResult::Success(replayed_commit_id) => {
                state.current_head = replayed_commit_id;
                // Move commit from todo to done
                state.todo.pop_front();
                state.done.push(commit_id);
                state.stopped_sha = None;

                // Update HEAD
                Head::update_with_conn(db, Head::Detached(state.current_head), None).await;

                println!(
                    "Applied: {} {}",
                    &state.current_head.to_string()[..7],
                    commit_subject(&commit_id)
                );

                // Save state after each successful commit
                if let Err(e) = state.save() {
                    eprintln!("warning: failed to save rebase state: {}", e);
                }
            }
            ReplayResult::Conflict { paths, message } => {
                // Save state with stopped_sha
                state.stopped_sha = Some(commit_id);
                if let Err(e) = state.save() {
                    eprintln!("fatal: failed to save rebase state: {}", e);
                }

                eprintln!(
                    "error: could not apply {}: {}",
                    &commit_id.to_string()[..7],
                    commit_subject(&commit_id)
                );
                if let Some(message) = message.as_ref() {
                    eprintln!("fatal: {}", message);
                }

                if !paths.is_empty() {
                    eprintln!("CONFLICT in {} file(s):", paths.len());
                    for path in &paths {
                        eprintln!("  {}", path.display());
                    }
                    eprintln!();
                    eprintln!("After resolving conflicts, mark them with 'libra add <file>'");
                    eprintln!("then run 'libra rebase --continue'");
                    eprintln!("To skip this commit, run 'libra rebase --skip'");
                    eprintln!(
                        "To abort and return to the original branch, run 'libra rebase --abort'"
                    );
                } else {
                    eprintln!("Rebase stopped due to an internal error.");
                    eprintln!(
                        "To abort and return to the original branch, run 'libra rebase --abort'"
                    );
                }
                return;
            }
        }
    }

    // All commits replayed successfully - finalize
    finalize_rebase(state).await;
}

/// Finalize rebase after all commits are replayed
async fn finalize_rebase(state: &RebaseState) {
    let db = get_db_conn_instance().await;
    let final_commit_id = state.current_head;

    let finish_action = ReflogAction::Rebase {
        state: "finish".to_string(),
        details: format!("returning to refs/heads/{}", state.head_name),
    };
    let finish_context = ReflogContext {
        old_oid: state.orig_head.to_string(),
        new_oid: final_commit_id.to_string(),
        action: finish_action,
    };

    let branch_name_cloned = state.head_name.clone();
    if let Err(e) = with_reflog(
        finish_context,
        move |txn: &sea_orm::DatabaseTransaction| {
            Box::pin(async move {
                // This is the crucial step: move the original branch from its old position
                // to the final replayed commit.
                Branch::update_branch_with_conn(
                    txn,
                    &branch_name_cloned,
                    &final_commit_id.to_string(),
                    None,
                )
                .await;

                // Also, re-attach HEAD to the newly moved branch.
                Head::update_with_conn(txn, Head::Branch(branch_name_cloned.clone()), None).await;
                Ok(())
            })
        },
        true,
    )
    .await
    {
        eprintln!("fatal: failed to finalize rebase: {e}");
        // Attempt to restore HEAD to a safe state
        Head::update_with_conn(db, Head::Detached(state.onto), None).await;
        return;
    }

    // Reset the working directory and index to match the final state
    // This ensures that the workspace reflects the rebased commits
    let final_commit: Commit = load_object(&state.current_head).unwrap();
    let final_tree: Tree = load_object(&final_commit.tree_id).unwrap();

    let index_file = path::index();
    let mut index = git_internal::internal::index::Index::new();
    rebuild_index_from_tree(&final_tree, &mut index, "").unwrap();
    index.save(&index_file).unwrap();
    reset_workdir_to_index(&index).unwrap();

    // Clean up rebase state
    if let Err(e) = RebaseState::cleanup() {
        eprintln!("warning: failed to clean up rebase state: {}", e);
    }

    println!(
        "Successfully rebased branch '{}' onto '{}'.",
        state.head_name,
        &state.onto.to_string()[..7]
    );
}

/// Continue a rebase after conflict resolution
async fn rebase_continue() {
    if !RebaseState::is_in_progress() {
        eprintln!("fatal: no rebase in progress");
        return;
    }

    let mut state = match RebaseState::load() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("fatal: failed to load rebase state: {}", e);
            return;
        }
    };

    // Check if there's a stopped commit that needs to be continued
    let stopped_sha = match state.stopped_sha {
        Some(sha) => sha,
        None => {
            // No conflict, just continue with remaining commits
            if state.todo.is_empty() {
                finalize_rebase(&state).await;
            } else {
                let head_name = state.head_name.clone();
                let onto_display = state.onto.to_string()[..7].to_string();
                continue_replay(&mut state, &head_name, &onto_display).await;
            }
            return;
        }
    };

    // Create a commit from the current index (user should have resolved conflicts)
    let index_file = path::index();
    let index = match git_internal::internal::index::Index::load(&index_file) {
        Ok(idx) => idx,
        Err(e) => {
            eprintln!("fatal: failed to load index: {:?}", e);
            return;
        }
    };

    // Check for unmerged entries (stage != 0)
    if has_unmerged_entries(&index) {
        eprintln!("error: you must resolve all conflicts before continuing");
        eprintln!("hint: use 'libra add <file>' to mark conflicts as resolved");
        return;
    }

    // Create tree from current index
    let new_tree_id = match create_tree_from_index(&index) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("fatal: failed to create tree: {}", e);
            return;
        }
    };

    // Get the original commit message
    let original_commit: Commit = match load_object(&stopped_sha) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("fatal: failed to load original commit: {:?}", e);
            return;
        }
    };

    // Create new commit
    let new_commit = Commit::from_tree_id(
        new_tree_id,
        vec![state.current_head],
        &original_commit.message,
    );
    if let Err(e) = save_object(&new_commit, &new_commit.id) {
        eprintln!("fatal: failed to save commit: {:?}", e);
        return;
    }

    println!(
        "Applied: {} {}",
        &new_commit.id.to_string()[..7],
        original_commit.message.lines().next().unwrap_or("")
    );

    // Update state
    state.current_head = new_commit.id;
    state.todo.pop_front();
    state.done.push(stopped_sha);
    state.stopped_sha = None;

    // Update HEAD
    let db = get_db_conn_instance().await;
    Head::update_with_conn(db, Head::Detached(state.current_head), None).await;

    if let Err(e) = state.save() {
        eprintln!("warning: failed to save state: {}", e);
    }

    // Continue with remaining commits
    if state.todo.is_empty() {
        finalize_rebase(&state).await;
    } else {
        let head_name = state.head_name.clone();
        let onto_display = state.onto.to_string()[..7].to_string();
        continue_replay(&mut state, &head_name, &onto_display).await;
    }
}

/// Abort the current rebase and restore the original state
async fn rebase_abort() {
    if !RebaseState::is_in_progress() {
        eprintln!("fatal: no rebase in progress");
        return;
    }

    let state = match RebaseState::load() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("fatal: failed to load rebase state: {}", e);
            return;
        }
    };

    let db = get_db_conn_instance().await;

    // Restore HEAD to original branch
    let abort_action = ReflogAction::Rebase {
        state: "abort".to_string(),
        details: format!("returning to refs/heads/{}", state.head_name),
    };
    let abort_context = ReflogContext {
        old_oid: state.current_head.to_string(),
        new_oid: state.orig_head.to_string(),
        action: abort_action,
    };

    let branch_name_cloned = state.head_name.clone();
    let orig_head = state.orig_head;
    if let Err(e) = with_reflog(
        abort_context,
        move |txn: &sea_orm::DatabaseTransaction| {
            Box::pin(async move {
                Head::update_with_conn(txn, Head::Branch(branch_name_cloned), None).await;
                Ok(())
            })
        },
        true,
    )
    .await
    {
        eprintln!("warning: failed to record reflog: {e}");
        // Continue anyway
    }

    Head::update_with_conn(db, Head::Branch(state.head_name.clone()), None).await;

    // Reset working directory to original HEAD
    let orig_commit: Commit = match load_object(&orig_head) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("fatal: failed to load original commit: {:?}", e);
            return;
        }
    };
    let orig_tree: Tree = match load_object(&orig_commit.tree_id) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("fatal: failed to load original tree: {:?}", e);
            return;
        }
    };

    let index_file = path::index();
    let mut index = git_internal::internal::index::Index::new();
    if let Err(e) = rebuild_index_from_tree(&orig_tree, &mut index, "") {
        eprintln!("fatal: failed to rebuild index: {}", e);
        return;
    }
    if let Err(e) = index.save(&index_file) {
        eprintln!("fatal: failed to save index: {:?}", e);
        return;
    }
    if let Err(e) = reset_workdir_to_index(&index) {
        eprintln!("fatal: failed to reset working directory: {}", e);
        return;
    }

    // Clean up rebase state
    if let Err(e) = RebaseState::cleanup() {
        eprintln!("warning: failed to clean up rebase state: {}", e);
    }

    println!("Rebase aborted. Restored branch '{}'.", state.head_name);
}

/// Skip the current commit and continue with the next
async fn rebase_skip() {
    if !RebaseState::is_in_progress() {
        eprintln!("fatal: no rebase in progress");
        return;
    }

    let mut state = match RebaseState::load() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("fatal: failed to load rebase state: {}", e);
            return;
        }
    };

    let skipped_sha = match state.stopped_sha {
        Some(sha) => sha,
        None => {
            if state.todo.is_empty() {
                eprintln!("fatal: no commit to skip");
                return;
            }
            match state.todo.front().cloned() {
                Some(sha) => sha,
                None => {
                    eprintln!("fatal: no commit to skip");
                    return;
                }
            }
        }
    };

    let original_commit: Commit = match load_object(&skipped_sha) {
        Ok(c) => c,
        Err(_) => {
            eprintln!("warning: could not load skipped commit");
            Commit::from_tree_id(ObjectHash::default(), vec![], "unknown")
        }
    };

    println!(
        "Skipped: {} {}",
        &skipped_sha.to_string()[..7],
        original_commit.message.lines().next().unwrap_or("")
    );

    // Remove the commit from todo
    state.todo.pop_front();
    state.stopped_sha = None;

    // Reset index and working directory to current_head
    let current_commit: Commit = match load_object(&state.current_head) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("fatal: failed to load current commit: {:?}", e);
            return;
        }
    };
    let current_tree: Tree = match load_object(&current_commit.tree_id) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("fatal: failed to load current tree: {:?}", e);
            return;
        }
    };

    let index_file = path::index();
    let mut index = git_internal::internal::index::Index::new();
    if let Err(e) = rebuild_index_from_tree(&current_tree, &mut index, "") {
        eprintln!("fatal: failed to rebuild index: {}", e);
        return;
    }
    if let Err(e) = index.save(&index_file) {
        eprintln!("fatal: failed to save index: {:?}", e);
        return;
    }
    if let Err(e) = reset_workdir_to_index(&index) {
        eprintln!("fatal: failed to reset working directory: {}", e);
        return;
    }

    if let Err(e) = state.save() {
        eprintln!("warning: failed to save state: {}", e);
    }

    // Continue with remaining commits
    if state.todo.is_empty() {
        finalize_rebase(&state).await;
    } else {
        let head_name = state.head_name.clone();
        let onto_display = state.onto.to_string()[..7].to_string();
        continue_replay(&mut state, &head_name, &onto_display).await;
    }
}

/// Check if index has unmerged entries (conflict markers)
///
/// A file is considered unmerged if it has any stage 1, 2, or 3 entry but NO stage 0 entry.
/// If a file has been staged at stage 0 (via `add`), it's considered resolved
/// even if older conflict stage entries (stages 1–3) still exist in the index.
fn has_unmerged_entries(index: &git_internal::internal::index::Index) -> bool {
    let resolved: HashSet<String> = index
        .tracked_entries(0)
        .into_iter()
        .map(|entry| entry.name.clone())
        .collect();

    for stage in 1..=3 {
        for entry in index.tracked_entries(stage) {
            if !resolved.contains(&entry.name) {
                return true;
            }
        }
    }
    false
}

/// Create a tree from the current index
fn create_tree_from_index(
    index: &git_internal::internal::index::Index,
) -> Result<ObjectHash, String> {
    let mut items: HashMap<PathBuf, ObjectHash> = HashMap::new();
    for path in index.tracked_files() {
        let path_str = path.to_string_lossy();
        if let Some(entry) = index.get(&path_str, 0) {
            items.insert(path.clone(), entry.hash);
        }
    }
    create_tree_from_items_map(&items)
}

fn write_workdir_file(workdir: &Path, path: &Path, content: &[u8]) -> Result<(), String> {
    let file_path = workdir.join(path);
    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create directory {}: {}", parent.display(), e))?;
    }
    fs::write(&file_path, content)
        .map_err(|e| format!("failed to write {}: {}", file_path.display(), e))
}

fn write_conflict_file(workdir: &Path, path: &Path, content: &str) -> Result<(), String> {
    write_workdir_file(workdir, path, content.as_bytes())
        .map_err(|e| format!("conflict file: {}", e))
}

fn conflict_marker_eol() -> &'static str {
    if cfg!(windows) {
        "\r\n"
    } else {
        "\n"
    }
}

/// Resolve a branch name or commit reference to a ObjectHash hash
///
/// This function first tries to find a branch with the given name,
/// then falls back to resolving it as a commit reference (hash, HEAD, etc.).
/// This allows the rebase command to work with both branch names and commit hashes.
async fn resolve_branch_or_commit(reference: &str) -> Result<ObjectHash, String> {
    util::get_commit_base(reference).await
}

/// Replay a single commit with conflict detection
///
/// This function performs a three-way merge to apply the changes from one commit
/// onto a different base commit, with proper conflict detection.
///
/// The three points of the merge are:
/// - Base: The original parent of the commit being replayed
/// - Theirs: The commit being replayed (contains the changes to apply)
/// - Ours: The new parent commit (where we want to apply the changes)
///
/// For each path, it compares the content in these three trees and constructs
/// a merged tree. If both `ours` and `theirs` modify the same path in
/// incompatible ways relative to `base`, the function reports a conflict
/// and leaves resolution to the caller.
async fn replay_commit_with_conflict_detection(
    commit_to_replay_id: &ObjectHash,
    new_parent_id: &ObjectHash,
) -> ReplayResult {
    let commit_to_replay: Commit = match load_object(commit_to_replay_id) {
        Ok(c) => c,
        Err(e) => return ReplayResult::error(format!("error: {}", e)),
    };

    let original_parent_id = match commit_to_replay.parent_commit_ids.first() {
        Some(id) => id,
        None => return ReplayResult::error("commit has no parents"),
    };

    // Load the three trees needed for the three-way merge
    let base_tree: Tree =
        match load_object::<Commit>(original_parent_id).and_then(|c| load_object(&c.tree_id)) {
            Ok(t) => t,
            Err(e) => return ReplayResult::error(format!("base tree: {}", e)),
        };

    let their_tree: Tree = match load_object(&commit_to_replay.tree_id) {
        Ok(t) => t,
        Err(e) => return ReplayResult::error(format!("their tree: {}", e)),
    };

    let our_tree: Tree =
        match load_object::<Commit>(new_parent_id).and_then(|c| load_object(&c.tree_id)) {
            Ok(t) => t,
            Err(e) => return ReplayResult::error(format!("our tree: {}", e)),
        };

    // Get all items from each tree
    let base_items: HashMap<PathBuf, ObjectHash> =
        base_tree.get_plain_items().into_iter().collect();
    let their_items: HashMap<PathBuf, ObjectHash> =
        their_tree.get_plain_items().into_iter().collect();
    let our_items: HashMap<PathBuf, ObjectHash> = our_tree.get_plain_items().into_iter().collect();

    // Collect all paths
    let all_paths: HashSet<PathBuf> = base_items
        .keys()
        .chain(their_items.keys())
        .chain(our_items.keys())
        .cloned()
        .collect();

    let mut merged_items: HashMap<PathBuf, ObjectHash> = HashMap::new();
    let mut conflicts: Vec<PathBuf> = Vec::new();
    let workdir = util::working_dir();
    let commit_abbrev = commit_to_replay_id.to_string();
    let marker_eol = conflict_marker_eol();

    for path in all_paths {
        let base_hash = base_items.get(&path);
        let their_hash = their_items.get(&path);
        let our_hash = our_items.get(&path);

        match (base_hash, their_hash, our_hash) {
            // No change from base - keep ours
            (Some(b), Some(t), Some(o)) if b == t => {
                merged_items.insert(path, *o);
            }
            // No change from base - keep theirs (which equals base equals ours)
            (Some(b), Some(t), Some(o)) if b == o => {
                merged_items.insert(path, *t);
            }
            // Both changed to same value
            (_, Some(t), Some(o)) if t == o => {
                merged_items.insert(path, *t);
            }
            // Both changed differently - CONFLICT
            (Some(_b), Some(t), Some(o)) if t != o => {
                // Write conflict markers to working directory
                let their_content = Blob::load(t).data;
                let our_content = Blob::load(o).data;

                let conflict_content = format!(
                    "<<<<<<< HEAD{marker_eol}{}{marker_eol}======={marker_eol}{}{marker_eol}>>>>>>> {}{marker_eol}",
                    String::from_utf8_lossy(&our_content),
                    String::from_utf8_lossy(&their_content),
                    &commit_abbrev[..7]
                );

                if let Err(e) = write_conflict_file(&workdir, &path, &conflict_content) {
                    return ReplayResult::error(e);
                }

                conflicts.push(path);
            }
            // Theirs added, ours doesn't have - use theirs
            (None, Some(t), None) => {
                merged_items.insert(path, *t);
            }
            // Ours added, theirs doesn't have - keep ours
            (None, None, Some(o)) => {
                merged_items.insert(path, *o);
            }
            // Both added differently - CONFLICT
            (None, Some(t), Some(o)) if t != o => {
                let their_content = Blob::load(t).data;
                let our_content = Blob::load(o).data;

                let conflict_content = format!(
                    "<<<<<<< HEAD{marker_eol}{}{marker_eol}======={marker_eol}{}{marker_eol}>>>>>>> {}{marker_eol}",
                    String::from_utf8_lossy(&our_content),
                    String::from_utf8_lossy(&their_content),
                    &commit_abbrev[..7]
                );

                if let Err(e) = write_conflict_file(&workdir, &path, &conflict_content) {
                    return ReplayResult::error(e);
                }

                conflicts.push(path);
            }
            // Theirs deleted, ours unchanged - delete
            (Some(b), None, Some(o)) if b == o => {
                // File deleted in theirs, don't include
            }
            // Ours deleted, theirs unchanged - keep deleted
            (Some(b), Some(t), None) if b == t => {
                // File deleted in ours, don't include
            }
            // Theirs deleted, ours modified - CONFLICT (delete/modify)
            (Some(_b), None, Some(o)) => {
                let our_content = Blob::load(o).data;
                let conflict_content = format!(
                    "<<<<<<< HEAD{marker_eol}{}{marker_eol}======={marker_eol}>>>>>>> {} (deleted){marker_eol}",
                    String::from_utf8_lossy(&our_content),
                    &commit_abbrev[..7]
                );

                if let Err(e) = write_conflict_file(&workdir, &path, &conflict_content) {
                    return ReplayResult::error(e);
                }

                conflicts.push(path);
            }
            // Ours deleted, theirs modified - CONFLICT (modify/delete)
            (Some(_b), Some(t), None) => {
                let their_content = Blob::load(t).data;
                let conflict_content = format!(
                    "<<<<<<< HEAD (deleted){marker_eol}======={marker_eol}{}{marker_eol}>>>>>>> {}{marker_eol}",
                    String::from_utf8_lossy(&their_content),
                    &commit_abbrev[..7]
                );

                if let Err(e) = write_conflict_file(&workdir, &path, &conflict_content) {
                    return ReplayResult::error(e);
                }

                conflicts.push(path);
            }
            // Both deleted - nothing to do
            (Some(_), None, None) => {}
            // Catch-all for any other case
            _ => {
                if let Some(t) = their_hash {
                    merged_items.insert(path, *t);
                } else if let Some(o) = our_hash {
                    merged_items.insert(path, *o);
                }
            }
        }
    }

    if !conflicts.is_empty() {
        // Update index with conflict entries
        let index_file = path::index();
        let mut index = git_internal::internal::index::Index::new();

        // Add non-conflicting files at stage 0
        for (path, hash) in &merged_items {
            let blob = Blob::load(hash);
            let entry = git_internal::internal::index::IndexEntry::new_from_blob(
                path.to_string_lossy().to_string(),
                *hash,
                blob.data.len() as u32,
            );
            index.add(entry);
        }

        // Add conflicting files at stages 1, 2, 3
        for path in &conflicts {
            let path_str = path.to_string_lossy().to_string();

            // Stage 1: base version
            if let Some(base_hash) = base_items.get(path) {
                let blob = Blob::load(base_hash);
                let mut entry = git_internal::internal::index::IndexEntry::new_from_blob(
                    path_str.clone(),
                    *base_hash,
                    blob.data.len() as u32,
                );
                entry.flags.stage = 1;
                index.add(entry);
            }

            // Stage 2: ours version
            if let Some(our_hash) = our_items.get(path) {
                let blob = Blob::load(our_hash);
                let mut entry = git_internal::internal::index::IndexEntry::new_from_blob(
                    path_str.clone(),
                    *our_hash,
                    blob.data.len() as u32,
                );
                entry.flags.stage = 2;
                index.add(entry);
            }

            // Stage 3: theirs version
            if let Some(their_hash) = their_items.get(path) {
                let blob = Blob::load(their_hash);
                let mut entry = git_internal::internal::index::IndexEntry::new_from_blob(
                    path_str.clone(),
                    *their_hash,
                    blob.data.len() as u32,
                );
                entry.flags.stage = 3;
                index.add(entry);
            }
        }

        if let Err(e) = index.save(&index_file) {
            return ReplayResult::Conflict {
                paths: conflicts,
                message: Some(format!("index save: {}", e)),
            };
        }

        // Update working directory for non-conflicting paths so users can see clean changes.
        let mut tracked_paths: HashSet<PathBuf> = HashSet::new();
        tracked_paths.extend(base_items.keys().cloned());
        tracked_paths.extend(their_items.keys().cloned());
        tracked_paths.extend(our_items.keys().cloned());

        let conflict_set: HashSet<PathBuf> = conflicts.iter().cloned().collect();

        for (path, hash) in &merged_items {
            let blob = Blob::load(hash);
            if let Err(e) = write_workdir_file(&workdir, path, &blob.data) {
                return ReplayResult::Conflict {
                    paths: conflicts,
                    message: Some(e),
                };
            }
        }

        for path in tracked_paths {
            if conflict_set.contains(&path) || merged_items.contains_key(&path) {
                continue;
            }
            let full_path = workdir.join(&path);
            if full_path.exists() {
                if let Err(e) = fs::remove_file(&full_path) {
                    return ReplayResult::Conflict {
                        paths: conflicts,
                        message: Some(format!(
                            "failed to remove {}: {}",
                            full_path.display(),
                            e
                        )),
                    };
                }
            }
        }

        return ReplayResult::conflict(conflicts);
    }

    // No conflicts - create the merged tree and commit
    let new_tree_id = match create_tree_from_items_map(&merged_items) {
        Ok(id) => id,
        Err(e) => return ReplayResult::error(format!("tree creation: {}", e)),
    };

    let new_commit =
        Commit::from_tree_id(new_tree_id, vec![*new_parent_id], &commit_to_replay.message);

    if let Err(e) = save_object(&new_commit, &new_commit.id) {
        return ReplayResult::error(format!("commit save: {}", e));
    }

    // Update index and working directory
    let index_file = path::index();
    let mut index = git_internal::internal::index::Index::new();
    let new_tree: Tree = match load_object(&new_tree_id) {
        Ok(tree) => tree,
        Err(e) => return ReplayResult::error(format!("new tree load: {}", e)),
    };
    if let Err(e) = rebuild_index_from_tree(&new_tree, &mut index, "") {
        return ReplayResult::error(format!("index rebuild: {}", e));
    }
    if let Err(e) = index.save(&index_file) {
        return ReplayResult::error(format!("index save: {}", e));
    }
    if let Err(e) = reset_workdir_to_index(&index) {
        return ReplayResult::error(format!("workdir reset: {}", e));
    }

    ReplayResult::Success(new_commit.id)
}

/// Find the merge base (common ancestor) of two commits
///
/// This function implements a simple merge base algorithm:
/// 1. Traverse all ancestors of the first commit and store them in a set
/// 2. Traverse ancestors of the second commit until we find one in the set
/// 3. Return the first common ancestor found
///
/// Note: This returns the first common ancestor found, not necessarily the
/// best common ancestor. A more sophisticated algorithm would find the
/// lowest common ancestor (LCA).
///
/// TODO: Implement proper LCA algorithm for better merge base detection
/// TODO: Optimize performance for large repositories with many commits
async fn find_merge_base(
    commit1_id: &ObjectHash,
    commit2_id: &ObjectHash,
) -> Result<Option<ObjectHash>, String> {
    let mut visited1 = HashSet::new();
    let mut visited2 = HashSet::new();
    let mut queue1 = vec![*commit1_id];
    let mut queue2 = vec![*commit2_id];
    while !queue1.is_empty() || !queue2.is_empty() {
        // Process one level of ancestors for commit1
        if let Some(current_id) = queue1.pop() {
            if visited2.contains(&current_id) {
                return Ok(Some(current_id)); // Found common ancestor
            }
            if visited1.insert(current_id) {
                let commit: Commit = load_object(&current_id).map_err(|e| e.to_string())?;
                for parent_id in &commit.parent_commit_ids {
                    queue1.push(*parent_id);
                }
            }
        }
        // Process one level of ancestors for commit2
        if let Some(current_id) = queue2.pop() {
            if visited1.contains(&current_id) {
                return Ok(Some(current_id)); // Found common ancestor
            }
            if visited2.insert(current_id) {
                let commit: Commit = load_object(&current_id).map_err(|e| e.to_string())?;
                for parent_id in &commit.parent_commit_ids {
                    queue2.push(*parent_id);
                }
            }
        }
    }
    Ok(None)
}

/// Collect all commits from base (exclusive) to head (inclusive) that need to be replayed
///
/// This function walks backwards from the head commit to the base commit,
/// collecting all commits in between. These are the commits that will be
/// replayed onto the new upstream base.
///
/// The commits are returned in chronological order (oldest first) so they
/// can be replayed in the correct sequence.
async fn collect_commits_to_replay(
    base_id: &ObjectHash,
    head_id: &ObjectHash,
) -> Result<Vec<ObjectHash>, String> {
    let mut commits = Vec::new();
    let mut current_id = *head_id;

    // Walk backwards from head to base, collecting commit IDs
    while current_id != *base_id {
        commits.push(current_id);
        let commit: Commit = load_object(&current_id).map_err(|e| e.to_string())?;
        if commit.parent_commit_ids.is_empty() {
            break; // Reached root commit
        }
        current_id = commit.parent_commit_ids[0]; // Follow first parent
        // TODO: Handle merge commits properly - currently only follows first parent
        // This may miss commits in complex branch histories
    }

    // Reverse to get chronological order (oldest first)
    commits.reverse();
    Ok(commits)
}

/// Compute the differences between two tree objects
///
/// This function compares two trees and returns a list of all files that
/// differ between them. Each difference is represented as a tuple containing:
/// - PathBuf: The file path that differs
/// - Option<ObjectHash>: The file hash in the "theirs" tree (None if deleted)
/// - Option<ObjectHash>: The file hash in the "base" tree (None if newly added)
///
/// This is used to determine what changes need to be applied during replay.
fn diff_trees(
    theirs: &Tree,
    base: &Tree,
) -> Vec<(PathBuf, Option<ObjectHash>, Option<ObjectHash>)> {
    let their_items: HashMap<_, _> = theirs.get_plain_items().into_iter().collect();
    let base_items: HashMap<_, _> = base.get_plain_items().into_iter().collect();
    let all_paths: HashSet<_> = their_items.keys().chain(base_items.keys()).collect();
    let mut diffs = Vec::new();

    for path in all_paths {
        let their_hash = their_items.get(path).cloned();
        let base_hash = base_items.get(path).cloned();
        if their_hash != base_hash {
            diffs.push((path.clone(), their_hash, base_hash));
        }
    }
    diffs
}

/// Create a tree object from a flat map of file paths to content hashes
///
/// This function takes a HashMap of file paths and their content hashes,
/// and builds a proper Git tree structure. It handles:
/// - Grouping files by their parent directories
/// - Creating tree objects for each directory
/// - Recursively building the tree structure from root to leaves
///
/// Returns the ObjectHash hash of the root tree object.
fn create_tree_from_items_map(items: &HashMap<PathBuf, ObjectHash>) -> Result<ObjectHash, String> {
    // Group files by their parent directories
    let mut entries_map: HashMap<PathBuf, Vec<git_internal::internal::object::tree::TreeItem>> =
        HashMap::new();
    for (path, hash) in items {
        let item = git_internal::internal::object::tree::TreeItem {
            mode: git_internal::internal::object::tree::TreeItemMode::Blob,
            name: path.file_name().unwrap().to_str().unwrap().to_string(),
            id: *hash,
        };
        // TODO: Handle file modes properly - currently assumes all files are blobs
        let parent_dir = path.parent().unwrap_or_else(|| Path::new("")).to_path_buf();
        entries_map.entry(parent_dir).or_default().push(item);
    }
    build_tree_recursively(Path::new(""), &mut entries_map)
}

/// Recursively build tree objects from a directory structure
///
/// This helper function processes a directory and all its subdirectories:
/// 1. Creates tree items for all files in the current directory
/// 2. Recursively processes subdirectories to create subtree objects  
/// 3. Combines files and subdirectories into a single tree object
/// 4. Saves the tree object and returns its hash
///
/// The algorithm works bottom-up, creating leaf trees first and then
/// combining them into parent trees.
fn build_tree_recursively(
    current_path: &Path,
    entries_map: &mut HashMap<PathBuf, Vec<git_internal::internal::object::tree::TreeItem>>,
) -> Result<ObjectHash, String> {
    // Get all files/items in the current directory
    let mut current_items = entries_map.remove(current_path).unwrap_or_default();

    // Find all subdirectories that are children of current directory
    let subdirs: Vec<_> = entries_map
        .keys()
        .filter(|p| p.parent() == Some(current_path))
        .cloned()
        .collect();

    // Recursively process each subdirectory
    for subdir_path in subdirs {
        let subdir_name = subdir_path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        let subtree_hash = build_tree_recursively(&subdir_path, entries_map)?;

        // Add the subdirectory as a tree item
        current_items.push(git_internal::internal::object::tree::TreeItem {
            mode: git_internal::internal::object::tree::TreeItemMode::Tree,
            name: subdir_name,
            id: subtree_hash,
        });
    }

    // Create and save the tree object for this directory
    let tree = Tree::from_tree_items(current_items).map_err(|e| e.to_string())?;
    save_object(&tree, &tree.id).map_err(|e| e.to_string())?;
    Ok(tree.id)
}

/// Reset the working directory to match the given index state
///
/// This function synchronizes the working directory with the index by:
/// 1. Removing any files that exist in the working directory but not in the index
/// 2. Writing out all files that are tracked in the index to the working directory
/// 3. Creating necessary parent directories as needed
///
/// This ensures the working directory reflects the final rebased state.
fn reset_workdir_to_index(index: &git_internal::internal::index::Index) -> Result<(), String> {
    let workdir = util::working_dir();
    let tracked_paths = index.tracked_files();
    let index_files_set: HashSet<_> = tracked_paths.iter().collect();

    // Remove files that are no longer tracked
    let all_files_in_workdir = util::list_workdir_files().unwrap_or_default();
    for path_from_root in all_files_in_workdir {
        if !index_files_set.contains(&path_from_root) {
            let full_path = workdir.join(path_from_root);
            if full_path.exists() {
                fs::remove_file(&full_path).map_err(|e| e.to_string())?;
                // TODO: Implement atomic file operations with rollback capability
                // TODO: Handle directory cleanup when all files are removed
            }
        }
    }

    // Write out all tracked files
    for path_buf in &tracked_paths {
        let path_str = path_buf.to_string_lossy();
        if let Some(entry) = index.get(&path_str, 0) {
            let blob = git_internal::internal::object::blob::Blob::load(&entry.hash);
            let target_path = workdir.join(&*path_str);

            // Create parent directories if needed
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            fs::write(&target_path, &blob.data).map_err(|e| e.to_string())?;
            // TODO: Preserve file permissions and timestamps
            // TODO: Handle large files efficiently (streaming)
        }
    }
    Ok(())
}

/// Rebuild an index from a tree object by recursively adding all files
///
/// This function traverses a tree object and adds all files to the given index.
/// It handles both files (blobs) and subdirectories (trees) by:
/// 1. For files: Loading the blob and creating an index entry
/// 2. For subdirectories: Recursively processing the subtree
///
/// The prefix parameter tracks the current directory path during recursion.
fn rebuild_index_from_tree(
    tree: &Tree,
    index: &mut git_internal::internal::index::Index,
    prefix: &str,
) -> Result<(), String> {
    for item in &tree.tree_items {
        let full_path = if prefix.is_empty() {
            item.name.clone()
        } else {
            format!("{}/{}", prefix, item.name)
        };

        if let git_internal::internal::object::tree::TreeItemMode::Tree = item.mode {
            // Recursively process subdirectory
            let subtree: Tree = load_object(&item.id).map_err(|e| e.to_string())?;
            rebuild_index_from_tree(&subtree, index, &full_path)?;
        } else {
            // Add file to index
            let blob = git_internal::internal::object::blob::Blob::load(&item.id);
            let entry = git_internal::internal::index::IndexEntry::new_from_blob(
                full_path,
                item.id,
                blob.data.len() as u32,
            );
            // TODO: Handle different file modes (executable, symlinks, etc.)
            // TODO: Add proper error handling for corrupted blob objects
            index.add(entry);
        }
    }
    Ok(())
}
