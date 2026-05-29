//! `libra agent clean [--all]` — drop temporary checkpoints from stopped
//! sessions per `docs/improvement/entire.md` §7.4.
//!
//! The default form scopes cleanup to the most recently stopped session;
//! `--all` widens that to every stopped session. Active sessions are never
//! cleaned because a temporary checkpoint may still be part of an in-flight
//! external-agent turn.
//!
//! V1 cuts temporary checkpoint rows from the SQLite catalog. Rewriting
//! `refs/libra/agent-traces` to make matching commits unreachable is a
//! follow-up in this same module.

use sea_orm::{ConnectionTrait, Statement};
use serde::Serialize;

use super::CleanArgs;
use crate::{
    internal::db::get_db_conn_instance,
    utils::{
        error::{CliError, CliResult},
        output::{OutputConfig, emit_json_data},
    },
};

#[derive(Debug, Serialize)]
struct CleanReport {
    sessions_inspected: i64,
    temporary_checkpoints_dropped: u64,
    note: &'static str,
}

pub async fn execute_safe(args: CleanArgs, output: &OutputConfig) -> CliResult<()> {
    let conn = get_db_conn_instance().await;
    let backend = conn.get_database_backend();

    if !table_exists(&conn, "agent_checkpoint").await? {
        return emit_report(
            &CleanReport {
                sessions_inspected: 0,
                temporary_checkpoints_dropped: 0,
                note: "agent_checkpoint table not present (run `libra init`?)",
            },
            output,
        );
    }

    let session_scope = session_scope_sql(args.all);
    let session_filter = format!("SELECT COUNT(*) AS n FROM ({session_scope}) AS scoped_sessions");
    let row = conn
        .query_one(Statement::from_string(backend, session_filter))
        .await
        .map_err(|e| CliError::fatal(format!("failed to count agent_session: {e}")))?
        .ok_or_else(|| CliError::fatal("agent_session count returned no rows".to_string()))?;
    let sessions_inspected: i64 = row.try_get_by("n").unwrap_or_default();

    // Delete the `scope='temporary'` checkpoints whose owning session is in
    // the chosen scope. The schema has ON DELETE CASCADE from session →
    // checkpoint, but a user calling `clean` only wants temporary rows
    // gone — committed rows stay regardless.
    let delete_sql = format!(
        "DELETE FROM agent_checkpoint WHERE scope = 'temporary' \
         AND session_id IN (SELECT session_id FROM ({session_scope}) AS scoped_sessions)"
    );
    let res = conn
        .execute(Statement::from_string(backend, delete_sql.to_string()))
        .await
        .map_err(|e| CliError::fatal(format!("failed to drop temporary checkpoints: {e}")))?;
    let dropped = res.rows_affected();

    emit_report(
        &CleanReport {
            sessions_inspected,
            temporary_checkpoints_dropped: dropped,
            note: "agent-traces ref rewrite is a follow-up; temporary commits will become \
                   unreachable once Phase 2 emits them, then `git gc` reclaims them",
        },
        output,
    )
}

fn session_scope_sql(all: bool) -> &'static str {
    if all {
        return "SELECT session_id FROM agent_session WHERE state = 'stopped'";
    }
    "SELECT session_id FROM agent_session \
     WHERE state = 'stopped' \
     ORDER BY COALESCE(stopped_at, last_event_at, started_at) DESC, session_id DESC \
     LIMIT 1"
}

fn emit_report(report: &CleanReport, output: &OutputConfig) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("agent_clean", report, output);
    }
    if output.quiet {
        return Ok(());
    }
    println!(
        "Sessions inspected            : {}",
        report.sessions_inspected
    );
    println!(
        "Temporary checkpoints dropped : {}",
        report.temporary_checkpoints_dropped
    );
    println!("Note                          : {}", report.note);
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
