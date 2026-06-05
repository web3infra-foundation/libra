//! Integration coverage for `libra agent checkpoint` mutation paths.

use std::{fs, path::Path, sync::Arc, time::Duration};

use libra::{
    internal::{
        ai::{
            history::{CheckpointCommitParams, CheckpointScope, HistoryManager},
            observed_agents::Redactor,
        },
        branch::AGENT_TRACES_BRANCH,
    },
    utils::client_storage::ClientStorage,
};
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection, Statement, Value};
use serial_test::serial;

use super::{
    ChangeDirGuard, Head, assert_cli_success, create_committed_repo_via_cli, parse_json_stdout,
    run_libra_command,
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

async fn seed_checkpoint_for_parent(conn: &DatabaseConnection, checkpoint_id: &str, parent: &str) {
    conn.execute(Statement::from_sql_and_values(
        conn.get_database_backend(),
        "INSERT INTO agent_session (
            session_id, agent_kind, provider_session_id, state, working_dir,
            metadata_json, redaction_report, started_at, last_event_at, stopped_at
         ) VALUES ('session-rewind', 'gemini', 'provider-rewind', 'stopped',
                   '/tmp/libra-agent-checkpoint-test', '{}', '{}', 10, 20, 30)",
        [],
    ))
    .await
    .expect("insert agent_session");

    conn.execute(Statement::from_sql_and_values(
        conn.get_database_backend(),
        "INSERT INTO agent_checkpoint (
            checkpoint_id, session_id, scope, parent_commit, tree_oid,
            metadata_blob_oid, traces_commit, created_at
         ) VALUES (?, 'session-rewind', 'committed', ?, ?, ?, ?, 40)",
        vec![
            Value::from(checkpoint_id),
            Value::from(parent),
            Value::from("1111111111111111111111111111111111111111"),
            Value::from("2222222222222222222222222222222222222222"),
            Value::from("3333333333333333333333333333333333333333"),
        ],
    ))
    .await
    .expect("insert agent_checkpoint");
}

/// Append a *real* committed checkpoint commit (real blobs + tree) and
/// register its catalog row, returning the redacted transcript byte length so
/// the caller can assert `checkpoint show` reports it. Mirrors the production
/// `write_committed_checkpoint` layout: `metadata.json` blob +
/// `transcript/<provider>` blob under `checkpoint/<id[:2]>/<id[2:]>/`.
async fn seed_real_committed_checkpoint(
    conn: &DatabaseConnection,
    repo: &Path,
    checkpoint_id: &str,
    session_id: &str,
) -> usize {
    conn.execute(Statement::from_sql_and_values(
        conn.get_database_backend(),
        "INSERT INTO agent_session (
            session_id, agent_kind, provider_session_id, state, working_dir,
            metadata_json, redaction_report, started_at, last_event_at, stopped_at
         ) VALUES (?, 'claude_code', ?, 'stopped', '/tmp/libra-agent-show-test',
                   '{}', '{}', 10, 20, 30)",
        vec![
            Value::from(session_id),
            Value::from(format!("provider-{session_id}")),
        ],
    ))
    .await
    .expect("insert agent_session");

    let repo_path = repo.join(".libra");
    let storage = Arc::new(ClientStorage::init(repo_path.join("objects")));
    let history = HistoryManager::new_with_ref(
        storage,
        repo_path.clone(),
        Arc::new(conn.clone()),
        AGENT_TRACES_BRANCH,
    );
    let redactor = Redactor::new_default();
    let (redacted, _) = redactor.redact(format!("transcript for {checkpoint_id}").as_bytes());
    let transcript_len = redacted.len();
    let metadata = format!(r#"{{"checkpoint_id":"{checkpoint_id}"}}"#);
    let written = history
        .append_checkpoint_commit(CheckpointCommitParams {
            checkpoint_id,
            session_id,
            agent_kind: "claude_code",
            parent_commit: None,
            scope: CheckpointScope::Committed,
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
         ) VALUES (?, ?, 'committed', NULL, ?, ?, ?, 40)",
        vec![
            Value::from(checkpoint_id),
            Value::from(session_id),
            Value::from(written.tree_oid.to_string()),
            Value::from(written.metadata_blob_oid.to_string()),
            Value::from(written.commit_hash.to_string()),
        ],
    ))
    .await
    .expect("insert agent_checkpoint");

    transcript_len
}

/// `checkpoint show --json` surfaces the redacted transcript byte length and a
/// tree summary enumerating the checkpoint commit's leaf blobs
/// (`metadata.json` + `transcript/<provider>`). Covers entire.md §7.3's
/// "metadata + transcript 长度 + tree 摘要".
#[tokio::test]
#[serial]
async fn agent_checkpoint_show_reports_tree_summary_and_transcript_bytes() {
    let repo = create_committed_repo_via_cli();
    let _guard = ChangeDirGuard::new(repo.path());
    let conn = connect_repo_db(repo.path()).await;
    let transcript_len =
        seed_real_committed_checkpoint(&conn, repo.path(), "cp-show", "sess-show").await;

    let output = run_libra_command(
        &["--json", "agent", "checkpoint", "show", "cp-show"],
        repo.path(),
    );
    assert_cli_success(&output, "agent checkpoint show --json");
    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "agent_checkpoint");

    // transcript_bytes is surfaced both at the top level and inside `tree`.
    assert_eq!(json["data"]["transcript_bytes"], transcript_len as u64);
    assert_eq!(
        json["data"]["tree"]["transcript_bytes"],
        transcript_len as u64
    );

    let entries = json["data"]["tree"]["entries"]
        .as_array()
        .expect("tree.entries array");
    assert!(
        entries.iter().any(|e| e["path"]
            .as_str()
            .is_some_and(|p| p.ends_with("metadata.json"))),
        "tree summary must list metadata.json, got {entries:?}"
    );
    assert!(
        entries.iter().any(|e| e["path"]
            .as_str()
            .is_some_and(|p| p.ends_with("/transcript/claude_code"))),
        "tree summary must list the transcript blob, got {entries:?}"
    );
}

#[tokio::test]
#[serial]
async fn agent_checkpoint_rewind_dry_run_and_apply_restore_worktree_only() {
    let repo = create_committed_repo_via_cli();
    let _guard = ChangeDirGuard::new(repo.path());
    let base = Head::current_commit()
        .await
        .expect("base commit exists")
        .to_string();

    fs::write(repo.path().join("tracked.txt"), "changed\n").unwrap();
    fs::write(repo.path().join("extra.txt"), "extra\n").unwrap();
    let output = run_libra_command(&["add", "tracked.txt", "extra.txt"], repo.path());
    assert_cli_success(&output, "add second commit files");
    let output = run_libra_command(&["commit", "-m", "second", "--no-verify"], repo.path());
    assert_cli_success(&output, "create second commit");
    let head_before_rewind = Head::current_commit()
        .await
        .expect("second commit exists")
        .to_string();
    assert_ne!(base, head_before_rewind);

    let conn = connect_repo_db(repo.path()).await;
    seed_checkpoint_for_parent(&conn, "cp-rewind", &base).await;

    let output = run_libra_command(
        &[
            "--json",
            "agent",
            "checkpoint",
            "rewind",
            "cp-rewind",
            "--dry-run",
        ],
        repo.path(),
    );
    assert_cli_success(&output, "agent checkpoint rewind dry-run");
    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "agent_checkpoint_rewind");
    assert_eq!(json["data"]["applied"], false);
    assert_eq!(json["data"]["parent_commit"], base);
    let restore_paths = json["data"]["would_restore_paths"].as_array().unwrap();
    assert!(restore_paths.iter().any(|path| path == "tracked.txt"));
    let delete_paths = json["data"]["would_delete_paths"].as_array().unwrap();
    assert!(delete_paths.iter().any(|path| path == "extra.txt"));
    assert_eq!(
        fs::read_to_string(repo.path().join("tracked.txt")).unwrap(),
        "changed\n"
    );
    assert!(repo.path().join("extra.txt").exists());
    assert_eq!(
        Head::current_commit().await.unwrap().to_string(),
        head_before_rewind
    );

    let output = run_libra_command(
        &[
            "--json",
            "agent",
            "checkpoint",
            "rewind",
            "cp-rewind",
            "--apply",
        ],
        repo.path(),
    );
    assert_cli_success(&output, "agent checkpoint rewind apply");
    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "agent_checkpoint_rewind");
    assert_eq!(json["data"]["applied"], true);
    assert_eq!(json["data"]["transcript_truncation"]["supported"], false);
    assert_eq!(
        fs::read_to_string(repo.path().join("tracked.txt")).unwrap(),
        "tracked\n"
    );
    assert!(!repo.path().join("extra.txt").exists());
    assert_eq!(
        Head::current_commit().await.unwrap().to_string(),
        head_before_rewind
    );
}
