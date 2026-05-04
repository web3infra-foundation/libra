//! Automation MVP for CEX-15.
//!
//! The first automation slice is deliberately small: parse project rules,
//! simulate cron triggers, preflight shell actions through the existing safety
//! classifier, isolate rule failures, and persist execution history through the
//! shared migration runner.

pub mod config;
pub mod events;
pub mod executor;
pub mod history;
pub mod scheduler;

pub use config::{AutomationAction, AutomationConfig, AutomationRule, AutomationTrigger};
pub use events::{AutomationError, AutomationRunResult, AutomationRunStatus};
pub use executor::AutomationExecutor;
pub use history::AutomationHistory;
pub use scheduler::AutomationScheduler;
