//! Ollama API client for libra.
//!
//! Ollama exposes a native local chat API. The completion implementation
//! converts native `/api/chat` payloads into Libra's OpenAI-compatible internal
//! chat shape.
//! Local Ollama does not require authentication. Direct Ollama Cloud API access
//! uses `https://ollama.com` as the host and requires `OLLAMA_API_KEY`.
//!
//! Because local inference (especially on large models) is significantly slower
//! than cloud API calls, the HTTP client avoids a total request deadline and
//! instead uses an idle read timeout while streaming response chunks.

use std::{fmt, time::Duration};

use reqwest::Client as HttpClient;
use url::Url;

use crate::internal::ai::client::{Client as GenericClient, Provider};

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:11434/v1";

/// Default idle read timeout for Ollama streams (5 minutes).
/// Streaming responses can run longer as long as chunks keep arriving.
const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 300;

/// Default connect timeout for the local or remote Ollama host.
const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 10;

/// Ollama API provider.
#[derive(Clone)]
pub struct OllamaProvider {
    cloud_api: bool,
    api_key: Option<String>,
    compact_tool_schema: bool,
}

impl fmt::Debug for OllamaProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OllamaProvider")
            .field("cloud_api", &self.cloud_api)
            .field("api_key", &self.api_key.as_ref().map(|_| "***"))
            .field("compact_tool_schema", &self.compact_tool_schema)
            .finish()
    }
}

impl Provider for OllamaProvider {
    fn on_request(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if self.cloud_api
            && let Some(api_key) = self.api_key.as_deref()
        {
            return request.bearer_auth(api_key);
        }

        request
    }
}

/// Ollama client type.
pub type Client = GenericClient<OllamaProvider>;

/// Build an Ollama client with streaming-friendly timeouts.
///
/// This function constructs the [`reqwest::Client`] manually rather than
/// delegating to [`GenericClient::new`] because the default total HTTP timeout
/// is too short for long local model inference. Ollama completions use
/// streaming reads, so the client enforces connection and idle-read timeouts
/// without imposing a total response deadline.
fn build_ollama_client(base_url: &str, api_key: Option<String>) -> Client {
    let http_client = HttpClient::builder()
        .connect_timeout(Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS))
        .read_timeout(Duration::from_secs(DEFAULT_IDLE_TIMEOUT_SECS))
        .build()
        .unwrap_or_else(|e| {
            tracing::warn!(
                "Failed to build HTTP client with Ollama streaming timeouts: {e}. \
                 Using default client (timeout may differ)."
            );
            HttpClient::new()
        });

    Client {
        base_url: base_url.to_string(),
        http_client,
        provider: OllamaProvider {
            cloud_api: is_ollama_cloud_base_url(base_url),
            api_key,
            compact_tool_schema: compact_tool_schema_from_env(),
        },
    }
}

fn api_key_for_base_url(base_url: &str) -> Option<String> {
    is_ollama_cloud_base_url(base_url)
        .then(|| std::env::var("OLLAMA_API_KEY").ok())
        .flatten()
        .filter(|key| !key.trim().is_empty())
}

fn is_ollama_cloud_base_url(base_url: &str) -> bool {
    let Ok(url) = Url::parse(base_url) else {
        return false;
    };

    matches!(url.host_str(), Some("ollama.com" | "www.ollama.com"))
}

fn compact_tool_schema_from_env() -> bool {
    std::env::var("OLLAMA_COMPACT_TOOLS")
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

impl Client {
    /// Creates an Ollama client from environment variables.
    ///
    /// Reads the optional `OLLAMA_BASE_URL` environment variable (defaults to
    /// `http://127.0.0.1:11434/v1`). When the base URL points at
    /// `https://ollama.com`, `OLLAMA_API_KEY` is used as a bearer token.
    pub fn from_env() -> Self {
        let base_url =
            std::env::var("OLLAMA_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());
        build_ollama_client(&base_url, api_key_for_base_url(&base_url))
    }

    /// Creates an Ollama client pointing to the default local instance.
    pub fn new_local() -> Self {
        build_ollama_client(DEFAULT_BASE_URL, None)
    }

    /// Creates an Ollama client with a custom base URL.
    pub fn with_base_url(base_url: &str) -> Self {
        build_ollama_client(base_url, api_key_for_base_url(base_url))
    }

    /// Creates an Ollama client with an explicit API key.
    pub fn with_base_url_and_api_key(base_url: &str, api_key: Option<String>) -> Self {
        build_ollama_client(base_url, api_key)
    }

    /// Enables or disables compact tool schemas for Ollama requests.
    pub fn with_compact_tool_schema(mut self, compact: bool) -> Self {
        self.provider.compact_tool_schema = compact;
        self
    }

    /// Returns true when tool schemas should be compacted before sending.
    pub fn compact_tool_schema(&self) -> bool {
        self.provider.compact_tool_schema
    }

    /// Returns true when the client points directly at Ollama Cloud.
    pub fn is_cloud_api(&self) -> bool {
        self.provider.cloud_api
    }

    /// Returns true when a direct Ollama Cloud client is missing authentication.
    pub fn missing_required_cloud_api_key(&self) -> bool {
        self.provider.cloud_api && self.provider.api_key.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ollama_provider_debug() {
        let provider = OllamaProvider {
            cloud_api: true,
            api_key: Some("secret-key".to_string()),
            compact_tool_schema: true,
        };
        let debug_str = format!("{:?}", provider);
        assert!(debug_str.contains("OllamaProvider"));
        assert!(debug_str.contains("***"));
        assert!(debug_str.contains("compact_tool_schema"));
        assert!(!debug_str.contains("secret-key"));
    }

    #[test]
    fn test_client_new_local() {
        let client = Client::new_local();
        assert_eq!(client.base_url, "http://127.0.0.1:11434/v1");
        assert!(!client.is_cloud_api());
    }

    #[test]
    fn test_client_with_base_url() {
        let client = Client::with_base_url("http://remote:11434/v1");
        assert_eq!(client.base_url, "http://remote:11434/v1");
        assert!(!client.is_cloud_api());
    }

    #[test]
    fn test_client_compact_tool_schema_override() {
        let client = Client::new_local().with_compact_tool_schema(true);

        assert!(client.compact_tool_schema());
    }

    #[test]
    fn test_client_detects_direct_cloud_base_url() {
        let client =
            Client::with_base_url_and_api_key("https://ollama.com", Some("test-key".to_string()));

        assert!(client.is_cloud_api());
        assert!(!client.missing_required_cloud_api_key());
    }

    #[test]
    fn test_direct_cloud_request_adds_bearer_auth() {
        let client =
            Client::with_base_url_and_api_key("https://ollama.com", Some("test-key".to_string()));
        let request = client
            .provider
            .on_request(client.http_client.get("https://ollama.com/api/tags"))
            .build()
            .unwrap();

        assert_eq!(
            request
                .headers()
                .get(reqwest::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer test-key")
        );
    }

    #[test]
    fn test_local_request_does_not_add_bearer_auth() {
        let client = Client::with_base_url_and_api_key(
            "http://127.0.0.1:11434/v1",
            Some("test-key".to_string()),
        );
        let request = client
            .provider
            .on_request(client.http_client.get("http://127.0.0.1:11434/api/tags"))
            .build()
            .unwrap();

        assert!(
            request
                .headers()
                .get(reqwest::header::AUTHORIZATION)
                .is_none()
        );
    }

    #[test]
    fn test_direct_cloud_requires_api_key() {
        let client = Client::with_base_url_and_api_key("https://ollama.com", None);

        assert!(client.is_cloud_api());
        assert!(client.missing_required_cloud_api_key());
    }
}
