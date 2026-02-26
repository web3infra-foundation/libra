//! Ollama API provider for libra.
//!
//! Ollama exposes an OpenAI-compatible API locally (default: `http://localhost:11434/v1`).
//! No API key is required.
//!
//! # Example
//! ```no_run
//! use libra::internal::ai::client::CompletionClient;
//! use libra::internal::ai::providers::ollama;
//!
//! let client = ollama::Client::from_env();
//! let model = client.completion_model("gpt-oss:120b");
//! ```

pub mod client;
pub mod completion;

pub use client::{Client, OllamaProvider};
pub use completion::{CompletionModel, Model};

// Model constants

/// Default model identifier for the GPT-OSS 120-billion-parameter model served by Ollama.
pub const GPT_OSS_120B: &str = "gpt-oss:120b";
