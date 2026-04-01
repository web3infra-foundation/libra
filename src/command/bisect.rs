//! Bisect implementation that uses binary search to find the commit that introduced a bug.
//!
//! This module provides the `bisect` command which helps locate the specific commit
//! that introduced a regression by systematically testing commits between a known
//! "good" and "bad" state.

use std::{
    collections::{HashSet, VecDeque},
    str::FromStr,
};

use git_internal::{
    hash::ObjectHash,
    internal::object::{commit::Commit, tree::Tree},
};
use sea_orm::{ConnectionTrait, DbBackend, Statement, TransactionTrait, Value};

use crate::{
    cli::Bisect,
    command::{
        load_object, restore,
        status::{changes_to_be_committed_safe, changes_to_be_staged},
    },
    internal::{config::ConfigKv, db::get_db_conn_instance, head::Head},
    utils::{
        error::{CliError, CliResult},
        object_ext::TreeExt,
        output::OutputConfig,
        util,
    },
};

/// Bisect state stored in the repo database
#[derive(Debug, Clone)]
pub struct BisectState {
    /// Original HEAD commit before bisect started
    pub orig_head: ObjectHash,
    /// Original branch name (if on branch), None if detached
    pub orig_head_name: Option<String>,
    /// Bad commit hash (the commit with the bug)
    pub bad: Option<ObjectHash>,
    /// Good commit hashes (commits known to be working)
    pub good: Vec<ObjectHash>,
    /// Current test commit being checked
    pub current: Option<ObjectHash>,
    /// Skipped commits (marked with `bisect skip`)
    pub skipped: Vec<ObjectHash>,
    /// Estimated steps remaining
    pub steps: Option<usize>,
    /// Whether bisect has found the culprit (session ended but state preserved for reset)
    pub completed: bool,
}

impl BisectState {
    /// Check if a bisect session is in progress (active, not completed)
    pub async fn is_in_progress() -> Result<bool, String> {
        let db = get_db_conn_instance().await;
        Self::ensure_bisect_state_table_exists(&db).await?;
        Self::has_active_state_in_db(&db).await
    }

    /// Check if there's any bisect state (active or completed)
    /// Used by reset to allow cleanup after bisect completes
    pub async fn has_state() -> Result<bool, String> {
        let db = get_db_conn_instance().await;
        Self::ensure_bisect_state_table_exists(&db).await?;
        Self::has_any_state_in_db(&db).await
    }

    /// Save bisect state to the database
    pub async fn save(&self) -> Result<(), String> {
        let db = get_db_conn_instance().await;
        Self::ensure_bisect_state_table_exists(&db).await?;
        Self::clear_state_in_db(&db).await?;
        Self::save_with_conn(&db, self).await
    }

    /// Load bisect state from the database
    pub async fn load() -> Result<Self, String> {
        let db = get_db_conn_instance().await;
        Self::ensure_bisect_state_table_exists(&db).await?;
        Self::load_from_db(&db)
            .await?
            .ok_or_else(|| "No bisect in progress".to_string())
    }

    /// Remove the bisect state from the database
    pub async fn cleanup() -> Result<(), String> {
        let db = get_db_conn_instance().await;
        Self::ensure_bisect_state_table_exists(&db).await?;
        Self::clear_state_in_db(&db).await
    }

    /// Create the bisect_state table if it doesn't exist
    async fn ensure_bisect_state_table_exists<C: ConnectionTrait>(db: &C) -> Result<(), String> {
        let stmt = Statement::from_sql_and_values(
            DbBackend::Sqlite,
            r#"
                SELECT COUNT(*)
                FROM sqlite_master
                WHERE type='table' AND name=?;
            "#,
            ["bisect_state".into()],
        );

        if let Some(result) = db
            .query_one(stmt)
            .await
            .map_err(|e| format!("failed to check bisect_state table: {e}"))?
        {
            let count: i64 = result.try_get_by_index(0).unwrap_or(0);
            if count > 0 {
                return Ok(());
            }
        }

        let create_table_stmt = Statement::from_string(
            DbBackend::Sqlite,
            r#"
                CREATE TABLE IF NOT EXISTS bisect_state (
                    id           INTEGER PRIMARY KEY AUTOINCREMENT,
                    orig_head    TEXT NOT NULL,
                    orig_head_name TEXT,
                    bad          TEXT,
                    good         TEXT NOT NULL,
                    current      TEXT,
                    skipped      TEXT,
                    steps        INTEGER,
                    completed    INTEGER NOT NULL DEFAULT 0
                );
            "#
            .to_string(),
        );

        db.execute(create_table_stmt)
            .await
            .map_err(|e| format!("failed to create bisect_state table: {e}"))?;

        Ok(())
    }

    async fn has_active_state_in_db<C: ConnectionTrait>(db: &C) -> Result<bool, String> {
        // Check if there's an in-progress (not completed) bisect session
        let stmt = Statement::from_string(
            DbBackend::Sqlite,
            "SELECT COUNT(*) FROM bisect_state WHERE completed = 0;".to_string(),
        );

        if let Some(result) = db
            .query_one(stmt)
            .await
            .map_err(|e| format!("failed to query bisect_state: {e}"))?
        {
            let count: i64 = result.try_get_by_index(0).unwrap_or(0);
            return Ok(count > 0);
        }

        Ok(false)
    }

    async fn has_any_state_in_db<C: ConnectionTrait>(db: &C) -> Result<bool, String> {
        // Check if there's any bisect state (active or completed)
        let stmt = Statement::from_string(
            DbBackend::Sqlite,
            "SELECT COUNT(*) FROM bisect_state;".to_string(),
        );

        if let Some(result) = db
            .query_one(stmt)
            .await
            .map_err(|e| format!("failed to query bisect_state: {e}"))?
        {
            let count: i64 = result.try_get_by_index(0).unwrap_or(0);
            return Ok(count > 0);
        }

        Ok(false)
    }

    async fn save_with_conn<C: ConnectionTrait>(db: &C, state: &BisectState) -> Result<(), String> {
        let good_json = serde_json::to_string(&state.good)
            .map_err(|e| format!("failed to serialize good commits: {e}"))?;
        let skipped_json = serde_json::to_string(&state.skipped)
            .map_err(|e| format!("failed to serialize skipped commits: {e}"))?;

        let stmt = Statement::from_sql_and_values(
            DbBackend::Sqlite,
            r#"
                INSERT INTO bisect_state (orig_head, orig_head_name, bad, good, current, skipped, steps, completed)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?);
            "#,
            [
                state.orig_head.to_string().into(),
                state
                    .orig_head_name
                    .clone()
                    .map(|s| s.into())
                    .unwrap_or(Value::String(None)),
                state
                    .bad
                    .map(|h| h.to_string().into())
                    .unwrap_or(Value::String(None)),
                good_json.into(),
                state
                    .current
                    .map(|h| h.to_string().into())
                    .unwrap_or(Value::String(None)),
                skipped_json.into(),
                state
                    .steps
                    .map(|s| s as i64)
                    .map(|v| v.into())
                    .unwrap_or(Value::BigInt(None)),
                (state.completed as i64).into(),
            ],
        );

        db.execute(stmt)
            .await
            .map_err(|e| format!("failed to save bisect state: {e}"))?;

        Ok(())
    }

    async fn load_from_db<C: ConnectionTrait>(db: &C) -> Result<Option<BisectState>, String> {
        let stmt = Statement::from_string(
            DbBackend::Sqlite,
            "SELECT orig_head, orig_head_name, bad, good, current, skipped, steps, completed FROM bisect_state LIMIT 1;".to_string(),
        );

        if let Some(result) = db
            .query_one(stmt)
            .await
            .map_err(|e| format!("failed to load bisect state: {e}"))?
        {
            let orig_head_str: String = result
                .try_get_by_index(0)
                .map_err(|e| format!("failed to read orig_head: {e}"))?;
            let orig_head_name: Option<String> = result.try_get_by_index(1).ok();
            let bad_str: Option<String> = result.try_get_by_index(2).ok();
            let good_json: String = result
                .try_get_by_index(3)
                .map_err(|e| format!("failed to read good: {e}"))?;
            let current_str: Option<String> = result.try_get_by_index(4).ok();
            let skipped_json: Option<String> = result.try_get_by_index(5).ok();
            let steps: Option<i64> = result.try_get_by_index(6).ok();
            let completed: i64 = result.try_get_by_index(7).unwrap_or(0);

            let orig_head = ObjectHash::from_str(&orig_head_str)
                .map_err(|e| format!("invalid orig_head hash: {e}"))?;

            let bad = bad_str.and_then(|s| ObjectHash::from_str(&s).ok());

            let good: Vec<ObjectHash> = serde_json::from_str(&good_json)
                .map_err(|e| format!("failed to parse good commits: {e}"))?;

            let current = current_str.and_then(|s| ObjectHash::from_str(&s).ok());

            let skipped: Vec<ObjectHash> = skipped_json
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default();

            return Ok(Some(BisectState {
                orig_head,
                orig_head_name,
                bad,
                good,
                current,
                skipped,
                steps: steps.map(|s| s as usize),
                completed: completed != 0,
            }));
        }

        Ok(None)
    }

    async fn clear_state_in_db<C: ConnectionTrait>(db: &C) -> Result<(), String> {
        let stmt =
            Statement::from_string(DbBackend::Sqlite, "DELETE FROM bisect_state;".to_string());

        db.execute(stmt)
            .await
            .map_err(|e| format!("failed to clear bisect state: {e}"))?;

        Ok(())
    }
}

/// Entry point for the bisect command
pub async fn execute_safe(bisect_cmd: Bisect, output: &OutputConfig) -> CliResult<()> {
    match bisect_cmd {
        Bisect::Start { bad, good } => handle_start(bad, good, output).await,
        Bisect::Bad { rev } => handle_bad(rev, output).await,
        Bisect::Good { rev } => handle_good(rev, output).await,
        Bisect::Reset { rev } => handle_reset(rev, output).await,
        Bisect::Skip { rev } => handle_skip(rev, output).await,
        Bisect::Log => handle_log(output).await,
    }
}

/// Check if the repository is bare (no working tree)
async fn is_bare_repository() -> bool {
    matches!(
        ConfigKv::get("core.bare").await.ok().flatten().map(|e| e.value),
        Some(value) if value.eq_ignore_ascii_case("true")
    )
}

/// Handle `bisect start` - initialize a new bisect session
async fn handle_start(
    bad: Option<String>,
    good: Option<String>,
    output: &OutputConfig,
) -> CliResult<()> {
    // Bare repositories have no working tree - bisect requires checkout operations
    if is_bare_repository().await {
        return Err(CliError::fatal(
            "bisect cannot be run in a bare repository",
        )
        .with_hint("bisect requires a working tree to check out commits for testing"));
    }

    // Require a clean working tree to prevent data loss
    // Bisect checkout removes and restores files, which would delete untracked content
    let staged = changes_to_be_committed_safe()
        .await
        .map_err(|e| CliError::fatal(format!("Failed to check staged changes: {e}")))?;
    let unstaged = changes_to_be_staged()
        .map_err(|e| CliError::fatal(format!("Failed to check unstaged changes: {e}")))?;
    if !staged.is_empty() || !unstaged.is_empty() {
        return Err(CliError::fatal(
            "working tree contains uncommitted changes",
        )
        .with_hint("commit or stash your changes before running bisect to prevent data loss"));
    }

    // Check if there's any existing bisect state (active or completed)
    // Must use has_state to prevent overwriting preserved orig_head from a completed session
    if BisectState::has_state()
        .await
        .map_err(CliError::fatal)?
    {
        return Err(CliError::fatal(
            "bisect is already in progress, use 'bisect reset' to end it first",
        ));
    }

    // Save original HEAD state
    let orig_head = Head::current_commit()
        .await
        .ok_or_else(|| CliError::fatal("Cannot start bisect in an empty repository"))?;

    let orig_head_name = match Head::current().await {
        Head::Branch(name) => Some(name),
        Head::Detached(_) => None,
    };

    // Parse optional bad and good commits
    let bad_hash = if let Some(bad_ref) = bad {
        Some(resolve_ref(&bad_ref).await?)
    } else {
        None
    };

    let good_hash = if let Some(good_ref) = good {
        Some(resolve_ref(&good_ref).await?)
    } else {
        None
    };

    let mut state = BisectState {
        orig_head,
        orig_head_name,
        bad: bad_hash,
        good: good_hash.map(|h| vec![h]).unwrap_or_default(),
        current: None,
        skipped: vec![],
        steps: None,
        completed: false,
    };

    state.save().await.map_err(CliError::fatal)?;

    crate::info_println!(output, "Bisect session started");

    // If bad is provided but no good, wait for good
    if bad_hash.is_some() && good_hash.is_none() {
        crate::info_println!(output, "Status: waiting for good commit(s)");
        return Ok(());
    }

    // If good is provided but no bad, wait for bad
    if good_hash.is_some() && bad_hash.is_none() {
        crate::info_println!(output, "Status: waiting for bad commit");
        return Ok(());
    }

    // If both bad and good are provided, try to find the first bisect point
    if bad_hash.is_some() && good_hash.is_some() {
        match find_next_bisect_point(&state)
            .await
            .map_err(CliError::fatal)?
        {
            Some(next) => {
                checkout_to_bisect_point(next, &mut state, output).await?;
            }
            None => {
                // Only one commit between bad and good - it's the culprit
                let bad_commit = state.bad.ok_or_else(|| CliError::fatal("No bad commit"))?;
                let commit = load_object::<Commit>(&bad_commit)
                    .map_err(|e| CliError::fatal(format!("Failed to load commit: {e}")))?;
                let subject = commit.message.lines().next().unwrap_or("");
                crate::info_println!(
                    output,
                    "{} is the first bad commit\n{}",
                    &bad_commit.to_string()[..7],
                    subject
                );
                // Move HEAD to the culprit commit, mark completed but keep state for reset
                checkout_to_commit(bad_commit, output).await?;
                state.current = Some(bad_commit);
                state.completed = true;
                state.save().await.map_err(CliError::fatal)?;
            }
        }
    }

    Ok(())
}

/// Handle `bisect bad` - mark a commit as bad
async fn handle_bad(rev: Option<String>, output: &OutputConfig) -> CliResult<()> {
    let mut state = BisectState::load().await.map_err(CliError::fatal)?;

    let bad_hash = if let Some(rev) = rev {
        resolve_ref(&rev).await?
    } else {
        Head::current_commit()
            .await
            .ok_or_else(|| CliError::fatal("Cannot mark HEAD as bad - no current commit"))?
    };

    state.bad = Some(bad_hash);

    crate::info_println!(output, "Marked {} as bad", &bad_hash.to_string()[..7]);

    // Check if we have both good and bad
    if state.good.is_empty() {
        state.save().await.map_err(CliError::fatal)?;
        crate::info_println!(output, "Status: waiting for good commit(s)");
        return Ok(());
    }

    // Find next bisect point
    if let Some(next) = find_next_bisect_point(&state)
        .await
        .map_err(CliError::fatal)?
    {
        checkout_to_bisect_point(next, &mut state, output).await?;
    } else {
        // We found the culprit!
        let bad = state
            .bad
            .ok_or_else(|| CliError::fatal("No bad commit set"))?;
        let commit = load_object::<Commit>(&bad)
            .map_err(|e| CliError::fatal(format!("Failed to load commit: {e}")))?;
        let subject = commit.message.lines().next().unwrap_or("");
        crate::info_println!(
            output,
            "{} is the first bad commit\n{}",
            &bad.to_string()[..7],
            subject
        );
        // Move HEAD to the culprit commit, mark completed but keep state for reset
        checkout_to_commit(bad, output).await?;
        state.current = Some(bad);
        state.completed = true;
        state.save().await.map_err(CliError::fatal)?;
    }

    Ok(())
}

/// Handle `bisect good` - mark a commit as good
async fn handle_good(rev: Option<String>, output: &OutputConfig) -> CliResult<()> {
    let mut state = BisectState::load().await.map_err(CliError::fatal)?;

    let good_hash = if let Some(rev) = rev {
        resolve_ref(&rev).await?
    } else {
        Head::current_commit()
            .await
            .ok_or_else(|| CliError::fatal("Cannot mark HEAD as good - no current commit"))?
    };

    state.good.push(good_hash);

    crate::info_println!(output, "Marked {} as good", &good_hash.to_string()[..7]);

    // Check if we have a bad commit
    if state.bad.is_none() {
        state.save().await.map_err(CliError::fatal)?;
        crate::info_println!(output, "Status: waiting for bad commit");
        return Ok(());
    }

    // Find next bisect point
    if let Some(next) = find_next_bisect_point(&state)
        .await
        .map_err(CliError::fatal)?
    {
        checkout_to_bisect_point(next, &mut state, output).await?;
    } else {
        // We found the culprit!
        let bad = state
            .bad
            .ok_or_else(|| CliError::fatal("No bad commit set"))?;
        let commit = load_object::<Commit>(&bad)
            .map_err(|e| CliError::fatal(format!("Failed to load commit: {e}")))?;
        let subject = commit.message.lines().next().unwrap_or("");
        crate::info_println!(
            output,
            "{} is the first bad commit\n{}",
            &bad.to_string()[..7],
            subject
        );
        // Move HEAD to the culprit commit, mark completed but keep state for reset
        checkout_to_commit(bad, output).await?;
        state.current = Some(bad);
        state.completed = true;
        state.save().await.map_err(CliError::fatal)?;
    }

    Ok(())
}

/// Handle `bisect reset` - end the bisect session
async fn handle_reset(rev: Option<String>, output: &OutputConfig) -> CliResult<()> {
    // Use has_state to check if there's any bisect state (active or completed)
    let has_state = BisectState::has_state()
        .await
        .map_err(CliError::fatal)?;

    if !has_state {
        crate::info_println!(output, "No bisect in progress");
        return Ok(());
    }

    let state = BisectState::load().await.map_err(CliError::fatal)?;

    // Determine where to reset
    let (target_hash, target_branch) = if let Some(rev) = rev {
        (resolve_ref(&rev).await?, None)
    } else {
        (state.orig_head, state.orig_head_name.clone())
    };

    // Restore original HEAD - use branch if available to avoid detached state
    if let Some(branch_name) = target_branch {
        restore_to_branch(branch_name, target_hash, output).await?;
    } else {
        checkout_to_commit(target_hash, output).await?;
    }

    // Clean up bisect state
    BisectState::cleanup().await.map_err(CliError::fatal)?;

    crate::info_println!(
        output,
        "Bisect session ended, HEAD restored to {}",
        &target_hash.to_string()[..7]
    );

    Ok(())
}

/// Restore HEAD to a branch (avoids detached state after reset)
async fn restore_to_branch(
    branch_name: String,
    commit_hash: ObjectHash,
    output: &OutputConfig,
) -> CliResult<()> {
    let db = get_db_conn_instance().await;

    let txn = db
        .begin()
        .await
        .map_err(|e| CliError::fatal(format!("Failed to begin transaction: {e}")))?;

    // Update HEAD to point to the branch
    let new_head = Head::Branch(branch_name.clone());
    Head::update_with_conn(&txn, new_head, None).await;

    txn.commit()
        .await
        .map_err(|e| CliError::fatal(format!("Failed to commit transaction: {e}")))?;

    // Restore working directory to the commit's tree
    restore_to_commit(commit_hash, output).await?;

    crate::info_println!(
        output,
        "HEAD is now at {} (on branch {})",
        &commit_hash.to_string()[..7],
        branch_name
    );
    Ok(())
}

/// Handle `bisect skip` - skip the current commit
async fn handle_skip(rev: Option<String>, output: &OutputConfig) -> CliResult<()> {
    let mut state = BisectState::load().await.map_err(CliError::fatal)?;

    let skip_hash = if let Some(rev) = rev {
        resolve_ref(&rev).await?
    } else {
        state
            .current
            .ok_or_else(|| CliError::fatal("No current commit to skip"))?
    };

    state.skipped.push(skip_hash);

    crate::info_println!(output, "Skipped {}", &skip_hash.to_string()[..7]);

    // Find next bisect point
    if let Some(next) = find_next_bisect_point(&state)
        .await
        .map_err(CliError::fatal)?
    {
        checkout_to_bisect_point(next, &mut state, output).await?;
    } else {
        crate::info_println!(
            output,
            "Cannot narrow down further - all commits have been skipped"
        );
        state.save().await.map_err(CliError::fatal)?;
    }

    Ok(())
}

/// Handle `bisect log` - show the bisect log
async fn handle_log(output: &OutputConfig) -> CliResult<()> {
    let state = BisectState::load().await.map_err(CliError::fatal)?;

    let bad_str = state
        .bad
        .map(|h| h.to_string()[..7].to_string())
        .unwrap_or_else(|| "not set".to_string());

    let good_strs = state
        .good
        .iter()
        .map(|h| h.to_string()[..7].to_string())
        .collect::<Vec<_>>()
        .join(", ");

    let current_str = state
        .current
        .map(|h| h.to_string()[..7].to_string())
        .unwrap_or_else(|| "not set".to_string());

    crate::info_println!(output, "Bisect log:");
    crate::info_println!(output, "  Bad: {}", bad_str);
    crate::info_println!(output, "  Good: {}", good_strs);
    crate::info_println!(output, "  Current: {}", current_str);
    crate::info_println!(output, "  Skipped: {} commits", state.skipped.len());
    crate::info_println!(output, "  Steps remaining: {:?}", state.steps);

    Ok(())
}

/// Resolve a reference (branch name, commit hash, etc.) to a commit hash
async fn resolve_ref(ref_str: &str) -> CliResult<ObjectHash> {
    util::get_commit_base(ref_str)
        .await
        .map_err(|e| CliError::fatal(format!("Cannot resolve '{}': {}", ref_str, e)))
}

/// Checkout to a specific commit (for bisect)
async fn checkout_to_commit(commit_hash: ObjectHash, output: &OutputConfig) -> CliResult<()> {
    let db = get_db_conn_instance().await;

    let txn = db
        .begin()
        .await
        .map_err(|e| CliError::fatal(format!("Failed to begin transaction: {e}")))?;

    let new_head = Head::Detached(commit_hash);
    Head::update_with_conn(&txn, new_head, None).await;

    txn.commit()
        .await
        .map_err(|e| CliError::fatal(format!("Failed to commit transaction: {e}")))?;

    // Restore working directory
    restore_to_commit(commit_hash, output).await?;

    crate::info_println!(output, "HEAD is now at {}", &commit_hash.to_string()[..7]);
    Ok(())
}

/// Checkout to a bisect point and update state
async fn checkout_to_bisect_point(
    commit_hash: ObjectHash,
    state: &mut BisectState,
    output: &OutputConfig,
) -> CliResult<()> {
    checkout_to_commit(commit_hash, output).await?;

    state.current = Some(commit_hash);

    // Calculate remaining steps
    if state.bad.is_some() {
        let remaining = count_commits_to_test(state)
            .await
            .map_err(CliError::fatal)?;
        state.steps = Some(remaining);
    }

    state.save().await.map_err(CliError::fatal)?;

    if let Some(steps) = state.steps {
        crate::info_println!(
            output,
            "Bisecting: {} revisions left to test after this",
            steps
        );
    }

    Ok(())
}

/// Restore working directory to a commit's tree
async fn restore_to_commit(commit_hash: ObjectHash, _output: &OutputConfig) -> CliResult<()> {
    let commit = load_object::<Commit>(&commit_hash)
        .map_err(|e| CliError::fatal(format!("Failed to load commit: {e}")))?;

    let tree = load_object::<Tree>(&commit.tree_id)
        .map_err(|e| CliError::fatal(format!("Failed to load tree: {e}")))?;

    let workdir = util::try_get_storage_path(None)
        .map_err(|e| CliError::fatal(format!("Cannot find storage path: {e}")))?;
    let workdir = workdir
        .parent()
        .ok_or_else(|| CliError::fatal("Cannot find working directory"))?
        .to_path_buf();

    // Clear working directory (except .libra)
    clear_workdir_except_libra(&workdir)?;

    // Restore files from tree (handles LFS pointers via restore::restore_to_file)
    restore_tree_to_workdir(&tree).await?;

    Ok(())
}

/// Clear working directory, preserving .libra directory
fn clear_workdir_except_libra(workdir: &std::path::Path) -> CliResult<()> {
    for entry in std::fs::read_dir(workdir)
        .map_err(|e| CliError::fatal(format!("Failed to read workdir: {e}")))?
    {
        let entry = entry.map_err(|e| CliError::fatal(format!("Failed to read entry: {e}")))?;
        let path = entry.path();

        // Skip .libra directory
        if path.file_name().map(|n| n == ".libra").unwrap_or(false) {
            continue;
        }

        if path.is_dir() {
            std::fs::remove_dir_all(&path).map_err(|e| {
                CliError::fatal(format!("Failed to remove dir {}: {}", path.display(), e))
            })?;
        } else {
            std::fs::remove_file(&path).map_err(|e| {
                CliError::fatal(format!("Failed to remove file {}: {}", path.display(), e))
            })?;
        }
    }

    Ok(())
}

/// Restore tree contents to working directory
/// Uses restore::restore_to_file to properly handle LFS pointers
async fn restore_tree_to_workdir(tree: &Tree) -> CliResult<()> {
    let items = tree.get_plain_items();
    for (path, hash) in items {
        // path is already a PathBuf relative to workdir
        restore::restore_to_file(&hash, &path).await.map_err(|e| {
            CliError::fatal(format!("Failed to restore file {}: {}", path.display(), e))
        })?;
    }

    Ok(())
}

/// Find the next commit to test using binary search
async fn find_next_bisect_point(state: &BisectState) -> Result<Option<ObjectHash>, String> {
    let bad = state.bad.ok_or("No bad commit set")?;

    if state.good.is_empty() {
        return Err("No good commits set".to_string());
    }

    // Get all ancestors of bad that are not ancestors of any good commit
    let testable = get_testable_commits(&bad, &state.good, &state.skipped).await?;

    if testable.is_empty() {
        // Empty testable set indicates invalid input (e.g., same commit marked both good and bad)
        return Err(
            "No commits left to test between good and bad bounds - check that good and bad commits have a valid ancestor relationship".to_string()
        );
    }

    // If only one commit is testable, it's the first bad commit
    if testable.len() == 1 {
        return Ok(None);
    }

    // Find the middle commit (prefer earlier commits to narrow down faster)
    // testable is sorted oldest first, so we pick the middle index
    let mid = (testable.len() - 1) / 2;
    Ok(Some(testable[mid]))
}

/// Get all commits that could be tested (ancestors of bad, not ancestors of good)
async fn get_testable_commits(
    bad: &ObjectHash,
    good: &[ObjectHash],
    skipped: &[ObjectHash],
) -> Result<Vec<ObjectHash>, String> {
    // Build set of good ancestors
    let good_ancestors: HashSet<ObjectHash> = get_all_ancestors(good).await?;

    // Build set of skipped commits
    let skipped_set: HashSet<ObjectHash> = skipped.iter().copied().collect();

    // BFS from bad, collecting commits not in good_ancestors or skipped
    let mut queue = VecDeque::new();
    let mut visited = HashSet::new();
    let mut testable = Vec::new();

    queue.push_back(*bad);

    while let Some(commit_hash) = queue.pop_front() {
        if visited.contains(&commit_hash) {
            continue;
        }
        visited.insert(commit_hash);

        // Skip if this is a good ancestor
        if good_ancestors.contains(&commit_hash) {
            continue;
        }

        // Skip if explicitly marked as skipped
        if skipped_set.contains(&commit_hash) {
            continue;
        }

        let commit = load_object::<Commit>(&commit_hash)
            .map_err(|e| format!("Failed to load commit {}: {}", commit_hash, e))?;

        // Add to testable list
        testable.push(commit_hash);

        // Add parents to queue
        for parent in &commit.parent_commit_ids {
            queue.push_back(*parent);
        }
    }

    // Sort by commit order (oldest first for proper bisect ordering)
    // We reverse the order since BFS gives us newest first
    testable.reverse();

    Ok(testable)
}

/// Get all ancestors of a set of commits
async fn get_all_ancestors(commits: &[ObjectHash]) -> Result<HashSet<ObjectHash>, String> {
    let mut ancestors = HashSet::new();
    let mut queue = VecDeque::new();

    for commit in commits {
        queue.push_back(*commit);
    }

    while let Some(commit_hash) = queue.pop_front() {
        if ancestors.contains(&commit_hash) {
            continue;
        }
        ancestors.insert(commit_hash);

        let commit = load_object::<Commit>(&commit_hash)
            .map_err(|e| format!("Failed to load commit {}: {}", commit_hash, e))?;

        for parent in &commit.parent_commit_ids {
            queue.push_back(*parent);
        }
    }

    Ok(ancestors)
}

/// Count remaining commits to test
async fn count_commits_to_test(state: &BisectState) -> Result<usize, String> {
    let bad = state.bad.ok_or("No bad commit set")?;

    if state.good.is_empty() {
        return Err("No good commits set".to_string());
    }

    let testable = get_testable_commits(&bad, &state.good, &state.skipped).await?;
    Ok(testable.len())
}
