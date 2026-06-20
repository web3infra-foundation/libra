//! `libra hooks <provider> <subcommand>` — compatibility entry point invoked
//! by hook configurations the existing `HookProvider`s install (Claude Code's
//! `.claude/settings.json`, Gemini's hooks). Adds the `Commands::Hooks(...)`
//! variant promised in `docs/development/commands/_general.md` (sections 1.2 and 6.1).
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

/// `--help` examples shown in `libra hooks --help` output.
///
/// `hooks` is the entry point invoked by external AI agent hook
/// configurations (Claude Code, Gemini) — it reads the hook event JSON
/// on stdin and records it into the libra session store. Each provider
/// exposes the seven Claude-Code-style lifecycle events; the banner
/// pins the most commonly wired ones (`session-start`, `prompt`,
/// `tool-use`, `stop`, `session-end`) for both providers so operators
/// see what to put in their hook config without reading the design
/// doc. Cross-cutting `--help` EXAMPLES rollout per
/// `docs/development/commands/_general.md` item B.
pub const HOOKS_EXAMPLES: &str = "\
EXAMPLES:
    libra hooks claude session-start         Claude SessionStart hook entry (reads JSON on stdin)
    libra hooks claude prompt                Claude UserPromptSubmit hook entry
    libra hooks claude tool-use              Claude PreToolUse / PostToolUse hook entry
    libra hooks claude stop                  Claude Stop hook entry
    libra hooks claude session-end           Claude SessionEnd hook entry
    libra hooks gemini session-start         Gemini SessionStart hook entry
    libra hooks gemini prompt                Gemini UserPromptSubmit hook entry
    libra hooks gemini tool-use              Gemini PreToolUse / PostToolUse hook entry";

#[derive(Args, Debug)]
#[command(after_help = HOOKS_EXAMPLES)]
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
