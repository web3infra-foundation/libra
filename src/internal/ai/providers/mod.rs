//! AI provider backends for Libra Code.
//!
//! This module contains pluggable provider implementations that translate
//! between libra's provider-agnostic [`CompletionRequest`](super::completion::CompletionRequest)
//! / [`CompletionResponse`](super::completion::request::CompletionResponse) types
//! and each vendor's native HTTP API.
//!
//! ## Architecture
//!
//! Every provider follows the same three-file convention:
//!
//! | File              | Responsibility |
//! |-------------------|----------------|
//! | `mod.rs`          | Public re-exports, model name constants |
//! | `client.rs`       | [`Provider`](super::client::Provider) trait impl (auth headers) + `Client` type alias |
//! | `completion.rs`   | [`CompletionModel`](super::completion::CompletionModel) trait impl (request/response mapping) |
//!
//! Adding a new provider only requires creating a new sub-module that
//! implements `Provider` (for authentication) and `CompletionModel`
//! (for the chat completions round-trip).
//!
//! ## Environment construction policy
//!
//! Runtime call sites that know the repository/global config identity should
//! build clients with each provider's async `Client::from_resolved_env(...)`.
//! That path resolves process environment variables plus Libra vault-backed
//! config entries such as `vault.env.<PROVIDER>_API_KEY`, and is the supported
//! bootstrap surface for `libra code`, headless web mode, and orchestration.
//!
//! `Client::from_env()` remains source-compatible for simple programmatic
//! callers and legacy tests in the 0.17 line. It is a compatibility helper,
//! not the preferred runtime bootstrap. The v0.18 release notes announce its
//! deprecation path and the migration to `from_resolved_env`.
//!
//! ## Available Providers
//!
//! | Module       | Vendor           | Auth method              | Default base URL |
//! |-------------|------------------|--------------------------|------------------|
//! | `anthropic` | Anthropic Claude | `x-api-key` header       | `https://api.anthropic.com` |
//! | `openai`    | OpenAI           | Bearer token             | `https://api.openai.com/v1` |
//! | `deepseek`  | DeepSeek         | Bearer token             | `https://api.deepseek.com` |
//! | `gemini`    | Google Gemini    | `x-goog-api-key` header  | `https://generativelanguage.googleapis.com` |
//! | `kimi`      | Moonshot AI Kimi | Bearer token             | `https://api.moonshot.cn/v1` |
//! | `zhipu`     | Zhipu GLM        | Bearer token             | `https://open.bigmodel.cn/api/paas/v4` |
//! | `ollama`    | Ollama (local)   | None                     | `http://127.0.0.1:11434/v1` |

pub mod anthropic;
pub mod capability;
pub mod deepseek;
pub mod error;
pub mod factory;
#[cfg(feature = "test-provider")]
pub mod fake;
pub mod gemini;
pub mod kimi;
pub mod ollama;
pub mod openai;
pub mod openai_compat;
pub mod runtime;
pub mod transform;
pub mod wire_helpers;
pub mod zhipu;

pub use capability::{ModelCapability, ModelCost};
pub use error::{
    ProviderError, RetryPolicy, StreamErrorKind, parse_api_error, parse_stream_error_kind,
};
pub use factory::{ProviderBuildOptions, ProviderFactory, ProviderFactoryError};
pub use runtime::{AnyCompletionModel, AnyCompletionRawResponse};
pub use transform::{ProviderTransform, TransformError, transform_for, variant};
