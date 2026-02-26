//! Ollama API client for libra.
//!
//! Ollama exposes an OpenAI-compatible chat completions API locally, so the
//! request and response payloads follow the same schema as the OpenAI provider.
//! Unlike cloud providers, Ollama does not require authentication -- no API key
//! or bearer token is sent with requests.
//!
//! Because local inference (especially on large models) is significantly slower
//! than cloud API calls, the HTTP client is configured with a 300-second timeout
//! instead of the default provided by [`GenericClient::new`].

use std::fmt;

use reqwest::Client as HttpClient;

use crate::internal::ai::client::{Client as GenericClient, Provider};

const DEFAULT_BASE_URL: &str = "http://localhost:11434/v1";

/// Default timeout for Ollama requests (5 minutes).
/// Local inference on large models can be significantly slower than cloud APIs.
const DEFAULT_TIMEOUT_SECS: u64 = 300;

/// Ollama API provider.
///
/// Ollama runs locally and does not require authentication by default.
#[derive(Clone)]
pub struct OllamaProvider;

impl fmt::Debug for OllamaProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OllamaProvider").finish()
    }
}

impl Provider for OllamaProvider {
    // No authentication headers needed for local Ollama.
}

/// Ollama client type.
pub type Client = GenericClient<OllamaProvider>;

/// Build an Ollama client with a generous timeout suitable for local inference.
///
/// This function constructs the [`reqwest::Client`] manually rather than
/// delegating to [`GenericClient::new`] because the default HTTP timeout is
/// too short for local model inference. The custom timeout
/// ([`DEFAULT_TIMEOUT_SECS`] = 300s) ensures that large models have enough
/// time to generate a full response before the connection is dropped.
fn build_ollama_client(base_url: &str) -> Client {
    let http_client = HttpClient::builder()
        .timeout(std::time::Duration::from_secs(DEFAULT_TIMEOUT_SECS))
        .build()
        .unwrap_or_else(|e| {
            tracing::warn!(
                "Failed to build HTTP client with {DEFAULT_TIMEOUT_SECS}s timeout: {e}. \
                 Using default client (timeout may differ)."
            );
            HttpClient::new()
        });

    Client {
        base_url: base_url.to_string(),
        http_client,
        provider: OllamaProvider,
    }
}

impl Client {
    /// Creates an Ollama client from environment variables.
    ///
    /// Reads the optional `OLLAMA_BASE_URL` environment variable (defaults to
    /// `http://localhost:11434/v1`).
    pub fn from_env() -> Self {
        let base_url =
            std::env::var("OLLAMA_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());
        build_ollama_client(&base_url)
    }

    /// Creates an Ollama client pointing to the default local instance.
    pub fn new_local() -> Self {
        build_ollama_client(DEFAULT_BASE_URL)
    }

    /// Creates an Ollama client with a custom base URL.
    pub fn with_base_url(base_url: &str) -> Self {
        build_ollama_client(base_url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ollama_provider_debug() {
        let provider = OllamaProvider;
        let debug_str = format!("{:?}", provider);
        assert!(debug_str.contains("OllamaProvider"));
    }

    #[test]
    fn test_client_new_local() {
        let client = Client::new_local();
        assert_eq!(client.base_url, DEFAULT_BASE_URL);
    }

    #[test]
    fn test_client_with_base_url() {
        let client = Client::with_base_url("http://remote:11434/v1");
        assert_eq!(client.base_url, "http://remote:11434/v1");
    }
}
