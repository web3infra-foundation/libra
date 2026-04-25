use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};

use libra::internal::ai::completion::{
    AssistantContent, CompletionError, CompletionModel, CompletionRequest, CompletionResponse, Text,
};

#[derive(Clone, Debug)]
pub enum MockCompletionStep {
    Response(String),
    Error(String),
    Timeout(Duration),
}

#[derive(Clone, Debug)]
pub struct MockCompletionModel {
    steps: Arc<Mutex<VecDeque<MockCompletionStep>>>,
}

impl MockCompletionModel {
    pub fn new(steps: impl IntoIterator<Item = MockCompletionStep>) -> Self {
        Self {
            steps: Arc::new(Mutex::new(steps.into_iter().collect())),
        }
    }

    pub fn text(text: impl Into<String>) -> Self {
        Self::new([MockCompletionStep::Response(text.into())])
    }

    pub fn remaining_steps(&self) -> usize {
        self.steps.lock().unwrap().len()
    }
}

impl CompletionModel for MockCompletionModel {
    type Response = serde_json::Value;

    async fn completion(
        &self,
        _request: CompletionRequest,
    ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
        let step =
            self.steps.lock().unwrap().pop_front().ok_or_else(|| {
                CompletionError::ResponseError("No mock responses remaining".into())
            })?;

        match step {
            MockCompletionStep::Response(text) => Ok(CompletionResponse {
                content: vec![AssistantContent::Text(Text { text })],
                reasoning_content: None,
                raw_response: serde_json::json!({ "provider": "mock" }),
            }),
            MockCompletionStep::Error(message) => Err(CompletionError::ProviderError(message)),
            MockCompletionStep::Timeout(duration) => {
                tokio::time::sleep(duration).await;
                Err(CompletionError::ProviderError(format!(
                    "mock timeout after {}ms",
                    duration.as_millis()
                )))
            }
        }
    }
}
