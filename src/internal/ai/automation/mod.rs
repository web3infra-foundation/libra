//! Automation MVP for CEX-15.
//!
//! CEX-15 的自动化 MVP。
//!
//! The first automation slice is deliberately small: parse project rules,
//! simulate cron triggers, preflight shell actions through the existing safety
//! classifier, isolate rule failures, and persist execution history through the
//! shared migration runner.

pub mod config;
pub mod events;
pub mod executor;
pub mod history;
pub mod runtime;
pub mod scheduler;

pub use config::{AutomationAction, AutomationConfig, AutomationRule, AutomationTrigger};
pub use events::{
    AutomationError, AutomationRunResult, AutomationRunStatus, AutomationRuntimeEvent,
    VCS_EVENT_POST_ADD, VCS_EVENT_POST_BRANCH, VCS_EVENT_POST_COMMIT, VCS_EVENT_POST_PUSH,
    VCS_EVENT_POST_SWITCH,
};
pub use executor::AutomationExecutor;
pub use history::AutomationHistory;
pub use runtime::{
    dispatch_current_repo_vcs_event_to_history, dispatch_hook_lifecycle_event_to_history,
    dispatch_repo_hook_lifecycle_event_to_history, dispatch_repo_vcs_event_to_history,
    dispatch_vcs_event_to_history,
};
pub use scheduler::AutomationScheduler;
