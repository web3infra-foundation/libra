//! `libra agent clean [--all]` — drop temporary checkpoints from stopped
//! sessions per `docs/improvement/entire.md` §7.4.
//!
//! The default form scopes cleanup to the most recently stopped session;
//! `--all` widens that to every stopped session. Active sessions are never
//! cleaned because a temporary checkpoint may still be part of an in-flight
//! external-agent turn.
//!
//! When checkpoint commits are present, cleanup rewrites
//! `refs/libra/agent-traces` so pruned temporary checkpoints stop being
//! reachable. Older DB-only fixtures with an empty ref still get the catalog
//! cleanup without a rewrite.

use std::sync::Arc;

use sea_orm::{ConnectionTrait, Statement};
use serde::Serialize;

use super::CleanArgs;
use crate::{
    internal::{
        ai::history::HistoryManager, branch::AGENT_TRACES_BRANCH, db::get_db_conn_instance,
    },
    utils::{
        client_storage::ClientStorage,
        error::{CliError, CliResult},
        output::{OutputConfig, emit_json_data},
        util,
    },
};

#[derive(Debug, Serialize)]
struct CleanReport {
    sessions_inspected: i64,
    temporary_checkpoints_dropped: u64,
    retained_checkpoints_rewritten: usize,
    agent_traces_ref_rewritten: bool,
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
                retained_checkpoints_rewritten: 0,
                agent_traces_ref_rewritten: false,
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

    let checkpoint_ids = temporary_checkpoint_ids(&conn, session_scope).await?;
    let repo_path = util::try_get_storage_path(None)
        .map_err(|e| CliError::fatal(format!("failed to locate .libra directory: {e}")))?;
    let storage = Arc::new(ClientStorage::init(repo_path.join("objects")));
    let history = HistoryManager::new_with_ref(
        storage,
        repo_path,
        Arc::new(conn.clone()),
        AGENT_TRACES_BRANCH,
    );
    let prune = history
        .prune_checkpoint_commits(&checkpoint_ids)
        .await
        .map_err(|e| CliError::fatal(format!("failed to prune agent-traces checkpoints: {e}")))?;

    emit_report(
        &CleanReport {
            sessions_inspected,
            temporary_checkpoints_dropped: prune.removed_checkpoints,
            retained_checkpoints_rewritten: prune.rewritten_checkpoints,
            agent_traces_ref_rewritten: prune.ref_rewritten,
            note: "temporary checkpoint rows were dropped; reachable agent-traces history was \
                   rewritten when checkpoint commits existed",
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

async fn temporary_checkpoint_ids(
    conn: &(impl ConnectionTrait + ?Sized),
    session_scope: &str,
) -> CliResult<Vec<String>> {
    let backend = conn.get_database_backend();
    let query = format!(
        "SELECT checkpoint_id FROM agent_checkpoint WHERE scope = 'temporary' \
         AND session_id IN (SELECT session_id FROM ({session_scope}) AS scoped_sessions) \
         ORDER BY created_at ASC, checkpoint_id ASC"
    );
    let rows = conn
        .query_all(Statement::from_string(backend, query))
        .await
        .map_err(|e| CliError::fatal(format!("failed to list temporary checkpoints: {e}")))?;
    rows.into_iter()
        .map(|row| {
            row.try_get_by("checkpoint_id")
                .map_err(|e| CliError::fatal(format!("failed to decode checkpoint_id: {e}")))
        })
        .collect()
}
