//! OpenAI API client for libra.

use std::fmt;

use crate::internal::ai::client::{Client as GenericClient, Provider};

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
    pub fn from_env() -> Result<Self, std::env::VarError> {
        let api_key = std::env::var("OPENAI_API_KEY")?;
        let base_url = std::env::var("OPENAI_BASE_URL")
            .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());

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
}
