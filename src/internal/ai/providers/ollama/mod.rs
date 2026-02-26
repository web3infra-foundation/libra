//! Ollama API provider for libra.
//!
//! Ollama exposes an OpenAI-compatible API locally (default: `http://localhost:11434/v1`).
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
