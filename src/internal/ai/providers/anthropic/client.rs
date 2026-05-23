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
//! credentials from Vault/environment (`ANTHROPIC_API_KEY`) or accept them
//! directly.

use std::fmt;

use crate::internal::ai::client::{Client as GenericClient, Provider};

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
    /// Creates an Anthropic client from Vault or environment variables.
    ///
    /// Functional scope:
    /// - Reads `vault.env.ANTHROPIC_API_KEY`, then `ANTHROPIC_API_KEY` (required).
    /// - Falls back to `https://api.anthropic.com` when neither
    ///   `vault.env.ANTHROPIC_BASE_URL` nor `ANTHROPIC_BASE_URL` is set.
    ///
    /// Boundary conditions:
    /// - Returns an actionable error when `ANTHROPIC_API_KEY` is missing across
    ///   Vault and process env.
    /// - `ANTHROPIC_BASE_URL`, when set, is forwarded verbatim — no scheme validation.
    ///
    /// New call sites should prefer [`Client::from_resolved_env`], which
    /// performs the same lookup chain asynchronously and accepts an
    /// explicit `LocalIdentityTarget<'_>` so vault values from a specific
    /// repository are honored. `from_env` is retained for backward
    /// compatibility and currently delegates to the same vault-aware
    /// resolvers (`resolve_required_env_sync` / `resolve_optional_env_sync`).
    pub fn from_env() -> anyhow::Result<Self> {
        let api_key = crate::internal::config::resolve_required_env_sync("ANTHROPIC_API_KEY")?;
        let base_url = crate::internal::config::resolve_optional_env_sync("ANTHROPIC_BASE_URL")?
            .unwrap_or_else(|| "https://api.anthropic.com".to_string());

        let provider = AnthropicProvider::new(api_key);
        Ok(Self::new(&base_url, provider))
    }

    /// Vault-aware async constructor: resolves `ANTHROPIC_API_KEY` (required)
    /// and `ANTHROPIC_BASE_URL` (optional override) through the libra-aware
    /// lookup chain: local `.libra/libra.db` (`vault.env.<name>`, when
    /// `local_target` selects a repo) → global `~/.libra/config.db` →
    /// process env.
    ///
    /// Mirrors the deepseek / gemini / openai `from_resolved_env`
    /// signatures — same `LocalIdentityTarget<'_>` parameter, same
    /// `anyhow::Result<Self>` return type — so call sites can pick a
    /// provider without branching on constructor shape.
    ///
    /// `ANTHROPIC_BASE_URL` defaults to `https://api.anthropic.com` when
    /// no layer supplies a value — matching `from_env`'s fallback exactly
    /// so migrated callers get byte-equivalent endpoints.
    pub async fn from_resolved_env(
        local_target: crate::internal::config::LocalIdentityTarget<'_>,
    ) -> anyhow::Result<Self> {
        use anyhow::anyhow;

        use crate::internal::config::resolve_env_for_target;

        let api_key = resolve_env_for_target("ANTHROPIC_API_KEY", local_target)
            .await?
            .ok_or_else(|| {
                anyhow!(
                    "ANTHROPIC_API_KEY is not configured; set vault.env.ANTHROPIC_API_KEY \
                     with `libra config set vault.env.ANTHROPIC_API_KEY <key>` or export \
                     ANTHROPIC_API_KEY"
                )
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

    #[tokio::test]
    #[serial_test::serial]
    async fn from_resolved_env_reads_anthropic_api_key_from_process_env() {
        let key_guard = TestEnvGuard::set("ANTHROPIC_API_KEY", Some("sk-ant-test-resolved"));
        let base_guard = TestEnvGuard::set("ANTHROPIC_BASE_URL", None);
        let global_guard = TestEnvGuard::set(
            "LIBRA_CONFIG_GLOBAL_DB",
            Some("/nonexistent/anthropic-from-resolved-env-test.db"),
        );

        let client = Client::from_resolved_env(crate::internal::config::LocalIdentityTarget::None)
            .await
            .expect("from_resolved_env should succeed when ANTHROPIC_API_KEY is set");
        assert_eq!(client.provider.api_key(), "sk-ant-test-resolved");

        drop(key_guard);
        drop(base_guard);
        drop(global_guard);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn from_resolved_env_errors_when_no_layer_supplies_api_key() {
        let key_guard = TestEnvGuard::set("ANTHROPIC_API_KEY", None);
        let base_guard = TestEnvGuard::set("ANTHROPIC_BASE_URL", None);
        let global_guard = TestEnvGuard::set(
            "LIBRA_CONFIG_GLOBAL_DB",
            Some("/nonexistent/anthropic-from-resolved-env-test.db"),
        );

        let err = Client::from_resolved_env(crate::internal::config::LocalIdentityTarget::None)
            .await
            .expect_err("from_resolved_env must fail without an API key");
        assert!(
            err.to_string().contains("ANTHROPIC_API_KEY"),
            "error should name the missing key, got: {err}"
        );

        drop(key_guard);
        drop(base_guard);
        drop(global_guard);
    }

    struct TestEnvGuard {
        key: &'static str,
        original: Option<std::ffi::OsString>,
    }

    impl TestEnvGuard {
        fn set(key: &'static str, value: Option<&str>) -> Self {
            let original = std::env::var_os(key);
            // SAFETY: tests are serialized via `#[serial_test::serial]`; the
            // guard restores the previous value on drop.
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
            // SAFETY: see [`TestEnvGuard::set`].
            unsafe {
                match &self.original {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }
}
