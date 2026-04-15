//! AI Agent Infrastructure for Libra
//!
//! This module provides the foundational components for integrating AI capabilities
//! into Libra's git workflows. The architecture consists of:
//!
//! - **Agent Framework**: Core [`Agent`] struct with [`AgentBuilder`] for configuration
//! - **Provider Abstractions**: [`CompletionModel`] trait for pluggable LLM backends
//! - **DAG Integration**: [`AgentAction`] adapter for workflow composition
//!
//! # Example
//! ```no_run
//! use libra::internal::ai::{AgentBuilder, providers::gemini::Client};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let client = Client::from_env()?;
//! let model = client.completion_model("gemini-2.5-flash");
//! let agent = AgentBuilder::new(model)
//!     .preamble("You are a helpful assistant")
//!     .temperature(0.7)?
//!     .build();
//! # Ok(())
//! # }
//! ```

pub mod agent;
pub mod claudecode;
pub mod client;
pub mod codex;
pub mod commands;
pub mod completion;
pub mod history;
pub mod hooks;
pub mod intent;
pub mod intentspec;
pub mod mcp;
pub mod node_adapter;
pub mod orchestrator;
pub mod projection;
pub mod prompt;
pub mod providers;
pub mod runtime;
pub mod sandbox;
pub mod session;
pub mod tools;
pub mod util;
pub mod web;
pub mod workflow_objects;
pub mod workspace_snapshot;

pub use agent::{Agent, AgentBuilder, ChatAgent};
pub use completion::{Chat, CompletionModel, Message, Prompt};
pub use node_adapter::{AgentAction, ToolLoopAction};
