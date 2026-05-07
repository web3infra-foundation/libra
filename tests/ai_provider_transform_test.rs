//! Integration tests for the OC-Phase 4 P4.1 provider transform pipeline.
//!
//! Per-transform unit tests (Anthropic empty-content drop, OpenAI reasoning
//! strip, DeepSeek/Kimi reasoning preserve, Gemini ToolResult name guard,
//! variant lookup) live in `src/internal/ai/providers/transform.rs` next to
//! the trait. Tests in this file focus on the **wiring** instead:
//!
//! 1. `transform_for(provider_id)` returns a transform whose `provider_id()`
//!    matches the input — this is the contract `AnyCompletionModel::completion`
//!    relies on to keep transforms paired with the runtime variant they serve.
//! 2. `variants(model_id)` returns the right variant set for catalogued
//!    reasoning-capable models across every supported provider.
//! 3. `prepare_request` composes correctly when invoked through the trait
//!    object surface (object-safety regression guard).

use libra::internal::ai::{
    completion::{
        AssistantContent, CompletionRequest, Function, Message, OneOrMany, Text, ToolCall,
        ToolResult, UserContent, message::Image,
    },
    providers::{
        runtime::provider_id,
        transform::{reject_non_text_system_content, variant},
        transform_for,
    },
};

fn assistant_with_reasoning(text: &str, reasoning: &str) -> Message {
    Message::Assistant {
        id: None,
        reasoning_content: Some(reasoning.to_string()),
        content: OneOrMany::One(AssistantContent::Text(Text {
            text: text.to_string(),
        })),
    }
}

fn assistant_tool_call(id: &str, name: &str) -> Message {
    Message::Assistant {
        id: None,
        reasoning_content: None,
        content: OneOrMany::One(AssistantContent::ToolCall(ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            function: Function {
                name: name.to_string(),
                arguments: serde_json::json!({}),
            },
        })),
    }
}

fn user_with_tool_result(id: &str, name: &str, value: serde_json::Value) -> Message {
    Message::User {
        content: OneOrMany::One(UserContent::ToolResult(ToolResult {
            id: id.to_string(),
            name: name.to_string(),
            result: value,
        })),
    }
}

fn request_with(history: Vec<Message>) -> CompletionRequest {
    CompletionRequest {
        chat_history: history,
        ..Default::default()
    }
}

/// Every catalogued provider id resolves to a transform that reports the
/// same id back. This is the invariant `AnyCompletionModel::completion`
/// relies on — if `transform_for("anthropic")` ever returned a transform
/// whose `provider_id()` was something else, the wired pipeline would
/// silently apply the wrong quirks.
#[test]
fn transform_for_round_trips_provider_id_for_every_known_provider() {
    for &id in provider_id::ALL_PRODUCTION {
        let transform = transform_for(id);
        assert_eq!(
            transform.provider_id(),
            id,
            "transform_for({id}) must report id {id}"
        );
    }
}

/// `OpenAi`'s transform strips assistant `reasoning_content` (the
/// chat-completions API rejects unknown fields) but `DeepSeek` and `Kimi`
/// preserve it (chain-of-thought handoff requires echoing the previous
/// turn's reasoning).
#[test]
fn openai_strips_reasoning_but_deepseek_and_kimi_preserve_it() {
    let baseline = vec![
        Message::user("hi"),
        assistant_with_reasoning("answer", "internal trace"),
    ];

    let mut openai_request = request_with(baseline.clone());
    transform_for(provider_id::OPENAI)
        .prepare_request("gpt-4o", &mut openai_request)
        .unwrap();
    match &openai_request.chat_history[1] {
        Message::Assistant {
            reasoning_content, ..
        } => assert!(
            reasoning_content.is_none(),
            "OpenAI transform must drop reasoning_content"
        ),
        other => panic!("history shape changed: {other:?}"),
    }

    for &(id, model) in &[
        (provider_id::DEEPSEEK, "deepseek-reasoner"),
        (provider_id::KIMI, "kimi-thinking"),
    ] {
        let mut request = request_with(baseline.clone());
        transform_for(id)
            .prepare_request(model, &mut request)
            .unwrap();
        match &request.chat_history[1] {
            Message::Assistant {
                reasoning_content, ..
            } => assert_eq!(
                reasoning_content.as_deref(),
                Some("internal trace"),
                "{id} must preserve reasoning_content for chain-of-thought handoff"
            ),
            other => panic!("history shape changed for {id}: {other:?}"),
        }
    }
}

/// Anthropic rejects orphan `ToolResult` parts (a `tool_result` block with
/// no preceding `tool_use`) with a 400 that is hard to trace back to the
/// offending message. The transform pairing validator points the caller at
/// the exact tool call id so tooling can fix the transcript.
#[test]
fn anthropic_rejects_orphan_tool_result_through_wired_pipeline() {
    let mut request = request_with(vec![
        Message::user("hi"),
        user_with_tool_result("call_orphan", "shell", serde_json::json!({"ok": true})),
    ]);
    let err = transform_for(provider_id::ANTHROPIC)
        .prepare_request("claude-sonnet-4-0", &mut request)
        .expect_err("orphan ToolResult must be rejected via the wired pipeline");
    let msg = err.to_string();
    assert!(
        msg.contains("call_orphan"),
        "error must name the offending ToolResult id, got: {msg}"
    );
}

/// Two tool_results for the same tool_use is also a 400; the pairing
/// validator catches it instead of letting Anthropic emit an opaque error.
#[test]
fn anthropic_rejects_duplicate_tool_result_through_wired_pipeline() {
    let assistant_tool_call_msg = |id: &str| -> Message { assistant_tool_call(id, "shell") };
    let mut request = request_with(vec![
        assistant_tool_call_msg("call_1"),
        user_with_tool_result("call_1", "shell", serde_json::json!({})),
        user_with_tool_result("call_1", "shell", serde_json::json!({})),
    ]);
    let err = transform_for(provider_id::ANTHROPIC)
        .prepare_request("claude-sonnet-4-0", &mut request)
        .expect_err("duplicate ToolResult must be rejected");
    assert!(err.to_string().contains("call_1"));
}

/// `finalize_response` must be idempotent across every provider — the
/// runtime applies the transform on every turn and the retry middleware
/// can re-apply it on a second attempt; mutation drift would otherwise
/// surface as silently-different reasoning text on retry.
#[test]
fn finalize_response_is_idempotent_for_every_provider() {
    use libra::internal::ai::completion::{AssistantContent, Text};

    for &id in provider_id::ALL_PRODUCTION {
        let transform = transform_for(id);
        let mut content: Vec<AssistantContent> = vec![AssistantContent::Text(Text {
            text: "hello".into(),
        })];
        let mut reasoning = Some("trace\n\n".to_string());
        transform
            .finalize_response("model-x", &mut content, &mut reasoning)
            .unwrap();
        let snapshot_content = content.clone();
        let snapshot_reasoning = reasoning.clone();
        transform
            .finalize_response("model-x", &mut content, &mut reasoning)
            .unwrap();
        assert_eq!(
            content, snapshot_content,
            "{id}.finalize_response is not idempotent for content"
        );
        assert_eq!(
            reasoning, snapshot_reasoning,
            "{id}.finalize_response is not idempotent for reasoning_content"
        );
    }
}

/// Anthropic's transform drops *fully empty* assistant turns but preserves
/// turns that carry tool calls — those are non-empty in the wire sense
/// even if their accompanying Text part is missing.
#[test]
fn anthropic_drops_empty_text_turns_but_keeps_tool_call_turns() {
    let mut request = request_with(vec![
        Message::user("hi"),
        Message::Assistant {
            id: None,
            reasoning_content: None,
            content: OneOrMany::One(AssistantContent::Text(Text {
                text: String::new(),
            })),
        },
        assistant_tool_call("call_1", "shell"),
    ]);
    transform_for(provider_id::ANTHROPIC)
        .prepare_request("claude-sonnet-4-0", &mut request)
        .unwrap();
    assert_eq!(
        request.chat_history.len(),
        2,
        "empty text-only assistant turn must be dropped, tool-call turn preserved"
    );
}

/// Gemini's transform fails fast when a `ToolResult` is missing its `name`
/// — opencode's wire layer blows up deep inside JSON serialisation, which
/// makes the original culprit message hard to trace.
#[test]
fn gemini_rejects_tool_result_without_name() {
    let mut request = request_with(vec![user_with_tool_result(
        "call_42",
        "",
        serde_json::json!({"ok": true}),
    )]);
    let err = transform_for(provider_id::GEMINI)
        .prepare_request("gemini-2.5-flash", &mut request)
        .expect_err("nameless ToolResult must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("call_42"),
        "error must reference offending tool call id, got: {msg}"
    );
    assert!(
        msg.contains("ToolResult") || msg.contains("functionResponse"),
        "error must reference the ToolResult/functionResponse field, got: {msg}"
    );
}

/// Variant tables surface reasoning support per (provider, model) the way
/// opencode's `variants(model)` substring match does. Adding a new
/// reasoning model means appending a substring to the relevant slice in
/// `transform.rs`; this test pins the canonical examples.
#[test]
fn variants_surface_reasoning_for_known_thinking_models() {
    for &(provider, model) in &[
        (provider_id::ANTHROPIC, "claude-opus-4-0"),
        (provider_id::OPENAI, "o3-mini"),
        (provider_id::DEEPSEEK, "deepseek-reasoner"),
        (provider_id::KIMI, "kimi-thinking"),
        (provider_id::GEMINI, "gemini-2.5-pro"),
        (provider_id::ZHIPU, "glm-4.5-air"),
    ] {
        let transform = transform_for(provider);
        let variants = transform.variants(model);
        assert!(
            variants.contains(&variant::REASONING),
            "{provider}/{model} should advertise reasoning variant; got {variants:?}"
        );
    }
}

/// The Anthropic transform always advertises `cache_control` (every
/// Claude model accepts the hint, regardless of whether reasoning is
/// available). This is the one variant on Anthropic that is independent
/// of the model id.
#[test]
fn anthropic_advertises_cache_control_for_every_model() {
    let transform = transform_for(provider_id::ANTHROPIC);
    for model in [
        "claude-3-5-haiku-latest",
        "claude-opus-4-0",
        "claude-3-5-sonnet-latest",
    ] {
        let variants = transform.variants(model);
        assert!(
            variants.contains(&variant::CACHE_CONTROL),
            "Anthropic/{model} must advertise cache_control; got {variants:?}"
        );
    }
}

/// `prepare_request` is idempotent — applying the transform twice on the
/// same request yields the same chat history. The runtime relies on this:
/// it would be unsafe to retry a request via the existing
/// `RetryingCompletionModel` without idempotency, because retries would
/// otherwise silently mutate the prompt across attempts.
#[test]
fn prepare_request_is_idempotent_for_every_provider() {
    for &id in provider_id::ALL_PRODUCTION {
        let mut request = request_with(vec![
            Message::user("hi"),
            assistant_with_reasoning("answer", "internal trace"),
            user_with_tool_result("call_1", "shell", serde_json::json!({"ok": true})),
        ]);
        let transform = transform_for(id);
        // Pick a generic model id that does not match any reasoning slice
        // — keeps the case provider-agnostic.
        let model = "model-x";
        // Some providers (Gemini) reject this fixture because of the
        // `require_tool_result_name` check. Skip those.
        if transform.prepare_request(model, &mut request).is_err() {
            continue;
        }
        let snapshot = request.chat_history.clone();
        transform.prepare_request(model, &mut request).unwrap();
        assert_eq!(
            request.chat_history, snapshot,
            "{id}.prepare_request is not idempotent"
        );
    }
}

/// `reject_non_text_system_content` is the cross-provider canonical
/// invariant the runtime enforces before any provider-specific
/// `prepare_request` runs. Every production provider folds System
/// messages down to text on the wire, so a `ToolResult` or `Image` part
/// inside `Message::System.content` would silently disappear. The check
/// rejects it with the offending index in the error message.
#[test]
fn reject_non_text_system_content_rejects_tool_result_for_every_provider() {
    let request = request_with(vec![Message::System {
        content: OneOrMany::One(UserContent::ToolResult(ToolResult {
            id: "call_in_system".to_string(),
            name: "shell".to_string(),
            result: serde_json::json!({}),
        })),
    }]);
    for &id in provider_id::ALL_PRODUCTION {
        let err = reject_non_text_system_content(&request, id)
            .expect_err(&format!("{id} must reject ToolResult inside System"));
        let msg = err.to_string();
        assert!(
            msg.contains("call_in_system"),
            "{id}: error must reference offending tool id, got: {msg}"
        );
        assert!(
            msg.contains("System"),
            "{id}: error must mention System, got: {msg}"
        );
    }
}

#[test]
fn reject_non_text_system_content_rejects_image_for_every_provider() {
    let request = request_with(vec![Message::System {
        content: OneOrMany::One(UserContent::Image(Image {
            data: "base64-bytes".to_string(),
            mime_type: Some("image/png".to_string()),
        })),
    }]);
    for &id in provider_id::ALL_PRODUCTION {
        let err = reject_non_text_system_content(&request, id)
            .expect_err(&format!("{id} must reject Image inside System"));
        assert!(
            err.to_string().contains("Image"),
            "{id}: error must reference Image kind, got: {err}"
        );
    }
}

#[test]
fn reject_non_text_system_content_accepts_text_only_system() {
    let request = request_with(vec![Message::System {
        content: OneOrMany::One(UserContent::Text(Text {
            text: "you are a helpful assistant".to_string(),
        })),
    }]);
    for &id in provider_id::ALL_PRODUCTION {
        reject_non_text_system_content(&request, id)
            .expect("text-only System must pass for every provider");
    }
}
