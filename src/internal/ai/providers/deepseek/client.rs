//! DeepSeek API client for libra.
//!
//! Provides the [`Client`] type (a specialization of the generic
//! [`crate::internal::ai::client::Client`]) and the [`DeepSeekProvider`]
//! that injects Bearer-token authentication into every outgoing request.
//!
//! The default base URL is `https://api.deepseek.com`; a custom URL can be
//! supplied via [`Client::with_base_url`].

use crate::internal::ai::client::{Client as GenericClient, Provider};

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
    pub fn new(api_key: String) -> Self {
        Self { api_key }
    }

    /// Returns the API key.
    pub fn api_key(&self) -> &str {
        &self.api_key
    }
}

/// Attaches the `Authorization: Bearer <api_key>` header to every outgoing
/// request, which is the authentication scheme required by the DeepSeek API.
impl Provider for DeepSeekProvider {
    fn on_request(&self, mut request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        // DeepSeek uses Bearer token authentication
        request = request.header(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", self.api_key),
        );
        request
    }
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
    /// Creates a DeepSeek client from environment variables.
    ///
    /// Reads the `DEEPSEEK_API_KEY` environment variable and uses the default
    /// base URL (`https://api.deepseek.com`).
    ///
    /// # Errors
    ///
    /// Returns [`std::env::VarError`] if `DEEPSEEK_API_KEY` is not set.
    pub fn from_env() -> Result<Self, std::env::VarError> {
        let api_key = std::env::var("DEEPSEEK_API_KEY")?;
        let base_url = "https://api.deepseek.com".to_string();

        let provider = DeepSeekProvider::new(api_key);
        Ok(Self::new(&base_url, provider))
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
    use super::*;

    #[test]
    fn test_deepseek_provider_debug() {
        let provider = DeepSeekProvider::new("test-key".to_string());
        let debug_str = format!("{:?}", provider);
        assert!(!debug_str.contains("test-key"));
        assert!(debug_str.contains("***"));
    }

    #[test]
    fn test_deepseek_provider_api_key() {
        let provider = DeepSeekProvider::new("test-key".to_string());
        assert_eq!(provider.api_key(), "test-key");
    }
}
