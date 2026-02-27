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

/// Claude Opus 4 -- the most capable model, strongest at complex reasoning,
/// advanced coding, and agentic tasks.
pub const CLAUDE_4_OPUS: &str = "claude-opus-4-0";

/// Claude Sonnet 4 -- high-performance model balancing intelligence and speed,
/// well-suited for agentic coding and nuanced analysis.
pub const CLAUDE_4_SONNET: &str = "claude-sonnet-4-0";

/// Claude 3.7 Sonnet -- hybrid reasoning model with extended thinking support.
pub const CLAUDE_3_7_SONNET: &str = "claude-3-7-sonnet-latest";

/// Claude 3.5 Sonnet -- fast, cost-effective model for everyday tasks including
/// code generation, analysis, and tool use.
pub const CLAUDE_3_5_SONNET: &str = "claude-3-5-sonnet-latest";

/// Claude 3.5 Haiku -- the fastest and most compact model, optimized for
/// near-instant responses and lightweight workloads.
pub const CLAUDE_3_5_HAIKU: &str = "claude-3-5-haiku-latest";

/// The Anthropic API version header value sent with every request.
/// This pins the wire format so that client and server agree on the
/// request/response schema.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";
