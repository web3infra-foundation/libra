//! Phase 0 deterministic mock provider smoke tests.

mod helpers;

use std::time::Duration;

use helpers::{
    mock_codex::{MockCodexServer, MockCodexTurn},
    mock_completion_model::{MockCompletionModel, MockCompletionStep},
};
use libra::internal::ai::completion::{AssistantContent, CompletionModel, CompletionRequest};

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
