//! Retry policy helpers for transient completion-provider failures.
//!
//! Boundary: retries are bounded and only wrap errors classified as transient; caller
//! cancellation and validation errors must surface immediately. Provider tests exercise
//! retryable transport failures and non-retryable schema errors.

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
        // HTTP status branches (`docs/development/commands/_general.md` line 1109).
        "status 429",
        "status 500",
        "status 502",
        "status 503",
        "status 504",
        // 429 rate-limit (line 1110).
        "rate limit",
        // Stream-error codes from the doc taxonomy table
        // (`docs/development/commands/_general.md` lines 1101-1102): both must
        // classify as retryable transients per `StreamErrorKind::Transient`.
        "server_is_overloaded",
        "server_error",
        // Generic transient hints we have observed from production
        // providers — the doc table treats unknown stream codes as
        // `Transient` (line 1101 default), so these stay retryable.
        "temporarily unavailable",
        "temporarily overloaded",
        "overloaded",
        "try again",
        "timeout",
        "timed out",
        "connection reset",
        "error decoding response body",
        "stream ended before a usable response",
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
                reasoning_content: None,
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

    #[test]
    fn retries_stream_body_decode_failures() {
        assert!(is_retryable_provider_message(
            "DeepSeek stream ended before a usable response: error decoding response body"
        ));
    }

    /// `CompletionRetryPolicy::default()` must produce the canonical
    /// 3-retry / 500ms-base / 5000ms-cap shape. Audit logs and the
    /// `RetryingCompletionModel::new` shortcut rely on this surface;
    /// pin it so a re-tune is detected here, not in production behavior.
    #[test]
    fn retry_policy_default_pins_canonical_values() {
        let policy = CompletionRetryPolicy::default();
        assert_eq!(policy.max_retries, 3);
        assert_eq!(policy.base_delay_ms, 500);
        assert_eq!(policy.max_delay_ms, 5_000);
    }

    /// `backoff_delay` grows exponentially with the attempt index but
    /// is clamped to `max_delay_ms`. Pin the canonical 100ms base /
    /// 800ms cap table:
    ///   attempt=1 → 100ms (100 * 2^0)
    ///   attempt=2 → 200ms (100 * 2^1)
    ///   attempt=3 → 400ms (100 * 2^2)
    ///   attempt=4 → 800ms (100 * 2^3, exactly at cap)
    ///   attempt=5 → 800ms (would be 1600 but clamped)
    #[test]
    fn backoff_delay_grows_exponentially_until_max() {
        let base = 100;
        let max = 800;
        assert_eq!(backoff_delay(1, base, max), Duration::from_millis(100));
        assert_eq!(backoff_delay(2, base, max), Duration::from_millis(200));
        assert_eq!(backoff_delay(3, base, max), Duration::from_millis(400));
        assert_eq!(backoff_delay(4, base, max), Duration::from_millis(800));
        assert_eq!(backoff_delay(5, base, max), Duration::from_millis(800));
        // Saturating arithmetic protects against overflow at huge attempt
        // counts — clamped at max.
        assert_eq!(backoff_delay(64, base, max), Duration::from_millis(800));
    }

    /// `is_retryable_error` must accept the HTTP-status-and-rate-limit
    /// taxonomy from `docs/development/commands/_general.md` line 1109. Pin the
    /// matrix so a future "tighten the retry set" refactor doesn't
    /// silently drop one of the documented retryable conditions.
    #[test]
    fn is_retryable_error_classifies_documented_provider_transients() {
        let cases = [
            "Provider returned status 429: too many requests",
            "Provider returned status 500: internal server error",
            "Provider returned status 502: bad gateway",
            "Provider returned status 503: service unavailable",
            "Provider returned status 504: gateway timeout",
            "rate limit exceeded",
            "server_is_overloaded",
            "server_error",
            "temporarily unavailable",
            "temporarily overloaded",
            "overloaded",
            "please try again later",
            "operation timeout",
            "connection timed out",
            "connection reset by peer",
            "error decoding response body",
            "stream ended before a usable response",
        ];
        for msg in cases {
            let err = CompletionError::ProviderError(msg.to_string());
            assert!(is_retryable_error(&err), "expected '{msg}' to be retryable",);
        }
    }

    /// Non-retryable error categories must surface immediately:
    /// `JsonError`, `RequestError`, `NotImplemented`. These come from
    /// schema/serialization paths where retrying would never change
    /// the outcome.
    #[test]
    fn is_retryable_error_rejects_schema_and_request_errors() {
        let json_err = CompletionError::JsonError(
            serde_json::from_str::<serde_json::Value>("not json").unwrap_err(),
        );
        assert!(!is_retryable_error(&json_err));

        let not_impl = CompletionError::NotImplemented("missing".to_string());
        assert!(!is_retryable_error(&not_impl));

        let req_err: Box<dyn std::error::Error + Send + Sync + 'static> = "bad request".into();
        let req_err = CompletionError::RequestError(req_err);
        assert!(!is_retryable_error(&req_err));
    }

    /// Provider messages that don't match any of the documented
    /// transient patterns must NOT classify as retryable. Pins the
    /// "unknown provider message = fail fast" rule.
    #[test]
    fn is_retryable_provider_message_rejects_non_transient_text() {
        let cases = [
            "Invalid API key",
            "Model not found",
            "Quota exceeded permanently",
            "Permission denied",
            "Request body too large",
            "",
        ];
        for msg in cases {
            assert!(
                !is_retryable_provider_message(msg),
                "msg '{msg}' must NOT be classified retryable",
            );
        }
    }

    /// `is_retryable_provider_message` is case-insensitive: the
    /// `RATE LIMIT` and `Rate Limit` forms both match.
    #[test]
    fn is_retryable_provider_message_is_case_insensitive() {
        assert!(is_retryable_provider_message("RATE LIMIT exceeded"));
        assert!(is_retryable_provider_message("Rate Limit"));
        assert!(is_retryable_provider_message("rate limit"));
        assert!(is_retryable_provider_message("STATUS 503"));
    }

    /// Non-retryable errors must surface on the first attempt without
    /// retrying. Verifies that the retry loop short-circuits via
    /// `is_retryable_error`, not just via exhausting the attempts.
    #[tokio::test]
    async fn non_retryable_error_surfaces_immediately_without_retry() {
        #[derive(Clone)]
        struct AlwaysNotImpl {
            attempts: Arc<AtomicUsize>,
        }
        impl CompletionModel for AlwaysNotImpl {
            type Response = ();
            async fn completion(
                &self,
                _request: CompletionRequest,
            ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
                self.attempts.fetch_add(1, Ordering::SeqCst);
                Err(CompletionError::NotImplemented(
                    "feature missing".to_string(),
                ))
            }
        }

        let attempts = Arc::new(AtomicUsize::new(0));
        let wrapped = RetryingCompletionModel::new(AlwaysNotImpl {
            attempts: Arc::clone(&attempts),
        })
        .with_policy(CompletionRetryPolicy {
            max_retries: 5, // would retry 5x if classification were wrong
            base_delay_ms: 1,
            max_delay_ms: 2,
        });

        let result = wrapped.completion(CompletionRequest::default()).await;
        assert!(matches!(result, Err(CompletionError::NotImplemented(_))));
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            1,
            "non-retryable error must NOT retry",
        );
    }

    /// When retries are exhausted on a retryable error, the wrapper
    /// must return that error (not the unreachable "retry loop exited"
    /// sentinel). Pin the `attempt >= total_attempts` exit path.
    #[tokio::test]
    async fn exhausted_retries_returns_last_provider_error() {
        #[derive(Clone)]
        struct AlwaysFlaky {
            attempts: Arc<AtomicUsize>,
        }
        impl CompletionModel for AlwaysFlaky {
            type Response = ();
            async fn completion(
                &self,
                _request: CompletionRequest,
            ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
                self.attempts.fetch_add(1, Ordering::SeqCst);
                Err(CompletionError::ProviderError(
                    "status 503: overloaded".to_string(),
                ))
            }
        }

        let attempts = Arc::new(AtomicUsize::new(0));
        let wrapped = RetryingCompletionModel::new(AlwaysFlaky {
            attempts: Arc::clone(&attempts),
        })
        .with_policy(CompletionRetryPolicy {
            max_retries: 2,
            base_delay_ms: 1,
            max_delay_ms: 2,
        });

        let result = wrapped.completion(CompletionRequest::default()).await;
        match result {
            Err(CompletionError::ProviderError(msg)) => {
                assert!(msg.contains("status 503"), "got: {msg}");
            }
            other => panic!("expected ProviderError; got {other:?}"),
        }
        // 1 initial + 2 retries = 3 total attempts.
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    /// `CompletionRetryObserver::on_retry` must be called once per
    /// retry attempt (i.e. `total_attempts - 1` times when retries
    /// are exhausted). Each event carries the correct attempt number,
    /// total, and error string.
    #[tokio::test]
    async fn retry_observer_is_invoked_per_retry_with_correct_event() {
        use std::sync::Mutex;

        #[derive(Default)]
        struct CapturingObserver {
            events: Mutex<Vec<CompletionRetryEvent>>,
        }
        impl CompletionRetryObserver for CapturingObserver {
            fn on_retry(&self, event: &CompletionRetryEvent) {
                self.events.lock().unwrap().push(event.clone());
            }
        }

        #[derive(Clone)]
        struct AlwaysFlaky {
            attempts: Arc<AtomicUsize>,
        }
        impl CompletionModel for AlwaysFlaky {
            type Response = ();
            async fn completion(
                &self,
                _request: CompletionRequest,
            ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
                self.attempts.fetch_add(1, Ordering::SeqCst);
                Err(CompletionError::ProviderError(
                    "status 503: overloaded".to_string(),
                ))
            }
        }

        let observer = Arc::new(CapturingObserver::default());
        let attempts = Arc::new(AtomicUsize::new(0));
        let wrapped = RetryingCompletionModel::new(AlwaysFlaky {
            attempts: Arc::clone(&attempts),
        })
        .with_policy(CompletionRetryPolicy {
            max_retries: 2,
            base_delay_ms: 1,
            max_delay_ms: 2,
        })
        .with_observer(observer.clone());

        let _ = wrapped.completion(CompletionRequest::default()).await;

        let events = observer.events.lock().unwrap();
        // max_retries = 2 → 3 total attempts → 2 retry events
        // (the final failure doesn't emit a retry event because
        // there's no next attempt to schedule).
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].next_attempt, 2);
        assert_eq!(events[0].total_attempts, 3);
        assert!(events[0].error.contains("status 503"));
        assert_eq!(events[1].next_attempt, 3);
        assert_eq!(events[1].total_attempts, 3);
    }

    /// `RetryingCompletionModel::new` constructor must initialize the
    /// policy to the canonical default. Combined with the
    /// `with_policy` / `with_observer` builders, callers should be
    /// able to construct the wrapper with sane defaults in one line.
    #[test]
    fn new_constructor_uses_default_policy_and_no_observer() {
        #[derive(Clone)]
        struct Dummy;
        impl CompletionModel for Dummy {
            type Response = ();
            async fn completion(
                &self,
                _request: CompletionRequest,
            ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
                unimplemented!()
            }
        }

        let wrapped = RetryingCompletionModel::new(Dummy);
        assert_eq!(wrapped.policy.max_retries, 3);
        assert_eq!(wrapped.policy.base_delay_ms, 500);
        assert_eq!(wrapped.policy.max_delay_ms, 5_000);
        assert!(wrapped.observer.is_none());
    }
}
