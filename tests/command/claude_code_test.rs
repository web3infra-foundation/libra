//! Integration tests for `claude-code` hook ingestion command.

use std::{fs, process::Command};

use libra::{internal::ai::history::HistoryManager, utils::test};
use serial_test::serial;
use tempfile::tempdir;

fn run_hook(temp: &tempfile::TempDir, subcmd: &str, payload: &str) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_libra"));
    cmd.current_dir(temp.path())
        .arg("claude-code")
        .arg(subcmd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let mut child = cmd.spawn().expect("spawn failed");
    {
        use std::io::Write;
        let stdin = child.stdin.as_mut().expect("stdin missing");
        stdin
            .write_all(payload.as_bytes())
            .expect("write stdin failed");
    }
    child.wait_with_output().expect("wait failed")
}

fn session_file(temp: &tempfile::TempDir, id: &str) -> std::path::PathBuf {
    temp.path()
        .join(".libra")
        .join("sessions")
        .join(format!("{id}.json"))
}

async fn build_history_manager(temp: &tempfile::TempDir) -> HistoryManager {
    let _guard = test::ChangeDirGuard::new(temp.path());
    let db = libra::internal::db::get_db_conn_instance().await;
    HistoryManager::new_with_ref(
        std::sync::Arc::new(libra::utils::storage::local::LocalStorage::new(
            temp.path().join(".libra").join("objects"),
        )),
        temp.path().join(".libra"),
        std::sync::Arc::new(db.clone()),
        "libra/intent",
    )
}

#[tokio::test]
#[serial]
async fn test_claude_code_normal_flow_and_persisted() {
    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;

    let session_id = "session-1";
    let cwd = temp.path().to_string_lossy();

    let start = format!(
        r#"{{"hook_event_name":"SessionStart","session_id":"{session_id}","cwd":"{cwd}","model":"claude-3","source":"claude-code"}}"#
    );
    let prompt = format!(
        r#"{{"hook_event_name":"UserPromptSubmit","session_id":"{session_id}","cwd":"{cwd}","prompt":"hello"}}"#
    );
    let tool = format!(
        r#"{{"hook_event_name":"PostToolUse","session_id":"{session_id}","cwd":"{cwd}","tool_name":"Read","tool_input":{{"path":"a.txt"}},"tool_response":{{"ok":true}}}}"#
    );
    let stop = format!(
        r#"{{"hook_event_name":"Stop","session_id":"{session_id}","cwd":"{cwd}","last_assistant_message":"done"}}"#
    );
    let end =
        format!(r#"{{"hook_event_name":"SessionEnd","session_id":"{session_id}","cwd":"{cwd}"}}"#);

    for (subcmd, payload) in [
        ("session-start", start.as_str()),
        ("prompt", prompt.as_str()),
        ("tool-use", tool.as_str()),
        ("stop", stop.as_str()),
        ("session-end", end.as_str()),
    ] {
        let out = run_hook(&temp, subcmd, payload);
        assert!(
            out.status.success(),
            "{subcmd} failed, stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let session_path = session_file(&temp, session_id);
    assert!(
        !session_path.exists(),
        "session file should be removed after successful persistence"
    );

    let history_manager = build_history_manager(&temp).await;
    let head = history_manager.resolve_history_head().await.unwrap();
    assert!(head.is_some(), "expected libra/intent ref to exist");
    let object_hash = history_manager
        .get_object_hash("claude_session", session_id)
        .await
        .unwrap();
    assert!(object_hash.is_some(), "claude_session object should exist");
}

#[tokio::test]
#[serial]
async fn test_claude_code_out_of_order_recovers() {
    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;

    let session_id = "session-recover";
    let cwd = temp.path().to_string_lossy();
    let tool = format!(
        r#"{{"hook_event_name":"PostToolUse","session_id":"{session_id}","cwd":"{cwd}","tool_name":"Read"}}"#
    );

    let out = run_hook(&temp, "tool-use", &tool);
    assert!(out.status.success());

    let session_path = session_file(&temp, session_id);
    let session_json = fs::read_to_string(session_path).unwrap();
    let session: serde_json::Value = serde_json::from_str(&session_json).unwrap();

    assert_eq!(
        session["metadata"]["recovered_from_out_of_order"],
        serde_json::json!(true)
    );
}

#[tokio::test]
#[serial]
async fn test_claude_code_duplicate_event_dedup() {
    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;

    let session_id = "session-dedup";
    let cwd = temp.path().to_string_lossy();
    let start = format!(
        r#"{{"hook_event_name":"SessionStart","session_id":"{session_id}","cwd":"{cwd}"}}"#
    );
    let prompt = format!(
        r#"{{"hook_event_name":"UserPromptSubmit","session_id":"{session_id}","cwd":"{cwd}","prompt":"hello"}}"#
    );

    assert!(run_hook(&temp, "session-start", &start).status.success());
    assert!(run_hook(&temp, "prompt", &prompt).status.success());
    assert!(run_hook(&temp, "prompt", &prompt).status.success());

    let session_path = session_file(&temp, session_id);
    let session_json = fs::read_to_string(session_path).unwrap();
    let session: serde_json::Value = serde_json::from_str(&session_json).unwrap();

    let messages = session["messages"].as_array().unwrap();
    let user_count = messages
        .iter()
        .filter(|m| m["role"] == "user" && m["content"] == "hello")
        .count();
    assert_eq!(user_count, 1, "duplicate prompt should be ignored");
}

#[tokio::test]
#[serial]
async fn test_claude_code_repeat_session_end_is_idempotent() {
    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;

    let session_id = "session-end-repeat";
    let cwd = temp.path().to_string_lossy();

    let start = format!(
        r#"{{"hook_event_name":"SessionStart","session_id":"{session_id}","cwd":"{cwd}"}}"#
    );
    let end =
        format!(r#"{{"hook_event_name":"SessionEnd","session_id":"{session_id}","cwd":"{cwd}"}}"#);

    assert!(run_hook(&temp, "session-start", &start).status.success());
    assert!(run_hook(&temp, "session-end", &end).status.success());
    let history_manager = build_history_manager(&temp).await;
    let head_after_first = history_manager.resolve_history_head().await.unwrap();
    assert!(run_hook(&temp, "session-end", &end).status.success());
    let head_after_second = history_manager.resolve_history_head().await.unwrap();

    let session_path = session_file(&temp, session_id);
    assert!(
        !session_path.exists(),
        "session file should be removed after successful persistence"
    );
    assert_eq!(
        head_after_first, head_after_second,
        "repeated SessionEnd should not create extra history commits"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_code_stop_accepts_session_stop_event_name() {
    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;

    let session_id = "session-stop-alias";
    let cwd = temp.path().to_string_lossy();

    let start = format!(
        r#"{{"hook_event_name":"SessionStart","session_id":"{session_id}","cwd":"{cwd}"}}"#
    );
    let stop_alias = format!(
        r#"{{"hook_event_name":"SessionStop","session_id":"{session_id}","cwd":"{cwd}","last_assistant_message":"done"}}"#
    );

    assert!(run_hook(&temp, "session-start", &start).status.success());
    assert!(run_hook(&temp, "stop", &stop_alias).status.success());

    let session_path = session_file(&temp, session_id);
    let session_json = fs::read_to_string(session_path).unwrap();
    let session: serde_json::Value = serde_json::from_str(&session_json).unwrap();
    assert_eq!(
        session["metadata"]["claude_session_phase"],
        serde_json::json!("Stopped")
    );
    assert_eq!(
        session["metadata"]["last_assistant_message"],
        serde_json::json!("done")
    );
}
