//! Provider contracts for lifecycle hook ingestion and setup.

use std::{fmt, path::Path};

use anyhow::Result;
use serde_json::Value;

use super::lifecycle::{LifecycleEvent, LifecycleEventKind, SessionHookEnvelope};
use crate::internal::ai::session::SessionState;

pub const CANONICAL_DEDUP_IDENTITY_KEYS: &[&str] = &[
    "event_id",
    "request_id",
    "turn_id",
    "message_id",
    "tool_use_id",
    "sequence",
    "timestamp",
];

/// Canonical hook command surface exposed by Libra.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderHookCommand {
    SessionStart,
    Prompt,
    ToolUse,
    ModelUpdate,
    Compaction,
    Stop,
    SessionEnd,
}

impl ProviderHookCommand {
    pub fn lifecycle_event_kind(self) -> LifecycleEventKind {
        match self {
            ProviderHookCommand::SessionStart => LifecycleEventKind::SessionStart,
            ProviderHookCommand::Prompt => LifecycleEventKind::TurnStart,
            ProviderHookCommand::ToolUse => LifecycleEventKind::ToolUse,
            ProviderHookCommand::ModelUpdate => LifecycleEventKind::ModelUpdate,
            ProviderHookCommand::Compaction => LifecycleEventKind::Compaction,
            ProviderHookCommand::Stop => LifecycleEventKind::TurnEnd,
            ProviderHookCommand::SessionEnd => LifecycleEventKind::SessionEnd,
        }
    }
}

impl fmt::Display for ProviderHookCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            ProviderHookCommand::SessionStart => "session-start",
            ProviderHookCommand::Prompt => "prompt",
            ProviderHookCommand::ToolUse => "tool-use",
            ProviderHookCommand::ModelUpdate => "model-update",
            ProviderHookCommand::Compaction => "compaction",
            ProviderHookCommand::Stop => "stop",
            ProviderHookCommand::SessionEnd => "session-end",
        };
        write!(f, "{value}")
    }
}

/// Generic install options passed from the command layer into a provider installer.
#[derive(Debug, Clone, Default)]
pub struct ProviderInstallOptions {
    pub binary_path: Option<String>,
    pub timeout_secs: Option<u64>,
}

/// A statically registered provider that can parse lifecycle payloads and manage hook setup.
pub trait HookProvider: Sync {
    fn provider_name(&self) -> &'static str;
    fn source_name(&self) -> &'static str;
    fn supported_commands(&self) -> &'static [ProviderHookCommand];
    fn parse_hook_event(
        &self,
        hook_event_name: &str,
        envelope: &SessionHookEnvelope,
    ) -> Result<LifecycleEvent>;
    fn dedup_identity_keys(&self) -> &'static [&'static str];
    fn lifecycle_fallback_events(&self) -> &'static [&'static str];
    fn command_output(&self, _command: ProviderHookCommand) -> Option<Value> {
        None
    }
    fn post_process_event(
        &self,
        _command: ProviderHookCommand,
        _storage_path: &Path,
        _session: &mut SessionState,
        _envelope: &SessionHookEnvelope,
        _event: &LifecycleEvent,
    ) -> Result<()> {
        Ok(())
    }
    fn install_hooks(&self, options: &ProviderInstallOptions) -> Result<()>;
    fn uninstall_hooks(&self) -> Result<()>;
    fn hooks_are_installed(&self) -> Result<bool>;
}
