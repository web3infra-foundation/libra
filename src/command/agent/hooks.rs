//! `libra agent hooks <agent> <subcommand>` — hook entry point invoked by
//! per-agent hook config that `libra agent enable` writes out.
//!
//! Phase 1 routes to [`process_hook_event_with_target`] with
//! [`HookTarget::AgentTraces`]; the runtime there does a minimal ingest
//! (parse → redact → upsert into `agent_session`). Phase 2 will extend the
//! AgentTraces branch to additionally write checkpoint commits on
//! `refs/libra/traces`.

use clap::Subcommand;

use crate::{
    internal::ai::hooks::{
        HookTarget, process_hook_event_with_target,
        provider::ProviderHookCommand,
        providers::{claude_provider, gemini_provider},
    },
    utils::{
        error::{CliError, CliResult},
        output::OutputConfig,
    },
};

#[derive(Subcommand, Debug)]
pub enum AgentHooksSubcommand {
    /// `libra agent hooks claude-code <subcommand>` family.
    #[command(about = "Claude Code hook entry points")]
    ClaudeCode {
        #[command(subcommand)]
        command: HookCommandKind,
    },
    /// `libra agent hooks gemini <subcommand>` family.
    #[command(about = "Gemini hook entry points")]
    Gemini {
        #[command(subcommand)]
        command: HookCommandKind,
    },
}

#[derive(Subcommand, Debug, Clone, Copy)]
pub enum HookCommandKind {
    SessionStart,
    Prompt,
    ToolUse,
    ModelUpdate,
    Compaction,
    Stop,
    SessionEnd,
}

impl HookCommandKind {
    fn as_command(self) -> ProviderHookCommand {
        match self {
            Self::SessionStart => ProviderHookCommand::SessionStart,
            Self::Prompt => ProviderHookCommand::Prompt,
            Self::ToolUse => ProviderHookCommand::ToolUse,
            Self::ModelUpdate => ProviderHookCommand::ModelUpdate,
            Self::Compaction => ProviderHookCommand::Compaction,
            Self::Stop => ProviderHookCommand::Stop,
            Self::SessionEnd => ProviderHookCommand::SessionEnd,
        }
    }
}

pub async fn execute_safe(cmd: AgentHooksSubcommand, _output: &OutputConfig) -> CliResult<()> {
    match cmd {
        AgentHooksSubcommand::ClaudeCode { command } => run(claude_provider(), command).await,
        AgentHooksSubcommand::Gemini { command } => run(gemini_provider(), command).await,
    }
}

async fn run(
    provider: &'static dyn crate::internal::ai::hooks::provider::HookProvider,
    sub: HookCommandKind,
) -> CliResult<()> {
    let cmd = sub.as_command();
    let expected_kind = cmd.lifecycle_event_kind();
    process_hook_event_with_target(cmd, expected_kind, provider, HookTarget::AgentTraces)
        .await
        .map_err(|err| CliError::fatal(format!("agent hook ingestion failed: {err}")))
}
