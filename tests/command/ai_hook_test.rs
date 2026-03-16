//! Integration tests for the unified `hooks` ingestion surface.

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

fn run_hook_in(
    workdir: &Path,
    provider: &str,
    subcmd: &str,
    payload: &str,
) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_libra"));
    cmd.current_dir(workdir)
        .arg("hooks")
        .arg(provider)
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

fn run_hook(
    temp: &tempfile::TempDir,
    provider: &str,
    subcmd: &str,
    payload: &str,
) -> std::process::Output {
    run_hook_in(temp.path(), provider, subcmd, payload)
}

fn ai_session_id(provider: &str, provider_session_id: &str) -> String {
    format!("{provider}__{provider_session_id}")
}

fn session_file(repo_root: &Path, provider: &str, id: &str) -> PathBuf {
    repo_root
        .join(".libra")
        .join("sessions")
        .join(format!("{}.json", ai_session_id(provider, id)))
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

async fn assert_basic_flow_and_persisted(provider: &str) {
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
                "cwd": cwd
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
        let out = run_hook(&temp, provider, subcmd, &payload);
        assert!(
            out.status.success(),
            "{subcmd} failed, stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let session_path = session_file(temp.path(), provider, session_id);
    assert!(
        !session_path.exists(),
        "session file should be removed after successful persistence"
    );

    let history_manager = build_history_manager(&temp).await;
    let head = history_manager.resolve_history_head().await.unwrap();
    assert!(head.is_some(), "expected libra/intent ref to exist");

    let ai_object_id = ai_session_id(provider, session_id);
    let object_hash = history_manager
        .get_object_hash("ai_session", &ai_object_id)
        .await
        .unwrap();
    assert!(object_hash.is_some(), "ai_session object should exist");

    let ai_type = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp.path())
        .args(["cat-file", "--ai-type", &ai_object_id])
        .output()
        .expect("failed to execute cat-file --ai-type");
    assert!(ai_type.status.success());
    assert_eq!(
        String::from_utf8_lossy(&ai_type.stdout).trim(),
        "ai_session"
    );

    let ai_pretty = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp.path())
        .args(["cat-file", "--ai", &ai_object_id])
        .output()
        .expect("failed to execute cat-file --ai");
    assert!(ai_pretty.status.success());
    let ai_pretty_stdout = String::from_utf8_lossy(&ai_pretty.stdout);
    assert!(ai_pretty_stdout.contains("type: ai_session"));
    assert!(ai_pretty_stdout.contains("schema: libra.ai_session.v2"));
    assert!(ai_pretty_stdout.contains(&format!("provider: {provider}")));
    assert!(ai_pretty_stdout.contains("phase: ended"));
    assert!(ai_pretty_stdout.contains("transcript_raw_event_count"));
    assert!(ai_pretty_stdout.contains(&format!("\"provider\": \"{provider}\"")));

    let ai_list = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp.path())
        .args(["cat-file", "--ai-list", "ai_session"])
        .output()
        .expect("failed to execute cat-file --ai-list");
    assert!(ai_list.status.success());
    assert!(String::from_utf8_lossy(&ai_list.stdout).contains(&ai_object_id));

    let ai_list_types = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp.path())
        .args(["cat-file", "--ai-list-types"])
        .output()
        .expect("failed to execute cat-file --ai-list-types");
    assert!(ai_list_types.status.success());
    assert!(String::from_utf8_lossy(&ai_list_types.stdout).contains("ai_session"));
}

#[tokio::test]
#[serial]
async fn test_ai_hooks_normal_flow_and_persisted() {
    assert_basic_flow_and_persisted("gemini").await;
    assert_basic_flow_and_persisted("claude").await;
}

#[tokio::test]
#[serial]
async fn test_ai_hooks_use_repo_object_format_for_persistence() {
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
        vault: false,
    })
    .await
    .unwrap();

    let provider = "gemini";
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

    assert!(
        run_hook(&temp, provider, "session-start", &start)
            .status
            .success()
    );
    assert!(
        run_hook(&temp, provider, "session-end", &end)
            .status
            .success()
    );

    let history_manager = build_history_manager(&temp).await;
    let object_hash = history_manager
        .get_object_hash("ai_session", &ai_session_id(provider, session_id))
        .await
        .unwrap()
        .expect("ai_session object should exist");
    assert_eq!(
        object_hash.to_string().len(),
        64,
        "sha256 repo should persist 64-char object hash"
    );
}

#[tokio::test]
#[serial]
async fn test_ai_hooks_out_of_order_recovers() {
    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;

    let provider = "gemini";
    let session_id = "session-recover";
    let cwd = temp.path().to_string_lossy().to_string();
    let tool = json!({
        "hook_event_name": "PostToolUse",
        "session_id": session_id,
        "cwd": cwd,
        "tool_name": "Read"
    })
    .to_string();

    let out = run_hook(&temp, provider, "tool-use", &tool);
    assert!(out.status.success());

    let session_json = fs::read_to_string(session_file(temp.path(), provider, session_id)).unwrap();
    let session: serde_json::Value = serde_json::from_str(&session_json).unwrap();
    assert_eq!(
        session["metadata"]["recovered_from_out_of_order"],
        serde_json::json!(true)
    );
}

#[tokio::test]
#[serial]
async fn test_ai_hooks_duplicate_event_dedup() {
    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;

    let provider = "gemini";
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

    assert!(
        run_hook(&temp, provider, "session-start", &start)
            .status
            .success()
    );
    assert!(
        run_hook(&temp, provider, "prompt", &prompt)
            .status
            .success()
    );
    assert!(
        run_hook(&temp, provider, "prompt", &prompt)
            .status
            .success()
    );

    let session_json = fs::read_to_string(session_file(temp.path(), provider, session_id)).unwrap();
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
async fn test_ai_hooks_repeated_payload_without_identity_is_not_deduped() {
    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;

    let provider = "gemini";
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

    assert!(
        run_hook(&temp, provider, "session-start", &start)
            .status
            .success()
    );
    assert!(
        run_hook(&temp, provider, "prompt", &prompt)
            .status
            .success()
    );
    assert!(
        run_hook(&temp, provider, "prompt", &prompt)
            .status
            .success()
    );

    let session_json = fs::read_to_string(session_file(temp.path(), provider, session_id)).unwrap();
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
async fn test_ai_hooks_lifecycle_without_identity_is_deduped() {
    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;

    let provider = "gemini";
    let session_id = "session-lifecycle-dedup";
    let cwd = temp.path().to_string_lossy().to_string();
    let start = json!({
        "hook_event_name": "SessionStart",
        "session_id": session_id,
        "cwd": cwd
    })
    .to_string();

    assert!(
        run_hook(&temp, provider, "session-start", &start)
            .status
            .success()
    );
    assert!(
        run_hook(&temp, provider, "session-start", &start)
            .status
            .success()
    );

    let session_json = fs::read_to_string(session_file(temp.path(), provider, session_id)).unwrap();
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
async fn test_ai_hooks_repeat_session_end_is_idempotent() {
    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;

    let provider = "gemini";
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

    assert!(
        run_hook(&temp, provider, "session-start", &start)
            .status
            .success()
    );
    assert!(
        run_hook(&temp, provider, "session-end", &end)
            .status
            .success()
    );
    let history_manager = build_history_manager(&temp).await;
    let head_after_first = history_manager.resolve_history_head().await.unwrap();

    assert!(
        run_hook(&temp, provider, "session-end", &end)
            .status
            .success()
    );
    let head_after_second = history_manager.resolve_history_head().await.unwrap();

    assert!(
        !session_file(temp.path(), provider, session_id).exists(),
        "session file should be removed after successful persistence"
    );
    assert_eq!(
        head_after_first, head_after_second,
        "repeated SessionEnd should not create extra history commits"
    );
}

#[tokio::test]
#[serial]
async fn test_ai_hooks_stop_accepts_session_stop_event_name() {
    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;

    let provider = "gemini";
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

    assert!(
        run_hook(&temp, provider, "session-start", &start)
            .status
            .success()
    );
    assert!(
        run_hook(&temp, provider, "stop", &stop_alias)
            .status
            .success()
    );

    let session_json = fs::read_to_string(session_file(temp.path(), provider, session_id)).unwrap();
    let session: serde_json::Value = serde_json::from_str(&session_json).unwrap();
    assert_eq!(
        session["metadata"]["session_phase"],
        serde_json::json!("stopped")
    );
    assert_eq!(
        session["metadata"]["last_assistant_message"],
        serde_json::json!("done")
    );
}

#[tokio::test]
#[serial]
async fn test_ai_hooks_use_repo_root_session_storage_from_subdir() {
    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;

    let provider = "gemini";
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
        run_hook_in(&nested, provider, "session-start", &start)
            .status
            .success()
    );

    let root_session_path = session_file(temp.path(), provider, session_id);
    assert!(
        root_session_path.exists(),
        "session should be stored at repo root"
    );
    assert!(
        !nested.join(".libra").exists(),
        "subdir must not create nested .libra directory"
    );

    assert!(
        run_hook_in(&nested, provider, "session-end", &end)
            .status
            .success()
    );
    assert!(
        !root_session_path.exists(),
        "session file should be cleaned after successful persistence"
    );
}

#[tokio::test]
#[serial]
async fn test_ai_hooks_set_hook_cwd_mismatch_metadata() {
    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;

    let provider = "gemini";
    let session_id = "session-cwd-mismatch";
    let payload = json!({
        "hook_event_name": "SessionStart",
        "session_id": session_id,
        "cwd": "/mismatch/path"
    })
    .to_string();

    assert!(
        run_hook(&temp, provider, "session-start", &payload)
            .status
            .success()
    );
    let session_json = fs::read_to_string(session_file(temp.path(), provider, session_id)).unwrap();
    let session: serde_json::Value = serde_json::from_str(&session_json).unwrap();

    assert_eq!(session["metadata"]["hook_cwd_mismatch"], json!(true));
    assert_eq!(
        session["metadata"]["hook_reported_cwd"],
        json!("/mismatch/path")
    );
}

#[tokio::test]
#[serial]
async fn test_ai_hooks_recover_from_corrupt_session_file() {
    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;

    let provider = "gemini";
    let session_id = "session-corrupt";
    let ai_id = ai_session_id(provider, session_id);
    let sessions_dir = temp.path().join(".libra").join("sessions");
    fs::create_dir_all(&sessions_dir).unwrap();
    let corrupt_path = sessions_dir.join(format!("{ai_id}.json"));
    fs::write(&corrupt_path, "{\n  \"id\": \"broken\"\n}\n}").unwrap();

    let payload = json!({
        "hook_event_name": "SessionStart",
        "session_id": session_id,
        "cwd": temp.path().to_string_lossy().to_string()
    })
    .to_string();

    let out = run_hook(&temp, provider, "session-start", &payload);
    assert!(
        out.status.success(),
        "hook should recover from malformed session, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let repaired_json =
        fs::read_to_string(session_file(temp.path(), provider, session_id)).unwrap();
    let repaired: serde_json::Value = serde_json::from_str(&repaired_json).unwrap();
    assert_eq!(
        repaired["metadata"]["recovered_from_corrupt_session"],
        json!(true)
    );
    assert!(
        repaired["metadata"]["recovery_error"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );

    let backup_count = fs::read_dir(&sessions_dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| name.starts_with(&format!("{ai_id}.corrupt.")) && name.ends_with(".json"))
        .count();
    assert_eq!(
        backup_count, 1,
        "expected one archived backup for malformed session file"
    );
}

#[tokio::test]
#[serial]
async fn test_ai_hooks_concurrent_events_do_not_corrupt_session_file() {
    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;

    let provider = "gemini";
    let session_id = "session-concurrent-hooks";
    let cwd = temp.path().to_string_lossy().to_string();
    let start = json!({
        "hook_event_name": "SessionStart",
        "session_id": session_id,
        "cwd": cwd,
        "source": "startup"
    })
    .to_string();
    let out = run_hook(&temp, provider, "session-start", &start);
    assert!(
        out.status.success(),
        "session-start failed, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let jobs_per_type = 8usize;
    let total_jobs = jobs_per_type * 2;
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(total_jobs));
    let mut handles = Vec::with_capacity(total_jobs);

    for index in 0..jobs_per_type {
        let workdir = temp.path().to_path_buf();
        let barrier_clone = std::sync::Arc::clone(&barrier);
        let payload = json!({
            "hook_event_name": "BeforeModel",
            "session_id": session_id,
            "cwd": cwd,
            "sequence": index,
            "llm_request": { "model": format!("gemini-2.5-pro-{index}") }
        })
        .to_string();
        handles.push(std::thread::spawn(move || {
            barrier_clone.wait();
            run_hook_in(&workdir, provider, "model-update", &payload)
        }));
    }

    for index in 0..jobs_per_type {
        let workdir = temp.path().to_path_buf();
        let barrier_clone = std::sync::Arc::clone(&barrier);
        let payload = json!({
            "hook_event_name": "PreCompress",
            "session_id": session_id,
            "cwd": cwd,
            "sequence": jobs_per_type + index
        })
        .to_string();
        handles.push(std::thread::spawn(move || {
            barrier_clone.wait();
            run_hook_in(&workdir, provider, "compaction", &payload)
        }));
    }

    for handle in handles {
        let output = handle.join().expect("concurrent hook thread panicked");
        assert!(
            output.status.success(),
            "concurrent hook failed, stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let session_path = session_file(temp.path(), provider, session_id);
    let session_json = fs::read_to_string(&session_path).unwrap();
    let session: serde_json::Value = serde_json::from_str(&session_json).unwrap();
    assert!(
        session["metadata"]
            .get("recovered_from_corrupt_session")
            .is_none(),
        "session file should not be corrupted under concurrent hook writes"
    );
    assert_eq!(
        session["metadata"]["compaction_count"],
        json!(jobs_per_type as u64)
    );

    let sessions_dir = temp.path().join(".libra").join("sessions");
    let backup_count = fs::read_dir(&sessions_dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| {
            name.starts_with(&format!("{}.corrupt.", ai_session_id(provider, session_id)))
                && name.ends_with(".json")
        })
        .count();
    assert_eq!(backup_count, 0, "no corrupt backup should be generated");

    let end = json!({
        "hook_event_name": "SessionEnd",
        "session_id": session_id,
        "cwd": cwd
    })
    .to_string();
    let out = run_hook(&temp, provider, "session-end", &end);
    assert!(
        out.status.success(),
        "session-end failed, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !session_path.exists(),
        "session file should be cleaned after persistence"
    );

    let history_manager = build_history_manager(&temp).await;
    let object_hash = history_manager
        .get_object_hash("ai_session", &ai_session_id(provider, session_id))
        .await
        .unwrap();
    assert!(
        object_hash.is_some(),
        "ai_session object should exist after concurrent hook ingestion"
    );
}

#[test]
fn test_ai_hooks_reject_empty_stdin() {
    let temp = tempdir().unwrap();
    let out = run_hook(&temp, "gemini", "session-start", "");
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
fn test_ai_hooks_reject_invalid_json() {
    let temp = tempdir().unwrap();
    let out = run_hook(&temp, "gemini", "session-start", "{invalid");
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
fn test_ai_hooks_reject_missing_required_field() {
    let temp = tempdir().unwrap();
    let out = run_hook(
        &temp,
        "gemini",
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
fn test_ai_hooks_reject_oversized_stdin() {
    let temp = tempdir().unwrap();
    let huge = "x".repeat(1_048_577);
    let out = run_hook(&temp, "gemini", "session-start", &huge);
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
async fn test_ai_hooks_session_end_persist_failure_returns_error() {
    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;

    let provider = "gemini";
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

    assert!(
        run_hook(&temp, provider, "session-start", &start)
            .status
            .success()
    );

    let objects_path = temp.path().join(".libra").join("objects");
    fs::remove_dir_all(&objects_path).unwrap();
    fs::write(&objects_path, "not-a-directory").unwrap();

    let out = run_hook(&temp, provider, "session-end", &end);
    assert!(
        !out.status.success(),
        "persistence failure should return non-zero exit status"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("failed to persist session history"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let session_json = fs::read_to_string(session_file(temp.path(), provider, session_id)).unwrap();
    let session: serde_json::Value = serde_json::from_str(&session_json).unwrap();
    assert_eq!(session["metadata"]["persist_failed"], json!(true));
    assert_eq!(session["metadata"]["persisted"], json!(false));
}
