//! Anthropic API provider for libra.
//!
//! # Example
//! ```no_run
//! use libra::internal::ai::client::CompletionClient;
//! use libra::internal::ai::providers::anthropic;
//!
//! let client = anthropic::Client::from_env().unwrap();
//! let model = client.completion_model("claude-3-5-sonnet-latest");
//! ```

pub mod client;
pub mod completion;

pub use client::{AnthropicProvider, Client};
pub use completion::{CompletionModel, Model};

// Model constants
pub const CLAUDE_4_OPUS: &str = "claude-opus-4-0";
pub const CLAUDE_4_SONNET: &str = "claude-sonnet-4-0";
pub const CLAUDE_3_7_SONNET: &str = "claude-3-7-sonnet-latest";
pub const CLAUDE_3_5_SONNET: &str = "claude-3-5-sonnet-latest";
pub const CLAUDE_3_5_HAIKU: &str = "claude-3-5-haiku-latest";

// Anthropic API version
pub const ANTHROPIC_VERSION: &str = "2023-06-01";
