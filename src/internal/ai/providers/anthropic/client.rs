//! Anthropic API client construction and authentication.
//!
//! This module provides [`AnthropicProvider`], which implements the generic
//! [`Provider`] trait by attaching Anthropic-specific authentication headers
//! to every outgoing HTTP request:
//!
//! - `x-api-key` -- the secret API key (Anthropic does **not** use Bearer
//!   token authentication).
//! - `anthropic-version` -- a required version header that pins the wire
//!   format of the Messages API.
//!
//! The [`Client`] type alias combines the generic HTTP client with
//! `AnthropicProvider` and exposes convenience constructors that read
//! credentials from environment variables (`ANTHROPIC_API_KEY`) or accept
//! them directly.

use std::fmt;

use anyhow::Result;

use crate::internal::{
    ai::client::{Client as GenericClient, Provider},
    config::{LocalIdentityTarget, resolve_env_for_target},
};

/// Anthropic API provider that carries the API key and injects
/// authentication headers into every request.
///
/// Cloning is cheap (the `String` is a single heap allocation) so the type is
/// freely clonable into background tasks. The `Debug` impl masks the key so it
/// cannot leak through `tracing` or panic messages.
#[derive(Clone)]
pub struct AnthropicProvider {
    api_key: String,
}

impl fmt::Debug for AnthropicProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnthropicProvider")
            .field("api_key", &"***")
            .finish()
    }
}

impl AnthropicProvider {
    /// Creates a new Anthropic provider with the given API key.
    ///
    /// Boundary conditions:
    /// - The API key is not validated here; the first request will fail with a
    ///   401 if the key is malformed or missing privileges.
    pub fn new(api_key: String) -> Self {
        Self { api_key }
    }

    /// Returns the API key.
    ///
    /// Boundary conditions:
    /// - Avoid logging the returned slice; prefer the `Debug` impl which masks it.
    pub fn api_key(&self) -> &str {
        &self.api_key
    }
}

impl Provider for AnthropicProvider {
    /// Injects Anthropic-specific headers into the outgoing request.
    ///
    /// Unlike most LLM providers that use `Authorization: Bearer <token>`,
    /// Anthropic authenticates via a dedicated `x-api-key` header. The
    /// `anthropic-version` header is also required by the API and controls
    /// which version of the Messages API wire format the server uses.
    fn on_request(&self, mut request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        // Anthropic uses x-api-key header (not Bearer token)
        request = request.header("x-api-key", &self.api_key);
        // Anthropic requires anthropic-version header
        request = request.header("anthropic-version", super::ANTHROPIC_VERSION);
        request
    }
}

/// Concrete Anthropic client type, combining the generic HTTP client with
/// [`AnthropicProvider`] for authentication.
pub type Client = GenericClient<AnthropicProvider>;

impl Client {
    /// Creates an Anthropic client from environment variables.
    ///
    /// Functional scope:
    /// - Reads `ANTHROPIC_API_KEY` (required).
    /// - Falls back to `https://api.anthropic.com` when `ANTHROPIC_BASE_URL` is unset.
    ///
    /// Boundary conditions:
    /// - Returns `std::env::VarError::NotPresent` when `ANTHROPIC_API_KEY` is missing
    ///   so callers can surface a friendly "no API key" message.
    /// - `ANTHROPIC_BASE_URL`, when set, is forwarded verbatim — no scheme validation.
    pub fn from_env() -> std::result::Result<Self, std::env::VarError> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")?;
        let base_url = std::env::var("ANTHROPIC_BASE_URL")
            .unwrap_or_else(|_| "https://api.anthropic.com".to_string());

        let provider = AnthropicProvider::new(api_key);
        Ok(Self::new(&base_url, provider))
    }

    /// Vault-aware async constructor: resolves `ANTHROPIC_API_KEY` and
    /// `ANTHROPIC_BASE_URL` via [`resolve_env_for_target`], so callers can
    /// store credentials in repo-local or global `vault.env.*` config without
    /// also exporting them into the process environment.
    ///
    /// Functional scope (priority order matches the rest of Libra's config
    /// cascade):
    /// 1. Process env var
    /// 2. Local repo config (`vault.env.ANTHROPIC_API_KEY` / `vault.env.ANTHROPIC_BASE_URL`)
    /// 3. Global config
    ///
    /// Boundary conditions:
    /// - Returns an error tagged `ANTHROPIC_API_KEY is not set in env, repo
    ///   vault, or global config` when the key cannot be resolved from any
    ///   layer. The `from_env` sync constructor is preserved as a low-level
    ///   programmatic API for callers that explicitly only want process-env
    ///   resolution.
    /// - `ANTHROPIC_BASE_URL` falls back to the canonical Anthropic URL when
    ///   no layer supplies it.
    pub async fn from_resolved_env(local_target: LocalIdentityTarget<'_>) -> Result<Self> {
        let api_key = resolve_env_for_target("ANTHROPIC_API_KEY", local_target)
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!("ANTHROPIC_API_KEY is not set in env, repo vault, or global config")
            })?;
        let base_url = resolve_env_for_target("ANTHROPIC_BASE_URL", local_target)
            .await?
            .unwrap_or_else(|| "https://api.anthropic.com".to_string());

        let provider = AnthropicProvider::new(api_key);
        Ok(Self::new(&base_url, provider))
    }

    /// Creates an Anthropic client with the given API key.
    ///
    /// Functional scope: Convenience constructor that always uses Anthropic's
    /// production base URL. Use [`Client::with_base_url`] for self-hosted gateways.
    pub fn with_api_key(api_key: String) -> Self {
        let provider = AnthropicProvider::new(api_key);
        Self::new("https://api.anthropic.com", provider)
    }

    /// Creates an Anthropic client with a custom base URL and API key.
    ///
    /// Functional scope: Useful for routing through enterprise gateways (e.g. an
    /// Anthropic-compatible proxy or a regional endpoint).
    pub fn with_base_url(base_url: &str, api_key: String) -> Self {
        let provider = AnthropicProvider::new(api_key);
        Self::new(base_url, provider)
    }
}

#[cfg(test)]
mod tests {
    use serial_test::serial;

    use super::*;

    /// Scenario: Debug formatting must never leak the API key, otherwise it would
    /// surface in panic backtraces and `tracing` spans during incident debugging.
    #[test]
    fn test_anthropic_provider_debug() {
        let provider = AnthropicProvider::new("sk-ant-test-key".to_string());
        let debug_str = format!("{:?}", provider);
        assert!(!debug_str.contains("sk-ant-test-key"));
        assert!(debug_str.contains("***"));
    }

    /// Scenario: `api_key()` is the documented escape hatch for tests and
    /// alternative auth flows. This guards the round-trip from constructor to getter.
    #[test]
    fn test_anthropic_provider_api_key() {
        let provider = AnthropicProvider::new("sk-ant-test-key".to_string());
        assert_eq!(provider.api_key(), "sk-ant-test-key");
    }

    /// Scenario: `from_resolved_env` reads `ANTHROPIC_API_KEY` from the
    /// process environment when it is set, mirroring the legacy `from_env`
    /// fast path and producing a usable client.
    #[tokio::test]
    #[serial]
    async fn from_resolved_env_reads_anthropic_api_key_from_process_env() {
        let key_guard = TestEnvGuard::set("ANTHROPIC_API_KEY", Some("sk-ant-test-resolved"));
        let base_guard = TestEnvGuard::set("ANTHROPIC_BASE_URL", None);
        // Redirect global config to a path that does not exist so the test
        // does not depend on the developer's real `~/.libra/config.db`.
        let global_guard = TestEnvGuard::set(
            "LIBRA_CONFIG_GLOBAL_DB",
            Some("/nonexistent/anthropic-from-resolved-env-test.db"),
        );

        let client = Client::from_resolved_env(LocalIdentityTarget::None)
            .await
            .expect("from_resolved_env should succeed when ANTHROPIC_API_KEY is set");
        assert_eq!(client.provider.api_key(), "sk-ant-test-resolved");

        drop(key_guard);
        drop(base_guard);
        drop(global_guard);
    }

    /// Scenario: `from_resolved_env` returns a clear error referencing the
    /// missing env var when no cascade layer (process env, repo vault, global
    /// vault) supplies `ANTHROPIC_API_KEY`.
    #[tokio::test]
    #[serial]
    async fn from_resolved_env_errors_when_no_layer_supplies_api_key() {
        let key_guard = TestEnvGuard::set("ANTHROPIC_API_KEY", None);
        let base_guard = TestEnvGuard::set("ANTHROPIC_BASE_URL", None);
        let global_guard = TestEnvGuard::set(
            "LIBRA_CONFIG_GLOBAL_DB",
            Some("/nonexistent/anthropic-from-resolved-env-test.db"),
        );

        let err = Client::from_resolved_env(LocalIdentityTarget::None)
            .await
            .expect_err("from_resolved_env must fail without an API key");
        let message = err.to_string();
        assert!(
            message.contains("ANTHROPIC_API_KEY"),
            "error should name the missing key, got: {message}"
        );

        drop(key_guard);
        drop(base_guard);
        drop(global_guard);
    }

    /// RAII guard that sets an env var for the duration of a test and
    /// restores the previous value (or removes the variable when there was
    /// none) on drop. Pairs with `#[serial]` to keep concurrent unit tests
    /// from racing on the same env var name.
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
            // SAFETY: see `set`. The guard runs after the test body and the
            // serial gate guarantees no concurrent reader.
            unsafe {
                match &self.original {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }
}
