//! Integration coverage for `libra agent session` filtering and the
//! `libra agent doctor` stuck-session diagnostic added for entire.md §3.4 /
//! §13 risk #9 (worktree partitioning) and §13 risk #8 (stuck sessions).

use std::{
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection, Statement, Value};
use serial_test::serial;

use super::{
    assert_cli_success, create_committed_repo_via_cli, parse_json_stdout, run_libra_command,
};

async fn connect_repo_db(repo: &Path) -> DatabaseConnection {
    let db_path = repo.join(".libra").join("libra.db");
    let mut opts = ConnectOptions::new(format!("sqlite://{}", db_path.display()));
    opts.sqlx_logging(false)
        .connect_timeout(Duration::from_secs(5));
    Database::connect(opts)
        .await
        .expect("connect repository database")
}

#[allow(clippy::too_many_arguments)]
async fn seed_session(
    conn: &DatabaseConnection,
    session_id: &str,
    state: &str,
    worktree_id: Option<&str>,
    last_event_at: i64,
) {
    conn.execute(Statement::from_sql_and_values(
        conn.get_database_backend(),
        "INSERT INTO agent_session (
            session_id, agent_kind, provider_session_id, state, working_dir, worktree_id,
            metadata_json, redaction_report, started_at, last_event_at, stopped_at
         ) VALUES (?, 'claude_code', ?, ?, '/tmp/libra-agent-session-test', ?,
                   '{}', '{}', 10, ?, NULL)",
        vec![
            Value::from(session_id),
            Value::from(format!("provider-{session_id}")),
            Value::from(state),
            Value::from(worktree_id.map(str::to_string)),
            Value::from(last_event_at),
        ],
    ))
    .await
    .expect("insert agent_session");
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_secs() as i64
}

/// `session list --worktree <id>` only returns sessions captured in that
/// worktree; the column round-trips through JSON output.
#[tokio::test]
#[serial]
async fn agent_session_list_filters_by_worktree() {
    let repo = create_committed_repo_via_cli();
    let conn = connect_repo_db(repo.path()).await;
    seed_session(&conn, "sess-a", "stopped", Some("wt-a"), 100).await;
    seed_session(&conn, "sess-b", "stopped", Some("wt-b"), 200).await;

    let output = run_libra_command(
        &["--json", "agent", "session", "list", "--worktree", "wt-a"],
        repo.path(),
    );
    assert_cli_success(&output, "agent session list --worktree wt-a");
    let json = parse_json_stdout(&output);
    let rows = json["data"].as_array().expect("sessions array");
    assert_eq!(
        rows.len(),
        1,
        "only the wt-a session should match: {rows:?}"
    );
    assert_eq!(rows[0]["session_id"], "sess-a");
    assert_eq!(rows[0]["worktree_id"], "wt-a");

    // A worktree id no session used returns an empty set, not an error.
    let output = run_libra_command(
        &["--json", "agent", "session", "list", "--worktree", "nope"],
        repo.path(),
    );
    assert_cli_success(&output, "agent session list --worktree nope");
    let json = parse_json_stdout(&output);
    assert_eq!(json["data"].as_array().expect("array").len(), 0);
}

/// `agent doctor` flags `active` sessions whose last event is older than the
/// stuck threshold (6h) while leaving freshly-active sessions uncounted.
#[tokio::test]
#[serial]
async fn agent_doctor_reports_stuck_sessions() {
    let repo = create_committed_repo_via_cli();
    let conn = connect_repo_db(repo.path()).await;
    // One active session abandoned long ago (epoch+1) — stuck.
    seed_session(&conn, "sess-stuck", "active", Some("wt"), 1).await;
    // One active session that fired an event just now — not stuck.
    seed_session(&conn, "sess-fresh", "active", Some("wt"), now_secs()).await;

    let output = run_libra_command(&["--json", "agent", "doctor"], repo.path());
    assert_cli_success(&output, "agent doctor --json");
    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "agent_doctor");
    assert_eq!(json["data"]["active_sessions"], 2);
    assert_eq!(
        json["data"]["stuck_sessions"], 1,
        "only the long-idle active session should count as stuck"
    );
}
