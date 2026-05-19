//! `libra agent clean [--all]` — drop temporary checkpoints from stopped
//! sessions per `docs/improvement/entire.md` §7.4.
//!
//! V1 cuts temporary checkpoint rows from the SQLite catalog. Rewriting the
//! `refs/libra/agent-traces` orphan ref tip to drop the matching commits is
//! a follow-up in this same module — but Phase 2 first cut so far only
//! emits `committed` checkpoints (no `temporary` ones), so the rewrite path
//! has nothing to delete in practice. The CLI surface and the SQL deletion
//! still ship now so users + tests can exercise the path; the ref-rewrite
//! becomes meaningful once temporary checkpoints land in a follow-up.

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

    // Count stopped sessions we'll consider. With --all we also include
    // active sessions so an operator can wipe stale temp checkpoints on
    // sessions whose host process crashed without firing SessionEnd.
    let session_filter = if args.all {
        "SELECT COUNT(*) AS n FROM agent_session"
    } else {
        "SELECT COUNT(*) AS n FROM agent_session WHERE state = 'stopped'"
    };
    let row = conn
        .query_one(Statement::from_sql_and_values(backend, session_filter, []))
        .await
        .map_err(|e| CliError::fatal(format!("failed to count agent_session: {e}")))?
        .ok_or_else(|| CliError::fatal("agent_session count returned no rows".to_string()))?;
    let sessions_inspected: i64 = row.try_get_by("n").unwrap_or_default();

    // Delete the `scope='temporary'` checkpoints whose owning session is in
    // the chosen scope. The schema has ON DELETE CASCADE from session →
    // checkpoint, but a user calling `clean` only wants temporary rows
    // gone — committed rows stay regardless.
    let delete_sql = if args.all {
        "DELETE FROM agent_checkpoint WHERE scope = 'temporary' \
         AND session_id IN (SELECT session_id FROM agent_session)"
    } else {
        "DELETE FROM agent_checkpoint WHERE scope = 'temporary' \
         AND session_id IN (SELECT session_id FROM agent_session WHERE state = 'stopped')"
    };
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
