//! Rebase implementation that parses onto/branch arguments, replays commits onto a new base, handles conflicts, and updates branch refs.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::Context;
use clap::Parser;
use git_internal::{
    hash::ObjectHash,
    internal::object::{blob::Blob, commit::Commit, tree::Tree},
};
use sea_orm::{ConnectionTrait, DbBackend, Statement, TransactionTrait, Value};

use crate::{
    command::{load_object, save_object, status},
    internal::{
        branch::Branch,
        db::get_db_conn_instance,
        head::Head,
        reflog,
        reflog::{ReflogAction, ReflogContext, ReflogError, with_reflog},
    },
    utils::{
        ignore::IgnorePolicy,
        object_ext::{BlobExt, TreeExt},
        path, util,
    },
};

/// Rebase state stored in the repo database (legacy .libra/rebase-merge/ is migrated on demand)
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
    /// Get the path to the legacy rebase-merge directory
    fn legacy_rebase_dir() -> PathBuf {
        util::storage_path().join("rebase-merge")
    }

    /// Check if a rebase is in progress
    pub async fn is_in_progress() -> Result<bool, String> {
        let db = get_db_conn_instance().await;
        Self::ensure_rebase_state_table_exists(db).await?;
        if Self::has_state_in_db(db).await? {
            return Ok(true);
        }

        if Self::legacy_rebase_dir().exists() {
            return Self::migrate_legacy_state(db)
                .await
                .map(|state| state.is_some());
        }
        Ok(false)
    }

    /// Save rebase state to the database
    pub async fn save(&self) -> Result<(), String> {
        let db = get_db_conn_instance().await;
        Self::ensure_rebase_state_table_exists(db).await?;
        Self::save_with_conn(db, self).await
    }

    /// Load rebase state from the database (migrates legacy files if present)
    pub async fn load() -> Result<Self, String> {
        let db = get_db_conn_instance().await;
        Self::ensure_rebase_state_table_exists(db).await?;
        if let Some(state) = Self::load_from_db(db).await? {
            return Ok(state);
        }

        if let Some(state) = Self::migrate_legacy_state(db).await? {
            return Ok(state);
        }

        Err("No rebase in progress".to_string())
    }

    /// Remove the rebase state from the database (and any legacy state on disk)
    pub async fn cleanup() -> Result<(), String> {
        let db = get_db_conn_instance().await;
        Self::ensure_rebase_state_table_exists(db).await?;
        Self::clear_state_in_db(db).await?;

        let legacy_dir = Self::legacy_rebase_dir();
        if legacy_dir.exists() {
            fs::remove_dir_all(&legacy_dir).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    async fn ensure_rebase_state_table_exists<C: ConnectionTrait>(
        db: &C,
    ) -> Result<(), String> {
        let stmt = Statement::from_sql_and_values(
            DbBackend::Sqlite,
            r#"
                SELECT COUNT(*)
                FROM sqlite_master
                WHERE type='table' AND name=?;
            "#,
            ["rebase_state".into()],
        );

        if let Some(result) = db
            .query_one(stmt)
            .await
            .map_err(|e| format!("failed to check rebase_state table: {e}"))?
        {
            let count: i64 = result.try_get_by_index(0).unwrap_or(0);
            if count > 0 {
                return Ok(());
            }
        }

        let create_table_stmt = Statement::from_string(
            DbBackend::Sqlite,
            r#"
                CREATE TABLE IF NOT EXISTS `rebase_state` (
                    `id`           INTEGER PRIMARY KEY AUTOINCREMENT,
                    `head_name`    TEXT NOT NULL,
                    `onto`         TEXT NOT NULL,
                    `orig_head`    TEXT NOT NULL,
                    `current_head` TEXT NOT NULL,
                    `todo`         TEXT NOT NULL,
                    `done`         TEXT NOT NULL,
                    `stopped_sha`  TEXT
                );
            "#
            .to_string(),
        );

        db.execute(create_table_stmt)
            .await
            .map_err(|e| format!("failed to create rebase_state table: {e}"))?;
        Ok(())
    }

    async fn has_state_in_db<C: ConnectionTrait>(db: &C) -> Result<bool, String> {
        let stmt = Statement::from_string(
            DbBackend::Sqlite,
            "SELECT 1 FROM rebase_state LIMIT 1".to_string(),
        );
        let row = db
            .query_one(stmt)
            .await
            .map_err(|e| format!("failed to query rebase_state: {e}"))?;
        Ok(row.is_some())
    }

    async fn load_from_db<C: ConnectionTrait>(db: &C) -> Result<Option<Self>, String> {
        let stmt = Statement::from_string(
            DbBackend::Sqlite,
            r#"
                SELECT head_name, onto, orig_head, current_head, todo, done, stopped_sha
                FROM rebase_state
                LIMIT 1
            "#
            .to_string(),
        );
        let row = db
            .query_one(stmt)
            .await
            .map_err(|e| format!("failed to load rebase_state: {e}"))?;
        let Some(row) = row else {
            return Ok(None);
        };

        let head_name: String = row
            .try_get_by_index(0)
            .map_err(|e| format!("invalid head_name: {e}"))?;
        let onto_str: String = row
            .try_get_by_index(1)
            .map_err(|e| format!("invalid onto: {e}"))?;
        let orig_head_str: String = row
            .try_get_by_index(2)
            .map_err(|e| format!("invalid orig_head: {e}"))?;
        let current_head_str: String = row
            .try_get_by_index(3)
            .map_err(|e| format!("invalid current_head: {e}"))?;
        let todo_str: String = row
            .try_get_by_index(4)
            .map_err(|e| format!("invalid todo: {e}"))?;
        let done_str: String = row
            .try_get_by_index(5)
            .map_err(|e| format!("invalid done: {e}"))?;
        let stopped_str: Option<String> = row
            .try_get_by_index(6)
            .map_err(|e| format!("invalid stopped_sha: {e}"))?;

        let onto =
            ObjectHash::from_str(onto_str.trim()).map_err(|e| format!("Invalid onto hash: {e}"))?;
        let orig_head = ObjectHash::from_str(orig_head_str.trim())
            .map_err(|e| format!("Invalid orig_head hash: {e}"))?;
        let current_head = ObjectHash::from_str(current_head_str.trim())
            .map_err(|e| format!("Invalid current_head hash: {e}"))?;
        let todo = VecDeque::from(Self::parse_hash_list(&todo_str)?);
        let done = Self::parse_hash_list(&done_str)?;
        let stopped_sha = match stopped_str {
            Some(s) if !s.trim().is_empty() => Some(
                ObjectHash::from_str(s.trim())
                    .map_err(|e| format!("Invalid stopped_sha hash: {e}"))?,
            ),
            _ => None,
        };

        Ok(Some(RebaseState {
            head_name,
            onto,
            orig_head,
            todo,
            done,
            stopped_sha,
            current_head,
        }))
    }

    async fn save_with_conn<C: ConnectionTrait>(db: &C, state: &RebaseState) -> Result<(), String> {
        let delete_stmt =
            Statement::from_string(DbBackend::Sqlite, "DELETE FROM rebase_state".to_string());
        db.execute(delete_stmt)
            .await
            .map_err(|e| format!("failed to clear existing rebase_state: {e}"))?;

        let todo = Self::format_hash_list(state.todo.iter().cloned());
        let done = Self::format_hash_list(state.done.iter().cloned());
        let stopped_value = match &state.stopped_sha {
            Some(sha) => sha.to_string().into(),
            None => Value::String(None),
        };

        let insert_stmt = Statement::from_sql_and_values(
            DbBackend::Sqlite,
            r#"
                INSERT INTO rebase_state
                (head_name, onto, orig_head, current_head, todo, done, stopped_sha)
                VALUES (?, ?, ?, ?, ?, ?, ?);
            "#,
            [
                state.head_name.clone().into(),
                state.onto.to_string().into(),
                state.orig_head.to_string().into(),
                state.current_head.to_string().into(),
                todo.into(),
                done.into(),
                stopped_value,
            ],
        );

        db.execute(insert_stmt)
            .await
            .map_err(|e| format!("failed to save rebase_state: {e}"))?;
        Ok(())
    }

    async fn clear_state_in_db<C: ConnectionTrait>(db: &C) -> Result<(), String> {
        let stmt =
            Statement::from_string(DbBackend::Sqlite, "DELETE FROM rebase_state".to_string());
        db.execute(stmt)
            .await
            .map_err(|e| format!("failed to clear rebase_state: {e}"))?;
        Ok(())
    }

    async fn migrate_legacy_state<C: ConnectionTrait>(
        db: &C,
    ) -> Result<Option<Self>, String> {
        let legacy_dir = Self::legacy_rebase_dir();
        if !legacy_dir.exists() {
            return Ok(None);
        }

        let state = Self::load_from_legacy_dir()?;
        Self::save_with_conn(db, &state).await?;
        if let Err(e) = fs::remove_dir_all(&legacy_dir) {
            eprintln!("warning: failed to remove legacy rebase state: {e}");
        }
        Ok(Some(state))
    }

    fn load_from_legacy_dir() -> Result<Self, String> {
        let dir = Self::legacy_rebase_dir();
        if !dir.exists() {
            return Err("No rebase in progress".to_string());
        }

        let head_name_raw = fs::read_to_string(dir.join("head-name"))
            .map_err(|e| format!("Failed to read head-name: {}", e))?;
        let head_name = head_name_raw
            .trim()
            .strip_prefix("refs/heads/")
            .unwrap_or(head_name_raw.trim())
            .to_string();

        let onto_str = fs::read_to_string(dir.join("onto"))
            .map_err(|e| format!("Failed to read onto: {}", e))?;
        let onto = ObjectHash::from_str(onto_str.trim())
            .map_err(|e| format!("Invalid onto hash: {}", e))?;

        let orig_head_str = fs::read_to_string(dir.join("orig-head"))
            .map_err(|e| format!("Failed to read orig-head: {}", e))?;
        let orig_head = ObjectHash::from_str(orig_head_str.trim())
            .map_err(|e| format!("Invalid orig-head hash: {}", e))?;

        let current_head_str = fs::read_to_string(dir.join("current-head"))
            .map_err(|e| format!("Failed to read current-head: {}", e))?;
        let current_head = ObjectHash::from_str(current_head_str.trim())
            .map_err(|e| format!("Invalid current-head hash: {}", e))?;

        let todo_content = fs::read_to_string(dir.join("todo")).unwrap_or_default();
        let todo = VecDeque::from(Self::parse_hash_list(&todo_content)?);

        let done_content = fs::read_to_string(dir.join("done")).unwrap_or_default();
        let done = Self::parse_hash_list(&done_content)?;

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

    fn parse_hash_list(content: &str) -> Result<Vec<ObjectHash>, String> {
        let mut commits = Vec::new();
        for line in content.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                let hash = ObjectHash::from_str(trimmed)
                    .map_err(|e| format!("Invalid commit hash '{}': {}", trimmed, e))?;
                commits.push(hash);
            }
        }
        Ok(commits)
    }

    fn format_hash_list(list: impl IntoIterator<Item = ObjectHash>) -> String {
        let mut out = String::new();
        for (idx, hash) in list.into_iter().enumerate() {
            if idx > 0 {
                out.push('\n');
            }
            out.push_str(&hash.to_string());
        }
        out
    }
}

/// Result of attempting to replay a commit.
///
/// This enum intentionally uses `Conflict` to represent both true merge conflicts and
/// non-conflict failures that should stop the rebase. Callers must examine `message` to
/// distinguish between them and decide whether to prompt for manual resolution or abort.
pub enum ReplayResult {
    /// Commit was successfully replayed; contains the new commit hash.
    Success(ObjectHash),
    /// A replay failure occurred.
    ///
    /// - `paths` lists files that are in a conflicted state and require manual resolution.
    ///   This is empty when the replay failed for a non-conflict reason (e.g. tree/index
    ///   errors), in which case `message` should be present.
    /// - `message` carries a human-readable error describing a non-conflict failure, or
    ///   is `None` when the failure is a merge conflict that can be resolved by the user.
    ///
    /// Callers should treat `paths.is_empty()` + `message.is_some()` as a hard error and
    /// abort the rebase, while `paths` with conflicts should surface guidance for resolving
    /// those files before retrying.
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
    match RebaseState::is_in_progress().await {
        Ok(true) => {
            eprintln!("fatal: rebase already in progress");
            eprintln!("hint: use 'libra rebase --continue' to continue rebasing");
            eprintln!("hint: use 'libra rebase --abort' to abort and restore the original branch");
            eprintln!("hint: use 'libra rebase --skip' to skip this commit");
            return;
        }
        Ok(false) => {}
        Err(e) => {
            eprintln!("fatal: failed to check rebase state: {e}");
            return;
        }
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
        let current_index = match git_internal::internal::index::Index::load(&index_file) {
            Ok(index) => index,
            Err(e) => {
                eprintln!("fatal: failed to load index: {e}");
                return;
            }
        };
        let mut index = git_internal::internal::index::Index::new();
        if let Err(e) = rebuild_index_from_tree(&upstream_tree, &mut index, "") {
            eprintln!("fatal: failed to rebuild index: {}", e);
            return;
        }
        if !fast_forward_guard(&index).await {
            return;
        }

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

        if let Err(e) = index.save(&index_file) {
            eprintln!("fatal: failed to save index: {:?}", e);
            return;
        }
        if let Err(e) = reset_workdir_tracked_only(&current_index, &index) {
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

    if let Err(e) = state.save().await {
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
                if let Err(e) = state.save().await {
                    eprintln!("warning: failed to save rebase state: {}", e);
                }
            }
            ReplayResult::Conflict { paths, message } => {
                // Save state with stopped_sha
                state.stopped_sha = Some(commit_id);
                if let Err(e) = state.save().await {
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
    if let Err(e) = finalize_rebase(state).await {
        eprintln!("fatal: failed to finalize rebase: {e}");
    }
}

/// Finalize rebase after all commits are replayed
async fn finalize_rebase(state: &RebaseState) -> anyhow::Result<()> {
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
        // Attempt to restore HEAD to a safe state
        Head::update_with_conn(db, Head::Detached(state.onto), None).await;
        return Err(e).context("failed to record reflog for rebase finish");
    }

    // Reset the working directory and index to match the final state
    // This ensures that the workspace reflects the rebased commits
    let final_commit: Commit =
        load_object(&state.current_head).context("failed to load final commit for rebase")?;
    let final_tree: Tree =
        load_object(&final_commit.tree_id).context("failed to load final tree for rebase")?;

    let index_file = path::index();
    let mut index = git_internal::internal::index::Index::new();
    rebuild_index_from_tree(&final_tree, &mut index, "")
        .map_err(|e| anyhow::anyhow!(e))
        .context("failed to rebuild index from final tree")?;
    index
        .save(&index_file)
        .map_err(|e| anyhow::anyhow!(e))
        .context("failed to save index after rebase")?;
    reset_workdir_to_index(&index)
        .map_err(|e| anyhow::anyhow!(e))
        .context("failed to reset working directory after rebase")?;

    // Clean up rebase state
    if let Err(e) = RebaseState::cleanup().await {
        eprintln!("warning: failed to clean up rebase state: {}", e);
    }

    println!(
        "Successfully rebased branch '{}' onto '{}'.",
        state.head_name,
        &state.onto.to_string()[..7]
    );
    Ok(())
}

/// Continue a rebase after conflict resolution
async fn rebase_continue() {
    match RebaseState::is_in_progress().await {
        Ok(true) => {}
        Ok(false) => {
            eprintln!("fatal: no rebase in progress");
            return;
        }
        Err(e) => {
            eprintln!("fatal: failed to check rebase state: {e}");
            return;
        }
    }

    let mut state = match RebaseState::load().await {
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
                if let Err(e) = finalize_rebase(&state).await {
                    eprintln!("fatal: failed to finalize rebase: {e}");
                }
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

    if let Err(e) = state.save().await {
        eprintln!("warning: failed to save state: {}", e);
    }

    // Continue with remaining commits
    if state.todo.is_empty() {
        if let Err(e) = finalize_rebase(&state).await {
            eprintln!("fatal: failed to finalize rebase: {e}");
        }
    } else {
        let head_name = state.head_name.clone();
        let onto_display = state.onto.to_string()[..7].to_string();
        continue_replay(&mut state, &head_name, &onto_display).await;
    }
}

/// Abort the current rebase and restore the original state
async fn rebase_abort() {
    match RebaseState::is_in_progress().await {
        Ok(true) => {}
        Ok(false) => {
            eprintln!("fatal: no rebase in progress");
            return;
        }
        Err(e) => {
            eprintln!("fatal: failed to check rebase state: {e}");
            return;
        }
    }

    let state = match RebaseState::load().await {
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
    if let Err(e) = RebaseState::cleanup().await {
        eprintln!("warning: failed to clean up rebase state: {}", e);
    }

    println!("Rebase aborted. Restored branch '{}'.", state.head_name);
}

/// Skip the current commit and continue with the next
async fn rebase_skip() {
    match RebaseState::is_in_progress().await {
        Ok(true) => {}
        Ok(false) => {
            eprintln!("fatal: no rebase in progress");
            return;
        }
        Err(e) => {
            eprintln!("fatal: failed to check rebase state: {e}");
            return;
        }
    }

    let mut state = match RebaseState::load().await {
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

    let skipped_message = match load_object::<Commit>(&skipped_sha) {
        Ok(c) => Some(c.message),
        Err(e) => {
            eprintln!("warning: could not load skipped commit: {:?}", e);
            None
        }
    };

    if let Some(message) = skipped_message.as_deref() {
        println!(
            "Skipped: {} {}",
            &skipped_sha.to_string()[..7],
            message.lines().next().unwrap_or("")
        );
    } else {
        println!(
            "Skipped: {} (message unavailable)",
            &skipped_sha.to_string()[..7]
        );
    }

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

    if let Err(e) = state.save().await {
        eprintln!("warning: failed to save state: {}", e);
    }

    // Continue with remaining commits
    if state.todo.is_empty() {
        if let Err(e) = finalize_rebase(&state).await {
            eprintln!("fatal: failed to finalize rebase: {e}");
        }
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
    if cfg!(windows) { "\r\n" } else { "\n" }
}

fn try_utf8(content: &[u8]) -> Option<&str> {
    std::str::from_utf8(content).ok()
}

fn collect_tree_items_and_paths<'a>(
    trees: impl IntoIterator<Item = &'a Tree>,
) -> (Vec<HashMap<PathBuf, ObjectHash>>, HashSet<PathBuf>) {
    let mut items = Vec::new();
    let mut all_paths = HashSet::new();
    for tree in trees {
        let map: HashMap<PathBuf, ObjectHash> = tree.get_plain_items().into_iter().collect();
        all_paths.extend(map.keys().cloned());
        items.push(map);
    }
    (items, all_paths)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{HashMap, HashSet},
        path::PathBuf,
    };

    use git_internal::{
        hash::ObjectHash,
        internal::object::tree::{Tree, TreeItem, TreeItemMode},
    };

    use super::{classify_relative_to_base, collect_tree_items_and_paths, resolve_three_way};

    #[test]
    fn collect_tree_items_and_paths_unions_paths_and_preserves_items() {
        let a_hash = ObjectHash::new(&[1; 20]);
        let b_hash = ObjectHash::new(&[2; 20]);
        let b2_hash = ObjectHash::new(&[3; 20]);
        let c_hash = ObjectHash::new(&[4; 20]);

        let tree1 = Tree::from_tree_items(vec![
            TreeItem::new(TreeItemMode::Blob, a_hash, "a.txt".to_string()),
            TreeItem::new(TreeItemMode::Blob, b_hash, "b.txt".to_string()),
        ])
        .expect("tree1");

        let tree2 = Tree::from_tree_items(vec![
            TreeItem::new(TreeItemMode::Blob, b2_hash, "b.txt".to_string()),
            TreeItem::new(TreeItemMode::Blob, c_hash, "c.txt".to_string()),
        ])
        .expect("tree2");

        let (items, all_paths) = collect_tree_items_and_paths([&tree1, &tree2]);
        assert_eq!(items.len(), 2);

        let expected_first: HashMap<PathBuf, ObjectHash> = HashMap::from([
            (PathBuf::from("a.txt"), a_hash),
            (PathBuf::from("b.txt"), b_hash),
        ]);
        let expected_second: HashMap<PathBuf, ObjectHash> = HashMap::from([
            (PathBuf::from("b.txt"), b2_hash),
            (PathBuf::from("c.txt"), c_hash),
        ]);
        assert_eq!(items[0], expected_first);
        assert_eq!(items[1], expected_second);

        let expected_paths: HashSet<PathBuf> = HashSet::from([
            PathBuf::from("a.txt"),
            PathBuf::from("b.txt"),
            PathBuf::from("c.txt"),
        ]);
        assert_eq!(all_paths, expected_paths);
    }

    #[test]
    fn classify_relative_to_base_tracks_state() {
        let base = ObjectHash::new(&[1; 20]);
        let same = base;
        let modified = ObjectHash::new(&[2; 20]);

        match classify_relative_to_base(Some(&base), Some(&same)) {
            super::RelativeState::Same(hash) => assert_eq!(hash, base),
            other => panic!("expected Same, got {:?}", other),
        }

        match classify_relative_to_base(Some(&base), Some(&modified)) {
            super::RelativeState::Modified(hash) => assert_eq!(hash, modified),
            other => panic!("expected Modified, got {:?}", other),
        }

        match classify_relative_to_base(Some(&base), None) {
            super::RelativeState::Deleted => {}
            other => panic!("expected Deleted, got {:?}", other),
        }

        match classify_relative_to_base(None, Some(&modified)) {
            super::RelativeState::Added(hash) => assert_eq!(hash, modified),
            other => panic!("expected Added, got {:?}", other),
        }

        match classify_relative_to_base(None, None) {
            super::RelativeState::Missing => {}
            other => panic!("expected Missing, got {:?}", other),
        }
    }

    #[test]
    fn resolve_three_way_merges_and_conflicts() {
        let base = ObjectHash::new(&[1; 20]);
        let ours = ObjectHash::new(&[2; 20]);
        let theirs = ObjectHash::new(&[3; 20]);

        match resolve_three_way(Some(&base), Some(&base), Some(&base)) {
            super::MergeResolution::Use(hash) => assert_eq!(hash, base),
            other => panic!("expected Use(base), got {:?}", other),
        }

        match resolve_three_way(Some(&base), Some(&base), Some(&ours)) {
            super::MergeResolution::Use(hash) => assert_eq!(hash, ours),
            other => panic!("expected Use(ours), got {:?}", other),
        }

        match resolve_three_way(Some(&base), Some(&theirs), Some(&base)) {
            super::MergeResolution::Use(hash) => assert_eq!(hash, theirs),
            other => panic!("expected Use(theirs), got {:?}", other),
        }

        match resolve_three_way(Some(&base), Some(&theirs), Some(&ours)) {
            super::MergeResolution::Conflict(super::ConflictKind::BothChanged {
                ours: o,
                theirs: t,
            }) => {
                assert_eq!(o, ours);
                assert_eq!(t, theirs);
            }
            other => panic!("expected BothChanged conflict, got {:?}", other),
        }

        match resolve_three_way(None, Some(&theirs), Some(&ours)) {
            super::MergeResolution::Conflict(super::ConflictKind::BothChanged {
                ours: o,
                theirs: t,
            }) => {
                assert_eq!(o, ours);
                assert_eq!(t, theirs);
            }
            other => panic!("expected BothChanged conflict (add/add), got {:?}", other),
        }

        match resolve_three_way(Some(&base), None, Some(&ours)) {
            super::MergeResolution::Conflict(super::ConflictKind::OursModifiedTheirsDeleted {
                ours: o,
            }) => assert_eq!(o, ours),
            other => panic!(
                "expected ours-modified/theirs-deleted conflict, got {:?}",
                other
            ),
        }

        match resolve_three_way(Some(&base), Some(&theirs), None) {
            super::MergeResolution::Conflict(super::ConflictKind::TheirsModifiedOursDeleted {
                theirs: t,
            }) => assert_eq!(t, theirs),
            other => panic!(
                "expected theirs-modified/ours-deleted conflict, got {:?}",
                other
            ),
        }
    }
}

async fn fast_forward_guard(new_index: &git_internal::internal::index::Index) -> bool {
    let unstaged = status::changes_to_be_staged_with_policy(IgnorePolicy::Respect);
    if !unstaged.modified.is_empty() || !unstaged.deleted.is_empty() {
        status::execute(status::StatusArgs::default()).await;
        eprintln!("fatal: unstaged changes, can't fast-forward rebase");
        return false;
    }

    let staged = status::changes_to_be_committed().await;
    if !staged.new.is_empty() || !staged.modified.is_empty() || !staged.deleted.is_empty() {
        status::execute(status::StatusArgs::default()).await;
        eprintln!("fatal: uncommitted changes, can't fast-forward rebase");
        return false;
    }

    if let Some(conflict) = untracked_overwrite_path(&unstaged.new, new_index) {
        eprintln!(
            "fatal: untracked working tree file would be overwritten by rebase: {}",
            conflict.display()
        );
        eprintln!("hint: move or remove it before you rebase.");
        return false;
    }

    true
}

fn untracked_overwrite_path(
    untracked: &[PathBuf],
    new_index: &git_internal::internal::index::Index,
) -> Option<PathBuf> {
    let new_tracked = new_index.tracked_files();
    for untracked_path in untracked {
        for tracked_path in &new_tracked {
            if paths_conflict(untracked_path, tracked_path) {
                return Some(untracked_path.clone());
            }
        }
    }
    None
}

fn paths_conflict(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

/// Resolve a branch name or commit reference to a ObjectHash hash
///
/// This function first tries to find a branch with the given name,
/// then falls back to resolving it as a commit reference (hash, HEAD, etc.).
/// This allows the rebase command to work with both branch names and commit hashes.
async fn resolve_branch_or_commit(reference: &str) -> Result<ObjectHash, String> {
    util::get_commit_base(reference).await
}

#[derive(Debug, Copy, Clone)]
enum MergeResolution {
    Use(ObjectHash),
    Delete,
    Conflict(ConflictKind),
}

#[derive(Debug, Copy, Clone)]
enum ConflictKind {
    BothChanged {
        ours: ObjectHash,
        theirs: ObjectHash,
    },
    OursModifiedTheirsDeleted {
        ours: ObjectHash,
    },
    TheirsModifiedOursDeleted {
        theirs: ObjectHash,
    },
}

#[derive(Debug, Copy, Clone)]
enum RelativeState {
    Same(ObjectHash),
    Modified(ObjectHash),
    Deleted,
    Added(ObjectHash),
    Missing,
}

fn classify_relative_to_base(
    base: Option<&ObjectHash>,
    side: Option<&ObjectHash>,
) -> RelativeState {
    match (base, side) {
        (Some(b), Some(s)) if b == s => RelativeState::Same(*s),
        (Some(_), Some(s)) => RelativeState::Modified(*s),
        (Some(_), None) => RelativeState::Deleted,
        (None, Some(s)) => RelativeState::Added(*s),
        (None, None) => RelativeState::Missing,
    }
}

fn resolve_three_way(
    base: Option<&ObjectHash>,
    theirs: Option<&ObjectHash>,
    ours: Option<&ObjectHash>,
) -> MergeResolution {
    let base_present = base.is_some();
    let theirs_state = classify_relative_to_base(base, theirs);
    let ours_state = classify_relative_to_base(base, ours);

    match (base_present, ours_state, theirs_state) {
        (false, RelativeState::Missing, RelativeState::Missing) => MergeResolution::Delete,
        (false, RelativeState::Added(o), RelativeState::Missing) => MergeResolution::Use(o),
        (false, RelativeState::Missing, RelativeState::Added(t)) => MergeResolution::Use(t),
        (false, RelativeState::Added(o), RelativeState::Added(t)) => {
            if o == t {
                MergeResolution::Use(t)
            } else {
                MergeResolution::Conflict(ConflictKind::BothChanged { ours: o, theirs: t })
            }
        }
        (true, RelativeState::Same(o), RelativeState::Same(_)) => MergeResolution::Use(o),
        (true, RelativeState::Same(_), RelativeState::Modified(t)) => MergeResolution::Use(t),
        (true, RelativeState::Modified(o), RelativeState::Same(_)) => MergeResolution::Use(o),
        (true, RelativeState::Modified(o), RelativeState::Modified(t)) => {
            if o == t {
                MergeResolution::Use(t)
            } else {
                MergeResolution::Conflict(ConflictKind::BothChanged { ours: o, theirs: t })
            }
        }
        (true, RelativeState::Deleted, RelativeState::Same(_)) => MergeResolution::Delete,
        (true, RelativeState::Same(_), RelativeState::Deleted) => MergeResolution::Delete,
        (true, RelativeState::Deleted, RelativeState::Deleted) => MergeResolution::Delete,
        (true, RelativeState::Deleted, RelativeState::Modified(t)) => {
            MergeResolution::Conflict(ConflictKind::TheirsModifiedOursDeleted { theirs: t })
        }
        (true, RelativeState::Modified(o), RelativeState::Deleted) => {
            MergeResolution::Conflict(ConflictKind::OursModifiedTheirsDeleted { ours: o })
        }
        _ => {
            debug_assert!(false, "unexpected three-way merge state");
            MergeResolution::Delete
        }
    }
}

fn write_conflict_markers(
    workdir: &Path,
    path: &Path,
    marker_eol: &str,
    commit_abbrev: &str,
    kind: ConflictKind,
) -> Result<(), String> {
    match kind {
        ConflictKind::BothChanged { ours, theirs } => {
            let our_content = Blob::load(&ours).data;
            let their_content = Blob::load(&theirs).data;
            if let (Some(our_text), Some(their_text)) =
                (try_utf8(&our_content), try_utf8(&their_content))
            {
                let conflict_content = format!(
                    "<<<<<<< HEAD{marker_eol}{}{marker_eol}======={marker_eol}{}{marker_eol}>>>>>>> {}{marker_eol}",
                    our_text, their_text, commit_abbrev
                );
                write_conflict_file(workdir, path, &conflict_content)?;
            }
        }
        ConflictKind::OursModifiedTheirsDeleted { ours } => {
            let our_content = Blob::load(&ours).data;
            if let Some(our_text) = try_utf8(&our_content) {
                let conflict_content = format!(
                    "<<<<<<< HEAD{marker_eol}{}{marker_eol}======={marker_eol}>>>>>>> {} (deleted){marker_eol}",
                    our_text, commit_abbrev
                );
                write_conflict_file(workdir, path, &conflict_content)?;
            }
        }
        ConflictKind::TheirsModifiedOursDeleted { theirs } => {
            let their_content = Blob::load(&theirs).data;
            if let Some(their_text) = try_utf8(&their_content) {
                let conflict_content = format!(
                    "<<<<<<< HEAD (deleted){marker_eol}======={marker_eol}{}{marker_eol}>>>>>>> {}{marker_eol}",
                    their_text, commit_abbrev
                );
                write_conflict_file(workdir, path, &conflict_content)?;
            }
        }
    }
    Ok(())
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

    // Get all items from each tree and a union of their paths.
    let (tree_items, all_paths) =
        collect_tree_items_and_paths([&base_tree, &their_tree, &our_tree]);
    let base_items = &tree_items[0];
    let their_items = &tree_items[1];
    let our_items = &tree_items[2];

    let mut merged_items: HashMap<PathBuf, ObjectHash> = HashMap::new();
    let mut conflicts: Vec<PathBuf> = Vec::new();
    let workdir = util::working_dir();
    let commit_abbrev = commit_to_replay_id.to_string();
    let commit_short = &commit_abbrev[..7];
    let marker_eol = conflict_marker_eol();

    for path in all_paths {
        let base_hash = base_items.get(&path);
        let their_hash = their_items.get(&path);
        let our_hash = our_items.get(&path);

        match resolve_three_way(base_hash, their_hash, our_hash) {
            MergeResolution::Use(hash) => {
                merged_items.insert(path, hash);
            }
            MergeResolution::Delete => {}
            MergeResolution::Conflict(kind) => {
                if let Err(e) =
                    write_conflict_markers(&workdir, &path, marker_eol, commit_short, kind)
                {
                    return ReplayResult::error(e);
                }
                conflicts.push(path);
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
            if !full_path.exists() {
                continue;
            }
            if let Err(e) = fs::remove_file(&full_path) {
                return ReplayResult::Conflict {
                    paths: conflicts,
                    message: Some(format!("failed to remove {}: {}", full_path.display(), e)),
                };
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

/// Reset the working directory to match the new index state without touching untracked files.
fn reset_workdir_tracked_only(
    current_index: &git_internal::internal::index::Index,
    new_index: &git_internal::internal::index::Index,
) -> Result<(), String> {
    let workdir = util::working_dir();
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
