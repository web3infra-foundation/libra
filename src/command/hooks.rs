//! Unified hook command entrypoint for provider adapters.

use anyhow::{Result, anyhow, bail};
use clap::{Parser, Subcommand};

use crate::internal::ai::hooks::{
    ProviderHookCommand, ProviderInstallOptions, find_provider, process_hook_event_from_stdin,
    supported_provider_names,
};

#[derive(Parser, Debug)]
pub struct HooksCommand {
    #[arg(help = "Hook provider identifier")]
    pub provider: String,
    #[command(subcommand)]
    pub command: HookSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum HookSubcommand {
    #[command(about = "Handle SessionStart lifecycle event")]
    SessionStart(HookEventArgs),
    #[command(about = "Handle TurnStart lifecycle event")]
    Prompt(HookEventArgs),
    #[command(about = "Handle ToolUse lifecycle event")]
    ToolUse(HookEventArgs),
    #[command(about = "Handle TurnEnd lifecycle event")]
    Stop(HookEventArgs),
    #[command(about = "Handle SessionEnd lifecycle event")]
    SessionEnd(HookEventArgs),
    #[command(about = "Handle ModelUpdate lifecycle event")]
    ModelUpdate(HookEventArgs),
    #[command(about = "Handle Compaction lifecycle event")]
    Compaction(HookEventArgs),
    #[command(about = "Install provider hook forwarding into provider settings")]
    Install(InstallHookArgs),
    #[command(about = "Uninstall provider hook forwarding from provider settings")]
    Uninstall,
    #[command(about = "Print whether provider hooks are installed")]
    IsInstalled,
}

#[derive(Parser, Debug, Clone)]
pub struct HookEventArgs {}

#[derive(Parser, Debug, Clone)]
pub struct InstallHookArgs {
    #[arg(
        long,
        default_value = "libra",
        help = "Command prefix used when generating provider hook command entries"
    )]
    pub command_prefix: String,
    #[arg(
        long,
        help = "Optional timeout in seconds for providers that support command-level hook timeouts"
    )]
    pub timeout: Option<u64>,
}

pub async fn execute(cmd: HooksCommand) -> Result<()> {
    let supported = supported_provider_names().join(", ");
    let provider = find_provider(&cmd.provider).ok_or_else(|| {
        anyhow!(
            "unsupported hook provider '{}'; supported providers: {}",
            cmd.provider,
            supported,
        )
    })?;

    match cmd.command {
        HookSubcommand::SessionStart(_) => {
            execute_event_command(provider, ProviderHookCommand::SessionStart).await
        }
        HookSubcommand::Prompt(_) => {
            execute_event_command(provider, ProviderHookCommand::Prompt).await
        }
        HookSubcommand::ToolUse(_) => {
            execute_event_command(provider, ProviderHookCommand::ToolUse).await
        }
        HookSubcommand::ModelUpdate(_) => {
            execute_event_command(provider, ProviderHookCommand::ModelUpdate).await
        }
        HookSubcommand::Compaction(_) => {
            execute_event_command(provider, ProviderHookCommand::Compaction).await
        }
        HookSubcommand::Stop(_) => execute_event_command(provider, ProviderHookCommand::Stop).await,
        HookSubcommand::SessionEnd(_) => {
            execute_event_command(provider, ProviderHookCommand::SessionEnd).await
        }
        HookSubcommand::Install(args) => provider.install_hooks(&ProviderInstallOptions {
            command_prefix: args.command_prefix,
            timeout_secs: args.timeout,
        }),
        HookSubcommand::Uninstall => provider.uninstall_hooks(),
        HookSubcommand::IsInstalled => {
            println!(
                "{}",
                if provider.hooks_are_installed()? {
                    "true"
                } else {
                    "false"
                }
            );
            Ok(())
        }
    }
}

async fn execute_event_command(
    provider: &dyn crate::internal::ai::hooks::HookProvider,
    command: ProviderHookCommand,
) -> Result<()> {
    if !provider.supported_commands().contains(&command) {
        bail!(
            "provider '{}' does not support '{}'",
            provider.provider_name(),
            command
        );
    }
    process_hook_event_from_stdin(command.lifecycle_event_kind(), provider).await
}
