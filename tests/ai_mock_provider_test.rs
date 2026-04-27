//! Phase 0 deterministic mock provider smoke tests.
//!
//! Exercises the two test fixtures that downstream AI tests rely on:
//! - `MockCompletionModel` — scripts a sequence of success/error/timeout responses
//!   so callers can drive the runtime without a live provider.
//! - `MockCodexServer` — boots a localhost TCP listener that returns a scripted
//!   stream of `MockCodexTurn` JSON lines, used for any test that needs a websocket
//!   endpoint shaped like a Codex provider.
//!
//! **Layer:** L1 — deterministic, in-process, no external dependencies.

mod helpers;

use std::time::Duration;

use helpers::{
    mock_codex::{MockCodexServer, MockCodexTurn},
    mock_completion_model::{MockCompletionModel, MockCompletionStep},
};
use libra::internal::ai::completion::{AssistantContent, CompletionModel, CompletionRequest};

/// Scenario: drive `MockCompletionModel` through three behaviors back-to-back:
/// scripted success, scripted error, and a scripted timeout step. Asserts the model
/// pops steps in order (`remaining_steps()` reaches 0 after the second call) and that
/// the convenience `MockCompletionModel::text` constructor seeds exactly one step.
/// Acts as a fixture-correctness pin so other tests can rely on its semantics.
#[tokio::test]
async fn mock_completion_model_scripts_success_and_error() {
    let model = MockCompletionModel::new([
        MockCompletionStep::Response("plan ready".into()),
        MockCompletionStep::Error("provider disconnected".into()),
    ]);

    let first = model
        .completion(CompletionRequest::default())
        .await
        .expect("mock response");
    assert!(matches!(
        first.content.first(),
        Some(AssistantContent::Text(text)) if text.text == "plan ready"
    ));

    let second = model.completion(CompletionRequest::default()).await;
    assert!(second.is_err());
    assert_eq!(model.remaining_steps(), 0);

    let text_model = MockCompletionModel::text("single response");
    assert_eq!(text_model.remaining_steps(), 1);

    let timeout_model =
        MockCompletionModel::new([MockCompletionStep::Timeout(Duration::from_millis(1))]);
    assert!(
        timeout_model
            .completion(CompletionRequest::default())
            .await
            .is_err()
    );
}

/// Scenario: start `MockCodexServer` with a single scripted turn, then verify the
/// `addr()` and `ws_url()` accessors return the expected loopback address shape. We
/// only check that the address is `127.0.0.1` and the URL is `ws://127.0.0.1:<port>`
/// because the dynamically-allocated port number cannot be hard-coded.
#[tokio::test]
async fn mock_codex_server_exposes_scripted_addr() {
    let server = MockCodexServer::start(vec![MockCodexTurn {
        plan_text: Some("plan".into()),
        patch_diff: None,
        tool_calls: vec![],
    }])
    .await;

    assert_eq!(server.addr().ip().to_string(), "127.0.0.1");
    assert!(server.ws_url().starts_with("ws://127.0.0.1:"));
}
