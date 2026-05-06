//! Kimi (Moonshot AI) API client for libra.
//!
//! Provides the [`Client`] type (a specialization of the generic
//! [`crate::internal::ai::client::Client`]) and the [`KimiProvider`] that
//! injects Bearer-token authentication into every outgoing request.
//!
//! The default base URL is `https://api.moonshot.cn/v1`; a custom URL can be
//! supplied via [`Client::with_base_url`] or the `MOONSHOT_BASE_URL`
//! environment variable. Authentication uses `MOONSHOT_API_KEY`, matching the
//! official Kimi platform documentation.

use std::fmt;

use crate::internal::ai::client::{Client as GenericClient, Provider};

/// Kimi (Moonshot AI) API provider.
///
/// Holds the API key and implements the [`Provider`] trait so that every
/// HTTP request sent through the generic client is authenticated with a
/// `Bearer` token in the `Authorization` header.
#[derive(Clone)]
pub struct KimiProvider {
    api_key: String,
}

impl fmt::Debug for KimiProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KimiProvider")
            .field("api_key", &"***")
            .finish()
    }
}

impl KimiProvider {
    /// Creates a new Kimi provider with the given API key.
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
/// request, which is the authentication scheme required by the Kimi /
/// Moonshot API.
impl Provider for KimiProvider {
    fn on_request(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        // Kimi (Moonshot) uses standard HTTP bearer authentication.
        request.bearer_auth(&self.api_key)
    }
}

/// Strip common pasting artefacts from a Kimi (Moonshot) API key.
///
/// Functional scope:
/// - Trims surrounding whitespace.
/// - Removes a `Bearer ` / `bearer ` prefix copied from documentation.
/// - Removes a balanced pair of single or double quotes from a shell-style
///   paste (`'sk-...'`, `"sk-..."`).
///
/// Boundary conditions:
/// - Quoting is only stripped when both ends match; mismatched quotes are
///   left in place so the user sees the auth failure rather than silently
///   mangling the key.
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

/// Default base URL used when neither `MOONSHOT_BASE_URL` nor an explicit
/// override is supplied. The Kimi documentation lists this as the canonical
/// production endpoint.
const DEFAULT_BASE_URL: &str = "https://api.moonshot.cn/v1";

/// Kimi client type.
///
/// A type alias for the generic [`crate::internal::ai::client::Client`]
/// parameterized with [`KimiProvider`]. Use [`Client::from_env`] to construct
/// from the `MOONSHOT_API_KEY` environment variable, or
/// [`Client::with_api_key`] / [`Client::with_base_url`] for programmatic
/// construction.
pub type Client = GenericClient<KimiProvider>;

impl Client {
    /// Creates a Kimi client from environment variables.
    ///
    /// Reads `MOONSHOT_API_KEY` for authentication. The base URL defaults to
    /// `https://api.moonshot.cn/v1` and can be overridden with
    /// `MOONSHOT_BASE_URL` (useful for the international endpoint or a
    /// self-hosted proxy).
    ///
    /// # Errors
    ///
    /// Returns [`std::env::VarError`] if `MOONSHOT_API_KEY` is not set.
    pub fn from_env() -> Result<Self, std::env::VarError> {
        let api_key = std::env::var("MOONSHOT_API_KEY")?;
        let base_url =
            std::env::var("MOONSHOT_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());

        let provider = KimiProvider::new(api_key);
        Ok(Self::new(&base_url, provider))
    }

    /// Creates a Kimi client with the given API key and the default base URL
    /// (`https://api.moonshot.cn/v1`).
    pub fn with_api_key(api_key: String) -> Self {
        let provider = KimiProvider::new(api_key);
        Self::new(DEFAULT_BASE_URL, provider)
    }

    /// Creates a Kimi client with a custom base URL and API key.
    ///
    /// Use this constructor when targeting the international endpoint
    /// (`https://api.moonshot.ai/v1`) or a self-hosted proxy that is
    /// compatible with the Moonshot/Kimi API.
    pub fn with_base_url(base_url: &str, api_key: String) -> Self {
        let provider = KimiProvider::new(api_key);
        Self::new(base_url, provider)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scenario: Debug formatting must mask the secret so it cannot leak into
    /// `tracing` output or panic backtraces.
    #[test]
    fn test_kimi_provider_debug() {
        let provider = KimiProvider::new("sk-test-key".to_string());
        let debug_str = format!("{:?}", provider);
        assert!(!debug_str.contains("sk-test-key"));
        assert!(debug_str.contains("***"));
    }

    /// Scenario: a clean key must round-trip through the constructor unchanged.
    #[test]
    fn test_kimi_provider_api_key() {
        let provider = KimiProvider::new("sk-test-key".to_string());
        assert_eq!(provider.api_key(), "sk-test-key");
    }

    /// Scenario: keys pasted from shell scripts often arrive with surrounding
    /// quotes and trailing whitespace. The normaliser must strip both before
    /// the value reaches the `Authorization` header.
    #[test]
    fn test_kimi_provider_normalizes_shell_quoted_api_key() {
        let provider = KimiProvider::new(" 'sk-test-key' \n".to_string());
        assert_eq!(provider.api_key(), "sk-test-key");
    }

    /// Scenario: documentation samples sometimes embed `Bearer ` in the key;
    /// the normaliser strips it so users do not produce a header with two
    /// `Bearer` tokens.
    #[test]
    fn test_kimi_provider_normalizes_bearer_prefixed_api_key() {
        let provider = KimiProvider::new("Bearer sk-test-key".to_string());
        assert_eq!(provider.api_key(), "sk-test-key");
    }
}
