//! Integration coverage for `libra agent clean`.
//!
//! The clean command is destructive over the external-agent checkpoint catalog,
//! so the stopped-vs-active session boundary is part of the command contract.

use std::{path::Path, str::FromStr, sync::Arc, time::Duration};

use git_internal::{
    hash::ObjectHash,
    internal::object::{ObjectTrait, commit::Commit},
};
use libra::{
    internal::{
        ai::{
            history::{CheckpointCommitParams, CheckpointScope, HistoryManager},
            observed_agents::Redactor,
        },
        branch::TRACES_BRANCH,
    },
    utils::{client_storage::ClientStorage, object::read_git_object},
};
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

async fn seed_checkpoint_commit(
    conn: &DatabaseConnection,
    repo: &Path,
    checkpoint_id: &str,
    session_id: &str,
    scope: CheckpointScope,
    created_at: i64,
) -> String {
    let repo_path = repo.join(".libra");
    let storage = Arc::new(ClientStorage::init(repo_path.join("objects")));
    let history = HistoryManager::new_with_ref(
        storage,
        repo_path.clone(),
        Arc::new(conn.clone()),
        TRACES_BRANCH,
    );
    let redactor = Redactor::new_default();
    let (redacted, _) = redactor.redact(format!("transcript for {checkpoint_id}").as_bytes());
    let metadata = format!(r#"{{"checkpoint_id":"{checkpoint_id}"}}"#);
    let written = history
        .append_checkpoint_commit(CheckpointCommitParams {
            checkpoint_id,
            session_id,
            agent_kind: "claude_code",
            parent_commit: None,
            scope,
            tool_use_id: None,
            metadata_json: metadata.as_bytes(),
            transcript_redacted: &redacted,
            provider_name: "claude_code",
            events_jsonl: None,
        })
        .await
        .expect("append checkpoint commit");

    conn.execute(Statement::from_sql_and_values(
        conn.get_database_backend(),
        "INSERT INTO agent_checkpoint (
            checkpoint_id, session_id, scope, parent_commit, tree_oid,
            metadata_blob_oid, traces_commit, created_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        vec![
            Value::from(checkpoint_id),
            Value::from(session_id),
            Value::from(scope.as_str()),
            Value::from(format!("{created_at:040x}")),
            Value::from(written.tree_oid.to_string()),
            Value::from(written.metadata_blob_oid.to_string()),
            Value::from(written.commit_hash.to_string()),
            Value::from(created_at),
        ],
    ))
    .await
    .expect("insert real agent_checkpoint");

    written.commit_hash.to_string()
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

async fn checkpoint_traces_commit(
    conn: &DatabaseConnection,
    checkpoint_id: &str,
) -> Option<String> {
    conn.query_one(Statement::from_sql_and_values(
        conn.get_database_backend(),
        "SELECT traces_commit FROM agent_checkpoint WHERE checkpoint_id = ? LIMIT 1",
        [Value::from(checkpoint_id)],
    ))
    .await
    .expect("query checkpoint traces_commit")
    .map(|row| {
        row.try_get_by("traces_commit")
            .expect("decode traces_commit")
    })
}

async fn agent_traces_head(conn: &DatabaseConnection) -> Option<String> {
    conn.query_one(Statement::from_sql_and_values(
        conn.get_database_backend(),
        "SELECT `commit` FROM reference WHERE name = ? AND kind = 'Branch' LIMIT 1",
        [Value::from(TRACES_BRANCH)],
    ))
    .await
    .expect("query traces ref")
    .and_then(|row| row.try_get_by("commit").ok().flatten())
}

fn reachable_agent_trace_commits(repo: &Path, head: &str) -> Vec<String> {
    let repo_path = repo.join(".libra");
    let mut commits = Vec::new();
    let mut next = Some(ObjectHash::from_str(head).expect("parse head"));
    while let Some(oid) = next {
        let data = read_git_object(&repo_path, &oid).expect("read commit object");
        let commit = Commit::from_bytes(&data, oid).expect("parse commit object");
        commits.push(oid.to_string());
        next = commit.parent_commit_ids.first().copied();
    }
    commits
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

#[tokio::test]
async fn agent_clean_rewrites_agent_traces_when_temporary_commit_is_ancestor() {
    let repo = tempfile::tempdir().expect("repo tempdir");
    init_repo_via_cli(repo.path());

    let conn = connect_repo_db(repo.path()).await;
    seed_session(&conn, "stopped-session", "stopped", 10, 20, 30).await;
    let temporary_commit = seed_checkpoint_commit(
        &conn,
        repo.path(),
        "aa000000-0000-4000-8000-000000000001",
        "stopped-session",
        CheckpointScope::Temporary,
        300,
    )
    .await;
    let original_committed_commit = seed_checkpoint_commit(
        &conn,
        repo.path(),
        "bb000000-0000-4000-8000-000000000002",
        "stopped-session",
        CheckpointScope::Committed,
        301,
    )
    .await;
    conn.close().await.expect("close seed connection");

    let output = run_libra_command(&["--quiet", "agent", "clean", "--all"], repo.path());
    assert_cli_success(&output, "libra agent clean --all");

    let conn = connect_repo_db(repo.path()).await;
    assert!(
        !checkpoint_exists(&conn, "aa000000-0000-4000-8000-000000000001").await,
        "temporary checkpoint row should be deleted"
    );
    let rewritten_committed_commit =
        checkpoint_traces_commit(&conn, "bb000000-0000-4000-8000-000000000002")
            .await
            .expect("committed checkpoint should remain");
    assert_ne!(
        rewritten_committed_commit, original_committed_commit,
        "retained committed checkpoints must be re-pointed at the rewritten history"
    );
    let head = agent_traces_head(&conn)
        .await
        .expect("traces should still point at the retained committed checkpoint");
    assert_eq!(head, rewritten_committed_commit);

    let reachable = reachable_agent_trace_commits(repo.path(), &head);
    assert!(
        !reachable.contains(&temporary_commit),
        "temporary checkpoint commit must become unreachable from traces"
    );
    assert!(
        reachable.contains(&rewritten_committed_commit),
        "rewritten committed checkpoint must remain reachable"
    );
    assert!(
        !reachable.contains(&original_committed_commit),
        "the old committed commit descended from the temporary checkpoint and must be replaced"
    );
}
