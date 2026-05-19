//! Step 1.0 / CEX-00 single-agent baseline tests.
//!
//! Acts as the regression net for `libra code` single-agent behavior **before** any
//! Step 1.x change lands (Step 1.1 safety hardening, Step 1.2 JSON repair, Step 1.3
//! semantic tools, Step 1.5 file-level undo, Step 1.8 JSONL session storage). Each
//! test pins one specific contract — tool result flow, file mutation through
//! `apply_patch`, `allowed_tools` definition + execution gating, raw-`git` rejection
//! in the shell tool, and JSON-blob session resume by canonical `thread_id` — so a
//! later refactor that breaks one of those contracts fails the matching scenario
//! by name rather than producing a silent behavior shift.
//!
//! All scenarios run against scripted in-process models and `tempfile::tempdir()`
//! workspaces; no live provider credentials are required and no global state is
//! mutated, so `#[serial]` is unnecessary. CP-S2-3 (Step 2's flag-off equivalence
//! gate) compares this file's normalized output against the CEX-00 commit
//! `48ea0ae`, so any future edit here that changes the test text must update
//! `CEX00_BASELINE_COMMIT` in the CP-S2-3 script and the agent.md Changelog.
//!
//! **Layer:** L1 — uses scripted `CompletionModel` impls and direct handler
//! invocations. The only L2-ish exception is
//! `shell_blocks_git_status_and_libra_status_cli_still_works`, which spawns the
//! `libra` binary in an isolated `HOME` to confirm `libra status` keeps working
//! after `ShellHandler` rejects raw `git`.

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

/// Deterministic in-process [`CompletionModel`] that replays a fixed script of
/// assistant turns, one per `completion()` call.
///
/// Each item in `steps` is the full `Vec<AssistantContent>` for one model turn,
/// emitted in FIFO order; running out of script items returns
/// `CompletionError::ResponseError("script exhausted")`. `seen_tools` records the
/// tool-name slice the runtime sent in each request so callers can assert that
/// `allowed_tools` filtering happened at the request layer (not just at dispatch).
///
/// `Arc<Mutex<...>>` is used because `CompletionModel::completion` takes `&self`
/// while the scripted state must mutate; `clone()` on the model only clones the
/// `Arc` handles, so multiple references see the same script and the same
/// recorded snapshots.
#[derive(Clone, Debug)]
struct ScriptedToolModel {
    /// FIFO queue of assistant turns to emit on successive `completion()` calls.
    steps: Arc<Mutex<VecDeque<Vec<AssistantContent>>>>,
    /// One entry per `completion()` call — the names of every tool the runtime
    /// included in `request.tools`. Used by the `allowed_tools` test to assert
    /// the filter applies to every request, not just the first.
    seen_tools: Arc<Mutex<Vec<Vec<String>>>>,
}

impl ScriptedToolModel {
    /// Build a scripted model from an ordered iterable of assistant turns.
    fn new(steps: impl IntoIterator<Item = Vec<AssistantContent>>) -> Self {
        Self {
            steps: Arc::new(Mutex::new(steps.into_iter().collect())),
            seen_tools: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Snapshot of every tool list the runtime sent to this model so far,
    /// in call order. Returned by clone so the caller can iterate without
    /// holding the inner `Mutex`.
    fn seen_tools(&self) -> Vec<Vec<String>> {
        // INVARIANT: tests run on a single tokio task; the lock cannot be poisoned.
        self.seen_tools.lock().unwrap().clone()
    }
}

impl CompletionModel for ScriptedToolModel {
    type Response = ();

    /// Records the tool definitions presented to the model on this call, then
    /// pops and returns the next scripted assistant turn. `Response` is `()`
    /// because the scripted model has no provider-specific raw response body.
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

/// `ToolLoopObserver` impl that records every tool-call result the runtime
/// emits, normalized to `Result<String, String>`.
///
/// `Ok` carries the success text (`ToolOutput::as_text`); `Err` carries the
/// runtime's structured error message. Used by the `apply_patch` test to assert
/// the success path produced exactly one observer entry, and by the
/// `allowed_tools` test to assert the runtime emitted the documented
/// "not in the allowed_tools list" rejection on the hallucinated mutating call.

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

/// Build a one-turn assistant script that emits a single text chunk and ends
/// the loop. Used as the "stop" sentinel after a tool-call sequence.
fn text_response(text: &str) -> Vec<AssistantContent> {
    vec![AssistantContent::Text(Text {
        text: text.to_string(),
    })]
}

/// Build a one-turn assistant script that emits a single tool call.
///
/// `id` is the call ID the runtime echoes back into the matching tool result;
/// `name` is duplicated into both the outer `ToolCall.name` and the nested
/// `Function.name` because some providers expose tool calls in either layer
/// and the runtime is tolerant of both.
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

/// Extract every tool-result content string the runtime appended to model
/// history, in chronological order.
///
/// Tool results travel back to the model as `User` messages whose `content`
/// is `OneOrMany<UserContent::ToolResult>`. This helper flattens that nested
/// shape to a flat `Vec<String>` so assertions can index into a single result
/// (e.g. "the first tool result was the `read_file` body") without traversing
/// the message tree at every call site.
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

/// Register the two read-only file tools (`read_file`, `list_dir`) every
/// baseline scenario expects, so individual tests don't repeat the
/// boilerplate. `apply_patch` and `shell` are registered per-test because
/// only some scenarios need them.
fn register_file_tools(registry: &mut ToolRegistry) {
    registry.register("read_file", Arc::new(ReadFileHandler));
    registry.register("list_dir", Arc::new(ListDirHandler));
}

/// Scenario: a scripted model calls `read_file` and then `list_dir`, the tool
/// loop dispatches both handlers, the runtime threads the results back into
/// model history as `UserContent::ToolResult`, and the model emits a final
/// `"baseline complete"` text turn that ends the loop.
///
/// Asserts the round trip end-to-end: (a) `final_text` matches the scripted
/// terminal text, (b) two tool results landed on history, (c) the `read_file`
/// result still uses the documented `L<n>:` line-number prefix, and (d) the
/// `list_dir` result starts with the documented `Absolute path:` header and
/// names the file we just wrote with no error markers.
///
/// Acts as the contract pin for the basic tool-result loop before any Step 1.x
/// semantic-tool work rewrites how `read_file` / `list_dir` shape their output.
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

/// Scenario: a scripted model issues a single `apply_patch` tool call with a
/// minimal `*** Update File:` body, and the runtime drives it through the
/// regular tool loop. Asserts that (a) the file actually mutates from `old\n`
/// to `new\n` on disk, (b) the loop's terminal text matches the scripted
/// `"patched"`, and (c) the observer sees exactly one `Ok(_)` result whose
/// success summary mentions the touched file.
///
/// Acts as the pin for the un-undoable file-mutation path before CEX-10
/// (Step 1.5 file-level undo) wraps `ApplyPatchHandler` with snapshot logic;
/// after Step 1.5 the same flow must still produce the same on-disk result
/// while emitting an additional pre-edit snapshot, so this test must stay
/// green by construction.
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

/// Scenario: with `ToolLoopConfig.allowed_tools = Some(["read_file"])` and
/// `apply_patch` registered in the registry, a scripted model still tries to
/// emit an `apply_patch` call. Asserts the runtime applies `allowed_tools` at
/// **two** independent layers:
///
/// 1. **Definition filter** — every snapshot in `seen_tools()` (one per
///    completion request, across the whole loop) shows only `read_file`,
///    proving `apply_patch` never reached the model in the request payload.
/// 2. **Execution filter** — even though the script forces the model to
///    request `apply_patch`, the runtime intercepts it before dispatch and
///    surfaces the documented `"not in the allowed_tools list"` error to the
///    observer; the file on disk stays at `"original\n"` byte-for-byte.
///
/// Iterating every turn snapshot (not just `seen_tools()[0]`) guards against
/// a regression where the loop re-includes the disallowed tool on the second
/// turn after the first call gets blocked. This is also the contract pin for
/// the exact rejection-error substring; Step 1.1 hardening that renames the
/// error key must update both this assertion and the runtime together.
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

/// Scenario: invoke `ShellHandler` directly with `command: "git status"` and
/// assert it returns the documented `"git is not allowed"` error — agents are
/// never allowed to execute raw `git` and must use `libra` / `run_libra_vcs`
/// instead. As the positive counterpart, spawn the actual `libra` binary in
/// the same temp dir and confirm `libra init` then `libra status` both
/// succeed, proving the safe alternative remained reachable.
///
/// Acts as the regression pin for the current command-level git rejection
/// before CEX-01 / CEX-02 (Step 1.1 safety hardening) replace the simple
/// "deny if argv[0] == git" check with a parameter-level safety decision
/// engine. After Step 1.1 lands the rejection message wording may change;
/// this assertion needs to be updated in lockstep.
///
/// The `libra` subprocess uses `run_libra_binary`, which sets `HOME` /
/// `XDG_CONFIG_HOME` to a tempdir and clears the rest of the environment so
/// the host's user config can never leak into the test outcome.
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

/// Scenario: write a `SessionState` with a canonical `thread_id` in metadata
/// plus a user+assistant message pair to a `SessionStore` rooted at a tempdir,
/// then load the same session by `(thread_id, working_dir)` and assert:
///
/// 1. `loaded.id == session.id` — the same on-disk session was returned.
/// 2. `loaded.to_history()` produces the documented two-message
///    User+Assistant alternation suitable for resume-time prompt rebuild.
/// 3. The assistant message's tool-result substring `"Tool result: L1"`
///    survives serialization (matches the `read_file` `L<n>:` convention
///    pinned in `basic_file_tools_return_results_to_the_agent_loop`).
/// 4. The `thread_id` metadata entry round-trips through serialize/deserialize
///    — without this assertion `load_for_thread_id` could regress to
///    "any most-recently-saved session for the workspace" and still pass.
///
/// Acts as the JSON-blob migration baseline for CEX-12 (Step 1.8 JSONL session
/// storage). After CEX-12 the same store is expected to produce the same
/// resumed history through a different on-disk format; this test must stay
/// green by construction or `CEX00_BASELINE_COMMIT` in CP-S2-3 must be bumped.
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

/// Scenario: persist a session whose `thread_id` metadata is a *different*
/// UUID, then call `load_for_thread_id` with the queried thread id; assert the
/// store returns `None` rather than the stored session.
///
/// This is the negative-match counterpart to
/// `session_store_resumes_thread_history_for_matching_workspace`. Without it,
/// a regression where `load_for_thread_id` silently fell back to "any session
/// whose working_dir matches" would still pass the positive test (because the
/// session it'd happen to return is the one we just saved). Both sides
/// together define the resume contract: thread_id is required for a hit, and
/// workspace-only matching is forbidden.
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

/// Scenario: persist a session with a `User("what is the answer?")` +
/// `Assistant("the answer file says 42")` pair, reload it via
/// `load_for_thread_id`, convert to model-facing history with `to_history()`,
/// and feed that history as `existing_history` into
/// `run_tool_loop_with_history_and_observer` along with a new prompt
/// `"say what you remember"`. Assert:
///
/// 1. The final history length is 4 (2 prior + 1 new prompt + 1 reply) — the
///    loop appended on top of resumed messages, not replaced them.
/// 2. The prior user message text survives end-to-end into the loop history.
/// 3. The new prompt is appended in the user role, in chronological order.
///
/// This pairs with the two store-only tests above to fully cover the CEX-00
/// acceptance criterion `带 `--resume` 的 session 能恢复上一轮对话和已有 tool
/// result` — without this scenario a regression that breaks the loop-side
/// reattach would still pass the persistence-only tests. Codex round-2 review
/// flagged this gap explicitly; the test was added to close it.
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

/// Spawn the `libra` binary built by `cargo test` (via `CARGO_BIN_EXE_libra`)
/// in a fully isolated environment so the test never reads or mutates the
/// host user's config.
///
/// The function clears `env_clear()`, sets a minimal `PATH`, and points
/// `HOME` / `USERPROFILE` / `XDG_CONFIG_HOME` at a tempdir under `cwd` so any
/// `libra` config / session / `.libra` directories the binary writes land in
/// the temp scope and disappear with the `TempDir`. `LANG` / `LC_ALL` are
/// pinned to `C` so localized error messages can't shift the assertion text.
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
