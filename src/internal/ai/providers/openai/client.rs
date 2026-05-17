//! OpenAI API client for libra.
//!
//! This module provides [`OpenAIProvider`] and a [`Client`] type alias that together
//! implement authenticated access to the OpenAI REST API. Authentication uses the
//! standard **Bearer token** scheme: every outgoing HTTP request receives an
//! `Authorization: Bearer <api_key>` header via the [`Provider`] trait implementation.
//!
//! # Client construction
//!
//! There are three ways to create a client:
//!
//! - [`Client::from_env`] -- reads `OPENAI_API_KEY` (and optionally `OPENAI_BASE_URL`)
//!   from environment variables. This is the recommended path for CLI usage.
//! - [`Client::with_api_key`] -- uses the default `https://api.openai.com/v1` base URL.
//! - [`Client::with_base_url`] -- allows pointing at a custom or proxy endpoint while
//!   still using OpenAI-compatible authentication.

use std::fmt;

use anyhow::Result;

use crate::internal::{
    ai::client::{Client as GenericClient, Provider},
    config::{LocalIdentityTarget, resolve_env_for_target},
};

/// OpenAI API provider.
#[derive(Clone)]
pub struct OpenAIProvider {
    api_key: String,
}

impl fmt::Debug for OpenAIProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenAIProvider")
            .field("api_key", &"***")
            .finish()
    }
}

impl OpenAIProvider {
    /// Creates a new OpenAI provider with the given API key.
    pub fn new(api_key: String) -> Self {
        Self { api_key }
    }

    /// Returns the API key.
    pub fn api_key(&self) -> &str {
        &self.api_key
    }
}

/// Implements the [`Provider`] trait to inject OpenAI-specific authentication.
///
/// Each outgoing request is augmented with an `Authorization: Bearer <api_key>`
/// header, which is the authentication scheme required by the OpenAI REST API.
impl Provider for OpenAIProvider {
    fn on_request(&self, mut request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        // OpenAI uses Bearer token authentication
        request = request.header(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", self.api_key),
        );
        request
    }
}

/// OpenAI client type.
pub type Client = GenericClient<OpenAIProvider>;

impl Client {
    /// Creates an OpenAI client from environment variables.
    ///
    /// Reads the `OPENAI_API_KEY` environment variable.
    /// Also supports `OPENAI_BASE_URL` for custom endpoints.
    pub fn from_env() -> std::result::Result<Self, std::env::VarError> {
        let api_key = std::env::var("OPENAI_API_KEY")?;
        let base_url = std::env::var("OPENAI_BASE_URL")
            .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());

        let provider = OpenAIProvider::new(api_key);
        Ok(Self::new(&base_url, provider))
    }

    /// Vault-aware async constructor: resolves `OPENAI_API_KEY` and
    /// `OPENAI_BASE_URL` via [`resolve_env_for_target`], so callers can
    /// store credentials in repo-local or global `vault.env.*` config without
    /// also exporting them into the process environment.
    ///
    /// Priority order matches the rest of Libra's config cascade:
    /// 1. Process env var
    /// 2. Local repo config (`vault.env.OPENAI_API_KEY` / `vault.env.OPENAI_BASE_URL`)
    /// 3. Global config
    ///
    /// `OPENAI_BASE_URL` falls back to the canonical OpenAI v1 URL when no
    /// layer supplies it. `from_env` is preserved as a low-level process-env
    /// only API.
    pub async fn from_resolved_env(local_target: LocalIdentityTarget<'_>) -> Result<Self> {
        let api_key = resolve_env_for_target("OPENAI_API_KEY", local_target)
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!("OPENAI_API_KEY is not set in env, repo vault, or global config")
            })?;
        let base_url = resolve_env_for_target("OPENAI_BASE_URL", local_target)
            .await?
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string());

        let provider = OpenAIProvider::new(api_key);
        Ok(Self::new(&base_url, provider))
    }

    /// Creates an OpenAI client with the given API key.
    pub fn with_api_key(api_key: String) -> Self {
        let provider = OpenAIProvider::new(api_key);
        Self::new("https://api.openai.com/v1", provider)
    }

    /// Creates an OpenAI client with a custom base URL and API key.
    pub fn with_base_url(base_url: &str, api_key: String) -> Self {
        let provider = OpenAIProvider::new(api_key);
        Self::new(base_url, provider)
    }
}

#[cfg(test)]
mod tests {
    use serial_test::serial;

    use super::*;

    #[test]
    fn test_openai_provider_debug() {
        let provider = OpenAIProvider::new("sk-test-key".to_string());
        let debug_str = format!("{:?}", provider);
        assert!(!debug_str.contains("sk-test-key"));
        assert!(debug_str.contains("***"));
    }

    #[test]
    fn test_openai_provider_api_key() {
        let provider = OpenAIProvider::new("sk-test-key".to_string());
        assert_eq!(provider.api_key(), "sk-test-key");
    }

    #[tokio::test]
    #[serial]
    async fn from_resolved_env_reads_openai_api_key_from_process_env() {
        let key_guard = TestEnvGuard::set("OPENAI_API_KEY", Some("sk-test-resolved"));
        let base_guard = TestEnvGuard::set("OPENAI_BASE_URL", None);
        let global_guard = TestEnvGuard::set(
            "LIBRA_CONFIG_GLOBAL_DB",
            Some("/nonexistent/openai-from-resolved-env-test.db"),
        );

        let client = Client::from_resolved_env(LocalIdentityTarget::None)
            .await
            .expect("from_resolved_env should succeed when OPENAI_API_KEY is set");
        assert_eq!(client.provider.api_key(), "sk-test-resolved");

        drop(key_guard);
        drop(base_guard);
        drop(global_guard);
    }

    #[tokio::test]
    #[serial]
    async fn from_resolved_env_errors_when_no_layer_supplies_api_key() {
        let key_guard = TestEnvGuard::set("OPENAI_API_KEY", None);
        let base_guard = TestEnvGuard::set("OPENAI_BASE_URL", None);
        let global_guard = TestEnvGuard::set(
            "LIBRA_CONFIG_GLOBAL_DB",
            Some("/nonexistent/openai-from-resolved-env-test.db"),
        );

        let err = Client::from_resolved_env(LocalIdentityTarget::None)
            .await
            .expect_err("from_resolved_env must fail without an API key");
        let message = err.to_string();
        assert!(
            message.contains("OPENAI_API_KEY"),
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
