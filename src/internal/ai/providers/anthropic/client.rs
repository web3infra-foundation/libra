//! Anthropic API client for libra.

use std::fmt;

use crate::internal::ai::client::{Client as GenericClient, Provider};

/// Anthropic API provider.
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
    pub fn new(api_key: String) -> Self {
        Self { api_key }
    }

    /// Returns the API key.
    pub fn api_key(&self) -> &str {
        &self.api_key
    }
}

impl Provider for AnthropicProvider {
    fn on_request(&self, mut request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        // Anthropic uses x-api-key header (not Bearer token)
        request = request.header("x-api-key", &self.api_key);
        // Anthropic requires anthropic-version header
        request = request.header("anthropic-version", super::ANTHROPIC_VERSION);
        request
    }
}

/// Anthropic client type.
pub type Client = GenericClient<AnthropicProvider>;

impl Client {
    /// Creates an Anthropic client from environment variables.
    ///
    /// Reads the `ANTHROPIC_API_KEY` environment variable.
    /// Also supports `ANTHROPIC_BASE_URL` for custom endpoints.
    pub fn from_env() -> Result<Self, std::env::VarError> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")?;
        let base_url = std::env::var("ANTHROPIC_BASE_URL")
            .unwrap_or_else(|_| "https://api.anthropic.com".to_string());

        let provider = AnthropicProvider::new(api_key);
        Ok(Self::new(&base_url, provider))
    }

    /// Creates an Anthropic client with the given API key.
    pub fn with_api_key(api_key: String) -> Self {
        let provider = AnthropicProvider::new(api_key);
        Self::new("https://api.anthropic.com", provider)
    }

    /// Creates an Anthropic client with a custom base URL and API key.
    pub fn with_base_url(base_url: &str, api_key: String) -> Self {
        let provider = AnthropicProvider::new(api_key);
        Self::new(base_url, provider)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_anthropic_provider_debug() {
        let provider = AnthropicProvider::new("sk-ant-test-key".to_string());
        let debug_str = format!("{:?}", provider);
        assert!(!debug_str.contains("sk-ant-test-key"));
        assert!(debug_str.contains("***"));
    }

    #[test]
    fn test_anthropic_provider_api_key() {
        let provider = AnthropicProvider::new("sk-ant-test-key".to_string());
        assert_eq!(provider.api_key(), "sk-ant-test-key");
    }
}
