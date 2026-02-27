//! Zhipu API provider for libra.
//!
//! # Example
//! ```no_run
//! use libra::internal::ai::client::CompletionClient;
//! use libra::internal::ai::providers::zhipu;
//!
//! let client = zhipu::Client::from_env().unwrap();
//! let model = client.completion_model("glm-5");
//! ```

pub mod client;
pub mod completion;

pub use client::{Client, ZhipuProvider};
pub use completion::{CompletionModel, Model};

// Model constants

/// GLM-5: Zhipu AI's latest and most capable model with broad general intelligence.
pub const GLM_5: &str = "glm-5";
/// GLM-4: Zhipu AI's previous-generation flagship model, strong reasoning and instruction following.
pub const GLM_4: &str = "glm-4";
/// GLM-4-Flash: A lightweight, low-latency variant of GLM-4 optimized for speed.
pub const GLM_4_FLASH: &str = "glm-4-flash";
/// GLM-3-Turbo: A cost-effective, high-throughput variant of the GLM-3 family.
pub const GLM_3_TURBO: &str = "glm-3-turbo";
/// GLM-3: Zhipu AI's earlier-generation general-purpose model.
pub const GLM_3: &str = "glm-3";
