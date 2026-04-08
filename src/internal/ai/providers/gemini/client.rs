//! Gemini API client construction and authentication.
//!
//! This module provides [`GeminiProvider`], which implements the generic
//! [`Provider`] trait by attaching the `x-goog-api-key` header to every
//! outgoing HTTP request. Unlike OpenAI-style providers that use
//! `Authorization: Bearer <token>`, the Gemini API authenticates via its
//! own header (it also accepts a `key=` query parameter, but the header
//! approach is used here for consistency with the generic `Provider` trait).
//!
//! The [`Client`] type alias combines the generic HTTP client with
//! `GeminiProvider` and exposes convenience constructors. The base URL
//! defaults to `https://generativelanguage.googleapis.com`.

use std::{env, fmt};

use crate::internal::ai::client::{Client as HttpClient, Provider};

/// Gemini API provider that carries the API key and injects the
/// `x-goog-api-key` authentication header into every request.
#[derive(Clone)]
pub struct GeminiProvider {
    api_key: String,
}

/// Manually implemented to redact the API key from debug output.
impl fmt::Debug for GeminiProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GeminiProvider")
            .field("api_key", &"***")
            .finish()
    }
}

impl GeminiProvider {
    /// Creates a new GeminiProvider with the given API key.
    pub fn new(api_key: String) -> Self {
        Self { api_key }
    }

    /// Returns the API key.
    pub fn api_key(&self) -> &str {
        &self.api_key
    }
}

impl Provider for GeminiProvider {
    /// Attaches the `x-goog-api-key` header for Gemini authentication.
    fn on_request(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request.header("x-goog-api-key", &self.api_key)
    }
}

/// Concrete Gemini client type, combining the generic HTTP client with
/// [`GeminiProvider`] for authentication.
pub type Client = HttpClient<GeminiProvider>;

impl Client {
    /// Creates a Gemini Client from environment variables.
    pub fn from_env() -> Result<Self, env::VarError> {
        let api_key = env::var("GEMINI_API_KEY")?;
        let provider = GeminiProvider::new(api_key);
        Ok(Self::new(
            "https://generativelanguage.googleapis.com",
            provider,
        ))
    }

    /// Creates a [`CompletionModel`](super::completion::CompletionModel) bound
    /// to this client for the given Gemini model identifier (e.g.,
    /// `"gemini-2.5-flash"` or one of the constants from [`super`]).
    pub fn completion_model(&self, model: &str) -> super::completion::CompletionModel {
        super::completion::CompletionModel::new(self.clone(), model)
    }
}
