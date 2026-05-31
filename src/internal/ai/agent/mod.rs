//! AI agent domain.
//!
//! AI 代理域。
//!
//! - `runtime`: execution runtime for AI agents (Agent, tool loop, chat state).
//! - `profile`: declarative agent profiles and auto-selection/lookup utilities.
//! - `budget`: per-session and per-agent budget enforcement (OC-Phase 5 P5.3).
pub mod budget;
pub mod classifier;
pub mod format;
pub mod profile;
pub mod runtime;

pub use budget::{
    BudgetAxis, BudgetExceededError, BudgetMeasurement, BudgetScope, BudgetTracker, BudgetWarning,
};
pub use classifier::{
    ExplicitCodeContext, TaskIntent, TaskIntentClassificationRequest, TaskIntentClassifier,
    TaskIntentClassifierError, TaskIntentDecision, TaskIntentDecisionSource,
};
pub use format::{format_agents_table, format_budget_status, format_usage_table};
pub use runtime::{
    Agent, AgentBuilder, ChatAgent, ToolLoopConfig, ToolLoopObserver, run_tool_loop,
    run_tool_loop_with_history_and_observer,
};
