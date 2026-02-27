//! OpenAI API provider for libra.
//!
//! # Example
//! ```no_run
//! use libra::internal::ai::client::CompletionClient;
//! use libra::internal::ai::providers::openai;
//!
//! let client = openai::Client::from_env().unwrap();
//! let model = client.completion_model("gpt-4o");
//! ```

pub mod client;
pub mod completion;

pub use client::{Client, OpenAIProvider};
pub use completion::{CompletionModel, Model};

// Model constants

/// GPT-4o: flagship multimodal model with vision, faster and cheaper than GPT-4 Turbo.
pub const GPT_4O: &str = "gpt-4o";
/// GPT-4o Mini: small, fast, and affordable model for lightweight tasks.
pub const GPT_4O_MINI: &str = "gpt-4o-mini";
/// GPT-4 Turbo: high-capability model with a 128k context window and vision support.
pub const GPT_4_TURBO: &str = "gpt-4-turbo";
/// GPT-4: original large language model, slower but highly capable.
pub const GPT_4: &str = "gpt-4";
/// GPT-3.5 Turbo: fast and cost-effective model for simpler tasks.
pub const GPT_3_5_TURBO: &str = "gpt-3.5-turbo";
/// o1-mini: small reasoning model optimized for STEM and code tasks.
pub const O1_MINI: &str = "o1-mini";
/// o1-preview: reasoning model with broad world knowledge for complex multi-step problems.
pub const O1_PREVIEW: &str = "o1-preview";
/// o1: full reasoning model with extended thinking for the hardest problems.
pub const O1: &str = "o1";
