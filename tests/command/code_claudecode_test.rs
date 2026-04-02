//! Integration tests for `libra code --provider claudecode`.
//!
//! **Layer:** L1 — deterministic, Unix-only (`#[cfg(unix)]`).

use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Arc,
};

use git_internal::internal::object::{provenance::Provenance, task::Task};
use libra::{
    internal::{
        ai::{
            codex::model::{PlanSnapshot, PlanStepSnapshot, TaskSnapshot},
            history::HistoryManager,
            hooks::runtime::AI_SESSION_SCHEMA,
        },
        db,
    },
    utils::{storage::local::LocalStorage, storage_ext::StorageExt, test},
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use serial_test::serial;
use tempfile::tempdir;

use super::{
    assert_cli_success, parse_cli_error_stderr, run_libra_command, run_libra_command_with_stdin,
    run_libra_command_with_stdin_and_env,
};

const SEMANTIC_FULL_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/data/ai/claude_managed_semantic_full_template.json"
));
const DEFAULT_MANAGED_PROMPT: &str = "Bridge a managed Claude Code session into Libra artifacts.";

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r#"'\''"#))
}

fn write_claude_local_settings(repo: &Path, settings: Value) {
    let claude_dir = repo.join(".claude");
    fs::create_dir_all(&claude_dir)
        .unwrap_or_else(|err| panic!("failed to create '{}': {err}", claude_dir.display()));
    fs::write(
        claude_dir.join("settings.local.json"),
        serde_json::to_vec_pretty(&settings)
            .unwrap_or_else(|err| panic!("failed to serialize local Claude settings: {err}")),
    )
    .unwrap_or_else(|err| panic!("failed to write local Claude settings: {err}"));
}

fn seed_local_claude_auth(repo: &Path) {
    write_claude_local_settings(
        repo,
        json!({
            "plansDirectory": ".claude/plans",
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "test-local-token"
            }
        }),
    );
}

fn read_json_file(path: &Path) -> Value {
    let body =
        fs::read_to_string(path).unwrap_or_else(|err| panic!("failed to read JSON file: {err}"));
    serde_json::from_str(&body).unwrap_or_else(|err| panic!("failed to parse JSON file: {err}"))
}

fn list_json_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = fs::read_dir(dir)
        .unwrap_or_else(|err| panic!("failed to read directory '{}': {err}", dir.display()))
        .filter_map(|entry| {
            let entry = entry
                .unwrap_or_else(|err| panic!("failed to read entry in '{}': {err}", dir.display()));
            let path = entry.path();
            (path.extension().and_then(|ext| ext.to_str()) == Some("json")).then_some(path)
        })
        .collect::<Vec<_>>();
    files.sort();
    files
}

fn only_json_file(dir: &Path, context: &str) -> PathBuf {
    let files = list_json_files(dir);
    assert_eq!(
        files.len(),
        1,
        "{context}: expected exactly one JSON file in '{}', found {:?}",
        dir.display(),
        files
    );
    files[0].clone()
}

async fn load_ai_objects<T>(repo: &Path, object_type: &str) -> Vec<(String, T)>
where
    T: DeserializeOwned + Send + Sync,
{
    let libra_dir = repo.join(".libra");
    let storage = Arc::new(LocalStorage::new(libra_dir.join("objects")));
    let db_conn = Arc::new(
        db::establish_connection(
            libra_dir
                .join("libra.db")
                .to_str()
                .expect("db path should be utf-8"),
        )
        .await
        .expect("db connection should succeed"),
    );
    let history = HistoryManager::new(storage.clone(), libra_dir, db_conn);

    let objects = history
        .list_objects(object_type)
        .await
        .unwrap_or_else(|err| panic!("failed to list AI objects of type '{object_type}': {err}"));
    let mut loaded = Vec::with_capacity(objects.len());
    for (id, hash) in objects {
        let value = storage
            .get_json::<T>(&hash)
            .await
            .unwrap_or_else(|err| panic!("failed to read '{object_type}' object '{id}': {err}"));
        loaded.push((id, value));
    }
    loaded
}

async fn load_ai_object<T>(repo: &Path, object_type: &str, object_id: &str) -> T
where
    T: DeserializeOwned + Send + Sync,
{
    let libra_dir = repo.join(".libra");
    let storage = Arc::new(LocalStorage::new(libra_dir.join("objects")));
    let db_conn = Arc::new(
        db::establish_connection(
            libra_dir
                .join("libra.db")
                .to_str()
                .expect("db path should be utf-8"),
        )
        .await
        .expect("db connection should succeed"),
    );
    let history = HistoryManager::new(storage.clone(), libra_dir, db_conn);
    let hash = history
        .get_object_hash(object_type, object_id)
        .await
        .unwrap_or_else(|err| panic!("failed to read object hash for '{object_type}': {err}"))
        .unwrap_or_else(|| panic!("missing AI object for type '{object_type}'"));
    storage
        .get_json::<T>(&hash)
        .await
        .unwrap_or_else(|err| panic!("failed to read '{object_type}' object body: {err}"))
}

fn first_assistant_message_mut(artifact: &mut Value) -> &mut Value {
    artifact["messages"]
        .as_array_mut()
        .expect("messages array")
        .iter_mut()
        .find(|message| message["type"] == json!("assistant"))
        .expect("assistant message in fixture")
}

fn first_system_message_mut_by_subtype<'a>(
    artifact: &'a mut Value,
    subtype: &str,
) -> &'a mut Value {
    artifact["messages"]
        .as_array_mut()
        .expect("messages array")
        .iter_mut()
        .find(|message| message["type"] == json!("system") && message["subtype"] == json!(subtype))
        .unwrap_or_else(|| panic!("system message with subtype '{subtype}' in fixture"))
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

fn semantic_full_artifact(repo: &Path, touched_file: &Path) -> Value {
    let mut artifact: Value = serde_json::from_str(SEMANTIC_FULL_TEMPLATE)
        .unwrap_or_else(|err| panic!("failed to parse managed artifact template: {err}"));
    let replacements = [
        ("__CWD__", json!(repo.to_string_lossy().to_string())),
        (
            "__TOUCHED_FILE__",
            json!(touched_file.to_string_lossy().to_string()),
        ),
        ("__PROMPT__", json!(DEFAULT_MANAGED_PROMPT)),
    ];
    replace_template_slots(&mut artifact, &replacements);
    artifact
}

fn semantic_full_artifact_with_numbered_plan(repo: &Path, touched_file: &Path) -> Value {
    let mut artifact = semantic_full_artifact(repo, touched_file);
    let assistant_text = first_assistant_message_mut(&mut artifact);
    assistant_text["message"]["content"][0]["text"] = json!(
        "1. Inspect the touched Rust source.\n2. Bridge the managed chat turn into formal intent and plan objects.\n3. Verify the persisted bindings can be queried later."
    );
    artifact
}

fn semantic_full_artifact_with_structured_plan(repo: &Path, touched_file: &Path) -> Value {
    let mut artifact = semantic_full_artifact(repo, touched_file);
    let assistant_text = first_assistant_message_mut(&mut artifact);
    assistant_text["message"]["content"][0]["text"] = json!(
        "I will inspect the touched Rust source, bridge the managed chat turn, and verify the persisted bindings."
    );
    artifact["resultMessage"]["structured_output"]["plan"] = json!([
        "Inspect the touched Rust source.",
        {"description": "Bridge the managed chat turn into formal intent and plan objects."},
        {"description": "Verify the persisted bindings can be queried later."}
    ]);
    artifact["resultMessage"]["structured_output"]["planningSummary"] =
        json!("Use the structured plan as the canonical formal-plan source.");
    artifact
}

fn semantic_full_artifact_with_execution_refresh(repo: &Path, touched_file: &Path) -> Value {
    let mut artifact = semantic_full_artifact(repo, touched_file);
    let touched_file = touched_file.to_string_lossy().to_string();
    first_system_message_mut_by_subtype(&mut artifact, "init")["permissionMode"] =
        json!("acceptEdits");
    first_system_message_mut_by_subtype(&mut artifact, "init")["tools"] =
        json!(["Read", "Edit", "Write", "StructuredOutput"]);
    first_system_message_mut_by_subtype(&mut artifact, "status")["permissionMode"] =
        json!("acceptEdits");
    first_assistant_message_mut(&mut artifact)["message"]["content"][0]["text"] = json!(
        "I implemented the AI opponent, localized the UI to Chinese, and refreshed the formal bindings."
    );
    artifact["hookEvents"] = json!([
        {
            "hook": "UserPromptSubmit",
            "input": {
                "session_id": "fixture-managed-session",
                "transcript_path": "/tmp/libra-fixtures/fixture-managed-session.jsonl",
                "cwd": repo.to_string_lossy().to_string(),
                "hook_event_name": "UserPromptSubmit",
                "prompt": "Finish the refreshed managed bridge"
            }
        },
        {
            "hook": "PreToolUse",
            "input": {
                "session_id": "fixture-managed-session",
                "transcript_path": "/tmp/libra-fixtures/fixture-managed-session.jsonl",
                "cwd": repo.to_string_lossy().to_string(),
                "hook_event_name": "PreToolUse",
                "tool_name": "Read",
                "tool_input": {
                    "file_path": touched_file.clone()
                },
                "tool_use_id": "tool-read-refresh"
            }
        },
        {
            "hook": "PostToolUse",
            "input": {
                "session_id": "fixture-managed-session",
                "transcript_path": "/tmp/libra-fixtures/fixture-managed-session.jsonl",
                "cwd": repo.to_string_lossy().to_string(),
                "hook_event_name": "PostToolUse",
                "tool_name": "Read",
                "tool_input": {
                    "file_path": touched_file.clone()
                },
                "tool_response": {
                    "file": {
                        "filePath": touched_file.clone()
                    }
                },
                "tool_use_id": "tool-read-refresh"
            }
        },
        {
            "hook": "PreToolUse",
            "input": {
                "session_id": "fixture-managed-session",
                "transcript_path": "/tmp/libra-fixtures/fixture-managed-session.jsonl",
                "cwd": repo.to_string_lossy().to_string(),
                "hook_event_name": "PreToolUse",
                "tool_name": "Edit",
                "tool_input": {
                    "file_path": touched_file.clone()
                },
                "tool_use_id": "tool-edit-refresh"
            }
        },
        {
            "hook": "PostToolUse",
            "input": {
                "session_id": "fixture-managed-session",
                "transcript_path": "/tmp/libra-fixtures/fixture-managed-session.jsonl",
                "cwd": repo.to_string_lossy().to_string(),
                "hook_event_name": "PostToolUse",
                "tool_name": "Edit",
                "tool_input": {
                    "file_path": touched_file.clone()
                },
                "tool_response": {
                    "ok": true
                },
                "tool_use_id": "tool-edit-refresh"
            }
        },
        {
            "hook": "Stop",
            "input": {
                "session_id": "fixture-managed-session",
                "transcript_path": "/tmp/libra-fixtures/fixture-managed-session.jsonl",
                "cwd": repo.to_string_lossy().to_string(),
                "hook_event_name": "Stop"
            }
        }
    ]);
    artifact["resultMessage"]["usage"] = json!({
        "input_tokens": 654,
        "output_tokens": 321
    });
    artifact["resultMessage"]["structured_output"]["summary"] =
        json!("Implement AI mode and synchronize the formal graph");
    artifact["resultMessage"]["structured_output"]["problemStatement"] = json!(
        "The managed Claude session must refresh its canonical intent, plan, run, and ai_session objects after execution."
    );
    artifact["resultMessage"]["structured_output"]["objectives"] = json!([
        "Refresh the ai_session snapshot",
        "Rebuild the canonical formal run family",
        "Persist the latest tool invocation objects"
    ]);
    artifact["resultMessage"]["structured_output"]["inScope"] =
        json!(["Cargo.toml", "src/game.rs", "src/main.rs"]);
    artifact["resultMessage"]["structured_output"]["planningSummary"] =
        json!("Use the refreshed structured plan as the canonical formal graph source.");
    artifact["resultMessage"]["structured_output"]["plan"] = json!([
        "Refresh the ai_session blob with the latest execution transcript",
        {"description": "Rebuild the canonical intent and formal run binding from the refreshed extraction"},
        "Persist the latest tool invocations and provenance for the execution turn"
    ]);
    artifact
}

fn write_executable_helper_script(path: &Path, script: &str) {
    fs::write(path, script)
        .unwrap_or_else(|err| panic!("failed to write helper script '{}': {err}", path.display()));
    let mut permissions = fs::metadata(path)
        .unwrap_or_else(|err| panic!("failed to stat helper script '{}': {err}", path.display()))
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap_or_else(|err| {
        panic!(
            "failed to set executable permissions on helper script '{}': {err}",
            path.display()
        )
    });
}

fn write_streaming_event_helper(
    path: &Path,
    request_path: &Path,
    events: &[Value],
    trailing_shell: &str,
) {
    let request_rendered = shell_single_quote(&request_path.to_string_lossy());
    let rendered_events = events
        .iter()
        .map(|event| {
            shell_single_quote(
                &serde_json::to_string(event)
                    .unwrap_or_else(|err| panic!("failed to serialize helper event: {err}")),
            )
        })
        .collect::<Vec<_>>();
    let mut script = format!(
        "#!/bin/sh\nrequest_tmp=$(mktemp)\ncat > \"$request_tmp\"\npython3 - \"$request_tmp\" {request_rendered} <<'PY'\nimport json\nimport os\nimport sys\n\nsource_path, dest_path = sys.argv[1:3]\nwith open(source_path, 'r', encoding='utf-8') as handle:\n    request = json.load(handle)\nfor env_key, field in (\n    ('LIBRA_CLAUDE_HELPER_RESUME', 'resume'),\n    ('LIBRA_CLAUDE_HELPER_SESSION_ID', 'sessionId'),\n    ('LIBRA_CLAUDE_HELPER_RESUME_SESSION_AT', 'resumeSessionAt'),\n):\n    value = os.environ.get(env_key)\n    if value:\n        request[field] = value\nwith open(dest_path, 'w', encoding='utf-8') as handle:\n    json.dump(request, handle)\nPY\nrm -f \"$request_tmp\"\n"
    );
    if !rendered_events.is_empty() {
        script.push_str("printf '%s\\n' ");
        script.push_str(&rendered_events.join(" "));
        script.push('\n');
    }
    if !trailing_shell.is_empty() {
        script.push_str(trailing_shell);
        if !trailing_shell.ends_with('\n') {
            script.push('\n');
        }
    }
    write_executable_helper_script(path, &script);
}

fn write_streaming_request_capture_helper(path: &Path, artifact: &Value, request_path: &Path) {
    write_streaming_event_helper(
        path,
        request_path,
        &[
            json!({
                "event": "runtime_snapshot",
                "artifact": artifact,
            }),
            json!({
                "event": "final_artifact",
                "artifact": artifact,
            }),
        ],
        "",
    );
}

#[tokio::test]
#[serial]
async fn test_code_claudecode_chat_persists_managed_artifact() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;
    seed_local_claude_auth(repo.path());

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn code_claudecode_chat() {}\n").expect("write source file");

    let request_path = repo.path().join("code-claudecode-chat-request.json");
    let helper_path = repo.path().join("capture-code-claudecode-chat-helper.sh");
    let artifact = semantic_full_artifact(repo.path(), &touched_file);
    write_streaming_request_capture_helper(&helper_path, &artifact, &request_path);

    let output = run_libra_command_with_stdin(
        &[
            "code",
            "--provider",
            "claudecode",
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
            "--model",
            "claude-sonnet-4-6",
        ],
        repo.path(),
        "Inspect src/lib.rs\n",
    );
    assert_cli_success(
        &output,
        "code --provider claudecode should succeed for a single managed chat turn",
    );

    let helper_request = read_json_file(&request_path);
    assert_eq!(helper_request["mode"], json!("queryStream"));
    assert_eq!(helper_request["prompt"], json!("Inspect src/lib.rs"));
    assert!(helper_request.get("systemPrompt").is_some());
    assert!(helper_request.get("outputSchema").is_some());
    assert_eq!(
        helper_request["permissionMode"],
        json!("plan"),
        "interactive claudecode sessions should keep the provider in plan mode until execution is approved"
    );
    assert_eq!(
        helper_request["libraPlanMode"],
        json!(true),
        "interactive claudecode sessions should enable Libra-local plan mode"
    );
    assert!(
        helper_request.get("allowedTools").is_none(),
        "managed chat should not serialize the full tool catalog as auto-approved tools"
    );

    let managed_artifact_dir = repo.path().join(".libra").join("managed-artifacts");
    let managed_artifact = only_json_file(
        &managed_artifact_dir,
        "code claudecode chat should persist a managed artifact",
    );
    let managed_artifact_json = read_json_file(&managed_artifact);
    assert_eq!(
        managed_artifact_json["cwd"],
        json!(repo.path().to_string_lossy().to_string())
    );
    assert_eq!(
        managed_artifact_json["prompt"],
        json!(DEFAULT_MANAGED_PROMPT)
    );
    assert_eq!(managed_artifact_json["helperTimedOut"], json!(false));
    assert_eq!(
        helper_request.get("enableFileCheckpointing"),
        None,
        "managed helper request should omit file checkpointing when the default is disabled"
    );
    let audit_bundle_dir = repo.path().join(".libra").join("audit-bundles");
    let audit_bundle = only_json_file(
        &audit_bundle_dir,
        "code claudecode chat should persist an audit bundle",
    );
    let audit_bundle_json = read_json_file(&audit_bundle);
    assert_eq!(
        audit_bundle_json["schema"],
        json!("libra.claude_managed_audit_bundle.v1")
    );
    assert_eq!(audit_bundle_json["provider"], json!("claude"));
    assert_eq!(
        audit_bundle_json["rawArtifact"]["prompt"],
        json!(DEFAULT_MANAGED_PROMPT)
    );
    assert_eq!(
        audit_bundle_json["bridge"]["aiSession"]["schema"],
        json!(AI_SESSION_SCHEMA)
    );
}

#[tokio::test]
#[serial]
async fn test_code_claudecode_chat_auto_finalizes_intent_and_plan_bindings() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;
    seed_local_claude_auth(repo.path());

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn code_claudecode_plan_binding() {}\n")
        .expect("write source file");

    let request_path = repo.path().join("code-claudecode-chat-plan-request.json");
    let helper_path = repo
        .path()
        .join("capture-code-claudecode-chat-plan-helper.sh");
    let artifact = semantic_full_artifact_with_numbered_plan(repo.path(), &touched_file);
    write_streaming_request_capture_helper(&helper_path, &artifact, &request_path);

    let output = run_libra_command_with_stdin(
        &[
            "code",
            "--provider",
            "claudecode",
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
            "--model",
            "claude-sonnet-4-6",
        ],
        repo.path(),
        "Plan the managed bridge changes\n",
    );
    assert_cli_success(
        &output,
        "code --provider claudecode should auto-finalize intent and plan bindings for chat",
    );

    let helper_request = read_json_file(&request_path);
    assert!(helper_request.get("outputSchema").is_some());
    assert_eq!(
        helper_request["permissionMode"],
        json!("plan"),
        "interactive claudecode sessions should keep the provider in plan mode until execution is approved"
    );
    assert_eq!(
        helper_request["libraPlanMode"],
        json!(true),
        "interactive claudecode sessions should enable Libra-local plan mode"
    );
    assert!(
        helper_request.get("allowedTools").is_none(),
        "planning-first chat should not auto-approve the full execution tool catalog"
    );

    let resolution_dir = repo.path().join(".libra").join("intent-resolutions");
    let resolution = only_json_file(
        &resolution_dir,
        "claudecode chat should persist a resolved intent artifact",
    );
    assert!(resolution.exists());
    let resolution_json = read_json_file(&resolution);

    let intent_input_dir = repo.path().join(".libra").join("intent-inputs");
    let intent_input = only_json_file(
        &intent_input_dir,
        "claudecode chat should persist an intent input binding",
    );
    assert!(intent_input.exists());
    let intent_input_json = read_json_file(&intent_input);

    let run_binding_dir = repo.path().join(".libra").join("claude-run-bindings");
    let run_binding_path = only_json_file(
        &run_binding_dir,
        "claudecode chat should persist a formal run binding",
    );
    let run_binding = read_json_file(&run_binding_path);
    assert!(
        run_binding["intentId"].as_str().is_some(),
        "formal run binding should capture an intentId after chat auto-finalize"
    );
    assert!(
        run_binding["planId"].as_str().is_some(),
        "formal run binding should capture a planId when the assistant emits a numbered plan"
    );
    let intent_id = run_binding["intentId"]
        .as_str()
        .expect("formal run binding should expose intentId");
    let resolution_summary = resolution_json["summary"]
        .as_str()
        .expect("resolved intent artifact should include a summary");
    let intent_input_summary = intent_input_json["summary"]
        .as_str()
        .expect("intent input binding should include a summary");
    let run_binding_summary = run_binding["summary"]
        .as_str()
        .expect("formal run binding should include a summary");
    assert!(
        resolution_summary.contains(intent_id),
        "resolved intent summary should contain the persisted intent id: {resolution_summary}"
    );
    assert!(
        intent_input_summary.contains(intent_id),
        "intent input summary should contain the persisted intent id: {intent_input_summary}"
    );
    assert!(
        run_binding_summary.contains(intent_id),
        "run binding summary should contain the persisted intent id: {run_binding_summary}"
    );
}

#[tokio::test]
#[serial]
async fn test_code_claudecode_chat_prefers_structured_plan_and_keeps_snapshot_dependencies_consistent()
 {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;
    seed_local_claude_auth(repo.path());

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(
        &touched_file,
        "pub fn code_claudecode_structured_plan() {}\n",
    )
    .expect("write source file");

    let request_path = repo
        .path()
        .join("code-claudecode-chat-structured-plan-request.json");
    let helper_path = repo
        .path()
        .join("capture-code-claudecode-chat-structured-plan-helper.sh");
    let mut artifact = semantic_full_artifact(repo.path(), &touched_file);
    let assistant_text = first_assistant_message_mut(&mut artifact);
    assistant_text["message"]["content"][0]["text"] =
        json!("1. Wrong raw step one.\n2. Wrong raw step two.\n3. Wrong raw step three.");
    artifact["resultMessage"]["structured_output"]["plan"] = json!([
        "Inspect the touched Rust source",
        {"description": "Bridge the managed chat turn into formal intent and plan objects"},
        "Verify the persisted bindings can be queried later"
    ]);
    write_streaming_request_capture_helper(&helper_path, &artifact, &request_path);

    let output = run_libra_command_with_stdin(
        &[
            "code",
            "--provider",
            "claudecode",
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
            "--model",
            "claude-sonnet-4-6",
        ],
        repo.path(),
        "Plan the managed bridge changes\n",
    );
    assert_cli_success(
        &output,
        "code --provider claudecode should prefer structured_output.plan for formal plans",
    );

    let run_binding_path = only_json_file(
        &repo.path().join(".libra").join("claude-run-bindings"),
        "claudecode chat should persist a formal run binding",
    );
    let run_binding = read_json_file(&run_binding_path);
    let plan_id = run_binding["planId"]
        .as_str()
        .expect("structured plan should create a formal plan")
        .to_string();

    let expected_steps = vec![
        "Inspect the touched Rust source".to_string(),
        "Bridge the managed chat turn into formal intent and plan objects".to_string(),
        "Verify the persisted bindings can be queried later".to_string(),
    ];

    let plan_snapshots = load_ai_objects::<PlanSnapshot>(repo.path(), "plan_snapshot").await;
    let plan_snapshot = plan_snapshots
        .into_iter()
        .find(|(id, _)| id == &plan_id)
        .map(|(_, snapshot)| snapshot)
        .expect("plan snapshot should exist");
    assert_eq!(plan_snapshot.step_text, expected_steps.join("\n"));

    let mut plan_step_snapshots =
        load_ai_objects::<PlanStepSnapshot>(repo.path(), "plan_step_snapshot")
            .await
            .into_iter()
            .map(|(_, snapshot)| snapshot)
            .filter(|snapshot| snapshot.plan_id == plan_id)
            .collect::<Vec<_>>();
    plan_step_snapshots.sort_by_key(|snapshot| snapshot.ordinal);
    assert_eq!(
        plan_step_snapshots
            .iter()
            .map(|snapshot| snapshot.text.clone())
            .collect::<Vec<_>>(),
        expected_steps
    );

    let step_task_ids = load_ai_objects::<Task>(repo.path(), "task")
        .await
        .into_iter()
        .filter_map(|(_, task)| {
            task.origin_step_id()
                .map(|step_id| (step_id.to_string(), task.header().object_id().to_string()))
        })
        .collect::<BTreeMap<_, _>>();

    let step_ordinals = plan_step_snapshots
        .iter()
        .map(|snapshot| (snapshot.id.clone(), snapshot.ordinal))
        .collect::<BTreeMap<_, _>>();

    let mut task_snapshots = load_ai_objects::<TaskSnapshot>(repo.path(), "task_snapshot")
        .await
        .into_iter()
        .map(|(_, snapshot)| snapshot)
        .filter(|snapshot| snapshot.plan_id.as_deref() == Some(plan_id.as_str()))
        .collect::<Vec<_>>();
    task_snapshots.sort_by_key(|snapshot| {
        let step_id = snapshot
            .origin_step_id
            .as_ref()
            .expect("derived task snapshot should include origin_step_id");
        *step_ordinals
            .get(step_id)
            .expect("task snapshot step should exist in plan_step_snapshot")
    });

    assert_eq!(
        task_snapshots.len(),
        3,
        "expected one task snapshot per plan step"
    );
    assert!(
        task_snapshots[0].dependencies.is_empty(),
        "first plan-step task snapshot should not depend on another task"
    );

    let first_step_id = task_snapshots[0]
        .origin_step_id
        .as_ref()
        .expect("first snapshot origin_step_id");
    let second_step_id = task_snapshots[1]
        .origin_step_id
        .as_ref()
        .expect("second snapshot origin_step_id");
    let second_expected_dependency = step_task_ids
        .get(first_step_id)
        .expect("first step task should exist")
        .clone();
    let third_expected_dependency = step_task_ids
        .get(second_step_id)
        .expect("second step task should exist")
        .clone();

    assert_eq!(
        task_snapshots[1].dependencies,
        vec![second_expected_dependency]
    );
    assert_eq!(
        task_snapshots[2].dependencies,
        vec![third_expected_dependency]
    );
}

#[tokio::test]
#[serial]
async fn test_code_claudecode_chat_filters_empty_reasoning_objects() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;
    seed_local_claude_auth(repo.path());

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn code_claudecode_reasoning() {}\n").expect("write source file");

    let request_path = repo
        .path()
        .join("code-claudecode-chat-reasoning-request.json");
    let helper_path = repo
        .path()
        .join("capture-code-claudecode-chat-reasoning-helper.sh");
    let mut artifact = semantic_full_artifact(repo.path(), &touched_file);
    let assistant_message = first_assistant_message_mut(&mut artifact);
    assistant_message["message"]["content"] = json!([
        {
            "type": "thinking",
            "thinking": "   \n\t   ",
            "signature": "sig-empty"
        },
        {
            "type": "thinking",
            "thinking": "Need to inspect src/lib.rs before bridging objects.",
            "signature": "sig-meaningful"
        },
        {
            "type": "text",
            "text": "I inspected the repository and prepared a structured draft."
        }
    ]);
    write_streaming_request_capture_helper(&helper_path, &artifact, &request_path);

    let output = run_libra_command_with_stdin(
        &[
            "code",
            "--provider",
            "claudecode",
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
            "--model",
            "claude-sonnet-4-6",
        ],
        repo.path(),
        "Inspect src/lib.rs\n",
    );
    assert_cli_success(
        &output,
        "code --provider claudecode should ignore empty reasoning blocks",
    );

    let reasoning_objects = load_ai_objects::<Value>(repo.path(), "reasoning").await;
    assert_eq!(
        reasoning_objects.len(),
        1,
        "only non-empty reasoning blocks should be persisted"
    );
    let reasoning = &reasoning_objects[0].1;
    assert_eq!(
        reasoning["text"],
        json!("Need to inspect src/lib.rs before bridging objects.")
    );
    assert_eq!(
        reasoning["summary"],
        json!(["Need to inspect src/lib.rs before bridging objects."])
    );
}

#[tokio::test]
#[serial]
async fn test_code_claudecode_chat_auto_finalizes_plan_bindings_from_structured_output() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;
    seed_local_claude_auth(repo.path());

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(
        &touched_file,
        "pub fn code_claudecode_structured_plan_binding() {}\n",
    )
    .expect("write source file");

    let request_path = repo
        .path()
        .join("code-claudecode-chat-structured-plan-request.json");
    let helper_path = repo
        .path()
        .join("capture-code-claudecode-chat-structured-plan-helper.sh");
    let artifact = semantic_full_artifact_with_structured_plan(repo.path(), &touched_file);
    write_streaming_request_capture_helper(&helper_path, &artifact, &request_path);

    let output = run_libra_command_with_stdin(
        &[
            "code",
            "--provider",
            "claudecode",
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
            "--model",
            "claude-sonnet-4-6",
        ],
        repo.path(),
        "Plan the managed bridge changes\n",
    );
    assert_cli_success(
        &output,
        "code --provider claudecode should auto-finalize a formal plan from structured output",
    );

    let run_binding_dir = repo.path().join(".libra").join("claude-run-bindings");
    let run_binding_path = only_json_file(
        &run_binding_dir,
        "claudecode chat should persist a formal run binding for structured plans",
    );
    let run_binding = read_json_file(&run_binding_path);
    assert!(
        run_binding["intentId"].as_str().is_some(),
        "formal run binding should capture an intentId after chat auto-finalize"
    );
    assert!(
        run_binding["planId"].as_str().is_some(),
        "formal run binding should capture a planId when structured_output.plan is present"
    );
}

#[tokio::test]
#[serial]
async fn test_code_claudecode_chat_refreshes_canonical_formal_graph_after_execution_turn() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;
    seed_local_claude_auth(repo.path());

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(
        &touched_file,
        "pub fn code_claudecode_formal_graph_refresh() {}\n",
    )
    .expect("write source file");

    let first_request_path = repo.path().join("code-claudecode-chat-first-request.json");
    let first_helper_path = repo
        .path()
        .join("capture-code-claudecode-chat-first-helper.sh");
    let first_artifact = semantic_full_artifact_with_structured_plan(repo.path(), &touched_file);
    write_streaming_request_capture_helper(
        &first_helper_path,
        &first_artifact,
        &first_request_path,
    );

    let first_output = run_libra_command_with_stdin(
        &[
            "code",
            "--provider",
            "claudecode",
            "--helper-path",
            first_helper_path.to_str().expect("helper path utf-8"),
            "--model",
            "claude-sonnet-4-6",
        ],
        repo.path(),
        "Plan the refreshed managed bridge\n",
    );
    assert_cli_success(
        &first_output,
        "first claudecode turn should materialize the initial canonical formal graph",
    );

    let run_binding_path = only_json_file(
        &repo.path().join(".libra").join("claude-run-bindings"),
        "initial claudecode turn should persist a formal run binding",
    );
    let first_run_binding = read_json_file(&run_binding_path);
    let ai_session_id = first_run_binding["aiSessionId"]
        .as_str()
        .expect("run binding should expose aiSessionId")
        .to_string();
    let initial_run_id = first_run_binding["runId"]
        .as_str()
        .expect("run binding should expose runId")
        .to_string();
    let initial_plan_id = first_run_binding["planId"]
        .as_str()
        .expect("initial planning turn should expose planId")
        .to_string();

    let second_request_path = repo.path().join("code-claudecode-chat-second-request.json");
    let second_helper_path = repo
        .path()
        .join("capture-code-claudecode-chat-second-helper.sh");
    let second_artifact = semantic_full_artifact_with_execution_refresh(repo.path(), &touched_file);
    write_streaming_request_capture_helper(
        &second_helper_path,
        &second_artifact,
        &second_request_path,
    );

    let second_output = run_libra_command_with_stdin(
        &[
            "code",
            "--provider",
            "claudecode",
            "--helper-path",
            second_helper_path.to_str().expect("helper path utf-8"),
            "--model",
            "claude-sonnet-4-6",
            "--permission-mode",
            "acceptEdits",
        ],
        repo.path(),
        "Implement the approved managed bridge changes\n",
    );
    assert_cli_success(
        &second_output,
        "second claudecode turn should refresh the canonical formal graph from the execution turn",
    );

    let refreshed_run_binding = read_json_file(&run_binding_path);
    let refreshed_run_id = refreshed_run_binding["runId"]
        .as_str()
        .expect("refreshed run binding should expose runId");
    let refreshed_plan_id = refreshed_run_binding["planId"]
        .as_str()
        .expect("refreshed run binding should expose planId");

    assert_ne!(
        refreshed_run_id, initial_run_id,
        "execution turn should rebuild the canonical formal run binding"
    );
    assert_ne!(
        refreshed_plan_id, initial_plan_id,
        "execution turn should refresh the canonical formal plan binding"
    );

    let ai_session = load_ai_object::<Value>(repo.path(), "ai_session", &ai_session_id).await;
    assert_eq!(
        ai_session["summary"]["last_assistant_message"],
        json!(
            "I implemented the AI opponent, localized the UI to Chinese, and refreshed the formal bindings."
        )
    );
    assert_eq!(
        ai_session["transcript"]["raw_event_count"],
        json!(6),
        "ai_session should be overwritten with the latest execution hook stream"
    );

    let refreshed_plan_snapshot =
        load_ai_object::<PlanSnapshot>(repo.path(), "plan_snapshot", refreshed_plan_id).await;
    assert_eq!(
        refreshed_plan_snapshot.step_text,
        [
            "Refresh the ai_session blob with the latest execution transcript",
            "Rebuild the canonical intent and formal run binding from the refreshed extraction",
            "Persist the latest tool invocations and provenance for the execution turn",
        ]
        .join("\n")
    );

    let provenance_objects = load_ai_objects::<Provenance>(repo.path(), "provenance").await;
    assert!(
        provenance_objects.iter().any(|(_, provenance)| {
            provenance.run_id().to_string() == refreshed_run_id
                && provenance
                    .parameters()
                    .and_then(|parameters| parameters.get("permissionMode"))
                    == Some(&json!("acceptEdits"))
        }),
        "refreshed run should carry execution-mode provenance"
    );

    let tool_invocation_binding = read_json_file(
        &repo
            .path()
            .join(".libra")
            .join("claude-tool-invocation-bindings")
            .join(format!("{ai_session_id}.json")),
    );
    assert_eq!(tool_invocation_binding["runId"], json!(refreshed_run_id));
    assert_eq!(
        tool_invocation_binding["invocations"]
            .as_array()
            .map(Vec::len),
        Some(2),
        "canonical tool invocation binding should point at the latest execution turn"
    );
    assert!(
        tool_invocation_binding["invocations"]
            .as_array()
            .expect("tool invocation binding entries")
            .iter()
            .any(|entry| entry["toolUseId"] == json!("tool-edit-refresh")),
        "latest execution tool invocation should be bound to the canonical run"
    );
}

#[tokio::test]
#[serial]
async fn test_code_claudecode_forwards_resume_session_controls() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;
    seed_local_claude_auth(repo.path());

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn code_resume_controls() {}\n").expect("write source file");

    let request_path = repo.path().join("code-claudecode-request.json");
    let helper_path = repo.path().join("capture-code-claudecode-helper.sh");
    let artifact = semantic_full_artifact(repo.path(), &touched_file);
    write_streaming_request_capture_helper(&helper_path, &artifact, &request_path);

    let resume_id = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    let forked_session_id = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    let resume_message_id = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
    let output = run_libra_command_with_stdin(
        &[
            "code",
            "--provider",
            "claudecode",
            "--resume-session",
            resume_id,
            "--fork-session",
            "--session-id",
            forked_session_id,
            "--resume-at",
            resume_message_id,
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
        "Inspect src/lib.rs\n",
    );
    assert_cli_success(
        &output,
        "code --provider claudecode should forward resume-session controls",
    );

    let helper_request = read_json_file(&request_path);
    assert_eq!(helper_request["mode"], json!("queryStream"));
    assert_eq!(helper_request["resume"], json!(resume_id));
    assert_eq!(helper_request["forkSession"], json!(true));
    assert_eq!(helper_request["sessionId"], json!(forked_session_id));
    assert_eq!(helper_request["resumeSessionAt"], json!(resume_message_id));
    assert!(helper_request.get("continue").is_none());
}

#[tokio::test]
#[serial]
async fn test_code_claudecode_bootstraps_project_settings_and_reports_process_env_credentials() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn code_project_bootstrap() {}\n").expect("write source file");

    let request_path = repo.path().join("code-claudecode-bootstrap-request.json");
    let helper_path = repo
        .path()
        .join("capture-code-claudecode-bootstrap-helper.sh");
    let artifact = semantic_full_artifact(repo.path(), &touched_file);
    write_streaming_request_capture_helper(&helper_path, &artifact, &request_path);

    let output = run_libra_command_with_stdin_and_env(
        &[
            "code",
            "--provider",
            "claudecode",
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
            "--model",
            "claude-sonnet-4-6",
        ],
        repo.path(),
        "Inspect src/lib.rs\n",
        &[("ANTHROPIC_AUTH_TOKEN", "process-token")],
    );
    assert_cli_success(
        &output,
        "code --provider claudecode should bootstrap project settings on first launch",
    );

    let claude_dir = repo.path().join(".claude");
    assert!(claude_dir.is_dir(), "bootstrap should create .claude/");
    assert!(
        claude_dir.join("settings.json").is_file(),
        "bootstrap should create shared Claude settings"
    );
    assert!(
        claude_dir.join("settings.local.json").is_file(),
        "bootstrap should create local Claude settings"
    );
    assert!(
        claude_dir.join("plans").is_dir(),
        "bootstrap should create the project-scoped plans directory"
    );

    let shared_settings = read_json_file(&claude_dir.join("settings.json"));
    let deny_rules = shared_settings["permissions"]["deny"]
        .as_array()
        .expect("shared permissions.deny should be an array")
        .iter()
        .map(|value| value.as_str().expect("deny rule string").to_string())
        .collect::<Vec<_>>();
    assert!(
        deny_rules.contains(&"Read(/.libra/**)".to_string()),
        "shared settings should deny Claude Read access to .libra"
    );
    assert!(
        deny_rules.contains(&"Edit(/.libra/**)".to_string()),
        "shared settings should deny Claude Edit access to .libra"
    );

    let local_settings = read_json_file(&claude_dir.join("settings.local.json"));
    assert_eq!(
        local_settings["plansDirectory"],
        json!(".claude/plans"),
        "bootstrap should create the project-local plans directory default"
    );

    let helper_request = read_json_file(&request_path);
    assert_eq!(
        helper_request["credentialSource"],
        json!("process_env_auth_token")
    );
    assert!(
        helper_request.get("providerEnvOverrides").is_none(),
        "process-env credentials should not require local override injection"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Claude project settings:"),
        "startup notice should mention project bootstrap state: {stderr}"
    );
    assert!(
        stderr.contains("process env.ANTHROPIC_AUTH_TOKEN"),
        "startup notice should identify the credential source: {stderr}"
    );
}

#[tokio::test]
#[serial]
async fn test_code_claudecode_rejects_base_url_without_credentials_before_helper_launch() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;
    write_claude_local_settings(
        repo.path(),
        json!({
            "plansDirectory": ".claude/plans",
            "env": {
                "ANTHROPIC_BASE_URL": "https://gateway.example.test"
            }
        }),
    );

    let output = run_libra_command_with_stdin(
        &[
            "code",
            "--provider",
            "claudecode",
            "--model",
            "claude-sonnet-4-6",
        ],
        repo.path(),
        "Inspect src/lib.rs\n",
    );
    assert!(
        !output.status.success(),
        "missing credentials should fail before helper launch"
    );

    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.category, "auth");
    assert!(
        report.message.contains("missing Anthropic credentials"),
        "structured auth error should mention missing credentials: {:?}",
        report.message
    );
    assert!(
        stderr.contains("missing Anthropic credentials"),
        "human-readable stderr should explain the missing credentials: {stderr}"
    );
}

#[tokio::test]
#[serial]
async fn test_code_claudecode_surfaces_helper_timeout_artifact_failure() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;
    seed_local_claude_auth(repo.path());

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn code_helper_timeout() {}\n").expect("write source file");

    let request_path = repo.path().join("code-claudecode-timeout-request.json");
    let helper_path = repo
        .path()
        .join("capture-code-claudecode-timeout-helper.sh");
    let mut artifact = semantic_full_artifact(repo.path(), &touched_file);
    artifact["helperTimedOut"] = json!(true);
    artifact["resultMessage"] = Value::Null;
    write_streaming_request_capture_helper(&helper_path, &artifact, &request_path);

    let output = run_libra_command_with_stdin(
        &[
            "code",
            "--provider",
            "claudecode",
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
            "--model",
            "claude-sonnet-4-6",
        ],
        repo.path(),
        "Inspect src/lib.rs\n",
    );
    assert!(
        !output.status.success(),
        "managed helper timeout artifacts should fail the turn"
    );

    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert!(
        report.message.contains("Claude Code helper timed out"),
        "structured error should mention helper timeout: {:?}",
        report.message
    );
    assert!(
        stderr.contains("Claude Code helper timed out"),
        "human-readable stderr should mention helper timeout: {stderr}"
    );
}

#[tokio::test]
#[serial]
async fn test_code_claudecode_surfaces_nonzero_helper_exit() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;
    seed_local_claude_auth(repo.path());

    let request_path = repo.path().join("code-claudecode-helper-exit-request.json");
    let helper_path = repo.path().join("capture-code-claudecode-helper-exit.sh");
    write_streaming_event_helper(
        &helper_path,
        &request_path,
        &[],
        "printf '%s\\n' 'helper exited unexpectedly' >&2\nexit 17\n",
    );

    let output = run_libra_command_with_stdin(
        &[
            "code",
            "--provider",
            "claudecode",
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
            "--model",
            "claude-sonnet-4-6",
        ],
        repo.path(),
        "Inspect src/lib.rs\n",
    );
    assert!(
        !output.status.success(),
        "non-zero helper exits should fail the turn"
    );

    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert!(
        report
            .message
            .contains("Claude Code helper failed with status"),
        "structured error should mention helper exit status: {:?}",
        report.message
    );
    assert!(
        report.message.contains("helper exited unexpectedly"),
        "structured error should preserve helper stderr: {:?}",
        report.message
    );
    assert!(
        stderr.contains("helper exited unexpectedly"),
        "human-readable stderr should preserve helper stderr: {stderr}"
    );
}

#[tokio::test]
#[serial]
async fn test_code_claudecode_rejects_malformed_final_artifact() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;
    seed_local_claude_auth(repo.path());

    let request_path = repo
        .path()
        .join("code-claudecode-malformed-artifact-request.json");
    let helper_path = repo
        .path()
        .join("capture-code-claudecode-malformed-artifact-helper.sh");
    write_streaming_event_helper(
        &helper_path,
        &request_path,
        &[json!({
            "event": "final_artifact",
            "artifact": {
                "cwd": 42,
                "prompt": ["unexpected"],
            }
        })],
        "",
    );

    let output = run_libra_command_with_stdin(
        &[
            "code",
            "--provider",
            "claudecode",
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
            "--model",
            "claude-sonnet-4-6",
        ],
        repo.path(),
        "Inspect src/lib.rs\n",
    );
    assert!(
        !output.status.success(),
        "malformed final artifacts should fail the turn"
    );

    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert!(
        report
            .message
            .contains("failed to parse final managed artifact from helper"),
        "structured error should mention malformed final artifacts: {:?}",
        report.message
    );
    assert!(
        stderr.contains("failed to parse final managed artifact from helper"),
        "human-readable stderr should mention malformed final artifacts: {stderr}"
    );
}

#[tokio::test]
#[serial]
async fn test_removed_claude_sdk_command_is_not_listed_in_help() {
    let repo = tempdir().expect("failed to create repo root");
    let output = run_libra_command(&["--help"], repo.path());
    assert_cli_success(&output, "libra --help should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("claude-sdk"),
        "claude-sdk should not be listed in public help output"
    );
}
