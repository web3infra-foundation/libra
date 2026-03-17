use std::{sync::Arc, time::Duration};

use super::{CompletionError, CompletionModel, CompletionRequest, CompletionResponse};

#[derive(Clone, Debug)]
pub struct CompletionRetryPolicy {
    pub max_retries: u32,
    pub base_delay_ms: u64,
    pub max_delay_ms: u64,
}

impl Default for CompletionRetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay_ms: 500,
            max_delay_ms: 5_000,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CompletionRetryEvent {
    pub next_attempt: u32,
    pub total_attempts: u32,
    pub delay: Duration,
    pub error: String,
}

pub trait CompletionRetryObserver: Send + Sync {
    fn on_retry(&self, _event: &CompletionRetryEvent) {}
}

#[derive(Clone)]
pub struct RetryingCompletionModel<M> {
    inner: M,
    policy: CompletionRetryPolicy,
    observer: Option<Arc<dyn CompletionRetryObserver>>,
}

impl<M> RetryingCompletionModel<M> {
    pub fn new(inner: M) -> Self {
        Self {
            inner,
            policy: CompletionRetryPolicy::default(),
            observer: None,
        }
    }

    pub fn with_policy(mut self, policy: CompletionRetryPolicy) -> Self {
        self.policy = policy;
        self
    }

    pub fn with_observer(mut self, observer: Arc<dyn CompletionRetryObserver>) -> Self {
        self.observer = Some(observer);
        self
    }
}

impl<M: CompletionModel> CompletionModel for RetryingCompletionModel<M> {
    type Response = M::Response;

    async fn completion(
        &self,
        request: CompletionRequest,
    ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
        let total_attempts = self.policy.max_retries.saturating_add(1);

        for attempt in 1..=total_attempts {
            match self.inner.completion(request.clone()).await {
                Ok(response) => return Ok(response),
                Err(err) => {
                    if attempt >= total_attempts || !is_retryable_error(&err) {
                        return Err(err);
                    }

                    let delay =
                        backoff_delay(attempt, self.policy.base_delay_ms, self.policy.max_delay_ms);
                    if let Some(observer) = &self.observer {
                        observer.on_retry(&CompletionRetryEvent {
                            next_attempt: attempt + 1,
                            total_attempts,
                            delay,
                            error: err.to_string(),
                        });
                    }
                    tokio::time::sleep(delay).await;
                }
            }
        }

        Err(CompletionError::ResponseError(
            "retry loop exited without returning a completion".to_string(),
        ))
    }
}

fn backoff_delay(attempt: u32, base_delay_ms: u64, max_delay_ms: u64) -> Duration {
    let exp = 2_u64.saturating_pow(attempt.saturating_sub(1));
    let delay_ms = base_delay_ms.saturating_mul(exp).min(max_delay_ms);
    Duration::from_millis(delay_ms)
}

fn is_retryable_error(err: &CompletionError) -> bool {
    match err {
        CompletionError::HttpError(http) => {
            http.is_timeout() || http.is_connect() || http.is_request() || http.is_body()
        }
        CompletionError::ProviderError(message) => is_retryable_provider_message(message),
        CompletionError::ResponseError(message) => is_retryable_provider_message(message),
        CompletionError::JsonError(_)
        | CompletionError::RequestError(_)
        | CompletionError::NotImplemented(_) => false,
    }
}

fn is_retryable_provider_message(message: &str) -> bool {
    let msg = message.to_ascii_lowercase();
    [
        "status 429",
        "status 500",
        "status 502",
        "status 503",
        "status 504",
        "rate limit",
        "temporarily unavailable",
        "temporarily overloaded",
        "overloaded",
        "try again",
        "timeout",
        "timed out",
        "connection reset",
    ]
    .iter()
    .any(|needle| msg.contains(needle))
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;
    use crate::internal::ai::completion::{
        CompletionRequest, CompletionResponse,
        message::{AssistantContent, Text},
    };

    #[derive(Clone)]
    struct FlakyModel {
        attempts: Arc<AtomicUsize>,
        fail_until: usize,
    }

    impl CompletionModel for FlakyModel {
        type Response = ();

        async fn completion(
            &self,
            _request: CompletionRequest,
        ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
            let current = self.attempts.fetch_add(1, Ordering::SeqCst) + 1;
            if current <= self.fail_until {
                return Err(CompletionError::ProviderError(
                    "status 503: overloaded".into(),
                ));
            }
            Ok(CompletionResponse {
                content: vec![AssistantContent::Text(Text { text: "ok".into() })],
                raw_response: (),
            })
        }
    }

    #[tokio::test]
    async fn retries_transient_provider_errors() {
        let model = FlakyModel {
            attempts: Arc::new(AtomicUsize::new(0)),
            fail_until: 2,
        };
        let wrapped =
            RetryingCompletionModel::new(model.clone()).with_policy(CompletionRetryPolicy {
                max_retries: 3,
                base_delay_ms: 1,
                max_delay_ms: 2,
            });

        let response = wrapped
            .completion(CompletionRequest::default())
            .await
            .unwrap();

        assert_eq!(response.content.len(), 1);
        assert_eq!(model.attempts.load(Ordering::SeqCst), 3);
    }
}
