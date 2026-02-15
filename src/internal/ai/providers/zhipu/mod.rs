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
pub const GLM_5: &str = "glm-5";
pub const GLM_4: &str = "glm-4";
pub const GLM_4_FLASH: &str = "glm-4-flash";
pub const GLM_3_TURBO: &str = "glm-3-turbo";
pub const GLM_3: &str = "glm-3";
