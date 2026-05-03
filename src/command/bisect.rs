//! Binary-search regression hunting (`libra bisect`).
//!
//! Implements the full `bisect` subcommand family (`start`, `bad`, `good`,
//! `reset`, `skip`, `log`) by walking the commit graph between a known "good"
//! ancestor and a known "bad" descendant.
//!
//! Persistent state lives in a dedicated `bisect_state` table inside the
//! repository's SQLite store; the schema is created lazily by
//! [`BisectState::ensure_bisect_state_table_exists`] and migrated in place
//! when the `completed` column is missing on older databases.
//!
//! Non-obvious responsibilities:
//! - Detached vs. branch HEAD recovery: `start` records the original branch
//!   name (if any) so `reset` can re-attach instead of leaving the user in a
//!   detached state.
//! - Working-tree safety: every `bisect good/bad/skip` calls
//!   [`restore_to_commit`], which clears the worktree (preserving `.libra/`)
//!   before re-laying the target tree. `start` therefore refuses to run with
//!   uncommitted or ignored changes that could be lost.
//! - Convergence semantics: [`BisectNext`] distinguishes between "more
//!   candidates", "single culprit found", and "all candidates skipped" so
//!   each handler can render the right user message.

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
        status::{changes_to_be_committed_safe, changes_to_be_staged_with_policy},
    },
    info_println,
    internal::{branch::Branch, config::ConfigKv, db::get_db_conn_instance, head::Head},
    utils::{
        error::{CliError, CliResult},
        ignore::IgnorePolicy,
        object_ext::TreeExt,
        output::OutputConfig,
        util,
    },
};

/// Bisect state stored in the repo database.
///
/// Persisted as a single row in the `bisect_state` SQLite table. Vector
/// fields (`good`, `skipped`) are serialised as JSON. `completed = true`
/// means the search converged; the row is preserved through completion so
/// `bisect reset` still has somewhere to read `orig_head` from.
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
    /// Returns true when a non-completed bisect session row exists. Used by
    /// the bare-repo guard and by `start` to refuse re-entry.
    pub async fn is_in_progress() -> Result<bool, String> {
        let db = get_db_conn_instance().await;
        Self::ensure_bisect_state_table_exists(&db).await?;
        Self::has_active_state_in_db(&db).await
    }

    /// Returns true when any row exists, including converged sessions whose
    /// state was preserved for `bisect reset`.
    pub async fn has_state() -> Result<bool, String> {
        let db = get_db_conn_instance().await;
        Self::ensure_bisect_state_table_exists(&db).await?;
        Self::has_any_state_in_db(&db).await
    }

    /// Persist `self` as the single row in `bisect_state`.
    ///
    /// Boundary conditions:
    /// - Wipes any pre-existing row first (the table is always single-row),
    ///   so concurrent writers race for last-writer-wins semantics.
    pub async fn save(&self) -> Result<(), String> {
        let db = get_db_conn_instance().await;
        Self::ensure_bisect_state_table_exists(&db).await?;
        Self::clear_state_in_db(&db).await?;
        Self::save_with_conn(&db, self).await
    }

    /// Load the persisted bisect state.
    ///
    /// Boundary conditions:
    /// - Returns `Err("No bisect in progress")` if the row is missing — this
    ///   propagates as a fatal CLI error in every caller.
    pub async fn load() -> Result<Self, String> {
        let db = get_db_conn_instance().await;
        Self::ensure_bisect_state_table_exists(&db).await?;
        Self::load_from_db(&db)
            .await?
            .ok_or_else(|| "No bisect in progress".to_string())
    }

    /// Drop the bisect state row. Idempotent on an empty table.
    pub async fn cleanup() -> Result<(), String> {
        let db = get_db_conn_instance().await;
        Self::ensure_bisect_state_table_exists(&db).await?;
        Self::clear_state_in_db(&db).await
    }

    /// Lazy DDL: ensure the `bisect_state` table exists and has the
    /// `completed` column.
    ///
    /// Functional scope:
    /// - Creates the table with `CREATE TABLE IF NOT EXISTS`.
    /// - Inspects `pragma_table_info` to detect older schemas missing the
    ///   `completed` column and adds it via `ALTER TABLE`.
    ///
    /// Boundary conditions:
    /// - Concurrent migrations are tolerated: if `ALTER TABLE` fails with
    ///   `"duplicate column name"` because another process won the race, the
    ///   error is swallowed and the function returns `Ok(())`.
    /// - Any other DDL failure is bubbled up as a `String` error.
    async fn ensure_bisect_state_table_exists<C: ConnectionTrait>(db: &C) -> Result<(), String> {
        // Use IF NOT EXISTS for idempotency (handles concurrent creation)
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

        // Check if completed column exists (migration for older tables without it)
        let check_column_stmt = Statement::from_string(
            DbBackend::Sqlite,
            r#"
                SELECT COUNT(*)
                FROM pragma_table_info('bisect_state')
                WHERE name='completed';
            "#
            .to_string(),
        );

        if let Some(result) = db
            .query_one(check_column_stmt)
            .await
            .map_err(|e| format!("failed to check bisect_state columns: {e}"))?
        {
            let count: i64 = result.try_get_by_index(0).unwrap_or(0);
            if count == 0 {
                // completed column doesn't exist - add it
                let alter_stmt = Statement::from_string(
                    DbBackend::Sqlite,
                    "ALTER TABLE bisect_state ADD COLUMN completed INTEGER NOT NULL DEFAULT 0;"
                        .to_string(),
                );

                // Handle concurrent migration: if another process already added the column,
                // SQLite returns "duplicate column name" error which we should treat as success
                match db.execute(alter_stmt).await {
                    Ok(_) => {}
                    Err(e) => {
                        let err_str = e.to_string();
                        if !err_str.contains("duplicate column name") {
                            return Err(format!("failed to add completed column: {e}"));
                        }
                        // Column already exists (added by concurrent process) - continue
                    }
                }
            }
        }

        Ok(())
    }

    /// Counts rows where `completed = 0`. Returns false on an empty table.
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

    /// Counts rows regardless of `completed`. Used by `reset` to recover even
    /// after the search converged.
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

    /// Insert `state` into `bisect_state`. Vector fields are JSON-encoded.
    /// Caller is responsible for clearing any existing row first.
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

    /// Read the single bisect-state row, decoding the JSON-encoded
    /// `good`/`skipped` vectors and re-parsing each `ObjectHash`. Returns
    /// `Ok(None)` when no row exists.
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

    /// Truncate the bisect-state table. Used by both `save` (before insert)
    /// and `cleanup` (after a session ends).
    async fn clear_state_in_db<C: ConnectionTrait>(db: &C) -> Result<(), String> {
        let stmt =
            Statement::from_string(DbBackend::Sqlite, "DELETE FROM bisect_state;".to_string());

        db.execute(stmt)
            .await
            .map_err(|e| format!("failed to clear bisect state: {e}"))?;

        Ok(())
    }
}

/// `--help` examples shown in `libra bisect --help` output.
pub const BISECT_EXAMPLES: &str = "\
EXAMPLES:
    libra bisect start <bad> <good>            Begin a session with explicit bounds
    libra bisect bad                           Mark the current HEAD as bad
    libra bisect good                          Mark the current HEAD as good
    libra bisect skip                          Skip the current commit and continue
    libra bisect view                          Show current state and remaining candidates
    libra bisect run cargo test                Auto-bisect by running a test command
    libra bisect run cargo test -- --ignored   Forward flags to the test command
    libra bisect log                           Print full session log
    libra bisect reset                         End the session and restore HEAD";

/// Entry point for the bisect command — dispatches the [`Bisect`] subvariant
/// to the matching `handle_*` function.
///
/// Boundary conditions:
/// - All variants forward their errors as `Err(CliError::fatal)` derived from
///   either DB failures, missing state, or rev-resolution failures.
pub async fn execute_safe(bisect_cmd: Bisect, output: &OutputConfig) -> CliResult<()> {
    match bisect_cmd {
        Bisect::Start { bad, good } => handle_start(bad, good, output).await,
        Bisect::Bad { rev } => handle_bad(rev, output).await,
        Bisect::Good { rev } => handle_good(rev, output).await,
        Bisect::Reset { rev } => handle_reset(rev, output).await,
        Bisect::Skip { rev } => handle_skip(rev, output).await,
        Bisect::Log => handle_log(output).await,
        Bisect::Run { cmd } => handle_run(cmd, output).await,
        Bisect::View => handle_view(output).await,
    }
}

/// Read `core.bare` and decide whether the repository has a working tree.
///
/// Functional scope:
/// - Parses any of Git's boolean spellings (`true/yes/on/1`,
///   `false/no/off/0`) case-insensitively.
///
/// Boundary conditions:
/// - Missing config row -> `Ok(false)` (default to non-bare).
/// - Unparseable value -> fatal `CliError`. Fails closed: bisect cannot run
///   safely if we cannot tell whether a worktree exists.
/// - Underlying DB read failure -> fatal `CliError`.
async fn is_bare_repository() -> CliResult<bool> {
    fn parse_git_bool(value: &str) -> Option<bool> {
        match value.trim() {
            v if v.eq_ignore_ascii_case("true")
                || v.eq_ignore_ascii_case("yes")
                || v.eq_ignore_ascii_case("on")
                || v == "1" =>
            {
                Some(true)
            }
            v if v.eq_ignore_ascii_case("false")
                || v.eq_ignore_ascii_case("no")
                || v.eq_ignore_ascii_case("off")
                || v == "0" =>
            {
                Some(false)
            }
            _ => None,
        }
    }

    match ConfigKv::get("core.bare").await {
        Ok(Some(entry)) => parse_git_bool(&entry.value).ok_or_else(|| {
            CliError::fatal(format!(
                "Invalid core.bare value: '{}'. Expected true/false/yes/no/on/off/1/0",
                entry.value
            ))
        }),
        Ok(None) => Ok(false), // No config = not bare
        Err(e) => Err(CliError::fatal(format!(
            "Failed to read core.bare config: {e}"
        ))),
    }
}

/// Handle `bisect start` — initialise a new search session.
///
/// Functional scope:
/// - Refuses to run on bare repos and on dirty worktrees (staged or unstaged
///   changes, ignored files included — see [`IgnorePolicy::IncludeIgnored`]).
/// - Captures the original branch (or detached commit) and stores it inside
///   [`BisectState`] so `reset` can recover it.
/// - When both `bad` and `good` are supplied, validates the bounds via
///   [`find_next_bisect_point`] *before* persisting state to avoid orphaned
///   rows on invalid input.
/// - May immediately converge: a one-commit interval is reported as the
///   culprit and the session is marked `completed`.
///
/// Boundary conditions:
/// - Returns fatal errors with hints describing recovery (commit/stash,
///   `bisect reset`, etc.).
/// - Returns fatal error in an empty repository (no current commit).
///
/// See: tests::test_bisect_start_creates_state in
/// tests/command/bisect_test.rs:115;
/// tests::test_bisect_start_already_in_progress_fails in
/// tests/command/bisect_test.rs:370.
async fn handle_start(
    bad: Option<String>,
    good: Option<String>,
    output: &OutputConfig,
) -> CliResult<()> {
    // Bare repositories have no working tree - bisect requires checkout operations
    if is_bare_repository().await? {
        return Err(CliError::fatal("bisect cannot be run in a bare repository")
            .with_hint("bisect requires a working tree to check out commits for testing"));
    }

    // Require a clean working tree to prevent data loss
    // Bisect checkout removes and restores files, which would delete untracked content
    // Use IncludeIgnored policy to catch ignored files (.env, cache dirs) that would also be deleted
    let staged = changes_to_be_committed_safe()
        .await
        .map_err(|e| CliError::fatal(format!("Failed to check staged changes: {e}")))?;
    let unstaged = changes_to_be_staged_with_policy(IgnorePolicy::IncludeIgnored)
        .map_err(|e| CliError::fatal(format!("Failed to check unstaged changes: {e}")))?;
    if !staged.is_empty() || !unstaged.is_empty() {
        return Err(CliError::fatal(
            "working tree contains uncommitted changes",
        )
        .with_hint("commit or stash your changes before running bisect. Note: each 'bisect good/bad/skip' step resets the working tree and deletes untracked/ignored files (including build artifacts), so keep important generated files outside the repo or stashed"));
    }

    // Check if there's any existing bisect state (active or completed)
    // Must use has_state to prevent overwriting preserved orig_head from a completed session
    if BisectState::has_state().await.map_err(CliError::fatal)? {
        return Err(CliError::fatal(
            "a previous bisect session exists (completed or in progress); run 'bisect reset' first",
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

    // If both bad and good are provided, validate bounds before saving state
    // This prevents leaving orphaned state if bounds are invalid
    if bad_hash.is_some() && good_hash.is_some() {
        // Validate that there are commits to test between bad and good
        if let Err(e) = find_next_bisect_point(&state).await {
            // Don't save state for invalid bounds - return error immediately
            return Err(CliError::fatal(e));
        }
    }

    state.save().await.map_err(CliError::fatal)?;

    info_println!(output, "Bisect session started");

    // If bad is provided but no good, wait for good
    if bad_hash.is_some() && good_hash.is_none() {
        info_println!(output, "Status: waiting for good commit(s)");
        return Ok(());
    }

    // If good is provided but no bad, wait for bad
    if good_hash.is_some() && bad_hash.is_none() {
        info_println!(output, "Status: waiting for bad commit");
        return Ok(());
    }

    // If both bad and good are provided, find the first bisect point (already validated above)
    if bad_hash.is_some() && good_hash.is_some() {
        match find_next_bisect_point(&state)
            .await
            .map_err(CliError::fatal)?
        {
            BisectNext::Next(next) => {
                checkout_to_bisect_point(next, &mut state, output).await?;
            }
            BisectNext::Converged => {
                // Only one commit between bad and good - it's the culprit
                let bad_commit = state.bad.ok_or_else(|| CliError::fatal("No bad commit"))?;
                let commit = load_object::<Commit>(&bad_commit)
                    .map_err(|e| CliError::fatal(format!("Failed to load commit: {e}")))?;
                let subject = commit.message.lines().next().unwrap_or("");
                info_println!(
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
            BisectNext::AllSkipped => {
                // This shouldn't happen on start since we haven't skipped anything yet
                // But handle gracefully just in case
                info_println!(
                    output,
                    "Cannot narrow down further - all commits have been skipped"
                );
                state.save().await.map_err(CliError::fatal)?;
            }
        }
    }

    Ok(())
}

/// Handle `bisect bad` — mark `rev` (or HEAD) as containing the bug.
///
/// Functional scope:
/// - Loads existing state, refuses to run on a completed session, then
///   updates `state.bad` and either waits for a good commit, advances to the
///   next test point, or converges.
///
/// Boundary conditions:
/// - Resolution failures return fatal errors via [`resolve_ref`].
/// - When `state.good` is empty the session simply records the bad commit
///   and prints "waiting for good commit(s)".
///
/// See: tests::test_bisect_mark_bad_then_good in
/// tests/command/bisect_test.rs:172;
/// tests::test_bisect_find_first_bad_commit in
/// tests/command/bisect_test.rs:211.
async fn handle_bad(rev: Option<String>, output: &OutputConfig) -> CliResult<()> {
    let mut state = BisectState::load().await.map_err(CliError::fatal)?;

    // Block operations on completed sessions - user must reset first
    if state.completed {
        return Err(
            CliError::fatal("bisect session has already found the culprit")
                .with_hint("run 'bisect reset' to end the session and restore your original HEAD"),
        );
    }

    let bad_hash = if let Some(rev) = rev {
        resolve_ref(&rev).await?
    } else {
        Head::current_commit()
            .await
            .ok_or_else(|| CliError::fatal("Cannot mark HEAD as bad - no current commit"))?
    };

    state.bad = Some(bad_hash);

    // Check if we have both good and bad
    if state.good.is_empty() {
        // No good commits yet - just save and wait for good
        state.save().await.map_err(CliError::fatal)?;
        info_println!(output, "Marked {} as bad", &bad_hash.to_string()[..7]);
        info_println!(output, "Status: waiting for good commit(s)");
        return Ok(());
    }

    // Validate bounds before printing mark message (ensures output matches actual state)
    match find_next_bisect_point(&state)
        .await
        .map_err(CliError::fatal)?
    {
        BisectNext::Next(next) => {
            // Mark is valid - print confirmation and continue
            info_println!(output, "Marked {} as bad", &bad_hash.to_string()[..7]);
            checkout_to_bisect_point(next, &mut state, output).await?;
        }
        BisectNext::Converged => {
            // We found the culprit!
            info_println!(output, "Marked {} as bad", &bad_hash.to_string()[..7]);
            let bad = state
                .bad
                .ok_or_else(|| CliError::fatal("No bad commit set"))?;
            let commit = load_object::<Commit>(&bad)
                .map_err(|e| CliError::fatal(format!("Failed to load commit: {e}")))?;
            let subject = commit.message.lines().next().unwrap_or("");
            info_println!(
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
        BisectNext::AllSkipped => {
            // Bounds valid but all candidates skipped - mark is saved
            info_println!(output, "Marked {} as bad", &bad_hash.to_string()[..7]);
            info_println!(
                output,
                "Cannot narrow down further - all commits have been skipped"
            );
            state.save().await.map_err(CliError::fatal)?;
        }
    }

    Ok(())
}

/// Handle `bisect good` — push `rev` (or HEAD) onto the known-good list.
///
/// Functional scope:
/// - Mirror image of [`handle_bad`]: appends to `state.good` and either waits
///   for a bad commit, advances to the next test point, or converges.
///
/// Boundary conditions:
/// - Refuses to run on a completed session (the user must `bisect reset`).
async fn handle_good(rev: Option<String>, output: &OutputConfig) -> CliResult<()> {
    let mut state = BisectState::load().await.map_err(CliError::fatal)?;

    // Block operations on completed sessions - user must reset first
    if state.completed {
        return Err(
            CliError::fatal("bisect session has already found the culprit")
                .with_hint("run 'bisect reset' to end the session and restore your original HEAD"),
        );
    }

    let good_hash = if let Some(rev) = rev {
        resolve_ref(&rev).await?
    } else {
        Head::current_commit()
            .await
            .ok_or_else(|| CliError::fatal("Cannot mark HEAD as good - no current commit"))?
    };

    state.good.push(good_hash);

    // Check if we have a bad commit
    if state.bad.is_none() {
        // No bad commit yet - just save and wait for bad
        state.save().await.map_err(CliError::fatal)?;
        info_println!(output, "Marked {} as good", &good_hash.to_string()[..7]);
        info_println!(output, "Status: waiting for bad commit");
        return Ok(());
    }

    // Validate bounds before printing mark message (ensures output matches actual state)
    match find_next_bisect_point(&state)
        .await
        .map_err(CliError::fatal)?
    {
        BisectNext::Next(next) => {
            // Mark is valid - print confirmation and continue
            info_println!(output, "Marked {} as good", &good_hash.to_string()[..7]);
            checkout_to_bisect_point(next, &mut state, output).await?;
        }
        BisectNext::Converged => {
            // We found the culprit!
            info_println!(output, "Marked {} as good", &good_hash.to_string()[..7]);
            let bad = state
                .bad
                .ok_or_else(|| CliError::fatal("No bad commit set"))?;
            let commit = load_object::<Commit>(&bad)
                .map_err(|e| CliError::fatal(format!("Failed to load commit: {e}")))?;
            let subject = commit.message.lines().next().unwrap_or("");
            info_println!(
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
        BisectNext::AllSkipped => {
            // Bounds valid but all candidates skipped - mark is saved
            info_println!(output, "Marked {} as good", &good_hash.to_string()[..7]);
            info_println!(
                output,
                "Cannot narrow down further - all commits have been skipped"
            );
            state.save().await.map_err(CliError::fatal)?;
        }
    }

    Ok(())
}

/// Handle `bisect reset` — terminate the session and restore HEAD.
///
/// Functional scope:
/// - With an explicit `rev`, jumps HEAD to that commit (detached).
/// - With the original branch still available, re-attaches HEAD to it via
///   [`restore_to_branch`]; otherwise falls back to a detached checkout of
///   the original commit.
/// - Always cleans up the bisect-state row last.
///
/// Boundary conditions:
/// - With no state row at all, prints "No bisect in progress" and returns
///   `Ok(())` — `reset` is the supported escape hatch for stale state.
///
/// See: tests::test_bisect_reset in tests/command/bisect_test.rs:264.
async fn handle_reset(rev: Option<String>, output: &OutputConfig) -> CliResult<()> {
    // Use has_state to check if there's any bisect state (active or completed)
    let has_state = BisectState::has_state().await.map_err(CliError::fatal)?;

    if !has_state {
        info_println!(output, "No bisect in progress");
        return Ok(());
    }

    let state = BisectState::load().await.map_err(CliError::fatal)?;

    // Determine where to reset
    let (target_hash, target_branch) = if let Some(rev) = rev {
        (resolve_ref(&rev).await?, None)
    } else if let Some(ref branch_name) = state.orig_head_name {
        // Restore to original branch - use its current commit (branch may have moved during bisect)
        match Branch::find_branch(branch_name, None).await {
            Some(branch) => (branch.commit, Some(branch_name.clone())),
            None => {
                // Branch no longer exists - fall back to orig_head commit
                (state.orig_head, None)
            }
        }
    } else {
        (state.orig_head, None)
    };

    // Restore original HEAD - use branch if available to avoid detached state
    if let Some(branch_name) = target_branch {
        restore_to_branch(branch_name, target_hash, output).await?;
    } else {
        checkout_to_commit(target_hash, output).await?;
    }

    // Clean up bisect state
    BisectState::cleanup().await.map_err(CliError::fatal)?;

    info_println!(
        output,
        "Bisect session ended, HEAD restored to {}",
        &target_hash.to_string()[..7]
    );

    Ok(())
}

/// Re-attach HEAD to `branch_name` and restore the worktree to `commit_hash`.
///
/// Functional scope:
/// - Updates HEAD inside a transaction, then verifies the update visibly
///   succeeded because [`Head::update_with_conn`] swallows write errors and
///   only logs them.
/// - Calls [`restore_to_commit`] to populate the worktree from the target
///   tree.
///
/// Boundary conditions:
/// - Transaction begin/commit failures and HEAD-mismatch detection both
///   return fatal `CliError`s with diagnostic messages.
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
    Head::update_with_conn(&txn, new_head.clone(), None).await;

    txn.commit()
        .await
        .map_err(|e| CliError::fatal(format!("Failed to commit transaction: {e}")))?;

    // Verify HEAD was updated correctly (update_with_conn logs errors but doesn't return Result)
    let actual_head = Head::current().await;
    if !matches!(actual_head, Head::Branch(ref name) if name == &branch_name) {
        return Err(CliError::fatal(format!(
            "Failed to update HEAD to branch '{}'",
            branch_name
        )));
    }

    // Restore working directory to the commit's tree
    restore_to_commit(commit_hash, output).await?;

    info_println!(
        output,
        "HEAD is now at {} (on branch {})",
        &commit_hash.to_string()[..7],
        branch_name
    );
    Ok(())
}

/// Handle `bisect skip` — exclude the current (or named) commit from
/// further candidates.
///
/// Functional scope:
/// - Pushes the commit to `state.skipped` and re-runs the search. The set of
///   skipped commits is consulted by [`get_testable_commits`].
///
/// Boundary conditions:
/// - Refuses to run on a completed session.
/// - Returns `BisectNext::AllSkipped` when every remaining candidate has been
///   skipped — the search reports the deadlock and saves state for `reset`.
///
/// See: tests::test_bisect_skip in tests/command/bisect_test.rs:309.
async fn handle_skip(rev: Option<String>, output: &OutputConfig) -> CliResult<()> {
    let mut state = BisectState::load().await.map_err(CliError::fatal)?;

    // Block operations on completed sessions - user must reset first
    if state.completed {
        return Err(
            CliError::fatal("bisect session has already found the culprit")
                .with_hint("run 'bisect reset' to end the session and restore your original HEAD"),
        );
    }

    let skip_hash = if let Some(rev) = rev {
        resolve_ref(&rev).await?
    } else {
        state
            .current
            .ok_or_else(|| CliError::fatal("No current commit to skip"))?
    };

    state.skipped.push(skip_hash);

    info_println!(output, "Skipped {}", &skip_hash.to_string()[..7]);

    // Find next bisect point
    match find_next_bisect_point(&state)
        .await
        .map_err(CliError::fatal)?
    {
        BisectNext::Next(next) => {
            checkout_to_bisect_point(next, &mut state, output).await?;
        }
        BisectNext::Converged => {
            // Should not happen in skip - no single culprit when skipping
            // But handle gracefully if all but one were skipped
            info_println!(
                output,
                "Cannot narrow down further after skip - only one candidate remains"
            );
            state.save().await.map_err(CliError::fatal)?;
        }
        BisectNext::AllSkipped => {
            info_println!(
                output,
                "Cannot narrow down further - all commits have been skipped"
            );
            state.save().await.map_err(CliError::fatal)?;
        }
    }

    Ok(())
}

/// Handle `bisect log` — print a human-readable status of the active session.
///
/// Boundary conditions:
/// - Returns the underlying load error when no session row exists; this is
///   the only handler that intentionally lacks a "no-state" early return.
///
/// See: tests::test_bisect_log in tests/command/bisect_test.rs:347.
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

    info_println!(output, "Bisect log:");
    info_println!(output, "  Bad: {}", bad_str);
    info_println!(output, "  Good: {}", good_strs);
    info_println!(output, "  Current: {}", current_str);
    info_println!(output, "  Skipped: {} commits", state.skipped.len());
    info_println!(output, "  Steps remaining: {:?}", state.steps);

    Ok(())
}

/// Handle `bisect view` — print the current bisect state in the same shape
/// as the JSON output described in the C4 plan.
///
/// Behavior:
/// - When no session row exists, returns a fatal `RepoStateInvalid` error
///   (the user must `bisect start` first).
/// - When a session is in progress (or completed), prints the current HEAD,
///   good/bad bounds, remaining-candidate count, and skipped commits.
async fn handle_view(output: &OutputConfig) -> CliResult<()> {
    if !BisectState::has_state().await.map_err(CliError::fatal)? {
        return Err(
            CliError::fatal("not in an active bisect; run `libra bisect start` first")
                .with_stable_code(crate::utils::error::StableErrorCode::RepoStateInvalid)
                .with_hint("start a bisect session before calling `bisect view`"),
        );
    }

    let state = BisectState::load().await.map_err(CliError::fatal)?;
    let head = Head::current_commit()
        .await
        .map(|h| h.to_string())
        .unwrap_or_else(|| "(none)".to_string());
    let good = state
        .good
        .first()
        .map(|h| h.to_string()[..7].to_string())
        .unwrap_or_else(|| "(unset)".to_string());
    let bad = state
        .bad
        .map(|h| h.to_string()[..7].to_string())
        .unwrap_or_else(|| "(unset)".to_string());
    let remaining = count_commits_to_test(&state).await.unwrap_or(0);
    let skipped = state
        .skipped
        .iter()
        .map(|h| h.to_string()[..7].to_string())
        .collect::<Vec<_>>();

    info_println!(
        output,
        "Bisecting between {} (good) and {} (bad)",
        good,
        bad
    );
    info_println!(output, "HEAD: {}", &head[..head.len().min(7)]);
    info_println!(output, "Remaining: {} candidate(s)", remaining);
    if skipped.is_empty() {
        info_println!(output, "Skipped: (none)");
    } else {
        info_println!(output, "Skipped: {}", skipped.join(", "));
    }
    Ok(())
}

/// Handle `bisect run <cmd> [args...]` — execute the script for each commit
/// until the search converges (or every candidate has been skipped).
///
/// Exit-code semantics (aligned with `git bisect run`):
/// - `0`            → mark good
/// - `125`          → mark skip (cannot test this commit)
/// - `1..=127`      → mark bad
/// - `128..`        → fatal: terminate the bisect with `BISECT_RUN_FAILED`
/// - signal / `None`→ fatal: same as 128+
async fn handle_run(cmd: Vec<String>, output: &OutputConfig) -> CliResult<()> {
    use std::process::Command;

    if !BisectState::is_in_progress()
        .await
        .map_err(CliError::fatal)?
    {
        return Err(
            CliError::fatal("not in an active bisect; run `libra bisect start` first")
                .with_stable_code(crate::utils::error::StableErrorCode::RepoStateInvalid)
                .with_hint("start a bisect session and mark good/bad bounds before `bisect run`"),
        );
    }

    let (executable, args) = cmd
        .split_first()
        .ok_or_else(|| CliError::fatal("`bisect run` requires a command to execute"))?;

    let mut steps = 0usize;
    let mut session_skipped: Vec<String> = Vec::new();

    loop {
        let state = BisectState::load().await.map_err(CliError::fatal)?;
        if state.completed {
            // Already converged before we got to act this iteration.
            let first_bad = state.bad.map(|h| h.to_string()).unwrap_or_default();
            info_println!(
                output,
                "Converged: first bad commit is {}",
                if first_bad.len() >= 7 {
                    &first_bad[..7]
                } else {
                    &first_bad
                }
            );
            info_println!(output, "{} steps, {} skipped", steps, session_skipped.len());
            return Ok(());
        }

        let head_short = Head::current_commit()
            .await
            .map(|h| h.to_string()[..7].to_string())
            .unwrap_or_else(|| "(no HEAD)".to_string());
        info_println!(
            output,
            "Bisecting: running `{} {}` at {}",
            executable,
            args.join(" "),
            head_short
        );

        let status = Command::new(executable).args(args).status().map_err(|e| {
            CliError::fatal(format!("failed to spawn `{executable}`: {e}"))
                .with_stable_code(crate::utils::error::StableErrorCode::IoReadFailed)
        })?;

        match status.code() {
            Some(0) => {
                steps += 1;
                handle_good(None, output).await?;
            }
            Some(125) => {
                steps += 1;
                session_skipped.push(head_short.clone());
                handle_skip(None, output).await?;
            }
            Some(code) if (1..=127).contains(&code) => {
                steps += 1;
                handle_bad(None, output).await?;
            }
            Some(code) => {
                return Err(CliError::fatal(format!(
                    "bisect run command failed with non-recoverable exit code {code}"
                ))
                .with_stable_code(crate::utils::error::StableErrorCode::InternalInvariant)
                .with_hint(
                    "re-run with a script that returns 0 (good) / 1-127 (bad) / 125 (skip)",
                ));
            }
            None => {
                return Err(
                    CliError::fatal("bisect run command terminated by signal".to_string())
                        .with_stable_code(crate::utils::error::StableErrorCode::InternalInvariant)
                        .with_hint("ensure the script exits cleanly; signals abort the bisect"),
                );
            }
        }

        // After the mark, check if the session converged or all candidates skipped.
        let next_state = BisectState::load().await.map_err(CliError::fatal)?;
        if next_state.completed {
            let first_bad = next_state.bad.map(|h| h.to_string()).unwrap_or_default();
            info_println!(
                output,
                "Converged: first bad commit is {}",
                if first_bad.len() >= 7 {
                    &first_bad[..7]
                } else {
                    &first_bad
                }
            );
            info_println!(output, "{} steps, {} skipped", steps, session_skipped.len());
            return Ok(());
        }

        let remaining = count_commits_to_test(&next_state).await.unwrap_or(0);
        if remaining == 0 {
            return Err(CliError::fatal(
                "no more candidate commits; bisect already converged".to_string(),
            )
            .with_stable_code(crate::utils::error::StableErrorCode::RepoStateInvalid)
            .with_hint("run `libra bisect view` to inspect remaining candidates"));
        }
    }
}

/// Convert a user-supplied revision string (branch, tag, hash) into the
/// concrete commit it points at. Errors out fatally with the underlying
/// resolver message if it cannot be resolved.
async fn resolve_ref(ref_str: &str) -> CliResult<ObjectHash> {
    util::get_commit_base(ref_str)
        .await
        .map_err(|e| CliError::fatal(format!("Cannot resolve '{}': {}", ref_str, e)))
}

/// Move HEAD to `commit_hash` in detached mode and lay the matching tree on
/// the worktree.
///
/// Boundary conditions:
/// - The HEAD update happens inside a SQLite transaction. The worktree
///   restore runs after commit, so a partial failure between them leaves the
///   worktree out of sync until `bisect reset`.
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

    info_println!(output, "HEAD is now at {}", &commit_hash.to_string()[..7]);
    Ok(())
}

/// Checkout to a candidate commit, record it as `state.current`, and update
/// the steps-remaining estimate.
///
/// Boundary conditions:
/// - The "remaining steps" count is only updated when `state.bad` is set;
///   otherwise the user is still in the bounds-collection phase.
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
        info_println!(
            output,
            "Bisecting: {} revisions left to test after this",
            steps
        );
    }

    Ok(())
}

/// Repaint the worktree from the tree of `commit_hash`.
///
/// Functional scope:
/// - Loads the commit and its root tree, identifies the working directory by
///   stripping `.libra` from the storage path, clears the worktree (keeping
///   the `.libra` directory itself), and restores every plain entry through
///   [`restore::restore_to_file`] so LFS pointers are honoured.
///
/// Boundary conditions:
/// - Failures to load the commit or tree are fatal — partial restoration
///   would leave the worktree corrupt.
/// - Calling this function effectively *deletes* every file under the
///   working tree that is not part of `commit_hash`'s tree, including
///   ignored files. `start` therefore guards against dirty worktrees.
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

/// Delete every top-level entry inside `workdir` except the `.libra/`
/// directory.
///
/// Boundary conditions:
/// - Used only in bisect's "burn down and lay back" worktree restore path.
///   Any I/O error aborts the restore with a fatal `CliError`.
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

/// Materialise every plain (non-tree) item from `tree` onto disk via
/// [`restore::restore_to_file`].
///
/// Boundary conditions:
/// - Stops at the first failure with a fatal error tagged with the offending
///   path. The worktree may already be partially restored.
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

/// Result of one bisect-search iteration.
///
/// Drives caller branching: keep going (`Next`), report the culprit
/// (`Converged`), or surface the deadlock (`AllSkipped`).
enum BisectNext {
    /// There are more commits to test - return the next midpoint
    Next(ObjectHash),
    /// Only one candidate remains - it's the culprit (first bad commit)
    Converged,
    /// All remaining candidates were skipped - cannot determine culprit
    AllSkipped,
}

/// Compute the next commit to check out under binary search.
///
/// Functional scope:
/// - Calls [`get_testable_commits`] to enumerate descendants of `bad` that
///   are not also ancestors of any `good` commit and have not been skipped.
/// - Returns `Converged` when only one candidate remains, `AllSkipped` when
///   the list is empty due to skips, or `Next(midpoint)` otherwise.
///
/// Boundary conditions:
/// - Returns an error string when state has no `bad` or no `good` set, or
///   when the bounds are inconsistent (no commits between them) — the
///   caller surfaces this as a fatal CLI error.
async fn find_next_bisect_point(state: &BisectState) -> Result<BisectNext, String> {
    let bad = state.bad.ok_or("No bad commit set")?;

    if state.good.is_empty() {
        return Err("No good commits set".to_string());
    }

    // Get all ancestors of bad that are not ancestors of any good commit
    let testable = get_testable_commits(&bad, &state.good, &state.skipped).await?;

    if testable.is_empty() {
        // Check if this is because all candidates were skipped
        // If there are no skipped commits, it's invalid bounds (bad is ancestor of good)
        // If there are skipped commits, check if unskipped testable would be non-empty
        if state.skipped.is_empty() {
            // No skipped commits but empty testable = invalid bounds
            return Err(
                "No commits left to test between good and bad bounds - check that good and bad commits have a valid ancestor relationship".to_string()
            );
        }
        // There are skipped commits - check if without skip filter we'd have candidates
        let testable_without_skip = get_testable_commits(&bad, &state.good, &[]).await?;
        if testable_without_skip.is_empty() {
            // Still empty without skip filter = invalid bounds
            return Err(
                "No commits left to test between good and bad bounds - check that good and bad commits have a valid ancestor relationship".to_string()
            );
        }
        // Would have candidates without skip = all candidates were skipped
        return Ok(BisectNext::AllSkipped);
    }

    // If only one commit is testable, it's the first bad commit (convergence)
    if testable.len() == 1 {
        return Ok(BisectNext::Converged);
    }

    // Find the middle commit (prefer earlier commits to narrow down faster)
    // testable is sorted oldest first, so we pick the middle index
    let mid = (testable.len() - 1) / 2;
    Ok(BisectNext::Next(testable[mid]))
}

/// Enumerate the candidate commits between `bad` and any `good`.
///
/// Functional scope:
/// - Builds the set of ancestors of every `good` commit, then BFS-walks
///   from `bad` skipping anything in that set or in `skipped`.
/// - Returns the candidates in oldest-first order, suitable for
///   midpoint selection by [`find_next_bisect_point`].
///
/// Boundary conditions:
/// - Returns an empty vector when the bounds are inconsistent or all
///   candidates have been skipped; callers must distinguish the two cases.
/// - Each commit object load is fatal on I/O / corruption errors.
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

/// Collect every transitive parent of the input commits into one set.
///
/// Boundary conditions:
/// - Each commit appears in the result; the search terminates naturally on
///   root commits whose `parent_commit_ids` is empty.
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

/// Length of the candidate list — i.e. the number of binary-search steps
/// remaining before convergence. Used purely for the user-visible
/// "X revisions left to test" message.
async fn count_commits_to_test(state: &BisectState) -> Result<usize, String> {
    let bad = state.bad.ok_or("No bad commit set")?;

    if state.good.is_empty() {
        return Err("No good commits set".to_string());
    }

    let testable = get_testable_commits(&bad, &state.good, &state.skipped).await?;
    Ok(testable.len())
}
