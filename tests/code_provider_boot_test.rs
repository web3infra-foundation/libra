//! Wave 10 / PR 10 — provider boot smoke (§5.2).
//!
//! Boots each AI provider's HTTP client against a tiny `axum`
//! mock server (no external mock crate; the helper lives in
//! `tests/helpers/mock_provider_server.rs`) and asserts the very
//! first `/chat/completions` request body shape, with focus on
//! provider-specific flag passthrough that the real CLI flags
//! (`--deepseek-thinking` / `--deepseek-reasoning-effort` /
//! `--deepseek-stream`) populate via `CompletionRequest`.
//!
//! Coverage included here:
//!   * **DeepSeek**: `thinking.type=enabled`,
//!     `reasoning_effort=high`, `stream=false` round-trip from
//!     `CompletionRequest` into the wire body. Pins the CLI →
//!     `CompletionRequest` → DeepSeekRequest serialisation
//!     contract.
//!   * **OpenAI-compat**: client built with custom base URL
//!     posts to `/chat/completions`; body carries `model` +
//!     non-empty `messages`. Smoke proof that the generic
//!     OpenAI client constructor + `with_base_url` form
//!     correctly route to the stub endpoint.
//!   * **Anthropic**: client built with custom base URL posts
//!     to `/v1/messages`; body carries `model`, non-empty
//!     `messages`, and the required `max_tokens` field
//!     (Anthropic-specific — the OpenAI-compat shape has no
//!     mandatory token cap). Smoke proof that the helper's
//!     `/v1/messages` route covers the second wire shape we
//!     ship.
//!   * **Kimi**: `--kimi-thinking enabled` round-trips to
//!     `thinking.type=enabled, keep=all`; `stream=false`
//!     serialises (`KimiRequest.stream` is also a non-skip
//!     boolean, mirroring DeepSeek). Pins the
//!     `--kimi-thinking` / `--kimi-stream` CLI →
//!     `CompletionRequest` → `KimiRequest` chain.
//!
//! Coverage deferred to follow-up PRs (same helper, same
//! pattern):
//!   * Ollama (`--ollama-thinking` / `--ollama-compact-tools`),
//!     Gemini, Zhipu provider boot smokes.
//!   * Missing-API-key error message + `--api-base` override
//!     surface tests.

mod helpers;

use anyhow::Result;
use helpers::mock_provider_server::MockProviderServer;
use libra::internal::ai::{
    client::CompletionClient,
    completion::{
        CompletionModel, CompletionReasoningEffort, CompletionThinking, Message,
        request::CompletionRequest,
    },
    providers::{
        anthropic::client::Client as AnthropicClient, deepseek::client::Client as DeepSeekClient,
        kimi::client::Client as KimiClient, openai::client::Client as OpenAiClient,
    },
};
use serde_json::json;

/// Anthropic Messages API responds with a different envelope
/// than OpenAI-compat — separate canned shape so the test
/// drives a real `AnthropicResponse` deserialise rather than
/// short-circuiting on a parse error.
fn canned_anthropic_response() -> serde_json::Value {
    json!({
        "id": "msg_test_completion",
        "type": "message",
        "role": "assistant",
        "content": [
            { "type": "text", "text": "ok" }
        ],
        "model": "claude-test",
        "stop_reason": "end_turn",
        "usage": {
            "input_tokens": 1,
            "output_tokens": 1
        }
    })
}

fn canned_chat_response() -> serde_json::Value {
    json!({
        "id": "test-completion",
        "object": "chat.completion",
        "created": 0,
        "model": "test-model",
        "choices": [
            {
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "ok"
                },
                "finish_reason": "stop"
            }
        ],
        "usage": {
            "prompt_tokens": 1,
            "completion_tokens": 1,
            "total_tokens": 2
        }
    })
}

/// Wave 10 §5.2 closure — DeepSeek boot smoke.
///
/// Builds a `DeepSeekClient` against a localhost stub (the same
/// constructor production uses for `--api-base` overrides), drives
/// a single `CompletionRequest` carrying `thinking=Enabled`,
/// `reasoning_effort=High`, `stream=false`, and asserts the
/// captured POST body has `thinking.type=enabled`,
/// `reasoning_effort=high`, and `stream=false`. This pins the
/// full path: CLI args → `CompletionRequest` → `DeepSeekRequest`
/// serialisation → wire body.
#[tokio::test]
async fn deepseek_completion_request_carries_thinking_and_reasoning_effort_flags() -> Result<()> {
    let server = MockProviderServer::start(canned_chat_response()).await;
    let client = DeepSeekClient::with_base_url(&server.base_url(), "test-key".to_string());
    let model = client.completion_model("deepseek-v4-flash");

    let mut request = CompletionRequest::new(vec![Message::user("hello deepseek")]);
    request.thinking = Some(CompletionThinking::Enabled);
    request.reasoning_effort = Some(CompletionReasoningEffort::High);
    request.stream = Some(false);

    let _response = model.completion(request).await?;

    let bodies = server.captured_bodies();
    assert_eq!(bodies.len(), 1, "expected exactly one POST captured");
    let body = &bodies[0];
    assert_eq!(
        body.get("model").and_then(|v| v.as_str()),
        Some("deepseek-v4-flash"),
        "model field must round-trip into the wire body; got {body:?}",
    );
    assert_eq!(
        body.pointer("/thinking/type").and_then(|v| v.as_str()),
        Some("enabled"),
        "DeepSeek thinking flag must serialise as type=enabled; got {body:?}",
    );
    assert_eq!(
        body.get("reasoning_effort").and_then(|v| v.as_str()),
        Some("high"),
        "DeepSeek reasoning_effort=high must round-trip; got {body:?}",
    );
    assert_eq!(
        body.get("stream").and_then(|v| v.as_bool()),
        Some(false),
        "DeepSeek stream field must serialise (DeepSeekRequest.stream is NOT skip_serializing_if); got {body:?}",
    );
    Ok(())
}

/// Wave 10 §5.2 closure — OpenAI-compat boot smoke.
///
/// Proves the generic `OpenAiClient::with_base_url` constructor
/// reaches a localhost endpoint over HTTP and serialises a
/// minimal `CompletionRequest` into a recognisable
/// `/chat/completions` body. The smoke is intentionally
/// lightweight: no flag passthrough, just confirmation that
/// `--api-base`-style overrides hit the configured host.
#[tokio::test]
async fn openai_completion_request_smoke_boots_against_localhost_stub() -> Result<()> {
    let server = MockProviderServer::start(canned_chat_response()).await;
    let client = OpenAiClient::with_base_url(&server.base_url(), "test-key".to_string());
    let model = client.completion_model("gpt-4o-mini");

    let request = CompletionRequest::new(vec![Message::user("hello openai")]);
    let _response = model.completion(request).await?;

    let bodies = server.captured_bodies();
    assert_eq!(bodies.len(), 1, "expected exactly one POST captured");
    let body = &bodies[0];
    assert_eq!(
        body.get("model").and_then(|v| v.as_str()),
        Some("gpt-4o-mini"),
        "model field must round-trip into the wire body; got {body:?}",
    );
    assert!(
        body.get("messages")
            .and_then(|v| v.as_array())
            .is_some_and(|arr| !arr.is_empty()),
        "OpenAI request must carry non-empty messages array; got {body:?}",
    );
    Ok(())
}

/// Wave 10 §5.2 closure — Kimi boot smoke + thinking/stream
/// flag passthrough.
///
/// Kimi's wire shape is OpenAI-compat (`POST /chat/completions`)
/// but adds a Moonshot-specific `thinking` object with both
/// `type` and `keep` discriminants. When `--kimi-thinking enabled`
/// is set on the CLI the `CompletionRequest.thinking` field
/// becomes `Some(CompletionThinking::Enabled)`; this test pins
/// that the resulting wire body has `thinking.type=enabled`
/// AND `thinking.keep=all` (the default for any non-Disabled
/// value), plus the always-emitted `stream` boolean.
#[tokio::test]
async fn kimi_completion_request_carries_thinking_and_stream_flags() -> Result<()> {
    let server = MockProviderServer::start(canned_chat_response()).await;
    let client = KimiClient::with_base_url(&server.base_url(), "test-key".to_string());
    let model = client.completion_model("kimi-k2-thinking");

    let mut request = CompletionRequest::new(vec![Message::user("hello kimi")]);
    request.thinking = Some(CompletionThinking::Enabled);
    request.stream = Some(false);

    let _response = model.completion(request).await?;

    let bodies = server.captured_bodies();
    assert_eq!(bodies.len(), 1, "expected exactly one POST captured");
    let body = &bodies[0];
    assert_eq!(
        body.get("model").and_then(|v| v.as_str()),
        Some("kimi-k2-thinking"),
        "Kimi model field must round-trip; got {body:?}",
    );
    assert_eq!(
        body.pointer("/thinking/type").and_then(|v| v.as_str()),
        Some("enabled"),
        "Kimi thinking flag must serialise as type=enabled; got {body:?}",
    );
    assert_eq!(
        body.pointer("/thinking/keep").and_then(|v| v.as_str()),
        Some("all"),
        "Kimi thinking-enabled requires keep=all (preserves prior reasoning); got {body:?}",
    );
    assert_eq!(
        body.get("stream").and_then(|v| v.as_bool()),
        Some(false),
        "Kimi stream field must serialise (KimiRequest.stream is NOT skip_serializing_if); got {body:?}",
    );
    Ok(())
}

/// Wave 10 §5.2 closure — Anthropic boot smoke against the
/// helper's `/v1/messages` route. Anthropic's request shape is
/// distinct from OpenAI-compat (top-level `system`,
/// mandatory `max_tokens`), so this test proves the second wire
/// shape we ship is reachable through the same mock helper.
#[tokio::test]
async fn anthropic_completion_request_smoke_boots_against_localhost_stub() -> Result<()> {
    let server = MockProviderServer::start(canned_anthropic_response()).await;
    let client = AnthropicClient::with_base_url(&server.base_url(), "test-key".to_string());
    let model = client.completion_model("claude-3-5-sonnet-20241022");

    let request = CompletionRequest::new(vec![Message::user("hello anthropic")]);
    let _response = model.completion(request).await?;

    let bodies = server.captured_bodies();
    assert_eq!(bodies.len(), 1, "expected exactly one POST captured");
    let body = &bodies[0];
    assert_eq!(
        body.get("model").and_then(|v| v.as_str()),
        Some("claude-3-5-sonnet-20241022"),
        "Anthropic model field must round-trip; got {body:?}",
    );
    assert!(
        body.get("messages")
            .and_then(|v| v.as_array())
            .is_some_and(|arr| !arr.is_empty()),
        "Anthropic request must carry non-empty messages array; got {body:?}",
    );
    assert!(
        body.get("max_tokens").and_then(|v| v.as_u64()).is_some(),
        "Anthropic request must carry max_tokens (mandatory in /v1/messages); got {body:?}",
    );
    Ok(())
}
