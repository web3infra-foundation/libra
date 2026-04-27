//! Provider contracts for lifecycle hook ingestion and setup.
//!
//! Each LLM provider Libra integrates with (Claude, Gemini, etc.) implements the
//! [`HookProvider`] trait declared here. The trait separates two concerns:
//! 1. **Parsing** — translate the provider's native hook envelope into a canonical
//!    [`LifecycleEvent`].
//! 2. **Setup** — install/uninstall the provider's hook binding files (settings.json,
//!    extension manifests, etc.) so the provider's runtime actually invokes Libra
//!    when one of its lifecycle events fires.
//!
//! Keeping these behind a trait lets the rest of the agent stack remain unaware of
//! provider-specific details and lets new providers be added without touching
//! event normalisation or the hook runner.

use std::{fmt, path::Path};

use anyhow::Result;
use serde_json::Value;

use super::lifecycle::{LifecycleEvent, LifecycleEventKind, SessionHookEnvelope};
use crate::internal::ai::session::SessionState;

/// Identity field names that providers most often use to make a hook envelope
/// uniquely identifiable. Listed in priority order: the first one that yields a
/// non-null value is used as the dedup primary key.
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
///
/// Each variant maps to a CLI subcommand the provider's hook configuration is told
/// to invoke (e.g. `libra hooks tool-use`). Internally each command is paired with a
/// [`LifecycleEventKind`] so the runner can apply the right session-state mutation.
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
    /// Map a hook command to its corresponding lifecycle event kind.
    ///
    /// Boundary conditions: the mapping is total — every command has exactly one
    /// lifecycle kind, so this method never fails.
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
///
/// Both fields are optional so the installer can fall back to provider-specific
/// defaults (the path to the running `libra` binary, a sensible timeout, etc.).
#[derive(Debug, Clone, Default)]
pub struct ProviderInstallOptions {
    pub binary_path: Option<String>,
    pub timeout_secs: Option<u64>,
}

/// A statically registered provider that can parse lifecycle payloads and manage hook setup.
///
/// Implementations are expected to be cheap to construct (typically zero-sized
/// types) and are looked up by name at runtime via [`super::providers::find_provider`].
/// All methods are sync because hook ingestion runs on the agent's main thread and
/// IO that providers perform is bounded by user-controlled config files.
pub trait HookProvider: Sync {
    /// Human-readable provider identifier used in logs and CLI feedback.
    fn provider_name(&self) -> &'static str;
    /// Tag applied to ingested events when persisted to session metadata, allowing
    /// downstream consumers to attribute an event to its origin provider.
    fn source_name(&self) -> &'static str;
    /// Hook commands this provider knows how to install and parse.
    fn supported_commands(&self) -> &'static [ProviderHookCommand];
    /// Translate a provider envelope into the canonical [`LifecycleEvent`].
    ///
    /// Returns an error when the envelope is malformed or names a hook event
    /// the provider does not support.
    fn parse_hook_event(
        &self,
        hook_event_name: &str,
        envelope: &SessionHookEnvelope,
    ) -> Result<LifecycleEvent>;
    /// Identity field names this provider checks when building dedup keys.
    fn dedup_identity_keys(&self) -> &'static [&'static str];
    /// Provider-native event names that should fall back to `session_id` when no
    /// identity field is present (typically session-scoped events that fire once).
    fn lifecycle_fallback_events(&self) -> &'static [&'static str];
    /// Optional command-level output payload (e.g. JSON the provider expects in
    /// stdout) — defaults to `None` for providers that signal purely via exit code.
    fn command_output(&self, _command: ProviderHookCommand) -> Option<Value> {
        None
    }
    /// Hook the provider can use to apply additional state mutations after the
    /// canonical event has been recorded — e.g. linking transcripts to objects on
    /// disk. Default impl is a no-op.
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
    /// Materialise the provider's hook configuration files on disk.
    fn install_hooks(&self, options: &ProviderInstallOptions) -> Result<()>;
    /// Remove anything previously written by [`install_hooks`].
    fn uninstall_hooks(&self) -> Result<()>;
    /// Detect whether the provider's hooks are currently wired up. Used for status
    /// reporting and idempotent installs.
    fn hooks_are_installed(&self) -> Result<bool>;
}
