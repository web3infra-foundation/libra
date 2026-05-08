//! OC-Phase 4 context-overflow compaction loop integration test.
//!
//! Spec: `docs/improvement/opencode.md` → "Provider Error Taxonomy &
//! Retry Policy" — `ContextOverflow` is *not* retryable through the
//! transient-error retry budget; the runtime instead takes a separate
//! `compaction → retry-once` branch that does **not** consume the
//! retry budget.
//!
//! What this test pins:
//!
//! 1. The taxonomy classifies `context_length_exceeded` (HTTP 413, in
//!    the message text, or in the JSON body) as
//!    [`ProviderError::ContextOverflow`] with
//!    `requires_compaction() == true` and `is_retryable() == false`.
//!    A regression here would silently route overflow through the
//!    transient retry path and burn money on a guaranteed-failing
//!    retry.
//! 2. After the canonical compaction agent rewrites the transcript
//!    into a [`ContextHandoff`], a follow-up provider call against a
//!    much smaller request succeeds.
//! 3. The retry budget set on a [`RetryingCompletionModel`] is
//!    untouched by the compaction loop — i.e. the wrapper's transient
//!    retry counter is the same after the compaction recovery as it
//!    would be on a single successful call. This is the doc's
//!    "compaction does not count against `max_retries`" invariant.
//!
//! Implementation note: the production `tool_loop` does not yet wire
//! `ContextOverflow → run_compaction → retry` into a single closed
//! loop; that integration is the follow-up to P4.1's structured-error
//! plumbing. This test demonstrates the algorithm at the orchestrator
//! level using the public primitives every consumer (tool_loop, sub
//! agent dispatcher, future Goal supervisor) will compose. When the
//! tool_loop integration lands, the same fixtures will move into
//! that module's tests with minimal changes.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use libra::internal::ai::{
    completion::{
        AssistantContent, CompletionError, CompletionModel, CompletionRequest, CompletionResponse,
        CompletionRetryEvent, CompletionRetryObserver, CompletionRetryPolicy, Message,
        RetryingCompletionModel, Text,
    },
    context_budget::{embedded_compaction_system_prompt, parse_handoff_template, run_compaction},
    providers::{ProviderError, parse_api_error},
};
use uuid::Uuid;

/// Canonical 8-section summary the fake compaction agent emits.
/// Matches the literal SUMMARY_TEMPLATE from
/// `docs/improvement/opencode.md` line 1176-1206 byte-for-byte (modulo
/// fixture content) so the strict parser is the authority on shape.
const VALID_SUMMARY: &str = "\
## Goal
- Pick the next file to refactor

## Constraints & Preferences
- Stay within the existing test harness

## Progress
### Done
- Read the original transcript

### In Progress
- Selecting the next module

### Blocked
- (none)

## Key Decisions
- Compact before retry to stay under the model's context window

## Next Steps
- Issue the smaller follow-up request

## Critical Context
- The first attempt failed with context_length_exceeded

## Relevant Files
- src/internal/ai/agent/runtime/tool_loop.rs: site of the future inline integration
";

/// Fake completion model with two distinct response paths:
///
/// - On the **first** call it emits a context-overflow error message
///   matching one of the three opencode error-classification paths
///   (HTTP 413, message-driven, or body-driven). The runtime feeds
///   this through [`parse_api_error`] / our manual orchestrator to
///   classify it as [`ProviderError::ContextOverflow`].
/// - On every **subsequent** call it emits a successful response.
///   This is what a recovery-after-compaction call should observe.
///
/// `calls` records the total number of `completion()` invocations the
/// orchestrator made — the test asserts on this to show that the
/// compaction-then-retry path does not double-call beyond the
/// expected 2 attempts.
#[derive(Clone)]
struct OverflowOnceModel {
    calls: Arc<AtomicUsize>,
    /// Captured chat-history length on the second (recovery) call. A
    /// regression that fed the original (over-budget) transcript back
    /// to the model would surface here.
    recovery_history_len: Arc<AtomicUsize>,
    overflow_message: String,
}

impl OverflowOnceModel {
    fn new(overflow_message: impl Into<String>) -> Self {
        Self {
            calls: Arc::new(AtomicUsize::new(0)),
            recovery_history_len: Arc::new(AtomicUsize::new(0)),
            overflow_message: overflow_message.into(),
        }
    }
}

impl CompletionModel for OverflowOnceModel {
    type Response = ();

    async fn completion(
        &self,
        request: CompletionRequest,
    ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
        let attempt = self.calls.fetch_add(1, Ordering::SeqCst);
        if attempt == 0 {
            // The taxonomy treats `ContextOverflow` as a non-retryable
            // signature, so the wrapping `RetryingCompletionModel`
            // surfaces it on the first attempt without burning the
            // transient retry budget. `is_retryable_provider_message`
            // does not match `context_length_exceeded`, so the
            // wrapper correctly classifies this as non-retryable.
            return Err(CompletionError::ProviderError(
                self.overflow_message.clone(),
            ));
        }
        self.recovery_history_len
            .store(request.chat_history.len(), Ordering::SeqCst);
        Ok(CompletionResponse {
            content: vec![AssistantContent::Text(Text {
                text: "ok-after-compaction".to_string(),
            })],
            reasoning_content: None,
            raw_response: (),
        })
    }
}

/// Capture compaction-agent invocations: the canonical implementation
/// emits the doc's 8-section summary regardless of input. Tests assert
/// that exactly one compaction round happened during the recovery.
#[derive(Clone)]
struct CannedSummaryModel {
    invocations: Arc<AtomicUsize>,
}

impl CannedSummaryModel {
    fn new() -> Self {
        Self {
            invocations: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl CompletionModel for CannedSummaryModel {
    type Response = ();

    async fn completion(
        &self,
        _request: CompletionRequest,
    ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
        self.invocations.fetch_add(1, Ordering::SeqCst);
        Ok(CompletionResponse {
            content: vec![AssistantContent::Text(Text {
                text: VALID_SUMMARY.to_string(),
            })],
            reasoning_content: None,
            raw_response: (),
        })
    }
}

/// Recorder used to verify the wrapping `RetryingCompletionModel` did
/// NOT issue any transient-retry callbacks during the overflow round.
#[derive(Default)]
struct RetryRecorder {
    events: std::sync::Mutex<Vec<CompletionRetryEvent>>,
}

impl RetryRecorder {
    fn count(&self) -> usize {
        self.events.lock().unwrap().len()
    }
}

impl CompletionRetryObserver for RetryRecorder {
    fn on_retry(&self, event: &CompletionRetryEvent) {
        self.events.lock().unwrap().push(event.clone());
    }
}

/// Tight retry policy: we want to prove the retry budget is not
/// touched by the compaction round, so even a max_retries=3 budget
/// must remain unused at the end of the test.
fn fast_policy() -> CompletionRetryPolicy {
    CompletionRetryPolicy {
        max_retries: 3,
        base_delay_ms: 1,
        max_delay_ms: 4,
    }
}

/// Build a synthetic over-budget chat history. The exact tokens do
/// not matter — only that the orchestrator can compress it via
/// [`run_compaction`] and produce a meaningfully shorter follow-up
/// request.
fn over_budget_history() -> Vec<Message> {
    let mut history = Vec::new();
    for i in 0..16 {
        history.push(Message::user(format!(
            "user prompt {i}: {}",
            "context payload ".repeat(64)
        )));
    }
    history
}

/// Scenario A: HTTP 413 → ContextOverflow.
///
/// Verifies the doc's classification path 1 (canonical 413). The
/// taxonomy must surface a `ContextOverflow` carrying the original
/// message + body for telemetry, and `requires_compaction()` must be
/// true so the orchestrator picks the recovery branch instead of the
/// transient retry branch.
#[test]
fn context_overflow_classification_path_413() {
    use std::collections::HashMap;
    let err = parse_api_error(
        Some(413),
        "Request entity too large",
        HashMap::new(),
        Some(r#"{"error":{"message":"Request entity too large"}}"#.to_string()),
        "openai",
        false,
    );
    assert!(matches!(err, ProviderError::ContextOverflow { .. }));
    assert!(err.requires_compaction());
    assert!(!err.is_retryable());
}

/// Scenario B: status 400 + body code → ContextOverflow.
///
/// Many providers report context overflow with a generic 400 plus the
/// canonical code in the JSON body (`{"error":{"code":"context_length_exceeded"}}`).
/// A regression here would silently demote the overflow to a generic
/// `ApiError`, costing one wasted retry before the wrapper gives up.
#[test]
fn context_overflow_classification_path_body_code() {
    use std::collections::HashMap;
    let err = parse_api_error(
        Some(400),
        "Bad Request",
        HashMap::new(),
        Some(r#"{"error":{"code":"context_length_exceeded","message":"too long"}}"#.to_string()),
        "anthropic",
        false,
    );
    assert!(matches!(err, ProviderError::ContextOverflow { .. }));
    assert!(err.requires_compaction());
}

/// Scenario C: end-to-end overflow → compaction → retry-once.
///
/// Walks the full algorithm:
///
/// 1. Wrap the fake provider in `RetryingCompletionModel`. The first
///    `completion()` returns the overflow error string. The wrapper's
///    classifier treats `context_length_exceeded` as non-retryable,
///    so it surfaces the error immediately without burning any of
///    the 3 retries — verified by the `RetryRecorder` at the end.
/// 2. The orchestrator (this test) classifies the surfaced error,
///    confirms it requires compaction, and runs the canonical
///    compaction agent against the over-budget history.
/// 3. Build a follow-up request whose `chat_history` is just the
///    summary — much smaller than the original — and call the wrapper
///    again. This time the model returns success.
/// 4. Assertions:
///    - exactly 2 model calls (overflow + recovery),
///    - exactly 1 compaction-agent invocation,
///    - 0 transient retry callbacks (`max_retries` budget unused),
///    - the recovery request carried a strictly smaller history than
///      the original.
#[tokio::test]
async fn context_overflow_drives_compaction_then_retry_without_burning_budget() {
    let initial_history = over_budget_history();
    let initial_history_len = initial_history.len();

    let provider = OverflowOnceModel::new(
        // Match the doc table's body-driven classification path: the
        // model surface emits a string with the canonical opencode
        // code so a future structured-`ProviderError` plumbing PR
        // recognises it via either path.
        "Bad Request: context_length_exceeded — payload too long",
    );
    let recorder = Arc::new(RetryRecorder::default());
    let wrapped = RetryingCompletionModel::new(provider.clone())
        .with_policy(fast_policy())
        .with_observer(recorder.clone());

    // ----- Step 1: first attempt surfaces the overflow error -----
    let request = CompletionRequest {
        chat_history: initial_history.clone(),
        ..Default::default()
    };
    let first = wrapped.completion(request).await;
    let err_message = match first {
        Err(CompletionError::ProviderError(message)) => message,
        other => panic!(
            "first attempt must surface a ProviderError carrying the overflow message, got {other:?}"
        ),
    };
    assert!(
        err_message.contains("context_length_exceeded"),
        "first-attempt error must round-trip the canonical opencode code, got `{err_message}`"
    );
    assert_eq!(
        provider.calls.load(Ordering::SeqCst),
        1,
        "wrapper must NOT have retried the overflow — it is non-retryable"
    );
    assert_eq!(
        recorder.count(),
        0,
        "wrapper must NOT have invoked the retry observer for an overflow"
    );

    // ----- Step 2: classify + compact -----
    let mut classified_headers = std::collections::HashMap::new();
    classified_headers.insert("content-type".to_string(), "application/json".to_string());
    let classified = parse_api_error(
        Some(400),
        &err_message,
        classified_headers,
        Some(format!(
            r#"{{"error":{{"code":"context_length_exceeded","message":"{err_message}"}}}}"#
        )),
        "anthropic",
        false,
    );
    assert!(classified.requires_compaction());
    assert!(!classified.is_retryable());

    let compaction_model = CannedSummaryModel::new();
    let frame_id = Uuid::new_v4();
    let handoff = run_compaction(
        &compaction_model,
        embedded_compaction_system_prompt(),
        // The orchestrator passes the over-budget transcript as a
        // single inline string; the canned model ignores it and
        // always emits the doc's 8-section template.
        "synthetic over-budget transcript placeholder",
        frame_id,
        Vec::new(),
        Vec::new(),
        4_096,
    )
    .await
    .expect("compaction must succeed against the canonical template");
    assert_eq!(
        compaction_model.invocations.load(Ordering::SeqCst),
        1,
        "compaction agent should be invoked exactly once during recovery"
    );

    // The summary must parse through the strict 8-section parser.
    // This is a pre-condition of the recovery: the dispatcher would
    // not feed an unparseable summary back into the model.
    let parsed = parse_handoff_template(&handoff.summary)
        .expect("handoff summary must parse via the strict 8-section parser");
    assert!(
        !parsed.goal.bullets.is_empty(),
        "Goal section must be populated"
    );

    // ----- Step 3: recovery call with the compacted transcript -----
    let recovered_request = CompletionRequest {
        chat_history: vec![Message::user(handoff.summary.clone())],
        ..Default::default()
    };
    let recovered_history_len = recovered_request.chat_history.len();
    let second = wrapped
        .completion(recovered_request)
        .await
        .expect("recovery call must succeed against the smaller transcript");
    let text = second
        .content
        .iter()
        .find_map(|c| match c {
            AssistantContent::Text(t) => Some(t.text.clone()),
            AssistantContent::ToolCall(_) => None,
        })
        .expect("recovery response must contain a text part");
    assert_eq!(text, "ok-after-compaction");

    // ----- Step 4: invariants -----
    assert_eq!(
        provider.calls.load(Ordering::SeqCst),
        2,
        "exactly two provider calls — overflow then recovery — must have happened"
    );
    assert_eq!(
        provider.recovery_history_len.load(Ordering::SeqCst),
        recovered_history_len,
        "recovery call must carry exactly the compacted history"
    );
    assert!(
        recovered_history_len < initial_history_len,
        "recovery history ({recovered_history_len}) must be strictly smaller than the original ({initial_history_len})"
    );
    assert_eq!(
        recorder.count(),
        0,
        "the transient retry observer must NEVER fire during a compaction recovery — \
         the doc's `not counted against retry budget` invariant"
    );
}
