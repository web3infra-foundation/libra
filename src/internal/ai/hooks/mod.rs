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
pub mod lifecycle;
pub mod provider;
pub mod providers;
pub mod runner;
pub mod runtime;
mod setup;

pub use config::{HookConfig, HookDefinition, load_hook_config};
pub use event::{HookAction, HookEvent, HookInput};
pub use lifecycle::{LifecycleEvent, LifecycleEventKind, SessionHookEnvelope};
pub use provider::{HookProvider, ProviderHookCommand, ProviderInstallOptions};
pub use providers::{claude_provider, find_provider, gemini_provider, supported_provider_names};
pub use runner::HookRunner;
pub use runtime::{
    AI_SESSION_SCHEMA, AI_SESSION_TYPE, build_ai_session_id, process_hook_event_from_stdin,
};
