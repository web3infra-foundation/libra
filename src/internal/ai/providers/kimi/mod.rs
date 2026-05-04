//! Kimi (Moonshot AI) API provider for libra.
//!
//! This module integrates with the [Kimi platform](https://platform.kimi.com/docs/api/overview),
//! exposed by Moonshot AI. The API is OpenAI-compatible: requests target a
//! `/chat/completions` endpoint with Bearer-token authentication. The default base
//! URL is `https://api.moonshot.cn/v1`; override with `MOONSHOT_BASE_URL` for the
//! international endpoint or a self-hosted proxy. Authentication is performed via
//! `MOONSHOT_API_KEY`, matching the official platform documentation.
//!
//! # Example
//! ```no_run
//! use libra::internal::ai::client::CompletionClient;
//! use libra::internal::ai::providers::kimi;
//!
//! let client = kimi::Client::from_env().unwrap();
//! let model = client.completion_model(kimi::KIMI_K2_6);
//! ```

pub mod client;
pub mod completion;

pub use client::{Client, KimiProvider};
pub use completion::{CompletionModel, Model};

// Model constants

/// Kimi K2.6: Moonshot's current flagship agent/coding model.
pub const KIMI_K2_6: &str = "kimi-k2.6";
/// Kimi K2.5: previous flagship model with text/vision and thinking support.
pub const KIMI_K2_5: &str = "kimi-k2.5";
/// Kimi K2 Thinking: dedicated long-thinking model.
pub const KIMI_K2_THINKING: &str = "kimi-k2-thinking";
/// Kimi K2 Thinking Turbo: faster long-thinking model.
pub const KIMI_K2_THINKING_TURBO: &str = "kimi-k2-thinking-turbo";
/// Kimi K2 (September 2025 preview): legacy coding-tuned model.
pub const KIMI_K2_0905_PREVIEW: &str = "kimi-k2-0905-preview";
/// Kimi K2 Turbo preview: faster, cheaper variant of the K2 family.
pub const KIMI_K2_TURBO_PREVIEW: &str = "kimi-k2-turbo-preview";
/// Kimi Thinking preview: retired reasoning-focused variant of the Kimi family.
pub const KIMI_THINKING_PREVIEW: &str = "kimi-thinking-preview";
/// Kimi Latest: legacy rolling alias. Prefer [`KIMI_K2_6`].
pub const KIMI_LATEST: &str = "kimi-latest";
/// Moonshot V1 (8K context window).
pub const MOONSHOT_V1_8K: &str = "moonshot-v1-8k";
/// Moonshot V1 (32K context window).
pub const MOONSHOT_V1_32K: &str = "moonshot-v1-32k";
/// Moonshot V1 (128K context window).
pub const MOONSHOT_V1_128K: &str = "moonshot-v1-128k";
/// Moonshot V1 Auto: server-side context-window selection.
pub const MOONSHOT_V1_AUTO: &str = "moonshot-v1-auto";
