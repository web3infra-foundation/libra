//! Claude Code lifecycle hook provider facade.
mod parser;
mod settings;

use anyhow::Result;

use super::super::{
    lifecycle::{LifecycleEvent, SessionHookEnvelope},
    provider::{
        CANONICAL_DEDUP_IDENTITY_KEYS, HookProvider, ProviderHookCommand, ProviderInstallOptions,
    },
};

pub static CLAUDE_PROVIDER: ClaudeProvider = ClaudeProvider;

const SUPPORTED_COMMANDS: &[ProviderHookCommand] = &[
    ProviderHookCommand::SessionStart,
    ProviderHookCommand::Prompt,
    ProviderHookCommand::ToolUse,
    ProviderHookCommand::ModelUpdate,
    ProviderHookCommand::Compaction,
    ProviderHookCommand::Stop,
    ProviderHookCommand::SessionEnd,
];

#[derive(Debug, Clone, Copy)]
pub struct ClaudeProvider;

impl HookProvider for ClaudeProvider {
    fn provider_name(&self) -> &'static str {
        "claude"
    }

    fn source_name(&self) -> &'static str {
        "claude_code_hook"
    }

    fn supported_commands(&self) -> &'static [ProviderHookCommand] {
        SUPPORTED_COMMANDS
    }

    fn parse_hook_event(
        &self,
        hook_event_name: &str,
        envelope: &SessionHookEnvelope,
    ) -> Result<LifecycleEvent> {
        parser::parse_claude_hook_event(hook_event_name, envelope)
    }

    fn dedup_identity_keys(&self) -> &'static [&'static str] {
        CANONICAL_DEDUP_IDENTITY_KEYS
    }

    fn lifecycle_fallback_events(&self) -> &'static [&'static str] {
        parser::CLAUDE_LIFECYCLE_FALLBACK_EVENTS
    }

    fn install_hooks(&self, options: &ProviderInstallOptions) -> Result<()> {
        settings::install_claude_hooks(options)
    }

    fn uninstall_hooks(&self) -> Result<()> {
        settings::uninstall_claude_hooks()
    }

    fn hooks_are_installed(&self) -> Result<bool> {
        settings::claude_hooks_are_installed()
    }
}
