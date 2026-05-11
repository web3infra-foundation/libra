//! `libra agent rpc …` — discover and invoke external
//! `libra-agent-<name>` RPC binaries. Phase 4.5 (entire.md §14.4
//! item 5).

use clap::{Args, Subcommand};
use serde::Serialize;

use crate::{
    internal::ai::observed_agents::{RpcAgent, discover_rpc_agents},
    utils::{
        error::{CliError, CliResult},
        output::{OutputConfig, emit_json_data},
    },
};

#[derive(Subcommand, Debug)]
pub enum AgentRpcSubcommand {
    /// List every `libra-agent-<name>` binary discovered on `$PATH`.
    #[command(about = "List discovered libra-agent-* binaries on PATH")]
    List(AgentRpcListArgs),
    /// Spawn a binary and invoke a single JSON-RPC method against it.
    /// Exits non-zero if the binary returns an `error` frame.
    #[command(about = "Invoke a single JSON-RPC method on a libra-agent binary")]
    Invoke(AgentRpcInvokeArgs),
}

#[derive(Args, Debug)]
pub struct AgentRpcListArgs {}

#[derive(Args, Debug)]
pub struct AgentRpcInvokeArgs {
    /// Slug after `libra-agent-`. The binary must already be on
    /// `$PATH`.
    pub slug: String,
    /// JSON-RPC method name (e.g. `provider_kind`,
    /// `read_transcript`, `protected_dirs`).
    pub method: String,
    /// Optional JSON params object. Defaults to `null`.
    #[arg(long, value_name = "JSON")]
    pub params: Option<String>,
}

#[derive(Debug, Serialize)]
struct RpcBinaryRow {
    slug: String,
    binary_path: String,
}

pub async fn execute_safe(cmd: AgentRpcSubcommand, output: &OutputConfig) -> CliResult<()> {
    match cmd {
        AgentRpcSubcommand::List(args) => list(args, output),
        AgentRpcSubcommand::Invoke(args) => invoke(args, output),
    }
}

fn list(_args: AgentRpcListArgs, output: &OutputConfig) -> CliResult<()> {
    let binaries = discover_rpc_agents();
    let rows: Vec<RpcBinaryRow> = binaries
        .iter()
        .map(|b| RpcBinaryRow {
            slug: b.slug.clone(),
            binary_path: b.binary_path.display().to_string(),
        })
        .collect();
    if output.is_json() {
        return emit_json_data("agent_rpc_binaries", &rows, output);
    }
    if output.quiet {
        return Ok(());
    }
    if rows.is_empty() {
        println!("(no libra-agent-* binaries discovered on PATH)");
        return Ok(());
    }
    println!("{:<24}  binary_path", "slug");
    for row in &rows {
        println!("{:<24}  {}", row.slug, row.binary_path);
    }
    Ok(())
}

fn invoke(args: AgentRpcInvokeArgs, output: &OutputConfig) -> CliResult<()> {
    let params = match args.params.as_deref() {
        Some(s) => Some(
            serde_json::from_str(s)
                .map_err(|e| CliError::command_usage(format!("--params is not valid JSON: {e}")))?,
        ),
        None => None,
    };
    let binary = discover_rpc_agents()
        .into_iter()
        .find(|b| b.slug == args.slug)
        .ok_or_else(|| {
            CliError::fatal(format!("no libra-agent-{} binary found on PATH", args.slug))
        })?;
    let mut agent = RpcAgent::spawn(binary)
        .map_err(|e| CliError::fatal(format!("spawn libra-agent-{}: {e}", args.slug)))?;

    // Capability negotiation is mandatory before any other invocation.
    let caps = agent
        .negotiate_capabilities()
        .map_err(|e| CliError::fatal(format!("capabilities negotiation failed: {e}")))?;
    if args.method != "capabilities" && !caps.iter().any(|m| m == &args.method) {
        return Err(CliError::fatal(format!(
            "binary libra-agent-{} does not advertise method '{}' (capabilities: {:?})",
            args.slug, args.method, caps
        )));
    }

    let result = if args.method == "capabilities" {
        // Caller wanted to see the capability set itself — return it
        // verbatim so scripted consumers don't have to make a second
        // call.
        serde_json::json!({"methods": caps})
    } else {
        agent
            .invoke(&args.method, params)
            .map_err(|e| CliError::fatal(format!("RPC invoke failed: {e}")))?
    };

    if output.is_json() {
        let payload = serde_json::json!({
            "slug": args.slug,
            "method": args.method,
            "result": result,
        });
        return emit_json_data("agent_rpc_invoke", &payload, output);
    }
    if !output.quiet {
        let pretty = serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string());
        println!("{pretty}");
    }
    Ok(())
}
