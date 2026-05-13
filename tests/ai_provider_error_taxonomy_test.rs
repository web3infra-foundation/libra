//! Integration fixtures for the OC-Phase 4 provider error taxonomy.
//!
//! Spec: `docs/improvement/opencode.md` → "Provider Error Taxonomy & Retry
//! Policy" — every documented opencode error code must round-trip through
//! [`parse_stream_error_kind`] / [`parse_api_error`] / [`ProviderError`]
//! exactly as the doc table prescribes. The fixtures live in their own
//! integration crate (rather than as `error.rs` unit tests) because the
//! doc explicitly calls out a top-level test file
//! (`tests/ai_provider_error_taxonomy_test.rs`) so a downstream auditor can
//! run it via `cargo test --test ai_provider_error_taxonomy_test`
//! without pulling the whole crate test corpus.
//!
//! The expectations in this file are intentionally fixture-shaped (a
//! `&[(code, kind, retryable)]` table driving every assertion) so a
//! regression that flips any single mapping is caught with the offending
//! code printed verbatim — not buried under a generic assertion failure.

use std::collections::HashMap;

use libra::internal::ai::providers::{
    ProviderError, RetryPolicy, StreamErrorKind, parse_api_error, parse_stream_error_kind,
};

/// Doc table verbatim: every opencode error code, the Libra
/// [`StreamErrorKind`] it must classify as, and whether the runtime
/// retries on it.
///
/// The order matches `docs/improvement/opencode.md` so a doc rewrite
/// that adds / reorders codes triggers a localised test diff.
const DOC_MAPPING: &[(&str, StreamErrorKind, bool)] = &[
    (
        "context_length_exceeded",
        StreamErrorKind::ContextOverflow,
        false,
    ),
    (
        "insufficient_quota",
        StreamErrorKind::UserActionRequired,
        false,
    ),
    (
        "usage_not_included",
        StreamErrorKind::UserActionRequired,
        false,
    ),
    ("invalid_prompt", StreamErrorKind::BadInput, false),
    ("server_is_overloaded", StreamErrorKind::Transient, true),
    ("server_error", StreamErrorKind::Transient, true),
];

/// Scenario: every error code in the doc mapping table classifies as
/// the documented [`StreamErrorKind`] AND its `is_retryable` matches
/// the doc's "is_retryable" column. A regression that flips any cell
/// fails with the offending code in the assertion message.
#[test]
fn doc_mapping_table_matches_expected_classifications() {
    for (code, expected_kind, expected_retryable) in DOC_MAPPING {
        let kind = parse_stream_error_kind(code);
        assert_eq!(
            kind, *expected_kind,
            "doc mapping for `{code}` regressed (StreamErrorKind)"
        );
        assert_eq!(
            kind.is_retryable(),
            *expected_retryable,
            "doc mapping for `{code}` regressed (is_retryable)"
        );
    }
}

/// Scenario: an unrecognised code (e.g. a brand-new provider error
/// shipped without Libra knowing about it) defaults to
/// [`StreamErrorKind::Transient`]. Defaulting to retry matches
/// opencode's permissive fallback — surfacing a brand-new code as a
/// hard user-facing error on first sight would be a regression.
#[test]
fn unknown_codes_default_to_transient_retryable() {
    let kind = parse_stream_error_kind("totally_made_up_code_2099");
    assert_eq!(kind, StreamErrorKind::Transient);
    assert!(kind.is_retryable());
}

/// Scenario: HTTP-level overflow detection covers the three doc paths:
///   1. status == 413 (canonical Payload Too Large)
///   2. message contains `context_length_exceeded` (provider returns
///      generic 4xx with the code in the human message)
///   3. JSON body contains `context_length_exceeded` (provider returns
///      generic 400 with the code embedded in the structured payload)
///
/// All three paths must surface as [`ProviderError::ContextOverflow`]
/// AND set `requires_compaction()` so the tool loop takes the
/// compaction-then-retry-once branch instead of the generic backoff
/// path.
#[test]
fn parse_api_error_classifies_all_three_overflow_paths() {
    // Path 1: canonical 413.
    let err = parse_api_error(
        Some(413),
        "Request entity too large",
        HashMap::new(),
        None,
        "openai",
        false,
    );
    assert!(matches!(err, ProviderError::ContextOverflow { .. }));
    assert!(err.requires_compaction());
    assert!(!err.is_retryable());

    // Path 2: generic 400 with the code embedded in the message text.
    let err = parse_api_error(
        Some(400),
        "Bad Request: context_length_exceeded",
        HashMap::new(),
        None,
        "anthropic",
        false,
    );
    assert!(matches!(err, ProviderError::ContextOverflow { .. }));
    assert!(err.requires_compaction());

    // Path 3: status 400 + generic message + body-only code.
    let err = parse_api_error(
        Some(400),
        "Bad Request",
        HashMap::new(),
        Some(r#"{"error":{"code":"context_length_exceeded"}}"#.to_string()),
        "deepseek",
        false,
    );
    assert!(matches!(err, ProviderError::ContextOverflow { .. }));
    assert!(err.requires_compaction());
}

/// Scenario: every server-side transient classifies as a retryable
/// [`ProviderError::ApiError`] regardless of which provider id we
/// pass. The runtime backs off and retries on these.
#[test]
fn parse_api_error_marks_5xx_and_429_retryable_for_every_provider() {
    let providers = ["anthropic", "openai", "deepseek", "kimi", "zhipu", "gemini"];
    for provider in providers {
        for status in [429u16, 502, 503, 504] {
            let err = parse_api_error(
                Some(status),
                "Service unavailable",
                HashMap::new(),
                None,
                provider,
                false,
            );
            match &err {
                ProviderError::ApiError { is_retryable, .. } => {
                    assert!(
                        *is_retryable,
                        "expected retryable ApiError for provider={provider} status={status}, got {err:?}"
                    );
                }
                other => panic!(
                    "expected ApiError variant for provider={provider} status={status}, got {other:?}"
                ),
            }
        }
    }
}

/// Scenario: a 429 response carrying a numeric `Retry-After` header
/// surfaces the seconds value through `retry_after_seconds()` for both
/// header capitalisations the wild produces. The retry loop honours
/// this in preference to its own exponential backoff (per the doc's
/// `respect_retry_after = true` default).
#[test]
fn retry_after_header_surfaces_for_both_capitalisations() {
    for header_name in ["Retry-After", "retry-after", "RETRY-AFTER"] {
        let mut headers = HashMap::new();
        headers.insert(header_name.to_string(), "12".to_string());
        let err = ProviderError::ApiError {
            message: "rate limited".to_string(),
            status_code: Some(429),
            is_retryable: true,
            response_headers: headers,
            response_body: None,
        };
        assert_eq!(
            err.retry_after_seconds(),
            Some(12),
            "Retry-After parsing regressed for header={header_name}"
        );
    }
}

/// Scenario: a `Retry-After` header in HTTP-date form returns
/// `None` — opencode does the date parsing at the call site, so the
/// taxonomy layer stays tight. A regression that started returning
/// `Some(0)` for HTTP dates would silently demote `Retry-After: Wed,
/// ...` to "retry immediately".
#[test]
fn retry_after_header_returns_none_for_http_date_form() {
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

/// Scenario: every concrete `ProviderError` variant correctly
/// classifies as retryable / non-retryable per the doc table. This
/// is the one assertion a downstream tool-loop integration pulls on
/// to decide whether to back off vs. surface immediately, so a
/// regression here directly translates into wrong runtime behaviour.
#[test]
fn provider_error_is_retryable_per_variant() {
    let context_overflow = ProviderError::ContextOverflow {
        message: "input too large".to_string(),
        response_body: None,
    };
    let api_retryable = ProviderError::ApiError {
        message: "Service Unavailable".to_string(),
        status_code: Some(503),
        is_retryable: true,
        response_headers: HashMap::new(),
        response_body: None,
    };
    let api_non_retryable = ProviderError::ApiError {
        message: "Unauthorized".to_string(),
        status_code: Some(401),
        is_retryable: false,
        response_headers: HashMap::new(),
        response_body: None,
    };
    let stream_transient = ProviderError::StreamError {
        kind: StreamErrorKind::Transient,
        message: "server_is_overloaded".to_string(),
        response_body: "{}".to_string(),
    };
    let stream_user_action = ProviderError::StreamError {
        kind: StreamErrorKind::UserActionRequired,
        message: "insufficient_quota".to_string(),
        response_body: "{}".to_string(),
    };
    let stream_bad_input = ProviderError::StreamError {
        kind: StreamErrorKind::BadInput,
        message: "invalid_prompt".to_string(),
        response_body: "{}".to_string(),
    };
    let stream_overflow = ProviderError::StreamError {
        kind: StreamErrorKind::ContextOverflow,
        message: "context_length_exceeded".to_string(),
        response_body: "{}".to_string(),
    };

    // ContextOverflow path: not retryable, but requires_compaction.
    assert!(!context_overflow.is_retryable());
    assert!(context_overflow.requires_compaction());
    assert!(!stream_overflow.is_retryable());
    assert!(stream_overflow.requires_compaction());

    // Generic ApiError honours its own `is_retryable` flag, never
    // requires compaction.
    assert!(api_retryable.is_retryable());
    assert!(!api_retryable.requires_compaction());
    assert!(!api_non_retryable.is_retryable());
    assert!(!api_non_retryable.requires_compaction());

    // Stream errors classify by kind.
    assert!(stream_transient.is_retryable());
    assert!(!stream_user_action.is_retryable());
    assert!(!stream_bad_input.is_retryable());
    assert!(!stream_transient.requires_compaction());
    assert!(!stream_user_action.requires_compaction());
    assert!(!stream_bad_input.requires_compaction());
}

/// Scenario: the default [`RetryPolicy`] matches the doc table. A
/// regression here silently widens or shrinks the retry budget every
/// `tool_loop` consumer sees.
#[test]
fn retry_policy_default_matches_doc_table() {
    let policy = RetryPolicy::default();
    assert_eq!(policy.max_retries, 3);
    assert_eq!(policy.base_delay_ms, 1_000);
    assert_eq!(policy.max_delay_ms, 30_000);
    assert_eq!(policy.backoff_factor, 2);
    assert!(policy.respect_retry_after);
}

/// Scenario: the deterministic delay for each attempt doubles
/// (exponential factor 2) and saturates at `max_delay_ms`. The doc
/// formula is
/// `delay = min(max, base * 2^attempt + rand(0..base/2))`; this
/// fixture asserts the `base * 2^attempt` portion only, since the
/// jitter is non-deterministic and applied at the caller's site.
#[test]
fn retry_policy_deterministic_delays_double_and_saturate() {
    use std::time::Duration;
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
    // 2^4 = 16 → 16_000 ms (still under 30_000 cap).
    assert_eq!(
        policy.deterministic_delay_for_attempt(4),
        Duration::from_millis(16_000)
    );
    // 2^5 = 32 → 32_000 ms, capped to 30_000.
    assert_eq!(
        policy.deterministic_delay_for_attempt(5),
        Duration::from_millis(30_000)
    );
    // Pathologically large attempts also saturate at the cap.
    assert_eq!(
        policy.deterministic_delay_for_attempt(20),
        Duration::from_millis(30_000)
    );
}
