//! `libra agent session …` subcommands. V1 ships read-only `list` and `show`
//! that surface rows from `agent_session`; mutating verbs (`stop`, `resume`)
//! return a phase-2 stub.

use clap::{Args, Subcommand};
use sea_orm::{ConnectionTrait, Statement};
use serde::Serialize;

use crate::{
    internal::{ai::observed_agents::AgentKind, db::get_db_conn_instance},
    utils::{
        error::{CliError, CliResult},
        output::{OutputConfig, emit_json_data},
    },
};

#[derive(Subcommand, Debug)]
pub enum SessionSubcommand {
    /// List captured sessions.
    #[command(about = "List captured sessions")]
    List(SessionListArgs),
    /// Show a single session by id.
    #[command(about = "Show a captured session")]
    Show(SessionShowArgs),
    /// Stop a session (phase 2).
    #[command(about = "Stop a captured session")]
    Stop(SessionStopArgs),
    /// Resume a stopped session (phase 2).
    #[command(about = "Resume a stopped session")]
    Resume(SessionResumeArgs),
}

#[derive(Args, Debug)]
pub struct SessionListArgs {
    /// Filter by agent kind (slug, e.g. `claude-code`).
    #[arg(long, value_name = "NAME")]
    pub agent: Option<String>,
    /// Filter by state (`active`, `stopped`, …).
    #[arg(long, value_name = "STATE")]
    pub state: Option<String>,
}

#[derive(Args, Debug)]
pub struct SessionShowArgs {
    pub session_id: String,
    /// Materialise the captured transcript at the given path. Phase 2.
    #[arg(long, value_name = "PATH")]
    pub extract_transcript: Option<String>,
}

#[derive(Args, Debug)]
pub struct SessionStopArgs {
    pub session_id: String,
}

#[derive(Args, Debug)]
pub struct SessionResumeArgs {
    pub session_id: String,
}

pub async fn execute_safe(cmd: SessionSubcommand, output: &OutputConfig) -> CliResult<()> {
    match cmd {
        SessionSubcommand::List(args) => list(args, output).await,
        SessionSubcommand::Show(args) => show(args, output).await,
        SessionSubcommand::Stop(_) | SessionSubcommand::Resume(_) => {
            if !output.quiet {
                println!(
                    "libra agent session: stop / resume not yet implemented in v1 phase 1; \
                     landing in phase 2."
                );
            }
            Ok(())
        }
    }
}

#[derive(Debug, Serialize)]
struct SessionRow {
    session_id: String,
    agent_kind: String,
    state: String,
    working_dir: String,
    started_at: i64,
    last_event_at: i64,
}

async fn list(args: SessionListArgs, output: &OutputConfig) -> CliResult<()> {
    let conn = get_db_conn_instance().await;
    let backend = conn.get_database_backend();

    if !table_exists(&conn, "agent_session").await? {
        return emit_list(&[], output);
    }

    let mut sql = String::from(
        "SELECT session_id, agent_kind, state, working_dir, started_at, last_event_at \
         FROM agent_session WHERE 1=1",
    );
    let mut values: Vec<sea_orm::Value> = Vec::new();
    if let Some(agent) = &args.agent {
        // The CLI accepts hyphenated slugs (`claude-code`) but the database
        // stores the snake_case `agent_kind` (`claude_code`). Translate to
        // the storage form so a `--agent claude-code` filter actually
        // matches rows. Codex review P1 #6.
        let normalized = match AgentKind::from_cli_slug(agent) {
            Some(kind) => kind.as_db_str().to_string(),
            None => agent.clone(),
        };
        sql.push_str(" AND agent_kind = ?");
        values.push(normalized.into());
    }
    if let Some(state) = &args.state {
        sql.push_str(" AND state = ?");
        values.push(state.clone().into());
    }
    sql.push_str(" ORDER BY started_at DESC LIMIT 200");

    let stmt = Statement::from_sql_and_values(backend, &sql, values);
    let rows = conn
        .query_all(stmt)
        .await
        .map_err(|e| CliError::fatal(format!("failed to query agent_session: {e}")))?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(SessionRow {
            session_id: row
                .try_get_by::<String, _>("session_id")
                .unwrap_or_default(),
            agent_kind: row
                .try_get_by::<String, _>("agent_kind")
                .unwrap_or_default(),
            state: row.try_get_by::<String, _>("state").unwrap_or_default(),
            working_dir: row
                .try_get_by::<String, _>("working_dir")
                .unwrap_or_default(),
            started_at: row.try_get_by::<i64, _>("started_at").unwrap_or_default(),
            last_event_at: row
                .try_get_by::<i64, _>("last_event_at")
                .unwrap_or_default(),
        });
    }
    emit_list(&out, output)
}

async fn show(args: SessionShowArgs, output: &OutputConfig) -> CliResult<()> {
    let conn = get_db_conn_instance().await;
    let backend = conn.get_database_backend();

    if !table_exists(&conn, "agent_session").await? {
        return Err(CliError::fatal(format!(
            "no captured session matches '{}': agent_session table not yet present (run `libra init`?)",
            args.session_id
        )));
    }

    let stmt = Statement::from_sql_and_values(
        backend,
        "SELECT session_id, agent_kind, state, working_dir, started_at, last_event_at \
         FROM agent_session WHERE session_id = ? LIMIT 1",
        [args.session_id.clone().into()],
    );
    let row = conn
        .query_one(stmt)
        .await
        .map_err(|e| CliError::fatal(format!("failed to query agent_session: {e}")))?;
    match row {
        Some(row) => {
            let payload = SessionRow {
                session_id: row
                    .try_get_by::<String, _>("session_id")
                    .unwrap_or_default(),
                agent_kind: row
                    .try_get_by::<String, _>("agent_kind")
                    .unwrap_or_default(),
                state: row.try_get_by::<String, _>("state").unwrap_or_default(),
                working_dir: row
                    .try_get_by::<String, _>("working_dir")
                    .unwrap_or_default(),
                started_at: row.try_get_by::<i64, _>("started_at").unwrap_or_default(),
                last_event_at: row
                    .try_get_by::<i64, _>("last_event_at")
                    .unwrap_or_default(),
            };
            if args.extract_transcript.is_some() && !output.quiet {
                println!(
                    "Note: --extract-transcript is not yet implemented in v1 phase 1 \
                     (transcripts ship in phase 2)."
                );
            }
            emit_one(&payload, output)
        }
        None => Err(CliError::fatal(format!(
            "no captured session matches id '{}'",
            args.session_id
        ))),
    }
}

fn emit_list(rows: &[SessionRow], output: &OutputConfig) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("agent_sessions", &rows, output);
    }
    if output.quiet {
        return Ok(());
    }
    if rows.is_empty() {
        println!("(no captured sessions)");
        return Ok(());
    }
    println!(
        "{:<37}  {:<14}  {:<10}  {:<20}",
        "session_id", "agent_kind", "state", "started_at"
    );
    for r in rows {
        println!(
            "{:<37}  {:<14}  {:<10}  {:<20}",
            r.session_id, r.agent_kind, r.state, r.started_at
        );
    }
    Ok(())
}

fn emit_one(row: &SessionRow, output: &OutputConfig) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("agent_session", row, output);
    }
    if output.quiet {
        return Ok(());
    }
    println!("session_id    : {}", row.session_id);
    println!("agent_kind    : {}", row.agent_kind);
    println!("state         : {}", row.state);
    println!("working_dir   : {}", row.working_dir);
    println!("started_at    : {}", row.started_at);
    println!("last_event_at : {}", row.last_event_at);
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
