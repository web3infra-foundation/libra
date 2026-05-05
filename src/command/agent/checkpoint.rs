//! `libra agent checkpoint …` subcommands. V1 ships read-only `list` /
//! `show`; the mutating `rewind` path stays a phase-2 dry-run stub.

use std::str::FromStr;

use git_internal::hash::ObjectHash;
use sea_orm::{ConnectionTrait, Statement};
use serde::Serialize;

use super::{CheckpointListArgs, CheckpointShowArgs, CheckpointSubcommand};
use crate::{
    internal::db::get_db_conn_instance,
    utils::{
        error::{CliError, CliResult},
        object::read_git_object,
        output::{OutputConfig, emit_json_data},
        util,
    },
};

pub async fn execute_safe(cmd: CheckpointSubcommand, output: &OutputConfig) -> CliResult<()> {
    match cmd {
        CheckpointSubcommand::List(args) => list(args, output).await,
        CheckpointSubcommand::Show(args) => show(args, output).await,
        CheckpointSubcommand::Rewind(_) => {
            if !output.quiet {
                println!(
                    "libra agent checkpoint rewind: not yet implemented in v1; \
                     working-tree-only rewind lands in a follow-up Phase 2 change."
                );
            }
            Ok(())
        }
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
