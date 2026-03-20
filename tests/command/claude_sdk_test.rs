//! Integration tests for `libra code --stdio` Claude SDK / MCP stdio transport.
//!
//! **Layer:** L1 — deterministic, Unix-only (`#[cfg(unix)]`).

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Output, Stdio},
    sync::Arc,
};

use git_internal::internal::object::{
    decision::Decision, evidence::Evidence, intent::Intent, run::Run, task::Task,
};
use libra::{
    internal::{
        ai::history::{AI_REF, HistoryManager},
        model::reference::{self, ConfigKind},
    },
    utils::{storage::local::LocalStorage, storage_ext::StorageExt, test},
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::de::DeserializeOwned;
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

async fn read_history_head(repo: &Path, history: &HistoryManager) -> String {
    assert_eq!(history.ref_name(), AI_REF);
    let db_path = repo.join(".libra/libra.db");
    let db_conn = libra::internal::db::establish_connection(
        db_path.to_str().expect("db path should be valid UTF-8"),
    )
    .await
    .expect("failed to connect test database");
    let row = reference::Entity::find()
        .filter(reference::Column::Name.eq(AI_REF))
        .filter(reference::Column::Kind.eq(ConfigKind::Branch))
        .one(&db_conn)
        .await
        .expect("failed to query AI history ref")
        .expect("AI history ref should exist");
    row.commit.expect("AI history ref should point to a commit")
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

fn test_change_type_artifact(repo: &Path, touched_file: &Path) -> Value {
    let mut artifact = semantic_full_artifact(repo, touched_file);
    let structured_output = artifact["resultMessage"]["structured_output"]
        .as_object_mut()
        .expect("semantic full artifact should contain structured_output");
    structured_output.insert(
        "summary".to_string(),
        json!("Add regression coverage for Claude SDK bridge persistence"),
    );
    structured_output.insert(
        "problemStatement".to_string(),
        json!("The Claude SDK bridge needs explicit regression coverage for persisted artifacts."),
    );
    structured_output.insert("changeType".to_string(), json!("test"));
    structured_output.insert(
        "objectives".to_string(),
        json!(["Add an integration regression test for persisted Claude SDK artifacts"]),
    );
    structured_output.insert(
        "successCriteria".to_string(),
        json!(["Regression test passes and covers persisted artifact behavior"]),
    );
    structured_output.insert(
        "riskRationale".to_string(),
        json!("The change is test-only and should not affect production behavior."),
    );
    artifact
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

async fn read_tracked_object<T>(repo: &Path, object_type: &str, object_id: &str) -> T
where
    T: DeserializeOwned + Send + Sync,
{
    let (storage, history) = load_intent_history(repo).await;
    let hash = history
        .get_object_hash(object_type, object_id)
        .await
        .expect("should query object hash")
        .unwrap_or_else(|| panic!("expected {object_type} object '{object_id}' to exist"));
    storage
        .get_json::<T>(&hash)
        .await
        .unwrap_or_else(|err| panic!("failed to load {object_type} '{object_id}': {err}"))
}

fn session_messages_fixture(session_id: &str) -> Value {
    json!([
        {
            "type": "system",
            "subtype": "init",
            "session_id": session_id,
            "uuid": "msg-1"
        },
        {
            "type": "user",
            "session_id": session_id,
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
            "session_id": session_id,
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
            "type": "system",
            "subtype": "task_progress",
            "session_id": session_id,
            "uuid": "msg-task-1",
            "description": "Reading provider runtime facts"
        },
        {
            "type": "tool_progress",
            "tool_use_id": "tool-1",
            "tool_name": "Read",
            "elapsed_time_seconds": 1,
            "session_id": session_id,
            "uuid": "msg-3"
        },
        {
            "type": "result",
            "subtype": "success",
            "session_id": session_id,
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
    ])
}

fn stage_provider_session_evidence_artifacts(repo: &Path, provider_session_id: &str) {
    let catalog_response_path = repo.join("session-catalog.json");
    fs::write(
        &catalog_response_path,
        serde_json::to_vec_pretty(&json!([
            {
                "sessionId": provider_session_id,
                "summary": "Claude provider session fixture",
                "lastModified": 1742025600000i64,
                "cwd": repo.to_string_lossy().to_string()
            }
        ]))
        .expect("serialize session catalog"),
    )
    .expect("write session catalog response");
    let catalog_request_path = repo.join("session-catalog-request.json");
    let catalog_helper_path = repo.join("fake-session-catalog-helper.sh");
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
        repo,
    );
    assert_cli_success(
        &sync,
        "claude-sdk sync-sessions should succeed for formal bridge tests",
    );

    let messages_response_path = repo.join("session-messages.json");
    fs::write(
        &messages_response_path,
        serde_json::to_vec_pretty(&session_messages_fixture(provider_session_id))
            .expect("serialize session messages"),
    )
    .expect("write session messages response");
    let messages_request_path = repo.join("session-messages-request.json");
    let messages_helper_path = repo.join("fake-session-messages-helper.sh");
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
            provider_session_id,
            "--helper-path",
            messages_helper_path
                .to_str()
                .expect("messages helper path utf-8"),
            "--node-binary",
            "/bin/sh",
        ],
        repo,
    );
    assert_cli_success(
        &hydrate,
        "claude-sdk hydrate-session should succeed for formal bridge tests",
    );

    let build = run_libra_command(
        &[
            "claude-sdk",
            "build-evidence-input",
            "--provider-session-id",
            provider_session_id,
        ],
        repo,
    );
    assert_cli_success(
        &build,
        "claude-sdk build-evidence-input should succeed for formal bridge tests",
    );
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
async fn test_claude_sdk_sync_sessions_preserves_existing_message_sync() {
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
    assert_cli_success(&sync, "initial claude-sdk sync-sessions should succeed");

    let messages_response_path = repo.path().join("session-messages.json");
    fs::write(
        &messages_response_path,
        serde_json::to_vec_pretty(&json!([
            {"type": "user", "session_id": "session-a"},
            {"type": "assistant", "session_id": "session-a"},
            {"type": "result", "subtype": "success", "session_id": "session-a"}
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
    assert_cli_success(&hydrate, "claude-sdk hydrate-session should succeed");

    let resync = run_libra_command(
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
    assert_cli_success(&resync, "repeat claude-sdk sync-sessions should succeed");

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
        "build-evidence-input should still succeed after re-syncing a hydrated session",
    );

    let snapshot_path = repo
        .path()
        .join(".libra/provider-sessions/claude_provider_session__session-a.json");
    let snapshot = read_json_file(&snapshot_path);
    assert_eq!(snapshot["messageSync"]["messageCount"], json!(3));
    assert_eq!(
        snapshot["messageSync"]["lastMessageKind"],
        json!("result:success")
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_sync_sessions_skips_history_append_when_snapshot_is_unchanged() {
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
                "cwd": repo.path().to_string_lossy().to_string()
            }
        ]))
        .expect("serialize session catalog"),
    )
    .expect("write session catalog response");

    let request_path = repo.path().join("session-catalog-request.json");
    let helper_path = repo.path().join("fake-session-helper.sh");
    write_json_response_capture_shell_helper(&helper_path, &response_path, &request_path);

    let first = run_libra_command(
        &[
            "claude-sdk",
            "sync-sessions",
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
            "--node-binary",
            "/bin/sh",
        ],
        repo.path(),
    );
    assert_cli_success(&first, "initial sync-sessions should succeed");

    let (_, history) = load_intent_history(repo.path()).await;
    let first_head = read_history_head(repo.path(), &history).await;

    let second = run_libra_command(
        &[
            "claude-sdk",
            "sync-sessions",
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
            "--node-binary",
            "/bin/sh",
        ],
        repo.path(),
    );
    assert_cli_success(&second, "repeat sync-sessions should succeed");

    let second_head = read_history_head(repo.path(), &history).await;
    assert_eq!(
        second_head, first_head,
        "unchanged sync-sessions runs should not append a new AI history commit"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_sync_sessions_keeps_history_in_current_repo_when_cwd_is_overridden() {
    let repo = tempdir().expect("failed to create repo root");
    let external_project = tempdir().expect("failed to create external project root");
    test::setup_with_new_libra_in(repo.path()).await;

    let response_path = repo.path().join("session-catalog.json");
    fs::write(
        &response_path,
        serde_json::to_vec_pretty(&json!([
            {
                "sessionId": "session-a",
                "summary": "Claude session A",
                "lastModified": 1742025600000i64,
                "cwd": external_project.path().to_string_lossy().to_string()
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
            "--cwd",
            external_project
                .path()
                .to_str()
                .expect("external cwd utf-8"),
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
            "--node-binary",
            "/bin/sh",
        ],
        repo.path(),
    );
    assert_cli_success(
        &sync,
        "claude-sdk sync-sessions should persist into the current repo even with --cwd override",
    );

    let (_, history) = load_intent_history(repo.path()).await;
    let sessions = history
        .list_objects("provider_session")
        .await
        .expect("should list provider_session objects from current repo");
    assert_eq!(sessions.len(), 1);

    assert!(
        !external_project.path().join(".libra/libra.db").exists(),
        "sync-sessions should not create a shadow Libra repo under the overridden cwd"
    );
}

#[test]
fn test_claude_sdk_helper_resolves_project_local_sdk_from_relative_cwd() {
    let repo = tempdir().expect("failed to create repo root");
    let module_dir = repo
        .path()
        .join("node_modules")
        .join("@anthropic-ai")
        .join("claude-agent-sdk");
    fs::create_dir_all(&module_dir).expect("failed to create fake sdk module directory");
    fs::write(
        module_dir.join("index.js"),
        r#"exports.query = async function* () {};
exports.listSessions = async () => ([{
  sessionId: "session-relative",
  summary: "Relative cwd session",
  lastModified: 1742025600000,
  cwd: process.cwd()
}]);"#,
    )
    .expect("failed to write fake sdk module");

    let helper_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("internal")
        .join("ai")
        .join("providers")
        .join("claude_sdk")
        .join("helper.cjs");
    let mut child = std::process::Command::new("node")
        .arg(&helper_path)
        .current_dir(repo.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn helper with node");

    let request = br#"{"mode":"listSessions","cwd":".","offset":0,"includeWorktrees":true}"#;
    child
        .stdin
        .as_mut()
        .expect("child stdin should exist")
        .write_all(request)
        .expect("failed to send request to helper");
    let output = child.wait_with_output().expect("failed to wait on helper");
    assert!(
        output.status.success(),
        "helper should resolve project-local sdk from relative cwd: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let sync_json: Value =
        serde_json::from_slice(&output.stdout).expect("helper stdout should be valid JSON");
    assert_eq!(sync_json.as_array().map(Vec::len), Some(1));
    assert_eq!(sync_json[0]["sessionId"], json!("session-relative"));
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
                "type": "system",
                "subtype": "task_progress",
                "session_id": "session-a",
                "uuid": "msg-task-1",
                "description": "Reading provider runtime facts"
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
    assert_eq!(build_json["messageCount"], json!(6));

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
    assert_eq!(evidence["messageOverview"]["messageCount"], json!(6));
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
    assert_eq!(evidence["runtimeSignals"]["taskRuntimeCount"], json!(1));
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
    assert!(ai_pretty_stdout.contains("message_count: 6"));
    assert!(ai_pretty_stdout.contains("has_structured_output: true"));

    let helper_request = read_json_file(&messages_request_path);
    assert_eq!(helper_request["mode"], json!("getSessionMessages"));
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_build_evidence_input_skips_history_append_when_artifact_is_unchanged() {
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
        "sync-sessions should succeed before evidence-input build",
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
                "type": "assistant",
                "session_id": "session-a",
                "uuid": "msg-2",
                "message": {
                    "content": [
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
                "type": "result",
                "subtype": "success",
                "session_id": "session-a",
                "uuid": "msg-3",
                "stop_reason": "end_turn",
                "structured_output": {
                    "summary": "Bridge runtime facts"
                }
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
        "hydrate-session should succeed before evidence-input build",
    );

    let first = run_libra_command(
        &[
            "claude-sdk",
            "build-evidence-input",
            "--provider-session-id",
            "session-a",
        ],
        repo.path(),
    );
    assert_cli_success(&first, "initial build-evidence-input should succeed");

    let (_, history) = load_intent_history(repo.path()).await;
    let first_head = read_history_head(repo.path(), &history).await;

    let second = run_libra_command(
        &[
            "claude-sdk",
            "build-evidence-input",
            "--provider-session-id",
            "session-a",
        ],
        repo.path(),
    );
    assert_cli_success(&second, "repeat build-evidence-input should succeed");

    let second_head = read_history_head(repo.path(), &history).await;
    assert_eq!(
        second_head, first_head,
        "unchanged evidence-input builds should not append a new AI history commit"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_sync_sessions_rejects_invalid_provider_session_id() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let response_path = repo.path().join("session-catalog.json");
    fs::write(
        &response_path,
        serde_json::to_vec_pretty(&json!([
            {
                "sessionId": "../session-a",
                "summary": "Claude session A",
                "lastModified": 1742025600000i64,
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
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
            "--node-binary",
            "/bin/sh",
        ],
        repo.path(),
    );
    assert!(
        !sync.status.success(),
        "sync-sessions should reject invalid provider session ids"
    );
    assert!(
        String::from_utf8_lossy(&sync.stderr).contains("invalid provider session id"),
        "expected invalid provider session id error, got: {}",
        String::from_utf8_lossy(&sync.stderr)
    );
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

#[tokio::test]
#[serial]
async fn test_claude_sdk_persist_intent_accepts_test_change_type() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn managed_bridge() {}\n").expect("write source file");

    let artifact_path = repo
        .path()
        .join("managed-run-artifact-test-change-type.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&test_change_type_artifact(repo.path(), &touched_file))
            .expect("serialize test artifact"),
    )
    .expect("write test artifact");

    let helper_path = repo.path().join("fake-managed-helper-test-change-type.sh");
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
    assert_cli_success(&run, "claude-sdk run should succeed for changeType=test");
    let run_json = parse_stdout_json(&run, "claude-sdk run changeType=test");
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
        "claude-sdk resolve-extraction should accept changeType=test",
    );
    let resolve_json = parse_stdout_json(&resolve, "claude-sdk resolve-extraction changeType=test");
    let resolved_spec_path = PathBuf::from(
        resolve_json["resolvedSpecPath"]
            .as_str()
            .expect("resolvedSpecPath should be present"),
    );
    let resolved_artifact = read_json_file(&resolved_spec_path);
    assert_eq!(
        resolved_artifact["intentspec"]["intent"]["changeType"],
        json!("test")
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
    assert_cli_success(
        &persist,
        "claude-sdk persist-intent should accept changeType=test",
    );
    let persist_json = parse_stdout_json(&persist, "claude-sdk persist-intent changeType=test");
    let intent_id = persist_json["intentId"]
        .as_str()
        .expect("intentId should be present")
        .to_string();

    let (storage, history) = load_intent_history(repo.path()).await;
    let intents = history
        .list_objects("intent")
        .await
        .expect("should list intent objects");
    assert_eq!(intents.len(), 1, "should persist exactly one formal intent");
    assert_eq!(intents[0].0, intent_id);

    let stored_intent: Intent = storage
        .get_json(&intents[0].1)
        .await
        .expect("should load persisted intent object");
    let stored_spec = stored_intent
        .spec()
        .expect("persisted formal intent should retain the canonical IntentSpec");
    assert_eq!(stored_spec.0["intent"]["changeType"], json!("test"));
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_formal_bridge_persists_task_run_evidence_and_decision() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn formal_bridge() {}\n").expect("write source file");

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
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
            "--node-binary",
            "/bin/sh",
        ],
        repo.path(),
    );
    assert_cli_success(&run, "claude-sdk run should succeed for formal bridge test");
    let run_json = parse_stdout_json(&run, "claude-sdk run formal bridge");
    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present")
        .to_string();
    let provider_session_id = run_json["providerSessionId"]
        .as_str()
        .expect("providerSessionId should be present")
        .to_string();

    stage_provider_session_evidence_artifacts(repo.path(), &provider_session_id);

    let resolve = run_libra_command(
        &[
            "claude-sdk",
            "resolve-extraction",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&resolve, "resolve-extraction should succeed");

    let persist_intent = run_libra_command(
        &[
            "claude-sdk",
            "persist-intent",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&persist_intent, "persist-intent should succeed");
    let persist_intent_json = parse_stdout_json(&persist_intent, "persist-intent output");
    let intent_id = persist_intent_json["intentId"]
        .as_str()
        .expect("intentId should be present")
        .to_string();

    let bridge = run_libra_command(
        &[
            "claude-sdk",
            "bridge-run",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&bridge, "bridge-run should succeed");
    let bridge_json = parse_stdout_json(&bridge, "bridge-run output");
    let task_id = bridge_json["taskId"]
        .as_str()
        .expect("taskId should be present")
        .to_string();
    let run_id = bridge_json["runId"]
        .as_str()
        .expect("runId should be present")
        .to_string();
    assert_eq!(bridge_json["intentId"], json!(intent_id.clone()));

    let bridge_repeat = run_libra_command(
        &[
            "claude-sdk",
            "bridge-run",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&bridge_repeat, "bridge-run should be idempotent");
    let bridge_repeat_json = parse_stdout_json(&bridge_repeat, "bridge-run repeat output");
    assert_eq!(bridge_repeat_json["taskId"], json!(task_id.clone()));
    assert_eq!(bridge_repeat_json["runId"], json!(run_id.clone()));

    let evidence = run_libra_command(
        &[
            "claude-sdk",
            "persist-evidence",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&evidence, "persist-evidence should succeed");
    let evidence_json = parse_stdout_json(&evidence, "persist-evidence output");
    let evidence_ids = evidence_json["evidenceIds"]
        .as_array()
        .expect("evidenceIds should be an array")
        .iter()
        .map(|value| {
            value
                .as_str()
                .expect("evidence id should be a string")
                .to_string()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        evidence_ids.len(),
        3,
        "should persist three Evidence records"
    );

    let evidence_repeat = run_libra_command(
        &[
            "claude-sdk",
            "persist-evidence",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&evidence_repeat, "persist-evidence should be idempotent");
    let evidence_repeat_json =
        parse_stdout_json(&evidence_repeat, "persist-evidence repeat output");
    assert_eq!(
        evidence_repeat_json["evidenceIds"],
        json!(evidence_ids.clone())
    );

    let decision = run_libra_command(
        &[
            "claude-sdk",
            "persist-decision",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&decision, "persist-decision should succeed");
    let decision_json = parse_stdout_json(&decision, "persist-decision output");
    let decision_id = decision_json["decisionId"]
        .as_str()
        .expect("decisionId should be present")
        .to_string();
    assert_eq!(decision_json["decisionType"], json!("checkpoint"));

    let decision_repeat = run_libra_command(
        &[
            "claude-sdk",
            "persist-decision",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&decision_repeat, "persist-decision should be idempotent");
    let decision_repeat_json =
        parse_stdout_json(&decision_repeat, "persist-decision repeat output");
    assert_eq!(
        decision_repeat_json["decisionId"],
        json!(decision_id.clone())
    );

    let task: Task = read_tracked_object(repo.path(), "task", &task_id).await;
    assert_eq!(task.intent().map(|id| id.to_string()), Some(intent_id));

    let formal_run: Run = read_tracked_object(repo.path(), "run", &run_id).await;
    assert_eq!(formal_run.task().to_string(), task_id);

    let evidence_objects = futures::future::join_all(
        evidence_ids
            .iter()
            .map(|id| read_tracked_object::<Evidence>(repo.path(), "evidence", id)),
    )
    .await;
    let evidence_kinds = evidence_objects
        .iter()
        .map(|evidence| evidence.kind().to_string())
        .collect::<Vec<_>>();
    assert!(evidence_kinds.contains(&"provider_session_snapshot".to_string()));
    assert!(evidence_kinds.contains(&"evidence_input_summary".to_string()));
    assert!(evidence_kinds.contains(&"intent_extraction_result".to_string()));
    assert!(
        evidence_objects
            .iter()
            .all(|evidence| evidence.run_id().to_string() == run_id),
        "all Evidence records should point at the bridged run"
    );

    let stored_decision: Decision =
        read_tracked_object(repo.path(), "decision", &decision_id).await;
    assert_eq!(stored_decision.run_id().to_string(), run_id);
    assert_eq!(stored_decision.decision_type().to_string(), "checkpoint");
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_bridge_run_without_intent_binding_creates_standalone_task() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn standalone_bridge() {}\n").expect("write source file");

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
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
            "--node-binary",
            "/bin/sh",
        ],
        repo.path(),
    );
    assert_cli_success(&run, "claude-sdk run should succeed");
    let run_json = parse_stdout_json(&run, "claude-sdk run output");
    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present");

    let bridge = run_libra_command(
        &["claude-sdk", "bridge-run", "--ai-session-id", ai_session_id],
        repo.path(),
    );
    assert_cli_success(&bridge, "bridge-run should succeed without intent binding");
    let bridge_json = parse_stdout_json(&bridge, "bridge-run output");
    assert!(
        bridge_json["intentId"].is_null(),
        "standalone bridge should not attach an intent id"
    );

    let task_id = bridge_json["taskId"]
        .as_str()
        .expect("taskId should be present");
    let task: Task = read_tracked_object(repo.path(), "task", task_id).await;
    assert!(
        task.intent().is_none(),
        "task should not be linked to an intent"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_bridge_run_rejects_missing_explicit_intent_binding() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn missing_intent_binding() {}\n").expect("write source file");

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
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
            "--node-binary",
            "/bin/sh",
        ],
        repo.path(),
    );
    assert_cli_success(&run, "claude-sdk run should succeed");
    let run_json = parse_stdout_json(&run, "claude-sdk run output");
    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present");
    let missing_binding = repo.path().join("missing-intent-binding.json");

    let bridge = run_libra_command(
        &[
            "claude-sdk",
            "bridge-run",
            "--ai-session-id",
            ai_session_id,
            "--intent-binding",
            missing_binding.to_str().expect("binding path utf-8"),
        ],
        repo.path(),
    );
    assert!(
        !bridge.status.success(),
        "bridge-run should reject an explicit missing intent binding"
    );
    assert!(
        String::from_utf8_lossy(&bridge.stderr).contains("does not exist"),
        "error should explain that the requested binding path is missing"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_bridge_run_rejects_mismatched_existing_intent_link() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn delayed_intent_link() {}\n").expect("write source file");

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
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
            "--node-binary",
            "/bin/sh",
        ],
        repo.path(),
    );
    assert_cli_success(&run, "claude-sdk run should succeed");
    let run_json = parse_stdout_json(&run, "claude-sdk run output");
    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present")
        .to_string();

    let standalone_bridge = run_libra_command(
        &[
            "claude-sdk",
            "bridge-run",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(
        &standalone_bridge,
        "standalone bridge-run should succeed before intent persistence",
    );

    let resolve = run_libra_command(
        &[
            "claude-sdk",
            "resolve-extraction",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&resolve, "resolve-extraction should succeed");

    let persist_intent = run_libra_command(
        &[
            "claude-sdk",
            "persist-intent",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&persist_intent, "persist-intent should succeed");
    let persist_intent_json = parse_stdout_json(&persist_intent, "persist-intent output");
    let binding_path = persist_intent_json["bindingPath"]
        .as_str()
        .expect("bindingPath should be present");

    let bridge_with_intent = run_libra_command(
        &[
            "claude-sdk",
            "bridge-run",
            "--ai-session-id",
            &ai_session_id,
            "--intent-binding",
            binding_path,
        ],
        repo.path(),
    );
    assert!(
        !bridge_with_intent.status.success(),
        "bridge-run should reject reusing a standalone binding when a concrete intent is requested"
    );
    assert!(
        String::from_utf8_lossy(&bridge_with_intent.stderr)
            .contains("remove the stale binding to rebuild intentionally"),
        "error should explain how to recover from the stale standalone binding"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_bridge_run_rejects_invalid_binding_schema_on_reuse() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn invalid_binding_schema() {}\n").expect("write source file");

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
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
            "--node-binary",
            "/bin/sh",
        ],
        repo.path(),
    );
    assert_cli_success(&run, "claude-sdk run should succeed");
    let run_json = parse_stdout_json(&run, "claude-sdk run output");
    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present")
        .to_string();

    let bridge = run_libra_command(
        &[
            "claude-sdk",
            "bridge-run",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&bridge, "bridge-run should succeed");
    let bridge_json = parse_stdout_json(&bridge, "bridge-run output");
    let binding_path = PathBuf::from(
        bridge_json["bindingPath"]
            .as_str()
            .expect("bindingPath should be present"),
    );

    let mut binding_json: Value =
        serde_json::from_slice(&fs::read(&binding_path).expect("read formal run binding"))
            .expect("deserialize formal run binding");
    binding_json["schema"] = json!("libra.invalid_binding.v1");
    fs::write(
        &binding_path,
        serde_json::to_vec_pretty(&binding_json).expect("serialize invalid binding"),
    )
    .expect("write invalid binding");

    let bridge_repeat = run_libra_command(
        &[
            "claude-sdk",
            "bridge-run",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert!(
        !bridge_repeat.status.success(),
        "bridge-run should reject a cached binding with the wrong schema"
    );
    assert!(
        String::from_utf8_lossy(&bridge_repeat.stderr)
            .contains("unsupported Claude formal run binding schema"),
        "error should name the invalid cached binding schema"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_formal_bridge_requires_prior_bindings() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn missing_bindings() {}\n").expect("write source file");

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
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
            "--node-binary",
            "/bin/sh",
        ],
        repo.path(),
    );
    assert_cli_success(&run, "claude-sdk run should succeed");
    let run_json = parse_stdout_json(&run, "claude-sdk run output");
    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present");

    let evidence_without_bridge = run_libra_command(
        &[
            "claude-sdk",
            "persist-evidence",
            "--ai-session-id",
            ai_session_id,
        ],
        repo.path(),
    );
    assert!(
        !evidence_without_bridge.status.success(),
        "persist-evidence should reject missing bridge-run binding"
    );
    assert!(
        String::from_utf8_lossy(&evidence_without_bridge.stderr).contains("bridge-run"),
        "error should guide the user toward bridge-run first"
    );

    let bridge = run_libra_command(
        &["claude-sdk", "bridge-run", "--ai-session-id", ai_session_id],
        repo.path(),
    );
    assert_cli_success(&bridge, "bridge-run should succeed");

    let decision_without_evidence = run_libra_command(
        &[
            "claude-sdk",
            "persist-decision",
            "--ai-session-id",
            ai_session_id,
        ],
        repo.path(),
    );
    assert!(
        !decision_without_evidence.status.success(),
        "persist-decision should reject missing evidence binding"
    );
    assert!(
        String::from_utf8_lossy(&decision_without_evidence.stderr).contains("persist-evidence"),
        "error should guide the user toward persist-evidence first"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_formal_decision_maps_invalid_and_timeout_cases() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let invalid_artifact_path = repo.path().join("probe-like-artifact.json");
    fs::write(&invalid_artifact_path, PROBE_LIKE_ARTIFACT).expect("write invalid artifact");
    let invalid_import = run_libra_command(
        &[
            "claude-sdk",
            "import",
            "--artifact",
            invalid_artifact_path
                .to_str()
                .expect("invalid artifact path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(
        &invalid_import,
        "claude-sdk import should succeed for invalid extraction",
    );
    let invalid_import_json = parse_stdout_json(&invalid_import, "invalid import output");
    let invalid_ai_session_id = invalid_import_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present")
        .to_string();

    let invalid_bridge = run_libra_command(
        &[
            "claude-sdk",
            "bridge-run",
            "--ai-session-id",
            &invalid_ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(
        &invalid_bridge,
        "bridge-run should succeed for invalid extraction",
    );
    let invalid_evidence = run_libra_command(
        &[
            "claude-sdk",
            "persist-evidence",
            "--ai-session-id",
            &invalid_ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(
        &invalid_evidence,
        "persist-evidence should succeed for invalid extraction",
    );
    let invalid_decision = run_libra_command(
        &[
            "claude-sdk",
            "persist-decision",
            "--ai-session-id",
            &invalid_ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(
        &invalid_decision,
        "persist-decision should succeed for invalid extraction",
    );
    let invalid_decision_json =
        parse_stdout_json(&invalid_decision, "invalid persist-decision output");
    assert_eq!(invalid_decision_json["decisionType"], json!("abandon"));

    let touched_file = repo.path().join("src").join("timed.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn timed_out() {}\n").expect("write timed source");
    let timed_artifact_path = repo.path().join("timed-artifact.json");
    fs::write(
        &timed_artifact_path,
        serde_json::to_vec_pretty(&timed_out_partial_artifact(repo.path(), &touched_file))
            .expect("serialize timed artifact"),
    )
    .expect("write timed artifact");

    let timed_import = run_libra_command(
        &[
            "claude-sdk",
            "import",
            "--artifact",
            timed_artifact_path
                .to_str()
                .expect("timed artifact path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(
        &timed_import,
        "claude-sdk import should succeed for timed artifact",
    );
    let timed_import_json = parse_stdout_json(&timed_import, "timed import output");
    let timed_ai_session_id = timed_import_json["aiSessionId"]
        .as_str()
        .expect("timed aiSessionId should be present")
        .to_string();

    let timed_bridge = run_libra_command(
        &[
            "claude-sdk",
            "bridge-run",
            "--ai-session-id",
            &timed_ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(
        &timed_bridge,
        "bridge-run should succeed for timed artifact",
    );
    let timed_evidence = run_libra_command(
        &[
            "claude-sdk",
            "persist-evidence",
            "--ai-session-id",
            &timed_ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(
        &timed_evidence,
        "persist-evidence should succeed for timed artifact",
    );
    let timed_decision = run_libra_command(
        &[
            "claude-sdk",
            "persist-decision",
            "--ai-session-id",
            &timed_ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(
        &timed_decision,
        "persist-decision should succeed for timed artifact",
    );
    let timed_decision_json = parse_stdout_json(&timed_decision, "timed persist-decision output");
    assert_eq!(timed_decision_json["decisionType"], json!("retry"));
}
