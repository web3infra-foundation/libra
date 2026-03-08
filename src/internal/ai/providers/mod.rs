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
//! ## Available Providers
//!
//! | Module       | Vendor           | Auth method              | Default base URL |
//! |-------------|------------------|--------------------------|------------------|
//! | `anthropic` | Anthropic Claude | `x-api-key` header       | `https://api.anthropic.com` |
//! | `openai`    | OpenAI           | Bearer token             | `https://api.openai.com/v1` |
//! | `deepseek`  | DeepSeek         | Bearer token             | `https://api.deepseek.com` |
//! | `gemini`    | Google Gemini    | `x-goog-api-key` header  | `https://generativelanguage.googleapis.com` |
//! | `zhipu`     | Zhipu GLM        | Bearer token             | `https://open.bigmodel.cn/api/paas/v4` |
//! | `ollama`    | Ollama (local)   | None                     | `http://localhost:11434/v1` |
//! | `codex`     | OpenAI Codex     | Bearer token             | `http://localhost:8080` |

pub mod anthropic;
pub mod codex;
pub mod deepseek;
pub mod gemini;
pub mod ollama;
pub mod openai;
pub(crate) mod openai_compat;
pub mod zhipu;
