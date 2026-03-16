use std::{
    fs,
    path::{Path, PathBuf},
    process::Output,
    sync::Arc,
};

use git_internal::internal::object::intent::Intent;
use libra::{
    internal::ai::history::HistoryManager,
    utils::{storage::local::LocalStorage, storage_ext::StorageExt, test},
};
use serde_json::{Value, json};
use serial_test::serial;
use tempfile::tempdir;

use super::{assert_cli_success, run_libra_command};

const PROBE_LIKE_ARTIFACT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/data/ai/claude_managed_probe_like.json"
));
const SEMANTIC_FULL_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/data/ai/claude_managed_semantic_full_template.json"
));
const PLAN_TASK_ONLY_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/data/ai/claude_managed_plan_task_only_template.json"
));
const PLAN_PROMPT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/data/ai/claude_sdk_plan_prompt.txt"
));

const DEFAULT_MANAGED_PROMPT: &str = "Bridge a managed Claude SDK session into Libra artifacts.";

fn parse_stdout_json(output: &Output, context: &str) -> Value {
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|err| panic!("{context}: failed to parse stdout JSON: {err}"))
}

fn read_json_file(path: &Path) -> Value {
    let body = fs::read_to_string(path)
        .unwrap_or_else(|err| panic!("failed to read JSON file '{}': {err}", path.display()));
    serde_json::from_str(&body)
        .unwrap_or_else(|err| panic!("failed to parse JSON file '{}': {err}", path.display()))
}

fn write_shell_helper(path: &Path, artifact_path: &Path) {
    let artifact_rendered = artifact_path.to_string_lossy().replace('\'', r#"'\''"#);
    let script = format!("#!/bin/sh\ncat '{artifact_rendered}'\n");
    fs::write(path, script)
        .unwrap_or_else(|err| panic!("failed to write helper script '{}': {err}", path.display()));
}

fn write_request_capture_shell_helper(path: &Path, artifact_path: &Path, request_path: &Path) {
    let artifact_rendered = artifact_path.to_string_lossy().replace('\'', r#"'\''"#);
    let request_rendered = request_path.to_string_lossy().replace('\'', r#"'\''"#);
    let script = format!("#!/bin/sh\ncat > '{request_rendered}'\ncat '{artifact_rendered}'\n");
    fs::write(path, script)
        .unwrap_or_else(|err| panic!("failed to write helper script '{}': {err}", path.display()));
}

fn write_json_response_capture_shell_helper(
    path: &Path,
    response_path: &Path,
    request_path: &Path,
) {
    let response_rendered = response_path.to_string_lossy().replace('\'', r#"'\''"#);
    let request_rendered = request_path.to_string_lossy().replace('\'', r#"'\''"#);
    let script = format!("#!/bin/sh\ncat > '{request_rendered}'\ncat '{response_rendered}'\n");
    fs::write(path, script)
        .unwrap_or_else(|err| panic!("failed to write helper script '{}': {err}", path.display()));
}

fn replace_template_slots(node: &mut Value, replacements: &[(&str, Value)]) {
    match node {
        Value::Array(items) => {
            for item in items {
                replace_template_slots(item, replacements);
            }
        }
        Value::Object(map) => {
            for value in map.values_mut() {
                replace_template_slots(value, replacements);
            }
        }
        Value::String(slot) => {
            if let Some((_, replacement)) = replacements.iter().find(|(key, _)| *key == slot) {
                *node = replacement.clone();
            }
        }
        _ => {}
    }
}

fn managed_artifact_from_template(
    template: &str,
    repo: &Path,
    touched_file: &Path,
    prompt: &str,
) -> Value {
    let mut artifact: Value = serde_json::from_str(template)
        .unwrap_or_else(|err| panic!("failed to parse managed artifact template: {err}"));
    let replacements = [
        ("__CWD__", json!(repo.to_string_lossy().to_string())),
        (
            "__TOUCHED_FILE__",
            json!(touched_file.to_string_lossy().to_string()),
        ),
        ("__PROMPT__", json!(prompt)),
    ];
    replace_template_slots(&mut artifact, &replacements);
    artifact
}

fn semantic_full_artifact(repo: &Path, touched_file: &Path) -> Value {
    managed_artifact_from_template(
        SEMANTIC_FULL_TEMPLATE,
        repo,
        touched_file,
        DEFAULT_MANAGED_PROMPT,
    )
}

fn plan_task_only_artifact(repo: &Path, touched_file: &Path) -> Value {
    managed_artifact_from_template(PLAN_TASK_ONLY_TEMPLATE, repo, touched_file, PLAN_PROMPT)
}

fn timed_out_partial_artifact(repo: &Path, touched_file: &Path) -> Value {
    let mut artifact = semantic_full_artifact(repo, touched_file);
    let object = artifact
        .as_object_mut()
        .expect("semantic full artifact should be an object");
    object.insert("helperTimedOut".to_string(), json!(true));
    object.insert(
        "helperError".to_string(),
        json!("Claude SDK helper timed out"),
    );
    object.insert("resultMessage".to_string(), Value::Null);
    artifact
}

async fn load_intent_history(repo: &Path) -> (Arc<LocalStorage>, HistoryManager) {
    let libra_dir = repo.join(".libra");
    let storage = Arc::new(LocalStorage::new(libra_dir.join("objects")));
    let db_conn = Arc::new(
        libra::internal::db::establish_connection(
            libra_dir
                .join("libra.db")
                .to_str()
                .expect("db path should be valid UTF-8"),
        )
        .await
        .expect("failed to connect test database"),
    );
    let history = HistoryManager::new(storage.clone(), libra_dir, db_conn);
    (storage, history)
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_import_persists_bridge_artifacts_and_is_idempotent() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let artifact_path = repo.path().join("probe-like-artifact.json");
    fs::write(&artifact_path, PROBE_LIKE_ARTIFACT).expect("failed to stage managed artifact");

    let first = run_libra_command(
        &[
            "claude-sdk",
            "import",
            "--artifact",
            artifact_path.to_str().expect("artifact path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&first, "claude-sdk import should succeed");
    let first_json = parse_stdout_json(&first, "first import");

    assert_eq!(first_json["ok"], json!(true));
    assert_eq!(first_json["mode"], json!("import"));
    assert_eq!(first_json["alreadyPersisted"], json!(false));
    assert!(
        first_json["intentExtractionPath"].is_null(),
        "probe-like fixture should not yield an intent extraction"
    );

    let ai_session_id = first_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present");
    let raw_artifact_path = PathBuf::from(
        first_json["rawArtifactPath"]
            .as_str()
            .expect("rawArtifactPath"),
    );
    let audit_bundle_path = PathBuf::from(
        first_json["auditBundlePath"]
            .as_str()
            .expect("auditBundlePath"),
    );

    assert!(
        raw_artifact_path.exists(),
        "raw artifact should be materialized"
    );
    assert!(
        audit_bundle_path.exists(),
        "audit bundle should be materialized"
    );

    let audit_bundle = read_json_file(&audit_bundle_path);
    assert_eq!(
        audit_bundle["bridge"]["intentExtraction"]["status"],
        json!("invalid")
    );
    assert_eq!(
        audit_bundle["bridge"]["objectCandidates"]["runSnapshot"]["id"],
        json!(format!("{ai_session_id}::run"))
    );
    assert_eq!(
        audit_bundle["bridge"]["objectCandidates"]["runEvent"]["status"],
        json!("completed")
    );
    assert_eq!(
        audit_bundle["bridge"]["objectCandidates"]["provenanceSnapshot"]["provider"],
        json!("claude")
    );
    assert_eq!(
        audit_bundle["bridge"]["aiSession"]["schema"],
        json!("libra.ai_session.v2")
    );

    let ai_type = run_libra_command(&["cat-file", "--ai-type", ai_session_id], repo.path());
    assert_cli_success(&ai_type, "cat-file --ai-type should succeed");
    assert_eq!(
        String::from_utf8_lossy(&ai_type.stdout).trim(),
        "ai_session"
    );

    let ai_pretty = run_libra_command(&["cat-file", "--ai", ai_session_id], repo.path());
    assert_cli_success(&ai_pretty, "cat-file --ai should succeed");
    let ai_pretty_stdout = String::from_utf8_lossy(&ai_pretty.stdout);
    assert!(ai_pretty_stdout.contains("schema: libra.ai_session.v2"));
    assert!(ai_pretty_stdout.contains("provider: claude"));

    let second = run_libra_command(
        &[
            "claude-sdk",
            "import",
            "--artifact",
            artifact_path.to_str().expect("artifact path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&second, "second claude-sdk import should succeed");
    let second_json = parse_stdout_json(&second, "second import");
    assert_eq!(second_json["alreadyPersisted"], json!(true));
    assert_eq!(second_json["aiSessionId"], json!(ai_session_id));
    assert_eq!(
        second_json["aiSessionObjectHash"],
        first_json["aiSessionObjectHash"]
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_run_with_custom_helper_persists_intent_extraction() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn managed_bridge() {}\n").expect("write source file");

    let artifact_json = semantic_full_artifact(repo.path(), &touched_file);

    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&artifact_json).expect("serialize test artifact"),
    )
    .expect("write test artifact");

    let helper_path = repo.path().join("fake-managed-helper.sh");
    write_shell_helper(&helper_path, &artifact_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
            "--node-binary",
            "/bin/sh",
        ],
        repo.path(),
    );
    assert_cli_success(&run, "claude-sdk run should succeed");
    let run_json = parse_stdout_json(&run, "claude-sdk run");

    assert_eq!(run_json["ok"], json!(true));
    assert_eq!(run_json["mode"], json!("run"));
    assert_eq!(run_json["alreadyPersisted"], json!(false));

    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present");
    let intent_extraction_path = PathBuf::from(
        run_json["intentExtractionPath"]
            .as_str()
            .expect("intentExtractionPath"),
    );
    let raw_artifact_path = PathBuf::from(
        run_json["rawArtifactPath"]
            .as_str()
            .expect("rawArtifactPath"),
    );
    let audit_bundle_path = PathBuf::from(
        run_json["auditBundlePath"]
            .as_str()
            .expect("auditBundlePath"),
    );

    assert!(
        intent_extraction_path.exists(),
        "intent extraction should be persisted"
    );
    assert!(
        raw_artifact_path.exists(),
        "raw artifact should be materialized"
    );
    assert!(
        audit_bundle_path.exists(),
        "audit bundle should be materialized"
    );

    let intent_extraction = read_json_file(&intent_extraction_path);
    assert_eq!(
        intent_extraction["schema"],
        json!("libra.intent_extraction.v1")
    );
    assert_eq!(
        intent_extraction["extraction"]["intent"]["summary"],
        json!("Persist the Claude SDK managed bridge")
    );

    let audit_bundle = read_json_file(&audit_bundle_path);
    assert_eq!(
        audit_bundle["bridge"]["intentExtraction"]["status"],
        json!("accepted")
    );
    assert_eq!(
        audit_bundle["bridge"]["intentExtractionArtifact"]["schema"],
        json!("libra.intent_extraction.v1")
    );
    assert_eq!(
        audit_bundle["bridge"]["objectCandidates"]["runSnapshot"]["id"],
        json!(format!("{ai_session_id}::run"))
    );
    assert_eq!(
        audit_bundle["bridge"]["objectCandidates"]["runEvent"]["status"],
        json!("completed")
    );
    assert_eq!(
        audit_bundle["bridge"]["objectCandidates"]["toolInvocationEvents"][0]["tool"],
        json!("Read")
    );
    assert_eq!(
        audit_bundle["bridge"]["objectCandidates"]["providerInitSnapshot"]["apiKeySource"],
        json!("oauth")
    );
    assert_eq!(
        audit_bundle["bridge"]["objectCandidates"]["providerInitSnapshot"]["claudeCodeVersion"],
        json!("2.1.76")
    );
    assert_eq!(
        audit_bundle["bridge"]["objectCandidates"]["taskRuntimeEvents"]
            .as_array()
            .map(Vec::len),
        Some(4)
    );
    assert_eq!(
        audit_bundle["bridge"]["objectCandidates"]["toolRuntimeEvents"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    assert_eq!(
        audit_bundle["bridge"]["objectCandidates"]["assistantRuntimeEvents"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    assert_eq!(
        audit_bundle["bridge"]["objectCandidates"]["decisionRuntimeEvents"]
            .as_array()
            .map(Vec::len),
        Some(4)
    );
    assert_eq!(
        audit_bundle["bridge"]["objectCandidates"]["contextRuntimeEvents"]
            .as_array()
            .map(Vec::len),
        Some(7)
    );
    assert_eq!(
        audit_bundle["bridge"]["sessionState"]["metadata"]["provider_runtime"]["providerInit"]["apiKeySource"],
        json!("oauth")
    );
    assert!(
        audit_bundle["bridge"]["touchHints"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item == "src/lib.rs")),
        "touch hints should include the repo-relative file observed from tool evidence"
    );
    assert!(
        audit_bundle["fieldProvenance"]
            .as_array()
            .is_some_and(|entries| entries
                .iter()
                .any(|entry| entry["fieldPath"] == "runtime.assistantEvents")),
        "assistant stream runtime facts should be recorded in field provenance"
    );

    let ai_pretty = run_libra_command(&["cat-file", "--ai", ai_session_id], repo.path());
    assert_cli_success(&ai_pretty, "cat-file --ai should succeed for run path");
    let ai_pretty_stdout = String::from_utf8_lossy(&ai_pretty.stdout);
    assert!(ai_pretty_stdout.contains("schema: libra.ai_session.v2"));
    assert!(ai_pretty_stdout.contains("provider: claude"));
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_run_can_disable_auto_tool_approval() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn managed_bridge() {}\n").expect("write source file");

    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&semantic_full_artifact(repo.path(), &touched_file))
            .expect("serialize test artifact"),
    )
    .expect("write test artifact");

    let request_path = repo.path().join("helper-request.json");
    let helper_path = repo.path().join("capture-managed-helper.sh");
    write_request_capture_shell_helper(&helper_path, &artifact_path, &request_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--tool",
            "Read",
            "--auto-approve-tools",
            "false",
            "--include-partial-messages",
            "true",
            "--prompt-suggestions",
            "true",
            "--agent-progress-summaries",
            "true",
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
            "--node-binary",
            "/bin/sh",
        ],
        repo.path(),
    );
    assert_cli_success(
        &run,
        "claude-sdk run should succeed when auto tool approval is disabled",
    );

    let helper_request = read_json_file(&request_path);
    assert_eq!(helper_request["tools"], json!(["Read"]));
    assert_eq!(helper_request["allowedTools"], json!(["Read"]));
    assert_eq!(helper_request["autoApproveTools"], json!(false));
    assert_eq!(helper_request["includePartialMessages"], json!(true));
    assert_eq!(helper_request["promptSuggestions"], json!(true));
    assert_eq!(helper_request["agentProgressSummaries"], json!(true));
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_sync_sessions_persists_provider_session_snapshots() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let response_path = repo.path().join("session-catalog.json");
    fs::write(
        &response_path,
        serde_json::to_vec_pretty(&json!([
            {
                "sessionId": "session-a",
                "summary": "Claude session A",
                "lastModified": 1742025600000i64,
                "fileSize": 2048,
                "customTitle": "A title",
                "firstPrompt": "Add tests",
                "gitBranch": "main",
                "cwd": repo.path().to_string_lossy().to_string(),
                "tag": "review",
                "createdAt": 1742022000000i64
            },
            {
                "sessionId": "session-b",
                "summary": "Claude session B",
                "lastModified": 1742029200000i64,
                "cwd": repo.path().to_string_lossy().to_string()
            }
        ]))
        .expect("serialize session catalog"),
    )
    .expect("write session catalog response");

    let request_path = repo.path().join("session-catalog-request.json");
    let helper_path = repo.path().join("fake-session-helper.sh");
    write_json_response_capture_shell_helper(&helper_path, &response_path, &request_path);

    let sync = run_libra_command(
        &[
            "claude-sdk",
            "sync-sessions",
            "--limit",
            "10",
            "--offset",
            "2",
            "--include-worktrees",
            "false",
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
            "--node-binary",
            "/bin/sh",
        ],
        repo.path(),
    );
    assert_cli_success(&sync, "claude-sdk sync-sessions should succeed");
    let sync_json = parse_stdout_json(&sync, "claude-sdk sync-sessions");
    assert_eq!(sync_json["ok"], json!(true));
    assert_eq!(sync_json["mode"], json!("sync-sessions"));
    assert_eq!(sync_json["syncedCount"], json!(2));

    let helper_request = read_json_file(&request_path);
    assert_eq!(helper_request["mode"], json!("listSessions"));
    assert_eq!(helper_request["limit"], json!(10));
    assert_eq!(helper_request["offset"], json!(2));
    assert_eq!(helper_request["includeWorktrees"], json!(false));

    let first_artifact = PathBuf::from(
        sync_json["sessions"][0]["artifactPath"]
            .as_str()
            .expect("artifactPath should be present"),
    );
    assert!(
        first_artifact.exists(),
        "provider session artifact should exist"
    );
    let first_snapshot = read_json_file(&first_artifact);
    assert_eq!(first_snapshot["schema"], json!("libra.provider_session.v3"));
    assert_eq!(first_snapshot["provider"], json!("claude"));
    assert_eq!(first_snapshot["providerSessionId"], json!("session-a"));
    assert_eq!(
        first_snapshot["objectId"],
        json!("claude_provider_session__session-a")
    );
    assert_eq!(first_snapshot["summary"], json!("Claude session A"));

    let (_, history) = load_intent_history(repo.path()).await;
    let sessions = history
        .list_objects("provider_session")
        .await
        .expect("should list provider_session objects");
    assert_eq!(
        sessions.len(),
        2,
        "should persist provider session snapshots"
    );

    let ai_pretty = run_libra_command(
        &["cat-file", "--ai", "claude_provider_session__session-a"],
        repo.path(),
    );
    assert_cli_success(
        &ai_pretty,
        "cat-file --ai should succeed for provider_session object",
    );
    let ai_pretty_stdout = String::from_utf8_lossy(&ai_pretty.stdout);
    assert!(ai_pretty_stdout.contains("type: provider_session"));
    assert!(ai_pretty_stdout.contains("schema: libra.provider_session.v3"));
    assert!(ai_pretty_stdout.contains("provider: claude"));
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_hydrate_session_updates_provider_session_with_messages() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let catalog_response_path = repo.path().join("session-catalog.json");
    fs::write(
        &catalog_response_path,
        serde_json::to_vec_pretty(&json!([
            {
                "sessionId": "session-a",
                "summary": "Claude session A",
                "lastModified": 1742025600000i64,
                "cwd": repo.path().to_string_lossy().to_string()
            }
        ]))
        .expect("serialize session catalog"),
    )
    .expect("write session catalog response");
    let catalog_request_path = repo.path().join("session-catalog-request.json");
    let catalog_helper_path = repo.path().join("fake-session-catalog-helper.sh");
    write_json_response_capture_shell_helper(
        &catalog_helper_path,
        &catalog_response_path,
        &catalog_request_path,
    );

    let sync = run_libra_command(
        &[
            "claude-sdk",
            "sync-sessions",
            "--helper-path",
            catalog_helper_path
                .to_str()
                .expect("catalog helper path utf-8"),
            "--node-binary",
            "/bin/sh",
        ],
        repo.path(),
    );
    assert_cli_success(
        &sync,
        "claude-sdk sync-sessions should succeed before hydration",
    );

    let messages_response_path = repo.path().join("session-messages.json");
    fs::write(
        &messages_response_path,
        serde_json::to_vec_pretty(&json!([
            {
                "type": "system",
                "subtype": "init",
                "session_id": "session-a",
                "uuid": "msg-1"
            },
            {
                "type": "user",
                "session_id": "session-a"
            },
            {
                "type": "assistant",
                "session_id": "session-a",
                "uuid": "msg-2"
            },
            {
                "type": "tool_progress",
                "tool_use_id": "tool-1",
                "tool_name": "Read",
                "elapsed_time_seconds": 1,
                "session_id": "session-a",
                "uuid": "msg-3"
            },
            {
                "type": "result",
                "subtype": "success",
                "session_id": "session-a",
                "uuid": "msg-4",
                "duration_ms": 10,
                "duration_api_ms": 8,
                "is_error": false,
                "num_turns": 1,
                "result": "ok",
                "stop_reason": "end_turn",
                "total_cost_usd": 0.01,
                "usage": {}
            }
        ]))
        .expect("serialize session messages"),
    )
    .expect("write session messages response");
    let messages_request_path = repo.path().join("session-messages-request.json");
    let messages_helper_path = repo.path().join("fake-session-messages-helper.sh");
    write_json_response_capture_shell_helper(
        &messages_helper_path,
        &messages_response_path,
        &messages_request_path,
    );

    let hydrate = run_libra_command(
        &[
            "claude-sdk",
            "hydrate-session",
            "--provider-session-id",
            "session-a",
            "--limit",
            "20",
            "--offset",
            "3",
            "--helper-path",
            messages_helper_path
                .to_str()
                .expect("messages helper path utf-8"),
            "--node-binary",
            "/bin/sh",
        ],
        repo.path(),
    );
    assert_cli_success(&hydrate, "claude-sdk hydrate-session should succeed");
    let hydrate_json = parse_stdout_json(&hydrate, "claude-sdk hydrate-session");
    assert_eq!(hydrate_json["ok"], json!(true));
    assert_eq!(hydrate_json["mode"], json!("hydrate-session"));
    assert_eq!(hydrate_json["providerSessionId"], json!("session-a"));
    assert_eq!(hydrate_json["messageCount"], json!(5));

    let helper_request = read_json_file(&messages_request_path);
    assert_eq!(helper_request["mode"], json!("getSessionMessages"));
    assert_eq!(helper_request["providerSessionId"], json!("session-a"));
    assert_eq!(helper_request["limit"], json!(20));
    assert_eq!(helper_request["offset"], json!(3));

    let artifact_path = PathBuf::from(
        hydrate_json["artifactPath"]
            .as_str()
            .expect("artifactPath should be present"),
    );
    let snapshot = read_json_file(&artifact_path);
    assert_eq!(snapshot["schema"], json!("libra.provider_session.v3"));
    assert_eq!(snapshot["messageSync"]["messageCount"], json!(5));
    assert_eq!(
        snapshot["messageSync"]["kindCounts"]["system:init"],
        json!(1)
    );
    assert_eq!(
        snapshot["messageSync"]["kindCounts"]["result:success"],
        json!(1)
    );
    assert_eq!(
        snapshot["messageSync"]["firstMessageKind"],
        json!("system:init")
    );
    assert_eq!(
        snapshot["messageSync"]["lastMessageKind"],
        json!("result:success")
    );

    let messages_artifact_path = PathBuf::from(
        hydrate_json["messagesArtifactPath"]
            .as_str()
            .expect("messagesArtifactPath should be present"),
    );
    let messages_artifact = read_json_file(&messages_artifact_path);
    assert_eq!(
        messages_artifact["schema"],
        json!("libra.provider_session_messages.v1")
    );
    assert_eq!(messages_artifact["providerSessionId"], json!("session-a"));
    assert_eq!(
        messages_artifact["messages"].as_array().map(Vec::len),
        Some(5)
    );

    let ai_pretty = run_libra_command(
        &["cat-file", "--ai", "claude_provider_session__session-a"],
        repo.path(),
    );
    assert_cli_success(
        &ai_pretty,
        "cat-file --ai should succeed for hydrated provider_session object",
    );
    let ai_pretty_stdout = String::from_utf8_lossy(&ai_pretty.stdout);
    assert!(ai_pretty_stdout.contains("message_count: 5"));
    assert!(ai_pretty_stdout.contains("first_message_kind: system:init"));
    assert!(ai_pretty_stdout.contains("last_message_kind: result:success"));
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_build_evidence_input_from_provider_session_messages() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let catalog_response_path = repo.path().join("session-catalog.json");
    fs::write(
        &catalog_response_path,
        serde_json::to_vec_pretty(&json!([
            {
                "sessionId": "session-a",
                "summary": "Claude session A",
                "lastModified": 1742025600000i64,
                "cwd": repo.path().to_string_lossy().to_string()
            }
        ]))
        .expect("serialize session catalog"),
    )
    .expect("write session catalog response");
    let catalog_request_path = repo.path().join("session-catalog-request.json");
    let catalog_helper_path = repo.path().join("fake-session-catalog-helper.sh");
    write_json_response_capture_shell_helper(
        &catalog_helper_path,
        &catalog_response_path,
        &catalog_request_path,
    );

    let sync = run_libra_command(
        &[
            "claude-sdk",
            "sync-sessions",
            "--helper-path",
            catalog_helper_path
                .to_str()
                .expect("catalog helper path utf-8"),
            "--node-binary",
            "/bin/sh",
        ],
        repo.path(),
    );
    assert_cli_success(
        &sync,
        "claude-sdk sync-sessions should succeed before evidence-input build",
    );

    let messages_response_path = repo.path().join("session-messages.json");
    fs::write(
        &messages_response_path,
        serde_json::to_vec_pretty(&json!([
            {
                "type": "system",
                "subtype": "init",
                "session_id": "session-a",
                "uuid": "msg-1"
            },
            {
                "type": "user",
                "session_id": "session-a",
                "message": {
                    "content": [
                        {
                            "type": "text",
                            "text": "Inspect src/lib.rs and summarize the bridge state."
                        }
                    ]
                }
            },
            {
                "type": "assistant",
                "session_id": "session-a",
                "uuid": "msg-2",
                "message": {
                    "content": [
                        {
                            "type": "text",
                            "text": "I will inspect src/lib.rs and then summarize the current bridge shape."
                        },
                        {
                            "type": "tool_use",
                            "name": "Read",
                            "input": {
                                "file_path": "src/lib.rs"
                            }
                        }
                    ]
                }
            },
            {
                "type": "tool_progress",
                "tool_use_id": "tool-1",
                "tool_name": "Read",
                "elapsed_time_seconds": 1,
                "session_id": "session-a",
                "uuid": "msg-3"
            },
            {
                "type": "result",
                "subtype": "success",
                "session_id": "session-a",
                "uuid": "msg-4",
                "duration_ms": 10,
                "duration_api_ms": 8,
                "is_error": false,
                "num_turns": 1,
                "result": "ok",
                "stop_reason": "end_turn",
                "total_cost_usd": 0.01,
                "permission_denials": [
                    {
                        "tool_name": "Edit"
                    }
                ],
                "structured_output": {
                    "summary": "Separate runtime facts from semantic candidates"
                },
                "usage": {}
            }
        ]))
        .expect("serialize session messages"),
    )
    .expect("write session messages response");
    let messages_request_path = repo.path().join("session-messages-request.json");
    let messages_helper_path = repo.path().join("fake-session-messages-helper.sh");
    write_json_response_capture_shell_helper(
        &messages_helper_path,
        &messages_response_path,
        &messages_request_path,
    );

    let hydrate = run_libra_command(
        &[
            "claude-sdk",
            "hydrate-session",
            "--provider-session-id",
            "session-a",
            "--helper-path",
            messages_helper_path
                .to_str()
                .expect("messages helper path utf-8"),
            "--node-binary",
            "/bin/sh",
        ],
        repo.path(),
    );
    assert_cli_success(
        &hydrate,
        "claude-sdk hydrate-session should succeed before evidence-input build",
    );

    let build = run_libra_command(
        &[
            "claude-sdk",
            "build-evidence-input",
            "--provider-session-id",
            "session-a",
        ],
        repo.path(),
    );
    assert_cli_success(
        &build,
        "claude-sdk build-evidence-input should succeed for a hydrated provider session",
    );
    let build_json = parse_stdout_json(&build, "claude-sdk build-evidence-input");
    assert_eq!(build_json["ok"], json!(true));
    assert_eq!(build_json["mode"], json!("build-evidence-input"));
    assert_eq!(build_json["providerSessionId"], json!("session-a"));
    assert_eq!(
        build_json["providerSessionObjectId"],
        json!("claude_provider_session__session-a")
    );
    assert_eq!(
        build_json["objectId"],
        json!("claude_evidence_input__session-a")
    );
    assert_eq!(build_json["messageCount"], json!(5));

    let evidence_path = PathBuf::from(
        build_json["artifactPath"]
            .as_str()
            .expect("artifactPath should be present"),
    );
    let evidence = read_json_file(&evidence_path);
    assert_eq!(evidence["schema"], json!("libra.evidence_input.v1"));
    assert_eq!(evidence["object_type"], json!("evidence_input"));
    assert_eq!(evidence["provider"], json!("claude"));
    assert_eq!(evidence["providerSessionId"], json!("session-a"));
    assert_eq!(
        evidence["providerSessionObjectId"],
        json!("claude_provider_session__session-a")
    );
    assert_eq!(evidence["messageOverview"]["messageCount"], json!(5));
    assert_eq!(
        evidence["contentOverview"]["assistantMessageCount"],
        json!(1)
    );
    assert_eq!(evidence["contentOverview"]["userMessageCount"], json!(1));
    assert_eq!(
        evidence["contentOverview"]["observedTools"]["Read"],
        json!(2)
    );
    assert_eq!(
        evidence["contentOverview"]["observedPaths"][0],
        json!("src/lib.rs")
    );
    assert_eq!(evidence["runtimeSignals"]["toolRuntimeCount"], json!(1));
    assert_eq!(evidence["runtimeSignals"]["resultMessageCount"], json!(1));
    assert_eq!(
        evidence["runtimeSignals"]["hasStructuredOutput"],
        json!(true)
    );
    assert_eq!(
        evidence["runtimeSignals"]["hasPermissionDenials"],
        json!(true)
    );
    assert_eq!(evidence["latestResult"]["stopReason"], json!("end_turn"));
    assert_eq!(evidence["latestResult"]["permissionDenialCount"], json!(1));

    let (_, history) = load_intent_history(repo.path()).await;
    let evidence_inputs = history
        .list_objects("evidence_input")
        .await
        .expect("should list evidence_input objects");
    assert_eq!(
        evidence_inputs.len(),
        1,
        "should persist evidence_input objects"
    );

    let ai_pretty = run_libra_command(
        &["cat-file", "--ai", "claude_evidence_input__session-a"],
        repo.path(),
    );
    assert_cli_success(
        &ai_pretty,
        "cat-file --ai should succeed for evidence_input object",
    );
    let ai_pretty_stdout = String::from_utf8_lossy(&ai_pretty.stdout);
    assert!(ai_pretty_stdout.contains("type: evidence_input"));
    assert!(ai_pretty_stdout.contains("schema: libra.evidence_input.v1"));
    assert!(ai_pretty_stdout.contains("message_count: 5"));
    assert!(ai_pretty_stdout.contains("has_structured_output: true"));

    let helper_request = read_json_file(&messages_request_path);
    assert_eq!(helper_request["mode"], json!("getSessionMessages"));
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_run_persists_partial_artifact_when_helper_times_out() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn managed_bridge() {}\n").expect("write source file");

    let artifact_path = repo.path().join("timed-out-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&timed_out_partial_artifact(repo.path(), &touched_file))
            .expect("serialize timeout artifact"),
    )
    .expect("write timeout artifact");

    let helper_path = repo.path().join("fake-timeout-helper.sh");
    write_shell_helper(&helper_path, &artifact_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
            "--node-binary",
            "/bin/sh",
        ],
        repo.path(),
    );
    assert_cli_success(
        &run,
        "claude-sdk run should persist partial artifacts when the helper times out",
    );
    let run_json = parse_stdout_json(&run, "claude-sdk run timeout artifact");
    assert!(
        run_json["intentExtractionPath"].is_null(),
        "partial timeout artifact should not produce a formal intent extraction"
    );

    let audit_bundle_path = PathBuf::from(
        run_json["auditBundlePath"]
            .as_str()
            .expect("auditBundlePath should be present"),
    );
    let audit_bundle = read_json_file(&audit_bundle_path);
    assert_eq!(
        audit_bundle["bridge"]["objectCandidates"]["runEvent"]["status"],
        json!("timed_out")
    );
    assert_eq!(
        audit_bundle["bridge"]["sessionState"]["metadata"]["managed_helper_timed_out"],
        json!(true)
    );
    assert_eq!(
        audit_bundle["bridge"]["sessionState"]["metadata"]["managed_helper_error"],
        json!("Claude SDK helper timed out")
    );
    assert!(
        audit_bundle["bridge"]["objectCandidates"]["decisionRuntimeEvents"]
            .as_array()
            .is_some_and(|events| !events.is_empty()),
        "partial timeout artifact should still preserve decision runtime facts"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_run_plan_prompt_fixture_persists_task_runtime_scenario() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("samples").join("managed.rs");
    fs::create_dir_all(touched_file.parent().expect("sample file parent")).expect("mkdir samples");
    fs::write(&touched_file, "pub fn provider_runtime() {}\n").expect("write sample file");

    let artifact_path = repo.path().join("managed-plan-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&plan_task_only_artifact(repo.path(), &touched_file))
            .expect("serialize plan scenario artifact"),
    )
    .expect("write plan scenario artifact");

    let helper_path = repo.path().join("fake-managed-helper.sh");
    write_shell_helper(&helper_path, &artifact_path);

    let prompt_path = repo.path().join("plan-prompt.txt");
    fs::write(&prompt_path, PLAN_PROMPT).expect("write plan prompt fixture");

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--prompt-file",
            prompt_path.to_str().expect("prompt path utf-8"),
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
            "--node-binary",
            "/bin/sh",
        ],
        repo.path(),
    );
    assert_cli_success(
        &run,
        "claude-sdk run should succeed for plan prompt fixture",
    );
    let run_json = parse_stdout_json(&run, "claude-sdk run with plan prompt fixture");

    let audit_bundle_path = PathBuf::from(
        run_json["auditBundlePath"]
            .as_str()
            .expect("auditBundlePath should be present"),
    );
    let intent_extraction_path = PathBuf::from(
        run_json["intentExtractionPath"]
            .as_str()
            .expect("intentExtractionPath should be present"),
    );

    let audit_bundle = read_json_file(&audit_bundle_path);
    let task_runtime_events = audit_bundle["bridge"]["objectCandidates"]["taskRuntimeEvents"]
        .as_array()
        .expect("taskRuntimeEvents should be an array");
    let decision_runtime_events =
        audit_bundle["bridge"]["objectCandidates"]["decisionRuntimeEvents"]
            .as_array()
            .expect("decisionRuntimeEvents should be an array");
    let context_runtime_events = audit_bundle["bridge"]["objectCandidates"]["contextRuntimeEvents"]
        .as_array()
        .expect("contextRuntimeEvents should be an array");

    assert_eq!(
        audit_bundle["rawArtifact"]["prompt"],
        json!(PLAN_PROMPT),
        "raw artifact should preserve the persisted prompt fixture"
    );
    assert_eq!(
        audit_bundle["bridge"]["intentExtraction"]["status"],
        json!("accepted")
    );
    assert_eq!(task_runtime_events.len(), 6);
    assert!(decision_runtime_events.is_empty());
    assert!(context_runtime_events.is_empty());
    assert_eq!(
        audit_bundle["bridge"]["objectCandidates"]["providerInitSnapshot"]["agents"],
        json!(["general-purpose", "statusline-setup", "Explore", "Plan"])
    );
    assert!(
        audit_bundle["rawArtifact"]["messages"]
            .as_array()
            .is_some_and(|messages| messages.iter().any(|message| {
                message["message"]["content"]
                    .as_array()
                    .is_some_and(|items| {
                        items.iter().any(|item| {
                            item["text"]
                                .as_str()
                                .is_some_and(|text| text.contains("3-Step Plan"))
                        })
                    })
            })),
        "plan scenario should preserve the assistant plan text in raw messages"
    );

    let task_kinds = task_runtime_events
        .iter()
        .map(|event| event["kind"].as_str().expect("task runtime kind"))
        .collect::<Vec<_>>();
    assert_eq!(
        task_kinds,
        vec![
            "task_started",
            "task_progress",
            "task_progress",
            "task_notification",
            "SubagentStart",
            "SubagentStop"
        ]
    );

    let intent_extraction = read_json_file(&intent_extraction_path);
    assert_eq!(
        intent_extraction["extraction"]["intent"]["summary"],
        json!(
            "Refactor Claude SDK bridge to separate provider-native runtime facts from semantic-layer candidates"
        )
    );
    assert_eq!(
        intent_extraction["extraction"]["risk"]["level"],
        json!("medium")
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_resolve_extraction_materializes_intentspec_preview() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn managed_bridge() {}\n").expect("write source file");

    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&semantic_full_artifact(repo.path(), &touched_file))
            .expect("serialize test artifact"),
    )
    .expect("write test artifact");

    let helper_path = repo.path().join("fake-managed-helper.sh");
    write_shell_helper(&helper_path, &artifact_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
            "--node-binary",
            "/bin/sh",
        ],
        repo.path(),
    );
    assert_cli_success(
        &run,
        "claude-sdk run should succeed before resolve-extraction",
    );
    let run_json = parse_stdout_json(&run, "claude-sdk run before resolve-extraction");
    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present");

    let resolve = run_libra_command(
        &[
            "claude-sdk",
            "resolve-extraction",
            "--ai-session-id",
            ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&resolve, "claude-sdk resolve-extraction should succeed");
    let resolve_json = parse_stdout_json(&resolve, "claude-sdk resolve-extraction");
    let expected_extraction_path = run_json["intentExtractionPath"]
        .as_str()
        .expect("run should emit intentExtractionPath");

    assert_eq!(resolve_json["ok"], json!(true));
    assert_eq!(resolve_json["mode"], json!("resolve-extraction"));
    assert_eq!(resolve_json["aiSessionId"], json!(ai_session_id));
    assert_eq!(
        resolve_json["extractionPath"],
        json!(expected_extraction_path)
    );
    assert_eq!(resolve_json["riskLevel"], json!("low"));
    assert!(
        resolve_json["summary"]
            .as_str()
            .is_some_and(|summary| summary.contains("IntentSpec generated.")),
        "resolve-extraction summary should be derived from the IntentSpec preview"
    );

    let resolved_spec_path = PathBuf::from(
        resolve_json["resolvedSpecPath"]
            .as_str()
            .expect("resolvedSpecPath should be present"),
    );
    assert!(
        resolved_spec_path.exists(),
        "resolved IntentSpec artifact should be materialized"
    );

    let resolved_artifact = read_json_file(&resolved_spec_path);
    assert_eq!(
        resolved_artifact["schema"],
        json!("libra.intent_resolution.v1")
    );
    assert_eq!(resolved_artifact["aiSessionId"], json!(ai_session_id));
    assert_eq!(resolved_artifact["riskLevel"], json!("low"));
    assert_eq!(
        resolved_artifact["extractionSource"],
        json!("claude_agent_sdk_managed.structured_output")
    );
    assert_eq!(resolved_artifact["intentspec"]["kind"], json!("IntentSpec"));
    assert_eq!(
        resolved_artifact["intentspec"]["apiVersion"],
        json!("intentspec.io/v1alpha1")
    );
    assert_eq!(
        resolved_artifact["intentspec"]["intent"]["summary"],
        json!("Persist the Claude SDK managed bridge")
    );
    assert_eq!(
        resolved_artifact["intentspec"]["risk"]["level"],
        json!("low")
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_persist_intent_writes_formal_intent_and_binding_artifact() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn managed_bridge() {}\n").expect("write source file");

    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&semantic_full_artifact(repo.path(), &touched_file))
            .expect("serialize test artifact"),
    )
    .expect("write test artifact");

    let helper_path = repo.path().join("fake-managed-helper.sh");
    write_shell_helper(&helper_path, &artifact_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
            "--node-binary",
            "/bin/sh",
        ],
        repo.path(),
    );
    assert_cli_success(&run, "claude-sdk run should succeed before persist-intent");
    let run_json = parse_stdout_json(&run, "claude-sdk run before persist-intent");
    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present");

    let resolve = run_libra_command(
        &[
            "claude-sdk",
            "resolve-extraction",
            "--ai-session-id",
            ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(
        &resolve,
        "claude-sdk resolve-extraction should succeed before persist-intent",
    );

    let persist = run_libra_command(
        &[
            "claude-sdk",
            "persist-intent",
            "--ai-session-id",
            ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&persist, "claude-sdk persist-intent should succeed");
    let persist_json = parse_stdout_json(&persist, "claude-sdk persist-intent");
    let expected_extraction_path = run_json["intentExtractionPath"]
        .as_str()
        .expect("run should emit intentExtractionPath");

    assert_eq!(persist_json["ok"], json!(true));
    assert_eq!(persist_json["mode"], json!("persist-intent"));
    assert_eq!(persist_json["aiSessionId"], json!(ai_session_id));

    let intent_id = persist_json["intentId"]
        .as_str()
        .expect("intentId should be present")
        .to_string();
    let binding_path = PathBuf::from(
        persist_json["bindingPath"]
            .as_str()
            .expect("bindingPath should be present"),
    );
    assert!(
        binding_path.exists(),
        "persist-intent should materialize a binding artifact"
    );

    let binding_artifact = read_json_file(&binding_path);
    assert_eq!(
        binding_artifact["schema"],
        json!("libra.intent_input_binding.v1")
    );
    assert_eq!(
        binding_artifact["extractionPath"],
        json!(expected_extraction_path)
    );
    assert_eq!(binding_artifact["aiSessionId"], json!(ai_session_id));
    assert_eq!(binding_artifact["intentId"], json!(intent_id));
    assert!(
        binding_artifact["summary"]
            .as_str()
            .is_some_and(|summary| summary.contains("IntentSpec generated.")),
        "binding artifact should retain the resolved IntentSpec summary"
    );

    let (storage, history) = load_intent_history(repo.path()).await;
    let intents = history
        .list_objects("intent")
        .await
        .expect("should list intent objects");
    assert_eq!(intents.len(), 1, "should persist exactly one formal intent");
    assert_eq!(
        intents[0].0, intent_id,
        "history should contain the persisted intent ID"
    );

    let stored_intent: Intent = storage
        .get_json(&intents[0].1)
        .await
        .expect("should load persisted intent object");
    assert_eq!(
        stored_intent.prompt(),
        "Persist the Claude SDK managed bridge"
    );
    assert!(
        stored_intent.spec().is_some(),
        "persisted formal intent should retain the canonical IntentSpec"
    );
}
