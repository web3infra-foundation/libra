//! Provider runtime adapter — `AnyCompletionModel` and `AnyCompletionRawResponse`.
//!
//! This module is the OC-Phase 1 P1.1 deliverable from
//! `docs/development/commands/_general.md`. It lets the runtime carry **any** provider's
//! completion model through the same generic call sites without resorting to
//! `Box<dyn CompletionModel>` — the trait is **not** object-safe (returns
//! position `impl Future`, has a `Clone` bound) so a trait object simply does
//! not compile. An enum adapter sidesteps the issue by giving the compiler a
//! closed set of variants to dispatch through.
//!
//! What this module is:
//! - A pair of wrapping enums covering every supported provider.
//! - A [`CompletionModel`] implementation that forwards `completion()` and
//!   `set_run_id()` via match dispatch.
//! - A [`CompletionUsage`] implementation on the response wrapper that forwards
//!   `usage_summary()` so `run_tool_loop` can keep its `M::Response:
//!   CompletionUsage` bound satisfied without a generic per provider.
//!
//! What this module is **not**:
//! - It does **not** build provider clients (OC-Phase 1 P1.2 introduces
//!   `ProviderFactory`).
//! - It does **not** know about [`crate::internal::ai::agent::profile::spec::ModelBinding`]
//!   string parsing — the factory will glue those layers together.
//! - It does **not** wire into [`crate::command::code`] yet (OC-Phase 1 P1.3
//!   migrates the main TUI path).
//!
//! Adding a new provider requires:
//! 1. Add a variant to [`AnyCompletionModel`] (the model struct) and to
//!    [`AnyCompletionRawResponse`] (the response struct).
//! 2. Extend the four `match` blocks below (`provider_id`, `set_run_id`,
//!    `completion`, `usage_summary`).
//! 3. Add a unit test that proves the response variant forwards
//!    `usage_summary` correctly when the inner provider returns a non-empty
//!    summary.

use crate::internal::ai::{
    completion::{
        CompletionError, CompletionModel, CompletionRequest, CompletionResponse, CompletionUsage,
        CompletionUsageSummary,
    },
    providers::{
        anthropic::{self, completion::AnthropicResponse},
        deepseek,
        gemini::{self, gemini_api_types::GenerateContentResponse},
        kimi, ollama, openai,
        openai_compat::ChatResponse,
        transform::transform_for,
        zhipu,
    },
};

/// Stable provider identifier strings used by the factory, usage logs, and
/// error messages. Keep in sync with [`AnyCompletionModel::provider_id`] and
/// [`AnyCompletionRawResponse::provider_id`] — every variant must appear here.
pub mod provider_id {
    pub const ANTHROPIC: &str = "anthropic";
    pub const OPENAI: &str = "openai";
    pub const DEEPSEEK: &str = "deepseek";
    pub const GEMINI: &str = "gemini";
    pub const KIMI: &str = "kimi";
    pub const ZHIPU: &str = "zhipu";
    pub const OLLAMA: &str = "ollama";
    #[cfg(feature = "test-provider")]
    pub const FAKE: &str = "fake";

    /// All production provider ids (no `fake`). Useful for error suggestions
    /// when a user-supplied provider is unknown.
    pub const ALL_PRODUCTION: &[&str] = &[ANTHROPIC, OPENAI, DEEPSEEK, GEMINI, KIMI, ZHIPU, OLLAMA];
}

/// Enum adapter wrapping any one of Libra's concrete completion model types.
///
/// Every variant carries the provider's `Model` (or, for Gemini and the test
/// provider, `CompletionModel`) struct, which implements
/// [`CompletionModel`] in its own right. Forwarding via `match` here gives the
/// caller a single `M: CompletionModel` type that still pays the same
/// monomorphisation cost as the underlying provider — there is no virtual
/// dispatch.
///
/// The enum is `Clone + Debug` so it composes with `tokio::spawn` patterns
/// that move a model handle into a task. Each inner provider type is itself
/// `Clone + Send + Sync`, so the enum inherits those bounds for free.
#[derive(Clone, Debug)]
pub enum AnyCompletionModel {
    Anthropic(anthropic::completion::Model),
    OpenAi(openai::completion::Model),
    DeepSeek(deepseek::completion::Model),
    Gemini(gemini::completion::CompletionModel),
    Kimi(kimi::completion::Model),
    Zhipu(zhipu::completion::Model),
    Ollama(ollama::completion::Model),
    #[cfg(feature = "test-provider")]
    Fake(super::fake::CompletionModel),
}

impl AnyCompletionModel {
    /// Stable provider id this model came from (for usage tagging and error
    /// surfaces). Matches the values in [`mod@provider_id`].
    pub fn provider_id(&self) -> &'static str {
        match self {
            Self::Anthropic(_) => provider_id::ANTHROPIC,
            Self::OpenAi(_) => provider_id::OPENAI,
            Self::DeepSeek(_) => provider_id::DEEPSEEK,
            Self::Gemini(_) => provider_id::GEMINI,
            Self::Kimi(_) => provider_id::KIMI,
            Self::Zhipu(_) => provider_id::ZHIPU,
            Self::Ollama(_) => provider_id::OLLAMA,
            #[cfg(feature = "test-provider")]
            Self::Fake(_) => provider_id::FAKE,
        }
    }

    /// Model id this enum variant currently wraps. Used by the transform
    /// pipeline to pick the right per-model variants and by usage tagging
    /// when a request fails before the response carries a model field. The
    /// value is the same string the caller supplied at construction time
    /// (e.g. `"claude-sonnet-4-0"`, `"gpt-4o-mini"`).
    pub fn model_id(&self) -> &str {
        match self {
            Self::Anthropic(m) => m.model_name(),
            Self::OpenAi(m) => m.model_name(),
            Self::DeepSeek(m) => m.model_name(),
            Self::Gemini(m) => m.model_name(),
            Self::Kimi(m) => m.model_name(),
            Self::Zhipu(m) => m.model_name(),
            Self::Ollama(m) => m.model_name(),
            #[cfg(feature = "test-provider")]
            Self::Fake(m) => m.model_name(),
        }
    }
}

/// Enum adapter wrapping any one of Libra's concrete provider response types.
///
/// Several providers (OpenAI, DeepSeek, Kimi, Zhipu, Ollama) share the same
/// [`ChatResponse`] payload because they all speak the OpenAI-compatible
/// `/chat/completions` shape. The variants are still kept distinct so the
/// caller can branch on provider identity (e.g. for usage cost lookups) even
/// when the wire payload looks identical.
#[derive(Debug)]
pub enum AnyCompletionRawResponse {
    Anthropic(AnthropicResponse),
    OpenAi(ChatResponse),
    DeepSeek(ChatResponse),
    Gemini(GenerateContentResponse),
    Kimi(ChatResponse),
    Zhipu(ChatResponse),
    Ollama(ChatResponse),
    #[cfg(feature = "test-provider")]
    Fake(super::fake::FakeRawResponse),
}

impl AnyCompletionRawResponse {
    /// Stable provider id this response variant carries — `Self::OpenAi(_)`
    /// always returns `"openai"`, etc. When the response comes back through
    /// [`AnyCompletionModel::completion`] the model and response provider ids
    /// are paired by construction; manual variant construction is the only
    /// way the two can disagree.
    pub fn provider_id(&self) -> &'static str {
        match self {
            Self::Anthropic(_) => provider_id::ANTHROPIC,
            Self::OpenAi(_) => provider_id::OPENAI,
            Self::DeepSeek(_) => provider_id::DEEPSEEK,
            Self::Gemini(_) => provider_id::GEMINI,
            Self::Kimi(_) => provider_id::KIMI,
            Self::Zhipu(_) => provider_id::ZHIPU,
            Self::Ollama(_) => provider_id::OLLAMA,
            #[cfg(feature = "test-provider")]
            Self::Fake(_) => provider_id::FAKE,
        }
    }
}

impl CompletionUsage for AnyCompletionRawResponse {
    fn usage_summary(&self) -> Option<CompletionUsageSummary> {
        match self {
            Self::Anthropic(r) => r.usage_summary(),
            Self::OpenAi(r) => r.usage_summary(),
            Self::DeepSeek(r) => r.usage_summary(),
            Self::Gemini(r) => r.usage_summary(),
            Self::Kimi(r) => r.usage_summary(),
            Self::Zhipu(r) => r.usage_summary(),
            Self::Ollama(r) => r.usage_summary(),
            #[cfg(feature = "test-provider")]
            Self::Fake(r) => r.usage_summary(),
        }
    }
}

impl CompletionModel for AnyCompletionModel {
    type Response = AnyCompletionRawResponse;

    async fn completion(
        &self,
        request: CompletionRequest,
    ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
        // OC-Phase 4 P4.1: apply provider transform before/after the wire
        // round-trip so per-provider quirks live in one auditable place
        // instead of bleeding into every provider's `completion.rs`. The
        // call is **outside** the per-variant `match` so a future provider
        // does not silently bypass the transform pipeline by forgetting an
        // arm — adding a new variant only requires extending the match
        // below, while transform wiring stays untouched.
        //
        // Cross-provider canonical-invariant checks
        // (`reject_non_text_system_content`) run *before* the provider's
        // own `prepare_request` so a contract violation that every
        // provider would silently truncate on the wire fails loud and
        // early with the offending message index. Provider-specific
        // checks (e.g. Anthropic tool_use/tool_result pairing) layer on
        // top inside `prepare_request`.
        let transform = transform_for(self.provider_id());
        let model_id = self.model_id().to_string();
        let mut request = request;
        super::transform::reject_non_text_system_content(&request, self.provider_id())?;
        transform.prepare_request(&model_id, &mut request)?;
        let mut response = match self {
            Self::Anthropic(m) => {
                let resp = m.completion(request).await?;
                wrap_response(resp, AnyCompletionRawResponse::Anthropic)
            }
            Self::OpenAi(m) => {
                let resp = m.completion(request).await?;
                wrap_response(resp, AnyCompletionRawResponse::OpenAi)
            }
            Self::DeepSeek(m) => {
                let resp = m.completion(request).await?;
                wrap_response(resp, AnyCompletionRawResponse::DeepSeek)
            }
            Self::Gemini(m) => {
                let resp = m.completion(request).await?;
                wrap_response(resp, AnyCompletionRawResponse::Gemini)
            }
            Self::Kimi(m) => {
                let resp = m.completion(request).await?;
                wrap_response(resp, AnyCompletionRawResponse::Kimi)
            }
            Self::Zhipu(m) => {
                let resp = m.completion(request).await?;
                wrap_response(resp, AnyCompletionRawResponse::Zhipu)
            }
            Self::Ollama(m) => {
                let resp = m.completion(request).await?;
                wrap_response(resp, AnyCompletionRawResponse::Ollama)
            }
            #[cfg(feature = "test-provider")]
            Self::Fake(m) => {
                let resp = m.completion(request).await?;
                wrap_response(resp, AnyCompletionRawResponse::Fake)
            }
        };
        transform.finalize_response(
            &model_id,
            &mut response.content,
            &mut response.reasoning_content,
        )?;
        Ok(response)
    }

    fn set_run_id(&self, run_id: String) {
        // Forward to every provider so a future provider that adds a real
        // `set_run_id` will be wired up by adding a new match arm — the
        // default no-op blanket impl is harmless to call on the others today.
        match self {
            Self::Anthropic(m) => m.set_run_id(run_id),
            Self::OpenAi(m) => m.set_run_id(run_id),
            Self::DeepSeek(m) => m.set_run_id(run_id),
            Self::Gemini(m) => m.set_run_id(run_id),
            Self::Kimi(m) => m.set_run_id(run_id),
            Self::Zhipu(m) => m.set_run_id(run_id),
            Self::Ollama(m) => m.set_run_id(run_id),
            #[cfg(feature = "test-provider")]
            Self::Fake(m) => m.set_run_id(run_id),
        }
    }
}

/// Lifts a provider-specific `CompletionResponse<T>` into the generic
/// `CompletionResponse<AnyCompletionRawResponse>` by wrapping `raw_response`
/// with the supplied variant constructor.
///
/// The helper exists only to keep the `match` arms in [`AnyCompletionModel::completion`]
/// concise and to centralize the field-by-field copy so a future field addition
/// to [`CompletionResponse`] gets caught by a single compile error.
fn wrap_response<T>(
    response: CompletionResponse<T>,
    wrap: fn(T) -> AnyCompletionRawResponse,
) -> CompletionResponse<AnyCompletionRawResponse> {
    CompletionResponse {
        content: response.content,
        reasoning_content: response.reasoning_content,
        raw_response: wrap(response.raw_response),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::providers::openai_compat::{
        ChatChoice, ChatCompletionTokensDetails, ChatMessage, ChatPromptTokensDetails, ChatUsage,
    };

    /// Build an OpenAI-compatible `ChatResponse` with a populated `usage`
    /// block. Used to assert that every shared-shape variant
    /// (`OpenAi` / `DeepSeek` / `Kimi` / `Zhipu` / `Ollama`) forwards
    /// `usage_summary()` into the same `CompletionUsageSummary`.
    fn chat_response_with_usage() -> ChatResponse {
        ChatResponse {
            id: "id".to_string(),
            object: "chat.completion".to_string(),
            created: 0,
            model: "fake".to_string(),
            choices: vec![ChatChoice {
                index: 0,
                message: ChatMessage::Assistant {
                    content: Some("ok".to_string()),
                    reasoning_content: None,
                    tool_calls: Vec::new(),
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: Some(ChatUsage {
                prompt_tokens: 12,
                completion_tokens: 7,
                total_tokens: 19,
                prompt_tokens_details: Some(ChatPromptTokensDetails {
                    cached_tokens: Some(3),
                }),
                completion_tokens_details: Some(ChatCompletionTokensDetails {
                    reasoning_tokens: Some(2),
                }),
            }),
        }
    }

    /// Scenario: every OpenAI-compatible variant of `AnyCompletionRawResponse`
    /// forwards `usage_summary()` to the inner `ChatResponse` impl. Differences
    /// between OpenAI, DeepSeek, Kimi, Zhipu, and Ollama only matter for
    /// outbound request shaping; the inbound usage projection must be
    /// identical.
    #[test]
    fn usage_summary_forwards_for_openai_compatible_variants() {
        let cases: Vec<AnyCompletionRawResponse> = vec![
            AnyCompletionRawResponse::OpenAi(chat_response_with_usage()),
            AnyCompletionRawResponse::DeepSeek(chat_response_with_usage()),
            AnyCompletionRawResponse::Kimi(chat_response_with_usage()),
            AnyCompletionRawResponse::Zhipu(chat_response_with_usage()),
            AnyCompletionRawResponse::Ollama(chat_response_with_usage()),
        ];
        for case in &cases {
            let summary = case.usage_summary().expect("usage summary expected");
            assert_eq!(summary.input_tokens, 12);
            assert_eq!(summary.output_tokens, 7);
            assert_eq!(summary.total_tokens, Some(19));
            assert_eq!(summary.cached_tokens, Some(3));
            assert_eq!(summary.reasoning_tokens, Some(2));
        }
    }

    /// Scenario: a `ChatResponse` whose `usage` block is `None` round-trips to
    /// `usage_summary() == None` rather than emitting a zero-valued summary
    /// that downstream usage aggregators would mistake for a free turn.
    #[test]
    fn usage_summary_returns_none_when_inner_usage_absent() {
        let mut resp = chat_response_with_usage();
        resp.usage = None;
        let wrapped = AnyCompletionRawResponse::OpenAi(resp);
        assert!(wrapped.usage_summary().is_none());
    }

    /// Scenario: every OpenAI-compatible variant reports the right provider id.
    /// This pins the symmetric pairing between [`AnyCompletionRawResponse`]
    /// and [`mod@provider_id`] for the variants that share `ChatResponse`.
    /// Anthropic and Gemini are covered by their own fixture tests below.
    #[test]
    fn response_provider_id_pairs_for_openai_compatible_variants() {
        let make = |variant: fn(ChatResponse) -> AnyCompletionRawResponse,
                    expected: &'static str| {
            let resp = variant(chat_response_with_usage());
            assert_eq!(resp.provider_id(), expected);
        };
        make(AnyCompletionRawResponse::OpenAi, provider_id::OPENAI);
        make(AnyCompletionRawResponse::DeepSeek, provider_id::DEEPSEEK);
        make(AnyCompletionRawResponse::Kimi, provider_id::KIMI);
        make(AnyCompletionRawResponse::Zhipu, provider_id::ZHIPU);
        make(AnyCompletionRawResponse::Ollama, provider_id::OLLAMA);
    }

    /// Scenario: `ALL_PRODUCTION` covers every production provider exactly
    /// once and the constant strings match the documented provider ids. A new
    /// production provider must update both lists; this test catches a
    /// half-applied addition (constant defined, list not extended).
    #[test]
    fn all_production_constant_is_complete_and_unique() {
        let mut sorted: Vec<&'static str> = provider_id::ALL_PRODUCTION.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            provider_id::ALL_PRODUCTION.len(),
            "ALL_PRODUCTION contains a duplicate"
        );
        for expected in [
            provider_id::ANTHROPIC,
            provider_id::OPENAI,
            provider_id::DEEPSEEK,
            provider_id::GEMINI,
            provider_id::KIMI,
            provider_id::ZHIPU,
            provider_id::OLLAMA,
        ] {
            assert!(
                provider_id::ALL_PRODUCTION.contains(&expected),
                "ALL_PRODUCTION missing {expected}"
            );
        }
    }

    /// Scenario (test-provider only): the `Fake` variant forwards
    /// `usage_summary()` faithfully (the fake provider returns `None`) and is
    /// reported as `provider_id::FAKE`.
    #[cfg(feature = "test-provider")]
    #[test]
    fn fake_variant_reports_none_usage_and_fake_provider_id() {
        use super::super::fake::FakeRawResponse;
        let resp = AnyCompletionRawResponse::Fake(FakeRawResponse {
            model: "fake".to_string(),
            matched_response_index: Some(0),
        });
        assert!(resp.usage_summary().is_none());
        assert_eq!(resp.provider_id(), provider_id::FAKE);
    }

    /// Scenario: the `Anthropic` variant forwards `usage_summary()` to the
    /// inner `AnthropicResponse::usage_summary()` impl. `AnthropicResponse`
    /// has private fields, so we deserialize a wire-shape JSON fixture that
    /// matches the real `POST /v1/messages` response. This pins the variant
    /// arm against silent regression: a future rename of an Anthropic field
    /// would surface here as a deserialize error or a usage mismatch.
    #[test]
    fn usage_summary_forwards_for_anthropic_variant() {
        let payload = serde_json::json!({
            "id": "msg_01",
            "type": "message",
            "role": "assistant",
            "content": [{"type": "text", "text": "ok"}],
            "model": "claude-3-5-sonnet-20241022",
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 12,
                "output_tokens": 7,
                "cache_read_input_tokens": 3,
                "cache_creation_input_tokens": 1
            }
        });
        let inner: AnthropicResponse =
            serde_json::from_value(payload).expect("anthropic fixture must deserialize");
        let wrapped = AnyCompletionRawResponse::Anthropic(inner);
        assert_eq!(wrapped.provider_id(), provider_id::ANTHROPIC);
        let summary = wrapped
            .usage_summary()
            .expect("anthropic produces a summary");
        assert_eq!(summary.input_tokens, 12);
        assert_eq!(summary.output_tokens, 7);
        // Anthropic merges cache_read + cache_creation into cached_tokens.
        assert_eq!(summary.cached_tokens, Some(4));
        // total_tokens is computed as input + output (Anthropic does not
        // report total directly).
        assert_eq!(summary.total_tokens, Some(19));
    }

    /// Scenario: the `Gemini` variant forwards `usage_summary()` to the inner
    /// `GenerateContentResponse::usage_summary()` impl. Constructed via a
    /// camelCase wire-shape JSON fixture so a rename of `usageMetadata` or
    /// `promptTokenCount` is caught here.
    #[test]
    fn usage_summary_forwards_for_gemini_variant() {
        let payload = serde_json::json!({
            "candidates": [],
            "usageMetadata": {
                "promptTokenCount": 12,
                "candidatesTokenCount": 7,
                "totalTokenCount": 19,
                "cachedContentTokenCount": 3,
                "thoughtsTokenCount": 2
            }
        });
        let inner: GenerateContentResponse =
            serde_json::from_value(payload).expect("gemini fixture must deserialize");
        let wrapped = AnyCompletionRawResponse::Gemini(inner);
        assert_eq!(wrapped.provider_id(), provider_id::GEMINI);
        let summary = wrapped.usage_summary().expect("gemini produces a summary");
        assert_eq!(summary.input_tokens, 12);
        assert_eq!(summary.output_tokens, 7);
        assert_eq!(summary.total_tokens, Some(19));
    }
}
