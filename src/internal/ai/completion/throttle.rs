//! Lightweight throttling primitives for provider completion calls.
//!
//! Boundary: throttling should bound concurrent provider work without hiding provider
//! errors or retry policy decisions. Runtime and mock-provider tests cover zero-delay
//! and concurrent request paths.

use std::sync::Arc;

use tokio::sync::Semaphore;

use super::{CompletionError, CompletionModel, CompletionRequest, CompletionResponse};

/// CompletionModel wrapper that limits concurrent provider requests.
#[derive(Clone)]
pub struct ThrottledCompletionModel<M> {
    inner: M,
    permits: Arc<Semaphore>,
}

impl<M> ThrottledCompletionModel<M> {
    pub fn new(inner: M, max_concurrency: usize) -> Self {
        Self {
            inner,
            permits: Arc::new(Semaphore::new(max_concurrency.max(1))),
        }
    }

    pub fn with_shared_semaphore(inner: M, permits: Arc<Semaphore>) -> Self {
        Self { inner, permits }
    }
}

impl<M: CompletionModel> CompletionModel for ThrottledCompletionModel<M> {
    type Response = M::Response;

    async fn completion(
        &self,
        request: CompletionRequest,
    ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
        let _permit = self.permits.acquire().await.map_err(|_| {
            CompletionError::ProviderError("completion throttle semaphore is closed".to_string())
        })?;
        self.inner.completion(request).await
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };

    use super::*;
    use crate::internal::ai::completion::{
        AssistantContent,
        message::{Text, UserContent},
    };

    #[derive(Clone)]
    struct SlowModel {
        active: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
    }

    impl CompletionModel for SlowModel {
        type Response = ();

        async fn completion(
            &self,
            _request: CompletionRequest,
        ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
            let current = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(current, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(25)).await;
            self.active.fetch_sub(1, Ordering::SeqCst);
            Ok(CompletionResponse {
                content: vec![AssistantContent::Text(Text {
                    text: "ok".to_string(),
                })],
                reasoning_content: None,
                raw_response: (),
            })
        }
    }

    #[tokio::test]
    async fn throttles_max_concurrency() {
        let model = SlowModel {
            active: Arc::new(AtomicUsize::new(0)),
            peak: Arc::new(AtomicUsize::new(0)),
        };
        let peak = Arc::clone(&model.peak);
        let wrapped = ThrottledCompletionModel::new(model, 2);

        let mut tasks = Vec::new();
        for _ in 0..8 {
            let model = wrapped.clone();
            tasks.push(tokio::spawn(async move {
                let request = CompletionRequest {
                    chat_history: vec![crate::internal::ai::completion::Message::User {
                        content: super::super::OneOrMany::One(UserContent::Text(Text {
                            text: "ping".into(),
                        })),
                    }],
                    ..Default::default()
                };
                model.completion(request).await
            }));
        }

        for task in tasks {
            let result = task.await.expect("join task");
            assert!(result.is_ok());
        }

        assert!(peak.load(Ordering::SeqCst) <= 2);
    }
}
