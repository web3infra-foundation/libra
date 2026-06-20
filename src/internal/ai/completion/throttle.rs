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

    /// `ThrottledCompletionModel::new(_, 0)` must clamp the
    /// concurrency limit to 1 (the `.max(1)` floor). Pin so a future
    /// refactor doesn't accidentally allow `Semaphore::new(0)` which
    /// would block every request indefinitely.
    #[tokio::test]
    async fn new_with_zero_max_concurrency_clamps_to_one() {
        let model = SlowModel {
            active: Arc::new(AtomicUsize::new(0)),
            peak: Arc::new(AtomicUsize::new(0)),
        };
        let peak = Arc::clone(&model.peak);
        let wrapped = ThrottledCompletionModel::new(model, 0);

        let mut tasks = Vec::new();
        for _ in 0..4 {
            let model = wrapped.clone();
            tasks.push(tokio::spawn(async move {
                let request = CompletionRequest::default();
                model.completion(request).await
            }));
        }
        for task in tasks {
            task.await.expect("join task").expect("completion ok");
        }
        // With zero-clamped-to-one max_concurrency, peak active count
        // must never exceed 1.
        assert_eq!(
            peak.load(Ordering::SeqCst),
            1,
            "zero max_concurrency must clamp to 1",
        );
    }

    /// `with_shared_semaphore` lets multiple wrappers share a single
    /// permit pool. Pin: spawning across two wrappers sharing a
    /// 1-permit semaphore must still serialize at peak=1.
    #[tokio::test]
    async fn with_shared_semaphore_enforces_pool_wide_limit() {
        let model_a = SlowModel {
            active: Arc::new(AtomicUsize::new(0)),
            peak: Arc::new(AtomicUsize::new(0)),
        };
        let model_b = SlowModel {
            active: Arc::clone(&model_a.active),
            peak: Arc::clone(&model_a.peak),
        };
        let peak = Arc::clone(&model_a.peak);
        let shared = Arc::new(Semaphore::new(1));

        let wrapped_a =
            ThrottledCompletionModel::with_shared_semaphore(model_a, Arc::clone(&shared));
        let wrapped_b =
            ThrottledCompletionModel::with_shared_semaphore(model_b, Arc::clone(&shared));

        let mut tasks = Vec::new();
        for _ in 0..3 {
            let m = wrapped_a.clone();
            tasks.push(tokio::spawn(async move {
                m.completion(CompletionRequest::default()).await
            }));
        }
        for _ in 0..3 {
            let m = wrapped_b.clone();
            tasks.push(tokio::spawn(async move {
                m.completion(CompletionRequest::default()).await
            }));
        }
        for task in tasks {
            task.await.expect("join task").expect("completion ok");
        }
        // Sharing one semaphore across two wrappers must keep peak
        // active count at 1 across the union of both pools.
        assert_eq!(
            peak.load(Ordering::SeqCst),
            1,
            "shared semaphore must limit across wrappers",
        );
    }

    /// When the underlying semaphore is closed, `acquire().await` fails
    /// and the wrapper must surface a `CompletionError::ProviderError`
    /// with the canonical `"completion throttle semaphore is closed"`
    /// message so audit logs can grep on it.
    #[tokio::test]
    async fn closed_semaphore_surfaces_provider_error() {
        #[derive(Clone)]
        struct NeverCalled;
        impl CompletionModel for NeverCalled {
            type Response = ();
            async fn completion(
                &self,
                _request: CompletionRequest,
            ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
                panic!("inner model must NOT be invoked when semaphore is closed");
            }
        }

        let shared = Arc::new(Semaphore::new(1));
        shared.close();
        let wrapped = ThrottledCompletionModel::with_shared_semaphore(NeverCalled, shared);

        let err = wrapped
            .completion(CompletionRequest::default())
            .await
            .expect_err("closed semaphore must surface an error");

        let rendered = format!("{err}");
        assert!(
            rendered.contains("completion throttle semaphore is closed"),
            "expected canonical close-error message; got: {rendered}",
        );
    }

    /// Happy-path passthrough: when the semaphore has capacity, the
    /// wrapper must invoke the inner model exactly once and return
    /// its response unchanged. Pins that the wrapper is transparent
    /// on the success path.
    #[tokio::test]
    async fn happy_path_invokes_inner_once_and_returns_response() {
        let invocations = Arc::new(AtomicUsize::new(0));

        #[derive(Clone)]
        struct CountingModel {
            invocations: Arc<AtomicUsize>,
        }
        impl CompletionModel for CountingModel {
            type Response = ();
            async fn completion(
                &self,
                _request: CompletionRequest,
            ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
                self.invocations.fetch_add(1, Ordering::SeqCst);
                Ok(CompletionResponse {
                    content: vec![AssistantContent::Text(Text {
                        text: "ok".to_string(),
                    })],
                    reasoning_content: None,
                    raw_response: (),
                })
            }
        }

        let wrapped = ThrottledCompletionModel::new(
            CountingModel {
                invocations: Arc::clone(&invocations),
            },
            4,
        );
        let response = wrapped
            .completion(CompletionRequest::default())
            .await
            .expect("completion ok");

        assert_eq!(invocations.load(Ordering::SeqCst), 1);
        assert_eq!(response.content.len(), 1);
        assert!(response.reasoning_content.is_none());
    }
}
