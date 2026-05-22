//! Google Gemini API provider for libra.
//!
//! This module integrates with the [Gemini REST API](https://ai.google.dev/api/generate-content)
//! to provide chat completion with function-calling support.
//!
//! # Authentication
//!
//! Set the `GEMINI_API_KEY` environment variable. The provider sends the key
//! via the `x-goog-api-key` HTTP header on every request.
//!
//! # Example
//! ```no_run
//! use libra::internal::ai::providers::gemini;
//!
//! let client = gemini::Client::from_env().unwrap();
//! let model = client.completion_model(gemini::GEMINI_2_5_FLASH);
//! ```

pub mod client;
pub mod completion;
pub mod gemini_api_types;

#[cfg(test)]
mod api_tests;

/// Re-export the concrete Gemini HTTP client.
pub use client::Client;
/// Re-export the Gemini completion model used by the agent loop.
pub use completion::CompletionModel;
/// `gemini-2.5-pro-preview-06-05` completion model
pub const GEMINI_2_5_PRO_PREVIEW_06_05: &str = "gemini-2.5-pro-preview-06-05";
/// `gemini-2.5-pro-preview-05-06` completion model
pub const GEMINI_2_5_PRO_PREVIEW_05_06: &str = "gemini-2.5-pro-preview-05-06";
/// `gemini-2.5-pro-preview-03-25` completion model
pub const GEMINI_2_5_PRO_PREVIEW_03_25: &str = "gemini-2.5-pro-preview-03-25";
/// `gemini-2.5-flash-preview-04-17` completion model
pub const GEMINI_2_5_FLASH_PREVIEW_04_17: &str = "gemini-2.5-flash-preview-04-17";
/// `gemini-2.5-pro-exp-03-25` experimental completion model
pub const GEMINI_2_5_PRO_EXP_03_25: &str = "gemini-2.5-pro-exp-03-25";
/// `gemini-2.5-flash` completion model
pub const GEMINI_2_5_FLASH: &str = "gemini-2.5-flash";
/// `gemini-2.0-flash-lite` completion model
pub const GEMINI_2_0_FLASH_LITE: &str = "gemini-2.0-flash-lite";
/// `gemini-2.0-flash` completion model
pub const GEMINI_2_0_FLASH: &str = "gemini-2.0-flash";
