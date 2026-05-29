//! Integration coverage for `libra agent clean`.
//!
//! The clean command is destructive over the external-agent checkpoint catalog,
//! so the stopped-vs-active session boundary is part of the command contract.

use std::{path::Path, time::Duration};

use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection, Statement, Value};

use super::{assert_cli_success, init_repo_via_cli, run_libra_command};

async fn connect_repo_db(repo: &Path) -> DatabaseConnection {
    let db_path = repo.join(".libra").join("libra.db");
    let mut opts = ConnectOptions::new(format!("sqlite://{}", db_path.display()));
    opts.sqlx_logging(false)
        .connect_timeout(Duration::from_secs(5));
    Database::connect(opts)
        .await
        .expect("connect repository database")
}

async fn seed_session(
    conn: &DatabaseConnection,
    session_id: &str,
    state: &str,
    started_at: i64,
    last_event_at: i64,
    stopped_at: i64,
) {
    conn.execute(Statement::from_sql_and_values(
        conn.get_database_backend(),
        "INSERT INTO agent_session (
            session_id, agent_kind, provider_session_id, state, working_dir,
            metadata_json, redaction_report, started_at, last_event_at, stopped_at
         ) VALUES (?, 'claude_code', ?, ?, ?, '{}', '{}', ?, ?, ?)",
        vec![
            Value::from(session_id),
            Value::from(format!("provider-{session_id}")),
            Value::from(state),
            Value::from("/tmp/libra-agent-clean-test"),
            Value::from(started_at),
            Value::from(last_event_at),
            Value::from(stopped_at),
        ],
    ))
    .await
    .expect("insert agent_session");
}

async fn seed_checkpoint(
    conn: &DatabaseConnection,
    checkpoint_id: &str,
    session_id: &str,
    scope: &str,
    created_at: i64,
) {
    let parent_commit = format!("{created_at:040x}");
    let tree_oid = format!("{:040x}", created_at + 1);
    let metadata_blob_oid = format!("{:040x}", created_at + 2);
    let traces_commit = format!("{:040x}", created_at + 3);

    conn.execute(Statement::from_sql_and_values(
        conn.get_database_backend(),
        "INSERT INTO agent_checkpoint (
            checkpoint_id, session_id, scope, parent_commit, tree_oid,
            metadata_blob_oid, traces_commit, created_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        vec![
            Value::from(checkpoint_id),
            Value::from(session_id),
            Value::from(scope),
            Value::from(parent_commit),
            Value::from(tree_oid),
            Value::from(metadata_blob_oid),
            Value::from(traces_commit),
            Value::from(created_at),
        ],
    ))
    .await
    .expect("insert agent_checkpoint");
}

async fn checkpoint_exists(conn: &DatabaseConnection, checkpoint_id: &str) -> bool {
    let row = conn
        .query_one(Statement::from_sql_and_values(
            conn.get_database_backend(),
            "SELECT COUNT(*) AS n FROM agent_checkpoint WHERE checkpoint_id = ?",
            [Value::from(checkpoint_id)],
        ))
        .await
        .expect("query checkpoint count")
        .expect("count row");
    let count: i64 = row.try_get_by("n").expect("decode count");
    count == 1
}

#[tokio::test]
async fn agent_clean_all_does_not_drop_active_session_checkpoints() {
    let repo = tempfile::tempdir().expect("repo tempdir");
    init_repo_via_cli(repo.path());

    let conn = connect_repo_db(repo.path()).await;
    seed_session(&conn, "stopped-session", "stopped", 10, 20, 30).await;
    seed_session(&conn, "active-session", "active", 40, 50, 0).await;
    seed_checkpoint(
        &conn,
        "cp-stopped-temp",
        "stopped-session",
        "temporary",
        100,
    )
    .await;
    seed_checkpoint(
        &conn,
        "cp-stopped-committed",
        "stopped-session",
        "committed",
        101,
    )
    .await;
    seed_checkpoint(&conn, "cp-active-temp", "active-session", "temporary", 102).await;
    conn.close().await.expect("close seed connection");

    let output = run_libra_command(&["--quiet", "agent", "clean", "--all"], repo.path());
    assert_cli_success(&output, "libra agent clean --all");

    let conn = connect_repo_db(repo.path()).await;
    assert!(
        !checkpoint_exists(&conn, "cp-stopped-temp").await,
        "--all should drop temporary checkpoints for stopped sessions"
    );
    assert!(
        checkpoint_exists(&conn, "cp-stopped-committed").await,
        "committed checkpoints must never be dropped"
    );
    assert!(
        checkpoint_exists(&conn, "cp-active-temp").await,
        "--all must not drop temporary checkpoints for active sessions"
    );
}

#[tokio::test]
async fn agent_clean_default_only_drops_most_recent_stopped_session() {
    let repo = tempfile::tempdir().expect("repo tempdir");
    init_repo_via_cli(repo.path());

    let conn = connect_repo_db(repo.path()).await;
    seed_session(&conn, "older-stopped", "stopped", 10, 20, 30).await;
    seed_session(&conn, "newer-stopped", "stopped", 40, 50, 60).await;
    seed_session(&conn, "active-session", "active", 70, 80, 0).await;
    seed_checkpoint(&conn, "cp-older-temp", "older-stopped", "temporary", 200).await;
    seed_checkpoint(&conn, "cp-newer-temp", "newer-stopped", "temporary", 201).await;
    seed_checkpoint(&conn, "cp-active-temp", "active-session", "temporary", 202).await;
    conn.close().await.expect("close seed connection");

    let output = run_libra_command(&["--quiet", "agent", "clean"], repo.path());
    assert_cli_success(&output, "libra agent clean");

    let conn = connect_repo_db(repo.path()).await;
    assert!(
        checkpoint_exists(&conn, "cp-older-temp").await,
        "default clean should leave older stopped sessions for a later --all run"
    );
    assert!(
        !checkpoint_exists(&conn, "cp-newer-temp").await,
        "default clean should drop the most recently stopped session"
    );
    assert!(
        checkpoint_exists(&conn, "cp-active-temp").await,
        "default clean must not drop temporary checkpoints for active sessions"
    );
}
