//! Zhipu API client for libra.

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
    pub fn from_env() -> Result<Self, std::env::VarError> {
        let api_key = std::env::var("ZHIPU_API_KEY")?;
        let base_url = std::env::var("ZHIPU_BASE_URL")
            .unwrap_or_else(|_| "https://open.bigmodel.cn/api/paas/v4".to_string());

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
}
