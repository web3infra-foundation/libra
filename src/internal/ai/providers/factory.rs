//! Provider factory — build a concrete [`AnyCompletionModel`] from a
//! [`ModelBinding`] and per-call build options.
//!
//! This module is the second half of OC-Phase 1 P1.2 from
//! `docs/improvement/opencode.md`. The command layer is responsible for
//! turning CLI flags / dotenv into a [`ProviderBuildOptions`]; the factory is
//! responsible for one thing only: dispatching to the right provider client
//! constructor and wrapping the resulting `Model` into [`AnyCompletionModel`].
//!
//! Design intent:
//! - The factory does **not** read env directly. Every API key / base URL it
//!   needs comes through [`ProviderBuildOptions`]; the caller resolves env,
//!   dotenv, and secret-manager layers before invoking `build()`.
//! - Errors are structured and human-actionable. `UnknownProvider` and
//!   `UnknownModel` carry suggestion lists so a TUI surface can render them
//!   directly without re-deriving the candidate set.
//! - Capability check is **best-effort**: an unknown model on a known
//!   provider produces an `UnknownModel` error with suggestions, but the
//!   caller may opt-out via [`ProviderBuildOptions::accept_unknown_models`]
//!   when they intentionally target a freshly released model id.
//! - Ollama bypasses the model-id check because user-defined local models
//!   cannot be enumerated at compile time. Ollama Cloud (`https://ollama.com`)
//!   uses the supplied `api_key` as a bearer token; the local default
//!   endpoint (`http://127.0.0.1:11434/v1`) ignores it.

use std::path::PathBuf;

use thiserror::Error;

use crate::internal::ai::{
    agent::profile::ModelBinding,
    client::CompletionClient,
    providers::{
        AnyCompletionModel, anthropic, capability, deepseek, gemini, kimi, ollama, openai,
        runtime::provider_id, zhipu,
    },
};

/// Per-call options consumed by [`ProviderFactory::build`].
///
/// The struct is a single shape that serves every provider — Ollama Cloud
/// needs `api_key`, Anthropic also needs `api_base` for proxies, fake needs
/// `fake_fixture_path`, and so on. Fields that do not apply to a given
/// provider are silently ignored so adding a new provider does not force
/// every existing call site to change. `bool` fields default to `false` to
/// represent "feature not requested".
#[derive(Clone, Debug, Default)]
pub struct ProviderBuildOptions {
    /// Bearer token / API key for providers that require one. Producer must
    /// have already resolved env / dotenv / interactive entry.
    pub api_key: Option<String>,
    /// Optional API base URL override (e.g. an enterprise proxy or
    /// `https://ollama.com` for Ollama Cloud).
    pub api_base: Option<String>,
    /// When `true`, the Ollama client emits a compact tool-schema instead of
    /// the full OpenAI-style schema. Has no effect for other providers.
    pub ollama_compact_tools: bool,
    /// Path to a fake-provider fixture file. Required for `provider = fake`
    /// builds and ignored everywhere else.
    pub fake_fixture_path: Option<PathBuf>,
    /// When `true`, an unknown model id under a known production provider is
    /// allowed to pass through as-is. Used by callers that intentionally
    /// target a model the static capability table has not been updated for
    /// yet (e.g. an early-access release).
    pub accept_unknown_models: bool,
}

/// Stateless factory.
///
/// The struct exists so the call site reads as `factory.build(...)` rather
/// than a free function — and to leave room for future caching (e.g. reusing
/// a provider client across binding pairs) without a churning API.
#[derive(Debug, Default)]
pub struct ProviderFactory;

impl ProviderFactory {
    /// Build a fresh [`AnyCompletionModel`] for the given binding.
    ///
    /// Steps (must stay in this order so error precedence matches the doc):
    /// 1. Validate `provider_id` against the known set.
    /// 2. Validate `model_id` via [`capability::lookup`] unless the provider
    ///    is Ollama or `accept_unknown_models` is set.
    /// 3. Resolve required options (API key, fixture path) and dispatch to
    ///    the matching provider client constructor.
    pub fn build(
        &self,
        binding: &ModelBinding,
        options: ProviderBuildOptions,
    ) -> Result<AnyCompletionModel, ProviderFactoryError> {
        if !is_known_provider(&binding.provider_id) {
            return Err(ProviderFactoryError::UnknownProvider {
                provider_id: binding.provider_id.clone(),
                available: available_provider_ids(),
            });
        }

        if needs_model_id_check(&binding.provider_id) && !options.accept_unknown_models {
            let known = capability::known_models_for(&binding.provider_id);
            // `known` is empty for ollama (already filtered above) and for
            // any provider whose row set is genuinely empty — in either case
            // we have no basis to reject the caller's id.
            if !known.is_empty()
                && capability::lookup(&binding.provider_id, &binding.model_id).is_none()
            {
                return Err(ProviderFactoryError::UnknownModel {
                    provider_id: binding.provider_id.clone(),
                    model_id: binding.model_id.clone(),
                    suggestions: known,
                });
            }
        }

        match binding.provider_id.as_str() {
            provider_id::ANTHROPIC => build_anthropic(binding, &options),
            provider_id::OPENAI => build_openai(binding, &options),
            provider_id::DEEPSEEK => build_deepseek(binding, &options),
            provider_id::GEMINI => build_gemini(binding, &options),
            provider_id::KIMI => build_kimi(binding, &options),
            provider_id::ZHIPU => build_zhipu(binding, &options),
            provider_id::OLLAMA => build_ollama(binding, &options),
            #[cfg(feature = "test-provider")]
            provider_id::FAKE => build_fake(binding, &options),
            // Already filtered by `is_known_provider` above; reachable only
            // if a new provider id is added to `runtime` without also being
            // added here. Treat as a programmer error.
            other => Err(ProviderFactoryError::UnknownProvider {
                provider_id: other.to_string(),
                available: available_provider_ids(),
            }),
        }
    }
}

fn is_known_provider(id: &str) -> bool {
    if provider_id::ALL_PRODUCTION.contains(&id) {
        return true;
    }
    #[cfg(feature = "test-provider")]
    {
        if id == provider_id::FAKE {
            return true;
        }
    }
    false
}

fn needs_model_id_check(id: &str) -> bool {
    // Ollama is intentionally exempt because user-defined local models can
    // have any name. The fake provider is also exempt: fixtures define
    // arbitrary model names so the test author can pick any string.
    if id == provider_id::OLLAMA {
        return false;
    }
    #[cfg(feature = "test-provider")]
    {
        if id == provider_id::FAKE {
            return false;
        }
    }
    true
}

/// Provider ids the factory is **prepared to dispatch to** in the current
/// build. Equal to [`provider_id::ALL_PRODUCTION`] in release builds and to
/// `ALL_PRODUCTION + [provider_id::FAKE]` when the `test-provider` feature
/// is enabled. This is what we surface in `UnknownProvider.available` so a
/// caller running with fake support sees `fake` listed.
fn available_provider_ids() -> Vec<&'static str> {
    #[cfg(feature = "test-provider")]
    {
        let mut ids = provider_id::ALL_PRODUCTION.to_vec();
        ids.push(provider_id::FAKE);
        ids
    }
    #[cfg(not(feature = "test-provider"))]
    {
        provider_id::ALL_PRODUCTION.to_vec()
    }
}

fn require_api_key<'a>(
    options: &'a ProviderBuildOptions,
    provider_id: &str,
    env_var: &'static str,
) -> Result<&'a str, ProviderFactoryError> {
    options
        .api_key
        .as_deref()
        .ok_or_else(|| ProviderFactoryError::MissingApiKey {
            provider_id: provider_id.to_string(),
            env_var,
        })
}

fn build_anthropic(
    binding: &ModelBinding,
    options: &ProviderBuildOptions,
) -> Result<AnyCompletionModel, ProviderFactoryError> {
    let api_key = require_api_key(options, &binding.provider_id, "ANTHROPIC_API_KEY")?;
    let client = match options.api_base.as_deref() {
        Some(base) => anthropic::Client::with_base_url(base, api_key.to_string()),
        None => anthropic::Client::with_api_key(api_key.to_string()),
    };
    Ok(AnyCompletionModel::Anthropic(
        client.completion_model(&binding.model_id),
    ))
}

fn build_openai(
    binding: &ModelBinding,
    options: &ProviderBuildOptions,
) -> Result<AnyCompletionModel, ProviderFactoryError> {
    let api_key = require_api_key(options, &binding.provider_id, "OPENAI_API_KEY")?;
    let client = match options.api_base.as_deref() {
        Some(base) => openai::Client::with_base_url(base, api_key.to_string()),
        None => openai::Client::with_api_key(api_key.to_string()),
    };
    Ok(AnyCompletionModel::OpenAi(
        client.completion_model(&binding.model_id),
    ))
}

fn build_deepseek(
    binding: &ModelBinding,
    options: &ProviderBuildOptions,
) -> Result<AnyCompletionModel, ProviderFactoryError> {
    let api_key = require_api_key(options, &binding.provider_id, "DEEPSEEK_API_KEY")?;
    let client = match options.api_base.as_deref() {
        Some(base) => deepseek::Client::with_base_url(base, api_key.to_string()),
        None => deepseek::Client::with_api_key(api_key.to_string()),
    };
    Ok(AnyCompletionModel::DeepSeek(
        client.completion_model(&binding.model_id),
    ))
}

fn build_gemini(
    binding: &ModelBinding,
    options: &ProviderBuildOptions,
) -> Result<AnyCompletionModel, ProviderFactoryError> {
    let api_key = require_api_key(options, &binding.provider_id, "GEMINI_API_KEY")?;
    let provider = gemini::client::GeminiProvider::new(api_key.to_string());
    let base_url = options
        .api_base
        .as_deref()
        .unwrap_or("https://generativelanguage.googleapis.com");
    let client = crate::internal::ai::client::Client::new(base_url, provider);
    Ok(AnyCompletionModel::Gemini(
        client.completion_model(&binding.model_id),
    ))
}

fn build_kimi(
    binding: &ModelBinding,
    options: &ProviderBuildOptions,
) -> Result<AnyCompletionModel, ProviderFactoryError> {
    let api_key = require_api_key(options, &binding.provider_id, "MOONSHOT_API_KEY")?;
    let client = match options.api_base.as_deref() {
        Some(base) => kimi::Client::with_base_url(base, api_key.to_string()),
        None => kimi::Client::with_api_key(api_key.to_string()),
    };
    Ok(AnyCompletionModel::Kimi(
        client.completion_model(&binding.model_id),
    ))
}

fn build_zhipu(
    binding: &ModelBinding,
    options: &ProviderBuildOptions,
) -> Result<AnyCompletionModel, ProviderFactoryError> {
    let api_key = require_api_key(options, &binding.provider_id, "ZHIPU_API_KEY")?;
    let client = match options.api_base.as_deref() {
        Some(base) => zhipu::Client::with_base_url(base, api_key.to_string()),
        None => zhipu::Client::with_api_key(api_key.to_string()),
    };
    Ok(AnyCompletionModel::Zhipu(
        client.completion_model(&binding.model_id),
    ))
}

fn build_ollama(
    binding: &ModelBinding,
    options: &ProviderBuildOptions,
) -> Result<AnyCompletionModel, ProviderFactoryError> {
    // Honor the caller-supplied `api_key` instead of having the Ollama
    // client read OLLAMA_API_KEY from process env behind our back. The
    // factory contract says env resolution is the caller's job.
    let api_key = options.api_key.clone();
    let mut client = match options.api_base.as_deref() {
        Some(base) => ollama::Client::with_base_url_and_api_key(base, api_key),
        None => ollama::Client::new_local(),
    };
    // Always set the compact-tool-schema flag from options so a stale
    // `OLLAMA_COMPACT_TOOLS=1` in the environment cannot silently override
    // a caller that asked for the verbose schema.
    client = client.with_compact_tool_schema(options.ollama_compact_tools);
    if client.missing_required_cloud_api_key() {
        return Err(ProviderFactoryError::MissingApiKey {
            provider_id: binding.provider_id.clone(),
            env_var: "OLLAMA_API_KEY",
        });
    }
    Ok(AnyCompletionModel::Ollama(
        client.completion_model(&binding.model_id),
    ))
}

#[cfg(feature = "test-provider")]
fn build_fake(
    binding: &ModelBinding,
    options: &ProviderBuildOptions,
) -> Result<AnyCompletionModel, ProviderFactoryError> {
    let path =
        options
            .fake_fixture_path
            .as_deref()
            .ok_or_else(|| ProviderFactoryError::BuildFailed {
                provider_id: binding.provider_id.clone(),
                reason: "fake_fixture_path is required for the fake provider".to_string(),
            })?;
    let client = super::fake::Client::from_fixture_path(path).map_err(|error| {
        ProviderFactoryError::BuildFailed {
            provider_id: binding.provider_id.clone(),
            reason: format!("failed to load fake fixture {}: {error}", path.display()),
        }
    })?;
    Ok(AnyCompletionModel::Fake(
        client.completion_model(&binding.model_id),
    ))
}

/// Structured failure modes from [`ProviderFactory::build`].
///
/// Each variant carries enough context for a TUI surface to render an
/// actionable message without re-deriving suggestion lists or env-var names.
#[derive(Debug, Error)]
pub enum ProviderFactoryError {
    /// The `provider_id` does not match any known production provider (or
    /// the gated `fake` provider, when enabled). `available` is the list of
    /// recognised ids so the surface can say "did you mean ...".
    #[error(
        "unknown provider '{provider_id}'; available providers: {}",
        available.join(", ")
    )]
    UnknownProvider {
        provider_id: String,
        available: Vec<&'static str>,
    },

    /// The `model_id` is unknown for an otherwise valid `provider_id` and
    /// the caller did not opt into [`ProviderBuildOptions::accept_unknown_models`].
    /// `suggestions` lists the catalogued ids in declaration order.
    #[error(
        "unknown model '{model_id}' for provider '{provider_id}'; known models: {}",
        suggestions.join(", ")
    )]
    UnknownModel {
        provider_id: String,
        model_id: String,
        suggestions: Vec<&'static str>,
    },

    /// The provider needs an API key (or other credential) that was not
    /// supplied via [`ProviderBuildOptions::api_key`]. `env_var` is the
    /// canonical env var name so the surface can prompt the user to set it.
    #[error("provider '{provider_id}' requires an API key; set {env_var}")]
    MissingApiKey {
        provider_id: String,
        env_var: &'static str,
    },

    /// Provider client construction failed for a reason other than missing
    /// credentials (e.g. unreadable fake fixture, malformed base URL).
    #[error("failed to build provider '{provider_id}': {reason}")]
    BuildFailed { provider_id: String, reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scenario: an unknown provider id surfaces `UnknownProvider` with the
    /// full production set in `available`. The surface needs at least the
    /// 7 production providers so it can render a "did you mean" hint
    /// covering the entire catalog.
    #[test]
    fn unknown_provider_returns_full_production_suggestions() {
        let factory = ProviderFactory;
        let binding = ModelBinding::parse("aleph-omega/foo").unwrap();
        let err = factory
            .build(&binding, ProviderBuildOptions::default())
            .expect_err("unknown provider must error");
        match err {
            ProviderFactoryError::UnknownProvider {
                provider_id,
                available,
            } => {
                assert_eq!(provider_id, "aleph-omega");
                assert!(available.contains(&"anthropic"));
                assert!(available.contains(&"openai"));
                assert!(available.contains(&"gemini"));
                assert!(available.contains(&"deepseek"));
                assert!(available.contains(&"kimi"));
                assert!(available.contains(&"zhipu"));
                assert!(available.contains(&"ollama"));
            }
            other => panic!("expected UnknownProvider, got {other:?}"),
        }
    }

    /// Scenario: a known provider with an unknown model id returns
    /// `UnknownModel` with at least one suggestion drawn from the static
    /// capability table.
    #[test]
    fn unknown_model_returns_at_least_one_suggestion() {
        let factory = ProviderFactory;
        let binding = ModelBinding::parse("anthropic/claude-from-the-future").unwrap();
        let err = factory
            .build(&binding, ProviderBuildOptions::default())
            .expect_err("unknown model must error");
        match err {
            ProviderFactoryError::UnknownModel {
                provider_id,
                model_id,
                suggestions,
            } => {
                assert_eq!(provider_id, "anthropic");
                assert_eq!(model_id, "claude-from-the-future");
                assert!(!suggestions.is_empty());
                assert!(suggestions.contains(&"claude-3-5-sonnet-latest"));
            }
            other => panic!("expected UnknownModel, got {other:?}"),
        }
    }

    /// Scenario: `accept_unknown_models = true` lets a freshly released
    /// model id pass the capability check; the build then fails on a later
    /// stage (here: missing API key) — proving the model gate was bypassed.
    #[test]
    fn accept_unknown_models_skips_capability_check() {
        let factory = ProviderFactory;
        let binding = ModelBinding::parse("anthropic/claude-from-the-future").unwrap();
        let options = ProviderBuildOptions {
            accept_unknown_models: true,
            ..ProviderBuildOptions::default()
        };
        let err = factory
            .build(&binding, options)
            .expect_err("unknown model + no api_key must surface MissingApiKey, not UnknownModel");
        assert!(
            matches!(err, ProviderFactoryError::MissingApiKey { env_var, .. }
                if env_var == "ANTHROPIC_API_KEY"),
            "expected MissingApiKey on ANTHROPIC_API_KEY, got {err:?}"
        );
    }

    /// Scenario: a provider that demands an API key but received none yields
    /// `MissingApiKey` with the canonical env var name.
    #[test]
    fn missing_api_key_reports_canonical_env_var() {
        let factory = ProviderFactory;
        let cases: &[(&str, &str)] = &[
            ("anthropic/claude-3-5-sonnet-latest", "ANTHROPIC_API_KEY"),
            ("openai/gpt-4o-mini", "OPENAI_API_KEY"),
            ("deepseek/deepseek-chat", "DEEPSEEK_API_KEY"),
            ("gemini/gemini-2.5-flash", "GEMINI_API_KEY"),
            ("kimi/kimi-k2.6", "MOONSHOT_API_KEY"),
            ("zhipu/glm-5", "ZHIPU_API_KEY"),
        ];
        for (binding_str, expected_env) in cases {
            let binding = ModelBinding::parse(binding_str).expect("binding parses");
            let err = factory
                .build(&binding, ProviderBuildOptions::default())
                .expect_err("api-key-required provider must error");
            match err {
                ProviderFactoryError::MissingApiKey { env_var, .. } => {
                    assert_eq!(
                        env_var, *expected_env,
                        "wrong env var for binding {binding_str}"
                    );
                }
                other => panic!("expected MissingApiKey for {binding_str}, got {other:?}"),
            }
        }
    }

    /// Scenario: Ollama bypasses both API-key and model-id gates when used
    /// against the local default endpoint. A made-up model name builds
    /// successfully, returning an `AnyCompletionModel::Ollama` whose
    /// `provider_id()` is `"ollama"`.
    #[test]
    fn ollama_bypasses_model_check_and_builds_local() {
        let factory = ProviderFactory;
        let binding = ModelBinding::parse("ollama/totally-made-up-model").unwrap();
        let model = factory
            .build(&binding, ProviderBuildOptions::default())
            .expect("ollama default-local build must succeed");
        assert_eq!(model.provider_id(), "ollama");
        assert!(matches!(model, AnyCompletionModel::Ollama(_)));
    }

    /// Scenario: pointing Ollama at the cloud endpoint (`https://ollama.com`)
    /// without an API key surfaces `MissingApiKey { env_var: "OLLAMA_API_KEY" }`
    /// — the doc explicitly calls this out as a UX trap, so the factory
    /// must detect it instead of returning a model that 401s on first use.
    #[test]
    fn ollama_cloud_without_api_key_surfaces_missing_api_key() {
        let factory = ProviderFactory;
        let binding = ModelBinding::parse("ollama/llama3.2").unwrap();
        let options = ProviderBuildOptions {
            api_base: Some("https://ollama.com".to_string()),
            ..ProviderBuildOptions::default()
        };
        let err = factory
            .build(&binding, options)
            .expect_err("ollama cloud + no api_key must error");
        match err {
            ProviderFactoryError::MissingApiKey { env_var, .. } => {
                assert_eq!(env_var, "OLLAMA_API_KEY");
            }
            other => panic!("expected MissingApiKey, got {other:?}"),
        }
    }

    /// Scenario: pointing Ollama at the cloud endpoint **with** an
    /// `api_key` provided through `ProviderBuildOptions` succeeds. This
    /// pins the contract that the factory honors caller-supplied
    /// credentials instead of silently falling back to process env.
    #[test]
    fn ollama_cloud_with_api_key_in_options_builds_successfully() {
        let factory = ProviderFactory;
        let binding = ModelBinding::parse("ollama/llama3.2").unwrap();
        let options = ProviderBuildOptions {
            api_base: Some("https://ollama.com".to_string()),
            api_key: Some("ol-supplied-from-options".to_string()),
            ..ProviderBuildOptions::default()
        };
        let model = factory
            .build(&binding, options)
            .expect("ollama cloud + supplied api_key must build");
        assert_eq!(model.provider_id(), "ollama");
        assert!(matches!(model, AnyCompletionModel::Ollama(_)));
    }

    /// Scenario (test-provider only): when the `test-provider` feature is
    /// enabled, `UnknownProvider.available` includes `fake` so a TUI surface
    /// can render the full set of dispatchable providers in this build.
    #[cfg(feature = "test-provider")]
    #[test]
    fn unknown_provider_available_includes_fake_under_test_provider_feature() {
        let factory = ProviderFactory;
        let binding = ModelBinding::parse("aleph-omega/foo").unwrap();
        let err = factory
            .build(&binding, ProviderBuildOptions::default())
            .expect_err("unknown provider must error");
        match err {
            ProviderFactoryError::UnknownProvider { available, .. } => {
                assert!(
                    available.contains(&"fake"),
                    "expected `fake` to be listed under --features test-provider, got {available:?}"
                );
            }
            other => panic!("expected UnknownProvider, got {other:?}"),
        }
    }

    /// Scenario (test-provider only): the fake provider builds from a
    /// fixture file and returns `AnyCompletionModel::Fake`. Without a
    /// fixture path the build fails with `BuildFailed` (not silently).
    #[cfg(feature = "test-provider")]
    #[test]
    fn fake_provider_builds_from_fixture_and_errors_without_one() {
        use std::io::Write;
        let factory = ProviderFactory;
        let binding = ModelBinding::parse("fake/anything").unwrap();

        // Without a fixture: BuildFailed.
        let err = factory
            .build(&binding, ProviderBuildOptions::default())
            .expect_err("fake without fixture must error");
        assert!(matches!(err, ProviderFactoryError::BuildFailed { .. }));

        // With a minimal fixture: AnyCompletionModel::Fake.
        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile");
        writeln!(
            tmp,
            r#"{{"responses":[],"fallback":{{"type":"text","text":"ok"}}}}"#
        )
        .expect("write fixture");
        let options = ProviderBuildOptions {
            fake_fixture_path: Some(tmp.path().to_path_buf()),
            ..ProviderBuildOptions::default()
        };
        let model = factory
            .build(&binding, options)
            .expect("fake with fixture must build");
        assert_eq!(model.provider_id(), "fake");
        assert!(matches!(model, AnyCompletionModel::Fake(_)));
    }
}
