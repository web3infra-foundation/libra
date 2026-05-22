//! DeepSeek API provider for libra.
//!
//! This module integrates with the [DeepSeek API](https://api-docs.deepseek.com/),
//! which exposes an OpenAI-compatible Chat Completions endpoint. Authentication
//! is performed via Bearer token using a `DEEPSEEK_API_KEY` environment variable.
//!
//! # Example
//! ```no_run
//! use libra::internal::ai::client::CompletionClient;
//! use libra::internal::ai::providers::deepseek;
//!
//! let client = deepseek::Client::from_env().unwrap();
//! let model = client.completion_model("deepseek-chat");
//! ```

pub mod client;
pub mod completion;

pub use client::{Client, DeepSeekProvider};
pub use completion::{CompletionModel, Model};
