//! DeepSeek API client.

use crate::internal::ai::client::{Client as GenericClient, Provider};

/// DeepSeek API provider.
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
pub type Client = GenericClient<DeepSeekProvider>;

impl Client {
    /// Creates a DeepSeek client from environment variables.
    ///
    /// Reads the `DEEPSEEK_API_KEY` environment variable.
    pub fn from_env() -> Result<Self, std::env::VarError> {
        let api_key = std::env::var("DEEPSEEK_API_KEY")?;
        let base_url = "https://api.deepseek.com".to_string();

        let provider = DeepSeekProvider::new(api_key);
        Ok(Self::new(&base_url, provider))
    }

    /// Creates a DeepSeek client with the given API key.
    pub fn with_api_key(api_key: String) -> Self {
        let provider = DeepSeekProvider::new(api_key);
        Self::new("https://api.deepseek.com", provider)
    }

    /// Creates a DeepSeek client with a custom base URL and API key.
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
