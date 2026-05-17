//! Gemini API client construction and authentication.
//!
//! This module provides [`GeminiProvider`], which implements the generic
//! [`Provider`] trait by attaching the `x-goog-api-key` header to every
//! outgoing HTTP request. Unlike OpenAI-style providers that use
//! `Authorization: Bearer <token>`, the Gemini API authenticates via its
//! own header (it also accepts a `key=` query parameter, but the header
//! approach is used here for consistency with the generic `Provider` trait).
//!
//! The [`Client`] type alias combines the generic HTTP client with
//! `GeminiProvider` and exposes convenience constructors. The base URL
//! defaults to `https://generativelanguage.googleapis.com`.

use std::{env, fmt};

use anyhow::Result;

use crate::internal::{
    ai::client::{Client as HttpClient, Provider},
    config::{LocalIdentityTarget, resolve_env_for_target},
};

/// Gemini API provider that carries the API key and injects the
/// `x-goog-api-key` authentication header into every request.
///
/// The `Debug` impl masks the secret to keep it out of `tracing` output and
/// panic backtraces.
#[derive(Clone)]
pub struct GeminiProvider {
    api_key: String,
}

/// Manually implemented to redact the API key from debug output.
impl fmt::Debug for GeminiProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GeminiProvider")
            .field("api_key", &"***")
            .finish()
    }
}

impl GeminiProvider {
    /// Creates a new GeminiProvider with the given API key.
    ///
    /// Boundary conditions: the key is not validated; an invalid key surfaces
    /// at request time as an HTTP 403 from the Gemini API.
    pub fn new(api_key: String) -> Self {
        Self { api_key }
    }

    /// Returns the API key.
    pub fn api_key(&self) -> &str {
        &self.api_key
    }
}

impl Provider for GeminiProvider {
    /// Attaches the `x-goog-api-key` header for Gemini authentication.
    fn on_request(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request.header("x-goog-api-key", &self.api_key)
    }
}

/// Concrete Gemini client type, combining the generic HTTP client with
/// [`GeminiProvider`] for authentication.
pub type Client = HttpClient<GeminiProvider>;

impl Client {
    /// Creates a Gemini Client from environment variables.
    ///
    /// Functional scope: reads `GEMINI_API_KEY` and points at the public
    /// `generativelanguage.googleapis.com` endpoint.
    ///
    /// Boundary conditions: returns `env::VarError::NotPresent` when
    /// `GEMINI_API_KEY` is unset so callers can render a friendly "no API key"
    /// message; the CLI deliberately does not expose a base-URL override —
    /// Gemini's public API has no stable proxy contract for end users.
    /// Test-only consumers that need to point at a localhost stub should
    /// use [`Client::with_base_url`].
    pub fn from_env() -> std::result::Result<Self, env::VarError> {
        let api_key = env::var("GEMINI_API_KEY")?;
        let provider = GeminiProvider::new(api_key);
        Ok(Self::new(
            "https://generativelanguage.googleapis.com",
            provider,
        ))
    }

    /// Vault-aware async constructor: resolves `GEMINI_API_KEY` via
    /// [`resolve_env_for_target`], so callers can store the key in repo-local
    /// or global `vault.env.GEMINI_API_KEY` config without exporting it to
    /// the process environment.
    ///
    /// Priority order:
    /// 1. Process env var
    /// 2. Local repo config (`vault.env.GEMINI_API_KEY`)
    /// 3. Global config
    ///
    /// Gemini does not expose a base-URL override env var (see [`from_env`])
    /// — `from_resolved_env` matches the same single-env-var contract. Tests
    /// that need a stub endpoint should keep using [`with_base_url`].
    pub async fn from_resolved_env(local_target: LocalIdentityTarget<'_>) -> Result<Self> {
        let api_key = resolve_env_for_target("GEMINI_API_KEY", local_target)
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!("GEMINI_API_KEY is not set in env, repo vault, or global config")
            })?;
        let provider = GeminiProvider::new(api_key);
        Ok(Self::new(
            "https://generativelanguage.googleapis.com",
            provider,
        ))
    }

    /// Creates a Gemini Client with a custom base URL and API key.
    ///
    /// Intended for tests that need to point the client at a localhost stub
    /// (Wave 10 §5.2 boot smoke); the CLI never invokes this constructor —
    /// `from_env` is the production entry point. A separate constructor
    /// (rather than mutating `from_env`) keeps the production base URL
    /// explicit and ensures CLI-driven configuration cannot accidentally
    /// reroute requests to an arbitrary host.
    pub fn with_base_url(base_url: &str, api_key: String) -> Self {
        let provider = GeminiProvider::new(api_key);
        Self::new(base_url, provider)
    }

    /// Creates a [`CompletionModel`](super::completion::CompletionModel) bound
    /// to this client for the given Gemini model identifier (e.g.,
    /// `"gemini-2.5-flash"` or one of the constants from [`super`]).
    ///
    /// Boundary conditions: the model id is forwarded verbatim; unknown ids
    /// fail at request time with a 404.
    pub fn completion_model(&self, model: &str) -> super::completion::CompletionModel {
        super::completion::CompletionModel::new(self.clone(), model)
    }
}

#[cfg(test)]
mod tests {
    use serial_test::serial;

    use super::*;

    #[tokio::test]
    #[serial]
    async fn from_resolved_env_reads_gemini_api_key_from_process_env() {
        let key_guard = TestEnvGuard::set("GEMINI_API_KEY", Some("gem-test-resolved"));
        let global_guard = TestEnvGuard::set(
            "LIBRA_CONFIG_GLOBAL_DB",
            Some("/nonexistent/gemini-from-resolved-env-test.db"),
        );

        let client = Client::from_resolved_env(LocalIdentityTarget::None)
            .await
            .expect("from_resolved_env should succeed when GEMINI_API_KEY is set");
        assert_eq!(client.provider.api_key(), "gem-test-resolved");

        drop(key_guard);
        drop(global_guard);
    }

    #[tokio::test]
    #[serial]
    async fn from_resolved_env_errors_when_no_layer_supplies_api_key() {
        let key_guard = TestEnvGuard::set("GEMINI_API_KEY", None);
        let global_guard = TestEnvGuard::set(
            "LIBRA_CONFIG_GLOBAL_DB",
            Some("/nonexistent/gemini-from-resolved-env-test.db"),
        );

        let err = Client::from_resolved_env(LocalIdentityTarget::None)
            .await
            .expect_err("from_resolved_env must fail without an API key");
        assert!(
            err.to_string().contains("GEMINI_API_KEY"),
            "error should name the missing key, got: {err}"
        );

        drop(key_guard);
        drop(global_guard);
    }

    struct TestEnvGuard {
        key: &'static str,
        original: Option<std::ffi::OsString>,
    }

    impl TestEnvGuard {
        fn set(key: &'static str, value: Option<&str>) -> Self {
            let original = std::env::var_os(key);
            // SAFETY: tests are serialized via `#[serial]`, and the guard
            // restores the previous value on drop.
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
            Self { key, original }
        }
    }

    impl Drop for TestEnvGuard {
        fn drop(&mut self) {
            // SAFETY: see `set`.
            unsafe {
                match &self.original {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }
}
