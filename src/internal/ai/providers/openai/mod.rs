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
pub const GPT_4O: &str = "gpt-4o";
pub const GPT_4O_MINI: &str = "gpt-4o-mini";
pub const GPT_4_TURBO: &str = "gpt-4-turbo";
pub const GPT_4: &str = "gpt-4";
pub const GPT_3_5_TURBO: &str = "gpt-3.5-turbo";
pub const O1_MINI: &str = "o1-mini";
pub const O1_PREVIEW: &str = "o1-preview";
pub const O1: &str = "o1";
