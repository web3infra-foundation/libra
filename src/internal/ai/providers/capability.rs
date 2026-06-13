//! Static model-capability matrix.
//!
//! This module is part of OC-Phase 1 P1.2 from `docs/development/commands/_general.md`.
//! It records what each `(provider_id, model_id)` pair is known to support so
//! the factory can give actionable error messages and the runtime can refuse
//! a feature the model cannot handle (e.g. structured tool calls on a
//! pre-tool-use model) **before** burning a request.
//!
//! Important constraints, lifted from the doc:
//!
//! - The capability table is **not a security boundary**. It exists only to
//!   produce friendlier early errors; missing entries should never silently
//!   relax a sandbox or approval policy.
//! - The first version is statically maintained. New providers / models are
//!   added by editing [`KNOWN_MODELS`] — there is no remote refresh.
//! - Ollama is excluded by design: the user can run *any* local model and we
//!   cannot enumerate them at compile time. Ollama lookups always return
//!   `None`, and the factory therefore accepts arbitrary model strings for
//!   that provider.

use crate::internal::ai::providers::runtime::provider_id;

/// Known capability flags for a single `(provider_id, model_id)` pair.
///
/// All fields are optional in spirit — `false` simply means "we have not
/// confirmed this", not "we have confirmed it is unsupported". Treat the
/// struct as best-effort metadata for the UX surface, not a contract.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModelCapability {
    /// The model can be invoked with structured tool definitions (function
    /// calling). All Libra agents require this today.
    pub supports_tool_calls: bool,
    /// The model emits incremental stream events.
    pub supports_streaming: bool,
    /// The model accepts image / multimodal input.
    pub supports_vision: bool,
    /// The model exposes a reasoning / thinking knob.
    pub supports_reasoning: bool,
    /// The model interleaves reasoning with tool calls within a single turn.
    pub supports_interleaved: bool,
    /// Maximum total context window in tokens (0 = unknown).
    pub context_window: u32,
    /// Maximum output tokens per response (0 = unknown).
    pub output_limit: u32,
    /// USD pricing per 1M tokens, when published.
    pub cost: Option<ModelCost>,
}

/// Per-million-token USD cost split between prompt and completion tokens.
///
/// Cached / batch / vision tier multipliers are deliberately excluded from the
/// first version — `run_tool_loop`'s usage aggregator only needs the basic
/// input/output split.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModelCost {
    pub input_per_million_tokens_usd: f64,
    pub output_per_million_tokens_usd: f64,
}

impl Eq for ModelCost {}

/// One row in the static capability table.
struct KnownModel {
    provider_id: &'static str,
    model_id: &'static str,
    capability: ModelCapability,
}

/// Static (provider_id, model_id) → capability table.
///
/// Lookups walk the slice linearly. The list is small enough (~30 rows) that
/// the linear scan is well below noise on every hot path; switching to a
/// `HashMap` would just add an `OnceLock` and lose `const`-ness.
///
/// Maintenance: when a provider releases a new flagship model, add a row
/// here and extend `lookup_returns_tool_call_support_for_canonical_models`
/// (or a more specific test). Capability bits that have not been verified
/// should stay `false`; the runtime is allowed to attempt a request even
/// when a flag is unset, since this table is best-effort.
static KNOWN_MODELS: &[KnownModel] = &[
    // ─── Anthropic ────────────────────────────────────────────────────────
    KnownModel {
        provider_id: provider_id::ANTHROPIC,
        model_id: "claude-opus-4-0",
        capability: ModelCapability {
            supports_tool_calls: true,
            supports_streaming: true,
            supports_vision: true,
            supports_reasoning: true,
            supports_interleaved: true,
            context_window: 200_000,
            output_limit: 32_000,
            cost: None,
        },
    },
    KnownModel {
        provider_id: provider_id::ANTHROPIC,
        model_id: "claude-sonnet-4-0",
        capability: ModelCapability {
            supports_tool_calls: true,
            supports_streaming: true,
            supports_vision: true,
            supports_reasoning: true,
            supports_interleaved: true,
            context_window: 200_000,
            output_limit: 64_000,
            cost: None,
        },
    },
    KnownModel {
        provider_id: provider_id::ANTHROPIC,
        model_id: "claude-3-7-sonnet-latest",
        capability: ModelCapability {
            supports_tool_calls: true,
            supports_streaming: true,
            supports_vision: true,
            supports_reasoning: true,
            supports_interleaved: false,
            context_window: 200_000,
            output_limit: 8_192,
            cost: None,
        },
    },
    KnownModel {
        provider_id: provider_id::ANTHROPIC,
        model_id: "claude-3-5-sonnet-latest",
        capability: ModelCapability {
            supports_tool_calls: true,
            supports_streaming: true,
            supports_vision: true,
            supports_reasoning: false,
            supports_interleaved: false,
            context_window: 200_000,
            output_limit: 8_192,
            cost: Some(ModelCost {
                input_per_million_tokens_usd: 3.0,
                output_per_million_tokens_usd: 15.0,
            }),
        },
    },
    KnownModel {
        provider_id: provider_id::ANTHROPIC,
        model_id: "claude-3-5-haiku-latest",
        capability: ModelCapability {
            supports_tool_calls: true,
            supports_streaming: true,
            supports_vision: true,
            supports_reasoning: false,
            supports_interleaved: false,
            context_window: 200_000,
            output_limit: 8_192,
            cost: Some(ModelCost {
                input_per_million_tokens_usd: 0.8,
                output_per_million_tokens_usd: 4.0,
            }),
        },
    },
    // ─── OpenAI ───────────────────────────────────────────────────────────
    KnownModel {
        provider_id: provider_id::OPENAI,
        model_id: "gpt-4o",
        capability: ModelCapability {
            supports_tool_calls: true,
            supports_streaming: true,
            supports_vision: true,
            supports_reasoning: false,
            supports_interleaved: false,
            context_window: 128_000,
            output_limit: 16_384,
            cost: Some(ModelCost {
                input_per_million_tokens_usd: 2.5,
                output_per_million_tokens_usd: 10.0,
            }),
        },
    },
    KnownModel {
        provider_id: provider_id::OPENAI,
        model_id: "gpt-4o-mini",
        capability: ModelCapability {
            supports_tool_calls: true,
            supports_streaming: true,
            supports_vision: true,
            supports_reasoning: false,
            supports_interleaved: false,
            context_window: 128_000,
            output_limit: 16_384,
            cost: Some(ModelCost {
                input_per_million_tokens_usd: 0.15,
                output_per_million_tokens_usd: 0.60,
            }),
        },
    },
    KnownModel {
        provider_id: provider_id::OPENAI,
        model_id: "o1",
        capability: ModelCapability {
            supports_tool_calls: true,
            supports_streaming: false,
            supports_vision: true,
            supports_reasoning: true,
            supports_interleaved: false,
            context_window: 200_000,
            output_limit: 100_000,
            cost: None,
        },
    },
    KnownModel {
        provider_id: provider_id::OPENAI,
        model_id: "o1-mini",
        capability: ModelCapability {
            supports_tool_calls: true,
            supports_streaming: false,
            supports_vision: false,
            supports_reasoning: true,
            supports_interleaved: false,
            context_window: 128_000,
            output_limit: 65_536,
            cost: None,
        },
    },
    // ─── DeepSeek ─────────────────────────────────────────────────────────
    KnownModel {
        provider_id: provider_id::DEEPSEEK,
        model_id: "deepseek-chat",
        capability: ModelCapability {
            supports_tool_calls: true,
            supports_streaming: true,
            supports_vision: false,
            supports_reasoning: false,
            supports_interleaved: false,
            context_window: 64_000,
            output_limit: 8_192,
            cost: None,
        },
    },
    KnownModel {
        provider_id: provider_id::DEEPSEEK,
        model_id: "deepseek-reasoner",
        capability: ModelCapability {
            supports_tool_calls: true,
            supports_streaming: true,
            supports_vision: false,
            supports_reasoning: true,
            supports_interleaved: false,
            context_window: 64_000,
            output_limit: 8_192,
            cost: None,
        },
    },
    // ─── Google Gemini ────────────────────────────────────────────────────
    KnownModel {
        provider_id: provider_id::GEMINI,
        model_id: "gemini-2.5-flash",
        capability: ModelCapability {
            supports_tool_calls: true,
            supports_streaming: true,
            supports_vision: true,
            supports_reasoning: true,
            supports_interleaved: false,
            context_window: 1_048_576,
            output_limit: 65_536,
            cost: None,
        },
    },
    KnownModel {
        provider_id: provider_id::GEMINI,
        model_id: "gemini-2.0-flash",
        capability: ModelCapability {
            supports_tool_calls: true,
            supports_streaming: true,
            supports_vision: true,
            supports_reasoning: false,
            supports_interleaved: false,
            context_window: 1_048_576,
            output_limit: 8_192,
            cost: None,
        },
    },
    // ─── Moonshot Kimi ────────────────────────────────────────────────────
    KnownModel {
        provider_id: provider_id::KIMI,
        model_id: "kimi-k2.6",
        capability: ModelCapability {
            supports_tool_calls: true,
            supports_streaming: true,
            supports_vision: false,
            supports_reasoning: true,
            supports_interleaved: false,
            context_window: 256_000,
            output_limit: 16_384,
            cost: None,
        },
    },
    KnownModel {
        provider_id: provider_id::KIMI,
        model_id: "kimi-k2-thinking",
        capability: ModelCapability {
            supports_tool_calls: true,
            supports_streaming: true,
            supports_vision: false,
            supports_reasoning: true,
            supports_interleaved: false,
            context_window: 256_000,
            output_limit: 16_384,
            cost: None,
        },
    },
    KnownModel {
        provider_id: provider_id::KIMI,
        model_id: "moonshot-v1-128k",
        capability: ModelCapability {
            supports_tool_calls: true,
            supports_streaming: true,
            supports_vision: false,
            supports_reasoning: false,
            supports_interleaved: false,
            context_window: 128_000,
            output_limit: 8_192,
            cost: None,
        },
    },
    // ─── Zhipu GLM ────────────────────────────────────────────────────────
    KnownModel {
        provider_id: provider_id::ZHIPU,
        model_id: "glm-5",
        capability: ModelCapability {
            supports_tool_calls: true,
            supports_streaming: true,
            supports_vision: false,
            supports_reasoning: true,
            supports_interleaved: false,
            context_window: 128_000,
            output_limit: 8_192,
            cost: None,
        },
    },
    KnownModel {
        provider_id: provider_id::ZHIPU,
        model_id: "glm-4",
        capability: ModelCapability {
            supports_tool_calls: true,
            supports_streaming: true,
            supports_vision: false,
            supports_reasoning: false,
            supports_interleaved: false,
            context_window: 128_000,
            output_limit: 4_096,
            cost: None,
        },
    },
];

/// Look up the static capability for a `(provider_id, model_id)` pair.
///
/// Returns:
/// - `Some(_)` when the pair is in the table.
/// - `None` for Ollama (any user-supplied model name is acceptable since the
///   user runs the inference locally), an unknown provider, or an unknown
///   model id under a known provider.
pub fn lookup(provider_id: &str, model_id: &str) -> Option<ModelCapability> {
    KNOWN_MODELS
        .iter()
        .find(|row| row.provider_id == provider_id && row.model_id == model_id)
        .map(|row| row.capability)
}

/// All model ids the table knows for a given provider, in declaration order.
///
/// Used by [`super::factory`] to build the `suggestions` field on
/// `ProviderFactoryError::UnknownModel` so the user can see which models we
/// recognise without leaving the terminal.
pub fn known_models_for(provider_id: &str) -> Vec<&'static str> {
    KNOWN_MODELS
        .iter()
        .filter(|row| row.provider_id == provider_id)
        .map(|row| row.model_id)
        .collect()
}

/// A conservative fallback capability for a provider, used when the caller
/// has chosen [`super::factory::ProviderBuildOptions::accept_unknown_models`]
/// and the static `(provider, model)` table has no row to consult.
///
/// "Conservative" means: only flags that hold for **every** catalogued model
/// of that provider are reported `true`. If any catalogued model lacks a
/// capability, the default reports `false`. Numeric fields collapse to the
/// minimum across catalogued rows so a UX surface that uses these to decide
/// "can I send this large prompt" never overestimates.
///
/// Returns `None` for providers with no catalogued rows (Ollama, unknown,
/// fake) — the caller cannot derive a meaningful default and must either
/// require the user to specify capabilities or proceed without them.
pub fn provider_default(provider_id: &str) -> Option<ModelCapability> {
    let rows: Vec<&KnownModel> = KNOWN_MODELS
        .iter()
        .filter(|row| row.provider_id == provider_id)
        .collect();
    if rows.is_empty() {
        return None;
    }
    let mut folded = rows[0].capability;
    for row in rows.iter().skip(1) {
        folded = fold_capability(folded, row.capability);
    }
    // Always strip cost from the default — provider-level "average" cost
    // would be misleading. Callers that need cost must look up a specific
    // model. (For a single-row provider `fold_capability` is never invoked,
    // so we drop cost explicitly here.)
    folded.cost = None;
    Some(folded)
}

fn fold_capability(left: ModelCapability, right: ModelCapability) -> ModelCapability {
    ModelCapability {
        supports_tool_calls: left.supports_tool_calls && right.supports_tool_calls,
        supports_streaming: left.supports_streaming && right.supports_streaming,
        supports_vision: left.supports_vision && right.supports_vision,
        supports_reasoning: left.supports_reasoning && right.supports_reasoning,
        supports_interleaved: left.supports_interleaved && right.supports_interleaved,
        context_window: left.context_window.min(right.context_window),
        output_limit: left.output_limit.min(right.output_limit),
        // Cost is intentionally dropped in the fold: averaging across the
        // catalog would be misleading. Callers that care about cost must
        // look up a specific model.
        cost: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scenario: a flagship model from each catalogued provider returns a
    /// capability with `supports_tool_calls = true`. This is the contract
    /// every Libra agent depends on; if any of these regress the factory
    /// would silently let the agent bypass tool calling.
    #[test]
    fn lookup_returns_tool_call_support_for_canonical_models() {
        let cases: &[(&str, &str)] = &[
            (provider_id::ANTHROPIC, "claude-3-5-sonnet-latest"),
            (provider_id::OPENAI, "gpt-4o-mini"),
            (provider_id::DEEPSEEK, "deepseek-chat"),
            (provider_id::GEMINI, "gemini-2.5-flash"),
            (provider_id::KIMI, "kimi-k2.6"),
            (provider_id::ZHIPU, "glm-5"),
        ];
        for (provider, model) in cases {
            let cap = lookup(provider, model)
                .unwrap_or_else(|| panic!("missing capability for {provider}/{model}"));
            assert!(
                cap.supports_tool_calls,
                "{provider}/{model} must support tools"
            );
            assert!(
                cap.context_window > 0,
                "{provider}/{model} must report a context window"
            );
        }
    }

    /// Scenario: Ollama lookups always return `None`. The user runs arbitrary
    /// local models so the table cannot enumerate them; the factory must
    /// therefore accept any `ollama/<anything>` binding without consulting
    /// the table.
    #[test]
    fn ollama_lookup_returns_none_for_any_model() {
        assert!(lookup(provider_id::OLLAMA, "llama3.2").is_none());
        assert!(lookup(provider_id::OLLAMA, "qwen2:14b").is_none());
        assert!(lookup(provider_id::OLLAMA, "totally-made-up").is_none());
    }

    /// Scenario: unknown provider strings return `None` instead of panicking
    /// or pattern-matching loosely.
    #[test]
    fn lookup_returns_none_for_unknown_provider() {
        assert!(lookup("aleph-omega", "foo").is_none());
    }

    /// Scenario: a known provider with an unknown model id returns `None`.
    /// `known_models_for` then offers the catalogued ids so the factory can
    /// surface a useful suggestion list.
    #[test]
    fn lookup_returns_none_for_unknown_model_under_known_provider() {
        assert!(lookup(provider_id::ANTHROPIC, "claude-imaginary").is_none());
        let suggestions = known_models_for(provider_id::ANTHROPIC);
        assert!(suggestions.contains(&"claude-3-5-sonnet-latest"));
        assert!(!suggestions.is_empty());
    }

    /// Scenario: `known_models_for` returns the empty slice for a provider
    /// that has no rows (Ollama, unknown).
    #[test]
    fn known_models_for_returns_empty_when_no_rows_match() {
        assert!(known_models_for(provider_id::OLLAMA).is_empty());
        assert!(known_models_for("aleph-omega").is_empty());
    }

    /// Scenario: every catalogued provider id is one of Libra's production
    /// provider ids. Guards against typos in `provider_id::*` references
    /// inside the table.
    #[test]
    fn every_row_uses_a_production_provider_id() {
        for row in KNOWN_MODELS {
            assert!(
                provider_id::ALL_PRODUCTION.contains(&row.provider_id),
                "row uses non-production provider id: {}",
                row.provider_id
            );
        }
    }

    /// Scenario: `provider_default()` for Anthropic returns a capability
    /// where every flag is the AND of catalogued models' flags. All
    /// Anthropic rows in the table support tool calls and streaming, so
    /// those should remain `true`; reasoning is `false` because the 3.5
    /// rows are not reasoning-enabled. `cost` is always dropped.
    #[test]
    fn provider_default_returns_conservative_intersection() {
        let cap = provider_default(provider_id::ANTHROPIC).expect("anthropic has catalogued rows");
        assert!(cap.supports_tool_calls);
        assert!(cap.supports_streaming);
        // Mixed across catalogued rows: 3.5-sonnet has no reasoning,
        // 3-7-sonnet does — the AND is `false`.
        assert!(!cap.supports_reasoning);
        // `interleaved` only on Claude 4 family — AND across all is false.
        assert!(!cap.supports_interleaved);
        assert!(cap.cost.is_none());
        // context_window is the min across rows; every Anthropic row in
        // the table reports 200_000.
        assert_eq!(cap.context_window, 200_000);
        // output_limit min across rows: 8_192 (3.5 / 3.7) wins over 32_000+.
        assert_eq!(cap.output_limit, 8_192);
    }

    /// Scenario: `provider_default()` returns `None` for providers we do
    /// not catalogue (Ollama because users supply local model names, fake
    /// because fixtures are arbitrary). The caller must handle these
    /// explicitly rather than receive a misleading "default".
    #[test]
    fn provider_default_returns_none_for_uncatalogued_providers() {
        assert!(provider_default(provider_id::OLLAMA).is_none());
        assert!(provider_default("aleph-omega").is_none());
    }

    /// Scenario: `provider_default` must always drop `cost` regardless of
    /// catalogue row count. Today every catalogued provider has multiple
    /// rows, so we cannot exercise the single-row branch through the real
    /// table — but the unconditional `folded.cost = None` guarantees the
    /// branch is safe whenever a future provider lands with one row.
    /// The test pins the post-condition that matters: any catalogued
    /// provider whose flagship row carries a cost still surfaces a
    /// cost-less default.
    #[test]
    fn provider_default_always_drops_cost_field() {
        // `claude-3-5-sonnet-latest` carries a documented cost, so a
        // shallow fold that leaks `rows[0].cost` would surface it.
        let model_cap = lookup(provider_id::ANTHROPIC, "claude-3-5-sonnet-latest")
            .expect("flagship row exists");
        assert!(model_cap.cost.is_some(), "fixture must carry a cost");

        let default_cap = provider_default(provider_id::ANTHROPIC).expect("anthropic default");
        assert!(
            default_cap.cost.is_none(),
            "provider_default must drop cost regardless of row count"
        );
    }
}
