//! DeepSeek API client for libra.
//!
//! Provides the [`Client`] type (a specialization of the generic
//! [`crate::internal::ai::client::Client`]) and the [`DeepSeekProvider`]
//! that injects Bearer-token authentication into every outgoing request.
//!
//! The default base URL is `https://api.deepseek.com`; a custom URL can be
//! supplied via [`Client::with_base_url`].

use anyhow::Result;

use crate::internal::{
    ai::client::{Client as GenericClient, Provider},
    config::{LocalIdentityTarget, resolve_env_for_target},
};

/// DeepSeek API provider.
///
/// Holds the API key and implements the [`Provider`] trait so that every
/// HTTP request sent through the generic client is authenticated with a
/// `Bearer` token in the `Authorization` header.
#[derive(Clone)]
pub struct DeepSeekProvider {
    api_key: String,
}

impl std::fmt::Debug for DeepSeekProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeepSeekProvider")
            .field("api_key", &"***")
            .finish()
    }
}

impl DeepSeekProvider {
    /// Creates a new DeepSeek provider with the given API key.
    ///
    /// Functional scope: stores the API key after passing it through
    /// [`normalize_api_key`] so that pasted shell-quoted values
    /// (`'...'`, `"..."`) and a leading `Bearer ` prefix are stripped before
    /// the key reaches `Authorization`.
    pub fn new(api_key: String) -> Self {
        Self {
            api_key: normalize_api_key(&api_key),
        }
    }

    /// Returns the API key.
    pub fn api_key(&self) -> &str {
        &self.api_key
    }
}

/// Attaches the `Authorization: Bearer <api_key>` header to every outgoing
/// request, which is the authentication scheme required by the DeepSeek API.
impl Provider for DeepSeekProvider {
    fn on_request(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        // DeepSeek uses standard HTTP bearer authentication.
        request.bearer_auth(&self.api_key)
    }
}

/// Strip common pasting artefacts from a DeepSeek API key.
///
/// Functional scope:
/// - Trims surrounding whitespace.
/// - Removes a `Bearer ` / `bearer ` prefix copied from documentation.
/// - Removes a balanced pair of single or double quotes from a shell-style paste
///   (`'sk-...'`, `"sk-..."`).
///
/// Boundary conditions:
/// - Quoting is only stripped when both ends match; mismatched quotes are left in
///   place so the user sees the auth failure rather than silently mangling the key.
/// - Empty strings are returned untouched; callers handle the missing-key case.
fn normalize_api_key(api_key: &str) -> String {
    let trimmed = api_key.trim();
    let without_scheme = trimmed
        .strip_prefix("Bearer ")
        .or_else(|| trimmed.strip_prefix("bearer "))
        .unwrap_or(trimmed)
        .trim();

    if without_scheme.len() >= 2 {
        let bytes = without_scheme.as_bytes();
        let is_single_quoted = bytes.first() == Some(&b'\'') && bytes.last() == Some(&b'\'');
        let is_double_quoted = bytes.first() == Some(&b'"') && bytes.last() == Some(&b'"');
        if is_single_quoted || is_double_quoted {
            return without_scheme[1..without_scheme.len() - 1]
                .trim()
                .to_string();
        }
    }

    without_scheme.to_string()
}

/// DeepSeek client type.
///
/// A type alias for the generic [`crate::internal::ai::client::Client`]
/// parameterized with [`DeepSeekProvider`]. Use [`Client::from_env`] to
/// construct from the `DEEPSEEK_API_KEY` environment variable, or
/// [`Client::with_api_key`] / [`Client::with_base_url`] for programmatic
/// construction.
pub type Client = GenericClient<DeepSeekProvider>;

impl Client {
    /// Creates a DeepSeek client from environment variables or Vault.
    ///
    /// Priority chain (12-Factor, see `docs/improvement/config.md`):
    /// 1. Process env `DEEPSEEK_API_KEY`
    /// 2. Local repo config (`vault.env.DEEPSEEK_API_KEY`)
    /// 3. Global config (`vault.env.DEEPSEEK_API_KEY`)
    ///
    /// Uses the default base URL (`https://api.deepseek.com`). DeepSeek does
    /// **not** honor a `DEEPSEEK_BASE_URL` env var; override the endpoint via
    /// [`Client::with_base_url`] or the `--api-base` CLI flag (which routes
    /// through `ProviderBuildOptions::api_base` in `providers::factory`).
    ///
    /// New call sites should prefer [`Client::from_resolved_env`], which
    /// performs the same lookup chain asynchronously and accepts an
    /// explicit `LocalIdentityTarget<'_>` so vault values from a specific
    /// repository are honored. `from_env` is retained for backward
    /// compatibility and currently delegates to the same vault-aware
    /// resolvers.
    ///
    /// # Errors
    ///
    /// Returns an actionable error if `DEEPSEEK_API_KEY` is not configured.
    pub fn from_env() -> anyhow::Result<Self> {
        let api_key = crate::internal::config::resolve_required_env_sync("DEEPSEEK_API_KEY")?;
        let base_url = "https://api.deepseek.com".to_string();

        let provider = DeepSeekProvider::new(api_key);
        Ok(Self::new(&base_url, provider))
    }

    /// Vault-aware async constructor: resolves `DEEPSEEK_API_KEY` via
    /// [`resolve_env_for_target`], so callers can store the key in repo-local
    /// or global `vault.env.DEEPSEEK_API_KEY` config without exporting it to
    /// the process environment.
    ///
    /// Priority order (12-Factor):
    /// 1. Process env `DEEPSEEK_API_KEY`
    /// 2. Local repo config (`vault.env.DEEPSEEK_API_KEY`)
    /// 3. Global config (`vault.env.DEEPSEEK_API_KEY`)
    ///
    /// DeepSeek does not expose a base-URL env override. Tests that need a
    /// stub endpoint should keep using [`Client::with_base_url`].
    pub async fn from_resolved_env(local_target: LocalIdentityTarget<'_>) -> Result<Self> {
        let api_key = resolve_env_for_target("DEEPSEEK_API_KEY", local_target)
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "DEEPSEEK_API_KEY is not configured; set vault.env.DEEPSEEK_API_KEY with \
                     `libra config set vault.env.DEEPSEEK_API_KEY <key>` or export DEEPSEEK_API_KEY"
                )
            })?;
        let provider = DeepSeekProvider::new(api_key);
        Ok(Self::new("https://api.deepseek.com", provider))
    }

    /// Creates a DeepSeek client with the given API key and the default
    /// base URL (`https://api.deepseek.com`).
    pub fn with_api_key(api_key: String) -> Self {
        let provider = DeepSeekProvider::new(api_key);
        Self::new("https://api.deepseek.com", provider)
    }

    /// Creates a DeepSeek client with a custom base URL and API key.
    ///
    /// Use this constructor when targeting a self-hosted or proxy endpoint
    /// that is compatible with the DeepSeek API.
    pub fn with_base_url(base_url: &str, api_key: String) -> Self {
        let provider = DeepSeekProvider::new(api_key);
        Self::new(base_url, provider)
    }
}

#[cfg(test)]
mod tests {
    use serial_test::serial;

    use super::*;

    /// Scenario: Debug formatting must mask the secret so it cannot leak into
    /// `tracing` output or panic backtraces.
    #[test]
    fn test_deepseek_provider_debug() {
        let provider = DeepSeekProvider::new("test-key".to_string());
        let debug_str = format!("{:?}", provider);
        assert!(!debug_str.contains("test-key"));
        assert!(debug_str.contains("***"));
    }

    /// Scenario: a clean key must round-trip through the constructor unchanged.
    #[test]
    fn test_deepseek_provider_api_key() {
        let provider = DeepSeekProvider::new("test-key".to_string());
        assert_eq!(provider.api_key(), "test-key");
    }

    /// Scenario: keys pasted from shell scripts often arrive with surrounding
    /// quotes and trailing whitespace. The normaliser must strip both before
    /// the value reaches the `Authorization` header.
    #[test]
    fn test_deepseek_provider_normalizes_shell_quoted_api_key() {
        let provider = DeepSeekProvider::new(" 'test-key' \n".to_string());
        assert_eq!(provider.api_key(), "test-key");
    }

    /// Scenario: documentation samples sometimes embed `Bearer ` in the key;
    /// the normaliser strips it so users do not produce a header with two
    /// `Bearer` tokens.
    #[test]
    fn test_deepseek_provider_normalizes_bearer_prefixed_api_key() {
        let provider = DeepSeekProvider::new("Bearer test-key".to_string());
        assert_eq!(provider.api_key(), "test-key");
    }

    #[tokio::test]
    #[serial]
    async fn from_resolved_env_reads_deepseek_api_key_from_process_env() {
        let key_guard = TestEnvGuard::set("DEEPSEEK_API_KEY", Some("ds-test-resolved"));
        let global_guard = TestEnvGuard::set(
            "LIBRA_CONFIG_GLOBAL_DB",
            Some("/nonexistent/deepseek-from-resolved-env-test.db"),
        );

        let client = Client::from_resolved_env(LocalIdentityTarget::None)
            .await
            .expect("from_resolved_env should succeed when DEEPSEEK_API_KEY is set");
        assert_eq!(client.provider.api_key(), "ds-test-resolved");

        drop(key_guard);
        drop(global_guard);
    }

    /// Per docs/improvement/config.md (12-Factor priority), process env wins
    /// over the global vault — `DEEPSEEK_API_KEY=B libra ...` is the sacred
    /// per-invocation override. Global vault is the fallback when env is
    /// unset. Mirrors v0.17.906's config_test::resolve_env_for_target_process_
    /// env_overrides_global_vault fix.
    #[test]
    #[serial]
    fn from_env_process_env_overrides_global_vault() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let key_guard = TestEnvGuard::set("DEEPSEEK_API_KEY", Some("ds-env-key"));
        let global_dir = tempfile::tempdir().unwrap();
        let global_db_path = global_dir.path().join("deepseek-global-config.db");
        let global_db_string = global_db_path.to_string_lossy().into_owned();
        let global_guard = TestEnvGuard::set("LIBRA_CONFIG_GLOBAL_DB", Some(&global_db_string));

        let global_conn = rt
            .block_on(crate::internal::db::create_database(&global_db_string))
            .unwrap();
        rt.block_on(crate::internal::config::ConfigKv::set_with_conn(
            &global_conn,
            "vault.env.DEEPSEEK_API_KEY",
            "ds-vault-key",
            false,
        ))
        .unwrap();

        // env wins.
        let client = Client::from_env().expect("from_env should pick up env DEEPSEEK_API_KEY");
        assert_eq!(client.provider.api_key(), "ds-env-key");

        // …and vault is the fallback when env is unset.
        drop(key_guard);
        let key_unset = TestEnvGuard::set("DEEPSEEK_API_KEY", None);
        let client = Client::from_env().expect("from_env should fall back to vault");
        assert_eq!(client.provider.api_key(), "ds-vault-key");

        drop(key_unset);
        drop(global_guard);
    }

    #[tokio::test]
    #[serial]
    async fn from_resolved_env_errors_when_no_layer_supplies_api_key() {
        let key_guard = TestEnvGuard::set("DEEPSEEK_API_KEY", None);
        let global_guard = TestEnvGuard::set(
            "LIBRA_CONFIG_GLOBAL_DB",
            Some("/nonexistent/deepseek-from-resolved-env-test.db"),
        );

        let err = Client::from_resolved_env(LocalIdentityTarget::None)
            .await
            .expect_err("from_resolved_env must fail without an API key");
        assert!(
            err.to_string().contains("DEEPSEEK_API_KEY"),
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
