//! Hook system for event-driven automation.
//!
//! Hooks are shell commands triggered by lifecycle events (tool use, session
//! start/end). They receive JSON on stdin and can optionally block operations
//! (PreToolUse only, via exit code 2).
//!
//! Hook configuration is loaded from:
//! 1. `{working_dir}/.libra/hooks.json` (project-local)
//! 2. `~/.config/libra/hooks.json` (user-global)
//!
//! Both are merged — hooks from all tiers are collected and executed.

pub mod config;
pub mod event;
pub mod runner;

pub use config::{HookConfig, HookDefinition, load_hook_config};
pub use event::{HookAction, HookEvent, HookInput};
pub use runner::HookRunner;
