//! Step 1.0 single-agent baseline tests.
//!
//! These tests pin the current deterministic agent/runtime behavior before the
//! Step 1 safety, JSON repair, semantic-tool, and session-storage refactors. They
//! intentionally use local tempdirs and scripted models so the baseline does not
//! depend on live provider credentials.

use std::{
    collections::VecDeque,
    fs,
    path::Path,
    process::{Command, Output},
    sync::{Arc, Mutex},
};

use libra::internal::ai::{
    agent::{ToolLoopConfig, ToolLoopObserver, run_tool_loop_with_history_and_observer},
    completion::{
        AssistantContent, CompletionError, CompletionModel, CompletionRequest, CompletionResponse,
        Function, Message, OneOrMany, Text, ToolCall, UserContent,
    },
    session::{SessionState, SessionStore},
    tools::{
        ToolInvocation, ToolOutput, ToolPayload, ToolRegistry,
        handlers::{ApplyPatchHandler, ListDirHandler, ReadFileHandler, ShellHandler},
        registry::ToolHandler,
    },
};
use serde_json::{Value, json};
use tempfile::TempDir;

#[derive(Clone, Debug)]
struct ScriptedToolModel {
    steps: Arc<Mutex<VecDeque<Vec<AssistantContent>>>>,
    seen_tools: Arc<Mutex<Vec<Vec<String>>>>,
}

impl ScriptedToolModel {
    fn new(steps: impl IntoIterator<Item = Vec<AssistantContent>>) -> Self {
        Self {
            steps: Arc::new(Mutex::new(steps.into_iter().collect())),
            seen_tools: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn seen_tools(&self) -> Vec<Vec<String>> {
        // INVARIANT: tests run on a single tokio task; the lock cannot be poisoned.
        self.seen_tools.lock().unwrap().clone()
    }
}

impl CompletionModel for ScriptedToolModel {
    type Response = ();

    async fn completion(
        &self,
        request: CompletionRequest,
    ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
        // INVARIANT: tests run on a single tokio task; the lock cannot be poisoned.
        self.seen_tools
            .lock()
            .unwrap()
            .push(request.tools.iter().map(|tool| tool.name.clone()).collect());
        // INVARIANT: tests run on a single tokio task; the lock cannot be poisoned.
        let content = self
            .steps
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| CompletionError::ResponseError("script exhausted".to_string()))?;

        Ok(CompletionResponse {
            content,
            reasoning_content: None,
            raw_response: (),
        })
    }
}

#[derive(Default)]
struct ToolResultObserver {
    results: Vec<Result<String, String>>,
}

impl ToolLoopObserver for ToolResultObserver {
    fn on_tool_call_end(
        &mut self,
        _call_id: &str,
        _tool_name: &str,
        result: &Result<ToolOutput, String>,
    ) {
        self.results.push(match result {
            Ok(output) => Ok(output.as_text().unwrap_or("").to_string()),
            Err(message) => Err(message.clone()),
        });
    }
}

fn text_response(text: &str) -> Vec<AssistantContent> {
    vec![AssistantContent::Text(Text {
        text: text.to_string(),
    })]
}

fn tool_call(id: &str, name: &str, arguments: Value) -> Vec<AssistantContent> {
    vec![AssistantContent::ToolCall(ToolCall {
        id: id.to_string(),
        name: name.to_string(),
        function: Function {
            name: name.to_string(),
            arguments,
        },
    })]
}

fn tool_result_contents(history: &[Message]) -> Vec<String> {
    history
        .iter()
        .filter_map(|message| match message {
            Message::User { content } => Some(content),
            Message::Assistant { .. } | Message::System { .. } => None,
        })
        .flat_map(OneOrMany::iter)
        .filter_map(|content| match content {
            UserContent::ToolResult(result) => {
                result.result["content"].as_str().map(str::to_string)
            }
            UserContent::Text(_) | UserContent::Image(_) => None,
        })
        .collect()
}

fn register_file_tools(registry: &mut ToolRegistry) {
    registry.register("read_file", Arc::new(ReadFileHandler));
    registry.register("list_dir", Arc::new(ListDirHandler));
}

/// Scenario: a scripted model calls `read_file` and then `list_dir`, receives both
/// tool results in model history, and finishes with a text response. This pins the
/// basic single-agent tool-result loop before later semantic-tool rewrites.
#[tokio::test]
async fn basic_file_tools_return_results_to_the_agent_loop() {
    let temp = TempDir::new().unwrap();
    fs::create_dir(temp.path().join("src")).unwrap();
    fs::write(
        temp.path().join("src").join("lib.rs"),
        "pub fn answer() -> u8 {\n    42\n}\n",
    )
    .unwrap();

    let model = ScriptedToolModel::new([
        tool_call(
            "call_read",
            "read_file",
            json!({"file_path": "src/lib.rs", "offset": 1, "limit": 5}),
        ),
        tool_call(
            "call_list",
            "list_dir",
            json!({"dir_path": "src", "offset": 1, "limit": 10, "depth": 1}),
        ),
        text_response("baseline complete"),
    ]);
    let mut registry = ToolRegistry::with_working_dir(temp.path().to_path_buf());
    register_file_tools(&mut registry);

    let turn = run_tool_loop_with_history_and_observer(
        &model,
        Vec::new(),
        "inspect the project",
        &registry,
        ToolLoopConfig::default(),
        &mut ToolResultObserver::default(),
    )
    .await
    .unwrap();

    assert_eq!(turn.final_text, "baseline complete");
    let tool_results = tool_result_contents(&turn.history);
    assert_eq!(tool_results.len(), 2);
    // CONTRACT: ReadFileHandler currently prefixes each line with `L<n>: ` in its
    // tool result. If a later Step 1 task changes that format, update this and the
    // matching assertion in the resume test together.
    assert!(tool_results[0].contains("L1: pub fn answer() -> u8 {"));
    // The list_dir handler always prefixes its successful output with the absolute path.
    // Checking that prefix is what distinguishes a real listing from an error string
    // that merely happens to mention "lib.rs".
    let listing = &tool_results[1];
    assert!(
        listing.starts_with("Absolute path:"),
        "expected list_dir success header, got: {listing}"
    );
    assert!(
        listing.lines().any(|line| line.contains("lib.rs")),
        "expected lib.rs in entries, got: {listing}"
    );
    assert!(
        !listing.to_lowercase().contains("error"),
        "list_dir output should not contain an error marker: {listing}"
    );
}

/// Scenario: the agent calls `apply_patch` through the regular tool loop and the
/// target file changes on disk. This guards the current edit path before Step 1.5
/// introduces file-level undo snapshots around the same handler.
#[tokio::test]
async fn apply_patch_tool_call_modifies_the_workspace() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("notes.txt"), "old\n").unwrap();
    let patch = "\
*** Begin Patch
*** Update File: notes.txt
@@
-old
+new
*** End Patch";

    let model = ScriptedToolModel::new([
        tool_call("call_patch", "apply_patch", json!({"input": patch})),
        text_response("patched"),
    ]);
    let mut registry = ToolRegistry::with_working_dir(temp.path().to_path_buf());
    registry.register("apply_patch", Arc::new(ApplyPatchHandler));
    let mut observer = ToolResultObserver::default();

    let turn = run_tool_loop_with_history_and_observer(
        &model,
        Vec::new(),
        "update notes",
        &registry,
        ToolLoopConfig::default(),
        &mut observer,
    )
    .await
    .unwrap();

    assert_eq!(turn.final_text, "patched");
    assert_eq!(
        fs::read_to_string(temp.path().join("notes.txt")).unwrap(),
        "new\n"
    );
    // The patch tool call must surface exactly one observer result, and it must be
    // a success — not an error string that happens to contain the file name.
    assert_eq!(
        observer.results.len(),
        1,
        "expected exactly one tool result"
    );
    let patch_result = observer
        .results
        .first()
        .expect("just asserted non-empty above");
    let patched_text = match patch_result {
        Ok(text) => text,
        Err(err) => panic!("expected apply_patch to succeed, got error: {err}"),
    };
    // CONTRACT: the apply_patch handler currently echoes touched file paths in its
    // success summary. If Step 1.5 (file-level undo) rewrites this output format,
    // update the assertion together with the handler.
    assert!(
        patched_text.contains("notes.txt"),
        "expected apply_patch summary to mention notes.txt, got: {patched_text}"
    );
}

/// Scenario: `allowed_tools` filters tool definitions before the request is sent
/// and still blocks a hallucinated mutating tool at execution time. The file must
/// remain unchanged even though the model emitted an `apply_patch` call.
#[tokio::test]
async fn allowed_tools_filters_definitions_and_blocks_hallucinated_calls() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("blocked.txt"), "original\n").unwrap();
    let patch = "\
*** Begin Patch
*** Update File: blocked.txt
@@
-original
+modified
*** End Patch";

    let model = ScriptedToolModel::new([
        tool_call("call_blocked", "apply_patch", json!({"input": patch})),
        text_response("saw denial"),
    ]);
    let mut registry = ToolRegistry::with_working_dir(temp.path().to_path_buf());
    registry.register("read_file", Arc::new(ReadFileHandler));
    registry.register("apply_patch", Arc::new(ApplyPatchHandler));
    let mut observer = ToolResultObserver::default();

    let turn = run_tool_loop_with_history_and_observer(
        &model,
        Vec::new(),
        "try to edit",
        &registry,
        ToolLoopConfig {
            allowed_tools: Some(vec!["read_file".to_string()]),
            ..Default::default()
        },
        &mut observer,
    )
    .await
    .unwrap();

    assert_eq!(turn.final_text, "saw denial");
    assert_eq!(
        fs::read_to_string(temp.path().join("blocked.txt")).unwrap(),
        "original\n"
    );
    // Every completion request the model saw must include only the allowed tool;
    // checking just the first snapshot would let a regression slip in if the loop
    // re-included `apply_patch` on subsequent turns after a blocked call.
    let snapshots = model.seen_tools();
    assert!(
        !snapshots.is_empty(),
        "model must have been called at least once"
    );
    for (turn_idx, snapshot) in snapshots.iter().enumerate() {
        assert_eq!(
            snapshot,
            &vec!["read_file".to_string()],
            "turn {turn_idx} exposed unexpected tools to the model: {snapshot:?}"
        );
    }
    // CONTRACT: the runtime emits this exact substring when a tool call is blocked
    // by the allowed_tools list. If Step 1.1 hardening renames the error key, update
    // this assertion alongside the runtime change.
    assert!(observer.results.iter().any(|result| {
        matches!(result, Err(message) if message.contains("not in the allowed_tools list"))
    }));
}

/// Scenario: direct Git execution remains blocked in the shell tool, while the
/// safe Libra CLI status path works in an initialized temp repository. This pins
/// the current "no raw git from agents" baseline before CEX-01/CEX-02 replace the
/// command-level allowlist with a parameter-level safety decision engine.
#[tokio::test]
async fn shell_blocks_git_status_and_libra_status_cli_still_works() {
    let temp = TempDir::new().unwrap();
    let git_invocation = ToolInvocation::new(
        "call_git",
        "shell",
        ToolPayload::Function {
            arguments: json!({"command": "git status"}).to_string(),
        },
        temp.path().to_path_buf(),
    );

    let error = ShellHandler.handle(git_invocation).await.unwrap_err();
    assert!(error.to_string().contains("git is not allowed"));

    let init = run_libra_binary(&["init"], temp.path());
    assert!(
        init.status.success(),
        "libra init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );
    let status = run_libra_binary(&["status"], temp.path());
    assert!(
        status.status.success(),
        "libra status failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
}

/// Scenario: the JSON blob session store can resume a canonical thread in the
/// matching workspace and reconstruct model-facing history from saved user and
/// assistant messages. This is the migration baseline for the later JSONL session
/// work in CEX-12.
#[test]
fn session_store_resumes_thread_history_for_matching_workspace() {
    let temp = TempDir::new().unwrap();
    let store = SessionStore::from_storage_path(temp.path());
    let working_dir = temp.path().join("repo");
    fs::create_dir(&working_dir).unwrap();
    let thread_id = "11111111-1111-4111-8111-111111111111";

    let mut session = SessionState::new(&working_dir.to_string_lossy());
    session.add_user_message("read src/lib.rs");
    session.add_assistant_message("Tool result: L1: pub fn answer() -> u8");
    session
        .metadata
        .insert("thread_id".to_string(), json!(thread_id));
    store.save(&session).unwrap();

    let loaded = store
        .load_for_thread_id(thread_id, &working_dir.to_string_lossy())
        .unwrap()
        .unwrap();
    let history = loaded.to_history();

    assert_eq!(loaded.id, session.id);
    assert_eq!(history.len(), 2);
    assert!(matches!(&history[0], Message::User { .. }));
    assert!(matches!(&history[1], Message::Assistant { .. }));
    assert!(loaded.messages[1].content.contains("Tool result: L1"));
    // Confirm the canonical thread_id round-tripped through serialization. Without
    // this assertion `load_for_thread_id` would still pass even if it silently
    // returned the most-recently-saved session for the workspace.
    assert_eq!(
        loaded.metadata.get("thread_id"),
        Some(&json!(thread_id)),
        "thread_id metadata must round-trip"
    );
}

/// Scenario: a session in the same workspace that does *not* declare the canonical
/// `thread_id` must NOT be returned when resuming by that thread id. This guards
/// against a regression where `load_for_thread_id` silently fell back to "any
/// session whose working_dir matches".
#[test]
fn session_store_does_not_resume_when_thread_id_is_unrelated() {
    let temp = TempDir::new().unwrap();
    let store = SessionStore::from_storage_path(temp.path());
    let working_dir = temp.path().join("repo");
    fs::create_dir(&working_dir).unwrap();
    let other_thread_id = "22222222-2222-4222-8222-222222222222";
    let queried_thread_id = "33333333-3333-4333-8333-333333333333";

    let mut session = SessionState::new(&working_dir.to_string_lossy());
    session.add_user_message("unrelated session");
    session
        .metadata
        .insert("thread_id".to_string(), json!(other_thread_id));
    store.save(&session).unwrap();

    let loaded = store
        .load_for_thread_id(queried_thread_id, &working_dir.to_string_lossy())
        .unwrap();
    assert!(
        loaded.is_none(),
        "load_for_thread_id must not return a session whose canonical thread_id differs"
    );
}

/// Scenario: a `--resume`-style continuation feeds the prior session history as the
/// `existing_history` argument to the tool loop, the loop appends the new user
/// prompt, and the model both *sees* the prior messages and the next assistant turn
/// is appended on top of them. This is the agent-loop side of the resume contract
/// — the store-only test above covers the persistence half. Together they pin the
/// CEX-00 acceptance criterion that "带 `--resume` 的 session 能恢复上一轮对话".
#[tokio::test]
async fn resumed_session_history_flows_into_the_tool_loop() {
    let temp = TempDir::new().unwrap();
    let store = SessionStore::from_storage_path(temp.path());
    let working_dir = temp.path().join("repo");
    fs::create_dir(&working_dir).unwrap();
    fs::write(working_dir.join("answer.txt"), "42\n").unwrap();
    let thread_id = "44444444-4444-4444-8444-444444444444";

    // First turn: a user question and an assistant reply that mentions a fact we
    // can later check the model "saw" on the next turn.
    let mut session = SessionState::new(&working_dir.to_string_lossy());
    session.add_user_message("what is the answer?");
    session.add_assistant_message("the answer file says 42");
    session
        .metadata
        .insert("thread_id".to_string(), json!(thread_id));
    store.save(&session).unwrap();

    // Resume: load the session and convert to model-facing history.
    let resumed = store
        .load_for_thread_id(thread_id, &working_dir.to_string_lossy())
        .unwrap()
        .expect("expected resumable session");
    let prior_history = resumed.to_history();
    assert_eq!(prior_history.len(), 2, "fixture has 2 prior messages");

    // A scripted model that records the request it sees, so we can assert prior
    // messages were forwarded in addition to the new prompt.
    let model = ScriptedToolModel::new([text_response("recalled the answer")]);

    let mut registry = ToolRegistry::with_working_dir(working_dir.clone());
    register_file_tools(&mut registry);

    let turn = run_tool_loop_with_history_and_observer(
        &model,
        prior_history.clone(),
        "say what you remember",
        &registry,
        ToolLoopConfig::default(),
        &mut ToolResultObserver::default(),
    )
    .await
    .unwrap();

    assert_eq!(turn.final_text, "recalled the answer");

    // The loop must keep the prior 2 messages, append the new user prompt, and
    // append the assistant reply — totalling 4 messages.
    assert_eq!(
        turn.history.len(),
        4,
        "expected prior 2 + new prompt + reply = 4, got {:?}",
        turn.history
    );
    let user_texts: Vec<String> = turn
        .history
        .iter()
        .filter_map(|message| match message {
            Message::User { content } => Some(content),
            _ => None,
        })
        .flat_map(OneOrMany::iter)
        .filter_map(|content| match content {
            UserContent::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .collect();
    assert!(
        user_texts.iter().any(|text| text == "what is the answer?"),
        "prior user message must be preserved on resume: {user_texts:?}"
    );
    assert!(
        user_texts
            .iter()
            .any(|text| text == "say what you remember"),
        "new user prompt must be appended: {user_texts:?}"
    );
}

fn run_libra_binary(args: &[&str], cwd: &Path) -> Output {
    let home = cwd.join(".libra-test-home");
    let config_home = home.join(".config");
    fs::create_dir_all(&config_home).unwrap();

    Command::new(env!("CARGO_BIN_EXE_libra"))
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .output()
        .expect("failed to execute libra binary")
}
