//! CEX-08 TaskIntent classifier contract tests.
//!
//! These tests pin the provider-neutral classifier boundary: prompt construction,
//! model-backed JSON parsing with deterministic repair, fixture coverage, and the
//! explicit-context path that skips the classifier model call.

mod helpers;

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use helpers::mock_completion_model::MockCompletionModel;
use libra::internal::ai::{
    agent::{
        ExplicitCodeContext, TaskIntent, TaskIntentClassificationRequest, TaskIntentClassifier,
        TaskIntentDecisionSource,
    },
    completion::{
        AssistantContent, CompletionError, CompletionModel, CompletionRequest, CompletionResponse,
        Text,
    },
};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct ClassifierFixture {
    name: String,
    user_input: String,
    model_response: String,
    expected_intent: TaskIntent,
}

fn classifier_fixtures() -> Vec<ClassifierFixture> {
    include_str!("data/classifier_fixtures.jsonl")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("classifier fixture line must be valid JSON"))
        .collect()
}

#[tokio::test]
async fn ai_classifier_parses_fixture_responses_from_mock_model() {
    for fixture in classifier_fixtures() {
        let classifier =
            TaskIntentClassifier::new(MockCompletionModel::text(fixture.model_response));

        let decision = classifier
            .classify(TaskIntentClassificationRequest::new(fixture.user_input))
            .await
            .unwrap_or_else(|error| panic!("fixture failed: {}: {error}", fixture.name));

        assert_eq!(
            decision.intent, fixture.expected_intent,
            "fixture failed: {}",
            fixture.name
        );
        assert_eq!(decision.source, TaskIntentDecisionSource::Model);
        assert!(
            (0.0..=1.0).contains(&decision.confidence),
            "confidence must be normalized for {}",
            fixture.name
        );
        assert!(
            !decision.rationale.trim().is_empty(),
            "rationale should explain {}",
            fixture.name
        );
    }
}

#[test]
fn ai_classifier_prompt_is_json_only_and_lists_supported_intents() {
    let prompt = TaskIntentClassifier::<MockCompletionModel>::classifier_preamble();

    for expected in [
        "bug_fix",
        "feature",
        "question",
        "review",
        "refactor",
        "test",
        "documentation",
        "command",
        "chore",
        "unknown",
    ] {
        assert!(
            prompt.contains(expected),
            "classifier prompt should mention {expected}"
        );
    }

    assert!(prompt.contains("Return JSON only"));
    assert!(prompt.contains("confidence"));
    assert!(prompt.contains("rationale"));
}

#[tokio::test]
async fn ai_classifier_sends_preamble_and_untrusted_user_input_to_model() {
    let model = CapturingModel::new(
        "{\"intent\":\"question\",\"confidence\":0.91,\"rationale\":\"The user asks for a location.\"}",
    );
    let classifier = TaskIntentClassifier::new(model.clone());

    let decision = classifier
        .classify(TaskIntentClassificationRequest::new(
            "Where is the approval store implemented?",
        ))
        .await
        .expect("classification should succeed");

    assert_eq!(decision.intent, TaskIntent::Question);
    assert_eq!(model.calls(), 1);
    let request = model.last_request().expect("request should be captured");
    let preamble = request.preamble.expect("classifier should set preamble");
    assert!(preamble.contains("Classify the user's libra code request"));
    assert!(
        request.chat_history.iter().any(|message| format!("{message:?}")
            .contains("<user_request_untrusted>Where is the approval store implemented?</user_request_untrusted>")),
        "user input should be clearly wrapped as untrusted data"
    );
    assert!(
        request.tools.is_empty(),
        "classifier must not expose tools to the model"
    );
}

#[tokio::test]
async fn ai_classifier_explicit_context_skips_model_call() {
    let model = CapturingModel::new(
        "{\"intent\":\"feature\",\"confidence\":0.4,\"rationale\":\"should not be used\"}",
    );
    let classifier = TaskIntentClassifier::new(model.clone());

    let decision = classifier
        .classify(
            TaskIntentClassificationRequest::new("Review this diff")
                .with_explicit_context(ExplicitCodeContext::Review),
        )
        .await
        .expect("explicit context should produce a decision");

    assert_eq!(decision.intent, TaskIntent::Review);
    assert_eq!(decision.source, TaskIntentDecisionSource::ExplicitContext);
    assert_eq!(decision.confidence, 1.0);
    assert_eq!(model.calls(), 0, "explicit --context must skip classifier");
}

#[derive(Clone)]
struct CapturingModel {
    response: String,
    calls: Arc<AtomicUsize>,
    last_request: Arc<std::sync::Mutex<Option<CompletionRequest>>>,
}

impl CapturingModel {
    fn new(response: impl Into<String>) -> Self {
        Self {
            response: response.into(),
            calls: Arc::new(AtomicUsize::new(0)),
            last_request: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn last_request(&self) -> Option<CompletionRequest> {
        self.last_request
            .lock()
            .expect("capture mutex should not be poisoned")
            .clone()
    }
}

impl CompletionModel for CapturingModel {
    type Response = serde_json::Value;

    async fn completion(
        &self,
        request: CompletionRequest,
    ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self
            .last_request
            .lock()
            .expect("capture mutex should not be poisoned") = Some(request);
        Ok(CompletionResponse {
            content: vec![AssistantContent::Text(Text {
                text: self.response.clone(),
            })],
            reasoning_content: None,
            raw_response: serde_json::json!({"provider": "capturing"}),
        })
    }
}
