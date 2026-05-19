//! `libra hooks <provider> <subcommand>` — compatibility entry point invoked
//! by hook configurations the existing `HookProvider`s install (Claude Code's
//! `.claude/settings.json`, Gemini's hooks). Adds the `Commands::Hooks(...)`
//! variant promised in `docs/improvement/entire.md` (sections 1.2 and 6.1).
//!
//! Phase 1.1: this entry point currently delegates to the canonical
//! `process_hook_event_from_stdin`, which writes to `refs/libra/intent`
//! (`HookTarget::AiIntent`). Phase 1.5 will refactor that helper to take a
//! [`HookTarget`] and route `libra agent hooks <agent>` through the same
//! plumbing with `HookTarget::AgentTraces`. The user-facing surface stays
//! unchanged.

use clap::{Args, Subcommand};

use crate::{
    internal::ai::hooks::{
        process_hook_event_from_stdin,
        provider::ProviderHookCommand,
        providers::{claude_provider, gemini_provider},
    },
    utils::{
        error::{CliError, CliResult},
        output::OutputConfig,
    },
};

#[derive(Args, Debug)]
pub struct HooksArgs {
    #[command(subcommand)]
    pub command: HooksProviderSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum HooksProviderSubcommand {
    /// `libra hooks claude <subcommand>`. Invoked by Claude Code hook configs.
    #[command(about = "Claude Code hook entry point")]
    Claude {
        #[command(subcommand)]
        command: ProviderHookSubcommand,
    },
    /// `libra hooks gemini <subcommand>`. Invoked by Gemini hook configs.
    #[command(about = "Gemini hook entry point")]
    Gemini {
        #[command(subcommand)]
        command: ProviderHookSubcommand,
    },
}

#[derive(Subcommand, Debug, Clone, Copy)]
pub enum ProviderHookSubcommand {
    SessionStart,
    Prompt,
    ToolUse,
    ModelUpdate,
    Compaction,
    Stop,
    SessionEnd,
}

impl ProviderHookSubcommand {
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

pub async fn execute_safe(args: HooksArgs, _output: &OutputConfig) -> CliResult<()> {
    match args.command {
        HooksProviderSubcommand::Claude { command } => {
            run_provider_hook(claude_provider(), command).await
        }
        HooksProviderSubcommand::Gemini { command } => {
            run_provider_hook(gemini_provider(), command).await
        }
    }
}

async fn run_provider_hook(
    provider: &'static dyn crate::internal::ai::hooks::provider::HookProvider,
    sub: ProviderHookSubcommand,
) -> CliResult<()> {
    let cmd = sub.as_command();
    let expected_kind = cmd.lifecycle_event_kind();
    process_hook_event_from_stdin(cmd, expected_kind, provider)
        .await
        .map_err(|err| CliError::fatal(format!("hook ingestion failed: {err}")))
}
