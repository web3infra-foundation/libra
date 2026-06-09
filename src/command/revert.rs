//! Implements the revert command by parsing targets, reversing commit changes into the index/worktree, and optionally creating a new commit.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

use clap::{ArgGroup, Parser};
use git_internal::{
    hash::ObjectHash,
    internal::{
        index::{Index, IndexEntry},
        object::{
            ObjectTrait,
            blob::Blob,
            commit::Commit,
            tree::{Tree, TreeItem, TreeItemMode},
            types::ObjectType,
        },
    },
};
use sea_orm::{ConnectionTrait, DbBackend, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};

use crate::{
    command::{load_object, merge, save_object},
    common_utils::format_commit_msg,
    internal::{branch::Branch, db::get_db_conn_instance, head::Head},
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
    libra revert HEAD~3..HEAD             Revert a range newest-first
    libra revert -n HEAD                  Revert without auto-committing
    libra revert -m 1 <merge>             Revert a merge commit relative to parent 1
    libra revert --continue               Resume after resolving conflicts
    libra revert --abort                  Cancel an in-progress revert sequence
    libra revert --json HEAD              Structured JSON output for agents";

// ── Typed error ──────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
enum RevertError {
    #[error("not a libra repository")]
    NotInRepo,

    #[error("failed to resolve commit reference '{0}'")]
    InvalidCommit(String),

    #[error("no commits specified")]
    NoCommitSpecified,

    #[error("a revert sequence is already in progress")]
    InProgress,

    #[error("no revert sequence in progress")]
    NoRevertInProgress,

    #[error("failed to load revert sequence: {0}")]
    SequenceLoad(String),

    #[error("failed to save revert sequence: {0}")]
    SequenceSave(String),

    #[error("--no-commit is only supported for a single commit")]
    NoCommitMultiUnsupported,

    #[error("commit {0} is a merge but no -m option was given")]
    MainlineRequired(String),

    #[error("mainline was specified but commit {0} is not a merge")]
    MainlineForNonMerge(String),

    #[error("commit {commit} does not have a parent number {mainline} (it has {parents})")]
    InvalidMainline {
        commit: String,
        mainline: usize,
        parents: usize,
    },

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
            Self::InvalidCommit(_) => StableErrorCode::CliInvalidTarget,
            Self::NoCommitSpecified | Self::NoCommitMultiUnsupported => {
                StableErrorCode::CliInvalidArguments
            }
            Self::InProgress | Self::NoRevertInProgress => StableErrorCode::RepoStateInvalid,
            Self::SequenceLoad(_) => StableErrorCode::IoReadFailed,
            Self::SequenceSave(_) => StableErrorCode::IoWriteFailed,
            Self::MainlineRequired(_)
            | Self::MainlineForNonMerge(_)
            | Self::InvalidMainline { .. } => StableErrorCode::CliInvalidArguments,
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
            RevertError::InvalidCommit(_) => CliError::fatal(message)
                .with_stable_code(stable_code)
                .with_hint("use 'libra log' to find valid commit references"),
            RevertError::NoCommitSpecified | RevertError::NoCommitMultiUnsupported => {
                CliError::fatal(message)
                    .with_stable_code(stable_code)
                    .with_exit_code(128)
            }
            RevertError::InProgress => CliError::fatal(message)
                .with_stable_code(stable_code)
                .with_hint("finish it with 'libra revert --continue'/--skip, or cancel with --abort/--quit"),
            RevertError::NoRevertInProgress => CliError::fatal(message)
                .with_stable_code(stable_code)
                .with_hint("start a revert with 'libra revert <commit>' first"),
            RevertError::SequenceLoad(_) | RevertError::SequenceSave(_) => {
                CliError::fatal(message).with_stable_code(stable_code)
            }
            // Mainline usage errors mirror Git's exit 128 (the Cli category
            // would otherwise default to 129), so override explicitly.
            RevertError::MainlineRequired(_) => CliError::fatal(message)
                .with_stable_code(stable_code)
                .with_exit_code(128)
                .with_hint("pass '-m <parent-number>' (e.g. '-m 1') to revert a merge commit"),
            RevertError::MainlineForNonMerge(_) => CliError::fatal(message)
                .with_stable_code(stable_code)
                .with_exit_code(128)
                .with_hint("'-m' is only valid when reverting a merge commit"),
            RevertError::InvalidMainline { .. } => CliError::fatal(message)
                .with_stable_code(stable_code)
                .with_exit_code(128)
                .with_hint("choose a parent number within the merge commit's parent count"),
            RevertError::Conflict { .. } => CliError::failure(message)
                .with_stable_code(stable_code)
                .with_hint("resolve conflicts and 'libra add' them, then run 'libra revert --continue' (or --skip / --abort / --quit)"),
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reverted_commits: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub new_commits: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restored_head: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RevertRuntimeOptions {
    no_commit: bool,
    mainline: Option<usize>,
    signoff: bool,
    edit: bool,
    no_edit: bool,
}

impl RevertRuntimeOptions {
    fn from_args(args: &RevertArgs) -> Self {
        Self {
            no_commit: args.no_commit,
            mainline: args.mainline,
            signoff: args.signoff,
            edit: args.edit,
            no_edit: args.no_edit,
        }
    }

    fn into_args(self) -> RevertArgs {
        RevertArgs {
            commits: Vec::new(),
            no_commit: self.no_commit,
            mainline: self.mainline,
            signoff: self.signoff,
            edit: self.edit,
            no_edit: self.no_edit,
            continue_revert: false,
            skip: false,
            abort: false,
            quit: false,
        }
    }
}

const REVERT_TODO_CAP: usize = 10_000;

#[derive(Debug, Clone)]
struct RevertSequence {
    head_name: String,
    head_orig: ObjectHash,
    current_oid: ObjectHash,
    todo: VecDeque<ObjectHash>,
    opts_json: String,
}

impl RevertSequence {
    async fn ensure_table_exists<C: ConnectionTrait>(db: &C) -> Result<(), String> {
        let create = Statement::from_string(
            DbBackend::Sqlite,
            r#"
                CREATE TABLE IF NOT EXISTS `revert_sequence` (
                    `id`          INTEGER PRIMARY KEY AUTOINCREMENT,
                    `head_name`   TEXT NOT NULL,
                    `head_orig`   TEXT NOT NULL,
                    `current_oid` TEXT NOT NULL,
                    `todo`        TEXT NOT NULL,
                    `opts_json`   TEXT NOT NULL,
                    `updated_at`  TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
                );
            "#
            .to_string(),
        );
        db.execute(create)
            .await
            .map_err(|error| format!("failed to create revert_sequence table: {error}"))?;
        Ok(())
    }

    async fn is_in_progress() -> Result<bool, String> {
        let db = get_db_conn_instance().await;
        Self::has_state_in_db(&db).await
    }

    async fn has_state_in_db<C: ConnectionTrait>(db: &C) -> Result<bool, String> {
        Self::ensure_table_exists(db).await?;
        let stmt = Statement::from_string(
            DbBackend::Sqlite,
            "SELECT 1 FROM revert_sequence LIMIT 1".to_string(),
        );
        let row = db
            .query_one(stmt)
            .await
            .map_err(|error| format!("failed to query revert_sequence: {error}"))?;
        Ok(row.is_some())
    }

    async fn load() -> Result<Option<Self>, String> {
        let db = get_db_conn_instance().await;
        Self::load_with_conn(&db).await
    }

    async fn load_with_conn<C: ConnectionTrait>(db: &C) -> Result<Option<Self>, String> {
        Self::ensure_table_exists(db).await?;
        let stmt = Statement::from_string(
            DbBackend::Sqlite,
            r#"
                SELECT head_name, head_orig, current_oid, todo, opts_json
                FROM revert_sequence
                LIMIT 1
            "#
            .to_string(),
        );
        let Some(row) = db
            .query_one(stmt)
            .await
            .map_err(|error| format!("failed to load revert_sequence: {error}"))?
        else {
            return Ok(None);
        };

        let head_name: String = row
            .try_get_by_index(0)
            .map_err(|error| format!("invalid head_name: {error}"))?;
        let head_orig_raw: String = row
            .try_get_by_index(1)
            .map_err(|error| format!("invalid head_orig: {error}"))?;
        let current_oid_raw: String = row
            .try_get_by_index(2)
            .map_err(|error| format!("invalid current_oid: {error}"))?;
        let todo_raw: String = row
            .try_get_by_index(3)
            .map_err(|error| format!("invalid todo: {error}"))?;
        let opts_json: String = row
            .try_get_by_index(4)
            .map_err(|error| format!("invalid opts_json: {error}"))?;

        let head_orig = ObjectHash::from_str(head_orig_raw.trim())
            .map_err(|error| format!("invalid head_orig hash: {error}"))?;
        let current_oid = ObjectHash::from_str(current_oid_raw.trim())
            .map_err(|error| format!("invalid current_oid hash: {error}"))?;
        let todo = VecDeque::from(Self::parse_todo(&todo_raw)?);

        Ok(Some(Self {
            head_name,
            head_orig,
            current_oid,
            todo,
            opts_json,
        }))
    }

    async fn save(&self) -> Result<(), String> {
        let db = get_db_conn_instance().await;
        Self::ensure_table_exists(&db).await?;
        let txn = db
            .begin()
            .await
            .map_err(|error| format!("failed to begin revert_sequence transaction: {error}"))?;
        Self::save_with_conn(&txn, self).await?;
        txn.commit()
            .await
            .map_err(|error| format!("failed to commit revert_sequence transaction: {error}"))?;
        Ok(())
    }

    async fn save_with_conn<C: ConnectionTrait>(db: &C, state: &Self) -> Result<(), String> {
        let delete =
            Statement::from_string(DbBackend::Sqlite, "DELETE FROM revert_sequence".to_string());
        db.execute(delete)
            .await
            .map_err(|error| format!("failed to clear revert_sequence: {error}"))?;

        let todo = Self::format_todo(state.todo.iter().copied());
        let insert = Statement::from_sql_and_values(
            DbBackend::Sqlite,
            r#"
                INSERT INTO revert_sequence
                (head_name, head_orig, current_oid, todo, opts_json)
                VALUES (?, ?, ?, ?, ?);
            "#,
            [
                state.head_name.clone().into(),
                state.head_orig.to_string().into(),
                state.current_oid.to_string().into(),
                todo.into(),
                state.opts_json.clone().into(),
            ],
        );
        db.execute(insert)
            .await
            .map_err(|error| format!("failed to save revert_sequence: {error}"))?;
        Ok(())
    }

    async fn clear() -> Result<(), String> {
        let db = get_db_conn_instance().await;
        Self::ensure_table_exists(&db).await?;
        Self::clear_with_conn(&db).await
    }

    async fn clear_with_conn<C: ConnectionTrait>(db: &C) -> Result<(), String> {
        let stmt =
            Statement::from_string(DbBackend::Sqlite, "DELETE FROM revert_sequence".to_string());
        db.execute(stmt)
            .await
            .map_err(|error| format!("failed to clear revert_sequence: {error}"))?;
        Ok(())
    }

    fn format_todo(items: impl Iterator<Item = ObjectHash>) -> String {
        items
            .map(|oid| oid.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn parse_todo(raw: &str) -> Result<Vec<ObjectHash>, String> {
        let mut out = Vec::new();
        for line in raw.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if out.len() >= REVERT_TODO_CAP {
                return Err(format!(
                    "revert_sequence todo exceeds {REVERT_TODO_CAP} entries"
                ));
            }
            let oid = ObjectHash::from_str(trimmed)
                .map_err(|error| format!("invalid todo OID '{trimmed}': {error}"))?;
            out.push(oid);
        }
        Ok(out)
    }
}

// ── Entry points ─────────────────────────────────────────────────────

/// Arguments for the revert command.
/// Reverts the specified commit by creating a new commit that undoes the changes.
#[derive(Parser, Debug)]
#[command(about = "Revert some existing commits")]
#[command(after_help = REVERT_EXAMPLES)]
#[command(group(
    ArgGroup::new("sequence_action")
        .multiple(false)
        .args(["continue_revert", "skip", "abort", "quit"]),
))]
pub struct RevertArgs {
    /// Commit(s) or ranges to revert (can be commit hash, branch name, HEAD, or A..B)
    #[clap(
        value_name = "commit",
        required_unless_present_any = ["continue_revert", "skip", "abort", "quit"],
        conflicts_with_all = ["continue_revert", "skip", "abort", "quit"]
    )]
    pub commits: Vec<String>,

    /// Don't automatically commit the revert, just stage the changes
    #[clap(short = 'n', long, conflicts_with = "sequence_action")]
    pub no_commit: bool,

    /// Parent number (1-based) to treat as the mainline when reverting a merge
    /// commit. Required for merge commits; rejected for non-merge commits.
    #[clap(
        short = 'm',
        long,
        value_name = "parent-number",
        conflicts_with = "sequence_action"
    )]
    pub mainline: Option<usize>,

    /// Add a Signed-off-by trailer to generated revert commits.
    #[clap(short = 's', long = "signoff", conflicts_with = "sequence_action")]
    pub signoff: bool,

    /// Edit the generated commit message before committing (accepted; editor integration deferred).
    #[clap(short = 'e', long = "edit", conflicts_with_all = ["no_edit", "sequence_action"])]
    pub edit: bool,

    /// Use the generated commit message without opening an editor.
    #[clap(long = "no-edit", conflicts_with_all = ["edit", "sequence_action"])]
    pub no_edit: bool,

    /// Continue an in-progress revert after resolving conflicts.
    #[clap(
        long = "continue",
        conflicts_with_all = ["commits", "skip", "abort", "quit", "no_commit"]
    )]
    pub continue_revert: bool,

    /// Skip the current conflicted commit and continue the sequence.
    #[clap(long, conflicts_with_all = ["commits", "continue_revert", "abort", "quit", "no_commit"])]
    pub skip: bool,

    /// Abort the in-progress revert sequence and reset to the original HEAD.
    #[clap(long, conflicts_with_all = ["commits", "continue_revert", "skip", "quit", "no_commit"])]
    pub abort: bool,

    /// Clear the in-progress revert state while keeping index/worktree changes.
    #[clap(long, conflicts_with_all = ["commits", "continue_revert", "skip", "abort", "no_commit"])]
    pub quit: bool,
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
    let result = run_revert(args, output).await.map_err(CliError::from)?;
    render_revert_output(&result, output)
}

// ── Core execution ───────────────────────────────────────────────────

async fn run_revert(args: RevertArgs, output: &OutputConfig) -> Result<RevertOutput, RevertError> {
    util::require_repo().map_err(|_| RevertError::NotInRepo)?;

    if args.continue_revert {
        return run_revert_continue().await;
    }
    if args.skip {
        return run_revert_skip(output).await;
    }
    if args.abort {
        return run_revert_abort(output).await;
    }
    if args.quit {
        return run_revert_quit().await;
    }

    if RevertSequence::is_in_progress()
        .await
        .map_err(RevertError::SequenceLoad)?
    {
        return Err(RevertError::InProgress);
    }

    let commit_ids = resolve_revert_commits(&args.commits).await?;
    if commit_ids.is_empty() {
        return Err(RevertError::NoCommitSpecified);
    }
    if args.no_commit && commit_ids.len() > 1 {
        return Err(RevertError::NoCommitMultiUnsupported);
    }

    let head_orig = Head::current_commit()
        .await
        .ok_or_else(|| RevertError::LoadObject("failed to resolve current HEAD".into()))?;
    let head_name = current_head_name_for_state().await;
    let opts_json =
        serde_json::to_string(&RevertRuntimeOptions::from_args(&args)).map_err(|error| {
            RevertError::SequenceSave(format!("failed to serialize options: {error}"))
        })?;

    let mut reverted = Vec::new();
    let mut new_commits = Vec::new();
    let mut files_changed = 0usize;

    for (index, commit_id) in commit_ids.iter().enumerate() {
        match revert_single_commit(commit_id, &args).await {
            Ok((new_commit_id, changed)) => {
                reverted.push(commit_id.to_string());
                if let Some(id) = new_commit_id {
                    new_commits.push(id.to_string());
                }
                files_changed += changed;
            }
            Err(error @ RevertError::Conflict { .. }) if !args.no_commit => {
                let state = RevertSequence {
                    head_name,
                    head_orig,
                    current_oid: *commit_id,
                    todo: commit_ids[index + 1..].iter().copied().collect(),
                    opts_json,
                };
                state.save().await.map_err(RevertError::SequenceSave)?;
                return Err(error);
            }
            Err(error) => return Err(error),
        }
    }

    let first_reverted = reverted.first().cloned().unwrap_or_default();
    let first_new = new_commits.first().cloned();

    Ok(RevertOutput {
        reverted_commit: first_reverted.clone(),
        short_reverted: short_display_hash(&first_reverted).to_string(),
        new_commit: first_new.clone(),
        short_new: first_new
            .as_ref()
            .map(|id| short_display_hash(id).to_string()),
        no_commit: args.no_commit,
        files_changed,
        reverted_commits: reverted,
        new_commits,
        action: None,
        restored_head: None,
    })
}

async fn run_revert_continue() -> Result<RevertOutput, RevertError> {
    let state = load_revert_state_or_err().await?;
    let index = Index::load(path::index())
        .map_err(|error| RevertError::LoadObject(format!("failed to load index: {error}")))?;
    let unresolved = merge::unresolved_conflicted_paths(&index, &[]);
    if !unresolved.is_empty() {
        return Err(RevertError::Conflict {
            path: unresolved.join(", "),
        });
    }

    let opts: RevertRuntimeOptions = serde_json::from_str(&state.opts_json)
        .map_err(|error| RevertError::SequenceLoad(format!("failed to parse options: {error}")))?;
    let args = opts.into_args();
    let parent = Head::current_commit()
        .await
        .ok_or_else(|| RevertError::LoadObject("failed to resolve current HEAD".into()))?;
    let tree_id = create_tree_from_index(&index)?;
    let new_commit = create_revert_commit(&state.current_oid, &parent, &tree_id, &args).await?;

    let mut reverted = vec![state.current_oid.to_string()];
    let mut new_commits = vec![new_commit.to_string()];
    resume_reverts(
        state.head_name,
        state.head_orig,
        state.todo,
        args,
        state.opts_json,
        &mut reverted,
        &mut new_commits,
    )
    .await?;

    make_sequence_output("continue", reverted, new_commits, None, false)
}

async fn run_revert_skip(output: &OutputConfig) -> Result<RevertOutput, RevertError> {
    let state = load_revert_state_or_err().await?;
    reset_hard("HEAD", output).await?;
    let opts: RevertRuntimeOptions = serde_json::from_str(&state.opts_json)
        .map_err(|error| RevertError::SequenceLoad(format!("failed to parse options: {error}")))?;
    let args = opts.into_args();
    let mut reverted = Vec::new();
    let mut new_commits = Vec::new();
    resume_reverts(
        state.head_name,
        state.head_orig,
        state.todo,
        args,
        state.opts_json,
        &mut reverted,
        &mut new_commits,
    )
    .await?;
    make_sequence_output("skip", reverted, new_commits, None, false)
}

async fn run_revert_abort(output: &OutputConfig) -> Result<RevertOutput, RevertError> {
    let state = load_revert_state_or_err().await?;
    let restored = state.head_orig.to_string();
    reset_hard(&restored, output).await?;
    RevertSequence::clear()
        .await
        .map_err(RevertError::SequenceSave)?;
    make_sequence_output("abort", Vec::new(), Vec::new(), Some(restored), false)
}

async fn run_revert_quit() -> Result<RevertOutput, RevertError> {
    load_revert_state_or_err().await?;
    RevertSequence::clear()
        .await
        .map_err(RevertError::SequenceSave)?;
    make_sequence_output("quit", Vec::new(), Vec::new(), None, false)
}

async fn resume_reverts(
    head_name: String,
    head_orig: ObjectHash,
    mut todo: VecDeque<ObjectHash>,
    args: RevertArgs,
    opts_json: String,
    reverted: &mut Vec<String>,
    new_commits: &mut Vec<String>,
) -> Result<(), RevertError> {
    while let Some(commit_id) = todo.pop_front() {
        let pending = RevertSequence {
            head_name: head_name.clone(),
            head_orig,
            current_oid: commit_id,
            todo: todo.clone(),
            opts_json: opts_json.clone(),
        };
        pending.save().await.map_err(RevertError::SequenceSave)?;

        match revert_single_commit(&commit_id, &args).await {
            Ok((new_commit_id, _)) => {
                reverted.push(commit_id.to_string());
                if let Some(id) = new_commit_id {
                    new_commits.push(id.to_string());
                }
            }
            Err(error @ RevertError::Conflict { .. }) => return Err(error),
            Err(error) => return Err(error),
        }
    }
    RevertSequence::clear()
        .await
        .map_err(RevertError::SequenceSave)?;
    Ok(())
}

fn make_sequence_output(
    action: &str,
    reverted: Vec<String>,
    new_commits: Vec<String>,
    restored_head: Option<String>,
    no_commit: bool,
) -> Result<RevertOutput, RevertError> {
    let first_reverted = reverted.first().cloned().unwrap_or_default();
    let first_new = new_commits.first().cloned();
    Ok(RevertOutput {
        reverted_commit: first_reverted.clone(),
        short_reverted: short_display_hash(&first_reverted).to_string(),
        new_commit: first_new.clone(),
        short_new: first_new
            .as_ref()
            .map(|id| short_display_hash(id).to_string()),
        no_commit,
        files_changed: 0,
        reverted_commits: reverted,
        new_commits,
        action: Some(action.to_string()),
        restored_head,
    })
}

async fn load_revert_state_or_err() -> Result<RevertSequence, RevertError> {
    RevertSequence::load()
        .await
        .map_err(RevertError::SequenceLoad)?
        .ok_or(RevertError::NoRevertInProgress)
}

async fn current_head_name_for_state() -> String {
    match Head::current().await {
        Head::Branch(name) => name,
        Head::Detached(hash) => format!("DETACHED:{hash}"),
    }
}

fn silent_child_output(output: &OutputConfig) -> OutputConfig {
    let mut child = output.child_output_config();
    child.quiet = true;
    child
}

async fn reset_hard(target: &str, output: &OutputConfig) -> Result<(), RevertError> {
    let child = silent_child_output(output);
    crate::command::reset::execute_safe(
        crate::command::reset::ResetArgs {
            target: target.to_string(),
            soft: false,
            mixed: false,
            hard: true,
            merge: false,
            keep: false,
            pathspecs: Vec::new(),
            pathspec_from_file: None,
            pathspec_file_nul: false,
            no_refresh: false,
        },
        &child,
    )
    .await
    .map_err(|error| {
        RevertError::WriteWorktree(format!(
            "failed to reset to '{target}': {}",
            error.message()
        ))
    })
}

async fn resolve_revert_commits(specs: &[String]) -> Result<Vec<ObjectHash>, RevertError> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for spec in specs {
        let resolved = if let Some((left, right)) = spec.split_once("..") {
            if spec.contains("...") {
                return Err(RevertError::InvalidCommit(spec.clone()));
            }
            resolve_revert_range(left, right, spec).await?
        } else {
            vec![
                resolve_commit(spec)
                    .await
                    .map_err(|_| RevertError::InvalidCommit(spec.clone()))?,
            ]
        };
        for id in resolved {
            if seen.insert(id) {
                out.push(id);
            }
        }
    }
    Ok(out)
}

async fn resolve_revert_range(
    left: &str,
    right: &str,
    original: &str,
) -> Result<Vec<ObjectHash>, RevertError> {
    let left_spec = if left.is_empty() { "HEAD" } else { left };
    let right_spec = if right.is_empty() { "HEAD" } else { right };
    let left_id = resolve_commit(left_spec)
        .await
        .map_err(|_| RevertError::InvalidCommit(original.to_string()))?;
    let right_id = resolve_commit(right_spec)
        .await
        .map_err(|_| RevertError::InvalidCommit(original.to_string()))?;

    let excluded_commits = crate::utils::graph::CommitWalker::new(&[left_id], HashSet::new())
        .map_err(|error| RevertError::LoadObject(error.message().to_string()))?
        .collect()
        .map_err(|error| RevertError::LoadObject(error.message().to_string()))?;
    let excluded: HashSet<ObjectHash> = excluded_commits
        .into_iter()
        .map(|commit| commit.id)
        .collect();

    let commits = crate::utils::graph::CommitWalker::new(&[right_id], excluded)
        .map_err(|error| RevertError::LoadObject(error.message().to_string()))?
        .collect()
        .map_err(|error| RevertError::LoadObject(error.message().to_string()))?;
    Ok(commits.into_iter().map(|commit| commit.id).collect())
}

// ── Rendering ────────────────────────────────────────────────────────

fn render_revert_output(result: &RevertOutput, output: &OutputConfig) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("revert", result, output);
    }

    if output.quiet {
        return Ok(());
    }

    match result.action.as_deref() {
        Some("abort") => {
            if let Some(head) = &result.restored_head {
                println!("revert aborted; HEAD reset to {}", short_display_hash(head));
            } else {
                println!("revert aborted");
            }
            return Ok(());
        }
        Some("quit") => {
            println!("revert state cleared; working tree left unchanged");
            return Ok(());
        }
        Some("skip") => {
            println!("revert skipped current commit");
            return Ok(());
        }
        Some("continue") => {
            println!("revert sequence continued");
            return Ok(());
        }
        Some(_) | None => {}
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

    // Select the baseline parent to diff against. A merge commit (>1 parent)
    // requires `-m <n>` to pick the mainline; a non-merge commit rejects `-m`.
    // The generated revert still records a single parent (the current HEAD).
    let parents = &reverted_commit.parent_commit_ids;
    let parent_commit_id = match (parents.len(), args.mainline) {
        (0, None) => return revert_root_commit(args).await,
        (0, Some(_)) | (1, Some(_)) => {
            return Err(RevertError::MainlineForNonMerge(commit_id.to_string()));
        }
        (1, None) => parents[0],
        (_, None) => return Err(RevertError::MainlineRequired(commit_id.to_string())),
        (count, Some(mainline)) => {
            if mainline < 1 || mainline > count {
                return Err(RevertError::InvalidMainline {
                    commit: commit_id.to_string(),
                    mainline,
                    parents: count,
                });
            }
            parents[mainline - 1]
        }
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
            write_revert_conflict(
                path,
                Some(reverted_hash),
                current_files.get(path).copied(),
                parent_hash.copied(),
                commit_id,
            )?;
            return Err(RevertError::Conflict {
                path: path.display().to_string(),
            });
        }

        if let Some(parent_hash) = parent_hash {
            if current_files.insert(path.clone(), *parent_hash) != Some(*parent_hash) {
                files_changed += 1;
            }
        } else if current_files.remove(path).is_some() {
            files_changed += 1;
        }
    }

    for (path, &parent_hash) in &parent_files {
        if !reverted_files.contains_key(path)
            && current_files.insert(path.clone(), parent_hash) != Some(parent_hash)
        {
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
            create_revert_commit(commit_id, &current_head_commit_id, &final_tree_id, args).await?;
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
        crate::utils::tree::sort_tree_items_for_git(&mut tree_items);
        let tree = tree_from_items_or_empty(tree_items)?;
        save_object(&tree, &tree.id).map_err(|e| RevertError::SaveObject(e.to_string()))?;
        Ok(tree)
    }

    let root_dir = PathBuf::new();
    let root_tree = build_subtree(&files, &root_dir)?;
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
        let revert_commit_id = create_empty_revert_commit(&current_head, args).await?;
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

fn write_revert_conflict(
    conflict_path: &Path,
    base: Option<ObjectHash>,
    ours: Option<ObjectHash>,
    theirs: Option<ObjectHash>,
    commit_id: &ObjectHash,
) -> Result<(), RevertError> {
    let mut index = Index::load(path::index()).unwrap_or_else(|_| Index::new());
    remove_index_entry_all_stages(&mut index, path_to_utf8(conflict_path)?);
    if let Some(hash) = base {
        add_conflict_index_entry(&mut index, conflict_path, hash, 1)?;
    }
    if let Some(hash) = ours {
        add_conflict_index_entry(&mut index, conflict_path, hash, 2)?;
    }
    if let Some(hash) = theirs {
        add_conflict_index_entry(&mut index, conflict_path, hash, 3)?;
    }
    index
        .save(path::index())
        .map_err(|error| RevertError::IndexSave(error.to_string()))?;

    let base_blob = load_optional_blob(base)?;
    let ours_blob = load_optional_blob(ours)?;
    let theirs_blob = load_optional_blob(theirs)?;
    let short = short_display_hash(&commit_id.to_string()).to_string();
    let marker = merge::render_conflict_marker_content(
        if cfg!(windows) { "\r\n" } else { "\n" },
        &short,
        base_blob.as_ref().map(|blob| blob.data.as_slice()),
        ours_blob.as_ref().map(|blob| blob.data.as_slice()),
        theirs_blob.as_ref().map(|blob| blob.data.as_slice()),
        merge::MergeConflictStyle::Merge,
    );
    let target = util::working_dir().join(conflict_path);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            RevertError::WriteWorktree(format!(
                "failed to create directory '{}': {error}",
                parent.display()
            ))
        })?;
    }
    fs::write(&target, marker.as_bytes()).map_err(|error| {
        RevertError::WriteWorktree(format!("failed to write '{}': {error}", target.display()))
    })
}

fn remove_index_entry_all_stages(index: &mut Index, path: &str) {
    for stage in 0..=3 {
        let _ = index.remove(path, stage);
    }
}

fn add_conflict_index_entry(
    index: &mut Index,
    conflict_path: &Path,
    hash: ObjectHash,
    stage: u8,
) -> Result<(), RevertError> {
    let blob: Blob =
        load_object(&hash).map_err(|error| RevertError::LoadObject(error.to_string()))?;
    let mut entry = IndexEntry::new_from_blob(
        path_to_utf8(conflict_path)?.to_string(),
        hash,
        blob.data.len() as u32,
    );
    entry.flags.stage = stage;
    index.add(entry);
    Ok(())
}

fn load_optional_blob(hash: Option<ObjectHash>) -> Result<Option<Blob>, RevertError> {
    hash.map(|hash| load_object(&hash).map_err(|error| RevertError::LoadObject(error.to_string())))
        .transpose()
}

fn create_tree_from_index(index: &Index) -> Result<ObjectHash, RevertError> {
    let mut entries_map: HashMap<PathBuf, Vec<TreeItem>> = HashMap::new();
    for path_buf in index.tracked_files() {
        let path_str = path_to_utf8(&path_buf)?;
        if let Some(entry) = index.get(path_str, 0) {
            let item = TreeItem {
                mode: index_mode_to_tree_item_mode(entry.mode)?,
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
    build_tree_recursively_from_entries(Path::new(""), &mut entries_map)
}

fn build_tree_recursively_from_entries(
    current_path: &Path,
    entries_map: &mut HashMap<PathBuf, Vec<TreeItem>>,
) -> Result<ObjectHash, RevertError> {
    let mut current_items = entries_map.remove(current_path).unwrap_or_default();
    let subdirs: Vec<_> = entries_map
        .keys()
        .filter(|path| path.parent() == Some(current_path))
        .cloned()
        .collect();
    for subdir in subdirs {
        let subtree_id = build_tree_recursively_from_entries(&subdir, entries_map)?;
        current_items.push(TreeItem {
            mode: TreeItemMode::Tree,
            name: file_name_to_utf8(&subdir)?,
            id: subtree_id,
        });
    }
    crate::utils::tree::sort_tree_items_for_git(&mut current_items);
    let tree = tree_from_items_or_empty(current_items)?;
    save_object(&tree, &tree.id).map_err(|error| RevertError::SaveObject(error.to_string()))?;
    Ok(tree.id)
}

fn tree_from_items_or_empty(tree_items: Vec<TreeItem>) -> Result<Tree, RevertError> {
    if tree_items.is_empty() {
        return empty_tree();
    }
    Tree::from_tree_items(tree_items).map_err(|error| RevertError::SaveObject(error.to_string()))
}

fn empty_tree() -> Result<Tree, RevertError> {
    let empty_id = ObjectHash::from_type_and_data(ObjectType::Tree, &[]);
    Tree::from_bytes(&[], empty_id)
        .map_err(|error| RevertError::SaveObject(format!("failed to create empty tree: {error}")))
}

fn index_mode_to_tree_item_mode(mode: u32) -> Result<TreeItemMode, RevertError> {
    match mode {
        0o100644 => Ok(TreeItemMode::Blob),
        0o100755 => Ok(TreeItemMode::BlobExecutable),
        0o120000 => Ok(TreeItemMode::Link),
        0o040000 => Ok(TreeItemMode::Tree),
        _ => Err(RevertError::LoadObject(format!(
            "unsupported index mode: {mode:#o}"
        ))),
    }
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
    args: &RevertArgs,
) -> Result<ObjectHash, RevertError> {
    let reverted_commit: Commit =
        load_object(reverted_commit_id).map_err(|e| RevertError::LoadObject(e.to_string()))?;

    let revert_message = with_optional_signoff(
        format!(
            "Revert \"{}\"\n\nThis reverts commit {}.",
            reverted_commit.message.lines().next().unwrap_or(""),
            reverted_commit_id
        ),
        args,
    )
    .await;

    let commit = Commit::from_tree_id(
        *tree_id,
        vec![*parent_id],
        &format_commit_msg(&revert_message, None),
    );

    save_object(&commit, &commit.id).map_err(|e| RevertError::SaveObject(e.to_string()))?;
    update_head(&commit.id.to_string()).await?;
    Ok(commit.id)
}

async fn create_empty_revert_commit(
    parent_id: &ObjectHash,
    args: &RevertArgs,
) -> Result<ObjectHash, RevertError> {
    let empty_tree = empty_tree()?;
    save_object(&empty_tree, &empty_tree.id).map_err(|e| RevertError::SaveObject(e.to_string()))?;

    let revert_message = with_optional_signoff(
        "Revert root commit\n\nThis reverts the initial commit.".to_string(),
        args,
    )
    .await;
    let commit = Commit::from_tree_id(
        empty_tree.id,
        vec![*parent_id],
        &format_commit_msg(&revert_message, None),
    );

    save_object(&commit, &commit.id).map_err(|e| RevertError::SaveObject(e.to_string()))?;
    update_head(&commit.id.to_string()).await?;
    Ok(commit.id)
}

async fn resolve_commit(reference: &str) -> Result<ObjectHash, String> {
    util::get_commit_base(reference).await
}

async fn update_head(commit_id: &str) -> Result<(), RevertError> {
    match Head::current().await {
        Head::Branch(name) => {
            Branch::update_branch(&name, commit_id, None)
                .await
                .map_err(|e| RevertError::UpdateHead(e.to_string()))?;
        }
        Head::Detached(_) => {
            let oid = ObjectHash::from_str(commit_id)
                .map_err(|error| RevertError::UpdateHead(error.to_string()))?;
            Head::update_result(Head::Detached(oid), None)
                .await
                .map_err(|error| RevertError::UpdateHead(error.to_string()))?;
        }
    }
    Ok(())
}

async fn with_optional_signoff(mut message: String, args: &RevertArgs) -> String {
    if args.signoff {
        let (_, committer) = util::create_signatures().await;
        message.push_str(&format!(
            "\n\nSigned-off-by: {} <{}>",
            committer.name, committer.email
        ));
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pin the `Display` format for every variant of [`RevertError`].
    /// The strings are surfaced as the `CliError` message via
    /// `From<RevertError> for CliError` and appear in both the human
    /// and `--json` envelopes for `libra revert`. Variants that wrap a
    /// `{0}` `String` use an "ignored" payload — every template ends
    /// with the bare interpolation, so the surface prefix is enough
    /// to lock the contract.
    #[test]
    fn revert_error_display_pins_each_variant() {
        assert_eq!(RevertError::NotInRepo.to_string(), "not a libra repository",);
        assert_eq!(
            RevertError::NoCommitSpecified.to_string(),
            "no commits specified",
        );
        assert_eq!(
            RevertError::InProgress.to_string(),
            "a revert sequence is already in progress",
        );
        assert_eq!(
            RevertError::NoRevertInProgress.to_string(),
            "no revert sequence in progress",
        );
        assert_eq!(
            RevertError::SequenceLoad("ignored".to_string()).to_string(),
            "failed to load revert sequence: ignored",
        );
        assert_eq!(
            RevertError::SequenceSave("ignored".to_string()).to_string(),
            "failed to save revert sequence: ignored",
        );
        assert_eq!(
            RevertError::NoCommitMultiUnsupported.to_string(),
            "--no-commit is only supported for a single commit",
        );
        assert_eq!(
            RevertError::InvalidCommit("deadbeef".to_string()).to_string(),
            "failed to resolve commit reference 'deadbeef'",
        );
        assert_eq!(
            RevertError::MainlineRequired("deadbeef".to_string()).to_string(),
            "commit deadbeef is a merge but no -m option was given",
        );
        assert_eq!(
            RevertError::MainlineForNonMerge("deadbeef".to_string()).to_string(),
            "mainline was specified but commit deadbeef is not a merge",
        );
        assert_eq!(
            RevertError::InvalidMainline {
                commit: "deadbeef".to_string(),
                mainline: 3,
                parents: 2,
            }
            .to_string(),
            "commit deadbeef does not have a parent number 3 (it has 2)",
        );
        assert_eq!(
            RevertError::Conflict {
                path: "src/main.rs".to_string(),
            }
            .to_string(),
            "conflict: file 'src/main.rs' was modified in a later commit",
        );
        assert_eq!(
            RevertError::LoadObject("ignored".to_string()).to_string(),
            "failed to load object: ignored",
        );
        assert_eq!(
            RevertError::SaveObject("ignored".to_string()).to_string(),
            "failed to save object: ignored",
        );
        assert_eq!(
            RevertError::WriteWorktree("ignored".to_string()).to_string(),
            "failed to write worktree: ignored",
        );
        assert_eq!(
            RevertError::IndexSave("ignored".to_string()).to_string(),
            "failed to save index: ignored",
        );
        assert_eq!(
            RevertError::UpdateHead("ignored".to_string()).to_string(),
            "failed to update HEAD: ignored",
        );
    }

    /// Pin the `stable_code()` mapping for every variant of
    /// [`RevertError`]. The [`StableErrorCode`] is what `--json`
    /// consumers read from the error envelope and branch on
    /// (e.g. `IoWriteFailed` is the retry-on-disk-failure code).
    /// Enumerate every variant explicitly so a future refactor that
    /// reroutes any variant — for example flipping `IndexSave` from
    /// `IoWriteFailed` to `IoReadFailed` — trips this guard rather
    /// than silently changing the wire surface.
    #[test]
    fn revert_error_stable_code_pins_each_variant() {
        assert_eq!(
            RevertError::NotInRepo.stable_code(),
            StableErrorCode::RepoNotFound,
        );
        assert_eq!(
            RevertError::NoCommitSpecified.stable_code(),
            StableErrorCode::CliInvalidArguments,
        );
        assert_eq!(
            RevertError::InProgress.stable_code(),
            StableErrorCode::RepoStateInvalid,
        );
        assert_eq!(
            RevertError::NoRevertInProgress.stable_code(),
            StableErrorCode::RepoStateInvalid,
        );
        assert_eq!(
            RevertError::SequenceLoad("x".to_string()).stable_code(),
            StableErrorCode::IoReadFailed,
        );
        assert_eq!(
            RevertError::SequenceSave("x".to_string()).stable_code(),
            StableErrorCode::IoWriteFailed,
        );
        assert_eq!(
            RevertError::NoCommitMultiUnsupported.stable_code(),
            StableErrorCode::CliInvalidArguments,
        );
        assert_eq!(
            RevertError::InvalidCommit("deadbeef".to_string()).stable_code(),
            StableErrorCode::CliInvalidTarget,
        );
        assert_eq!(
            RevertError::MainlineRequired("x".to_string()).stable_code(),
            StableErrorCode::CliInvalidArguments,
        );
        assert_eq!(
            RevertError::MainlineForNonMerge("x".to_string()).stable_code(),
            StableErrorCode::CliInvalidArguments,
        );
        assert_eq!(
            RevertError::InvalidMainline {
                commit: "x".to_string(),
                mainline: 3,
                parents: 2,
            }
            .stable_code(),
            StableErrorCode::CliInvalidArguments,
        );
        assert_eq!(
            RevertError::Conflict {
                path: "ignored".to_string(),
            }
            .stable_code(),
            StableErrorCode::ConflictUnresolved,
        );
        assert_eq!(
            RevertError::LoadObject("ignored".to_string()).stable_code(),
            StableErrorCode::IoReadFailed,
        );
        assert_eq!(
            RevertError::SaveObject("ignored".to_string()).stable_code(),
            StableErrorCode::IoWriteFailed,
        );
        assert_eq!(
            RevertError::WriteWorktree("ignored".to_string()).stable_code(),
            StableErrorCode::IoWriteFailed,
        );
        assert_eq!(
            RevertError::IndexSave("ignored".to_string()).stable_code(),
            StableErrorCode::IoWriteFailed,
        );
        assert_eq!(
            RevertError::UpdateHead("ignored".to_string()).stable_code(),
            StableErrorCode::IoWriteFailed,
        );
    }
}
