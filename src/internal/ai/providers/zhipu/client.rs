//! Zhipu API client for libra.
//!
//! Zhipu AI (also known as GLM / ChatGLM) is a Chinese AI research lab that provides
//! large language models through a cloud API. Authentication is performed via a Bearer
//! token included in the `Authorization` header of each HTTP request. The default base
//! URL points to `https://open.bigmodel.cn/api/paas/v4`.

use std::fmt;

use crate::internal::ai::client::{Client as GenericClient, Provider};

/// Zhipu API provider.
#[derive(Clone)]
pub struct ZhipuProvider {
    api_key: String,
}

impl fmt::Debug for ZhipuProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ZhipuProvider")
            .field("api_key", &"***")
            .finish()
    }
}

impl ZhipuProvider {
    /// Creates a new Zhipu provider with the given API key.
    pub fn new(api_key: String) -> Self {
        Self { api_key }
    }

    /// Returns the API key.
    pub fn api_key(&self) -> &str {
        &self.api_key
    }
}

/// Implements the [`Provider`] trait so that every outgoing HTTP request is
/// annotated with a `Bearer <api_key>` authorization header, which is the
/// authentication scheme required by the Zhipu API.
impl Provider for ZhipuProvider {
    fn on_request(&self, mut request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        // Zhipu uses Bearer token authentication
        request = request.header(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", self.api_key),
        );
        request
    }
}

/// Zhipu client type.
pub type Client = GenericClient<ZhipuProvider>;

impl Client {
    /// Creates a Zhipu client from environment variables.
    ///
    /// Reads the `ZHIPU_API_KEY` environment variable.
    /// Also supports `ZHIPU_BASE_URL` for custom endpoints.
    ///
    /// Note: unlike the other providers (anthropic / deepseek / gemini /
    /// kimi / ollama / openai), this constructor reads process env
    /// **directly** and does not consult vault. New call sites that need
    /// vault-aware resolution should prefer [`Client::from_resolved_env`].
    /// `from_env` is retained for backward compatibility.
    pub fn from_env() -> Result<Self, std::env::VarError> {
        let api_key = std::env::var("ZHIPU_API_KEY")?;
        let base_url = std::env::var("ZHIPU_BASE_URL")
            .unwrap_or_else(|_| "https://open.bigmodel.cn/api/paas/v4".to_string());

        let provider = ZhipuProvider::new(api_key);
        Ok(Self::new(&base_url, provider))
    }

    /// Vault-aware async constructor: resolves `ZHIPU_API_KEY` (required)
    /// and `ZHIPU_BASE_URL` (optional override) through the libra-aware
    /// lookup chain. See deepseek / gemini / openai / anthropic
    /// `from_resolved_env` for the shared contract; `ZHIPU_BASE_URL`
    /// defaults to `https://open.bigmodel.cn/api/paas/v4`.
    pub async fn from_resolved_env(
        local_target: crate::internal::config::LocalIdentityTarget<'_>,
    ) -> anyhow::Result<Self> {
        use anyhow::anyhow;

        use crate::internal::config::resolve_env_for_target;

        let api_key = resolve_env_for_target("ZHIPU_API_KEY", local_target)
            .await?
            .ok_or_else(|| {
                anyhow!(
                    "ZHIPU_API_KEY is not set in env, repo vault, or global config \
                     (set the environment variable or run `libra config --global add \
                     vault.env.ZHIPU_API_KEY <key>`)"
                )
            })?;
        let base_url = resolve_env_for_target("ZHIPU_BASE_URL", local_target)
            .await?
            .unwrap_or_else(|| "https://open.bigmodel.cn/api/paas/v4".to_string());

        let provider = ZhipuProvider::new(api_key);
        Ok(Self::new(&base_url, provider))
    }

    /// Creates a Zhipu client with the given API key.
    pub fn with_api_key(api_key: String) -> Self {
        let provider = ZhipuProvider::new(api_key);
        Self::new("https://open.bigmodel.cn/api/paas/v4", provider)
    }

    /// Creates a Zhipu client with a custom base URL and API key.
    pub fn with_base_url(base_url: &str, api_key: String) -> Self {
        let provider = ZhipuProvider::new(api_key);
        Self::new(base_url, provider)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zhipu_provider_debug() {
        let provider = ZhipuProvider::new("test-key".to_string());
        let debug_str = format!("{:?}", provider);
        assert!(!debug_str.contains("test-key"));
        assert!(debug_str.contains("***"));
    }

    #[test]
    fn test_zhipu_provider_api_key() {
        let provider = ZhipuProvider::new("test-key".to_string());
        assert_eq!(provider.api_key(), "test-key");
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn from_resolved_env_reads_zhipu_api_key_from_process_env() {
        let key_guard = TestEnvGuard::set("ZHIPU_API_KEY", Some("zh-test-resolved"));
        let base_guard = TestEnvGuard::set("ZHIPU_BASE_URL", None);
        let global_guard = TestEnvGuard::set(
            "LIBRA_CONFIG_GLOBAL_DB",
            Some("/nonexistent/zhipu-from-resolved-env-test.db"),
        );

        let client = Client::from_resolved_env(crate::internal::config::LocalIdentityTarget::None)
            .await
            .expect("from_resolved_env should succeed when ZHIPU_API_KEY is set");
        assert_eq!(client.provider.api_key(), "zh-test-resolved");

        drop(key_guard);
        drop(base_guard);
        drop(global_guard);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn from_resolved_env_errors_when_no_layer_supplies_api_key() {
        let key_guard = TestEnvGuard::set("ZHIPU_API_KEY", None);
        let base_guard = TestEnvGuard::set("ZHIPU_BASE_URL", None);
        let global_guard = TestEnvGuard::set(
            "LIBRA_CONFIG_GLOBAL_DB",
            Some("/nonexistent/zhipu-from-resolved-env-test.db"),
        );

        let err = Client::from_resolved_env(crate::internal::config::LocalIdentityTarget::None)
            .await
            .expect_err("from_resolved_env must fail without an API key");
        assert!(
            err.to_string().contains("ZHIPU_API_KEY"),
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
            // SAFETY: tests are serialized via `#[serial_test::serial]`.
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
            unsafe {
                match &self.original {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }
}
