//! Ollama API provider for libra.
//!
//! Ollama exposes a native local API. Libra accepts an Ollama base URL with or
//! without a trailing `/v1` for compatibility, but completions are sent to
//! `/api/chat` (default base: `http://127.0.0.1:11434/v1`).
//! No API key is required. Use `--model` to specify which local model to use.
//!
//! # Example
//! ```no_run
//! use libra::internal::ai::client::CompletionClient;
//! use libra::internal::ai::providers::ollama;
//!
//! let client = ollama::Client::from_env();
//! let model = client.completion_model("llama3.2");
//! ```

pub mod client;
pub mod completion;

pub use client::{Client, OllamaProvider};
pub use completion::Model;
