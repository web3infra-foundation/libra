//! OC-Phase 4 retry-policy integration test.
//!
//! Spec: `docs/improvement/opencode.md` → "Retry Policy 实现要求" — the
//! tool-loop side wraps every `model.completion(request)` with a retry
//! that:
//!   * keeps retrying on transient (`server_is_overloaded` /
//!     `server_error` / 5xx / 429) up to `max_retries`,
//!   * stops the moment a retry returns success,
//!   * surfaces non-retryable errors immediately,
//!   * never counts `ContextOverflow` against the budget (covered by
//!     the dedicated `ai_provider_context_overflow_compact_loop_test`).
//!
//! Implementation under test: [`RetryingCompletionModel`] — the
//! production wrapper that the TUI installs around every concrete
//! [`CompletionModel`]. The fixtures here drive a deterministic fake
//! model so a regression that flips the wrapper's public retry
//! contract (classes that retry, classes that surface immediately,
//! attempt budget) surfaces as a localised diff instead of as a
//! downstream tool-loop hang. Tests pin behaviour through the public
//! [`CompletionModel`] surface only — they never reach into the
//! wrapper's internal classifier so a future swap to the structured
//! [`ProviderError`] taxonomy stays a non-breaking change.
//!
//! Why not pull in `tool_loop::run_tool_loop` here: the retry happens
//! transparently at the [`CompletionModel`] layer, so wrapping the
//! whole tool loop would pull a large set of unrelated dependencies
//! (registry, observer, hook runner, etc.) without exercising any
//! additional retry semantics. The wrapper is the natural seam.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use libra::internal::ai::completion::{
    AssistantContent, CompletionError, CompletionModel, CompletionRequest, CompletionResponse,
    CompletionRetryEvent, CompletionRetryObserver, CompletionRetryPolicy, RetryingCompletionModel,
    Text,
};

/// Fake completion model that emits a scripted sequence of error
/// strings, then (optionally) a successful response. Each call to
/// `completion()` consumes the next entry from `script` — when the
/// script is exhausted, calls return success or panic per the
/// constructor.
#[derive(Clone)]
struct ScriptedFlakyModel {
    /// Recorded number of `completion()` invocations the wrapper made
    /// against this model. The retry loop drives this — a regression
    /// in the wrapper's "stop after success" semantics would push this
    /// past `script.len()`.
    calls: Arc<AtomicUsize>,
    /// Pre-recorded outcomes. Each `Some(message)` yields an Err with
    /// the given message; each `None` yields a successful response.
    /// Cloned per-call (the wrapper retries with cloned requests, so
    /// the script must be cheap to read repeatedly).
    script: Arc<Vec<Option<String>>>,
}

impl ScriptedFlakyModel {
    fn new(script: Vec<Option<String>>) -> Self {
        Self {
            calls: Arc::new(AtomicUsize::new(0)),
            script: Arc::new(script),
        }
    }
}

impl CompletionModel for ScriptedFlakyModel {
    type Response = ();

    async fn completion(
        &self,
        _request: CompletionRequest,
    ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
        let idx = self.calls.fetch_add(1, Ordering::SeqCst);
        let entry = self
            .script
            .get(idx)
            .cloned()
            .unwrap_or_else(|| Some("script exhausted".to_string()));

        match entry {
            Some(message) => Err(CompletionError::ProviderError(message)),
            None => Ok(CompletionResponse {
                content: vec![AssistantContent::Text(Text {
                    text: "ok".to_string(),
                })],
                reasoning_content: None,
                raw_response: (),
            }),
        }
    }
}

/// Capture every `on_retry` callback the wrapper makes so the test can
/// assert *how* the wrapper retried, not just *whether*.
#[derive(Default)]
struct RetryRecorder {
    events: std::sync::Mutex<Vec<CompletionRetryEvent>>,
}

impl RetryRecorder {
    fn snapshot(&self) -> Vec<CompletionRetryEvent> {
        self.events.lock().unwrap().clone()
    }
}

impl CompletionRetryObserver for RetryRecorder {
    fn on_retry(&self, event: &CompletionRetryEvent) {
        self.events.lock().unwrap().push(event.clone());
    }
}

/// Tight retry policy keeps the test fast while still exercising the
/// "max_retries" boundary. Real production policy is 3 retries with a
/// 1s base delay; we shrink the delays to milliseconds because we
/// already have unit coverage for the delay computation in
/// `error.rs::retry_policy_delays_double_then_saturate`.
fn fast_policy(max_retries: u32) -> CompletionRetryPolicy {
    CompletionRetryPolicy {
        max_retries,
        base_delay_ms: 1,
        max_delay_ms: 4,
    }
}

/// Scenario: the model returns the bare `server_is_overloaded`
/// stream-error code (no HTTP status prefix) for every call in the
/// budget (`max_retries + 1` total attempts). The wrapper must
/// surface the final error without exceeding that attempt budget —
/// a regression that infinitely retries would hang the test.
///
/// The fixture intentionally **omits** the `status 503:` prefix so
/// the doc's `server_is_overloaded` mapping (`docs/improvement/opencode.md`
/// line 1101) is exercised in isolation from the doc's HTTP-5xx rule
/// (line 1109). A combined `"status 503: server_is_overloaded"` string
/// would let either branch satisfy the test, hiding a regression in
/// the code-driven path.
#[tokio::test]
async fn retry_exhausts_budget_then_surfaces_final_error() {
    let max_retries = 3;
    // Total attempts = max_retries + 1 = 4.
    let model = ScriptedFlakyModel::new(vec![
        Some("server_is_overloaded".to_string()),
        Some("server_is_overloaded".to_string()),
        Some("server_is_overloaded".to_string()),
        Some("server_is_overloaded".to_string()),
        // A 5th entry that should never be observed; if the wrapper
        // exceeded its budget, the test would fail with
        // "ok"-as-success instead of an error.
        None,
    ]);
    let recorder = Arc::new(RetryRecorder::default());

    let wrapped = RetryingCompletionModel::new(model.clone())
        .with_policy(fast_policy(max_retries))
        .with_observer(recorder.clone());

    let result = wrapped.completion(CompletionRequest::default()).await;

    assert!(
        matches!(result, Err(CompletionError::ProviderError(_))),
        "expected ProviderError after retry exhaustion, got {result:?}"
    );
    assert_eq!(
        model.calls.load(Ordering::SeqCst),
        usize::try_from(max_retries + 1).unwrap(),
        "wrapper should make exactly max_retries + 1 attempts before surfacing"
    );
    let events = recorder.snapshot();
    // 3 retry callbacks (between attempts 1→2, 2→3, 3→4); none after
    // the final attempt because the wrapper surfaced the failure.
    assert_eq!(events.len(), usize::try_from(max_retries).unwrap());
    for (i, event) in events.iter().enumerate() {
        let attempt = u32::try_from(i).unwrap();
        assert_eq!(event.next_attempt, attempt + 2);
        assert_eq!(event.total_attempts, max_retries + 1);
        assert!(
            event.error.contains("server_is_overloaded"),
            "retry event at index {i} should carry the upstream message verbatim, got `{}`",
            event.error
        );
    }
}

/// Scenario: a bare HTTP `503` (status only, neutral message) also
/// retries — exercising the doc's HTTP-5xx rule from
/// `docs/improvement/opencode.md` line 1109 in isolation from the
/// `server_is_overloaded` code branch from line 1101. Splitting the
/// two ensures a regression in either branch fails with the offending
/// signature instead of being shadowed by the other.
#[tokio::test]
async fn retry_exhausts_budget_for_pure_http_503_branch() {
    let max_retries = 2;
    let model = ScriptedFlakyModel::new(vec![
        Some("status 503 service unavailable".to_string()),
        Some("status 503 service unavailable".to_string()),
        Some("status 503 service unavailable".to_string()),
        None,
    ]);
    let recorder = Arc::new(RetryRecorder::default());

    let wrapped = RetryingCompletionModel::new(model.clone())
        .with_policy(fast_policy(max_retries))
        .with_observer(recorder.clone());

    let result = wrapped.completion(CompletionRequest::default()).await;

    assert!(matches!(result, Err(CompletionError::ProviderError(_))));
    assert_eq!(
        model.calls.load(Ordering::SeqCst),
        usize::try_from(max_retries + 1).unwrap(),
        "HTTP-503 branch should follow the same max_retries + 1 attempt budget"
    );
    assert_eq!(
        recorder.snapshot().len(),
        usize::try_from(max_retries).unwrap()
    );
}

/// Scenario: the model returns transient overload twice then success.
/// The wrapper must stop retrying the moment success arrives — a
/// regression that kept retrying past success would consume the
/// remaining budget and amplify upstream load.
#[tokio::test]
async fn retry_stops_after_first_success() {
    let max_retries = 3;
    let model = ScriptedFlakyModel::new(vec![
        Some("status 503: server_is_overloaded".to_string()),
        Some("status 503: server_is_overloaded".to_string()),
        None, // success on the 3rd attempt
    ]);
    let recorder = Arc::new(RetryRecorder::default());

    let wrapped = RetryingCompletionModel::new(model.clone())
        .with_policy(fast_policy(max_retries))
        .with_observer(recorder.clone());

    let response = wrapped
        .completion(CompletionRequest::default())
        .await
        .expect("retry-then-success path");
    assert_eq!(response.content.len(), 1);
    // 2 transient + 1 success = 3 attempts; nothing after.
    assert_eq!(model.calls.load(Ordering::SeqCst), 3);
    assert_eq!(
        recorder.snapshot().len(),
        2,
        "exactly 2 retry callbacks between the failures and the success"
    );
}

/// Scenario: a non-retryable error (the doc table's `BadInput` /
/// `UserActionRequired` classes — `invalid_prompt`,
/// `insufficient_quota`, `usage_not_included`) surfaces immediately
/// on the first attempt. A regression that retried `invalid_prompt`
/// would loop the model on the same broken input until the budget
/// drained, so the test additionally asserts the model was called
/// exactly once and the retry observer never fired.
#[tokio::test]
async fn non_retryable_errors_surface_immediately() {
    let non_retryable_messages = [
        "invalid_prompt: tool schema rejected",
        "insufficient_quota: subscription required",
        "usage_not_included",
        "401 Unauthorized",
    ];

    for message in non_retryable_messages {
        let model = ScriptedFlakyModel::new(vec![
            Some(message.to_string()),
            // The next entry should never be observed; if it were,
            // the wrapper would return Ok and the assertion below
            // would fail.
            None,
        ]);
        let recorder = Arc::new(RetryRecorder::default());

        let wrapped = RetryingCompletionModel::new(model.clone())
            .with_policy(fast_policy(3))
            .with_observer(recorder.clone());

        let result = wrapped.completion(CompletionRequest::default()).await;

        assert!(
            matches!(result, Err(CompletionError::ProviderError(_))),
            "non-retryable {message:?} should surface as ProviderError on the first attempt, got {result:?}"
        );
        assert_eq!(
            model.calls.load(Ordering::SeqCst),
            1,
            "non-retryable {message:?} should not trigger a retry"
        );
        assert!(
            recorder.snapshot().is_empty(),
            "non-retryable {message:?} should not invoke the retry observer"
        );
    }
}

/// Scenario: the wrapper retries on every doc-listed retryable
/// signature in isolation. One fake call per signature so a
/// regression in any single match arm fails with the offending
/// signature in the assertion message rather than being shadowed by
/// a sibling rule. The signatures cover both `docs/improvement/opencode.md`
/// stream-error codes (`server_is_overloaded`, `server_error`) and
/// the HTTP-status branches (`429` + 5xx) called out at lines
/// 1101-1110.
///
/// Each fixture is hand-picked so dropping its target rule from the
/// wrapper's classifier would actually flip the signature into the
/// non-retryable branch. Where a doc rule is a strict substring of
/// another (`server_is_overloaded` overlaps the more permissive
/// `overloaded` keyword), the fixture cannot achieve perfect
/// isolation; the comment beside that signature documents the
/// overlap so a reader does not assume the wrapper is more
/// specific than it actually is.
#[tokio::test]
async fn every_retryable_signature_triggers_a_retry() {
    let signatures = [
        // Stream-error codes (doc table lines 1101-1102).
        // Note: this fixture is *also* matched by the generic
        // `overloaded` keyword the wrapper recognises for production
        // provider quirks — a strict-isolation fixture is not
        // achievable through public substring classification because
        // `server_is_overloaded` ⊃ `overloaded` by design.
        "server_is_overloaded",
        "server_error: backend stalled",
        // HTTP-status branches (doc rule line 1109): each phrasing
        // is chosen so the only retryable-needle hit is the numeric
        // `status XYZ` substring. None of them contains `timeout`,
        // `rate limit`, `unavailable`, or any other sibling needle.
        "status 500 internal failure",
        "status 502 bad gateway",
        "status 503 backend down",
        "status 504 gateway down",
        // `status 429` numeric branch — `too many requests` carries
        // no other retryable needles, so this isolates the numeric
        // status path from the bare `rate limit` keyword.
        "status 429 too many requests",
        // `rate limit` keyword path (line 1110) without an HTTP
        // status prefix, so the keyword is the only matching needle.
        "rate limit exceeded by upstream",
    ];

    for signature in signatures {
        let model = ScriptedFlakyModel::new(vec![
            Some(signature.to_string()),
            None, // success on retry
        ]);
        let recorder = Arc::new(RetryRecorder::default());

        let wrapped = RetryingCompletionModel::new(model.clone())
            .with_policy(fast_policy(3))
            .with_observer(recorder.clone());

        let response = wrapped
            .completion(CompletionRequest::default())
            .await
            .unwrap_or_else(|err| {
                panic!("retryable signature {signature:?} should recover on retry, got {err:?}");
            });

        assert_eq!(response.content.len(), 1);
        assert_eq!(
            model.calls.load(Ordering::SeqCst),
            2,
            "retryable signature {signature:?} should retry exactly once"
        );
        assert_eq!(
            recorder.snapshot().len(),
            1,
            "retryable signature {signature:?} should invoke the retry observer once"
        );
    }
}
