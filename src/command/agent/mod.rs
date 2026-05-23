//! Top-level `libra agent` command surface.
//!
//! The Phase 1 cut implements parsing, the read-only `status` subcommand, and
//! stub handlers for the rest of the CLI surface so users can discover the
//! shape via `libra agent --help`. Subsequent phases fill in checkpoint /
//! session / hook routing on top of this scaffold; see
//! `docs/improvement/entire.md` section 9.

use clap::{Args, Subcommand};

use crate::{
    internal::ai::{
        hooks::{
            provider::{HookProvider, ProviderInstallOptions},
            providers::find_provider,
        },
        observed_agents::{AgentKind, is_preview},
    },
    utils::{
        error::{CliError, CliResult},
        output::OutputConfig,
    },
};

mod checkpoint;
mod clean;
mod doctor;
mod hooks;
mod push;
mod rpc;
mod session;
mod status;

/// `--help` examples shown in `libra agent --help` output.
///
/// `agent` is the operator surface for the external Agent capture
/// pipeline. It exposes eight visible sub-commands (status, enable,
/// disable, session, checkpoint, clean, doctor, push, rpc) plus a
/// hidden `hooks` entry point invoked by installed provider hooks.
/// The banner pins the canonical invocation per sub-command plus the
/// `--all` clean form, a named `--remote` push, and a JSON variant
/// for agents so users see all supported forms without reading the
/// design doc. Cross-cutting `--help` EXAMPLES rollout per
/// `docs/improvement/README.md` item B.
pub const AGENT_EXAMPLES: &str = "\
EXAMPLES:
    libra agent status                              Show captured-session counts and recent checkpoint summary
    libra agent enable --agent claude               Enable Claude Code capture and install its hooks
    libra agent enable                              Enable every stable external agent
    libra agent disable --agent claude              Disable Claude Code capture and uninstall its hooks
    libra agent session list                        List captured sessions
    libra agent checkpoint list                     List captured checkpoints
    libra agent checkpoint show <id>                Show a single checkpoint by id
    libra agent checkpoint rewind <id>              Replay a checkpoint as a JSONL transcript
    libra agent clean                               Drop temporary checkpoints from the most recent stopped session
    libra agent clean --all                         Drop temporary checkpoints from every stopped session
    libra agent doctor                              Diagnose hook installation and capture state
    libra agent push                                Push refs/libra/agent-traces to the default remote
    libra agent push --remote origin                Push refs/libra/agent-traces to a named remote
    libra agent rpc list                            Discover libra-agent-<name> RPC binaries on PATH
    libra agent rpc invoke <slug> <method>          Invoke a single JSON-RPC method (use --params '<json>' for arguments)
    libra agent --json status                       Structured JSON output for agents";

#[derive(Args, Debug)]
#[command(after_help = AGENT_EXAMPLES)]
pub struct AgentArgs {
    #[command(subcommand)]
    pub command: AgentSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum AgentSubcommand {
    /// Show captured-session counts and recent checkpoint summary.
    #[command(about = "Report captured external-agent session status")]
    Status(status::StatusArgs),

    /// Enable an external Agent and install its hooks.
    #[command(about = "Enable an external agent and install its hooks")]
    Enable(EnableArgs),

    /// Disable an external Agent and uninstall its hooks.
    #[command(about = "Disable an external agent and uninstall its hooks")]
    Disable(DisableArgs),

    /// Inspect captured sessions.
    #[command(subcommand, about = "Inspect captured sessions")]
    Session(session::SessionSubcommand),

    /// Inspect captured checkpoints.
    #[command(subcommand, about = "Inspect captured checkpoints")]
    Checkpoint(CheckpointSubcommand),

    /// Remove temporary checkpoints from stopped sessions.
    #[command(about = "Clean up temporary checkpoints from stopped sessions")]
    Clean(CleanArgs),

    /// Diagnose hook installation, stuck sessions, and orphan checkpoints.
    #[command(about = "Diagnose hook installation and capture state")]
    Doctor(DoctorArgs),

    /// Push `refs/libra/agent-traces` to a remote.
    #[command(about = "Push refs/libra/agent-traces to a remote")]
    Push(PushArgs),

    /// Internal hook entry point (called by hook configs installed by `enable`).
    #[command(subcommand, about = "Hook entry point", hide = true)]
    Hooks(hooks::AgentHooksSubcommand),

    /// Discover and invoke external `libra-agent-<name>` RPC binaries.
    /// Phase 4.5 (entire.md §14.4 item 5).
    #[command(subcommand, about = "External libra-agent-<name> RPC")]
    Rpc(rpc::AgentRpcSubcommand),
}

#[derive(Args, Debug)]
pub struct EnableArgs {
    /// One or more agent names. Empty means "all stable agents".
    #[arg(long = "agent", value_name = "NAME")]
    pub agents: Vec<String>,
}

#[derive(Args, Debug)]
pub struct DisableArgs {
    /// One or more agent names to disable. Empty means "all stable agents"
    #[arg(long = "agent", value_name = "NAME")]
    pub agents: Vec<String>,
}

#[derive(Args, Debug)]
pub struct CleanArgs {
    /// Drop temporary checkpoints from every stopped session, not just the
    /// most recent.
    #[arg(long)]
    pub all: bool,
}

#[derive(Args, Debug)]
pub struct DoctorArgs {}

#[derive(Args, Debug)]
pub struct PushArgs {
    /// Remote name to push refs/libra/agent-traces to (default: origin)
    #[arg(long, value_name = "NAME")]
    pub remote: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum CheckpointSubcommand {
    /// List captured checkpoints, newest first.
    #[command(about = "List captured checkpoints")]
    List(CheckpointListArgs),
    /// Show a single checkpoint's metadata and tree summary.
    #[command(about = "Show checkpoint metadata")]
    Show(CheckpointShowArgs),
    /// Inspect what `rewind` would do (`--apply` to actually run; v1 only
    /// restores the working tree and leaves the agent's own transcript file
    /// untouched, with a warning).
    #[command(
        about = "Rewind a checkpoint (v1: --dry-run by default; --apply restores worktree only)"
    )]
    Rewind(CheckpointRewindArgs),
}

#[derive(Args, Debug)]
pub struct CheckpointListArgs {
    /// Filter checkpoints to those belonging to a single session id
    #[arg(long, value_name = "ID")]
    pub session: Option<String>,
}

#[derive(Args, Debug)]
pub struct CheckpointShowArgs {
    /// Checkpoint identifier returned by `libra agent checkpoint list`
    #[arg(value_name = "CHECKPOINT_ID")]
    pub checkpoint_id: String,
}

#[derive(Args, Debug)]
pub struct CheckpointRewindArgs {
    /// Checkpoint identifier to rewind to (from `libra agent checkpoint list`)
    #[arg(value_name = "CHECKPOINT_ID")]
    pub checkpoint_id: String,
    /// Show the impact without modifying anything (default)
    #[arg(long, conflicts_with = "apply")]
    pub dry_run: bool,
    /// Actually restore. v1 limits this to working-tree restore; the agent's transcript file is NOT rewritten and a warning is printed
    #[arg(long)]
    pub apply: bool,
}

/// Run an `agent` subcommand.
///
/// V1 ships a stable status path and stub handlers that emit a clear
/// "not yet implemented in this phase" message rather than panicking — this
/// keeps the CLI discoverable while later phases land checkpoint / session
/// machinery.
pub async fn execute_safe(args: AgentArgs, output: &OutputConfig) -> CliResult<()> {
    match args.command {
        AgentSubcommand::Status(args) => status::execute_safe(args, output).await,
        AgentSubcommand::Enable(args) => enable_agents(&args.agents, output),
        AgentSubcommand::Disable(args) => disable_agents(&args.agents, output),
        AgentSubcommand::Session(cmd) => session::execute_safe(cmd, output).await,
        AgentSubcommand::Checkpoint(cmd) => checkpoint::execute_safe(cmd, output).await,
        AgentSubcommand::Clean(cmd) => clean::execute_safe(cmd, output).await,
        AgentSubcommand::Doctor(cmd) => doctor::execute_safe(cmd, output).await,
        AgentSubcommand::Push(cmd) => push::execute_safe(cmd, output).await,
        AgentSubcommand::Hooks(cmd) => hooks::execute_safe(cmd, output).await,
        AgentSubcommand::Rpc(cmd) => rpc::execute_safe(cmd, output).await,
    }
}

/// Set of stable agent slugs whose `HookProvider` is fully installable today.
/// Phase 1 only ships Claude Code and Gemini stable; everything else is
/// preview and surfaces as a clear "not yet" rather than a silent no-op.
const STABLE_AGENT_SLUGS: &[&str] = &["claude-code", "gemini"];

fn enable_agents(agents: &[String], output: &OutputConfig) -> CliResult<()> {
    install_or_uninstall(agents, output, true)
}

fn disable_agents(agents: &[String], output: &OutputConfig) -> CliResult<()> {
    install_or_uninstall(agents, output, false)
}

fn install_or_uninstall(agents: &[String], output: &OutputConfig, install: bool) -> CliResult<()> {
    let verb_present = if install { "enable" } else { "disable" };
    let verb_past = if install { "enabled" } else { "disabled" };

    let resolved = resolve_agent_slugs(agents)?;
    if resolved.is_empty() {
        return Err(CliError::fatal(format!(
            "no installable agents to {verb_present}"
        )));
    }

    for slug in resolved {
        let kind = AgentKind::from_cli_slug(&slug).ok_or_else(|| {
            CliError::fatal(format!(
                "unknown agent '{slug}'; supported: {}",
                STABLE_AGENT_SLUGS.join(", "),
            ))
        })?;
        let provider_name = provider_name_for(kind);
        let Some(provider) = find_provider(provider_name) else {
            // Preview-only adapter: surface a friendly diagnostic on stderr
            // (not stdout — this is informational, not program output) and
            // don't fail the whole batch. Phase 3.1 added the preview
            // registry in `observed_agents::preview`; phase 4 will flesh
            // out the install path for each one.
            if !output.quiet {
                if is_preview(kind) {
                    eprintln!(
                        "libra agent {verb_present}: skipping '{slug}' \
                         (preview adapter — hook installation lands in phase 4; \
                         see observed_agents::preview)"
                    );
                } else {
                    eprintln!(
                        "libra agent {verb_present}: skipping '{slug}' \
                         (no HookProvider registered for this agent)"
                    );
                }
            }
            continue;
        };
        if install {
            install_provider_hooks(provider)
                .map_err(|err| CliError::fatal(format!("failed to enable '{slug}': {err}")))?;
            if !output.quiet {
                println!("libra agent enable: {verb_past} '{slug}' (provider hooks installed)");
            }
        } else {
            provider
                .uninstall_hooks()
                .map_err(|err| CliError::fatal(format!("failed to disable '{slug}': {err}")))?;
            if !output.quiet {
                println!("libra agent disable: {verb_past} '{slug}' (provider hooks removed)");
            }
        }
    }
    Ok(())
}

/// `agent.as_cli_slug()` returns hyphens (`claude-code`); the existing
/// `HookProvider` registry keys on shorter names (`claude`, `gemini`). This
/// helper bridges the two.
fn provider_name_for(kind: AgentKind) -> &'static str {
    match kind {
        AgentKind::ClaudeCode => "claude",
        AgentKind::Gemini => "gemini",
        // Preview adapters don't have a registered HookProvider in Phase 1.
        AgentKind::Cursor => "cursor",
        AgentKind::Codex => "codex",
        AgentKind::OpenCode => "opencode",
        AgentKind::Copilot => "copilot",
        AgentKind::FactoryAi => "factory-ai",
    }
}

fn install_provider_hooks(provider: &dyn HookProvider) -> anyhow::Result<()> {
    // Use the running binary's path so installed hooks point at exactly the
    // libra the user is invoking — falling back to the bare `libra` symbol
    // (which `HookProvider`s will substitute) if `current_exe` fails.
    let binary_path = std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(str::to_string));
    let opts = ProviderInstallOptions {
        binary_path,
        timeout_secs: None,
    };
    provider.install_hooks(&opts)
}

/// Validate `agents`. Empty input expands to every stable slug; non-empty
/// passes through unchanged after a known-slug check.
fn resolve_agent_slugs(agents: &[String]) -> CliResult<Vec<String>> {
    if agents.is_empty() {
        return Ok(STABLE_AGENT_SLUGS
            .iter()
            .map(|s| (*s).to_string())
            .collect());
    }
    let mut out = Vec::with_capacity(agents.len());
    for slug in agents {
        if AgentKind::from_cli_slug(slug).is_none() {
            return Err(CliError::fatal(format!(
                "unknown agent '{slug}'; supported: {}",
                STABLE_AGENT_SLUGS.join(", ")
            )));
        }
        out.push(slug.clone());
    }
    Ok(out)
}

/// Helper used by stubs that should still surface as a non-zero exit when
/// called outside an interactive shell. Reserved for future expansions of
/// the agent CLI that need an explicit refuse path.
#[allow(dead_code)]
fn refuse(message: &str) -> CliResult<()> {
    Err(CliError::fatal(message.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_agent_slugs_expands_empty_to_stable() {
        let resolved = resolve_agent_slugs(&[]).expect("empty resolves cleanly");
        assert_eq!(resolved, vec!["claude-code", "gemini"]);
    }

    #[test]
    fn resolve_agent_slugs_passes_known() {
        let resolved =
            resolve_agent_slugs(&["claude-code".to_string()]).expect("known slug resolves");
        assert_eq!(resolved, vec!["claude-code"]);
    }

    #[test]
    fn resolve_agent_slugs_rejects_unknown() {
        let err = resolve_agent_slugs(&["bogus".to_string()]).unwrap_err();
        assert!(err.to_string().contains("unknown agent 'bogus'"));
    }

    #[test]
    fn provider_name_for_maps_stable() {
        assert_eq!(provider_name_for(AgentKind::ClaudeCode), "claude");
        assert_eq!(provider_name_for(AgentKind::Gemini), "gemini");
    }
}
