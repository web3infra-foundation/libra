//! Headless web-only runtime smoke tests.
//!
//! Exercises [`HeadlessCodeRuntime`] end-to-end against the deterministic
//! `test-provider` fixture: submitting a prompt should drive a tool-loop turn
//! whose final assistant text lands in the live `CodeUiSession`. Used as the
//! L1 verification anchor for Phase 3 of `docs/improvement/web.md` (the
//! `--web-only --provider <non-codex>` path that previously fell back to a
//! read-only placeholder).

#![cfg(feature = "test-provider")]

use std::{path::PathBuf, sync::Arc, time::Duration};

use libra::internal::ai::{
    agent::runtime::tool_loop::ToolLoopConfig,
    providers::fake,
    runtime::{ToolBoundaryRuntime, TracingAuditSink},
    tools::context::{UserInputQuestion, UserInputRequest, UserInputResponse},
    tools::{ToolRegistryBuilder, handlers::ReadFileHandler},
    web::{
        code_ui::{
            CodeUiCommandAdapter, CodeUiInteractionResponse, CodeUiInteractionStatus,
            CodeUiProviderInfo, CodeUiReadModel, CodeUiSession, CodeUiSessionStatus,
            initial_snapshot,
        },
        headless::{HeadlessCodeRuntime, headless_capabilities},
    },
};
use tokio::sync::mpsc;
use uuid::Uuid;

fn fixture_path(name: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests/fixtures/code_ui");
    path.push(format!("{name}.json"));
    path
}

fn build_runtime(
    fixture: &str,
    working_dir: PathBuf,
) -> (
    Arc<HeadlessCodeRuntime<fake::CompletionModel>>,
    mpsc::UnboundedSender<UserInputRequest>,
) {
    let fake_client = fake::Client::from_fixture_path(&fixture_path(fixture))
        .expect("fake provider fixture must load");
    let model = fake_client.completion_model("fake");
    let capabilities = headless_capabilities();
    let provider = CodeUiProviderInfo {
        provider: "fake".to_string(),
        model: Some("fake".to_string()),
        mode: Some("web-headless".to_string()),
        managed: false,
    };
    let session = CodeUiSession::new(initial_snapshot(
        working_dir.to_string_lossy().to_string(),
        provider,
        capabilities.clone(),
    ));
    let (user_input_tx, user_input_rx) = mpsc::unbounded_channel::<UserInputRequest>();

    let registry = Arc::new(
        ToolRegistryBuilder::with_working_dir(working_dir)
            .hardening(ToolBoundaryRuntime::system(
                Uuid::new_v4(),
                Arc::new(TracingAuditSink),
            ))
            .register("read_file", Arc::new(ReadFileHandler))
            .build(),
    );

    let config_factory: Arc<dyn Fn() -> ToolLoopConfig + Send + Sync> =
        Arc::new(ToolLoopConfig::default);

    (
        HeadlessCodeRuntime::new(
            session,
            capabilities,
            model,
            registry,
            user_input_rx,
            config_factory,
        ),
        user_input_tx,
    )
}

/// Submitting a plain message must produce an assistant transcript entry that
/// matches the fake provider's deterministic response, with the snapshot
/// returning to `Idle` once the turn settles. This is the single anchor that
/// proves the headless runtime actually drives a model turn — every other
/// scenario (cancel, reject-on-empty, capability flags) builds on it.
#[tokio::test(flavor = "multi_thread")]
async fn submit_message_streams_assistant_reply_into_snapshot() {
    let workdir = tempfile::tempdir().expect("tempdir for headless workdir");
    let (runtime, _) = build_runtime("basic_chat", workdir.path().to_path_buf());

    runtime
        .submit_message("hello headless".to_string())
        .await
        .expect("headless submit_message accepts non-empty text");

    // Wait for the spawned turn to finalize the assistant entry.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut final_snapshot = runtime.snapshot().await;
    while std::time::Instant::now() < deadline {
        if final_snapshot.status == CodeUiSessionStatus::Idle
            && final_snapshot.transcript.iter().any(|entry| {
                entry.kind
                    == libra::internal::ai::web::code_ui::CodeUiTranscriptEntryKind::AssistantMessage
                    && entry
                        .content
                        .as_deref()
                        .is_some_and(|c| c.contains("fake assistant"))
            })
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
        final_snapshot = runtime.snapshot().await;
    }

    assert_eq!(
        final_snapshot.status,
        CodeUiSessionStatus::Idle,
        "snapshot must return to idle once the turn finishes",
    );

    let assistant = final_snapshot
        .transcript
        .iter()
        .find(|entry| {
            entry.kind
                == libra::internal::ai::web::code_ui::CodeUiTranscriptEntryKind::AssistantMessage
        })
        .expect("an assistant entry must be appended");
    assert!(!assistant.streaming);
    assert_eq!(assistant.status.as_deref(), Some("completed"));
    assert!(
        assistant
            .content
            .as_deref()
            .is_some_and(|c| c.contains("fake assistant")),
        "assistant entry must carry the fake fixture text, got {:?}",
        assistant.content,
    );
}

/// `submit_message("")` must fail loud rather than silently appending an
/// empty transcript entry — the browser will treat this as a UI bug rather
/// than a queued turn.
#[tokio::test(flavor = "multi_thread")]
async fn empty_message_is_rejected_before_any_transcript_mutation() {
    let workdir = tempfile::tempdir().expect("tempdir for headless workdir");
    let (runtime, _) = build_runtime("basic_chat", workdir.path().to_path_buf());

    let result = runtime.submit_message("   ".to_string()).await;
    assert!(result.is_err(), "whitespace-only messages must be rejected");

    let snapshot = runtime.snapshot().await;
    assert!(
        snapshot.transcript.is_empty(),
        "rejected submits must not leave transcript residue",
    );
    assert_eq!(snapshot.status, CodeUiSessionStatus::Idle);
}

/// The headless runtime advertises only the surfaces it can actually deliver
/// in v0; locking these down catches accidental capability drift (e.g.
/// turning on `interactiveApprovals` before the InteractionPanel routing is
/// wired into the headless path).
#[test]
fn headless_capabilities_match_phase3_v0_contract() {
    let caps = headless_capabilities();
    assert!(caps.message_input);
    assert!(caps.streaming_text);
    assert!(caps.tool_calls);
    assert!(!caps.plan_updates);
    assert!(!caps.patchsets);
    assert!(caps.interactive_approvals);
    assert!(caps.structured_questions);
    assert!(!caps.provider_session_resume);
}

/// `cancel_turn` must finalize the streaming assistant entry — leaving it
/// flagged `streaming: true` would render as a perpetual typing indicator
/// in the browser. The fixture's delay() lets us cancel mid-flight with
/// a deterministic race window.
#[tokio::test(flavor = "multi_thread")]
async fn cancel_turn_finalizes_streaming_assistant_entry() {
    let workdir = tempfile::tempdir().expect("tempdir for headless workdir");
    let (runtime, _) = build_runtime("delayed_chat", workdir.path().to_path_buf());

    runtime
        .submit_message("slow".to_string())
        .await
        .expect("submit must accept the prompt before delay fires");

    // Wait until the in-flight assistant entry shows up as streaming, then
    // cancel before the fake provider's delay completes.
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut saw_streaming = false;
    while std::time::Instant::now() < deadline {
        let snapshot = runtime.snapshot().await;
        if snapshot.transcript.iter().any(|entry| {
            entry.kind
                == libra::internal::ai::web::code_ui::CodeUiTranscriptEntryKind::AssistantMessage
                && entry.streaming
        }) {
            saw_streaming = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        saw_streaming,
        "assistant entry must be visible as streaming before cancel fires",
    );

    runtime.cancel_turn().await.expect("cancel must succeed");

    let snapshot = runtime.snapshot().await;
    assert_eq!(snapshot.status, CodeUiSessionStatus::Idle);
    let assistant = snapshot
        .transcript
        .iter()
        .find(|entry| {
            entry.kind
                == libra::internal::ai::web::code_ui::CodeUiTranscriptEntryKind::AssistantMessage
        })
        .expect("assistant entry must remain in the transcript after cancel");
    assert!(!assistant.streaming, "cancel must clear the streaming flag",);
    assert_eq!(assistant.status.as_deref(), Some("cancelled"));
}

/// Late-arriving stream deltas (e.g. from a still-pending tokio task spawned
/// by `HeadlessTurnObserver::on_model_stream_event`) must not resurrect the
/// `streaming: true` flag once the assistant entry has been finalized as
/// `cancelled`. Without this, the browser would briefly clear its typing
/// indicator and then see it return for any text delta that races past
/// `cancel_turn`.
#[tokio::test(flavor = "multi_thread")]
async fn late_stream_delta_does_not_resurrect_cancelled_entry() {
    use libra::internal::ai::web::code_ui::{
        CodeUiCapabilities, CodeUiProviderInfo, CodeUiSession, CodeUiTranscriptEntry,
        CodeUiTranscriptEntryKind, initial_snapshot,
    };

    let session = CodeUiSession::new(initial_snapshot(
        "/tmp/late-delta",
        CodeUiProviderInfo {
            provider: "fake".to_string(),
            model: None,
            mode: None,
            managed: false,
        },
        CodeUiCapabilities::default(),
    ));
    let now = chrono::Utc::now();
    let entry_id = "assistant-1".to_string();
    session
        .upsert_transcript_entry(CodeUiTranscriptEntry {
            id: entry_id.clone(),
            kind: CodeUiTranscriptEntryKind::AssistantMessage,
            title: None,
            content: Some(String::from("partial")),
            status: Some("cancelled".to_string()),
            streaming: false,
            metadata: serde_json::json!({}),
            created_at: now,
            updated_at: now,
        })
        .await;

    // Late delta from an already-finalized turn arrives — it must be ignored.
    session
        .append_assistant_delta(&entry_id, " more text")
        .await;

    let snapshot = session.snapshot().await;
    let entry = snapshot
        .transcript
        .iter()
        .find(|e| e.id == entry_id)
        .expect("entry must still exist");
    assert!(
        !entry.streaming,
        "late delta must not flip a finalized entry back to streaming",
    );
    assert_eq!(entry.status.as_deref(), Some("cancelled"));
    assert_eq!(
        entry.content.as_deref(),
        Some("partial"),
        "late delta must not append to finalized content",
    );
}

/// `append_assistant_delta` must keep accepting deltas while the entry is
/// in any non-terminal state (e.g. the TUI flow flags entries as
/// `thinking` rather than `streaming`). Only the terminal statuses
/// (`completed` / `error` / `cancelled`) short-circuit the append. This
/// regression test guards against tightening the guard back to a strict
/// `status == "streaming"` check that breaks the TUI's live streaming.
#[tokio::test(flavor = "multi_thread")]
async fn append_assistant_delta_still_accepts_thinking_status() {
    use libra::internal::ai::web::code_ui::{
        CodeUiCapabilities, CodeUiProviderInfo, CodeUiSession, CodeUiTranscriptEntry,
        CodeUiTranscriptEntryKind, initial_snapshot,
    };

    let session = CodeUiSession::new(initial_snapshot(
        "/tmp/thinking-delta",
        CodeUiProviderInfo {
            provider: "fake".to_string(),
            model: None,
            mode: None,
            managed: false,
        },
        CodeUiCapabilities::default(),
    ));
    let now = chrono::Utc::now();
    let entry_id = "assistant-tui".to_string();
    session
        .upsert_transcript_entry(CodeUiTranscriptEntry {
            id: entry_id.clone(),
            kind: CodeUiTranscriptEntryKind::AssistantMessage,
            title: None,
            content: Some(String::new()),
            // The TUI's live assistant row carries `status: "thinking"`
            // alongside `streaming: true` until the model finishes —
            // mirror that here.
            status: Some("thinking".to_string()),
            streaming: true,
            metadata: serde_json::json!({}),
            created_at: now,
            updated_at: now,
        })
        .await;

    session.append_assistant_delta(&entry_id, "hello ").await;
    session.append_assistant_delta(&entry_id, "world").await;

    let snapshot = session.snapshot().await;
    let entry = snapshot
        .transcript
        .iter()
        .find(|e| e.id == entry_id)
        .expect("entry must exist");
    assert!(entry.streaming);
    assert_eq!(entry.content.as_deref(), Some("hello world"));
}

/// `respond_interaction` should reject unknown interactions and only
/// accept requests that are currently pending.
#[tokio::test(flavor = "multi_thread")]
async fn respond_interaction_unknown_id() {
    let workdir = tempfile::tempdir().expect("tempdir for headless workdir");
    let (runtime, _) = build_runtime("basic_chat", workdir.path().to_path_buf());

    let result = runtime
        .respond_interaction("ignored", CodeUiInteractionResponse::default())
        .await;
    let error = result.expect_err("interactions must surface a concrete error for unknown id");
    assert!(
        error
            .to_string()
            .contains("Unknown pending interaction"),
        "error message must call out unknown interaction ids, got {error}",
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn request_user_input_request_is_reflected_in_snapshot_and_responded_to() {
    let workdir = tempfile::tempdir().expect("tempdir for headless workdir");
    let (runtime, user_input_tx) = build_runtime("basic_chat", workdir.path().to_path_buf());

    let interaction_id = "request-user-input-1".to_string();
    let question_id = "q1".to_string();
    let (response_tx, response_rx) = tokio::sync::oneshot::channel::<UserInputResponse>();
    user_input_tx
        .send(UserInputRequest {
            call_id: interaction_id.clone(),
            questions: vec![UserInputQuestion {
                id: question_id.clone(),
                header: "Approve".to_string(),
                question: "Choose approach".to_string(),
                is_other: false,
                is_secret: false,
                options: None,
            }],
            response_tx,
        })
        .expect("request_user_input request should enqueue in runtime");

    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut saw_pending = false;
    while std::time::Instant::now() < deadline {
        let snapshot = runtime.snapshot().await;
        if snapshot
            .interactions
            .iter()
            .any(|interaction| {
                interaction.id == interaction_id
                    && interaction.status == CodeUiInteractionStatus::Pending
            })
        {
            saw_pending = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        saw_pending,
        "request_user_input request should appear as pending interaction",
    );

    runtime
        .respond_interaction(
            &interaction_id,
            CodeUiInteractionResponse {
                selected_option: Some("selected option".to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("respond_interaction should forward to pending request sender");

    let response = response_rx
        .await
        .expect("request_user_input request should deliver response");
    assert_eq!(
        response
            .answers
            .get(&question_id)
            .expect("response should include requested question")
            .answers,
        vec!["selected option".to_string()]
    );

    let final_snapshot = runtime.snapshot().await;
    assert_eq!(
        final_snapshot.status,
        CodeUiSessionStatus::ExecutingTool,
        "respond_interaction should set runtime status to executing tool",
    );
    assert!(
        final_snapshot
            .interactions
            .iter()
            .all(|interaction| interaction.status != CodeUiInteractionStatus::Pending),
        "all pending interactions should be resolved",
    );
}
