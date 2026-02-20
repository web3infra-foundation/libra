//! Composable system prompt builder for the AI agent.
//!
//! This module provides a rules-based prompt system where the system prompt is
//! composed from modular rule files. Rules are loaded from a three-tier hierarchy:
//!
//! 1. **Project-local**: `.libra/rules/{category}.md` in the working directory
//! 2. **User-global**: `~/.config/libra/rules/{category}.md`
//! 3. **Embedded**: Default rules compiled into the binary
//!
//! # Example
//! ```no_run
//! use libra::internal::ai::prompt::{SystemPromptBuilder, ContextMode};
//!
//! let prompt = SystemPromptBuilder::new(std::path::Path::new("/my/project"))
//!     .with_context(ContextMode::Dev)
//!     .extra_section("Project Info", "This is a web service")
//!     .build();
//! ```

mod builder;
pub mod context;
mod loader;
pub mod rules;

pub use builder::SystemPromptBuilder;
pub use context::ContextMode;
pub use rules::RuleCategory;
