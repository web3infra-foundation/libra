//! Rebase implementation that parses onto/branch arguments, replays commits onto a new base, handles conflicts, and updates branch refs.

use std::{
    borrow::Cow,
    collections::{HashMap, HashSet, VecDeque},
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::Context;
use clap::{Parser, ValueEnum};
use git_internal::{
    hash::ObjectHash,
    internal::object::{
        ObjectTrait,
        blob::Blob,
        commit::Commit,
        tree::{Tree, TreeItem, TreeItemMode},
        types::ObjectType,
    },
};
use sea_orm::{ConnectionTrait, DbBackend, Statement, TransactionTrait, Value};
use serde::Serialize;

use crate::{
    cli_error,
    command::{commit, load_object, merge, merge_base, save_object, stash, status, switch},
    common_utils::{format_commit_msg, parse_commit_msg},
    internal::{
        branch::Branch,
        config::{LocalIdentityTarget, read_cascaded_config_value},
        db::get_db_conn_instance,
        head::Head,
        reflog,
        reflog::{ReflogAction, ReflogContext, ReflogError, with_reflog},
    },
    utils::{
        error::{CliError, CliResult, StableErrorCode, emit_warning},
        ignore::IgnorePolicy,
        object_ext::{BlobExt, TreeExt},
        output::{OutputConfig, emit_json_data},
        path, util, worktree,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EmptyMode {
    Drop,
    Keep,
    Stop,
}

impl EmptyMode {
    fn as_str(self) -> &'static str {
        match self {
            EmptyMode::Drop => "drop",
            EmptyMode::Keep => "keep",
            EmptyMode::Stop => "stop",
        }
    }

    fn from_db(value: &str) -> Self {
        match value {
            "keep" => EmptyMode::Keep,
            "stop" => EmptyMode::Stop,
            _ => EmptyMode::Drop,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RebaseRuntimeOptions {
    pub autosquash: bool,
    pub reapply_cherry_picks: bool,
    pub keep_empty: bool,
    pub empty_mode: EmptyMode,
    pub signoff: bool,
    pub gpg_sign: bool,
}

impl Default for RebaseRuntimeOptions {
    fn default() -> Self {
        Self {
            autosquash: false,
            reapply_cherry_picks: false,
            keep_empty: true,
            empty_mode: EmptyMode::Drop,
            signoff: false,
            gpg_sign: false,
        }
    }
}

/// Rebase state stored in the repo database
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
    pub autostash_ref: Option<String>,
    pub options: RebaseRuntimeOptions,
}

impl RebaseState {
    /// Get the path to the legacy rebase-merge directory
    fn legacy_rebase_dir() -> PathBuf {
        util::storage_path().join("rebase-merge")
    }

    /// Check if a rebase is in progress
    pub async fn is_in_progress() -> Result<bool, String> {
        let db = get_db_conn_instance().await;
        Self::ensure_rebase_state_table_exists(&db).await?;
        if Self::has_state_in_db(&db).await? {
            return Ok(true);
        }

        if Self::legacy_rebase_dir().exists() {
            return Self::migrate_legacy_state(&db)
                .await
                .map(|state| state.is_some());
        }
        Ok(false)
    }

    /// Save rebase state to the database
    pub async fn save(&self) -> Result<(), String> {
        let db = get_db_conn_instance().await;
        Self::ensure_rebase_state_table_exists(&db).await?;
        Self::save_with_conn(&db, self).await
    }

    /// Load rebase state from the database (migrates legacy files if present)
    pub async fn load() -> Result<Self, String> {
        let db = get_db_conn_instance().await;
        Self::ensure_rebase_state_table_exists(&db).await?;
        if let Some(state) = Self::load_from_db(&db).await? {
            return Ok(state);
        }

        if let Some(state) = Self::migrate_legacy_state(&db).await? {
            return Ok(state);
        }

        Err("No rebase in progress".to_string())
    }

    /// Remove the rebase state from the database (and any legacy state on disk)
    pub async fn cleanup() -> Result<(), String> {
        let db = get_db_conn_instance().await;
        Self::ensure_rebase_state_table_exists(&db).await?;
        Self::clear_state_in_db(&db).await?;

        let legacy_dir = Self::legacy_rebase_dir();
        if legacy_dir.exists() {
            fs::remove_dir_all(&legacy_dir).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    async fn ensure_rebase_state_table_exists<C: ConnectionTrait>(db: &C) -> Result<(), String> {
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
                Self::ensure_rebase_state_columns(db).await?;
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
                    `stopped_sha`  TEXT,
                    `autostash_ref` TEXT,
                    `autosquash` INTEGER NOT NULL DEFAULT 0,
                    `reapply_cherry_picks` INTEGER NOT NULL DEFAULT 0,
                    `keep_empty` INTEGER NOT NULL DEFAULT 1,
                    `empty_mode` TEXT NOT NULL DEFAULT 'drop',
                    `signoff` INTEGER NOT NULL DEFAULT 0,
                    `gpg_sign` INTEGER NOT NULL DEFAULT 0
                );
            "#
            .to_string(),
        );

        db.execute(create_table_stmt)
            .await
            .map_err(|e| format!("failed to create rebase_state table: {e}"))?;
        Ok(())
    }

    async fn ensure_rebase_state_columns<C: ConnectionTrait>(db: &C) -> Result<(), String> {
        for (column, definition) in [
            ("autostash_ref", "TEXT"),
            ("autosquash", "INTEGER NOT NULL DEFAULT 0"),
            ("reapply_cherry_picks", "INTEGER NOT NULL DEFAULT 0"),
            ("keep_empty", "INTEGER NOT NULL DEFAULT 1"),
            ("empty_mode", "TEXT NOT NULL DEFAULT 'drop'"),
            ("signoff", "INTEGER NOT NULL DEFAULT 0"),
            ("gpg_sign", "INTEGER NOT NULL DEFAULT 0"),
        ] {
            if Self::rebase_state_has_column(db, column).await? {
                continue;
            }
            let stmt = Statement::from_string(
                DbBackend::Sqlite,
                format!("ALTER TABLE rebase_state ADD COLUMN `{column}` {definition};"),
            );
            db.execute(stmt)
                .await
                .map_err(|e| format!("failed to add rebase_state.{column}: {e}"))?;
        }
        Ok(())
    }

    async fn rebase_state_has_column<C: ConnectionTrait>(
        db: &C,
        column: &str,
    ) -> Result<bool, String> {
        let stmt = Statement::from_string(
            DbBackend::Sqlite,
            "PRAGMA table_info(rebase_state)".to_string(),
        );
        let rows = db
            .query_all(stmt)
            .await
            .map_err(|e| format!("failed to inspect rebase_state columns: {e}"))?;
        for row in rows {
            let name: String = row
                .try_get_by("name")
                .map_err(|e| format!("invalid rebase_state column metadata: {e}"))?;
            if name == column {
                return Ok(true);
            }
        }
        Ok(false)
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
                SELECT head_name, onto, orig_head, current_head, todo, done, stopped_sha,
                       autostash_ref, autosquash, reapply_cherry_picks, keep_empty,
                       empty_mode, signoff, gpg_sign
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
        let autostash_ref: Option<String> = row
            .try_get_by_index(7)
            .map_err(|e| format!("invalid autostash_ref: {e}"))?;
        let autosquash: i64 = row
            .try_get_by_index(8)
            .map_err(|e| format!("invalid autosquash: {e}"))?;
        let reapply_cherry_picks: i64 = row
            .try_get_by_index(9)
            .map_err(|e| format!("invalid reapply_cherry_picks: {e}"))?;
        let keep_empty: i64 = row
            .try_get_by_index(10)
            .map_err(|e| format!("invalid keep_empty: {e}"))?;
        let empty_mode_raw: String = row
            .try_get_by_index(11)
            .map_err(|e| format!("invalid empty_mode: {e}"))?;
        let signoff: i64 = row
            .try_get_by_index(12)
            .map_err(|e| format!("invalid signoff: {e}"))?;
        let gpg_sign: i64 = row
            .try_get_by_index(13)
            .map_err(|e| format!("invalid gpg_sign: {e}"))?;

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
            autostash_ref,
            options: RebaseRuntimeOptions {
                autosquash: autosquash != 0,
                reapply_cherry_picks: reapply_cherry_picks != 0,
                keep_empty: keep_empty != 0,
                empty_mode: EmptyMode::from_db(empty_mode_raw.trim()),
                signoff: signoff != 0,
                gpg_sign: gpg_sign != 0,
            },
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
        let autostash_value = match &state.autostash_ref {
            Some(stash) => stash.clone().into(),
            None => Value::String(None),
        };

        let insert_stmt = Statement::from_sql_and_values(
            DbBackend::Sqlite,
            r#"
                INSERT INTO rebase_state
                (head_name, onto, orig_head, current_head, todo, done, stopped_sha,
                 autostash_ref, autosquash, reapply_cherry_picks, keep_empty,
                 empty_mode, signoff, gpg_sign)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?);
            "#,
            [
                state.head_name.clone().into(),
                state.onto.to_string().into(),
                state.orig_head.to_string().into(),
                state.current_head.to_string().into(),
                todo.into(),
                done.into(),
                stopped_value,
                autostash_value,
                (state.options.autosquash as i64).into(),
                (state.options.reapply_cherry_picks as i64).into(),
                (state.options.keep_empty as i64).into(),
                state.options.empty_mode.as_str().to_string().into(),
                (state.options.signoff as i64).into(),
                (state.options.gpg_sign as i64).into(),
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

    async fn migrate_legacy_state<C: ConnectionTrait>(db: &C) -> Result<Option<Self>, String> {
        let legacy_dir = Self::legacy_rebase_dir();
        if !legacy_dir.exists() {
            return Ok(None);
        }

        let state = Self::load_from_legacy_dir()?;
        Self::save_with_conn(db, &state).await?;
        if let Err(e) = fs::remove_dir_all(&legacy_dir) {
            emit_warning(format!("failed to remove legacy rebase state: {e}"));
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
            autostash_ref: None,
            options: RebaseRuntimeOptions::default(),
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
    DroppedEmpty,
    /// A user-visible merge conflict was hit while replaying the commit.
    ///
    /// - `paths` lists files left in a conflicted state and waiting for manual resolution.
    /// - `message` is `None` for a clean conflict; it is populated when an IO failure
    ///   happened while materializing the conflict state on disk (e.g. failed to save the
    ///   index with stage 1/2/3 entries, or failed to write a working-tree file).
    Conflict {
        paths: Vec<PathBuf>,
        message: Option<String>,
    },
    /// A non-conflict internal failure occurred (e.g. object load, tree creation,
    /// commit save, index/workdir IO). `kind` classifies the cause so the caller can
    /// surface a precise stable error code; `detail` carries the human-readable cause.
    Internal {
        kind: ReplayErrorKind,
        detail: String,
    },
}

/// Categorizes the cause of a non-conflict failure inside
/// [`replay_commit_with_conflict_detection`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayErrorKind {
    IndexLoad,
    CommitLoad,
    MissingParent,
    BaseTreeLoad,
    TheirTreeLoad,
    OurTreeLoad,
    UntrackedOverwrite,
    ConflictMarker,
    TreeCreate,
    CommitSave,
    NewTreeLoad,
    IndexRebuild,
    IndexSave,
    WorkdirReset,
}

impl ReplayErrorKind {
    /// Snake-case identifier surfaced in JSON error details and human messages.
    pub fn as_str(self) -> &'static str {
        match self {
            ReplayErrorKind::IndexLoad => "index_load",
            ReplayErrorKind::CommitLoad => "commit_load",
            ReplayErrorKind::MissingParent => "missing_parent",
            ReplayErrorKind::BaseTreeLoad => "base_tree_load",
            ReplayErrorKind::TheirTreeLoad => "their_tree_load",
            ReplayErrorKind::OurTreeLoad => "our_tree_load",
            ReplayErrorKind::UntrackedOverwrite => "untracked_overwrite",
            ReplayErrorKind::ConflictMarker => "conflict_marker",
            ReplayErrorKind::TreeCreate => "tree_create",
            ReplayErrorKind::CommitSave => "commit_save",
            ReplayErrorKind::NewTreeLoad => "new_tree_load",
            ReplayErrorKind::IndexRebuild => "index_rebuild",
            ReplayErrorKind::IndexSave => "index_save",
            ReplayErrorKind::WorkdirReset => "workdir_reset",
        }
    }

    /// Maps this internal failure cause to its stable error code so distinct
    /// kinds no longer collapse to `ConflictUnresolved`.
    pub fn stable_code(self) -> StableErrorCode {
        match self {
            ReplayErrorKind::IndexLoad => StableErrorCode::IoReadFailed,
            ReplayErrorKind::CommitLoad
            | ReplayErrorKind::MissingParent
            | ReplayErrorKind::BaseTreeLoad
            | ReplayErrorKind::TheirTreeLoad
            | ReplayErrorKind::OurTreeLoad
            | ReplayErrorKind::NewTreeLoad => StableErrorCode::RepoCorrupt,
            ReplayErrorKind::UntrackedOverwrite => StableErrorCode::ConflictOperationBlocked,
            ReplayErrorKind::ConflictMarker
            | ReplayErrorKind::TreeCreate
            | ReplayErrorKind::CommitSave
            | ReplayErrorKind::IndexRebuild
            | ReplayErrorKind::IndexSave
            | ReplayErrorKind::WorkdirReset => StableErrorCode::IoWriteFailed,
        }
    }
}

impl std::fmt::Display for ReplayErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl ReplayResult {
    fn conflict(paths: Vec<PathBuf>) -> Self {
        ReplayResult::Conflict {
            paths,
            message: None,
        }
    }

    fn internal(kind: ReplayErrorKind, detail: impl Into<String>) -> Self {
        ReplayResult::Internal {
            kind,
            detail: detail.into(),
        }
    }
}

/// `--help` examples shown in `libra rebase --help` output.
///
/// Rebase exposes a small four-mode state machine: start (positional
/// upstream), `--continue`, `--abort`, `--skip`. The banner pins one
/// example per mode plus a JSON variant so users see all transitions
/// without reading `docs/improvement/rebase.md`. Cross-cutting `--help`
/// EXAMPLES rollout per `docs/improvement/README.md` item B.
pub const REBASE_EXAMPLES: &str = "\
EXAMPLES:
    libra rebase main             Replay current branch on top of main
    libra rebase --onto next main topic
                                  Replay topic's main..topic commits onto next
    libra rebase --root --onto main
                                  Replay the entire branch history onto main
    libra rebase --autostash main Stash dirty work before replay and restore it afterward
    libra rebase --autosquash main
                                  Fold fixup!/squash! commits without opening an editor
    libra rebase --empty=keep main
                                  Keep commits that become empty after replay
    libra rebase --signoff main   Add Signed-off-by trailers to replayed commits
    libra rebase -S main          Vault-sign replayed commits
    libra rebase --continue       Resume an in-progress rebase after fixing conflicts
    libra rebase --skip           Drop the current conflicting commit and continue
    libra rebase --abort          Restore the original branch and clear rebase state
    libra rebase --json main      Structured JSON output for agents";

/// Command-line arguments for the rebase operation
#[derive(Parser, Debug, Default)]
#[command(after_help = REBASE_EXAMPLES)]
pub struct RebaseArgs {
    /// The upstream branch to rebase the current branch onto.
    /// This can be a branch name, commit hash, or other Git reference.
    #[clap(required_unless_present_any = ["continue_rebase", "abort", "skip", "root"])]
    pub upstream: Option<String>,

    /// Optional branch to switch to before rebasing.
    #[clap(conflicts_with_all = ["continue_rebase", "abort", "skip"])]
    pub branch: Option<String>,

    /// Replay commits onto this new base while using <upstream> only to define the range.
    #[clap(long, value_name = "newbase", conflicts_with_all = ["continue_rebase", "abort", "skip"])]
    pub onto: Option<String>,

    /// Rebase all commits reachable from the root commit
    #[clap(long, conflicts_with_all = ["continue_rebase", "abort", "skip"])]
    pub root: bool,

    /// Stash dirty work before rebasing and restore it after completion or abort
    #[clap(long, conflicts_with = "no_autostash")]
    pub autostash: bool,

    /// Disable rebase.autoStash for this invocation
    #[clap(long = "no-autostash", conflicts_with = "autostash")]
    pub no_autostash: bool,

    /// Move and fold fixup!/squash! commits automatically
    #[clap(long, conflicts_with = "no_autosquash")]
    pub autosquash: bool,

    /// Disable rebase.autoSquash for this invocation
    #[clap(long = "no-autosquash", conflicts_with = "autosquash")]
    pub no_autosquash: bool,

    /// Reapply commits even when an equivalent patch already exists upstream
    #[clap(long, conflicts_with = "no_reapply_cherry_picks")]
    pub reapply_cherry_picks: bool,

    /// Skip commits whose patch already exists upstream
    #[clap(
        long = "no-reapply-cherry-picks",
        conflicts_with = "reapply_cherry_picks"
    )]
    pub no_reapply_cherry_picks: bool,

    /// Preserve commits that were already empty before replay
    #[clap(long, conflicts_with = "no_keep_empty")]
    pub keep_empty: bool,

    /// Drop commits that were already empty before replay
    #[clap(long = "no-keep-empty", conflicts_with = "keep_empty")]
    pub no_keep_empty: bool,

    /// Control commits that become empty after replay
    #[clap(long, value_enum, value_name = "drop|keep|stop")]
    pub empty: Option<EmptyMode>,

    /// Add a Signed-off-by trailer to replayed commits
    #[clap(short = 's', long = "signoff", conflicts_with = "no_signoff")]
    pub signoff: bool,

    /// Do not add a Signed-off-by trailer
    #[clap(long = "no-signoff", conflicts_with = "signoff")]
    pub no_signoff: bool,

    /// GPG-sign replayed commits using the vault signing key
    #[clap(short = 'S', long = "gpg-sign", conflicts_with = "no_gpg_sign")]
    pub gpg_sign: bool,

    /// Do not GPG-sign replayed commits
    #[clap(long = "no-gpg-sign", conflicts_with = "gpg_sign")]
    pub no_gpg_sign: bool,

    /// Continue an in-progress rebase after resolving conflicts
    #[clap(
        long = "continue",
        conflicts_with_all = [
            "abort",
            "skip",
            "upstream",
            "branch",
            "onto",
            "root",
            "autostash",
            "no_autostash",
            "autosquash",
            "no_autosquash",
            "reapply_cherry_picks",
            "no_reapply_cherry_picks",
            "keep_empty",
            "no_keep_empty",
            "empty",
            "signoff",
            "no_signoff",
            "gpg_sign",
            "no_gpg_sign"
        ]
    )]
    pub continue_rebase: bool,

    /// Abort the current rebase and restore the original branch
    #[clap(
        long,
        conflicts_with_all = [
            "continue_rebase",
            "skip",
            "upstream",
            "branch",
            "onto",
            "root",
            "autostash",
            "no_autostash",
            "autosquash",
            "no_autosquash",
            "reapply_cherry_picks",
            "no_reapply_cherry_picks",
            "keep_empty",
            "no_keep_empty",
            "empty",
            "signoff",
            "no_signoff",
            "gpg_sign",
            "no_gpg_sign"
        ]
    )]
    pub abort: bool,

    /// Skip the current commit and continue with the next
    #[clap(
        long,
        conflicts_with_all = [
            "continue_rebase",
            "abort",
            "upstream",
            "branch",
            "onto",
            "root",
            "autostash",
            "no_autostash",
            "autosquash",
            "no_autosquash",
            "reapply_cherry_picks",
            "no_reapply_cherry_picks",
            "keep_empty",
            "no_keep_empty",
            "empty",
            "signoff",
            "no_signoff",
            "gpg_sign",
            "no_gpg_sign"
        ]
    )]
    pub skip: bool,
}

#[derive(Debug, Clone, Serialize)]
struct RebaseOutput {
    action: String,
    status: String,
    branch: String,
    commit: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    upstream: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    onto: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    common_ancestor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    replay_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    restored: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    applied_commits: Vec<RebaseAppliedCommitOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    skipped_commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    skipped_subject: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    remaining: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    autostashed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    autosquashed: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dropped_empty: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
struct RebaseAppliedCommitOutput {
    original_commit: String,
    commit: String,
    subject: String,
}

#[derive(Debug, Default)]
struct RebaseReplaySummary {
    applied_commits: Vec<RebaseAppliedCommitOutput>,
    autosquashed: usize,
    dropped_empty: usize,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RebaseError {
    #[error("no rebase in progress")]
    NoRebaseInProgress,
    #[error("failed to check rebase state: {0}")]
    StateCheck(String),
    #[error("failed to load rebase state: {0}")]
    StateLoad(String),
    #[error("not on a branch or in detached HEAD state, cannot rebase")]
    NotOnBranch,
    #[error("current branch '{branch}' has no commits")]
    BranchHasNoCommits { branch: String },
    #[error("failed to resolve upstream '{upstream}': {detail}")]
    UpstreamResolve { upstream: String, detail: String },
    #[error("no common ancestor found")]
    NoCommonAncestor,
    #[error("multiple best merge bases found ({bases}); criss-cross merge bases are unsupported")]
    AmbiguousMergeBase { bases: String },
    #[error("failed to determine working tree status: {0}")]
    WorktreeStatus(String),
    #[error("{detail}, can't {action}")]
    WorktreeDirty { action: String, detail: String },
    #[error("autostash failed: {0}")]
    Autostash(String),
    #[error("{0}")]
    InvalidArguments(String),
    #[error("failed to sign rebased commit: {0}")]
    Sign(String),
    #[error("untracked working tree file would be overwritten by rebase: {path}")]
    UntrackedOverwrite { path: String },
    #[error("you must resolve all conflicts before continuing")]
    UnresolvedConflicts,
    #[error("no commit to skip")]
    NoCommitToSkip,
    #[error("rebase stopped while applying {commit}: {subject}")]
    ReplayConflict {
        commit: String,
        subject: String,
        paths: Vec<PathBuf>,
        message: Option<String>,
    },
    #[error("rebase stopped while applying {commit}: {kind} failed ({detail})")]
    ReplayInternal {
        commit: String,
        subject: String,
        kind: ReplayErrorKind,
        detail: String,
    },
    #[error("failed to restore branch '{branch}' during rebase abort: {detail}")]
    BranchRestore { branch: String, detail: String },
    #[error("failed to load commit '{commit}': {detail}")]
    CommitLoad { commit: String, detail: String },
    #[error("failed to load original commit '{commit}': {detail}")]
    OriginalCommitLoad { commit: String, detail: String },
    #[error("failed to load original tree '{tree}': {detail}")]
    OriginalTreeLoad { tree: String, detail: String },
    #[error("failed to load current index: {0}")]
    IndexLoad(String),
    #[error("failed to create tree from index: {0}")]
    TreeCreate(String),
    #[error("failed to save rebased commit: {0}")]
    CommitSave(String),
    #[error("failed to rebuild index: {0}")]
    IndexRebuild(String),
    #[error("failed to save index: {0}")]
    IndexSave(String),
    #[error("failed to reset working directory: {0}")]
    WorkdirReset(String),
    #[error("failed to save rebase state: {0}")]
    StateSave(String),
    #[error("failed to finalize rebase: {0}")]
    Finalize(String),
}

impl From<RebaseError> for CliError {
    fn from(error: RebaseError) -> Self {
        match &error {
            RebaseError::NoRebaseInProgress => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::RepoStateInvalid),
            RebaseError::StateCheck(..) | RebaseError::StateLoad(..) => {
                CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::IoReadFailed)
            }
            RebaseError::NotOnBranch | RebaseError::BranchHasNoCommits { .. } => {
                CliError::fatal(error.to_string())
                    .with_stable_code(StableErrorCode::RepoStateInvalid)
            }
            RebaseError::UpstreamResolve { .. } | RebaseError::NoCommonAncestor => {
                CliError::fatal(error.to_string())
                    .with_stable_code(StableErrorCode::CliInvalidTarget)
            }
            RebaseError::AmbiguousMergeBase { .. } => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::ConflictOperationBlocked)
                .with_hint("choose a history with a single best merge base before rebasing"),
            RebaseError::WorktreeStatus(..) => {
                CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::IoReadFailed)
            }
            RebaseError::WorktreeDirty { .. } => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::RepoStateInvalid)
                .with_hint("commit or stash your changes before rebasing."),
            RebaseError::Autostash(..) => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::RepoStateInvalid)
                .with_hint("inspect 'libra stash list' and recover the saved changes manually."),
            RebaseError::InvalidArguments(..) => CliError::command_usage(error.to_string())
                .with_stable_code(StableErrorCode::CliInvalidArguments),
            RebaseError::Sign(..) => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::AuthMissingCredentials)
                .with_hint("configure the Libra vault signing key or retry without --gpg-sign."),
            RebaseError::UntrackedOverwrite { .. } => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::ConflictOperationBlocked)
                .with_hint("move or remove it before you rebase."),
            RebaseError::UnresolvedConflicts => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::ConflictUnresolved)
                .with_hint("use 'libra add <file>' to mark conflicts as resolved.")
                .with_hint("then run 'libra rebase --continue' again."),
            RebaseError::NoCommitToSkip => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::RepoStateInvalid),
            RebaseError::ReplayConflict {
                commit,
                paths,
                message,
                ..
            } => {
                let mut resolution_hint =
                    "resolve conflicts, stage them, then run 'libra rebase --continue'."
                        .to_string();
                if !paths.is_empty() {
                    let path_list = paths
                        .iter()
                        .map(|path| format!("  {}", path.display()))
                        .collect::<Vec<_>>()
                        .join("\n");
                    resolution_hint = format!(
                        "conflicted files:\n{path_list}\nresolve conflicts, stage them, then run 'libra rebase --continue'."
                    );
                }
                let mut error = CliError::fatal(error.to_string())
                    .with_stable_code(StableErrorCode::ConflictUnresolved)
                    .with_hint(resolution_hint)
                    .with_hint("or run 'libra rebase --skip' / 'libra rebase --abort'.")
                    .with_detail("commit", commit.clone());
                if !paths.is_empty() {
                    let paths = paths
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>();
                    error = error.with_detail("paths", serde_json::json!(paths));
                }
                if let Some(message) = message {
                    error = error.with_detail("message", message.clone());
                }
                error
            }
            RebaseError::ReplayInternal {
                commit,
                subject,
                kind,
                detail,
            } => CliError::fatal(error.to_string())
                .with_stable_code(kind.stable_code())
                .with_hint("run 'libra rebase --abort' to return to the original branch.")
                .with_detail("commit", commit.clone())
                .with_detail("subject", subject.clone())
                .with_detail("kind", kind.as_str())
                .with_detail("detail", detail.clone()),
            RebaseError::CommitLoad { .. } => {
                CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::RepoCorrupt)
            }
            RebaseError::OriginalCommitLoad { .. } | RebaseError::OriginalTreeLoad { .. } => {
                CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::RepoCorrupt)
            }
            RebaseError::BranchRestore { .. }
            | RebaseError::TreeCreate(..)
            | RebaseError::CommitSave(..)
            | RebaseError::IndexRebuild(..)
            | RebaseError::IndexSave(..)
            | RebaseError::WorkdirReset(..)
            | RebaseError::StateSave(..)
            | RebaseError::Finalize(..) => {
                CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::IoWriteFailed)
            }
            RebaseError::IndexLoad(..) => {
                CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::IoReadFailed)
            }
        }
    }
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
    if let Err(error) = execute_safe(args, &OutputConfig::default()).await {
        error.print_stderr();
    }
}

/// Safe CLI entry point with preflight validation for argument and state errors.
pub async fn execute_safe(args: RebaseArgs, output: &OutputConfig) -> CliResult<()> {
    util::require_repo().map_err(|_| CliError::repo_not_found())?;

    // Refuse to start a NEW rebase while a cherry-pick sequence is in progress
    // (rebase's own --continue/--abort/--skip operate on rebase state, not
    // cherry-pick, so they are exempt from this guard).
    if !(args.continue_rebase || args.abort || args.skip) {
        crate::command::cherry_pick::ensure_no_cherry_pick_in_progress().await?;
    }

    // For --continue, --abort, --skip: verify that a rebase is actually in
    // progress before delegating to typed runners.  This ensures
    // a non-zero exit code (128) is returned when there is nothing to do,
    // matching the behaviour of `git rebase --abort` / `--continue` / `--skip`.
    if args.continue_rebase || args.abort || args.skip {
        match RebaseState::is_in_progress().await {
            Ok(true) => { /* rebase in progress – proceed */ }
            Ok(false) => {
                let verb = if args.abort {
                    "abort"
                } else if args.skip {
                    "skip"
                } else {
                    "continue"
                };
                return Err(CliError::fatal("no rebase in progress")
                    .with_stable_code(StableErrorCode::RepoStateInvalid)
                    .with_hint(format!(
                        "cannot --{verb} because there is no rebase in progress."
                    )));
            }
            Err(err) => {
                return Err(
                    CliError::fatal(format!("failed to check rebase state: {err}"))
                        .with_stable_code(StableErrorCode::IoReadFailed),
                );
            }
        }
    }

    preflight_rebase(&args).await?;
    if args.abort {
        let result = run_rebase_abort().await.map_err(CliError::from)?;
        return render_rebase_output(&result, output);
    }
    if args.continue_rebase {
        let result = run_rebase_continue().await.map_err(CliError::from)?;
        return render_rebase_output(&result, output);
    }
    if args.skip {
        let result = run_rebase_skip().await.map_err(CliError::from)?;
        return render_rebase_output(&result, output);
    }
    if let Some(start) = resolve_rebase_start_request(&args)? {
        if let Some(branch) = start.branch.as_deref() {
            switch_to_rebase_branch(branch, output).await?;
        }
        let options = resolve_rebase_runtime_options(&args).await?;
        let autostash = resolve_rebase_autostash(&args).await?;
        let result = if start.root {
            run_rebase_root_start(start.onto.as_deref(), options, autostash)
                .await
                .map_err(CliError::from)?
        } else {
            let upstream = start.upstream.as_deref().ok_or_else(|| {
                CliError::command_usage("no upstream specified")
                    .with_stable_code(StableErrorCode::CliInvalidArguments)
            })?;
            run_rebase_start(upstream, start.onto.as_deref(), options, autostash)
                .await
                .map_err(CliError::from)?
        };
        return render_rebase_output(&result, output);
    }
    Ok(())
}

#[derive(Debug)]
struct RebaseStartRequest {
    upstream: Option<String>,
    branch: Option<String>,
    onto: Option<String>,
    root: bool,
}

fn resolve_rebase_start_request(args: &RebaseArgs) -> CliResult<Option<RebaseStartRequest>> {
    if args.continue_rebase || args.abort || args.skip {
        return Ok(None);
    }
    if args.root {
        if args.branch.is_some() {
            return Err(CliError::command_usage(
                "rebase --root accepts at most one optional branch positional",
            )
            .with_stable_code(StableErrorCode::CliInvalidArguments));
        }
        return Ok(Some(RebaseStartRequest {
            upstream: None,
            branch: args.upstream.clone(),
            onto: args.onto.clone(),
            root: true,
        }));
    }
    Ok(Some(RebaseStartRequest {
        upstream: args.upstream.clone(),
        branch: args.branch.clone(),
        onto: args.onto.clone(),
        root: false,
    }))
}

async fn resolve_rebase_runtime_options(args: &RebaseArgs) -> CliResult<RebaseRuntimeOptions> {
    let autosquash = resolve_bool_flag_config(
        args.autosquash,
        args.no_autosquash,
        &["rebase.autoSquash", "rebase.autosquash"],
        false,
    )
    .await;
    let keep_empty = resolve_bool_flag_config(
        args.keep_empty,
        args.no_keep_empty,
        &["rebase.keepEmpty", "rebase.keepempty"],
        true,
    )
    .await;
    let empty_mode = match args.empty {
        Some(mode) => mode,
        None => read_first_rebase_config(&["rebase.empty"])
            .await
            .as_deref()
            .map(EmptyMode::from_db)
            .unwrap_or(EmptyMode::Drop),
    };
    Ok(RebaseRuntimeOptions {
        autosquash,
        reapply_cherry_picks: args.reapply_cherry_picks && !args.no_reapply_cherry_picks,
        keep_empty,
        empty_mode,
        signoff: args.signoff && !args.no_signoff,
        gpg_sign: args.gpg_sign && !args.no_gpg_sign,
    })
}

async fn resolve_rebase_autostash(args: &RebaseArgs) -> CliResult<bool> {
    Ok(resolve_bool_flag_config(
        args.autostash,
        args.no_autostash,
        &["rebase.autoStash", "rebase.autostash"],
        false,
    )
    .await)
}

async fn resolve_bool_flag_config(
    positive: bool,
    negative: bool,
    keys: &[&str],
    default: bool,
) -> bool {
    if negative {
        return false;
    }
    if positive {
        return true;
    }
    read_first_rebase_config(keys)
        .await
        .as_deref()
        .map(parse_config_bool)
        .unwrap_or(default)
}

async fn read_first_rebase_config(keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Ok(Some(value)) =
            read_cascaded_config_value(LocalIdentityTarget::CurrentRepo, key).await
        {
            return Some(value);
        }
    }
    None
}

fn parse_config_bool(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "true" | "yes" | "on" | "1"
    )
}

async fn switch_to_rebase_branch(branch: &str, output: &OutputConfig) -> CliResult<()> {
    let mut child_output = output.child_output_config();
    child_output.quiet = true;
    switch::execute_safe(
        switch::SwitchArgs {
            branch: Some(branch.to_string()),
            ..Default::default()
        },
        &child_output,
    )
    .await
}

fn render_rebase_output(result: &RebaseOutput, output: &OutputConfig) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("rebase", result, output);
    }
    if output.quiet {
        return Ok(());
    }

    if result.action == "start" {
        render_rebase_start_output(result);
        return Ok(());
    }

    if result.action == "abort" {
        println!("Rebase aborted. Restored branch '{}'.", result.branch);
        return Ok(());
    }

    if result.action == "skip" {
        let skipped_commit = result
            .skipped_commit
            .as_deref()
            .map(short_id)
            .unwrap_or_else(|| "unknown".to_string());
        if let Some(subject) = result.skipped_subject.as_deref() {
            println!("Skipped: {skipped_commit} {subject}");
        } else {
            println!("Skipped: {skipped_commit} (message unavailable)");
        }
    }

    for applied in &result.applied_commits {
        println!("Applied: {} {}", short_id(&applied.commit), applied.subject);
    }

    if matches!(result.action.as_str(), "continue" | "skip") && result.status == "completed" {
        let onto = result.onto.as_deref().unwrap_or(&result.commit);
        println!(
            "Successfully rebased branch '{}' onto '{}'.",
            result.branch,
            short_id(onto)
        );
    }
    Ok(())
}

fn render_rebase_start_output(result: &RebaseOutput) {
    let upstream = result
        .upstream
        .as_deref()
        .or(result.onto.as_deref())
        .unwrap_or(&result.commit);

    match result.status.as_str() {
        "fast-forwarded" => {
            println!(
                "Fast-forwarded branch '{}' to '{}'.",
                result.branch, upstream
            );
        }
        "already-up-to-date" => {
            println!("Current branch is ahead of upstream. No rebase needed.");
        }
        "no-commits" => {
            println!("No commits to rebase on branch '{}'.", result.branch);
        }
        _ => {
            if let Some(common_ancestor) = result.common_ancestor.as_deref() {
                println!("Found common ancestor: {}", short_id(common_ancestor));
            }
            if let Some(replay_count) = result.replay_count {
                println!(
                    "Rebasing {replay_count} commits from `{}` onto `{upstream}`...",
                    result.branch
                );
            }
            for applied in &result.applied_commits {
                println!("Applied: {} {}", short_id(&applied.commit), applied.subject);
            }
            println!(
                "Successfully rebased branch '{}' onto '{}'.",
                result.branch,
                short_id(&result.commit)
            );
        }
    }
}

async fn ensure_rebase_in_progress() -> Result<(), RebaseError> {
    match RebaseState::is_in_progress().await {
        Ok(true) => Ok(()),
        Ok(false) => Err(RebaseError::NoRebaseInProgress),
        Err(e) => Err(RebaseError::StateCheck(e)),
    }
}

fn short_id(value: &str) -> String {
    value.chars().take(7).collect()
}

fn short_object_id(value: &ObjectHash) -> String {
    short_id(&value.to_string())
}

fn commit_subject_from_message(message: &str) -> String {
    parse_commit_msg(message)
        .0
        .lines()
        .next()
        .unwrap_or("")
        .to_string()
}

fn commit_subject_lossy(commit_id: &ObjectHash, emit_human: bool) -> String {
    match load_object::<Commit>(commit_id) {
        Ok(commit) => commit_subject_from_message(&commit.message),
        Err(e) => {
            if emit_human {
                cli_error!(
                    e,
                    "warning: failed to load commit {}",
                    short_object_id(commit_id)
                );
            }
            "unknown".to_string()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RebaseTodoAction {
    Pick,
    Fixup,
    Squash,
}

impl RebaseTodoAction {
    fn from_message(message: &str) -> Self {
        let subject = commit_subject_from_message(message);
        if subject.starts_with("fixup! ") || subject.starts_with("amend! ") {
            RebaseTodoAction::Fixup
        } else if subject.starts_with("squash! ") {
            RebaseTodoAction::Squash
        } else {
            RebaseTodoAction::Pick
        }
    }
}

#[derive(Debug)]
struct AutosquashResult {
    commits: Vec<ObjectHash>,
    moved: usize,
}

fn autosquash_commits(commits: Vec<ObjectHash>) -> Result<AutosquashResult, RebaseError> {
    let mut picks: Vec<ObjectHash> = Vec::new();
    let mut fixups: Vec<ObjectHash> = Vec::new();
    for commit_id in commits {
        let commit: Commit = load_object(&commit_id).map_err(|error| RebaseError::CommitLoad {
            commit: commit_id.to_string(),
            detail: error.to_string(),
        })?;
        match RebaseTodoAction::from_message(&commit.message) {
            RebaseTodoAction::Pick => picks.push(commit_id),
            RebaseTodoAction::Fixup | RebaseTodoAction::Squash => fixups.push(commit_id),
        }
    }

    let mut moved = 0;
    for fixup_id in fixups {
        let fixup_commit: Commit =
            load_object(&fixup_id).map_err(|error| RebaseError::CommitLoad {
                commit: fixup_id.to_string(),
                detail: error.to_string(),
            })?;
        let target = autosquash_target(&fixup_commit.message).ok_or_else(|| {
            RebaseError::InvalidArguments(format!(
                "could not parse autosquash target for {}",
                short_object_id(&fixup_id)
            ))
        })?;
        let target_pos = picks
            .iter()
            .position(|candidate| autosquash_target_matches(candidate, &target))
            .ok_or_else(|| {
                RebaseError::InvalidArguments(format!(
                    "autosquash target '{target}' was not found in the rebase todo"
                ))
            })?;
        let mut insert_at = target_pos + 1;
        while insert_at < picks.len() {
            let commit: Commit =
                load_object(&picks[insert_at]).map_err(|error| RebaseError::CommitLoad {
                    commit: picks[insert_at].to_string(),
                    detail: error.to_string(),
                })?;
            if matches!(
                RebaseTodoAction::from_message(&commit.message),
                RebaseTodoAction::Fixup | RebaseTodoAction::Squash
            ) {
                insert_at += 1;
            } else {
                break;
            }
        }
        picks.insert(insert_at, fixup_id);
        moved += 1;
    }

    Ok(AutosquashResult {
        commits: picks,
        moved,
    })
}

fn autosquash_target(message: &str) -> Option<String> {
    let subject = commit_subject_from_message(message);
    for prefix in ["fixup! ", "squash! ", "amend! "] {
        if let Some(target) = subject.strip_prefix(prefix) {
            let trimmed = target.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn autosquash_target_matches(commit_id: &ObjectHash, target: &str) -> bool {
    let full = commit_id.to_string();
    if full.starts_with(target) {
        return true;
    }
    load_object::<Commit>(commit_id)
        .map(|commit| commit_subject_from_message(&commit.message) == target)
        .unwrap_or(false)
}

fn filter_redundant_cherry_picks(
    commits: &[ObjectHash],
    onto_id: &ObjectHash,
) -> Result<Vec<ObjectHash>, RebaseError> {
    let upstream_patch_ids = reachable_patch_ids(onto_id)?;
    let mut filtered = Vec::new();
    for commit_id in commits {
        let patch_id = patch_id_for_commit(commit_id)?;
        if upstream_patch_ids.contains(&patch_id) {
            continue;
        }
        filtered.push(*commit_id);
    }
    Ok(filtered)
}

fn reachable_patch_ids(head_id: &ObjectHash) -> Result<HashSet<String>, RebaseError> {
    let mut ids = HashSet::new();
    let mut stack = vec![*head_id];
    let mut seen = HashSet::new();
    while let Some(commit_id) = stack.pop() {
        if !seen.insert(commit_id) {
            continue;
        }
        if let Ok(patch_id) = patch_id_for_commit(&commit_id) {
            ids.insert(patch_id);
        }
        let commit: Commit = load_object(&commit_id).map_err(|error| RebaseError::CommitLoad {
            commit: commit_id.to_string(),
            detail: error.to_string(),
        })?;
        stack.extend(commit.parent_commit_ids);
    }
    Ok(ids)
}

fn patch_id_for_commit(commit_id: &ObjectHash) -> Result<String, RebaseError> {
    let commit: Commit = load_object(commit_id).map_err(|error| RebaseError::CommitLoad {
        commit: commit_id.to_string(),
        detail: error.to_string(),
    })?;
    let parent_tree = match commit.parent_commit_ids.first() {
        Some(parent_id) => {
            let parent: Commit =
                load_object(parent_id).map_err(|error| RebaseError::CommitLoad {
                    commit: parent_id.to_string(),
                    detail: error.to_string(),
                })?;
            load_object::<Tree>(&parent.tree_id).map_err(|error| RebaseError::OriginalTreeLoad {
                tree: parent.tree_id.to_string(),
                detail: error.to_string(),
            })?
        }
        None => empty_tree().map_err(|detail| RebaseError::OriginalTreeLoad {
            tree: "<empty>".to_string(),
            detail,
        })?,
    };
    let commit_tree: Tree =
        load_object(&commit.tree_id).map_err(|error| RebaseError::OriginalTreeLoad {
            tree: commit.tree_id.to_string(),
            detail: error.to_string(),
        })?;
    let parent_items = tree_item_fingerprint_map(&parent_tree);
    let commit_items = tree_item_fingerprint_map(&commit_tree);
    let mut paths: Vec<_> = parent_items
        .keys()
        .chain(commit_items.keys())
        .cloned()
        .collect();
    paths.sort();
    paths.dedup();
    let mut payload = String::new();
    for path in paths {
        if parent_items.get(&path) == commit_items.get(&path) {
            continue;
        }
        payload.push_str(&path);
        payload.push('\0');
        payload.push_str(parent_items.get(&path).map(String::as_str).unwrap_or("-"));
        payload.push('\0');
        payload.push_str(commit_items.get(&path).map(String::as_str).unwrap_or("-"));
        payload.push('\n');
    }
    Ok(ObjectHash::from_type_and_data(ObjectType::Blob, payload.as_bytes()).to_string())
}

fn tree_item_fingerprint_map(tree: &Tree) -> HashMap<String, String> {
    tree.get_plain_items_with_mode()
        .into_iter()
        .filter(|(_, _, mode)| *mode != TreeItemMode::Commit)
        .map(|(path, hash, mode)| (path.display().to_string(), format!("{mode:?}:{hash}")))
        .collect()
}

async fn preflight_rebase(args: &RebaseArgs) -> CliResult<()> {
    if args.continue_rebase || args.abort || args.skip {
        return Ok(());
    }

    let start = resolve_rebase_start_request(args)?
        .ok_or_else(|| CliError::command_usage("no rebase start request"))?;

    match RebaseState::is_in_progress().await {
        Ok(true) => {
            return Err(CliError::fatal("rebase already in progress")
                .with_stable_code(StableErrorCode::RepoStateInvalid)
                .with_hint("use 'libra rebase --continue' to continue rebasing.")
                .with_hint(
                    "use 'libra rebase --abort' to abort and restore the original branch.",
                ));
        }
        Ok(false) => {}
        Err(err) => {
            return Err(
                CliError::fatal(format!("failed to check rebase state: {err}"))
                    .with_stable_code(StableErrorCode::IoReadFailed),
            );
        }
    }

    // `resolve_branch_or_commit` returns legacy `"fatal: ..."` prefixed strings,
    // so `from_legacy_string` strips the prefix to avoid double-prefix rendering.
    if let Some(upstream) = start.upstream.as_deref() {
        resolve_branch_or_commit(upstream)
            .await
            .map_err(CliError::from_legacy_string)?;
    }
    if let Some(onto) = start.onto.as_deref() {
        resolve_branch_or_commit(onto)
            .await
            .map_err(CliError::from_legacy_string)?;
    }
    Ok(())
}

async fn run_rebase_start(
    upstream: &str,
    onto: Option<&str>,
    options: RebaseRuntimeOptions,
    autostash: bool,
) -> Result<RebaseOutput, RebaseError> {
    let db = get_db_conn_instance().await;

    let current_branch_name = match Head::current().await {
        Head::Branch(name) if !name.is_empty() => name,
        _ => return Err(RebaseError::NotOnBranch),
    };

    let head_to_rebase_id =
        Head::current_commit()
            .await
            .ok_or_else(|| RebaseError::BranchHasNoCommits {
                branch: current_branch_name.clone(),
            })?;

    let upstream_id = resolve_branch_or_commit(upstream).await.map_err(|detail| {
        RebaseError::UpstreamResolve {
            upstream: upstream.to_string(),
            detail,
        }
    })?;
    let onto_id = match onto {
        Some(newbase) => resolve_branch_or_commit(newbase).await.map_err(|detail| {
            RebaseError::UpstreamResolve {
                upstream: newbase.to_string(),
                detail,
            }
        })?,
        None => upstream_id,
    };
    let onto_display = onto.unwrap_or(upstream);

    let base_id = find_merge_base(&head_to_rebase_id, &upstream_id)
        .await?
        .ok_or(RebaseError::NoCommonAncestor)?;

    if onto.is_none() && base_id == head_to_rebase_id {
        let upstream_commit: Commit =
            load_object(&upstream_id).map_err(|e| RebaseError::CommitLoad {
                commit: upstream_id.to_string(),
                detail: e.to_string(),
            })?;
        let upstream_tree: Tree =
            load_object(&upstream_commit.tree_id).map_err(|e| RebaseError::OriginalTreeLoad {
                tree: upstream_commit.tree_id.to_string(),
                detail: e.to_string(),
            })?;

        let index_file = path::index();
        let current_index = git_internal::internal::index::Index::load(&index_file)
            .map_err(|e| RebaseError::IndexLoad(e.to_string()))?;
        let mut index = git_internal::internal::index::Index::new();
        rebuild_index_from_tree(&upstream_tree, &mut index, "")
            .map_err(RebaseError::IndexRebuild)?;
        let autostash_ref = prepare_rebase_autostash(autostash).await?;
        if let Err(error) = rebase_worktree_guard_structured(&index, "fast-forward rebase").await {
            restore_rebase_autostash(autostash_ref.as_deref()).await;
            return Err(error);
        }

        let fast_forward_action = ReflogAction::Rebase {
            state: "fast-forward".to_string(),
            details: format!("moving {} to {}", current_branch_name, upstream),
        };
        let fast_forward_context = ReflogContext {
            old_oid: head_to_rebase_id.to_string(),
            new_oid: upstream_id.to_string(),
            action: fast_forward_action,
            message: None,
        };

        let branch_name_cloned = current_branch_name.clone();
        let upstream_id_str = upstream_id.to_string();
        with_reflog(
            fast_forward_context,
            move |txn: &sea_orm::DatabaseTransaction| {
                Box::pin(async move {
                    Branch::update_branch_with_conn(
                        txn,
                        &branch_name_cloned,
                        &upstream_id_str,
                        None,
                    )
                    .await?;
                    Head::update_with_conn(txn, Head::Branch(branch_name_cloned), None).await;
                    Ok(())
                })
            },
            true,
        )
        .await
        .map_err(|e| RebaseError::Finalize(format!("failed to fast-forward: {e}")))?;

        index
            .save(&index_file)
            .map_err(|e| RebaseError::IndexSave(e.to_string()))?;
        reset_workdir_tracked_only(&current_index, &index).map_err(RebaseError::WorkdirReset)?;
        restore_rebase_autostash(autostash_ref.as_deref()).await;

        return Ok(RebaseOutput {
            action: "start".to_string(),
            status: "fast-forwarded".to_string(),
            branch: current_branch_name,
            commit: upstream_id.to_string(),
            upstream: Some(upstream.to_string()),
            onto: Some(upstream_id.to_string()),
            common_ancestor: Some(base_id.to_string()),
            replay_count: Some(0),
            previous_commit: Some(head_to_rebase_id.to_string()),
            restored: None,
            applied_commits: Vec::new(),
            skipped_commit: None,
            skipped_subject: None,
            remaining: Some(0),
            autostashed: Some(autostash_ref.is_some()),
            autosquashed: None,
            dropped_empty: None,
        });
    }

    if onto.is_none() && base_id == upstream_id {
        return Ok(RebaseOutput {
            action: "start".to_string(),
            status: "already-up-to-date".to_string(),
            branch: current_branch_name,
            commit: head_to_rebase_id.to_string(),
            upstream: Some(upstream.to_string()),
            onto: Some(upstream_id.to_string()),
            common_ancestor: Some(base_id.to_string()),
            replay_count: Some(0),
            previous_commit: Some(head_to_rebase_id.to_string()),
            restored: None,
            applied_commits: Vec::new(),
            skipped_commit: None,
            skipped_subject: None,
            remaining: Some(0),
            autostashed: None,
            autosquashed: None,
            dropped_empty: None,
        });
    }

    let mut commits_to_replay = collect_commits_to_replay(&base_id, &head_to_rebase_id)
        .await
        .map_err(|detail| RebaseError::CommitLoad {
            commit: head_to_rebase_id.to_string(),
            detail,
        })?;
    if !options.reapply_cherry_picks {
        commits_to_replay = filter_redundant_cherry_picks(&commits_to_replay, &onto_id)?;
    }
    let autosquashed = if options.autosquash {
        let autosquash = autosquash_commits(commits_to_replay)?;
        commits_to_replay = autosquash.commits;
        autosquash.moved
    } else {
        0
    };
    if commits_to_replay.is_empty() {
        return Ok(RebaseOutput {
            action: "start".to_string(),
            status: "no-commits".to_string(),
            branch: current_branch_name,
            commit: head_to_rebase_id.to_string(),
            upstream: Some(upstream.to_string()),
            onto: Some(upstream_id.to_string()),
            common_ancestor: Some(base_id.to_string()),
            replay_count: Some(0),
            previous_commit: Some(head_to_rebase_id.to_string()),
            restored: None,
            applied_commits: Vec::new(),
            skipped_commit: None,
            skipped_subject: None,
            remaining: Some(0),
            autostashed: None,
            autosquashed: Some(autosquashed),
            dropped_empty: None,
        });
    }

    let upstream_commit: Commit = load_object(&onto_id).map_err(|e| RebaseError::CommitLoad {
        commit: onto_id.to_string(),
        detail: e.to_string(),
    })?;
    let upstream_tree: Tree =
        load_object(&upstream_commit.tree_id).map_err(|e| RebaseError::OriginalTreeLoad {
            tree: upstream_commit.tree_id.to_string(),
            detail: e.to_string(),
        })?;
    let mut guard_index = git_internal::internal::index::Index::new();
    rebuild_index_from_tree(&upstream_tree, &mut guard_index, "")
        .map_err(RebaseError::IndexRebuild)?;
    let autostash_ref = prepare_rebase_autostash(autostash).await?;
    if let Err(error) = rebase_worktree_guard_structured(&guard_index, "rebase").await {
        restore_rebase_autostash(autostash_ref.as_deref()).await;
        return Err(error);
    }

    let start_action = ReflogAction::Rebase {
        state: "start".to_string(),
        details: format!("checkout {}", onto_display),
    };
    let start_context = ReflogContext {
        old_oid: head_to_rebase_id.to_string(),
        new_oid: onto_id.to_string(),
        action: start_action,
        message: None,
    };
    db.transaction(|txn| {
        Box::pin(async move {
            reflog::Reflog::insert_single_entry(txn, &start_context, "HEAD").await?;
            Head::update_with_conn(txn, Head::Detached(onto_id), None).await;
            Ok::<_, ReflogError>(())
        })
    })
    .await
    .map_err(|e| RebaseError::Finalize(format!("failed to start rebase: {e}")))?;

    let replay_count = commits_to_replay.len();
    let mut state = RebaseState {
        head_name: current_branch_name.clone(),
        onto: onto_id,
        orig_head: head_to_rebase_id,
        todo: VecDeque::from(commits_to_replay),
        done: Vec::new(),
        stopped_sha: None,
        current_head: onto_id,
        autostash_ref,
        options,
    };

    state.save().await.map_err(RebaseError::StateSave)?;
    Head::update_with_conn(&db, Head::Detached(onto_id), None).await;

    let replay = continue_replay(&mut state, &current_branch_name, onto_display, false).await?;

    Ok(RebaseOutput {
        action: "start".to_string(),
        status: "completed".to_string(),
        branch: current_branch_name,
        commit: state.current_head.to_string(),
        upstream: Some(upstream.to_string()),
        onto: Some(onto_id.to_string()),
        common_ancestor: Some(base_id.to_string()),
        replay_count: Some(replay_count),
        previous_commit: Some(head_to_rebase_id.to_string()),
        restored: None,
        applied_commits: replay.applied_commits,
        skipped_commit: None,
        skipped_subject: None,
        remaining: Some(state.todo.len()),
        autostashed: Some(state.autostash_ref.is_some()),
        autosquashed: Some(replay.autosquashed),
        dropped_empty: Some(replay.dropped_empty),
    })
}

async fn run_rebase_root_start(
    onto: Option<&str>,
    options: RebaseRuntimeOptions,
    autostash: bool,
) -> Result<RebaseOutput, RebaseError> {
    let db = get_db_conn_instance().await;
    let current_branch_name = match Head::current().await {
        Head::Branch(name) if !name.is_empty() => name,
        _ => return Err(RebaseError::NotOnBranch),
    };
    let head_to_rebase_id =
        Head::current_commit()
            .await
            .ok_or_else(|| RebaseError::BranchHasNoCommits {
                branch: current_branch_name.clone(),
            })?;

    let onto_id = match onto {
        Some(newbase) => Some(resolve_branch_or_commit(newbase).await.map_err(|detail| {
            RebaseError::UpstreamResolve {
                upstream: newbase.to_string(),
                detail,
            }
        })?),
        None => None,
    };

    let mut commits_to_replay = collect_root_commits_to_replay(&head_to_rebase_id)?;
    if let Some(onto_id) = onto_id
        && !options.reapply_cherry_picks
    {
        commits_to_replay = filter_redundant_cherry_picks(&commits_to_replay, &onto_id)?;
    }
    if options.autosquash {
        let autosquash = autosquash_commits(commits_to_replay)?;
        commits_to_replay = autosquash.commits;
    }

    let guard_tree = match onto_id {
        Some(onto_id) => {
            let onto_commit: Commit =
                load_object(&onto_id).map_err(|error| RebaseError::CommitLoad {
                    commit: onto_id.to_string(),
                    detail: error.to_string(),
                })?;
            load_object::<Tree>(&onto_commit.tree_id).map_err(|error| {
                RebaseError::OriginalTreeLoad {
                    tree: onto_commit.tree_id.to_string(),
                    detail: error.to_string(),
                }
            })?
        }
        None => {
            let head_commit: Commit =
                load_object(&head_to_rebase_id).map_err(|error| RebaseError::CommitLoad {
                    commit: head_to_rebase_id.to_string(),
                    detail: error.to_string(),
                })?;
            load_object::<Tree>(&head_commit.tree_id).map_err(|error| {
                RebaseError::OriginalTreeLoad {
                    tree: head_commit.tree_id.to_string(),
                    detail: error.to_string(),
                }
            })?
        }
    };
    let mut guard_index = git_internal::internal::index::Index::new();
    rebuild_index_from_tree(&guard_tree, &mut guard_index, "")
        .map_err(RebaseError::IndexRebuild)?;
    let autostash_ref = prepare_rebase_autostash(autostash).await?;
    if let Err(error) = rebase_worktree_guard_structured(&guard_index, "rebase").await {
        restore_rebase_autostash(autostash_ref.as_deref()).await;
        return Err(error);
    }

    let replay_count = commits_to_replay.len();
    let mut done = Vec::new();
    let initial_head = if let Some(onto_id) = onto_id {
        onto_id
    } else {
        let first = commits_to_replay
            .first()
            .copied()
            .ok_or_else(|| RebaseError::InvalidArguments("no commits to rebase".to_string()))?;
        let first_commit: Commit =
            load_object(&first).map_err(|error| RebaseError::CommitLoad {
                commit: first.to_string(),
                detail: error.to_string(),
            })?;
        let rewritten =
            create_replayed_root_commit(&first_commit, first_commit.tree_id, options).await?;
        save_object(&rewritten, &rewritten.id)
            .map_err(|error| RebaseError::CommitSave(error.to_string()))?;
        commits_to_replay.remove(0);
        done.push(first);
        rewritten.id
    };

    let start_action = ReflogAction::Rebase {
        state: "start".to_string(),
        details: "rebase --root".to_string(),
    };
    let start_context = ReflogContext {
        old_oid: head_to_rebase_id.to_string(),
        new_oid: initial_head.to_string(),
        action: start_action,
        message: None,
    };
    db.transaction(|txn| {
        Box::pin(async move {
            reflog::Reflog::insert_single_entry(txn, &start_context, "HEAD").await?;
            Head::update_with_conn(txn, Head::Detached(initial_head), None).await;
            Ok::<_, ReflogError>(())
        })
    })
    .await
    .map_err(|e| RebaseError::Finalize(format!("failed to start rebase: {e}")))?;

    let mut state = RebaseState {
        head_name: current_branch_name.clone(),
        onto: initial_head,
        orig_head: head_to_rebase_id,
        todo: VecDeque::from(commits_to_replay),
        done,
        stopped_sha: None,
        current_head: initial_head,
        autostash_ref,
        options,
    };
    state.save().await.map_err(RebaseError::StateSave)?;

    let replay = continue_replay(&mut state, &current_branch_name, "--root", false).await?;

    Ok(RebaseOutput {
        action: "start".to_string(),
        status: "completed".to_string(),
        branch: current_branch_name,
        commit: state.current_head.to_string(),
        upstream: None,
        onto: Some(state.onto.to_string()),
        common_ancestor: None,
        replay_count: Some(replay_count),
        previous_commit: Some(head_to_rebase_id.to_string()),
        restored: None,
        applied_commits: replay.applied_commits,
        skipped_commit: None,
        skipped_subject: None,
        remaining: Some(state.todo.len()),
        autostashed: Some(state.autostash_ref.is_some()),
        autosquashed: Some(replay.autosquashed),
        dropped_empty: Some(replay.dropped_empty),
    })
}

/// Slim summary returned to `libra pull --rebase`. The full
/// [`RebaseOutput`] carries fields that only make sense for the
/// rebase subcommand (e.g. `restored`, `applied_commits`,
/// `skipped_subject`); pull only needs to render the integration
/// outcome alongside its fetch summary.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct PullRebaseSummary {
    /// One of `"fast-forwarded"`, `"already-up-to-date"`,
    /// `"completed"`, or `"no-commits"`.
    pub status: String,
    /// The branch that was rebased.
    pub branch: String,
    /// HEAD before the rebase.
    pub old_commit: String,
    /// HEAD after the rebase (== `old_commit` for the no-op cases).
    pub commit: String,
    /// The upstream tip the branch was rebased onto.
    pub onto: String,
    /// Number of commits replayed during the rebase. `0` for the
    /// fast-forward / already-up-to-date / no-commits branches.
    pub replay_count: usize,
}

/// Run `run_rebase_start` and project the result down to the
/// [`PullRebaseSummary`] that `libra pull --rebase` renders. Failure
/// modes (conflict, dirty worktree, etc.) propagate via
/// [`RebaseError`] which already has a `From<…> for CliError` impl
/// with structured hints — pull just wraps it in its own error
/// variant so the `phase=rebase` detail can be attached.
pub(crate) async fn run_rebase_for_pull(upstream: &str) -> Result<PullRebaseSummary, RebaseError> {
    let output = run_rebase_start(upstream, None, RebaseRuntimeOptions::default(), false).await?;
    let old_commit = output
        .previous_commit
        .clone()
        .unwrap_or_else(|| output.commit.clone());
    Ok(PullRebaseSummary {
        status: output.status,
        branch: output.branch,
        old_commit,
        commit: output.commit,
        onto: output.onto.unwrap_or_else(|| upstream.to_string()),
        replay_count: output.replay_count.unwrap_or(0),
    })
}

async fn prepare_rebase_autostash(enabled: bool) -> Result<Option<String>, RebaseError> {
    if !enabled {
        return Ok(None);
    }
    stash::autostash_push_with_message("rebase: autostash")
        .await
        .map_err(RebaseError::Autostash)
}

async fn restore_rebase_autostash(stash_id: Option<&str>) {
    let Some(stash_id) = stash_id else {
        return;
    };
    if let Err(error) = stash::autostash_pop_by_oid(stash_id).await {
        emit_warning(format!("failed to reapply autostashed changes: {error}"));
    }
}

/// Continue replaying commits from the current state
async fn continue_replay(
    state: &mut RebaseState,
    branch_name: &str,
    upstream_display: &str,
    emit_human: bool,
) -> Result<RebaseReplaySummary, RebaseError> {
    let db = get_db_conn_instance().await;
    let mut summary = RebaseReplaySummary::default();

    if emit_human {
        println!(
            "Rebasing {} commits from `{}` onto `{}`...",
            state.todo.len(),
            branch_name,
            upstream_display
        );
    }

    while let Some(commit_id) = state.todo.front().cloned() {
        let action = if state.options.autosquash {
            load_object::<Commit>(&commit_id)
                .map(|commit| RebaseTodoAction::from_message(&commit.message))
                .unwrap_or(RebaseTodoAction::Pick)
        } else {
            RebaseTodoAction::Pick
        };
        match replay_commit_with_conflict_detection(
            &commit_id,
            &state.current_head,
            action,
            state.options,
        )
        .await
        {
            ReplayResult::Success(replayed_commit_id) => {
                let subject = commit_subject_lossy(&commit_id, emit_human);
                state.current_head = replayed_commit_id;
                // Move commit from todo to done
                state.todo.pop_front();
                state.done.push(commit_id);
                state.stopped_sha = None;

                // Update HEAD
                Head::update_with_conn(&db, Head::Detached(state.current_head), None).await;

                if emit_human {
                    println!(
                        "Applied: {} {}",
                        short_object_id(&state.current_head),
                        subject
                    );
                }
                summary.applied_commits.push(RebaseAppliedCommitOutput {
                    original_commit: commit_id.to_string(),
                    commit: state.current_head.to_string(),
                    subject,
                });
                if action != RebaseTodoAction::Pick {
                    summary.autosquashed += 1;
                }

                // Save state after each successful commit
                if let Err(e) = state.save().await {
                    if emit_human {
                        emit_warning(format!("failed to save rebase state: {}", e));
                    } else {
                        return Err(RebaseError::StateSave(e));
                    }
                }
            }
            ReplayResult::DroppedEmpty => {
                state.todo.pop_front();
                state.done.push(commit_id);
                state.stopped_sha = None;
                summary.dropped_empty += 1;
                if let Err(e) = state.save().await {
                    return Err(RebaseError::StateSave(e));
                }
            }
            ReplayResult::Conflict { paths, message } => {
                let subject = commit_subject_lossy(&commit_id, emit_human);
                // Save state with stopped_sha
                state.stopped_sha = Some(commit_id);
                if let Err(e) = state.save().await {
                    return Err(RebaseError::StateSave(e));
                }

                if emit_human {
                    eprintln!(
                        "error: could not apply {}: {}",
                        short_object_id(&commit_id),
                        subject
                    );
                    if let Some(message) = message.as_ref() {
                        eprintln!("fatal: {}", message);
                    }

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
                }
                return Err(RebaseError::ReplayConflict {
                    commit: commit_id.to_string(),
                    subject,
                    paths,
                    message,
                });
            }
            ReplayResult::Internal { kind, detail } => {
                let subject = commit_subject_lossy(&commit_id, emit_human);
                state.stopped_sha = Some(commit_id);
                if let Err(e) = state.save().await {
                    return Err(RebaseError::StateSave(e));
                }

                if emit_human {
                    eprintln!(
                        "error: could not apply {}: {}",
                        short_object_id(&commit_id),
                        subject
                    );
                    eprintln!("fatal: {}: {}", kind.as_str(), detail);
                    eprintln!(
                        "To abort and return to the original branch, run 'libra rebase --abort'"
                    );
                }
                return Err(RebaseError::ReplayInternal {
                    commit: commit_id.to_string(),
                    subject,
                    kind,
                    detail,
                });
            }
        }
    }

    // All commits replayed successfully - finalize
    finalize_rebase(state, emit_human)
        .await
        .map_err(|e| RebaseError::Finalize(e.to_string()))?;
    Ok(summary)
}

/// Finalize rebase after all commits are replayed
async fn finalize_rebase(state: &RebaseState, emit_human: bool) -> anyhow::Result<()> {
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
        message: None,
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
                .await?;

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
        Head::update_with_conn(&db, Head::Detached(state.onto), None).await;
        return Err(e).context("failed to record reflog for rebase finish");
    }

    // Reset the working directory and index to match the final state
    // This ensures that the workspace reflects the rebased commits
    let final_commit: Commit =
        load_object(&state.current_head).context("failed to load final commit for rebase")?;
    let final_tree: Tree =
        load_object(&final_commit.tree_id).context("failed to load final tree for rebase")?;

    let index_file = path::index();
    let current_index = git_internal::internal::index::Index::load(&index_file)
        .map_err(|e| anyhow::anyhow!(e))
        .context("failed to load current index before rebase finish")?;
    let mut index = git_internal::internal::index::Index::new();
    rebuild_index_from_tree(&final_tree, &mut index, "")
        .map_err(|e| anyhow::anyhow!(e))
        .context("failed to rebuild index from final tree")?;
    index
        .save(&index_file)
        .map_err(|e| anyhow::anyhow!(e))
        .context("failed to save index after rebase")?;
    reset_workdir_tracked_only(&current_index, &index)
        .map_err(|e| anyhow::anyhow!(e))
        .context("failed to reset working directory after rebase")?;

    restore_rebase_autostash(state.autostash_ref.as_deref()).await;

    // Clean up rebase state
    if let Err(e) = RebaseState::cleanup().await {
        emit_warning(format!("failed to clean up rebase state: {}", e));
    }

    if emit_human {
        println!(
            "Successfully rebased branch '{}' onto '{}'.",
            state.head_name,
            short_object_id(&state.onto)
        );
    }
    Ok(())
}

async fn run_rebase_continue() -> Result<RebaseOutput, RebaseError> {
    ensure_rebase_in_progress().await?;
    let mut state = RebaseState::load().await.map_err(RebaseError::StateLoad)?;
    let previous_commit = state.current_head.to_string();
    let branch = state.head_name.clone();
    let onto_display = short_object_id(&state.onto);
    let mut applied_commits = Vec::new();
    let mut autosquashed = 0;
    let mut dropped_empty = 0;

    if let Some(stopped_sha) = state.stopped_sha {
        // Create a commit from the current index after the user has resolved
        // conflicts and staged the resolution.
        let index_file = path::index();
        let index = git_internal::internal::index::Index::load(&index_file)
            .map_err(|e| RebaseError::IndexLoad(e.to_string()))?;

        if has_unmerged_entries(&index) {
            return Err(RebaseError::UnresolvedConflicts);
        }

        let new_tree_id =
            create_tree_from_index(&index).map_err(|e| RebaseError::TreeCreate(e.to_string()))?;

        let original_commit: Commit =
            load_object(&stopped_sha).map_err(|e| RebaseError::CommitLoad {
                commit: stopped_sha.to_string(),
                detail: e.to_string(),
            })?;
        let subject = commit_subject_from_message(&original_commit.message);

        let action = if state.options.autosquash {
            RebaseTodoAction::from_message(&original_commit.message)
        } else {
            RebaseTodoAction::Pick
        };
        let new_commit = create_replayed_commit(
            &original_commit,
            new_tree_id,
            state.current_head,
            action,
            state.options,
        )
        .await?;
        save_object(&new_commit, &new_commit.id)
            .map_err(|e| RebaseError::CommitSave(e.to_string()))?;

        state.current_head = new_commit.id;
        state.todo.pop_front();
        state.done.push(stopped_sha);
        state.stopped_sha = None;

        let db = get_db_conn_instance().await;
        Head::update_with_conn(&db, Head::Detached(state.current_head), None).await;

        applied_commits.push(RebaseAppliedCommitOutput {
            original_commit: stopped_sha.to_string(),
            commit: state.current_head.to_string(),
            subject,
        });
    }

    if state.todo.is_empty() {
        finalize_rebase(&state, false)
            .await
            .map_err(|e| RebaseError::Finalize(e.to_string()))?;
    } else {
        state.save().await.map_err(RebaseError::StateSave)?;
        let replay = continue_replay(&mut state, &branch, &onto_display, false).await?;
        autosquashed += replay.autosquashed;
        dropped_empty += replay.dropped_empty;
        applied_commits.extend(replay.applied_commits);
    }

    Ok(RebaseOutput {
        action: "continue".to_string(),
        status: "completed".to_string(),
        branch,
        commit: state.current_head.to_string(),
        upstream: None,
        onto: Some(state.onto.to_string()),
        common_ancestor: None,
        replay_count: None,
        previous_commit: Some(previous_commit),
        restored: None,
        applied_commits,
        skipped_commit: None,
        skipped_subject: None,
        remaining: Some(state.todo.len()),
        autostashed: None,
        autosquashed: Some(autosquashed),
        dropped_empty: Some(dropped_empty),
    })
}

async fn run_rebase_abort() -> Result<RebaseOutput, RebaseError> {
    match RebaseState::is_in_progress().await {
        Ok(true) => {}
        Ok(false) => return Err(RebaseError::NoRebaseInProgress),
        Err(e) => return Err(RebaseError::StateCheck(e)),
    }

    let state = RebaseState::load().await.map_err(RebaseError::StateLoad)?;

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
        message: None,
    };

    let branch_name_cloned = state.head_name.clone();
    let orig_head = state.orig_head;
    let orig_head_str = orig_head.to_string();
    let orig_head_str_for_txn = orig_head_str.clone();
    let reflog_result = with_reflog(
        abort_context,
        move |txn: &sea_orm::DatabaseTransaction| {
            Box::pin(async move {
                Branch::update_branch_with_conn(
                    txn,
                    &branch_name_cloned,
                    &orig_head_str_for_txn,
                    None,
                )
                .await?;
                Head::update_with_conn(txn, Head::Branch(branch_name_cloned), None).await;
                Ok(())
            })
        },
        true,
    )
    .await;
    match reflog_result {
        Ok(()) => {}
        Err(e) => {
            emit_warning(format!("failed to record reflog: {e}"));
            // Continue anyway; ensure branch ref is corrected.
            if let Err(err) =
                Branch::update_branch_with_conn(&db, &state.head_name, &orig_head_str, None).await
            {
                return Err(RebaseError::BranchRestore {
                    branch: state.head_name.clone(),
                    detail: err.to_string(),
                });
            }
        }
    }
    Head::update_with_conn(&db, Head::Branch(state.head_name.clone()), None).await;

    // Reset working directory to original HEAD
    let orig_commit: Commit =
        load_object(&orig_head).map_err(|error| RebaseError::OriginalCommitLoad {
            commit: orig_head.to_string(),
            detail: error.to_string(),
        })?;
    let orig_tree: Tree =
        load_object(&orig_commit.tree_id).map_err(|error| RebaseError::OriginalTreeLoad {
            tree: orig_commit.tree_id.to_string(),
            detail: error.to_string(),
        })?;

    let index_file = path::index();
    let current_index = git_internal::internal::index::Index::load(&index_file)
        .map_err(|error| RebaseError::IndexLoad(error.to_string()))?;
    let mut index = git_internal::internal::index::Index::new();
    if let Err(e) = rebuild_index_from_tree(&orig_tree, &mut index, "") {
        return Err(RebaseError::IndexRebuild(e));
    }
    if let Err(e) = index.save(&index_file) {
        return Err(RebaseError::IndexSave(e.to_string()));
    }
    if let Err(e) = reset_workdir_tracked_only(&current_index, &index) {
        return Err(RebaseError::WorkdirReset(e));
    }

    restore_rebase_autostash(state.autostash_ref.as_deref()).await;

    // Clean up rebase state
    if let Err(e) = RebaseState::cleanup().await {
        emit_warning(format!("failed to clean up rebase state: {}", e));
    }

    Ok(RebaseOutput {
        action: "abort".to_string(),
        status: "aborted".to_string(),
        branch: state.head_name,
        commit: orig_head_str,
        upstream: None,
        onto: None,
        common_ancestor: None,
        replay_count: None,
        previous_commit: Some(state.current_head.to_string()),
        restored: Some(true),
        applied_commits: Vec::new(),
        skipped_commit: None,
        skipped_subject: None,
        remaining: None,
        autostashed: None,
        autosquashed: None,
        dropped_empty: None,
    })
}

async fn run_rebase_skip() -> Result<RebaseOutput, RebaseError> {
    ensure_rebase_in_progress().await?;
    let mut state = RebaseState::load().await.map_err(RebaseError::StateLoad)?;
    let previous_commit = state.current_head.to_string();
    let branch = state.head_name.clone();
    let onto_display = short_object_id(&state.onto);

    let skipped_sha = state
        .stopped_sha
        .or_else(|| state.todo.front().cloned())
        .ok_or(RebaseError::NoCommitToSkip)?;
    let skipped_subject = match load_object::<Commit>(&skipped_sha) {
        Ok(commit) => Some(commit_subject_from_message(&commit.message)),
        Err(_) => None,
    };

    state.todo.pop_front();
    state.stopped_sha = None;

    let current_commit: Commit =
        load_object(&state.current_head).map_err(|e| RebaseError::CommitLoad {
            commit: state.current_head.to_string(),
            detail: e.to_string(),
        })?;
    let current_tree: Tree =
        load_object(&current_commit.tree_id).map_err(|e| RebaseError::OriginalTreeLoad {
            tree: current_commit.tree_id.to_string(),
            detail: e.to_string(),
        })?;

    let index_file = path::index();
    let current_index = git_internal::internal::index::Index::load(&index_file)
        .map_err(|e| RebaseError::IndexLoad(e.to_string()))?;
    let mut index = git_internal::internal::index::Index::new();
    rebuild_index_from_tree(&current_tree, &mut index, "")
        .map_err(|e| RebaseError::IndexRebuild(e.to_string()))?;
    index
        .save(&index_file)
        .map_err(|e| RebaseError::IndexSave(e.to_string()))?;
    reset_workdir_tracked_only(&current_index, &index)
        .map_err(|e| RebaseError::WorkdirReset(e.to_string()))?;

    let mut applied_commits = Vec::new();
    let mut autosquashed = 0;
    let mut dropped_empty = 0;
    if state.todo.is_empty() {
        finalize_rebase(&state, false)
            .await
            .map_err(|e| RebaseError::Finalize(e.to_string()))?;
    } else {
        state.save().await.map_err(RebaseError::StateSave)?;
        let replay = continue_replay(&mut state, &branch, &onto_display, false).await?;
        autosquashed += replay.autosquashed;
        dropped_empty += replay.dropped_empty;
        applied_commits.extend(replay.applied_commits);
    }

    Ok(RebaseOutput {
        action: "skip".to_string(),
        status: "completed".to_string(),
        branch,
        commit: state.current_head.to_string(),
        upstream: None,
        onto: Some(state.onto.to_string()),
        common_ancestor: None,
        replay_count: None,
        previous_commit: Some(previous_commit),
        restored: None,
        applied_commits,
        skipped_commit: Some(skipped_sha.to_string()),
        skipped_subject,
        remaining: Some(state.todo.len()),
        autostashed: None,
        autosquashed: Some(autosquashed),
        dropped_empty: Some(dropped_empty),
    })
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
    let mut items: HashMap<PathBuf, RebaseTreeEntry> = HashMap::new();
    for path in index.tracked_files() {
        let path_str = path_to_index_key(&path)?;
        if let Some(entry) = index.get(path_str, 0) {
            items.insert(
                path.clone(),
                RebaseTreeEntry {
                    hash: entry.hash,
                    mode: index_mode_to_tree_item_mode(entry.mode)?,
                },
            );
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
    if let Ok(metadata) = fs::symlink_metadata(&file_path)
        && metadata.file_type().is_symlink()
    {
        fs::remove_file(&file_path)
            .map_err(|e| format!("failed to replace symlink {}: {}", file_path.display(), e))?;
    }
    fs::write(&file_path, content)
        .map_err(|e| format!("failed to write {}: {}", file_path.display(), e))
}

fn write_rebase_workdir_entry(
    workdir: &Path,
    path: &Path,
    entry: RebaseTreeEntry,
) -> Result<(), String> {
    let blob: Blob = load_object(&entry.hash).map_err(|error| {
        format!(
            "failed to load blob {} for worktree path '{}': {error}",
            entry.hash,
            path.display()
        )
    })?;
    write_workdir_blob(workdir, path, entry.mode, &blob.data)
}

fn write_workdir_blob(
    workdir: &Path,
    path: &Path,
    mode: TreeItemMode,
    content: &[u8],
) -> Result<(), String> {
    match mode {
        TreeItemMode::Blob => write_workdir_file(workdir, path, content),
        TreeItemMode::BlobExecutable => {
            write_workdir_file(workdir, path, content)?;
            set_executable_workdir_mode(&workdir.join(path))
        }
        TreeItemMode::Link => write_workdir_symlink(workdir, path, content),
        TreeItemMode::Tree => Err(format!(
            "tree entry cannot be written as a file: {}",
            path.display()
        )),
        TreeItemMode::Commit => Err(format!(
            "gitlink entries are not supported by rebase: {}",
            path.display()
        )),
    }
}

#[cfg(unix)]
fn set_executable_workdir_mode(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).map_err(|error| {
        format!(
            "failed to set executable mode on {}: {error}",
            path.display()
        )
    })
}

#[cfg(not(unix))]
fn set_executable_workdir_mode(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn write_workdir_symlink(workdir: &Path, path: &Path, target: &[u8]) -> Result<(), String> {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};

    let file_path = workdir.join(path);
    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    if fs::symlink_metadata(&file_path).is_ok() {
        fs::remove_file(&file_path)
            .map_err(|error| format!("failed to replace {}: {error}", file_path.display()))?;
    }
    std::os::unix::fs::symlink(
        PathBuf::from(OsString::from_vec(target.to_vec())),
        &file_path,
    )
    .map_err(|error| format!("failed to create symlink {}: {error}", file_path.display()))
}

#[cfg(not(unix))]
fn write_workdir_symlink(workdir: &Path, path: &Path, target: &[u8]) -> Result<(), String> {
    write_workdir_file(workdir, path, target)
}

fn write_conflict_file(workdir: &Path, path: &Path, content: &str) -> Result<(), String> {
    write_workdir_file(workdir, path, content.as_bytes())
        .map_err(|e| format!("conflict file: {}", e))
}

fn conflict_marker_eol() -> &'static str {
    if cfg!(windows) { "\r\n" } else { "\n" }
}

fn conflict_payload(content: &[u8]) -> Cow<'_, str> {
    match std::str::from_utf8(content) {
        Ok(text) => Cow::Borrowed(text),
        Err(_) => Cow::Owned(format!("[binary content, {} bytes]", content.len())),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RebaseTreeEntry {
    hash: ObjectHash,
    mode: TreeItemMode,
}

fn collect_tree_items_and_paths<'a>(
    trees: impl IntoIterator<Item = &'a Tree>,
) -> (Vec<HashMap<PathBuf, RebaseTreeEntry>>, HashSet<PathBuf>) {
    let mut items = Vec::new();
    let mut all_paths = HashSet::new();
    for tree in trees {
        let map: HashMap<PathBuf, RebaseTreeEntry> = tree
            .get_plain_items_with_mode()
            .into_iter()
            .filter_map(|(path, hash, mode)| {
                if mode == TreeItemMode::Commit {
                    None
                } else {
                    Some((path, RebaseTreeEntry { hash, mode }))
                }
            })
            .collect();
        all_paths.extend(map.keys().cloned());
        items.push(map);
    }
    (items, all_paths)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{HashMap, HashSet},
        path::{Path, PathBuf},
    };

    use clap::Parser;
    use git_internal::{
        hash::ObjectHash,
        internal::object::{
            blob::Blob,
            commit::Commit,
            tree::{Tree, TreeItem, TreeItemMode},
        },
    };
    use tempfile::tempdir;

    #[cfg(unix)]
    use super::path_to_index_key;
    use super::{
        EmptyMode, RebaseArgs, RebaseError, RebaseTreeEntry, ReplayErrorKind,
        classify_relative_to_base, collect_tree_items_and_paths, create_tree_from_items_map,
        index_mode_to_tree_item_mode, resolve_rebase_start_request, resolve_three_way,
        tree_item_mode_to_index_mode, tree_item_name, write_workdir_blob,
    };
    use crate::{
        command::{load_object, save_object},
        utils::{
            error::{CliError, StableErrorCode},
            test::{ChangeDirGuard, setup_with_new_libra_in},
        },
    };

    fn rebase_entry(byte: u8, mode: TreeItemMode) -> RebaseTreeEntry {
        RebaseTreeEntry {
            hash: ObjectHash::new(&[byte; 20]),
            mode,
        }
    }

    #[test]
    fn rebase_args_onto_three_positionals_parse() {
        let args = RebaseArgs::try_parse_from(["rebase", "--onto", "next", "main", "topic"])
            .expect("--onto with upstream and branch should parse");
        assert_eq!(args.onto.as_deref(), Some("next"));
        assert_eq!(args.upstream.as_deref(), Some("main"));
        assert_eq!(args.branch.as_deref(), Some("topic"));
        assert!(!args.continue_rebase);
        assert!(!args.abort);
        assert!(!args.skip);
    }

    #[test]
    fn rebase_args_root_with_optional_branch_parse() {
        let args = RebaseArgs::try_parse_from(["rebase", "--root", "topic"])
            .expect("--root with one optional branch should parse");
        assert!(args.root);
        assert_eq!(args.upstream.as_deref(), Some("topic"));
        assert_eq!(args.branch.as_deref(), None);

        let request = resolve_rebase_start_request(&args)
            .expect("root request should resolve")
            .expect("root request should start a rebase");
        assert!(request.root);
        assert_eq!(request.upstream.as_deref(), None);
        assert_eq!(request.branch.as_deref(), Some("topic"));
    }

    #[test]
    fn rebase_args_root_rejects_two_positionals() {
        let args = RebaseArgs::try_parse_from(["rebase", "--root", "first", "second"])
            .expect("clap should leave semantic root positional validation to rebase");
        let error = resolve_rebase_start_request(&args)
            .expect_err("--root accepts only one optional branch positional");
        assert_eq!(error.stable_code(), StableErrorCode::CliInvalidArguments);
    }

    #[test]
    fn rebase_args_advanced_flags_parse() {
        let args = RebaseArgs::try_parse_from([
            "rebase",
            "--autostash",
            "--autosquash",
            "--reapply-cherry-picks",
            "--keep-empty",
            "--empty=keep",
            "--signoff",
            "-S",
            "main",
        ])
        .expect("advanced rebase flags should parse");

        assert!(args.autostash);
        assert!(args.autosquash);
        assert!(args.reapply_cherry_picks);
        assert!(args.keep_empty);
        assert_eq!(args.empty, Some(EmptyMode::Keep));
        assert!(args.signoff);
        assert!(args.gpg_sign);
        assert_eq!(args.upstream.as_deref(), Some("main"));
    }

    #[test]
    fn rebase_args_state_modes_reject_onto_and_branch() {
        let continue_with_onto =
            RebaseArgs::try_parse_from(["rebase", "--continue", "--onto", "next"]);
        assert!(continue_with_onto.is_err());

        let abort_with_branch = RebaseArgs::try_parse_from(["rebase", "--abort", "topic"]);
        assert!(abort_with_branch.is_err());
    }

    #[test]
    fn replay_error_kind_stable_codes_route_distinct_failures() {
        // Object load failures point at repository corruption.
        for kind in [
            ReplayErrorKind::CommitLoad,
            ReplayErrorKind::MissingParent,
            ReplayErrorKind::BaseTreeLoad,
            ReplayErrorKind::TheirTreeLoad,
            ReplayErrorKind::OurTreeLoad,
            ReplayErrorKind::NewTreeLoad,
        ] {
            assert_eq!(
                kind.stable_code(),
                StableErrorCode::RepoCorrupt,
                "{kind:?} should map to RepoCorrupt"
            );
        }

        // Pure index read maps to IO read.
        assert_eq!(
            ReplayErrorKind::IndexLoad.stable_code(),
            StableErrorCode::IoReadFailed
        );

        // Untracked file collision is a blocked operation, not an unresolved conflict.
        assert_eq!(
            ReplayErrorKind::UntrackedOverwrite.stable_code(),
            StableErrorCode::ConflictOperationBlocked
        );

        // Write/save side failures all surface as IO write failed.
        for kind in [
            ReplayErrorKind::ConflictMarker,
            ReplayErrorKind::TreeCreate,
            ReplayErrorKind::CommitSave,
            ReplayErrorKind::IndexRebuild,
            ReplayErrorKind::IndexSave,
            ReplayErrorKind::WorkdirReset,
        ] {
            assert_eq!(
                kind.stable_code(),
                StableErrorCode::IoWriteFailed,
                "{kind:?} should map to IoWriteFailed"
            );
        }
    }

    #[test]
    fn replay_error_kind_serializes_snake_case_identifiers() {
        assert_eq!(ReplayErrorKind::IndexLoad.as_str(), "index_load");
        assert_eq!(ReplayErrorKind::CommitLoad.as_str(), "commit_load");
        assert_eq!(ReplayErrorKind::MissingParent.as_str(), "missing_parent");
        assert_eq!(ReplayErrorKind::BaseTreeLoad.as_str(), "base_tree_load");
        assert_eq!(ReplayErrorKind::TheirTreeLoad.as_str(), "their_tree_load");
        assert_eq!(ReplayErrorKind::OurTreeLoad.as_str(), "our_tree_load");
        assert_eq!(
            ReplayErrorKind::UntrackedOverwrite.as_str(),
            "untracked_overwrite"
        );
        assert_eq!(ReplayErrorKind::ConflictMarker.as_str(), "conflict_marker");
        assert_eq!(ReplayErrorKind::TreeCreate.as_str(), "tree_create");
        assert_eq!(ReplayErrorKind::CommitSave.as_str(), "commit_save");
        assert_eq!(ReplayErrorKind::NewTreeLoad.as_str(), "new_tree_load");
        assert_eq!(ReplayErrorKind::IndexRebuild.as_str(), "index_rebuild");
        assert_eq!(ReplayErrorKind::IndexSave.as_str(), "index_save");
        assert_eq!(ReplayErrorKind::WorkdirReset.as_str(), "workdir_reset");
    }

    /// Pin the `Display` format for the static-message `RebaseError`
    /// variants. These strings are used directly as the `CliError`
    /// message via `CliError::fatal(error.to_string())` in the
    /// `From<RebaseError> for CliError` mapping, so they're part of
    /// the human + JSON output contract.
    ///
    /// Source-chained variants (CheckStateLoad, LoadStateError,
    /// UpstreamLookup, WorktreeStatus, etc.) are intentionally not
    /// pinned here — their `{0}` slot forwards to upstream Display
    /// strings owned by other modules.
    #[test]
    fn rebase_error_display_pins_static_message_variants() {
        assert_eq!(
            RebaseError::NoRebaseInProgress.to_string(),
            "no rebase in progress",
        );
        assert_eq!(
            RebaseError::NotOnBranch.to_string(),
            "not on a branch or in detached HEAD state, cannot rebase",
        );
        assert_eq!(
            RebaseError::NoCommonAncestor.to_string(),
            "no common ancestor found",
        );
        assert_eq!(
            RebaseError::AmbiguousMergeBase {
                bases: "aaa, bbb".to_string(),
            }
            .to_string(),
            "multiple best merge bases found (aaa, bbb); criss-cross merge bases are unsupported",
        );
        assert_eq!(
            RebaseError::UnresolvedConflicts.to_string(),
            "you must resolve all conflicts before continuing",
        );
        assert_eq!(RebaseError::NoCommitToSkip.to_string(), "no commit to skip");
        assert_eq!(
            RebaseError::BranchHasNoCommits {
                branch: "main".to_string(),
            }
            .to_string(),
            "current branch 'main' has no commits",
        );
        assert_eq!(
            RebaseError::UntrackedOverwrite {
                path: "scratch.txt".to_string(),
            }
            .to_string(),
            "untracked working tree file would be overwritten by rebase: scratch.txt",
        );
        assert_eq!(
            RebaseError::UpstreamResolve {
                upstream: "origin/main".to_string(),
                detail: "not a valid object".to_string(),
            }
            .to_string(),
            "failed to resolve upstream 'origin/main': not a valid object",
        );
        assert_eq!(
            RebaseError::WorktreeDirty {
                action: "switch".to_string(),
                detail: "uncommitted changes".to_string(),
            }
            .to_string(),
            "uncommitted changes, can't switch",
        );
        assert_eq!(
            RebaseError::Autostash("stash apply failed".to_string()).to_string(),
            "autostash failed: stash apply failed",
        );
        assert_eq!(
            RebaseError::InvalidArguments("bad root form".to_string()).to_string(),
            "bad root form",
        );
        assert_eq!(
            RebaseError::Sign("vault is locked".to_string()).to_string(),
            "failed to sign rebased commit: vault is locked",
        );
    }

    /// Pin the `From<RebaseError> for CliError` stable_code mapping
    /// for every RebaseError variant. RebaseError itself has no
    /// `stable_code()` method — the routing lives in the `From`
    /// impl at `:623-722`, so this is the only place where the
    /// wire surface ("which StableErrorCode does each variant
    /// produce in --json envelopes?") can be locked down.
    ///
    /// The variants collapse into a small stable-code set via a match
    /// with many alternations. A future refactor that re-routed
    /// any variant — e.g. flipping `OriginalTreeLoad` from
    /// `RepoCorrupt` to `IoReadFailed`, or accidentally landing
    /// `IndexLoad` in the IoWriteFailed group with its siblings —
    /// would silently change client retry classification unless
    /// every variant has its own guard.
    ///
    /// `ReplayInternal` delegates to `ReplayErrorKind::stable_code()`
    /// which has its own enumeration in
    /// `replay_error_kind_stable_codes_route_distinct_failures`; we
    /// pin one representative kind (`CommitSave`) here to lock the
    /// delegation itself.
    ///
    /// Continuation of the v0.17.701..v0.17.708 surface-contract
    /// sweep (TuiControlError / CherryPickError / RevertError /
    /// RestoreError / StashError / ResetError / FuseUmountError /
    /// WorktreeError). Per the prioritised backlog, rebase.rs was
    /// the last HIGH-priority pin gap.
    #[test]
    fn rebase_error_stable_code_pins_each_variant() {
        fn code_of(err: RebaseError) -> StableErrorCode {
            CliError::from(err).stable_code()
        }

        assert_eq!(
            code_of(RebaseError::NoRebaseInProgress),
            StableErrorCode::RepoStateInvalid,
        );
        assert_eq!(
            code_of(RebaseError::StateCheck("ignored".to_string())),
            StableErrorCode::IoReadFailed,
        );
        assert_eq!(
            code_of(RebaseError::StateLoad("ignored".to_string())),
            StableErrorCode::IoReadFailed,
        );
        assert_eq!(
            code_of(RebaseError::NotOnBranch),
            StableErrorCode::RepoStateInvalid,
        );
        assert_eq!(
            code_of(RebaseError::BranchHasNoCommits {
                branch: "ignored".to_string(),
            }),
            StableErrorCode::RepoStateInvalid,
        );
        assert_eq!(
            code_of(RebaseError::UpstreamResolve {
                upstream: "ignored".to_string(),
                detail: "ignored".to_string(),
            }),
            StableErrorCode::CliInvalidTarget,
        );
        assert_eq!(
            code_of(RebaseError::NoCommonAncestor),
            StableErrorCode::CliInvalidTarget,
        );
        assert_eq!(
            code_of(RebaseError::AmbiguousMergeBase {
                bases: "aaa, bbb".to_string(),
            }),
            StableErrorCode::ConflictOperationBlocked,
        );
        assert_eq!(
            code_of(RebaseError::WorktreeStatus("ignored".to_string())),
            StableErrorCode::IoReadFailed,
        );
        assert_eq!(
            code_of(RebaseError::WorktreeDirty {
                action: "ignored".to_string(),
                detail: "ignored".to_string(),
            }),
            StableErrorCode::RepoStateInvalid,
        );
        assert_eq!(
            code_of(RebaseError::Autostash("ignored".to_string())),
            StableErrorCode::RepoStateInvalid,
        );
        assert_eq!(
            code_of(RebaseError::InvalidArguments("ignored".to_string())),
            StableErrorCode::CliInvalidArguments,
        );
        assert_eq!(
            code_of(RebaseError::Sign("ignored".to_string())),
            StableErrorCode::AuthMissingCredentials,
        );
        assert_eq!(
            code_of(RebaseError::UntrackedOverwrite {
                path: "ignored".to_string(),
            }),
            StableErrorCode::ConflictOperationBlocked,
        );
        assert_eq!(
            code_of(RebaseError::UnresolvedConflicts),
            StableErrorCode::ConflictUnresolved,
        );
        assert_eq!(
            code_of(RebaseError::NoCommitToSkip),
            StableErrorCode::RepoStateInvalid,
        );
        assert_eq!(
            code_of(RebaseError::ReplayConflict {
                commit: "ignored".to_string(),
                subject: "ignored".to_string(),
                paths: Vec::new(),
                message: None,
            }),
            StableErrorCode::ConflictUnresolved,
        );
        // ReplayInternal delegates to ReplayErrorKind::stable_code();
        // exhaustive ReplayErrorKind routing is pinned by
        // replay_error_kind_stable_codes_route_distinct_failures.
        assert_eq!(
            code_of(RebaseError::ReplayInternal {
                commit: "ignored".to_string(),
                subject: "ignored".to_string(),
                kind: ReplayErrorKind::CommitSave,
                detail: "ignored".to_string(),
            }),
            StableErrorCode::IoWriteFailed,
        );
        assert_eq!(
            code_of(RebaseError::BranchRestore {
                branch: "ignored".to_string(),
                detail: "ignored".to_string(),
            }),
            StableErrorCode::IoWriteFailed,
        );
        assert_eq!(
            code_of(RebaseError::CommitLoad {
                commit: "ignored".to_string(),
                detail: "ignored".to_string(),
            }),
            StableErrorCode::RepoCorrupt,
        );
        assert_eq!(
            code_of(RebaseError::OriginalCommitLoad {
                commit: "ignored".to_string(),
                detail: "ignored".to_string(),
            }),
            StableErrorCode::RepoCorrupt,
        );
        assert_eq!(
            code_of(RebaseError::OriginalTreeLoad {
                tree: "ignored".to_string(),
                detail: "ignored".to_string(),
            }),
            StableErrorCode::RepoCorrupt,
        );
        assert_eq!(
            code_of(RebaseError::IndexLoad("ignored".to_string())),
            StableErrorCode::IoReadFailed,
        );
        assert_eq!(
            code_of(RebaseError::TreeCreate("ignored".to_string())),
            StableErrorCode::IoWriteFailed,
        );
        assert_eq!(
            code_of(RebaseError::CommitSave("ignored".to_string())),
            StableErrorCode::IoWriteFailed,
        );
        assert_eq!(
            code_of(RebaseError::IndexRebuild("ignored".to_string())),
            StableErrorCode::IoWriteFailed,
        );
        assert_eq!(
            code_of(RebaseError::IndexSave("ignored".to_string())),
            StableErrorCode::IoWriteFailed,
        );
        assert_eq!(
            code_of(RebaseError::WorkdirReset("ignored".to_string())),
            StableErrorCode::IoWriteFailed,
        );
        assert_eq!(
            code_of(RebaseError::StateSave("ignored".to_string())),
            StableErrorCode::IoWriteFailed,
        );
        assert_eq!(
            code_of(RebaseError::Finalize("ignored".to_string())),
            StableErrorCode::IoWriteFailed,
        );
    }

    #[tokio::test]
    async fn find_merge_base_maps_criss_cross_to_typed_rebase_error() {
        let repo = tempdir().expect("repo tempdir");
        setup_with_new_libra_in(repo.path()).await;
        let _guard = ChangeDirGuard::new(repo.path());

        let blob = Blob::from_content("fixture\n");
        save_object(&blob, &blob.id).expect("save fixture blob");
        let tree = Tree::from_tree_items(vec![TreeItem::new(
            TreeItemMode::Blob,
            blob.id,
            "fixture.txt".to_string(),
        )])
        .expect("fixture tree should be valid");
        save_object(&tree, &tree.id).expect("save tree");

        let base_a = Commit::from_tree_id(tree.id, Vec::new(), "base a");
        save_object(&base_a, &base_a.id).expect("save base a");
        let base_b = Commit::from_tree_id(tree.id, Vec::new(), "base b");
        save_object(&base_b, &base_b.id).expect("save base b");
        let left = Commit::from_tree_id(tree.id, vec![base_a.id, base_b.id], "left");
        save_object(&left, &left.id).expect("save left");
        let right = Commit::from_tree_id(tree.id, vec![base_b.id, base_a.id], "right");
        save_object(&right, &right.id).expect("save right");

        let err = super::find_merge_base(&left.id, &right.id)
            .await
            .expect_err("criss-cross graph should be ambiguous");

        match err {
            RebaseError::AmbiguousMergeBase { bases } => {
                assert!(bases.contains(&base_a.id.to_string()));
                assert!(bases.contains(&base_b.id.to_string()));
            }
            other => panic!("expected AmbiguousMergeBase, got {other:?}"),
        }
    }

    #[test]
    fn replay_internal_error_maps_to_typed_cli_error() {
        let rebase_err = RebaseError::ReplayInternal {
            commit: "deadbeef".to_string(),
            subject: "refactor: split error kinds".to_string(),
            kind: ReplayErrorKind::CommitSave,
            detail: "disk full".to_string(),
        };
        let cli_err: CliError = rebase_err.into();
        let json: serde_json::Value = serde_json::from_str(&cli_err.render_json())
            .expect("CliError JSON payload should parse");

        assert_eq!(
            json.get("error_code").and_then(|v| v.as_str()),
            Some("LBR-IO-002")
        );
        assert_eq!(
            json.pointer("/details/kind").and_then(|v| v.as_str()),
            Some("commit_save")
        );
        assert_eq!(
            json.pointer("/details/commit").and_then(|v| v.as_str()),
            Some("deadbeef")
        );
        assert_eq!(
            json.pointer("/details/detail").and_then(|v| v.as_str()),
            Some("disk full")
        );
    }

    #[test]
    fn replay_internal_repo_corrupt_kind_keeps_separate_code() {
        let rebase_err = RebaseError::ReplayInternal {
            commit: "feedface".to_string(),
            subject: "feat: add provider".to_string(),
            kind: ReplayErrorKind::BaseTreeLoad,
            detail: "object 1234 not found".to_string(),
        };
        let cli_err: CliError = rebase_err.into();
        let json: serde_json::Value = serde_json::from_str(&cli_err.render_json())
            .expect("CliError JSON payload should parse");

        // Was previously LBR-CONFLICT-001; now distinct from real merge conflicts.
        assert_eq!(
            json.get("error_code").and_then(|v| v.as_str()),
            Some("LBR-REPO-002")
        );
        assert_eq!(
            json.pointer("/details/kind").and_then(|v| v.as_str()),
            Some("base_tree_load")
        );
    }

    #[test]
    fn tree_item_name_rejects_paths_without_file_name() {
        let err = tree_item_name(Path::new("")).expect_err("empty path should fail");
        assert!(err.contains("path has no file name"));
    }

    #[cfg(unix)]
    #[test]
    fn tree_item_name_rejects_non_utf8_paths() {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};

        let path = PathBuf::from(OsString::from_vec(vec![0x66, 0x80]));
        let err = tree_item_name(&path).expect_err("non-UTF-8 path should fail");
        assert!(err.contains("path is not valid UTF-8"));
    }

    #[cfg(unix)]
    #[test]
    fn path_to_index_key_rejects_non_utf8_paths() {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};

        let path = PathBuf::from(OsString::from_vec(vec![0x66, 0x80]));
        let err = path_to_index_key(&path).expect_err("non-UTF-8 path should fail");
        assert!(err.contains("path is not valid UTF-8"));
    }

    #[test]
    fn collect_tree_items_and_paths_unions_paths_and_preserves_items() {
        let a_hash = ObjectHash::new(&[1; 20]);
        let b_hash = ObjectHash::new(&[2; 20]);
        let b2_hash = ObjectHash::new(&[3; 20]);
        let c_hash = ObjectHash::new(&[4; 20]);

        let tree1 = Tree::from_tree_items(vec![
            TreeItem::new(TreeItemMode::Blob, a_hash, "a.txt".to_string()),
            TreeItem::new(TreeItemMode::BlobExecutable, b_hash, "b.txt".to_string()),
        ])
        .expect("tree1");

        let tree2 = Tree::from_tree_items(vec![
            TreeItem::new(TreeItemMode::Blob, b2_hash, "b.txt".to_string()),
            TreeItem::new(TreeItemMode::Link, c_hash, "c.txt".to_string()),
        ])
        .expect("tree2");

        let (items, all_paths) = collect_tree_items_and_paths([&tree1, &tree2]);
        assert_eq!(items.len(), 2);

        let expected_first: HashMap<PathBuf, RebaseTreeEntry> = HashMap::from([
            (
                PathBuf::from("a.txt"),
                RebaseTreeEntry {
                    hash: a_hash,
                    mode: TreeItemMode::Blob,
                },
            ),
            (
                PathBuf::from("b.txt"),
                RebaseTreeEntry {
                    hash: b_hash,
                    mode: TreeItemMode::BlobExecutable,
                },
            ),
        ]);
        let expected_second: HashMap<PathBuf, RebaseTreeEntry> = HashMap::from([
            (
                PathBuf::from("b.txt"),
                RebaseTreeEntry {
                    hash: b2_hash,
                    mode: TreeItemMode::Blob,
                },
            ),
            (
                PathBuf::from("c.txt"),
                RebaseTreeEntry {
                    hash: c_hash,
                    mode: TreeItemMode::Link,
                },
            ),
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
        let base = rebase_entry(1, TreeItemMode::Blob);
        let same = base;
        let modified = rebase_entry(2, TreeItemMode::BlobExecutable);

        match classify_relative_to_base(Some(&base), Some(&same)) {
            super::RelativeState::Same(entry) => assert_eq!(entry, base),
            other => panic!("expected Same, got {:?}", other),
        }

        match classify_relative_to_base(Some(&base), Some(&modified)) {
            super::RelativeState::Modified(entry) => assert_eq!(entry, modified),
            other => panic!("expected Modified, got {:?}", other),
        }

        match classify_relative_to_base(Some(&base), None) {
            super::RelativeState::Deleted => {}
            other => panic!("expected Deleted, got {:?}", other),
        }

        match classify_relative_to_base(None, Some(&modified)) {
            super::RelativeState::Added(entry) => assert_eq!(entry, modified),
            other => panic!("expected Added, got {:?}", other),
        }

        match classify_relative_to_base(None, None) {
            super::RelativeState::Missing => {}
            other => panic!("expected Missing, got {:?}", other),
        }
    }

    #[test]
    fn resolve_three_way_merges_and_conflicts() {
        let base = rebase_entry(1, TreeItemMode::Blob);
        let ours = rebase_entry(2, TreeItemMode::BlobExecutable);
        let theirs = rebase_entry(3, TreeItemMode::Link);

        match resolve_three_way(Some(&base), Some(&base), Some(&base)) {
            super::MergeResolution::Use(entry) => assert_eq!(entry, base),
            other => panic!("expected Use(base), got {:?}", other),
        }

        match resolve_three_way(Some(&base), Some(&base), Some(&ours)) {
            super::MergeResolution::Use(entry) => assert_eq!(entry, ours),
            other => panic!("expected Use(ours), got {:?}", other),
        }

        match resolve_three_way(Some(&base), Some(&theirs), Some(&base)) {
            super::MergeResolution::Use(entry) => assert_eq!(entry, theirs),
            other => panic!("expected Use(theirs), got {:?}", other),
        }

        match resolve_three_way(Some(&base), Some(&theirs), Some(&ours)) {
            super::MergeResolution::Conflict(super::ConflictKind::BothChanged {
                ours: o,
                theirs: t,
            }) => {
                assert_eq!(o, ours.hash);
                assert_eq!(t, theirs.hash);
            }
            other => panic!("expected BothChanged conflict, got {:?}", other),
        }

        match resolve_three_way(None, Some(&theirs), Some(&ours)) {
            super::MergeResolution::Conflict(super::ConflictKind::BothChanged {
                ours: o,
                theirs: t,
            }) => {
                assert_eq!(o, ours.hash);
                assert_eq!(t, theirs.hash);
            }
            other => panic!("expected BothChanged conflict (add/add), got {:?}", other),
        }

        match resolve_three_way(Some(&base), None, Some(&ours)) {
            super::MergeResolution::Conflict(super::ConflictKind::OursModifiedTheirsDeleted {
                ours: o,
            }) => assert_eq!(o, ours.hash),
            other => panic!(
                "expected ours-modified/theirs-deleted conflict, got {:?}",
                other
            ),
        }

        match resolve_three_way(Some(&base), Some(&theirs), None) {
            super::MergeResolution::Conflict(super::ConflictKind::TheirsModifiedOursDeleted {
                theirs: t,
            }) => assert_eq!(t, theirs.hash),
            other => panic!(
                "expected theirs-modified/ours-deleted conflict, got {:?}",
                other
            ),
        }
    }

    #[test]
    fn rebase_index_tree_mode_conversions_pin_supported_modes() {
        assert_eq!(
            tree_item_mode_to_index_mode(TreeItemMode::Blob).expect("regular blob"),
            0o100644
        );
        assert_eq!(
            tree_item_mode_to_index_mode(TreeItemMode::BlobExecutable).expect("executable blob"),
            0o100755
        );
        assert_eq!(
            tree_item_mode_to_index_mode(TreeItemMode::Link).expect("symlink"),
            0o120000
        );

        assert_eq!(
            index_mode_to_tree_item_mode(0o100644).expect("regular blob"),
            TreeItemMode::Blob
        );
        assert_eq!(
            index_mode_to_tree_item_mode(0o100755).expect("executable blob"),
            TreeItemMode::BlobExecutable
        );
        assert_eq!(
            index_mode_to_tree_item_mode(0o120000).expect("symlink"),
            TreeItemMode::Link
        );
        assert!(tree_item_mode_to_index_mode(TreeItemMode::Commit).is_err());
        assert!(index_mode_to_tree_item_mode(0o160000).is_err());
    }

    #[tokio::test]
    async fn create_tree_from_items_map_preserves_blob_modes() {
        let repo = tempdir().expect("temp repo");
        setup_with_new_libra_in(repo.path()).await;
        let _guard = ChangeDirGuard::new(repo.path());

        let executable = rebase_entry(1, TreeItemMode::BlobExecutable);
        let symlink = rebase_entry(2, TreeItemMode::Link);
        let regular = rebase_entry(3, TreeItemMode::Blob);
        let items = HashMap::from([
            (PathBuf::from("run.sh"), executable),
            (PathBuf::from("link"), symlink),
            (PathBuf::from("plain.txt"), regular),
        ]);

        let tree_id = create_tree_from_items_map(&items).expect("create tree");
        let tree: Tree = load_object(&tree_id).expect("load created tree");
        let modes: HashMap<_, _> = tree
            .tree_items
            .iter()
            .map(|item| (item.name.as_str(), item.mode))
            .collect();

        assert_eq!(modes.get("run.sh"), Some(&TreeItemMode::BlobExecutable));
        assert_eq!(modes.get("link"), Some(&TreeItemMode::Link));
        assert_eq!(modes.get("plain.txt"), Some(&TreeItemMode::Blob));
    }

    #[cfg(unix)]
    #[test]
    fn write_workdir_blob_replaces_existing_symlink() {
        let repo = tempdir().expect("temp repo");
        let target = repo.path().join("outside-target.txt");
        std::fs::write(&target, "outside\n").expect("write target");
        let link = repo.path().join("path.txt");
        std::os::unix::fs::symlink(&target, &link).expect("create symlink");

        write_workdir_blob(
            repo.path(),
            Path::new("path.txt"),
            TreeItemMode::Blob,
            b"regular\n",
        )
        .expect("write regular blob");

        assert!(
            !std::fs::symlink_metadata(&link)
                .expect("path metadata")
                .file_type()
                .is_symlink(),
            "regular blob write must replace an existing symlink"
        );
        assert_eq!(
            std::fs::read_to_string(&link).expect("read rewritten path"),
            "regular\n"
        );
        assert_eq!(
            std::fs::read_to_string(&target).expect("read symlink target"),
            "outside\n",
            "regular blob write must not follow and overwrite the old symlink target"
        );
    }

    #[test]
    fn replay_error_kind_display_pins_snake_case_for_each_variant() {
        assert_eq!(ReplayErrorKind::IndexLoad.to_string(), "index_load");
        assert_eq!(ReplayErrorKind::CommitLoad.to_string(), "commit_load");
        assert_eq!(ReplayErrorKind::MissingParent.to_string(), "missing_parent");
        assert_eq!(ReplayErrorKind::BaseTreeLoad.to_string(), "base_tree_load");
        assert_eq!(
            ReplayErrorKind::TheirTreeLoad.to_string(),
            "their_tree_load",
        );
        assert_eq!(ReplayErrorKind::OurTreeLoad.to_string(), "our_tree_load");
        assert_eq!(
            ReplayErrorKind::UntrackedOverwrite.to_string(),
            "untracked_overwrite",
        );
        assert_eq!(
            ReplayErrorKind::ConflictMarker.to_string(),
            "conflict_marker",
        );
        assert_eq!(ReplayErrorKind::TreeCreate.to_string(), "tree_create");
        assert_eq!(ReplayErrorKind::CommitSave.to_string(), "commit_save");
        assert_eq!(ReplayErrorKind::NewTreeLoad.to_string(), "new_tree_load");
        assert_eq!(ReplayErrorKind::IndexRebuild.to_string(), "index_rebuild");
        assert_eq!(ReplayErrorKind::IndexSave.to_string(), "index_save");
        assert_eq!(ReplayErrorKind::WorkdirReset.to_string(), "workdir_reset");
    }
}

async fn rebase_worktree_guard_structured(
    new_index: &git_internal::internal::index::Index,
    action: &str,
) -> Result<(), RebaseError> {
    let unstaged = status::changes_to_be_staged_with_policy(IgnorePolicy::Respect)
        .map_err(|err| RebaseError::WorktreeStatus(err.to_string()))?;
    if !unstaged.modified.is_empty() || !unstaged.deleted.is_empty() {
        return Err(RebaseError::WorktreeDirty {
            action: action.to_string(),
            detail: "unstaged changes".to_string(),
        });
    }

    let staged = status::changes_to_be_committed_safe()
        .await
        .map_err(|err| RebaseError::WorktreeStatus(err.to_string()))?;
    if !staged.new.is_empty() || !staged.modified.is_empty() || !staged.deleted.is_empty() {
        return Err(RebaseError::WorktreeDirty {
            action: action.to_string(),
            detail: "uncommitted changes".to_string(),
        });
    }

    if let Some(conflict) = worktree::untracked_overwrite_path(&unstaged.new, new_index) {
        return Err(RebaseError::UntrackedOverwrite {
            path: conflict.display().to_string(),
        });
    }

    Ok(())
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
    Use(RebaseTreeEntry),
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
    Same(RebaseTreeEntry),
    Modified(RebaseTreeEntry),
    Deleted,
    Added(RebaseTreeEntry),
    Missing,
}

fn classify_relative_to_base(
    base: Option<&RebaseTreeEntry>,
    side: Option<&RebaseTreeEntry>,
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
    base: Option<&RebaseTreeEntry>,
    theirs: Option<&RebaseTreeEntry>,
    ours: Option<&RebaseTreeEntry>,
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
                MergeResolution::Conflict(ConflictKind::BothChanged {
                    ours: o.hash,
                    theirs: t.hash,
                })
            }
        }
        (true, RelativeState::Same(o), RelativeState::Same(_)) => MergeResolution::Use(o),
        (true, RelativeState::Same(_), RelativeState::Modified(t)) => MergeResolution::Use(t),
        (true, RelativeState::Modified(o), RelativeState::Same(_)) => MergeResolution::Use(o),
        (true, RelativeState::Modified(o), RelativeState::Modified(t)) => {
            if o == t {
                MergeResolution::Use(t)
            } else {
                MergeResolution::Conflict(ConflictKind::BothChanged {
                    ours: o.hash,
                    theirs: t.hash,
                })
            }
        }
        (true, RelativeState::Deleted, RelativeState::Same(_)) => MergeResolution::Delete,
        (true, RelativeState::Same(_), RelativeState::Deleted) => MergeResolution::Delete,
        (true, RelativeState::Deleted, RelativeState::Deleted) => MergeResolution::Delete,
        (true, RelativeState::Deleted, RelativeState::Modified(t)) => {
            MergeResolution::Conflict(ConflictKind::TheirsModifiedOursDeleted { theirs: t.hash })
        }
        (true, RelativeState::Modified(o), RelativeState::Deleted) => {
            MergeResolution::Conflict(ConflictKind::OursModifiedTheirsDeleted { ours: o.hash })
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
            let our_text = conflict_payload(&our_content);
            let their_text = conflict_payload(&their_content);
            let conflict_content = format!(
                "<<<<<<< HEAD{marker_eol}{}{marker_eol}======={marker_eol}{}{marker_eol}>>>>>>> {}{marker_eol}",
                our_text, their_text, commit_abbrev
            );
            write_conflict_file(workdir, path, &conflict_content)?;
        }
        ConflictKind::OursModifiedTheirsDeleted { ours } => {
            let our_content = Blob::load(&ours).data;
            let our_text = conflict_payload(&our_content);
            let conflict_content = format!(
                "<<<<<<< HEAD{marker_eol}{}{marker_eol}======={marker_eol}>>>>>>> {} (deleted){marker_eol}",
                our_text, commit_abbrev
            );
            write_conflict_file(workdir, path, &conflict_content)?;
        }
        ConflictKind::TheirsModifiedOursDeleted { theirs } => {
            let their_content = Blob::load(&theirs).data;
            let their_text = conflict_payload(&their_content);
            let conflict_content = format!(
                "<<<<<<< HEAD (deleted){marker_eol}======={marker_eol}{}{marker_eol}>>>>>>> {}{marker_eol}",
                their_text, commit_abbrev
            );
            write_conflict_file(workdir, path, &conflict_content)?;
        }
    }
    Ok(())
}

fn collect_root_commits_to_replay(head_id: &ObjectHash) -> Result<Vec<ObjectHash>, RebaseError> {
    let roots = reachable_roots(head_id)?;
    if roots.len() > 1 {
        return Err(RebaseError::InvalidArguments(
            "rebase --root does not support histories with multiple root commits".to_string(),
        ));
    }
    let mut commits = Vec::new();
    let mut current = *head_id;
    loop {
        commits.push(current);
        let commit: Commit = load_object(&current).map_err(|error| RebaseError::CommitLoad {
            commit: current.to_string(),
            detail: error.to_string(),
        })?;
        let Some(parent) = commit.parent_commit_ids.first() else {
            break;
        };
        current = *parent;
    }
    commits.reverse();
    Ok(commits)
}

fn reachable_roots(head_id: &ObjectHash) -> Result<HashSet<ObjectHash>, RebaseError> {
    let mut roots = HashSet::new();
    let mut stack = vec![*head_id];
    let mut seen = HashSet::new();
    while let Some(commit_id) = stack.pop() {
        if !seen.insert(commit_id) {
            continue;
        }
        let commit: Commit = load_object(&commit_id).map_err(|error| RebaseError::CommitLoad {
            commit: commit_id.to_string(),
            detail: error.to_string(),
        })?;
        if commit.parent_commit_ids.is_empty() {
            roots.insert(commit_id);
        } else {
            stack.extend(commit.parent_commit_ids);
        }
    }
    Ok(roots)
}

async fn create_replayed_root_commit(
    original_commit: &Commit,
    tree_id: ObjectHash,
    options: RebaseRuntimeOptions,
) -> Result<Commit, RebaseError> {
    let mut message = original_commit.message.clone();
    if options.signoff {
        let (name, email) = merge::resolve_signoff_identity()
            .await
            .map_err(|error| RebaseError::Sign(error.to_string()))?;
        let trailer = format!("Signed-off-by: {name} <{email}>");
        if !message.contains(&trailer) {
            message.push_str("\n\n");
            message.push_str(&trailer);
        }
    }
    let parents = Vec::new();
    let committer = commit::current_committer_signature()
        .await
        .map_err(RebaseError::CommitSave)?;
    if options.gpg_sign {
        let gpgsig = commit::vault_sign_commit(
            &tree_id,
            &parents,
            &original_commit.author,
            &committer,
            &message,
            true,
        )
        .await
        .map_err(|error| RebaseError::Sign(error.to_string()))?;
        let sig = gpgsig.ok_or_else(|| {
            RebaseError::Sign(
                "vault signing key unavailable; configure libra vault to use --gpg-sign"
                    .to_string(),
            )
        })?;
        return Ok(Commit::new(
            original_commit.author.clone(),
            committer,
            tree_id,
            parents,
            &format_commit_msg(&message, Some(&sig)),
        ));
    }
    Ok(Commit::new(
        original_commit.author.clone(),
        committer,
        tree_id,
        parents,
        &format_commit_msg(&message, None),
    ))
}

async fn create_replayed_commit(
    original_commit: &Commit,
    tree_id: ObjectHash,
    new_parent_id: ObjectHash,
    action: RebaseTodoAction,
    options: RebaseRuntimeOptions,
) -> Result<Commit, RebaseError> {
    let (author, parents, mut message) = match action {
        RebaseTodoAction::Pick => (
            original_commit.author.clone(),
            vec![new_parent_id],
            original_commit.message.clone(),
        ),
        RebaseTodoAction::Fixup | RebaseTodoAction::Squash => {
            let target: Commit =
                load_object(&new_parent_id).map_err(|error| RebaseError::CommitLoad {
                    commit: new_parent_id.to_string(),
                    detail: error.to_string(),
                })?;
            let mut message = target.message.clone();
            if action == RebaseTodoAction::Squash {
                message.push_str("\n\n");
                message.push_str(original_commit.message.trim());
            }
            (
                target.author.clone(),
                target.parent_commit_ids.clone(),
                message,
            )
        }
    };

    if options.signoff {
        let (name, email) = merge::resolve_signoff_identity()
            .await
            .map_err(|error| RebaseError::Sign(error.to_string()))?;
        let trailer = format!("Signed-off-by: {name} <{email}>");
        if !message.contains(&trailer) {
            message.push_str("\n\n");
            message.push_str(&trailer);
        }
    }

    let committer = commit::current_committer_signature()
        .await
        .map_err(RebaseError::CommitSave)?;
    if options.gpg_sign {
        let gpgsig =
            commit::vault_sign_commit(&tree_id, &parents, &author, &committer, &message, true)
                .await
                .map_err(|error| RebaseError::Sign(error.to_string()))?;
        let sig = gpgsig.ok_or_else(|| {
            RebaseError::Sign(
                "vault signing key unavailable; configure libra vault to use --gpg-sign"
                    .to_string(),
            )
        })?;
        return Ok(Commit::new(
            author,
            committer,
            tree_id,
            parents,
            &format_commit_msg(&message, Some(&sig)),
        ));
    }

    Ok(Commit::new(
        author,
        committer,
        tree_id,
        parents,
        &format_commit_msg(&message, None),
    ))
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
    action: RebaseTodoAction,
    options: RebaseRuntimeOptions,
) -> ReplayResult {
    let index_file = path::index();
    let current_index = match git_internal::internal::index::Index::load(&index_file) {
        Ok(idx) => idx,
        Err(e) => {
            return ReplayResult::internal(ReplayErrorKind::IndexLoad, format!("{:?}", e));
        }
    };

    let commit_to_replay: Commit = match load_object(commit_to_replay_id) {
        Ok(c) => c,
        Err(e) => return ReplayResult::internal(ReplayErrorKind::CommitLoad, e.to_string()),
    };

    let original_parent_id = commit_to_replay.parent_commit_ids.first().copied();

    // Load the three trees needed for the three-way merge
    let base_tree: Tree = match original_parent_id {
        Some(parent_id) => match load_object::<Commit>(&parent_id)
            .and_then(|c| load_object(&c.tree_id))
        {
            Ok(t) => t,
            Err(e) => return ReplayResult::internal(ReplayErrorKind::BaseTreeLoad, e.to_string()),
        },
        None => match empty_tree() {
            Ok(tree) => tree,
            Err(e) => return ReplayResult::internal(ReplayErrorKind::BaseTreeLoad, e),
        },
    };

    let their_tree: Tree = match load_object(&commit_to_replay.tree_id) {
        Ok(t) => t,
        Err(e) => return ReplayResult::internal(ReplayErrorKind::TheirTreeLoad, e.to_string()),
    };

    let our_tree: Tree =
        match load_object::<Commit>(new_parent_id).and_then(|c| load_object(&c.tree_id)) {
            Ok(t) => t,
            Err(e) => return ReplayResult::internal(ReplayErrorKind::OurTreeLoad, e.to_string()),
        };

    // Get all items from each tree and a union of their paths.
    let (tree_items, all_paths) =
        collect_tree_items_and_paths([&base_tree, &their_tree, &our_tree]);
    let base_items = &tree_items[0];
    let their_items = &tree_items[1];
    let our_items = &tree_items[2];

    let mut merged_items: HashMap<PathBuf, RebaseTreeEntry> = HashMap::new();
    let mut conflict_items: Vec<(PathBuf, ConflictKind)> = Vec::new();
    let workdir = util::working_dir();
    let commit_abbrev = commit_to_replay_id.to_string();
    let commit_short = &commit_abbrev[..7];
    let marker_eol = conflict_marker_eol();
    let untracked_paths = match worktree::untracked_workdir_paths(&current_index) {
        Ok(paths) => paths,
        Err(e) => return ReplayResult::internal(ReplayErrorKind::IndexLoad, e.to_string()),
    };

    for path in all_paths {
        let base_entry = base_items.get(&path);
        let their_entry = their_items.get(&path);
        let our_entry = our_items.get(&path);

        match resolve_three_way(base_entry, their_entry, our_entry) {
            MergeResolution::Use(entry) => {
                merged_items.insert(path, entry);
            }
            MergeResolution::Delete => {}
            MergeResolution::Conflict(kind) => {
                conflict_items.push((path, kind));
            }
        }
    }

    let conflicts: Vec<PathBuf> = conflict_items
        .iter()
        .map(|(path, _)| path.clone())
        .collect();

    if !conflicts.is_empty() {
        let mut untracked_conflict = None;
        for untracked in &untracked_paths {
            for path in conflicts.iter().chain(merged_items.keys()) {
                if worktree::paths_conflict(untracked, path) {
                    untracked_conflict = Some(untracked.clone());
                    break;
                }
            }
            if untracked_conflict.is_some() {
                break;
            }
        }
        if let Some(conflict) = untracked_conflict {
            return ReplayResult::internal(
                ReplayErrorKind::UntrackedOverwrite,
                format!(
                    "untracked working tree file would be overwritten by rebase: {}",
                    conflict.display()
                ),
            );
        }

        for (path, kind) in &conflict_items {
            if let Err(e) = write_conflict_markers(&workdir, path, marker_eol, commit_short, *kind)
            {
                return ReplayResult::internal(ReplayErrorKind::ConflictMarker, e);
            }
        }

        // Update index with conflict entries
        let index_file = path::index();
        let mut index = git_internal::internal::index::Index::new();

        // Add non-conflicting files at stage 0
        for (path, entry) in &merged_items {
            if let Err(e) = add_rebase_index_entry(&mut index, path, *entry, 0) {
                return ReplayResult::internal(ReplayErrorKind::IndexSave, e);
            }
        }

        // Add conflicting files at stages 1, 2, 3
        for path in &conflicts {
            // Stage 1: base version
            if let Some(base_entry) = base_items.get(path)
                && let Err(e) = add_rebase_index_entry(&mut index, path, *base_entry, 1)
            {
                return ReplayResult::internal(ReplayErrorKind::IndexSave, e);
            }

            // Stage 2: ours version
            if let Some(our_entry) = our_items.get(path)
                && let Err(e) = add_rebase_index_entry(&mut index, path, *our_entry, 2)
            {
                return ReplayResult::internal(ReplayErrorKind::IndexSave, e);
            }

            // Stage 3: theirs version
            if let Some(their_entry) = their_items.get(path)
                && let Err(e) = add_rebase_index_entry(&mut index, path, *their_entry, 3)
            {
                return ReplayResult::internal(ReplayErrorKind::IndexSave, e);
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
        tracked_paths.extend(current_index.tracked_files());
        tracked_paths.extend(base_items.keys().cloned());
        tracked_paths.extend(their_items.keys().cloned());
        tracked_paths.extend(our_items.keys().cloned());

        let conflict_set: HashSet<PathBuf> = conflicts.iter().cloned().collect();

        for (path, entry) in &merged_items {
            if let Err(e) = write_rebase_workdir_entry(&workdir, path, *entry) {
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
        Err(e) => return ReplayResult::internal(ReplayErrorKind::TreeCreate, e.to_string()),
    };

    let original_empty = match original_parent_id {
        Some(parent_id) => match load_object::<Commit>(&parent_id) {
            Ok(parent) => parent.tree_id == commit_to_replay.tree_id,
            Err(e) => return ReplayResult::internal(ReplayErrorKind::CommitLoad, e.to_string()),
        },
        None => base_tree.id == commit_to_replay.tree_id,
    };
    let becomes_empty = new_tree_id == our_tree.id;
    if original_empty && !options.keep_empty {
        return ReplayResult::DroppedEmpty;
    }
    if !original_empty && becomes_empty {
        match options.empty_mode {
            EmptyMode::Drop => return ReplayResult::DroppedEmpty,
            EmptyMode::Stop => {
                return ReplayResult::internal(
                    ReplayErrorKind::CommitSave,
                    "commit became empty during rebase; use --skip or --empty=keep".to_string(),
                );
            }
            EmptyMode::Keep => {}
        }
    }

    let new_commit = match create_replayed_commit(
        &commit_to_replay,
        new_tree_id,
        *new_parent_id,
        action,
        options,
    )
    .await
    {
        Ok(commit) => commit,
        Err(error) => {
            return ReplayResult::internal(ReplayErrorKind::CommitSave, error.to_string());
        }
    };

    if let Err(e) = save_object(&new_commit, &new_commit.id) {
        return ReplayResult::internal(ReplayErrorKind::CommitSave, e.to_string());
    }

    // Update index and working directory
    let mut index = git_internal::internal::index::Index::new();
    let new_tree: Tree = match load_object(&new_tree_id) {
        Ok(tree) => tree,
        Err(e) => return ReplayResult::internal(ReplayErrorKind::NewTreeLoad, e.to_string()),
    };
    if let Err(e) = rebuild_index_from_tree(&new_tree, &mut index, "") {
        return ReplayResult::internal(ReplayErrorKind::IndexRebuild, e.to_string());
    }
    if let Err(e) = index.save(&index_file) {
        return ReplayResult::internal(ReplayErrorKind::IndexSave, e.to_string());
    }
    if let Err(e) = reset_workdir_tracked_only(&current_index, &index) {
        return ReplayResult::internal(ReplayErrorKind::WorkdirReset, e.to_string());
    }

    ReplayResult::Success(new_commit.id)
}

async fn find_merge_base(
    commit1_id: &ObjectHash,
    commit2_id: &ObjectHash,
) -> Result<Option<ObjectHash>, RebaseError> {
    merge_base::find_best_merge_base(*commit1_id, *commit2_id).map_err(|error| match error {
        merge_base::MergeBaseError::Load { commit_id, detail } => RebaseError::CommitLoad {
            commit: commit_id,
            detail,
        },
        merge_base::MergeBaseError::Ambiguous { bases } => {
            RebaseError::AmbiguousMergeBase { bases }
        }
    })
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
fn create_tree_from_items_map(
    items: &HashMap<PathBuf, RebaseTreeEntry>,
) -> Result<ObjectHash, String> {
    if items.is_empty() {
        let tree = empty_tree()?;
        save_object(&tree, &tree.id).map_err(|e| e.to_string())?;
        return Ok(tree.id);
    }

    // Group files by their parent directories
    let mut entries_map: HashMap<PathBuf, Vec<TreeItem>> = HashMap::new();
    for (path, entry) in items {
        let item = TreeItem {
            mode: entry.mode,
            name: tree_item_name(path)?,
            id: entry.hash,
        };
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
    entries_map: &mut HashMap<PathBuf, Vec<TreeItem>>,
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
        let subdir_name = tree_item_name(&subdir_path)?;

        let subtree_hash = build_tree_recursively(&subdir_path, entries_map)?;

        // Add the subdirectory as a tree item
        current_items.push(TreeItem {
            mode: TreeItemMode::Tree,
            name: subdir_name,
            id: subtree_hash,
        });
    }

    crate::utils::tree::sort_tree_items_for_git(&mut current_items);
    // Create and save the tree object for this directory
    let tree = Tree::from_tree_items(current_items).map_err(|e| e.to_string())?;
    save_object(&tree, &tree.id).map_err(|e| e.to_string())?;
    Ok(tree.id)
}

fn empty_tree() -> Result<Tree, String> {
    let empty_id = ObjectHash::from_type_and_data(ObjectType::Tree, &[]);
    Tree::from_bytes(&[], empty_id).map_err(|e| format!("failed to create empty tree: {e}"))
}

/// Reset the working directory to match the new index state without overwriting untracked files.
fn reset_workdir_tracked_only(
    current_index: &git_internal::internal::index::Index,
    new_index: &git_internal::internal::index::Index,
) -> Result<(), String> {
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
        let path_str = path_to_index_key(&path_buf)?;
        if let Some(entry) = new_index.get(path_str, 0) {
            let mode = index_mode_to_tree_item_mode(entry.mode)?;
            write_rebase_workdir_entry(
                &workdir,
                &path_buf,
                RebaseTreeEntry {
                    hash: entry.hash,
                    mode,
                },
            )?;
        }
    }

    Ok(())
}

fn tree_item_name(path: &Path) -> Result<String, String> {
    let name = path
        .file_name()
        .ok_or_else(|| format!("path has no file name: {}", path.display()))?;
    name.to_str()
        .map(str::to_string)
        .ok_or_else(|| format!("path is not valid UTF-8: {}", path.display()))
}

fn path_to_index_key(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| format!("path is not valid UTF-8: {}", path.display()))
}

fn add_rebase_index_entry(
    index: &mut git_internal::internal::index::Index,
    path: &Path,
    item: RebaseTreeEntry,
    stage: u8,
) -> Result<(), String> {
    let blob: Blob = load_object(&item.hash).map_err(|error| {
        format!(
            "failed to load blob {} for index entry '{}': {error}",
            item.hash,
            path.display()
        )
    })?;
    let mut entry = git_internal::internal::index::IndexEntry::new_from_blob(
        path_to_index_key(path)?.to_string(),
        item.hash,
        blob.data.len() as u32,
    );
    entry.mode = tree_item_mode_to_index_mode(item.mode)?;
    entry.flags.stage = stage;
    index.add(entry);
    Ok(())
}

fn tree_item_mode_to_index_mode(mode: TreeItemMode) -> Result<u32, String> {
    match mode {
        TreeItemMode::Blob => Ok(0o100644),
        TreeItemMode::BlobExecutable => Ok(0o100755),
        TreeItemMode::Link => Ok(0o120000),
        TreeItemMode::Tree => {
            Err("tree entry cannot be represented as a file index entry".to_string())
        }
        TreeItemMode::Commit => Err("gitlink entries are not supported by rebase".to_string()),
    }
}

fn index_mode_to_tree_item_mode(mode: u32) -> Result<TreeItemMode, String> {
    match mode {
        0o100644 => Ok(TreeItemMode::Blob),
        0o100755 => Ok(TreeItemMode::BlobExecutable),
        0o120000 => Ok(TreeItemMode::Link),
        other => Err(format!(
            "unsupported index mode {other:o} while creating rebase tree"
        )),
    }
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

        let index_mode = match item.mode {
            git_internal::internal::object::tree::TreeItemMode::Tree => {
                let subtree: Tree = load_object(&item.id).map_err(|e| {
                    format!(
                        "failed to load tree {} for rebase index entry '{}': {e}",
                        item.id, full_path
                    )
                })?;
                rebuild_index_from_tree(&subtree, index, &full_path)?;
                continue;
            }
            git_internal::internal::object::tree::TreeItemMode::Blob => 0o100644,
            git_internal::internal::object::tree::TreeItemMode::BlobExecutable => 0o100755,
            git_internal::internal::object::tree::TreeItemMode::Link => 0o120000,
            git_internal::internal::object::tree::TreeItemMode::Commit => {
                return Err(format!(
                    "unsupported gitlink tree entry '{}' while rebuilding rebase index",
                    full_path
                ));
            }
        };

        let blob: git_internal::internal::object::blob::Blob =
            load_object(&item.id).map_err(|e| {
                format!(
                    "failed to load blob {} for rebase index entry '{}': {e}",
                    item.id, full_path
                )
            })?;
        let mut entry = git_internal::internal::index::IndexEntry::new_from_blob(
            full_path,
            item.id,
            blob.data.len() as u32,
        );
        entry.mode = index_mode;
        index.add(entry);
    }
    Ok(())
}

#[cfg(test)]
mod rebuild_index_tests {
    use std::str::FromStr;

    use git_internal::{
        hash::ObjectHash,
        internal::{
            index::Index,
            object::{
                blob::Blob,
                tree::{Tree, TreeItem, TreeItemMode},
            },
        },
    };
    use tempfile::tempdir;

    use super::rebuild_index_from_tree;
    use crate::{
        command::save_object,
        utils::test::{ChangeDirGuard, setup_with_new_libra_in},
    };

    #[tokio::test]
    async fn rebuild_index_from_tree_preserves_executable_and_symlink_modes() {
        let repo = tempdir().unwrap();
        setup_with_new_libra_in(repo.path()).await;
        let _guard = ChangeDirGuard::new(repo.path());

        let executable_blob = Blob::from_content("run\n");
        save_object(&executable_blob, &executable_blob.id).unwrap();
        let symlink_blob = Blob::from_content("target.txt");
        save_object(&symlink_blob, &symlink_blob.id).unwrap();
        let regular_blob = Blob::from_content("plain\n");
        save_object(&regular_blob, &regular_blob.id).unwrap();

        let tree = Tree::from_tree_items(vec![
            TreeItem::new(
                TreeItemMode::BlobExecutable,
                executable_blob.id,
                "run.sh".to_string(),
            ),
            TreeItem::new(TreeItemMode::Link, symlink_blob.id, "link".to_string()),
            TreeItem::new(TreeItemMode::Blob, regular_blob.id, "plain.txt".to_string()),
        ])
        .unwrap();
        let mut index = Index::new();

        rebuild_index_from_tree(&tree, &mut index, "").unwrap();

        assert_eq!(index.get("run.sh", 0).unwrap().mode, 0o100755);
        assert_eq!(index.get("link", 0).unwrap().mode, 0o120000);
        assert_eq!(index.get("plain.txt", 0).unwrap().mode, 0o100644);
    }

    #[tokio::test]
    async fn rebuild_index_from_tree_returns_path_context_for_missing_blob() {
        let repo = tempdir().unwrap();
        setup_with_new_libra_in(repo.path()).await;
        let _guard = ChangeDirGuard::new(repo.path());

        let missing_blob =
            ObjectHash::from_str("0123456789abcdef0123456789abcdef01234567").unwrap();
        let tree = Tree::from_tree_items(vec![TreeItem::new(
            TreeItemMode::Blob,
            missing_blob,
            "missing.txt".to_string(),
        )])
        .unwrap();
        let mut index = Index::new();

        let err = rebuild_index_from_tree(&tree, &mut index, "").unwrap_err();

        assert!(err.contains("failed to load blob"));
        assert!(err.contains("missing.txt"));
    }

    #[tokio::test]
    async fn rebuild_index_from_tree_rejects_gitlink_entries() {
        let repo = tempdir().unwrap();
        setup_with_new_libra_in(repo.path()).await;
        let _guard = ChangeDirGuard::new(repo.path());

        let gitlink = ObjectHash::from_str("0123456789abcdef0123456789abcdef01234567").unwrap();
        let tree = Tree::from_tree_items(vec![TreeItem::new(
            TreeItemMode::Commit,
            gitlink,
            "vendor".to_string(),
        )])
        .unwrap();
        let mut index = Index::new();

        let err = rebuild_index_from_tree(&tree, &mut index, "").unwrap_err();

        assert!(err.contains("unsupported gitlink tree entry"));
        assert!(err.contains("vendor"));
    }
}
