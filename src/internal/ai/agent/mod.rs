//! AI agent domain.
//!
//! - `runtime`: execution runtime for AI agents (Agent, tool loop, chat state).
//! - `profile`: declarative agent profiles and auto-selection/lookup utilities.
pub mod profile;
pub mod runtime;

pub use runtime::{
    Agent, AgentBuilder, ChatAgent, ToolLoopConfig, ToolLoopObserver, run_tool_loop,
    run_tool_loop_with_history_and_observer,
};
