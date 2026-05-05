//! `libra agent checkpoint …` subcommands. V1 ships read-only `list` /
//! `show`; the mutating `rewind` path stays a phase-2 dry-run stub.

use std::str::FromStr;

use git_internal::{
    hash::ObjectHash,
    internal::object::{commit::Commit, tree::Tree},
};
use sea_orm::{ConnectionTrait, Statement};
use serde::Serialize;

use super::{CheckpointListArgs, CheckpointRewindArgs, CheckpointShowArgs, CheckpointSubcommand};
use crate::{
    command::load_object,
    internal::db::get_db_conn_instance,
    utils::{
        error::{CliError, CliResult},
        object::read_git_object,
        object_ext::TreeExt,
        output::{OutputConfig, emit_json_data},
        util,
    },
};

pub async fn execute_safe(cmd: CheckpointSubcommand, output: &OutputConfig) -> CliResult<()> {
    match cmd {
        CheckpointSubcommand::List(args) => list(args, output).await,
        CheckpointSubcommand::Show(args) => show(args, output).await,
        CheckpointSubcommand::Rewind(args) => rewind(args, output).await,
    }
}

#[derive(Debug, Serialize)]
struct CheckpointRow {
    checkpoint_id: String,
    session_id: String,
    scope: String,
    /// Nullable in the schema since the `2026050501` follow-up — stays
    /// `Option<String>` end-to-end so JSON consumers can distinguish a
    /// missing parent from an empty string.
    parent_commit: Option<String>,
    tree_oid: String,
    metadata_blob_oid: String,
    traces_commit: String,
    created_at: i64,
}

async fn list(args: CheckpointListArgs, output: &OutputConfig) -> CliResult<()> {
    let conn = get_db_conn_instance().await;
    if !table_exists(&conn, "agent_checkpoint").await? {
        return emit_list(&[], output);
    }
    let backend = conn.get_database_backend();

    let mut sql = String::from(
        "SELECT checkpoint_id, session_id, scope, parent_commit, tree_oid, \
                metadata_blob_oid, traces_commit, created_at \
         FROM agent_checkpoint WHERE 1=1",
    );
    let mut values: Vec<sea_orm::Value> = Vec::new();
    if let Some(session) = &args.session {
        sql.push_str(" AND session_id = ?");
        values.push(session.clone().into());
    }
    sql.push_str(" ORDER BY created_at DESC LIMIT 500");

    let rows = conn
        .query_all(Statement::from_sql_and_values(backend, &sql, values))
        .await
        .map_err(|e| CliError::fatal(format!("failed to query agent_checkpoint: {e}")))?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(CheckpointRow {
            checkpoint_id: row.try_get_by("checkpoint_id").unwrap_or_default(),
            session_id: row.try_get_by("session_id").unwrap_or_default(),
            scope: row.try_get_by("scope").unwrap_or_default(),
            parent_commit: row.try_get_by("parent_commit").ok().flatten(),
            tree_oid: row.try_get_by("tree_oid").unwrap_or_default(),
            metadata_blob_oid: row.try_get_by("metadata_blob_oid").unwrap_or_default(),
            traces_commit: row.try_get_by("traces_commit").unwrap_or_default(),
            created_at: row.try_get_by("created_at").unwrap_or_default(),
        });
    }
    emit_list(&out, output)
}

async fn show(args: CheckpointShowArgs, output: &OutputConfig) -> CliResult<()> {
    let conn = get_db_conn_instance().await;
    if !table_exists(&conn, "agent_checkpoint").await? {
        return Err(CliError::fatal(format!(
            "no checkpoint matches '{}': agent_checkpoint table not yet present (run `libra init`?)",
            args.checkpoint_id
        )));
    }
    let backend = conn.get_database_backend();
    let row = conn
        .query_one(Statement::from_sql_and_values(
            backend,
            "SELECT checkpoint_id, session_id, scope, parent_commit, tree_oid, \
                    metadata_blob_oid, traces_commit, created_at \
             FROM agent_checkpoint WHERE checkpoint_id = ? LIMIT 1",
            [args.checkpoint_id.clone().into()],
        ))
        .await
        .map_err(|e| CliError::fatal(format!("failed to query agent_checkpoint: {e}")))?;
    match row {
        Some(row) => {
            let payload = CheckpointRow {
                checkpoint_id: row.try_get_by("checkpoint_id").unwrap_or_default(),
                session_id: row.try_get_by("session_id").unwrap_or_default(),
                scope: row.try_get_by("scope").unwrap_or_default(),
                parent_commit: row.try_get_by("parent_commit").ok().flatten(),
                tree_oid: row.try_get_by("tree_oid").unwrap_or_default(),
                metadata_blob_oid: row.try_get_by("metadata_blob_oid").unwrap_or_default(),
                traces_commit: row.try_get_by("traces_commit").unwrap_or_default(),
                created_at: row.try_get_by("created_at").unwrap_or_default(),
            };
            // Best-effort metadata blob load: if the user is in a libra
            // workspace, read the metadata.json blob and surface it; if
            // path resolution fails (e.g. running from outside any libra
            // repo), fall back to the row-only render rather than erroring.
            let metadata = load_metadata_blob(&payload.metadata_blob_oid).ok();
            emit_one(&payload, metadata.as_deref(), output)
        }
        None => Err(CliError::fatal(format!(
            "no checkpoint matches id '{}'",
            args.checkpoint_id
        ))),
    }
}

/// `libra agent checkpoint rewind <id> [--dry-run|--apply]`.
///
/// `dry-run` (the default when neither flag is set) lists the files the
/// checkpoint's `parent_commit` snapshot would restore, without touching the
/// worktree. `--apply` actually runs the worktree restore (delegating to the
/// existing `restore --source <parent_commit>` path), prints the
/// transcript-untouched warning, and leaves HEAD plus `refs/heads/*`
/// untouched per `docs/improvement/entire.md` §7.3.
async fn rewind(args: CheckpointRewindArgs, output: &OutputConfig) -> CliResult<()> {
    let conn = get_db_conn_instance().await;
    if !table_exists(&conn, "agent_checkpoint").await? {
        return Err(CliError::fatal(format!(
            "no checkpoint matches '{}': agent_checkpoint table not yet present (run `libra init`?)",
            args.checkpoint_id
        )));
    }
    let backend = conn.get_database_backend();
    let row = conn
        .query_one(Statement::from_sql_and_values(
            backend,
            "SELECT parent_commit, traces_commit FROM agent_checkpoint \
             WHERE checkpoint_id = ? LIMIT 1",
            [args.checkpoint_id.clone().into()],
        ))
        .await
        .map_err(|e| CliError::fatal(format!("failed to query agent_checkpoint: {e}")))?
        .ok_or_else(|| {
            CliError::fatal(format!("no checkpoint matches id '{}'", args.checkpoint_id))
        })?;

    let parent_commit: Option<String> = row.try_get_by("parent_commit").ok().flatten();
    let traces_commit: String = row.try_get_by("traces_commit").unwrap_or_default();

    // Without a parent_commit (unborn HEAD at ingest time) there is nothing
    // to restore the worktree to. Surface a clear diagnostic rather than a
    // silent no-op.
    let parent_commit = match parent_commit {
        Some(c) if !c.is_empty() => c,
        _ => {
            return Err(CliError::fatal(format!(
                "checkpoint '{}' has no recorded parent_commit (unborn HEAD or pre-commit ingest); \
                 nothing to rewind to. checkpoint commit: {traces_commit}",
                args.checkpoint_id
            )));
        }
    };

    // Resolve the parent commit's tree and enumerate files that would be
    // restored. We use this both for dry-run output and for a "summary
    // before apply" line.
    let parent_oid = ObjectHash::from_str(&parent_commit).map_err(|e| {
        CliError::fatal(format!(
            "checkpoint '{}' has invalid parent_commit '{parent_commit}': {e}",
            args.checkpoint_id
        ))
    })?;
    // Codex Phase-2-followups round-1 P1 #2: dry-run was previously
    // emitting only the additions/modifications side, leaving users
    // surprised when `--apply` also DELETED tracked files that were absent
    // from the target commit. The plan now surfaces both sides:
    //   restore = files in the target commit's tree (will be written)
    //   delete  = files tracked by the index but absent from the target
    //             tree (will be removed by the worktree-restore pass)
    let plan = build_rewind_plan(&parent_oid).map_err(|e| {
        CliError::fatal(format!("failed to enumerate files for rewind preview: {e}"))
    })?;

    if !args.apply {
        // dry-run path. We arrived here because either `--dry-run` was
        // explicit or neither flag was passed.
        if output.is_json() {
            let payload = serde_json::json!({
                "checkpoint_id": args.checkpoint_id,
                "parent_commit": parent_commit,
                "traces_commit": traces_commit,
                "would_restore_paths": plan.restore,
                "would_delete_paths": plan.delete,
                "applied": false,
                "transcript_truncation_supported": false,
            });
            return emit_json_data("agent_checkpoint_rewind", &payload, output);
        }
        if output.quiet {
            return Ok(());
        }
        println!("Dry run — no files modified.");
        println!("checkpoint_id : {}", args.checkpoint_id);
        println!("parent_commit : {parent_commit}");
        println!("traces_commit : {traces_commit}");
        println!("would restore {} path(s):", plan.restore.len());
        for path in &plan.restore {
            println!("  + {path}");
        }
        println!("would delete  {} path(s):", plan.delete.len());
        for path in &plan.delete {
            println!("  - {path}");
        }
        println!(
            "Re-run with --apply to restore the working tree. The agent's \
             local transcript file will NOT be rewritten."
        );
        return Ok(());
    }

    // --apply path: drive the typed restore for working-tree only,
    // matching the dry-run preview's file set. Re-using `restore` keeps
    // the LFS / index / pathspec semantics consistent with the rest of
    // the CLI.
    use crate::command::restore::{RestoreArgs, execute_checked_typed};
    let restore_args = RestoreArgs {
        pathspec: vec![".".to_string()],
        source: Some(parent_commit.clone()),
        worktree: true,
        staged: false,
    };
    execute_checked_typed(restore_args)
        .await
        .map_err(|e| CliError::fatal(format!("rewind --apply failed: {e}")))?;

    if output.is_json() {
        // Codex Phase-2-followups round-1 P2: --apply previously emitted
        // human text even when --json was set. The structured payload
        // mirrors the dry-run shape with `applied: true`.
        let payload = serde_json::json!({
            "checkpoint_id": args.checkpoint_id,
            "parent_commit": parent_commit,
            "traces_commit": traces_commit,
            "restored_paths": plan.restore,
            "deleted_paths": plan.delete,
            "applied": true,
            "transcript_truncation_supported": false,
            "transcript_warning": "Transcript truncation for the captured agent is not yet \
                                   implemented in v1; the local transcript file remains unchanged.",
        });
        return emit_json_data("agent_checkpoint_rewind", &payload, output);
    }
    if !output.quiet {
        println!(
            "Restored {} path(s), deleted {} path(s) from {parent_commit}.",
            plan.restore.len(),
            plan.delete.len()
        );
        println!(
            "Note: Transcript truncation for the captured agent is not yet \
             implemented in v1. The agent's local transcript file remains \
             unchanged. Re-running the agent may produce inconsistent context."
        );
    }
    Ok(())
}

/// Files affected by a `rewind --apply`, broken down by side. `restore`
/// = present in the target commit (will be written to the worktree
/// after `--apply`); `delete` = tracked by the index but absent from the
/// target commit's tree (will be removed from the worktree by the
/// underlying restore's deleted-files pass — see
/// `command::restore::restore_worktree_tracked`).
struct RewindPlan {
    restore: Vec<String>,
    delete: Vec<String>,
}

fn build_rewind_plan(commit_oid: &ObjectHash) -> Result<RewindPlan, anyhow::Error> {
    use std::{collections::HashSet, path::PathBuf};

    use git_internal::internal::index::Index;

    let commit: Commit = load_object(commit_oid)
        .map_err(|e| anyhow::anyhow!("failed to load commit {commit_oid}: {e}"))?;
    let tree: Tree = load_object(&commit.tree_id)
        .map_err(|e| anyhow::anyhow!("failed to load tree {}: {e}", commit.tree_id))?;
    let target: Vec<(PathBuf, ObjectHash)> = tree.get_plain_items();
    let target_set: HashSet<PathBuf> = target.iter().map(|(p, _)| p.clone()).collect();

    let mut restore: Vec<String> = target
        .iter()
        .map(|(p, _)| p.display().to_string())
        .collect();
    restore.sort();

    // The index is the authoritative tracked-files view. Any path tracked
    // there but absent from the target tree will be removed by the
    // worktree restore — surface it in the dry-run so users see both
    // sides of the diff.
    let mut delete: Vec<String> = match Index::load(crate::utils::path::index()) {
        Ok(index) => index
            .tracked_entries(0)
            .into_iter()
            .filter_map(|entry| {
                let path = PathBuf::from(&entry.name);
                if target_set.contains(&path) {
                    None
                } else {
                    Some(path.display().to_string())
                }
            })
            .collect(),
        // Index unreadable (e.g. fresh repo with no staged files) — leave
        // the deletion set empty and let the user proceed with --apply.
        Err(_) => Vec::new(),
    };
    delete.sort();

    Ok(RewindPlan { restore, delete })
}

fn load_metadata_blob(oid: &str) -> Result<String, CliError> {
    let hash = ObjectHash::from_str(oid)
        .map_err(|e| CliError::fatal(format!("invalid metadata_blob_oid '{oid}': {e}")))?;
    let storage = util::try_get_storage_path(None)
        .map_err(|e| CliError::fatal(format!("not in a libra repository: {e}")))?;
    let raw = read_git_object(&storage, &hash).map_err(|e| {
        CliError::fatal(format!(
            "failed to read metadata blob {oid} from object store: {e}"
        ))
    })?;
    String::from_utf8(raw)
        .map_err(|e| CliError::fatal(format!("metadata blob {oid} is not UTF-8: {e}")))
}

fn emit_list(rows: &[CheckpointRow], output: &OutputConfig) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("agent_checkpoints", &rows, output);
    }
    if output.quiet {
        return Ok(());
    }
    if rows.is_empty() {
        println!("(no captured checkpoints)");
        return Ok(());
    }
    println!(
        "{:<37}  {:<37}  {:<10}  {:<20}",
        "checkpoint_id", "session_id", "scope", "created_at"
    );
    for r in rows {
        println!(
            "{:<37}  {:<37}  {:<10}  {:<20}",
            r.checkpoint_id, r.session_id, r.scope, r.created_at
        );
    }
    Ok(())
}

fn emit_one(
    row: &CheckpointRow,
    metadata_blob: Option<&str>,
    output: &OutputConfig,
) -> CliResult<()> {
    if output.is_json() {
        // Inline the metadata content as parsed JSON so JSON consumers can
        // join on it without doing a second blob fetch.
        let metadata_json = metadata_blob
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
            .unwrap_or(serde_json::Value::Null);
        let payload = serde_json::json!({
            "checkpoint": row,
            "metadata": metadata_json,
        });
        return emit_json_data("agent_checkpoint", &payload, output);
    }
    if output.quiet {
        return Ok(());
    }
    println!("checkpoint_id     : {}", row.checkpoint_id);
    println!("session_id        : {}", row.session_id);
    println!("scope             : {}", row.scope);
    let parent_display = match row.parent_commit.as_deref() {
        Some(commit) if !commit.is_empty() => commit,
        _ => "(none — unborn HEAD or pre-commit ingest)",
    };
    println!("parent_commit     : {parent_display}");
    println!("tree_oid          : {}", row.tree_oid);
    println!("metadata_blob_oid : {}", row.metadata_blob_oid);
    println!("traces_commit     : {}", row.traces_commit);
    println!("created_at        : {}", row.created_at);
    if let Some(metadata) = metadata_blob {
        println!("---");
        println!("metadata.json:");
        println!("{metadata}");
    }
    Ok(())
}

async fn table_exists(conn: &(impl ConnectionTrait + ?Sized), name: &str) -> CliResult<bool> {
    let backend = conn.get_database_backend();
    let stmt = Statement::from_sql_and_values(
        backend,
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ? LIMIT 1",
        [name.into()],
    );
    conn.query_one(stmt)
        .await
        .map(|row| row.is_some())
        .map_err(|e| CliError::fatal(format!("failed to query sqlite_master: {e}")))
}
