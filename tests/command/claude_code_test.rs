//! Integration tests for `claude-code` hook ingestion command.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use libra::{
    command::init::{InitArgs, init},
    internal::ai::history::HistoryManager,
    utils::test,
};
use serde_json::json;
use serial_test::serial;
use tempfile::tempdir;

fn run_hook_in(workdir: &Path, subcmd: &str, payload: &str) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_libra"));
    cmd.current_dir(workdir)
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

fn run_hook(temp: &tempfile::TempDir, subcmd: &str, payload: &str) -> std::process::Output {
    run_hook_in(temp.path(), subcmd, payload)
}

fn session_file(repo_root: &Path, id: &str) -> PathBuf {
    repo_root
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
    let cwd = temp.path().to_string_lossy().to_string();

    let events = vec![
        (
            "session-start",
            json!({
                "hook_event_name": "SessionStart",
                "session_id": session_id,
                "cwd": cwd,
                "model": "claude-3",
                "source": "claude-code"
            })
            .to_string(),
        ),
        (
            "prompt",
            json!({
                "hook_event_name": "UserPromptSubmit",
                "session_id": session_id,
                "cwd": cwd,
                "prompt": "hello"
            })
            .to_string(),
        ),
        (
            "tool-use",
            json!({
                "hook_event_name": "PostToolUse",
                "session_id": session_id,
                "cwd": cwd,
                "tool_name": "Read",
                "tool_input": {"path": "a.txt"},
                "tool_response": {"ok": true}
            })
            .to_string(),
        ),
        (
            "stop",
            json!({
                "hook_event_name": "Stop",
                "session_id": session_id,
                "cwd": cwd,
                "last_assistant_message": "done"
            })
            .to_string(),
        ),
        (
            "session-end",
            json!({
                "hook_event_name": "SessionEnd",
                "session_id": session_id,
                "cwd": cwd
            })
            .to_string(),
        ),
    ];

    for (subcmd, payload) in events {
        let out = run_hook(&temp, subcmd, &payload);
        assert!(
            out.status.success(),
            "{subcmd} failed, stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let session_path = session_file(temp.path(), session_id);
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
async fn test_claude_code_uses_repo_object_format_for_persistence() {
    let temp = tempdir().unwrap();
    test::setup_clean_testing_env_in(temp.path());
    let _guard = test::ChangeDirGuard::new(temp.path());
    init(InitArgs {
        bare: false,
        initial_branch: Some("main".to_string()),
        template: None,
        repo_directory: temp.path().to_string_lossy().to_string(),
        quiet: true,
        shared: None,
        separate_libra_dir: None,
        object_format: Some("sha256".to_string()),
        ref_format: None,
        from_git_repository: None,
    })
    .await
    .unwrap();

    let session_id = "session-sha256";
    let cwd = temp.path().to_string_lossy().to_string();
    let start = json!({
        "hook_event_name": "SessionStart",
        "session_id": session_id,
        "cwd": cwd
    })
    .to_string();
    let end = json!({
        "hook_event_name": "SessionEnd",
        "session_id": session_id,
        "cwd": cwd
    })
    .to_string();

    assert!(run_hook(&temp, "session-start", &start).status.success());
    assert!(run_hook(&temp, "session-end", &end).status.success());

    let history_manager = build_history_manager(&temp).await;
    let object_hash = history_manager
        .get_object_hash("claude_session", session_id)
        .await
        .unwrap()
        .expect("claude_session object should exist");
    assert_eq!(
        object_hash.to_string().len(),
        64,
        "sha256 repo should persist 64-char object hash"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_code_out_of_order_recovers() {
    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;

    let session_id = "session-recover";
    let cwd = temp.path().to_string_lossy().to_string();
    let tool = json!({
        "hook_event_name": "PostToolUse",
        "session_id": session_id,
        "cwd": cwd,
        "tool_name": "Read"
    })
    .to_string();

    let out = run_hook(&temp, "tool-use", &tool);
    assert!(out.status.success());

    let session_path = session_file(temp.path(), session_id);
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
    let cwd = temp.path().to_string_lossy().to_string();
    let start = json!({
        "hook_event_name": "SessionStart",
        "session_id": session_id,
        "cwd": cwd
    })
    .to_string();
    let prompt = json!({
        "hook_event_name": "UserPromptSubmit",
        "session_id": session_id,
        "cwd": cwd,
        "event_id": "evt-1",
        "prompt": "hello"
    })
    .to_string();

    assert!(run_hook(&temp, "session-start", &start).status.success());
    assert!(run_hook(&temp, "prompt", &prompt).status.success());
    assert!(run_hook(&temp, "prompt", &prompt).status.success());

    let session_path = session_file(temp.path(), session_id);
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
async fn test_claude_code_repeated_payload_without_identity_is_not_deduped() {
    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;

    let session_id = "session-no-identity";
    let cwd = temp.path().to_string_lossy().to_string();
    let start = json!({
        "hook_event_name": "SessionStart",
        "session_id": session_id,
        "cwd": cwd
    })
    .to_string();
    let prompt = json!({
        "hook_event_name": "UserPromptSubmit",
        "session_id": session_id,
        "cwd": cwd,
        "prompt": "hello"
    })
    .to_string();

    assert!(run_hook(&temp, "session-start", &start).status.success());
    assert!(run_hook(&temp, "prompt", &prompt).status.success());
    assert!(run_hook(&temp, "prompt", &prompt).status.success());

    let session_path = session_file(temp.path(), session_id);
    let session_json = fs::read_to_string(session_path).unwrap();
    let session: serde_json::Value = serde_json::from_str(&session_json).unwrap();

    let messages = session["messages"].as_array().unwrap();
    let user_count = messages
        .iter()
        .filter(|m| m["role"] == "user" && m["content"] == "hello")
        .count();
    assert_eq!(
        user_count, 2,
        "without identity, duplicate payload must be kept"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_code_lifecycle_without_identity_is_deduped() {
    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;

    let session_id = "session-lifecycle-dedup";
    let cwd = temp.path().to_string_lossy().to_string();
    let start = json!({
        "hook_event_name": "SessionStart",
        "session_id": session_id,
        "cwd": cwd
    })
    .to_string();

    assert!(run_hook(&temp, "session-start", &start).status.success());
    assert!(run_hook(&temp, "session-start", &start).status.success());

    let session_path = session_file(temp.path(), session_id);
    let session_json = fs::read_to_string(session_path).unwrap();
    let session: serde_json::Value = serde_json::from_str(&session_json).unwrap();

    let raw_events = session["metadata"]["raw_hook_events"].as_array().unwrap();
    assert_eq!(
        raw_events.len(),
        1,
        "duplicate lifecycle event should be deduped even without identity"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_code_repeat_session_end_is_idempotent() {
    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;

    let session_id = "session-end-repeat";
    let cwd = temp.path().to_string_lossy().to_string();

    let start = json!({
        "hook_event_name": "SessionStart",
        "session_id": session_id,
        "cwd": cwd
    })
    .to_string();
    let end = json!({
        "hook_event_name": "SessionEnd",
        "session_id": session_id,
        "cwd": cwd
    })
    .to_string();

    assert!(run_hook(&temp, "session-start", &start).status.success());
    assert!(run_hook(&temp, "session-end", &end).status.success());
    let history_manager = build_history_manager(&temp).await;
    let head_after_first = history_manager.resolve_history_head().await.unwrap();
    assert!(run_hook(&temp, "session-end", &end).status.success());
    let head_after_second = history_manager.resolve_history_head().await.unwrap();

    let session_path = session_file(temp.path(), session_id);
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
    let cwd = temp.path().to_string_lossy().to_string();

    let start = json!({
        "hook_event_name": "SessionStart",
        "session_id": session_id,
        "cwd": cwd
    })
    .to_string();
    let stop_alias = json!({
        "hook_event_name": "SessionStop",
        "session_id": session_id,
        "cwd": cwd,
        "last_assistant_message": "done"
    })
    .to_string();

    assert!(run_hook(&temp, "session-start", &start).status.success());
    assert!(run_hook(&temp, "stop", &stop_alias).status.success());

    let session_path = session_file(temp.path(), session_id);
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

#[tokio::test]
#[serial]
async fn test_claude_code_uses_repo_root_session_storage_from_subdir() {
    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;

    let nested = temp.path().join("nested").join("deeper");
    fs::create_dir_all(&nested).unwrap();

    let session_id = "session-subdir";
    let cwd = nested.to_string_lossy().to_string();
    let start = json!({
        "hook_event_name": "SessionStart",
        "session_id": session_id,
        "cwd": cwd
    })
    .to_string();
    let end = json!({
        "hook_event_name": "SessionEnd",
        "session_id": session_id,
        "cwd": cwd
    })
    .to_string();

    assert!(
        run_hook_in(&nested, "session-start", &start)
            .status
            .success()
    );
    let root_session_path = session_file(temp.path(), session_id);
    assert!(
        root_session_path.exists(),
        "session should be stored at repo root"
    );
    assert!(
        !nested.join(".libra").exists(),
        "subdir must not create nested .libra directory"
    );

    assert!(run_hook_in(&nested, "session-end", &end).status.success());
    assert!(
        !root_session_path.exists(),
        "session file should be cleaned after successful persistence"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_code_sets_hook_cwd_mismatch_metadata() {
    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;

    let session_id = "session-cwd-mismatch";
    let payload = json!({
        "hook_event_name": "SessionStart",
        "session_id": session_id,
        "cwd": "/mismatch/path"
    })
    .to_string();

    assert!(run_hook(&temp, "session-start", &payload).status.success());
    let session_json = fs::read_to_string(session_file(temp.path(), session_id)).unwrap();
    let session: serde_json::Value = serde_json::from_str(&session_json).unwrap();

    assert_eq!(session["metadata"]["hook_cwd_mismatch"], json!(true));
    assert_eq!(
        session["metadata"]["hook_reported_cwd"],
        json!("/mismatch/path")
    );
}

#[test]
fn test_claude_code_rejects_empty_stdin() {
    let temp = tempdir().unwrap();
    let out = run_hook(&temp, "session-start", "");
    assert!(
        !out.status.success(),
        "invalid input should return non-zero exit status"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("hook input is empty"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn test_claude_code_rejects_invalid_json() {
    let temp = tempdir().unwrap();
    let out = run_hook(&temp, "session-start", "{invalid");
    assert!(
        !out.status.success(),
        "invalid input should return non-zero exit status"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("invalid hook JSON payload"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn test_claude_code_rejects_missing_required_field() {
    let temp = tempdir().unwrap();
    let out = run_hook(
        &temp,
        "session-start",
        &json!({
            "hook_event_name": "SessionStart",
            "cwd": "/tmp"
        })
        .to_string(),
    );
    assert!(
        !out.status.success(),
        "invalid input should return non-zero exit status"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("missing required field: session_id")
            || stderr.contains("missing field `session_id`"),
        "stderr: {stderr}"
    );
}

#[test]
fn test_claude_code_rejects_oversized_stdin() {
    let temp = tempdir().unwrap();
    let huge = "x".repeat(1_048_577);
    let out = run_hook(&temp, "session-start", &huge);
    assert!(
        !out.status.success(),
        "invalid input should return non-zero exit status"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("hook input exceeds 1048576 bytes"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[tokio::test]
#[serial]
async fn test_claude_code_session_end_persist_failure_returns_error() {
    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;

    let session_id = "session-persist-fail";
    let cwd = temp.path().to_string_lossy().to_string();
    let start = json!({
        "hook_event_name": "SessionStart",
        "session_id": session_id,
        "cwd": cwd
    })
    .to_string();
    let end = json!({
        "hook_event_name": "SessionEnd",
        "session_id": session_id,
        "cwd": cwd
    })
    .to_string();
    assert!(run_hook(&temp, "session-start", &start).status.success());

    let objects_path = temp.path().join(".libra").join("objects");
    fs::remove_dir_all(&objects_path).unwrap();
    fs::write(&objects_path, "not-a-directory").unwrap();

    let out = run_hook(&temp, "session-end", &end);
    assert!(
        !out.status.success(),
        "persistence failure should return non-zero exit status"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("failed to persist session history"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let session_json = fs::read_to_string(session_file(temp.path(), session_id)).unwrap();
    let session: serde_json::Value = serde_json::from_str(&session_json).unwrap();
    assert_eq!(session["metadata"]["persist_failed"], json!(true));
    assert_eq!(session["metadata"]["persisted"], json!(false));
}
