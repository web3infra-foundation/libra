//! Structured provider error taxonomy + retry policy types.
//!
//! This module is the OC-Phase 4 pre-P4.1 deliverable from
//! `docs/improvement/opencode.md`. It mirrors opencode's
//! `provider/error.ts:105-202` so a Libra runtime that wraps a real
//! HTTP failure can pick the right recovery strategy:
//!
//! - [`ProviderError::ContextOverflow`] — input exceeded the model's
//!   context window. The runtime triggers compaction and retries
//!   **once** without consuming the retry budget.
//! - [`ProviderError::ApiError`] — generic HTTP/transport failure with
//!   an `is_retryable` flag and the response headers (so a `429`
//!   response can have its `Retry-After` honored).
//! - [`ProviderError::StreamError`] — mid-stream JSON event carrying
//!   an error code, classified by [`StreamErrorKind`] so retry vs.
//!   user-action surfaces are distinct.
//!
//! What this module is:
//! - Pure data types + a small parsing helper. Object-safe Send + Sync.
//! - The error code → kind mapping table verbatim from the doc.
//!
//! What this module is **not**:
//! - It does not call retry from inside `tool_loop`. The actual retry
//!   wire-up lands in a later PR (P4.X) on top of this module.
//! - It does not yet own the per-provider `parseAPICallError` logic.
//!   Today's per-provider clients hand-roll their error mapping; the
//!   migration is gated on P4.1's `ProviderTransform` trait introducing
//!   the right hook.

use std::{collections::HashMap, time::Duration};

use serde::{Deserialize, Serialize};

/// How the runtime should react to a streaming-mode error code.
///
/// Order of variants is **stable** so a future serialization channel
/// can use it as a discriminant. New variants must be appended at the
/// end and reflected in [`parse_stream_error_kind`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamErrorKind {
    /// Backoff + retry usually recovers (HTTP 5xx, server overload).
    Transient,
    /// User must take action (quota exhausted, subscription expired).
    /// The runtime surfaces an actionable message instead of retrying.
    UserActionRequired,
    /// The request itself was malformed (`invalid_prompt`). The model
    /// must see the error to self-correct; retry would loop on the
    /// same input.
    BadInput,
    /// Request exceeded the model's context window. The runtime
    /// triggers compaction and retries once.
    ContextOverflow,
}

/// Parse a mid-stream provider error **code** into a
/// [`StreamErrorKind`].
///
/// The mapping table is verbatim from the doc:
///
/// | opencode code             | Libra kind               | retryable |
/// |---------------------------|--------------------------|-----------|
/// | `context_length_exceeded` | `ContextOverflow`        | false     |
/// | `insufficient_quota`      | `UserActionRequired`     | false     |
/// | `usage_not_included`      | `UserActionRequired`     | false     |
/// | `invalid_prompt`          | `BadInput`               | false     |
/// | `server_is_overloaded`    | `Transient`              | true      |
/// | `server_error`            | `Transient`              | true      |
///
/// Unrecognized codes default to [`StreamErrorKind::Transient`] so the
/// runtime errs on the side of retry — matching opencode's permissive
/// fallback. Callers that want strict behavior must additionally check
/// against an allow-list before retrying.
pub fn parse_stream_error_kind(code: &str) -> StreamErrorKind {
    match code {
        "context_length_exceeded" => StreamErrorKind::ContextOverflow,
        "insufficient_quota" | "usage_not_included" => StreamErrorKind::UserActionRequired,
        "invalid_prompt" => StreamErrorKind::BadInput,
        "server_is_overloaded" | "server_error" => StreamErrorKind::Transient,
        _ => StreamErrorKind::Transient,
    }
}

impl StreamErrorKind {
    /// Whether the runtime should retry on this kind of stream error.
    /// `Transient` retries with backoff; everything else is terminal.
    /// `ContextOverflow` is **not** considered retryable here — the
    /// runtime takes a separate compaction-then-retry-once branch.
    pub fn is_retryable(self) -> bool {
        matches!(self, Self::Transient)
    }
}

/// Top-level structured failure from a provider call.
///
/// Each variant carries the metadata the runtime needs to choose a
/// recovery path. The struct derives `Debug` only — a `Display` impl
/// arrives alongside the `tool_loop` retry wiring (P4.X) when the
/// runtime knows what to surface to the user.
#[derive(Debug, Clone)]
pub enum ProviderError {
    /// Input exceeded the model's context window. Carries the body so
    /// a future telemetry sink can log the original error verbatim.
    ContextOverflow {
        message: String,
        response_body: Option<String>,
    },
    /// Generic HTTP / transport failure. `is_retryable` reflects the
    /// runtime's classification (5xx + 429 → true, 4xx-not-413 → false
    /// unless overridden by provider-specific heuristics).
    ApiError {
        message: String,
        status_code: Option<u16>,
        is_retryable: bool,
        response_headers: HashMap<String, String>,
        response_body: Option<String>,
    },
    /// A streaming JSON event carried an error payload. The `kind`
    /// drives retry / surface decisions; the `message` and
    /// `response_body` go straight to the user's error toast.
    StreamError {
        kind: StreamErrorKind,
        message: String,
        response_body: String,
    },
}

impl ProviderError {
    /// Whether the runtime should retry on this error.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::ContextOverflow { .. } => false,
            Self::ApiError { is_retryable, .. } => *is_retryable,
            Self::StreamError { kind, .. } => kind.is_retryable(),
        }
    }

    /// Whether the runtime should trigger compaction and retry once
    /// (NOT counted against the retry budget).
    pub fn requires_compaction(&self) -> bool {
        matches!(
            self,
            Self::ContextOverflow { .. }
                | Self::StreamError {
                    kind: StreamErrorKind::ContextOverflow,
                    ..
                }
        )
    }

    /// Convenience for the runtime: `Some(secs)` when the response
    /// carried a `Retry-After: <seconds>` header (HTTP-date variant
    /// is intentionally not parsed here — opencode's runtime does the
    /// HTTP-date conversion at the call site).
    pub fn retry_after_seconds(&self) -> Option<u64> {
        let Self::ApiError {
            response_headers, ..
        } = self
        else {
            return None;
        };
        response_headers.iter().find_map(|(name, value)| {
            if name.eq_ignore_ascii_case("retry-after") {
                value.trim().parse::<u64>().ok()
            } else {
                None
            }
        })
    }
}

/// Classify a provider HTTP failure given the response status,
/// optional message, and provider id.
///
/// Algorithm (mirrors the doc's 5 branches; the OpenAI sub-branch
/// is approximated, see note below):
///
/// 1. message **or** `response_body` contains `"context_length_exceeded"`,
///    or status == 413 → [`ProviderError::ContextOverflow`]. Body-driven
///    detection covers providers that report context overflow as a
///    400 with the canonical code in the JSON body.
/// 2. status ∈ {502, 503, 504} → `ApiError { is_retryable: true }`.
/// 3. status == 429 → `ApiError { is_retryable: true }` and the
///    caller is expected to honor the `Retry-After` header.
/// 4. provider_id starts with `"openai"` → defer to a hand-rolled
///    `isOpenAiErrorRetryable` heuristic. **Approximation note:**
///    Libra's first cut classifies any 5xx as retryable and any 4xx
///    as not retryable, instead of porting the full opencode
///    heuristic. Tracked for P4.1; widening this is a non-breaking
///    change because callers receive `ApiError` either way.
/// 5. otherwise → `ApiError { is_retryable: default_retryable }`.
///    The caller may pass `true` when the provider SDK already
///    classified the error as retryable, or `false` for "we have no
///    opinion".
pub fn parse_api_error(
    status_code: Option<u16>,
    message: &str,
    response_headers: HashMap<String, String>,
    response_body: Option<String>,
    provider_id: &str,
    default_retryable: bool,
) -> ProviderError {
    let body_mentions_overflow = response_body
        .as_deref()
        .is_some_and(|body| body.contains("context_length_exceeded"));
    if message.contains("context_length_exceeded")
        || status_code == Some(413)
        || body_mentions_overflow
    {
        return ProviderError::ContextOverflow {
            message: message.to_string(),
            response_body,
        };
    }
    let retryable = match status_code {
        Some(502) | Some(503) | Some(504) => true,
        Some(429) => true,
        Some(status) if provider_id.starts_with("openai") => is_5xx(status),
        _ => default_retryable,
    };
    ProviderError::ApiError {
        message: message.to_string(),
        status_code,
        is_retryable: retryable,
        response_headers,
        response_body,
    }
}

fn is_5xx(status: u16) -> bool {
    (500..600).contains(&status)
}

/// Tool-loop retry configuration. The defaults match the doc table.
///
/// Backoff schedule:
/// `delay = min(max_delay_ms, base_delay_ms * 2^attempt + rand(0..base_delay_ms/2))`
///
/// The randomized half-base jitter is opencode's pattern; it spreads
/// out simultaneous-failure clients without the precise variance that
/// a full-base jitter would introduce.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryPolicy {
    pub max_retries: u32,
    pub base_delay_ms: u64,
    pub max_delay_ms: u64,
    pub backoff_factor: u32,
    pub respect_retry_after: bool,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay_ms: 1_000,
            max_delay_ms: 30_000,
            backoff_factor: 2,
            respect_retry_after: true,
        }
    }
}

impl RetryPolicy {
    /// Compute the deterministic component of the delay for `attempt`
    /// (0-indexed: the first retry uses `attempt = 0`). Real callers
    /// add the doc's `rand(0..base_delay_ms/2)` jitter on top before
    /// sleeping. Capped by `max_delay_ms`.
    pub fn deterministic_delay_for_attempt(&self, attempt: u32) -> Duration {
        let factor = u64::from(self.backoff_factor).pow(attempt);
        let base = self.base_delay_ms.saturating_mul(factor);
        Duration::from_millis(base.min(self.max_delay_ms))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scenario: every documented opencode error code maps to the
    /// kind the doc table claims. A regression that flips
    /// `insufficient_quota` to `Transient` would silently retry a
    /// quota error against the user's spend.
    #[test]
    fn parse_stream_error_kind_matches_doc_mapping_table() {
        let cases: &[(&str, StreamErrorKind)] = &[
            ("context_length_exceeded", StreamErrorKind::ContextOverflow),
            ("insufficient_quota", StreamErrorKind::UserActionRequired),
            ("usage_not_included", StreamErrorKind::UserActionRequired),
            ("invalid_prompt", StreamErrorKind::BadInput),
            ("server_is_overloaded", StreamErrorKind::Transient),
            ("server_error", StreamErrorKind::Transient),
        ];
        for (code, expected) in cases {
            assert_eq!(
                parse_stream_error_kind(code),
                *expected,
                "doc mapping for `{code}` regressed"
            );
        }
        // Unrecognised codes default to `Transient` so a brand-new
        // error code from the provider does not surface as a hard
        // user-facing failure on first sight.
        assert_eq!(
            parse_stream_error_kind("totally_made_up_code"),
            StreamErrorKind::Transient
        );
    }

    /// Scenario: `is_retryable()` matches the doc rule — only
    /// `Transient` returns true. Importantly `ContextOverflow` is NOT
    /// retryable through the regular budget; the runtime takes a
    /// separate compaction path.
    #[test]
    fn stream_error_kind_is_retryable_only_for_transient() {
        assert!(StreamErrorKind::Transient.is_retryable());
        assert!(!StreamErrorKind::UserActionRequired.is_retryable());
        assert!(!StreamErrorKind::BadInput.is_retryable());
        assert!(!StreamErrorKind::ContextOverflow.is_retryable());
    }

    /// Scenario: HTTP 413, a generic message containing
    /// `context_length_exceeded`, OR a JSON body carrying the same
    /// code all classify as `ContextOverflow`. The body path is what
    /// catches providers that report context overflow as 400 with the
    /// canonical code embedded in the response payload — a regression
    /// would silently turn those into ApiError and skip compaction.
    #[test]
    fn parse_api_error_classifies_context_overflow() {
        // Pure 413.
        let err = parse_api_error(
            Some(413),
            "Request entity too large",
            HashMap::new(),
            None,
            "openai",
            false,
        );
        assert!(matches!(err, ProviderError::ContextOverflow { .. }));

        // Message-driven detection (status 400, code embedded in
        // message text).
        let err = parse_api_error(
            Some(400),
            "Bad Request: context_length_exceeded",
            HashMap::new(),
            None,
            "anthropic",
            false,
        );
        assert!(matches!(err, ProviderError::ContextOverflow { .. }));

        // Body-driven detection (status 400, generic message, but the
        // JSON body carries the canonical code). This is the path the
        // doc table calls out for body-only overflow reports.
        let err = parse_api_error(
            Some(400),
            "Bad Request",
            HashMap::new(),
            Some(r#"{"error":{"code":"context_length_exceeded"}}"#.to_string()),
            "anthropic",
            false,
        );
        assert!(
            matches!(err, ProviderError::ContextOverflow { .. }),
            "body-driven context-overflow detection regressed"
        );
    }

    /// Scenario: HTTP 5xx (502/503/504) and 429 are retryable
    /// regardless of provider. The retry policy's caller honors
    /// `Retry-After` for 429 separately.
    #[test]
    fn parse_api_error_marks_5xx_and_429_as_retryable() {
        for status in [429u16, 502, 503, 504] {
            let err = parse_api_error(
                Some(status),
                "Service unavailable",
                HashMap::new(),
                None,
                "deepseek",
                false,
            );
            match err {
                ProviderError::ApiError {
                    is_retryable: true, ..
                } => {}
                other => panic!("expected retryable ApiError for {status}, got {other:?}"),
            }
        }
    }

    /// Scenario: the OpenAI heuristic treats 5xx as retryable when
    /// the more specific 5xx branch did not already match. A 599
    /// (catch-all 5xx) under provider_id `"openai-compat"` should
    /// still be retryable.
    #[test]
    fn parse_api_error_openai_heuristic_retries_5xx() {
        let err = parse_api_error(
            Some(599),
            "Network connect timeout",
            HashMap::new(),
            None,
            "openai-compat",
            false,
        );
        match err {
            ProviderError::ApiError {
                is_retryable: true, ..
            } => {}
            other => panic!("expected retryable openai 5xx, got {other:?}"),
        }

        // A 4xx under the openai heuristic is NOT retryable.
        let err = parse_api_error(
            Some(400),
            "Bad Request",
            HashMap::new(),
            None,
            "openai",
            false,
        );
        match err {
            ProviderError::ApiError {
                is_retryable: false,
                ..
            } => {}
            other => panic!("expected non-retryable openai 4xx, got {other:?}"),
        }
    }

    /// Scenario: a non-OpenAI 4xx falls back to `default_retryable`.
    /// The caller can pass `false` for "we have no opinion" or `true`
    /// when the SDK provided its own retry hint.
    #[test]
    fn parse_api_error_falls_back_to_default_for_unmapped_status() {
        let err = parse_api_error(
            Some(418),
            "I'm a teapot",
            HashMap::new(),
            None,
            "anthropic",
            false,
        );
        assert!(matches!(
            err,
            ProviderError::ApiError {
                is_retryable: false,
                ..
            }
        ));

        let err = parse_api_error(
            Some(418),
            "I'm a teapot",
            HashMap::new(),
            None,
            "anthropic",
            true,
        );
        assert!(matches!(
            err,
            ProviderError::ApiError {
                is_retryable: true,
                ..
            }
        ));
    }

    /// Scenario: a `Retry-After: 12` header surfaces as `Some(12)`
    /// from `retry_after_seconds()`. The header lookup is
    /// case-insensitive (some servers ship `retry-after`, some
    /// `Retry-After`).
    #[test]
    fn retry_after_seconds_extracts_numeric_header() {
        let mut headers = HashMap::new();
        headers.insert("Retry-After".to_string(), "12".to_string());
        let err = ProviderError::ApiError {
            message: "rate limited".to_string(),
            status_code: Some(429),
            is_retryable: true,
            response_headers: headers,
            response_body: None,
        };
        assert_eq!(err.retry_after_seconds(), Some(12));

        // Lower-case spelling.
        let mut headers = HashMap::new();
        headers.insert("retry-after".to_string(), "  7  ".to_string());
        let err = ProviderError::ApiError {
            message: "rate limited".to_string(),
            status_code: Some(429),
            is_retryable: true,
            response_headers: headers,
            response_body: None,
        };
        assert_eq!(err.retry_after_seconds(), Some(7));
    }

    /// Scenario: a non-numeric `Retry-After` (HTTP-date) returns
    /// `None`; opencode's runtime does the date parsing at the call
    /// site. The taxonomy layer stays scope-tight.
    #[test]
    fn retry_after_seconds_is_none_for_http_date_form() {
        let mut headers = HashMap::new();
        headers.insert(
            "Retry-After".to_string(),
            "Wed, 21 Oct 2026 07:28:00 GMT".to_string(),
        );
        let err = ProviderError::ApiError {
            message: "rate limited".to_string(),
            status_code: Some(429),
            is_retryable: true,
            response_headers: headers,
            response_body: None,
        };
        assert_eq!(err.retry_after_seconds(), None);
    }

    /// Scenario: `requires_compaction()` fires on both
    /// `ContextOverflow` paths (HTTP-level and stream-level). A
    /// generic ApiError does NOT trigger compaction.
    #[test]
    fn requires_compaction_covers_both_overflow_paths() {
        let http_overflow = ProviderError::ContextOverflow {
            message: "too long".to_string(),
            response_body: None,
        };
        let stream_overflow = ProviderError::StreamError {
            kind: StreamErrorKind::ContextOverflow,
            message: "context_length_exceeded".to_string(),
            response_body: "{}".to_string(),
        };
        let api = ProviderError::ApiError {
            message: "x".to_string(),
            status_code: Some(500),
            is_retryable: true,
            response_headers: HashMap::new(),
            response_body: None,
        };
        assert!(http_overflow.requires_compaction());
        assert!(stream_overflow.requires_compaction());
        assert!(!api.requires_compaction());
    }

    /// Scenario: the default retry policy matches the doc table.
    /// A regression here is highly visible because the runtime's
    /// retry budget would silently widen or shrink.
    #[test]
    fn retry_policy_default_matches_doc_table() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.max_retries, 3);
        assert_eq!(policy.base_delay_ms, 1_000);
        assert_eq!(policy.max_delay_ms, 30_000);
        assert_eq!(policy.backoff_factor, 2);
        assert!(policy.respect_retry_after);
    }

    /// Scenario: deterministic delays double on each attempt and
    /// saturate at `max_delay_ms`.
    /// attempt 0: 1000 ms
    /// attempt 1: 2000 ms
    /// attempt 2: 4000 ms
    /// attempt 3: 8000 ms
    /// attempt 4: 16000 ms
    /// attempt 5: 30000 ms (capped from 32000)
    #[test]
    fn retry_policy_delays_double_then_saturate() {
        let policy = RetryPolicy::default();
        assert_eq!(
            policy.deterministic_delay_for_attempt(0),
            Duration::from_millis(1_000)
        );
        assert_eq!(
            policy.deterministic_delay_for_attempt(1),
            Duration::from_millis(2_000)
        );
        assert_eq!(
            policy.deterministic_delay_for_attempt(2),
            Duration::from_millis(4_000)
        );
        assert_eq!(
            policy.deterministic_delay_for_attempt(3),
            Duration::from_millis(8_000)
        );
        assert_eq!(
            policy.deterministic_delay_for_attempt(4),
            Duration::from_millis(16_000)
        );
        // Capped at max_delay_ms even when the exponential would go
        // higher.
        assert_eq!(
            policy.deterministic_delay_for_attempt(5),
            Duration::from_millis(30_000)
        );
        // Way past the cap also returns max.
        assert_eq!(
            policy.deterministic_delay_for_attempt(20),
            Duration::from_millis(30_000)
        );
    }

    /// Scenario: `is_retryable()` on `ProviderError` forwards through
    /// the underlying classification — HTTP retry flag, stream kind,
    /// or context overflow (always false).
    #[test]
    fn provider_error_is_retryable_forwards_through_variant() {
        let api_yes = ProviderError::ApiError {
            message: "x".to_string(),
            status_code: Some(503),
            is_retryable: true,
            response_headers: HashMap::new(),
            response_body: None,
        };
        let api_no = ProviderError::ApiError {
            message: "x".to_string(),
            status_code: Some(401),
            is_retryable: false,
            response_headers: HashMap::new(),
            response_body: None,
        };
        let stream_transient = ProviderError::StreamError {
            kind: StreamErrorKind::Transient,
            message: "x".to_string(),
            response_body: "{}".to_string(),
        };
        let stream_user = ProviderError::StreamError {
            kind: StreamErrorKind::UserActionRequired,
            message: "x".to_string(),
            response_body: "{}".to_string(),
        };
        let overflow = ProviderError::ContextOverflow {
            message: "x".to_string(),
            response_body: None,
        };
        assert!(api_yes.is_retryable());
        assert!(!api_no.is_retryable());
        assert!(stream_transient.is_retryable());
        assert!(!stream_user.is_retryable());
        assert!(!overflow.is_retryable());
    }
}
