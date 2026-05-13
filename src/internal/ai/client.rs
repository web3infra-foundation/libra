//! Generic AI provider HTTP client wrapper.
//!
//! Every concrete provider (OpenAI, Anthropic, Gemini, DeepSeek, Kimi, Zhipu,
//! Ollama, etc.) shares the same set of cross-cutting concerns: a base URL,
//! a tuned `reqwest` client with sane timeouts, and a hook for injecting
//! provider-specific headers (auth, API version, organisation id, ...). This
//! module factors those concerns out into [`Client<P>`] and the
//! [`Provider`] / [`CompletionClient`] traits so individual provider modules
//! only need to implement the parts that actually differ between APIs.
//!
//! The configured timeouts are tuned for long reasoning turns and tool
//! planning loops, which can legitimately spend several minutes on the
//! server side before producing a token. Connect/read timeouts remain short
//! so that genuine network failures still surface quickly.

use reqwest::Client as HttpClient;

use crate::internal::ai::completion::CompletionModel;

/// Maximum total duration for a single AI HTTP request.
///
/// Long reasoning models and multi-turn tool-planning loops can stay quiet on
/// the wire for many minutes before emitting tokens, so the overall ceiling
/// is set generously. Per-stage timeouts (`connect`, `read`) below remain
/// short to fail fast on genuine network problems.
pub(crate) const AI_HTTP_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);
/// Maximum time to wait for the TCP/TLS handshake to complete.
pub(crate) const AI_HTTP_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// Maximum gap between consecutive bytes while streaming a response body.
pub(crate) const AI_HTTP_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// A generic client for AI providers.
///
/// It holds the shared HTTP client, base URL, and provider-specific extension.
/// This client handles common HTTP logic like timeouts and proxy configuration.
///
/// The type parameter `P` carries the provider-specific state (typically API
/// keys and version metadata). Provider modules implement [`Provider`] on
/// `P` so that outgoing requests can be augmented with the right headers
/// without leaking those concerns into the shared transport layer.
#[derive(Clone, Debug)]
pub struct Client<P> {
    /// The base URL of the AI provider's API.
    pub base_url: String,
    /// The shared HTTP client (reqwest).
    pub http_client: HttpClient,
    /// Provider-specific logic (e.g., authentication).
    pub provider: P,
}

impl<P> Client<P> {
    /// Creates a new generic Client.
    ///
    /// # Arguments
    /// * `base_url` - The base API URL.
    /// * `provider` - The provider-specific implementation.
    ///
    /// Functional scope:
    /// - Builds a [`reqwest::Client`] with the AI-tuned timeout triple
    ///   ([`AI_HTTP_REQUEST_TIMEOUT`], [`AI_HTTP_CONNECT_TIMEOUT`],
    ///   [`AI_HTTP_READ_TIMEOUT`]).
    /// - Uses `reqwest`'s default builder, which honours system proxy
    ///   environment variables (`HTTPS_PROXY`, `HTTP_PROXY`, `NO_PROXY`).
    ///
    /// Boundary conditions:
    /// - If `reqwest` cannot construct the configured client (e.g. due to a
    ///   broken native TLS backend), the constructor logs a warning and
    ///   falls back to a default `HttpClient::new()`. The fallback has no
    ///   custom timeouts, so failures will manifest as slower overall
    ///   timeouts rather than panics — preferable to crashing during
    ///   startup.
    pub fn new(base_url: &str, provider: P) -> Self {
        // Build client with timeout and system proxy support
        let http_client = HttpClient::builder()
            .timeout(AI_HTTP_REQUEST_TIMEOUT)
            .connect_timeout(AI_HTTP_CONNECT_TIMEOUT)
            .read_timeout(AI_HTTP_READ_TIMEOUT)
            .build()
            .unwrap_or_else(|e| {
                // Falling back to a default client preserves availability;
                // the lost timeouts are an observable degradation but better
                // than panicking at startup.
                tracing::warn!(
                    "Failed to build HTTP client with timeout: {}. Using default client.",
                    e
                );
                HttpClient::new()
            });

        Self {
            base_url: base_url.to_string(),
            http_client,
            provider,
        }
    }
}

/// Trait defining provider-specific behavior.
///
/// Implementors decorate outgoing HTTP requests just before they are sent —
/// typically to inject authentication headers (`Authorization`,
/// `x-api-key`), API version selectors, or organisation identifiers. The
/// default implementation is a no-op so providers can opt in to header
/// injection only when needed.
pub trait Provider: Send + Sync {
    /// Allows the provider to customize the HTTP request (e.g., adding headers).
    ///
    /// Functional scope:
    /// - Receives the partially-built [`reqwest::RequestBuilder`] and returns
    ///   either the same or a modified builder. Implementations should be
    ///   idempotent because the framework may call this once per attempt.
    ///
    /// Boundary conditions:
    /// - The default implementation returns the request unchanged. Providers
    ///   that authenticate via URL parameters rather than headers can simply
    ///   skip overriding this method.
    ///
    /// # Arguments
    /// * `request` - The pending HTTP request builder.
    ///
    /// # Returns
    /// The modified request builder.
    fn on_request(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request
    }
}

/// Trait for clients that support text completion.
///
/// Each provider exposes a different concrete [`CompletionModel`] type
/// (carrying provider-specific request shaping). This trait gives the rest
/// of the AI runtime a uniform way to obtain a model instance from a client
/// and a model identifier string.
pub trait CompletionClient {
    /// The concrete CompletionModel type returned by this client.
    type Model: CompletionModel;

    /// Creates a completion model instance for the given model name.
    ///
    /// Functional scope:
    /// - Binds a model identifier (e.g. `"gpt-4o"`, `"claude-opus-4-5"`) to
    ///   the underlying client so subsequent prompts route to the correct
    ///   endpoint and model selector.
    ///
    /// Boundary conditions:
    /// - The model name is not validated up front; an invalid name will
    ///   typically surface as an HTTP error on the first completion call.
    fn completion_model(&self, model: impl Into<String>) -> Self::Model;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scenario: the configured HTTP timeout constants must be permissive
    /// enough to host long reasoning turns while still bounding obviously
    /// stuck connections.
    #[test]
    fn ai_http_timeouts_allow_long_reasoning_requests() {
        assert!(AI_HTTP_REQUEST_TIMEOUT >= std::time::Duration::from_secs(300));
        assert_eq!(AI_HTTP_CONNECT_TIMEOUT, std::time::Duration::from_secs(30));
        assert!(AI_HTTP_READ_TIMEOUT >= std::time::Duration::from_secs(60));
    }
}
