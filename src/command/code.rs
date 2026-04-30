//! # Code Command — Interactive AI-Powered Coding Sessions
//!
//! This module implements the `libra code` subcommand, which is the primary entry point
//! for AI-agent-driven and human-collaborative development within a Libra repository.
//!
//! ## Architecture Overview
//!
//! The command orchestrates several concurrent subsystems:
//!
//! - **TUI (Terminal UI)**: A `ratatui`/`crossterm`-based interactive terminal interface
//!   that renders the chat conversation, tool outputs, and approval prompts.
//! - **Web Server**: An embedded `axum` HTTP server that serves the Next.js static export
//!   from `web/out/`, providing a browser-based UI alternative.
//! - **MCP Server**: A Model Context Protocol server (using `rmcp`) that exposes Libra's
//!   tools (read, grep, patch, shell, etc.) over Streamable HTTP or Stdio transport,
//!   enabling integration with external AI clients such as Claude Desktop.
//! - **AI Agent**: A tool-calling loop powered by configurable LLM providers (Gemini,
//!   OpenAI, Anthropic, DeepSeek, Kimi, Zhipu, Ollama) or the managed Codex runtime.
//!
//! ## Supported Modes
//!
//! The command supports three mutually exclusive operating modes:
//!
//! | Mode | Flag | Description |
//! |------|------|-------------|
//! | **TUI** (default) | *(none)* | Full interactive terminal UI with background web + MCP servers |
//! | **Web-only** | `--web` | Headless web server + MCP server; no terminal UI |
//! | **Stdio** | `--stdio` | MCP server over stdin/stdout for AI client integration |
//!
//! ## Provider Dispatch
//!
//! The `--provider` flag selects the AI backend. Each provider follows the same pattern:
//! 1. Create a client from environment variables (API keys).
//! 2. Instantiate a completion model with the selected (or default) model name.
//! 3. Pass the model into the shared `run_tui_with_model` function.
//!
//! The `codex` provider bypasses the generic completion model path and uses its
//! managed app-server runtime with a dedicated execution flow.
//!
//! ## Sandbox & Approval
//!
//! Tool execution is governed by a layered sandbox and approval system:
//! - **SandboxPolicy**: Controls filesystem and network access (read-only for review/research,
//!   workspace-write for dev mode).
//! - **AskForApproval**: Determines when to prompt the user for tool execution approval
//!   (never, on-failure, on-request, unless-trusted).
//!
//! ## Session Persistence
//!
//! Conversation history is persisted via `SessionStore` under the `.libra/` storage
//! directory, supporting `--resume <thread_id>` to continue a canonical Libra thread.
//!
//! Cross-references for agents extending this command:
//! - Agent workflow and object model: `docs/agent/agent-workflow.md`
//! - MCP upgrade and transport notes: `docs/agent/mcp-upgrade-report.md`
//! - IntentSpec contract examples: `docs/agent/intentspec_typical.yaml`

use std::{
    collections::BTreeMap,
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use chrono::Utc;
use clap::{Parser, ValueEnum};
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    service::TowerToHyperService,
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpService, session::local::LocalSessionManager,
};
use serde::{Deserialize, Serialize};
use tokio::{
    process::{Child, Command},
    sync::oneshot,
    time::{Duration, Instant, sleep},
};
use tokio_tungstenite::connect_async;
use url::Url;
use uuid::Uuid;

#[cfg(feature = "test-provider")]
use crate::internal::ai::providers::fake::{Client as FakeClient, FAKE_DEFAULT_MODEL};
use crate::{
    cli_error,
    command::code_control_files::{
        ControlInfo, ControlLockError, ControlLockGuard, ControlPaths, acquire_control_lock,
        cleanup_control_files, ensure_control_token_file, resolve_control_paths,
        write_control_info,
    },
    internal::{
        ai::{
            agent::{
                ToolLoopConfig,
                profile::{AgentProfileRouter, load_profiles},
            },
            client::CompletionClient,
            codex as agent_codex,
            commands::{CommandDispatcher, load_commands},
            completion::{
                CompletionError, CompletionModel, CompletionReasoningEffort, CompletionRequest,
                CompletionResponse, CompletionThinking, CompletionUsage,
            },
            history::HistoryManager,
            hooks::HookRunner,
            mcp::server::LibraMcpServer,
            projection::{ProjectionRebuilder, ProjectionResolver, ThreadBundle},
            prompt::{ContextMode, SystemPromptBuilder},
            providers::{
                anthropic::{CLAUDE_3_5_SONNET, Client as AnthropicClient},
                deepseek::client::Client as DeepSeekClient,
                gemini::{Client as GeminiClient, GEMINI_2_5_FLASH},
                kimi::{Client as KimiClient, KIMI_K2_6},
                ollama::Client as OllamaClient,
                openai::{Client as OpenAIClient, GPT_4O_MINI},
                zhipu::{Client as ZhipuClient, GLM_5},
            },
            runtime::{ToolBoundaryRuntime, TracingAuditSink},
            sandbox::{
                ApprovalStore, AskForApproval, ExecApprovalRequest, SandboxPermissions,
                SandboxPolicy, ToolApprovalContext, ToolRuntimeContext, ToolSandboxContext,
            },
            session::{SessionState, SessionStore},
            tools::{
                ToolRegistry, ToolRegistryBuilder,
                context::UserInputRequest,
                handlers::{
                    ApplyPatchHandler, GrepFilesHandler, ListDirHandler, McpBridgeHandler,
                    PlanHandler, ReadFileHandler, RequestUserInputHandler, SearchFilesHandler,
                    ShellHandler, SubmitIntentDraftHandler, SubmitPlanDraftHandler,
                    SubmitTaskCompleteHandler, WebSearchHandler,
                },
            },
            web::{
                WebServerHandle, WebServerOptions,
                code_ui::{
                    CodeUiCapabilities, CodeUiControllerKind, CodeUiInitialController,
                    CodeUiProviderAdapter, CodeUiProviderInfo, CodeUiRuntimeHandle, CodeUiSession,
                    CodeUiSessionStatus, CodeUiTranscriptEntry, CodeUiTranscriptEntryKind,
                    ReadOnlyCodeUiAdapter, initial_snapshot, snapshot_from_thread_bundle,
                },
                start as start_web_server,
            },
        },
        db::establish_connection,
        tui::{
            App, AppConfig, ExitReason, Tui, TuiCodeUiAdapter, control::TuiControlCommand,
            tui_init, tui_restore,
        },
    },
    utils::{
        client_storage::ClientStorage,
        error::{CliError, CliResult, StableErrorCode},
        output::OutputConfig,
        storage::local::LocalStorage,
        util::try_get_storage_path,
    },
};

// ---------------------------------------------------------------------------
// Constants — default network ports, bind address, and Codex startup tuning
// ---------------------------------------------------------------------------

/// Default port for the embedded web server serving the Next.js static export.
const DEFAULT_WEB_PORT: u16 = 3000;

/// Default port for the MCP (Model Context Protocol) HTTP server.
const DEFAULT_MCP_PORT: u16 = 6789;

/// Default network interface to bind servers to (localhost only).
const DEFAULT_BIND_HOST: &str = "127.0.0.1";

/// Default executable name for the Codex CLI app-server.
const DEFAULT_CODEX_BIN: &str = "codex";

/// Maximum time to wait for the Codex app-server WebSocket to become reachable.
const CODEX_STARTUP_TIMEOUT: Duration = Duration::from_secs(15);

/// Interval between WebSocket connectivity checks during Codex startup.
const CODEX_STARTUP_POLL_INTERVAL: Duration = Duration::from_millis(200);

// ---------------------------------------------------------------------------
// Enums — provider selection, context mode, and approval policy
// ---------------------------------------------------------------------------

/// Available AI provider backends for the `libra code` command.
///
/// Each variant maps to a specific LLM client implementation. The provider
/// determines which API key environment variable is required and which
/// default model is used when `--model` is omitted.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum CodeProvider {
    Gemini,
    Openai,
    Anthropic,
    Deepseek,
    Kimi,
    Zhipu,
    Ollama,
    Codex,
    #[cfg(feature = "test-provider")]
    #[value(name = "fake", hide = true)]
    Fake,
}

/// Operating context that shapes the agent's system prompt and sandbox policy.
///
/// - `Dev`: Full read-write access to the workspace; the agent can modify files.
/// - `Review`: Read-only sandbox; the agent focuses on code review feedback.
/// - `Research`: Read-only sandbox; the agent focuses on codebase exploration.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum CodeContext {
    #[value(alias = "development")]
    Dev,
    #[value(alias = "code-review")]
    Review,
    #[value(alias = "explore")]
    Research,
}

/// Local TUI automation control mode.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlMode {
    /// Keep the current loopback-only read behavior; no write token is created.
    Observe,
    /// Enable local automation write control with token and controller checks.
    Write,
}

/// Ollama-specific thinking/reasoning mode.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum OllamaThinkingArg {
    /// Let Ollama decide by omitting the `think` field.
    Auto,
    /// Disable thinking for faster local tool-calling responses.
    Off,
    /// Enable thinking without specifying a depth.
    On,
    /// Request low thinking depth.
    Low,
    /// Request medium thinking depth.
    Medium,
    /// Request high thinking depth.
    High,
}

impl From<OllamaThinkingArg> for CompletionThinking {
    fn from(value: OllamaThinkingArg) -> Self {
        match value {
            OllamaThinkingArg::Auto => CompletionThinking::Auto,
            OllamaThinkingArg::Off => CompletionThinking::Disabled,
            OllamaThinkingArg::On => CompletionThinking::Enabled,
            OllamaThinkingArg::Low => CompletionThinking::Low,
            OllamaThinkingArg::Medium => CompletionThinking::Medium,
            OllamaThinkingArg::High => CompletionThinking::High,
        }
    }
}

/// DeepSeek-specific thinking mode.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum DeepSeekThinkingArg {
    /// Send `thinking: {"type": "enabled"}` to DeepSeek.
    Enabled,
    /// Send `thinking: {"type": "disabled"}` to DeepSeek.
    Disabled,
}

impl From<DeepSeekThinkingArg> for CompletionThinking {
    fn from(value: DeepSeekThinkingArg) -> Self {
        match value {
            DeepSeekThinkingArg::Enabled => CompletionThinking::Enabled,
            DeepSeekThinkingArg::Disabled => CompletionThinking::Disabled,
        }
    }
}

/// Kimi-specific thinking mode.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum KimiThinkingArg {
    /// Send `thinking: {"type": "enabled"}` to Kimi.
    Enabled,
    /// Send `thinking: {"type": "disabled"}` to Kimi.
    Disabled,
}

impl From<KimiThinkingArg> for CompletionThinking {
    fn from(value: KimiThinkingArg) -> Self {
        match value {
            KimiThinkingArg::Enabled => CompletionThinking::Enabled,
            KimiThinkingArg::Disabled => CompletionThinking::Disabled,
        }
    }
}

/// DeepSeek-specific reasoning effort.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum DeepSeekReasoningEffortArg {
    Low,
    Medium,
    High,
    #[value(alias = "xhigh")]
    Max,
}

impl From<DeepSeekReasoningEffortArg> for CompletionReasoningEffort {
    fn from(value: DeepSeekReasoningEffortArg) -> Self {
        match value {
            DeepSeekReasoningEffortArg::Low => CompletionReasoningEffort::Low,
            DeepSeekReasoningEffortArg::Medium => CompletionReasoningEffort::Medium,
            DeepSeekReasoningEffortArg::High => CompletionReasoningEffort::High,
            DeepSeekReasoningEffortArg::Max => CompletionReasoningEffort::Max,
        }
    }
}

/// User-facing approval policy controlling when tool execution requires
/// explicit human confirmation in the TUI.
///
/// This enum is the CLI-facing representation; it converts into the internal
/// [`AskForApproval`] enum via the `From` impl below.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum CodeApprovalPolicy {
    /// Never prompt; dangerous commands are rejected.
    Never,
    /// Never prompt; allow every command for this interactive session.
    #[value(
        alias = "allow-all",
        alias = "allow_all",
        alias = "always",
        alias = "accept"
    )]
    AllowAll,
    /// Prompt only when retrying after sandbox denial.
    #[value(alias = "on-failure")]
    OnFailure,
    /// Run inside sandbox by default; prompt when escalation or policy requires it.
    #[value(alias = "on-request")]
    OnRequest,
    /// Prompt for non-trusted operations (safe read commands are auto-allowed).
    #[value(alias = "unless-trusted", alias = "untrusted")]
    Untrusted,
}

/// Developer-selected network access policy for TUI execution.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum CodeNetworkAccess {
    /// Allow shell and gate tasks to use network access.
    Allow,
    /// Deny network access for shell and gate tasks.
    Deny,
}

impl CodeNetworkAccess {
    fn is_allowed(self) -> bool {
        matches!(self, Self::Allow)
    }
}

impl CodeApprovalPolicy {
    fn allows_all_commands(self) -> bool {
        matches!(self, Self::AllowAll)
    }
}

/// Maps the user-facing [`CodeApprovalPolicy`] to the internal [`AskForApproval`]
/// enum used by the sandbox/approval subsystem.
impl From<CodeApprovalPolicy> for AskForApproval {
    fn from(value: CodeApprovalPolicy) -> Self {
        match value {
            CodeApprovalPolicy::Never => AskForApproval::Never,
            CodeApprovalPolicy::AllowAll => AskForApproval::OnRequest,
            CodeApprovalPolicy::OnFailure => AskForApproval::OnFailure,
            CodeApprovalPolicy::OnRequest => AskForApproval::OnRequest,
            CodeApprovalPolicy::Untrusted => AskForApproval::UnlessTrusted,
        }
    }
}

// ---------------------------------------------------------------------------
// CLI argument definition
// ---------------------------------------------------------------------------

/// Command-line arguments for `libra code`.
///
/// This struct is parsed by `clap` and drives all three operating modes
/// (TUI, web-only, stdio). Many flags are mode-specific and validated
/// at runtime by [`validate_mode_args`].
#[derive(Parser, Debug)]
pub struct CodeArgs {
    /// Run the web server only (no TUI). Alias: `--web`.
    #[arg(long, alias = "web", conflicts_with = "stdio")]
    pub web_only: bool,

    /// Port to listen on (web server)
    #[arg(short, long, default_value_t = DEFAULT_WEB_PORT)]
    pub port: u16,

    /// Host address to bind to (web server)
    #[arg(long, default_value = DEFAULT_BIND_HOST)]
    pub host: String,

    /// Working directory for the code session.
    #[arg(long)]
    pub cwd: Option<PathBuf>,

    /// Path to a libra repository. When specified, the code session uses this
    /// repository instead of discovering one from the current working directory.
    #[arg(long)]
    pub repo: Option<PathBuf>,

    /// Load provider environment variables from a dotenv-style file.
    ///
    /// Values in this file take precedence over already exported process
    /// environment variables for provider bootstrap.
    #[arg(long = "env-file", value_name = "PATH")]
    pub env_file: Option<PathBuf>,

    /// Local TUI automation control mode.
    #[arg(long, value_enum, default_value_t = ControlMode::Observe)]
    pub control: ControlMode,

    /// Path to the local automation control token file.
    #[arg(long)]
    pub control_token_file: Option<PathBuf>,

    /// Path to the local automation control discovery info file.
    #[arg(long)]
    pub control_info_file: Option<PathBuf>,

    /// AI provider backend
    #[arg(long, value_enum, default_value_t = CodeProvider::Gemini)]
    pub provider: CodeProvider,

    /// Model id (provider-specific)
    #[arg(long)]
    pub model: Option<String>,

    /// Sampling temperature
    #[arg(long)]
    pub temperature: Option<f64>,

    /// Ollama thinking mode: auto, off, on, low, medium, or high.
    ///
    /// If omitted, Ollama uses OLLAMA_THINK and then defaults to `off`.
    #[arg(long = "ollama-thinking", alias = "thinking", value_enum)]
    pub ollama_thinking: Option<OllamaThinkingArg>,

    /// Send compact Ollama tool schemas for providers that reject complex JSON schemas.
    #[arg(long = "ollama-compact-tools")]
    pub ollama_compact_tools: bool,

    /// DeepSeek thinking mode: enabled or disabled.
    #[arg(long = "deepseek-thinking", value_enum)]
    pub deepseek_thinking: Option<DeepSeekThinkingArg>,

    /// DeepSeek reasoning effort: low, medium, high, or max.
    #[arg(long = "deepseek-reasoning-effort", value_enum)]
    pub deepseek_reasoning_effort: Option<DeepSeekReasoningEffortArg>,

    /// DeepSeek stream mode: true or false.
    #[arg(long = "deepseek-stream", alias = "stream", value_name = "BOOL")]
    pub deepseek_stream: Option<bool>,

    /// Kimi thinking mode: enabled or disabled.
    #[arg(long = "kimi-thinking", value_enum)]
    pub kimi_thinking: Option<KimiThinkingArg>,

    /// Kimi stream mode: true or false. Defaults to true for Kimi.
    #[arg(long = "kimi-stream", value_name = "BOOL")]
    pub kimi_stream: Option<bool>,

    /// Test-only fake provider fixture.
    #[cfg(feature = "test-provider")]
    #[arg(long = "fake-fixture", hide = true, value_name = "PATH")]
    pub fake_fixture: Option<PathBuf>,

    /// Operating context mode (dev, review, research)
    #[arg(long, value_enum)]
    pub context: Option<CodeContext>,

    /// Resume a canonical Libra thread by thread_id.
    #[arg(long, value_name = "THREAD_ID")]
    pub resume: Option<String>,

    /// Tool approval policy:
    /// - `never`: no prompts, dangerous commands are rejected
    /// - `allow-all`: no prompts, all commands are allowed for this session
    /// - `on-failure`: prompt only for retry outside sandbox after sandbox denial
    /// - `on-request`: run sandboxed by default; prompt for escalation/policy-required cases
    /// - `untrusted`: prompt for non-trusted operations, auto-allow known-safe reads
    #[arg(long, value_enum, default_value_t = CodeApprovalPolicy::OnRequest)]
    pub approval_policy: CodeApprovalPolicy,

    /// Network access policy for TUI shell and gate execution.
    #[arg(long, value_enum, default_value_t = CodeNetworkAccess::Deny)]
    pub network_access: CodeNetworkAccess,

    /// Port to listen on (MCP server)
    #[arg(long, default_value_t = DEFAULT_MCP_PORT)]
    pub mcp_port: u16,

    /// Run the MCP server over Stdio (for Claude Desktop integration)
    #[arg(long, alias = "mcp-stdio", conflicts_with = "web_only")]
    pub stdio: bool,

    /// Provider API base URL.
    ///
    /// For Ollama, use a local/remote daemon URL such as
    /// `http://remote-host:11434/v1`, or `https://ollama.com` for direct
    /// Ollama Cloud API access with `OLLAMA_API_KEY`.
    #[arg(long)]
    pub api_base: Option<String>,

    /// Codex executable used to launch the managed app-server.
    #[arg(long, default_value = DEFAULT_CODEX_BIN)]
    pub codex_bin: String,

    /// Override the Codex app-server port. Omit to use a random local free port.
    #[arg(long)]
    pub codex_port: Option<u16>,

    /// In Codex mode, require the agent to produce a plan before execution.
    #[arg(long, default_value_t = false)]
    pub plan_mode: bool,
}

// ---------------------------------------------------------------------------
// Top-level entry point — mode dispatch
// ---------------------------------------------------------------------------

/// Entry point for the `libra code` subcommand.
///
/// Validates CLI flag combinations, then dispatches to one of three mode-specific
/// execution paths: stdio (MCP over stdin/stdout), web-only (headless HTTP servers),
/// or TUI (full interactive terminal with background servers).
///
/// # Side Effects
/// - May start local web, MCP, and Codex app-server processes depending on mode.
/// - May create `.libra/objects` and connect to `.libra/libra.db` for history.
/// - In TUI mode, may mutate the workspace through registered tools, subject to
///   sandbox and approval policy.
/// - In stdio mode, owns stdin/stdout for the MCP session.
///
/// # Errors
/// Returns [`CliError`] for invalid mode combinations, provider credential
/// failures, network bind failures, Codex app-server startup failures, or
/// terminal/session initialization failures. Error classification follows
/// `docs/development/cli-error-contract-design.md`.
pub async fn execute(args: CodeArgs, output: &OutputConfig) -> CliResult<()> {
    validate_mode_args(&args, output).map_err(CliError::command_usage)?;
    if args.stdio {
        execute_stdio(&args).await
    } else if args.web_only {
        execute_web_only(&args).await
    } else {
        execute_tui(args).await
    }
}

// ---------------------------------------------------------------------------
// Server handles — RAII wrappers for graceful shutdown
// ---------------------------------------------------------------------------

/// Handle to a running MCP server.
///
/// In addition to the shared shutdown mechanism, this tracks individual
/// per-connection tasks so they can be aborted during shutdown — preventing
/// leaked tasks when the server is torn down.
struct McpServerHandle {
    addr: SocketAddr,
    shutdown_tx: oneshot::Sender<()>,
    join: tokio::task::JoinHandle<anyhow::Result<()>>,
    /// Tracks spawned per-connection Hyper service tasks for cleanup.
    connection_tasks: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
}

impl McpServerHandle {
    async fn shutdown(self) {
        let _ = self.shutdown_tx.send(());
        let _ = self.join.await;
        let pending = match self.connection_tasks.lock() {
            Ok(mut handles) => std::mem::take(&mut *handles),
            Err(_) => Vec::new(),
        };
        for handle in pending {
            handle.abort();
            let _ = handle.await;
        }
    }
}

// ---------------------------------------------------------------------------
// Mode: Web-only — headless web + MCP servers (no TUI)
// ---------------------------------------------------------------------------

/// Runs the web server and MCP server without a terminal UI.
///
/// Blocks on `Ctrl-C`, then performs graceful shutdown of both servers.
/// This mode is useful for remote/headless environments where the user
/// interacts through a browser or external MCP client.
///
/// # Side Effects
/// - Starts the embedded web server and Streamable HTTP MCP server.
/// - For the Codex provider, starts and later shuts down a managed Codex
///   app-server child process.
/// - Prints connection details to stdout and listens for `Ctrl-C`.
///
/// # Errors
/// Returns [`CliError`] when the working directory cannot be resolved, the web
/// or MCP listener cannot bind, the Codex app-server fails to start, or the
/// selected host would expose loopback-only browser control.
async fn execute_web_only(args: &CodeArgs) -> CliResult<()> {
    let working_dir = resolve_code_working_dir(args)?;
    let control_runtime = prepare_control_runtime(args, &working_dir).await?;
    let mcp_server = init_mcp_server(&working_dir).await;

    let mut managed_codex_server = None;
    let code_ui_runtime = if args.provider == CodeProvider::Codex {
        ensure_loopback_browser_control_host(&args.host)?;

        let server =
            start_managed_codex_server(&args.codex_bin, args.codex_port, &working_dir).await?;
        println!("Starting Libra Code Web UI with Codex provider");
        println!("Working directory: {}", working_dir.display());
        println!("Codex WebSocket: {}", server.ws_url);
        println!("Codex app-server: auto-started");
        managed_codex_server = Some(server);

        let ws_url = managed_codex_server
            .as_ref()
            .map(|server| server.ws_url.as_str())
            .unwrap_or_default();
        start_codex_code_ui_runtime(
            args,
            &working_dir,
            ws_url,
            mcp_server.clone(),
            true,
            CodeUiInitialController::Unclaimed,
        )
        .await?
    } else {
        build_placeholder_web_code_ui_runtime(args, &working_dir).await
    };

    let web_handle = match start_web_server(
        &args.host,
        args.port,
        working_dir.clone(),
        WebServerOptions {
            code_ui: Some(code_ui_runtime.clone()),
            automation_control_token: control_runtime.token.clone(),
            audit_sink: None,
        },
    )
    .await
    {
        Ok(handle) => handle,
        Err(err) => {
            let _ = code_ui_runtime.shutdown().await;
            if let Some(server) = managed_codex_server.as_mut() {
                server.shutdown().await;
            }
            return Err(
                CliError::network(format!("failed to start web server: {err}"))
                    .with_detail("component", "web_server"),
            );
        }
    };
    let base_url = format!("http://{}", web_handle.addr);
    let thread_id = code_ui_runtime.snapshot().await.thread_id;
    if let Err(error) =
        control_runtime.write_info_file(&working_dir, base_url.clone(), None, thread_id.clone())
    {
        let _ = code_ui_runtime.shutdown().await;
        if let Some(server) = managed_codex_server.as_mut() {
            server.shutdown().await;
        }
        web_handle.shutdown().await;
        return Err(error);
    }
    println!("Libra Code server running at {base_url}");

    // Start MCP Server
    let mcp_handle = match start_mcp_server(&args.host, args.mcp_port, mcp_server.clone()).await {
        Ok(handle) => {
            let mcp_url = format!("http://{}", handle.addr);
            if let Err(error) = control_runtime.write_info_file(
                &working_dir,
                base_url.clone(),
                Some(mcp_url.clone()),
                thread_id.clone(),
            ) {
                let _ = code_ui_runtime.shutdown().await;
                if let Some(server) = managed_codex_server.as_mut() {
                    server.shutdown().await;
                }
                web_handle.shutdown().await;
                handle.shutdown().await;
                return Err(error);
            }
            println!("MCP: {mcp_url}");
            handle
        }
        Err(err) => {
            let _ = code_ui_runtime.shutdown().await;
            if let Some(server) = managed_codex_server.as_mut() {
                server.shutdown().await;
            }
            web_handle.shutdown().await;
            return Err(
                CliError::network(format!("failed to start MCP server: {err}"))
                    .with_detail("component", "mcp_server"),
            );
        }
    };

    let _ = tokio::signal::ctrl_c().await;
    let _ = code_ui_runtime.shutdown().await;
    web_handle.shutdown().await;
    mcp_handle.shutdown().await;
    if let Some(server) = managed_codex_server.as_mut() {
        server.shutdown().await;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Mode: TUI — full interactive terminal with background servers
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct CodeEnvFile {
    values: BTreeMap<String, String>,
}

impl CodeEnvFile {
    fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }
}

fn load_code_env_file(path: Option<&Path>) -> CliResult<CodeEnvFile> {
    let Some(path) = path else {
        return Ok(CodeEnvFile::default());
    };

    let contents = fs::read_to_string(path).map_err(|error| {
        CliError::io(format!(
            "failed to read --env-file {}: {error}",
            path.display()
        ))
    })?;
    parse_code_env_file(&contents, path).map_err(CliError::command_usage)
}

fn parse_code_env_file(contents: &str, path: &Path) -> Result<CodeEnvFile, String> {
    let mut values = BTreeMap::new();
    for (index, raw_line) in contents.lines().enumerate() {
        let line_no = index + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!(
                "{}:{line_no}: expected KEY=VALUE entry",
                path.display()
            ));
        };
        let key = key.trim();
        if !is_valid_env_key(key) {
            return Err(format!(
                "{}:{line_no}: invalid environment variable name `{key}`",
                path.display()
            ));
        }

        let value = parse_env_file_value(value).map_err(|message| {
            format!(
                "{}:{line_no}: invalid value for `{key}`: {message}",
                path.display()
            )
        })?;
        values.insert(key.to_string(), value);
    }

    Ok(CodeEnvFile { values })
}

fn is_valid_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return false;
    }
    chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn parse_env_file_value(raw: &str) -> Result<String, String> {
    let value = raw.trim();
    if value.is_empty() {
        return Ok(String::new());
    }

    let first = value.as_bytes()[0];
    match first {
        b'\'' | b'"' => {
            if value.as_bytes().last() != Some(&first) || value.len() < 2 {
                return Err("quoted values must end with the matching quote".to_string());
            }
            let inner = &value[1..value.len() - 1];
            if first == b'"' {
                parse_double_quoted_env_value(inner)
            } else {
                Ok(inner.to_string())
            }
        }
        _ => Ok(strip_inline_env_comment(value).trim_end().to_string()),
    }
}

fn parse_double_quoted_env_value(value: &str) -> Result<String, String> {
    let mut parsed = String::new();
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            parsed.push(ch);
            continue;
        }

        let Some(escaped) = chars.next() else {
            return Err("trailing backslash in quoted value".to_string());
        };
        match escaped {
            'n' => parsed.push('\n'),
            'r' => parsed.push('\r'),
            't' => parsed.push('\t'),
            '\\' => parsed.push('\\'),
            '"' => parsed.push('"'),
            other => parsed.push(other),
        }
    }
    Ok(parsed)
}

fn strip_inline_env_comment(value: &str) -> &str {
    for (index, ch) in value.char_indices() {
        if ch == '#' && (index == 0 || value[..index].ends_with(char::is_whitespace)) {
            return &value[..index];
        }
    }
    value
}

fn provider_env_value(env_file: &CodeEnvFile, key: &str) -> Option<String> {
    provider_env_value_with_lookup(env_file, key, |key| std::env::var(key).ok())
}

fn provider_env_value_with_lookup(
    env_file: &CodeEnvFile,
    key: &str,
    lookup: impl FnOnce(&str) -> Option<String>,
) -> Option<String> {
    env_file
        .get(key)
        .map(str::to_string)
        .or_else(|| lookup(key))
}

/// Main TUI execution path: initializes the AI provider, builds the tool
/// registry, starts background web/MCP servers, and launches the interactive
/// terminal application.
///
/// This function handles provider-specific client creation (API key validation,
/// model selection) and delegates the actual TUI lifecycle to [`run_tui_with_model`].
///
/// # Side Effects
/// - Reads provider credentials from environment variables and optional dotenv
///   files.
/// - Registers local file, shell, planning, and MCP bridge tools for the agent.
/// - May start web/MCP background services and a managed Codex app-server.
/// - May mutate the workspace through tools when the selected context permits it.
///
/// # Errors
/// Returns [`CliError`] for missing credentials, invalid provider configuration,
/// unsafe mode/host combinations, provider bootstrap failures, or failures from
/// the shared TUI lifecycle.
async fn execute_tui(args: CodeArgs) -> CliResult<()> {
    let working_dir = resolve_code_working_dir(&args)?;
    let env_file = load_code_env_file(args.env_file.as_deref())?;
    let control_runtime = prepare_control_runtime(&args, &working_dir).await?;

    // Validate --api-base: only honored for Ollama via CLI flag. Other providers
    // accept custom base URLs through their respective environment variables.
    if args.api_base.is_some()
        && !matches!(args.provider, CodeProvider::Ollama | CodeProvider::Codex)
    {
        eprintln!(
            "warning: --api-base is only honored for the ollama provider; \
             use provider-specific env vars (e.g. OPENAI_BASE_URL) for others; ignoring"
        );
    } else if args.provider == CodeProvider::Ollama
        && let Some(ref base_url) = args.api_base
    {
        match Url::parse(base_url) {
            Ok(u) if u.scheme() == "http" || u.scheme() == "https" => {}
            Ok(u) => {
                return Err(CliError::command_usage(format!(
                    "--api-base must use http or https (got {})",
                    u.scheme()
                )));
            }
            Err(e) => {
                return Err(CliError::command_usage(format!(
                    "--api-base is not a valid URL: {e}"
                )));
            }
        }
    }

    let preamble = system_preamble(&working_dir, args.context);
    let temperature = args.temperature;
    let thinking = completion_thinking_for_args(&args);
    let reasoning_effort = completion_reasoning_effort_for_args(&args);
    let stream = completion_stream_for_args(&args);
    let preserve_reasoning_content = preserve_reasoning_content_for_provider(args.provider);
    let resume_thread_id = args.resume.clone();
    let host = args.host.clone();
    let trace_id = resume_thread_id
        .as_deref()
        .and_then(|thread_id| Uuid::parse_str(thread_id).ok())
        .unwrap_or_else(Uuid::new_v4);

    // Prepare MCP server instance shared between the HTTP transport and TUI bridge.
    // INVARIANT: the same server instance backs both transports so an agent sees
    // one coherent history/object store regardless of whether a tool is invoked
    // through HTTP MCP or the in-process TUI bridge.
    let mcp_server = init_mcp_server(&working_dir).await;

    // Create the bridge channel for request_user_input tool <-> TUI communication.
    let (user_input_tx, user_input_rx) = tokio::sync::mpsc::unbounded_channel::<UserInputRequest>();
    let (exec_approval_tx, exec_approval_rx) =
        tokio::sync::mpsc::unbounded_channel::<ExecApprovalRequest>();

    // Build registry: basic file tools + MCP workflow tools.
    //
    // AI user story: let a coding agent inspect files, search context, make
    // bounded edits, run verification commands, ask the human for missing
    // choices, and record structured planning artifacts without leaving the
    // sandbox/approval model.
    let mut builder = ToolRegistryBuilder::with_working_dir(working_dir.clone())
        .hardening(ToolBoundaryRuntime::system(
            trace_id,
            Arc::new(TracingAuditSink),
        ))
        .register("read_file", Arc::new(ReadFileHandler))
        .register("list_dir", Arc::new(ListDirHandler))
        .register("grep_files", Arc::new(GrepFilesHandler))
        .register("search_files", Arc::new(SearchFilesHandler))
        .register("web_search", Arc::new(WebSearchHandler))
        .register("apply_patch", Arc::new(ApplyPatchHandler))
        .register("shell", Arc::new(ShellHandler))
        .register("update_plan", Arc::new(PlanHandler))
        .register("submit_intent_draft", Arc::new(SubmitIntentDraftHandler))
        .register("submit_plan_draft", Arc::new(SubmitPlanDraftHandler))
        .register("submit_task_complete", Arc::new(SubmitTaskCompleteHandler))
        .register(
            "request_user_input",
            Arc::new(RequestUserInputHandler::new(user_input_tx.clone())),
        );

    // AI user story: MCP bridge tools let the agent persist intent/task/run,
    // evidence, provenance, and Libra VCS operations in the same workflow graph
    // that external MCP clients use. Keep these names aligned with
    // `docs/agent/intentspec_typical.yaml` and `docs/agent/agent-workflow.md`.
    for (name, handler) in McpBridgeHandler::all_handlers(mcp_server.clone()) {
        builder = builder.register(name, handler);
    }

    let registry = Arc::new(builder.build());

    let provider_name = format!("{:?}", args.provider).to_lowercase();
    let launch_config = TuiLaunchConfig {
        host,
        port: args.port,
        mcp_port: args.mcp_port,
        registry,
        preamble,
        temperature,
        thinking,
        reasoning_effort,
        stream,
        preserve_reasoning_content,
        context: args.context,
        resume_thread_id,
        approval_policy: args.approval_policy.into(),
        allow_all_commands: args.approval_policy.allows_all_commands(),
        network_access: args.network_access.is_allowed(),
        user_input_rx,
        exec_approval_rx,
        exec_approval_tx,
        mcp_server,
        control_runtime,
    };

    // Create agent based on provider
    match args.provider {
        CodeProvider::Gemini => {
            let client = match GeminiClient::from_env() {
                Ok(client) => client,
                Err(_) => return Err(CliError::auth("GEMINI_API_KEY is not set")),
            };
            let model_name = args.model.unwrap_or_else(|| GEMINI_2_5_FLASH.to_string());
            let model = client.completion_model(&model_name);
            run_tui_with_model(model, launch_config, model_name, provider_name).await?;
        }
        CodeProvider::Openai => {
            let client = match OpenAIClient::from_env() {
                Ok(client) => client,
                Err(_) => return Err(CliError::auth("OPENAI_API_KEY is not set")),
            };
            let model_name = args.model.unwrap_or_else(|| GPT_4O_MINI.to_string());
            let model = client.completion_model(&model_name);
            run_tui_with_model(model, launch_config, model_name, provider_name).await?;
        }
        CodeProvider::Anthropic => {
            let client = match AnthropicClient::from_env() {
                Ok(client) => client,
                Err(_) => return Err(CliError::auth("ANTHROPIC_API_KEY is not set")),
            };
            let model_name = args.model.unwrap_or_else(|| CLAUDE_3_5_SONNET.to_string());
            let model = client.completion_model(&model_name);
            run_tui_with_model(model, launch_config, model_name, provider_name).await?;
        }
        CodeProvider::Deepseek => {
            let api_key = provider_env_value(&env_file, "DEEPSEEK_API_KEY")
                .ok_or_else(|| CliError::auth("DEEPSEEK_API_KEY is not set"))?;
            let client = DeepSeekClient::with_api_key(api_key);
            let model_name = args.model.unwrap_or_else(|| "deepseek-chat".to_string());
            let model = client.completion_model(&model_name);
            run_tui_with_model(model, launch_config, model_name, provider_name).await?;
        }
        CodeProvider::Kimi => {
            let api_key = provider_env_value(&env_file, "MOONSHOT_API_KEY")
                .ok_or_else(|| CliError::auth("MOONSHOT_API_KEY is not set"))?;
            let client = match provider_env_value(&env_file, "MOONSHOT_BASE_URL") {
                Some(base_url) => KimiClient::with_base_url(&base_url, api_key),
                None => KimiClient::with_api_key(api_key),
            };
            let model_name = args.model.unwrap_or_else(|| KIMI_K2_6.to_string());
            let model = client.completion_model(&model_name);
            run_tui_with_model(model, launch_config, model_name, provider_name).await?;
        }
        CodeProvider::Zhipu => {
            let client = match ZhipuClient::from_env() {
                Ok(client) => client,
                Err(_) => return Err(CliError::auth("ZHIPU_API_KEY is not set")),
            };
            let model_name = args.model.unwrap_or_else(|| GLM_5.to_string());
            let model = client.completion_model(&model_name);
            run_tui_with_model(model, launch_config, model_name, provider_name).await?;
        }
        CodeProvider::Ollama => {
            let mut client = if let Some(base_url) = &args.api_base {
                OllamaClient::with_base_url(base_url)
            } else {
                OllamaClient::from_env()
            };
            if args.ollama_compact_tools {
                client = client.with_compact_tool_schema(true);
            }
            if client.missing_required_cloud_api_key() {
                return Err(CliError::auth(
                    "OLLAMA_API_KEY is required when using Ollama Cloud directly (set --api-base https://ollama.com or OLLAMA_BASE_URL=https://ollama.com)",
                ));
            }
            let model_name = match args.model {
                Some(m) => m,
                None => {
                    return Err(CliError::command_usage(
                        "--model is required when using --provider ollama (e.g. --model llama3.2)",
                    ));
                }
            };
            let model = client.completion_model(&model_name);
            run_tui_with_model(model, launch_config, model_name, provider_name).await?;
        }
        #[cfg(feature = "test-provider")]
        CodeProvider::Fake => {
            let fixture = args.fake_fixture.as_deref().ok_or_else(|| {
                CliError::command_usage("--fake-fixture is required with --provider=fake")
            })?;
            let client = FakeClient::from_fixture_path(fixture).map_err(|error| {
                CliError::io(format!(
                    "failed to load fake provider fixture '{}': {error}",
                    fixture.display()
                ))
            })?;
            let model_name = args.model.unwrap_or_else(|| FAKE_DEFAULT_MODEL.to_string());
            let model = client.completion_model(&model_name);
            run_tui_with_model(model, launch_config, model_name, provider_name).await?;
        }
        CodeProvider::Codex => {
            let mut server =
                start_managed_codex_server(&args.codex_bin, args.codex_port, &working_dir).await?;
            let initial_controller = if launch_config.control_runtime.is_write() {
                CodeUiInitialController::LocalTui {
                    owner_label: "Terminal UI".to_string(),
                    reason: Some("The terminal UI controls this live Codex run".to_string()),
                }
            } else {
                CodeUiInitialController::Fixed {
                    kind: CodeUiControllerKind::Tui,
                    owner_label: "Terminal UI".to_string(),
                    reason: Some("The terminal UI controls this live Codex run".to_string()),
                }
            };
            let code_ui_runtime = match start_codex_code_ui_runtime(
                &args,
                &working_dir,
                &server.ws_url,
                launch_config.mcp_server.clone(),
                false,
                initial_controller,
            )
            .await
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    server.shutdown().await;
                    return Err(error);
                }
            };
            let model_name = args.model.clone().unwrap_or_else(|| "codex".to_string());
            let result = run_tui_with_managed_code_runtime(
                code_ui_runtime,
                launch_config,
                model_name,
                provider_name,
            )
            .await;
            server.shutdown().await;
            result?;
        }
    }

    Ok(())
}

fn completion_thinking_for_args(args: &CodeArgs) -> Option<CompletionThinking> {
    match args.provider {
        CodeProvider::Ollama => args.ollama_thinking.map(CompletionThinking::from),
        CodeProvider::Deepseek => args.deepseek_thinking.map(CompletionThinking::from),
        CodeProvider::Kimi => args.kimi_thinking.map(CompletionThinking::from),
        _ => None,
    }
}

fn completion_reasoning_effort_for_args(args: &CodeArgs) -> Option<CompletionReasoningEffort> {
    match args.provider {
        CodeProvider::Deepseek => args
            .deepseek_reasoning_effort
            .map(CompletionReasoningEffort::from),
        _ => None,
    }
}

fn completion_stream_for_args(args: &CodeArgs) -> Option<bool> {
    match args.provider {
        CodeProvider::Deepseek => args.deepseek_stream,
        CodeProvider::Kimi => Some(args.kimi_stream.unwrap_or(true)),
        _ => None,
    }
}

fn preserve_reasoning_content_for_provider(provider: CodeProvider) -> bool {
    matches!(provider, CodeProvider::Deepseek | CodeProvider::Kimi)
}

// ---------------------------------------------------------------------------
// Codex provider — managed app-server lifecycle
// ---------------------------------------------------------------------------

/// Represents a managed Codex app-server child process and its WebSocket URL.
///
/// The server is spawned as a child process and communicated with over WebSocket.
/// [`ManagedCodexServer::shutdown`] sends SIGKILL and waits up to 5 seconds.
struct ManagedCodexServer {
    ws_url: String,
    child: Child,
}

impl ManagedCodexServer {
    /// Gracefully shuts down the managed Codex app-server process.
    ///
    /// If the child process has already exited (`id()` returns `None`), this is
    /// a no-op. Otherwise it sends a kill signal via `start_kill()` and waits up
    /// to 5 seconds for the process to terminate. If the timeout expires the
    /// process is abandoned (the OS will reap it when the handle is dropped).
    async fn shutdown(&mut self) {
        if self.child.id().is_none() {
            return;
        }
        let _ = self.child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(5), self.child.wait()).await;
    }
}

struct ControlRuntimeConfig {
    mode: ControlMode,
    paths: ControlPaths,
    token: Option<Arc<str>>,
    _lock_guard: Option<ControlLockGuard>,
    write_info: bool,
    cleanup_token: bool,
    info_written: AtomicBool,
    started_at: chrono::DateTime<Utc>,
}

impl ControlRuntimeConfig {
    fn is_write(&self) -> bool {
        self.mode == ControlMode::Write
    }

    fn mode_name(&self) -> &'static str {
        match self.mode {
            ControlMode::Observe => "observe",
            ControlMode::Write => "write",
        }
    }

    fn cleanup(&self) {
        cleanup_control_files(
            &self.paths,
            self.cleanup_token,
            self.info_written.load(Ordering::Relaxed),
        );
    }

    fn write_info_file(
        &self,
        working_dir: &Path,
        base_url: String,
        mcp_url: Option<String>,
        thread_id: Option<String>,
    ) -> CliResult<()> {
        if !self.write_info {
            return Ok(());
        }

        let info = ControlInfo {
            version: 1,
            mode: self.mode_name().to_string(),
            pid: std::process::id(),
            base_url,
            mcp_url,
            working_dir: working_dir.to_path_buf(),
            thread_id,
            started_at: self.started_at,
        };
        write_control_info(&self.paths.info, &info).map_err(|error| {
            CliError::fatal(format!(
                "failed to write local TUI control info '{}': {error}",
                self.paths.info.display()
            ))
            .with_stable_code(StableErrorCode::IoWriteFailed)
        })?;
        self.info_written.store(true, Ordering::Relaxed);
        Ok(())
    }
}

impl Drop for ControlRuntimeConfig {
    fn drop(&mut self) {
        self.cleanup();
    }
}

async fn prepare_control_runtime(
    args: &CodeArgs,
    working_dir: &Path,
) -> CliResult<ControlRuntimeConfig> {
    let paths = resolve_control_paths(
        working_dir,
        args.control_token_file.as_deref(),
        args.control_info_file.as_deref(),
    );
    let started_at = Utc::now();

    match args.control {
        ControlMode::Observe => Ok(ControlRuntimeConfig {
            mode: ControlMode::Observe,
            paths,
            token: None,
            _lock_guard: None,
            write_info: args.control_info_file.is_some(),
            cleanup_token: false,
            info_written: AtomicBool::new(false),
            started_at,
        }),
        ControlMode::Write => {
            let lock_guard = acquire_control_lock(&paths.lock).map_err(|error| match error {
                ControlLockError::AlreadyHeld { .. } => CliError::conflict(error.to_string()),
                ControlLockError::Io(error) => CliError::io(format!(
                    "failed to acquire local TUI control lock '{}': {error}",
                    paths.lock.display()
                )),
            })?;
            let token = ensure_control_token_file(&paths.token)
                .await
                .map_err(|error| {
                    CliError::fatal(format!(
                        "failed to prepare local TUI control token '{}': {error}",
                        paths.token.display()
                    ))
                    .with_stable_code(StableErrorCode::IoWriteFailed)
                })?;

            Ok(ControlRuntimeConfig {
                mode: ControlMode::Write,
                paths,
                token: Some(Arc::<str>::from(token)),
                _lock_guard: Some(lock_guard),
                write_info: true,
                cleanup_token: true,
                info_written: AtomicBool::new(false),
                started_at,
            })
        }
    }
}

fn ensure_loopback_browser_control_host(host: &str) -> CliResult<()> {
    let normalized = host.trim().trim_matches('[').trim_matches(']');
    let is_loopback = matches!(normalized, "localhost" | "127.0.0.1" | "::1")
        || normalized
            .parse::<std::net::IpAddr>()
            .map(|addr| addr.is_loopback())
            .unwrap_or(false);

    if is_loopback {
        return Ok(());
    }

    Err(CliError::command_usage(
        "interactive web control is restricted to loopback hosts in v1; use --host 127.0.0.1",
    ))
}

async fn build_placeholder_web_code_ui_runtime(
    args: &CodeArgs,
    working_dir: &Path,
) -> Arc<CodeUiRuntimeHandle> {
    let capabilities = CodeUiCapabilities {
        message_input: false,
        streaming_text: false,
        plan_updates: false,
        tool_calls: false,
        patchsets: false,
        interactive_approvals: false,
        structured_questions: false,
        provider_session_resume: false,
    };

    let mut snapshot = initial_snapshot(
        working_dir.to_string_lossy().to_string(),
        CodeUiProviderInfo {
            provider: format!("{:?}", args.provider).to_lowercase(),
            model: args.model.clone(),
            mode: Some("web".to_string()),
            managed: matches!(args.provider, CodeProvider::Codex),
        },
        capabilities.clone(),
    );
    let now = Utc::now();
    snapshot.status = CodeUiSessionStatus::Idle;
    snapshot.transcript.push(CodeUiTranscriptEntry {
        id: "web-ui-placeholder".to_string(),
        kind: CodeUiTranscriptEntryKind::InfoNote,
        title: Some("Web Control Unavailable".to_string()),
        content: Some(
            "Interactive browser control is fully implemented for `--provider codex`. For other providers, launch `libra code` without `--web-only` to observe the live terminal session in the browser."
                .to_string(),
        ),
        status: Some("completed".to_string()),
        streaming: false,
        metadata: serde_json::json!({ "providerAgnostic": true }),
        created_at: now,
        updated_at: now,
    });

    CodeUiRuntimeHandle::build(
        ReadOnlyCodeUiAdapter::new(CodeUiSession::new(snapshot), capabilities),
        false,
        CodeUiInitialController::Unclaimed,
    )
    .await
}

async fn start_codex_code_ui_runtime(
    args: &CodeArgs,
    working_dir: &Path,
    ws_url: &str,
    mcp_server: Arc<LibraMcpServer>,
    browser_write_enabled: bool,
    initial_controller: CodeUiInitialController,
) -> CliResult<Arc<CodeUiRuntimeHandle>> {
    let ui_mode = match &initial_controller {
        CodeUiInitialController::Fixed {
            kind: CodeUiControllerKind::Tui,
            ..
        } => Some("tui".to_string()),
        CodeUiInitialController::Fixed {
            kind: CodeUiControllerKind::Cli,
            ..
        } => Some("cli".to_string()),
        CodeUiInitialController::LocalTui { .. } => Some("managed-tui".to_string()),
        _ => Some("web".to_string()),
    };
    let agent_args = agent_codex::AgentCodexArgs {
        url: ws_url.to_string(),
        cwd: working_dir.to_string_lossy().to_string(),
        approval: approval_policy_to_codex(args.approval_policy).to_string(),
        model_provider: None,
        service_tier: None,
        personality: None,
        model: args.model.clone(),
        plan_mode: args.plan_mode,
        debug: false,
        ui_mode,
    };

    agent_codex::start_code_ui_runtime(
        agent_args,
        mcp_server,
        browser_write_enabled,
        initial_controller,
    )
    .await
    .map_err(|error| CliError::fatal(error.to_string()))
}

// ---------------------------------------------------------------------------
// Approval policy mapping helpers
// ---------------------------------------------------------------------------

/// Maps [`CodeApprovalPolicy`] to the Codex app-server's approval string.
/// Codex only distinguishes between "accept" (auto-approve) and "ask" (prompt).
fn approval_policy_to_codex(policy: CodeApprovalPolicy) -> &'static str {
    match policy {
        CodeApprovalPolicy::Never | CodeApprovalPolicy::AllowAll => "accept",
        CodeApprovalPolicy::OnFailure
        | CodeApprovalPolicy::OnRequest
        | CodeApprovalPolicy::Untrusted => "ask",
    }
}

/// Starts the Codex app-server as a managed child process.
///
/// 1. Resolves the WebSocket URL (using the requested port or auto-selecting a free one).
/// 2. Spawns the Codex binary with `app-server --listen <ws_url>`.
/// 3. Polls the WebSocket endpoint until it becomes reachable (or times out).
///
/// On failure, the child process is killed before returning the error.
async fn start_managed_codex_server(
    codex_bin: &str,
    requested_port: Option<u16>,
    working_dir: &Path,
) -> CliResult<ManagedCodexServer> {
    let ws_url = resolve_codex_ws_url(requested_port)?;
    let mut child = spawn_codex_app_server(codex_bin, &ws_url, working_dir)?;

    if let Err(err) = wait_for_codex_ready(&ws_url).await {
        let _ = child.start_kill();
        let _ = child.wait().await;
        return Err(err);
    }

    Ok(ManagedCodexServer { ws_url, child })
}

/// Builds a `tokio::process::Command` for the Codex app-server.
/// Stdin/stdout/stderr are all set to null since the server communicates
/// exclusively over WebSocket.
fn build_codex_command(program: &str, ws_url: &str, working_dir: &Path) -> Command {
    let mut command = Command::new(program);
    command
        .arg("app-server")
        .arg("--listen")
        .arg(ws_url)
        .current_dir(working_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

/// Windows fallback: wraps the Codex binary invocation in `cmd /C` to
/// handle `.cmd`/`.bat` shims that are common on Windows (e.g. from npm).
#[cfg(target_os = "windows")]
fn build_windows_shell_codex_command(codex_bin: &str, ws_url: &str, working_dir: &Path) -> Command {
    let mut command = Command::new("cmd");
    command
        .arg("/C")
        .arg(codex_bin)
        .arg("app-server")
        .arg("--listen")
        .arg(ws_url)
        .current_dir(working_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

/// Attempts to spawn the Codex app-server process. On Windows, falls back
/// to `cmd /C` if the direct spawn fails with `NotFound` (handles `.cmd` shims).
fn spawn_codex_app_server(codex_bin: &str, ws_url: &str, working_dir: &Path) -> CliResult<Child> {
    match build_codex_command(codex_bin, ws_url, working_dir).spawn() {
        Ok(child) => Ok(child),
        #[cfg(target_os = "windows")]
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            build_windows_shell_codex_command(codex_bin, ws_url, working_dir)
                .spawn()
                .map_err(|shell_err| {
                    CliError::io(format!(
                        "failed to start Codex app-server using '{}': {}. Direct spawn error: {}. Make sure the Codex CLI is installed and available in PATH.",
                        codex_bin, shell_err, err
                    ))
                })
        }
        Err(err) => Err(CliError::io(format!(
            "failed to start Codex app-server using '{}': {}. Make sure the Codex CLI is installed and available in PATH.",
            codex_bin, err
        ))),
    }
}

/// Resolves the WebSocket URL for the Codex app-server.
/// If no port is specified, auto-selects a free local port via [`pick_free_local_port`].
fn resolve_codex_ws_url(requested_port: Option<u16>) -> CliResult<String> {
    let port = match requested_port {
        Some(0) => {
            return Err(CliError::command_usage(
                "--codex-port must be a non-zero TCP port; omit it to auto-select a free port",
            ));
        }
        Some(port) => port,
        None => pick_free_local_port(DEFAULT_BIND_HOST)?,
    };
    Ok(format!("ws://{DEFAULT_BIND_HOST}:{port}"))
}

/// Binds to port 0 on the given host to let the OS assign a free ephemeral
/// port, then returns that port number. The listener is dropped immediately,
/// releasing the port for the Codex server to bind to.
fn pick_free_local_port(host: &str) -> CliResult<u16> {
    let listener = std::net::TcpListener::bind((host, 0)).map_err(|e| {
        CliError::network(format!(
            "failed to reserve a local port for the Codex app-server on {}: {}",
            host, e
        ))
    })?;
    listener.local_addr().map(|addr| addr.port()).map_err(|e| {
        CliError::network(format!(
            "failed to determine the reserved Codex app-server port: {}",
            e
        ))
    })
}

/// Polls the Codex app-server WebSocket endpoint until a connection succeeds
/// or [`CODEX_STARTUP_TIMEOUT`] is exceeded. The probe connection is immediately
/// dropped after a successful handshake.
async fn wait_for_codex_ready(ws_url: &str) -> CliResult<()> {
    let deadline = Instant::now() + CODEX_STARTUP_TIMEOUT;

    loop {
        match connect_async(ws_url).await {
            Ok((stream, _)) => {
                drop(stream);
                return Ok(());
            }
            Err(err) => {
                let detail = err.to_string();
                if Instant::now() >= deadline {
                    return Err(CliError::network(format!(
                        "timed out waiting for Codex app-server at {}: {}",
                        ws_url, detail
                    )));
                }
                sleep(CODEX_STARTUP_POLL_INTERVAL).await;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Working directory resolution
// ---------------------------------------------------------------------------

/// Resolves the effective working directory for the code session.
///
/// Priority: `--cwd` > `--repo` > current working directory.
/// Validates that the resolved path exists and is a directory.
/// `--cwd` and `--repo` are mutually exclusive.
pub(crate) fn resolve_code_preflight_working_dir(args: &CodeArgs) -> CliResult<PathBuf> {
    resolve_code_working_dir(args)
}

fn resolve_code_working_dir(args: &CodeArgs) -> CliResult<PathBuf> {
    if args.cwd.is_some() && args.repo.is_some() {
        return Err(CliError::command_usage(
            "--cwd and --repo cannot be used together".to_string(),
        ));
    }

    let working_dir = args
        .cwd
        .clone()
        .or_else(|| args.repo.clone())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let flag = if args.repo.is_some() {
        "--repo"
    } else {
        "--cwd"
    };
    validate_code_working_dir(working_dir, flag)
}

fn validate_code_working_dir(working_dir: PathBuf, flag: &str) -> CliResult<PathBuf> {
    if !working_dir.exists() {
        return Err(CliError::command_usage(format!(
            "{flag} path does not exist: {}",
            working_dir.display()
        )));
    }
    if !working_dir.is_dir() {
        return Err(CliError::command_usage(format!(
            "{flag} must point to a directory: {}",
            working_dir.display()
        )));
    }
    Ok(working_dir)
}

// ---------------------------------------------------------------------------
// TUI launch configuration and model abstraction
// ---------------------------------------------------------------------------

/// Aggregates all parameters needed to launch the TUI application.
///
/// This struct is built once in [`execute_tui`] and consumed by
/// [`run_tui_with_model`]. It bundles network config, tool registry,
/// prompt/temperature settings, session state, and inter-component channels.
struct TuiLaunchConfig {
    host: String,
    port: u16,
    mcp_port: u16,
    registry: Arc<ToolRegistry>,
    preamble: String,
    temperature: Option<f64>,
    thinking: Option<CompletionThinking>,
    reasoning_effort: Option<CompletionReasoningEffort>,
    stream: Option<bool>,
    preserve_reasoning_content: bool,
    context: Option<CodeContext>,
    resume_thread_id: Option<String>,
    approval_policy: AskForApproval,
    allow_all_commands: bool,
    network_access: bool,
    user_input_rx: tokio::sync::mpsc::UnboundedReceiver<UserInputRequest>,
    exec_approval_rx: tokio::sync::mpsc::UnboundedReceiver<ExecApprovalRequest>,
    exec_approval_tx: tokio::sync::mpsc::UnboundedSender<ExecApprovalRequest>,
    mcp_server: Arc<LibraMcpServer>,
    control_runtime: ControlRuntimeConfig,
}

#[derive(Clone)]
struct ManagedCodeRuntimeModel;

impl CompletionModel for ManagedCodeRuntimeModel {
    type Response = ();

    async fn completion(
        &self,
        _request: CompletionRequest,
    ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
        Err(CompletionError::NotImplemented(
            "managed code runtime handles turns outside the generic completion model".to_string(),
        ))
    }
}

fn build_tui_code_ui_capabilities() -> CodeUiCapabilities {
    CodeUiCapabilities {
        message_input: true,
        streaming_text: true,
        plan_updates: true,
        tool_calls: true,
        patchsets: true,
        interactive_approvals: true,
        structured_questions: true,
        provider_session_resume: false,
    }
}

fn build_tui_code_ui_transcript(session: &SessionState) -> Vec<CodeUiTranscriptEntry> {
    session
        .messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| {
            let kind = match message.role.as_str() {
                "user" => CodeUiTranscriptEntryKind::UserMessage,
                "assistant" => CodeUiTranscriptEntryKind::AssistantMessage,
                _ => return None,
            };
            Some(CodeUiTranscriptEntry {
                id: format!("session-message-{}", index + 1),
                kind,
                title: Some(match message.role.as_str() {
                    "user" => "Developer".to_string(),
                    _ => "Assistant".to_string(),
                }),
                content: Some(message.content.clone()),
                status: Some("completed".to_string()),
                streaming: false,
                metadata: serde_json::json!({ "restored": true }),
                created_at: message.timestamp,
                updated_at: message.timestamp,
            })
        })
        .collect()
}

fn session_canonical_thread_id(session: &SessionState) -> Option<String> {
    ["thread_id", "threadId", "canonical_thread_id"]
        .iter()
        .find_map(|key| session.metadata.get(*key).and_then(|value| value.as_str()))
        .map(str::to_string)
        .or_else(|| {
            Uuid::parse_str(&session.id)
                .ok()
                .map(|thread_id| thread_id.to_string())
        })
}

async fn build_tui_code_ui_runtime(
    working_dir: &str,
    session: &SessionState,
    provider_name: &str,
    model_name: &str,
    projection_bundle: Option<&ThreadBundle>,
    code_control_tx: Option<tokio::sync::mpsc::UnboundedSender<TuiControlCommand>>,
    automation_write_enabled: bool,
) -> Arc<CodeUiRuntimeHandle> {
    let capabilities = build_tui_code_ui_capabilities();
    let provider = CodeUiProviderInfo {
        provider: provider_name.to_string(),
        model: Some(model_name.to_string()),
        mode: Some("tui".to_string()),
        managed: false,
    };
    let mut snapshot = if let Some(bundle) = projection_bundle {
        snapshot_from_thread_bundle(
            working_dir.to_string(),
            provider,
            capabilities.clone(),
            bundle,
        )
    } else {
        initial_snapshot(working_dir.to_string(), provider, capabilities.clone())
    };
    if projection_bundle.is_none() {
        snapshot.session_id = session.id.clone();
        snapshot.thread_id = session_canonical_thread_id(session);
    }
    snapshot.transcript = build_tui_code_ui_transcript(session);
    snapshot.updated_at = Utc::now();

    let code_ui_session = CodeUiSession::new(snapshot);
    let adapter: Arc<dyn CodeUiProviderAdapter> = if let Some(control_tx) = code_control_tx {
        TuiCodeUiAdapter::new(code_ui_session, capabilities, control_tx)
    } else {
        ReadOnlyCodeUiAdapter::new(code_ui_session, capabilities)
    };
    let initial_controller = if automation_write_enabled {
        CodeUiInitialController::LocalTui {
            owner_label: "Terminal UI".to_string(),
            reason: Some("The terminal UI controls this live session".to_string()),
        }
    } else {
        CodeUiInitialController::Fixed {
            kind: CodeUiControllerKind::Tui,
            owner_label: "Terminal UI".to_string(),
            reason: Some("The terminal UI controls this live session".to_string()),
        }
    };
    CodeUiRuntimeHandle::build_with_control(
        adapter,
        false,
        automation_write_enabled,
        initial_controller,
    )
    .await
}

async fn load_code_ui_projection_bundle(
    working_dir: &Path,
    thread_id: Uuid,
) -> anyhow::Result<Option<ThreadBundle>> {
    let storage_root = resolve_storage_root(working_dir);
    let db_path = storage_root.join("libra.db");
    let db_path = db_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("database path is not valid UTF-8"))?;
    let db_conn = establish_connection(db_path).await?;
    let storage = Arc::new(LocalStorage::new(storage_root.join("objects")));
    let history = HistoryManager::new(storage.clone(), storage_root, Arc::new(db_conn.clone()));
    let rebuilder = ProjectionRebuilder::new(storage.as_ref(), &history);
    let resolver = ProjectionResolver::new(db_conn);
    resolver
        .load_or_rebuild_thread_bundle(thread_id, &rebuilder)
        .await
}

/// Core TUI lifecycle: wires up the terminal, background servers, agent
/// configuration, session persistence, and the interactive `App` event loop.
///
/// This function is generic over the completion model `M`, allowing all
/// providers to share the same TUI setup code. The flow is:
///
/// 1. Load git hooks from the working directory.
/// 2. Build the agent's `ToolLoopConfig` (preamble, temperature, sandbox policy).
/// 3. Initialize the terminal via `tui_init()` with a restore guard.
/// 4. Start the web server and MCP server as background tasks.
/// 5. Load slash commands and agent profiles from disk.
/// 6. Restore or create a new session.
/// 7. Run the `App` event loop until the user exits.
/// 8. Gracefully shut down all background servers.
///
/// # Side Effects
/// - Switches the terminal into TUI mode and restores it on exit.
/// - Starts background web and MCP listeners when their ports are available.
/// - Reads hook, slash-command, profile, session, and projection state from the
///   working directory.
/// - Persists session updates and may drive tool-mediated workspace writes.
///
/// # Errors
/// Returns [`CliError`] for terminal initialization failures, invalid resume
/// thread IDs, missing sessions, session/projection load failures, or fatal app
/// exits reported by the TUI event loop.
async fn run_tui_with_model<M>(
    model: M,
    params: TuiLaunchConfig,
    model_name: String,
    provider_name: String,
) -> CliResult<()>
where
    M: CompletionModel + Clone + 'static,
    M::Response: CompletionUsage,
{
    run_tui_with_model_inner(model, params, model_name, provider_name, None).await
}

async fn run_tui_with_managed_code_runtime(
    code_ui_runtime: Arc<CodeUiRuntimeHandle>,
    params: TuiLaunchConfig,
    model_name: String,
    provider_name: String,
) -> CliResult<()> {
    run_tui_with_model_inner(
        ManagedCodeRuntimeModel,
        params,
        model_name,
        provider_name,
        Some(code_ui_runtime),
    )
    .await
}

async fn run_tui_with_model_inner<M>(
    model: M,
    params: TuiLaunchConfig,
    model_name: String,
    provider_name: String,
    managed_code_ui_runtime: Option<Arc<CodeUiRuntimeHandle>>,
) -> CliResult<()>
where
    M: CompletionModel + Clone + 'static,
    M::Response: CompletionUsage,
{
    let registry = params.registry;
    let control_runtime = params.control_runtime;
    let hook_runner = {
        let runner = HookRunner::load(registry.working_dir());
        if runner.has_hooks() {
            Some(std::sync::Arc::new(runner))
        } else {
            None
        }
    };

    let config = ToolLoopConfig {
        preamble: Some(params.preamble),
        temperature: params.temperature,
        thinking: params.thinking,
        reasoning_effort: params.reasoning_effort,
        stream: params.stream,
        hook_runner,
        allowed_tools: None,
        runtime_context: Some(default_tui_runtime_context(
            registry.working_dir(),
            params.context,
            params.approval_policy,
            params.allow_all_commands,
            params.network_access,
            params.exec_approval_tx.clone(),
        )),
        max_turns: None,
        preserve_reasoning_content: params.preserve_reasoning_content,
        ..Default::default()
    };

    // Initialize terminal.
    let terminal = match tui_init() {
        Ok(t) => t,
        Err(e) => return Err(CliError::io(format!("failed to initialize terminal: {e}"))),
    };

    // INVARIANT: every successful `tui_init` must install this guard before any
    // await point that can fail, otherwise a later error could leave the user's
    // terminal in raw/alternate-screen mode.
    let _guard = scopeguard::guard((), |_| {
        let _ = tui_restore();
    });

    let tui = Tui::new(terminal);

    // Set up session persistence
    let working_dir_str = registry.working_dir().to_string_lossy().to_string();
    let storage_root = resolve_storage_root(registry.working_dir());
    let session_store = SessionStore::from_storage_path(&storage_root);
    let session = if let Some(thread_id) = params.resume_thread_id.as_deref() {
        Uuid::parse_str(thread_id).map_err(|error| {
            CliError::command_usage(format!(
                "--resume expects a canonical thread_id UUID (got '{thread_id}': {error})"
            ))
        })?;
        match session_store.load_for_thread_id(thread_id, &working_dir_str) {
            Ok(Some(session)) => session,
            Ok(None) => {
                return Err(CliError::fatal(format!(
                    "no Libra Code session found for thread_id '{thread_id}' in working directory '{working_dir_str}'"
                )));
            }
            Err(error) => {
                return Err(CliError::io(format!(
                    "failed to load Libra Code session for thread_id '{thread_id}': {error}"
                )));
            }
        }
    } else {
        SessionState::new(&working_dir_str)
    };

    let (code_control_tx, code_control_rx) = if control_runtime.is_write() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<TuiControlCommand>();
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };
    let automation_write_enabled = code_control_tx.is_some();

    let code_ui_runtime = if let Some(runtime) = managed_code_ui_runtime.clone() {
        if let Some(control_tx) = code_control_tx {
            let adapter = runtime.adapter();
            let code_ui_session = adapter.session();
            let capabilities = adapter.capabilities();
            let tui_adapter: Arc<dyn CodeUiProviderAdapter> =
                TuiCodeUiAdapter::new(code_ui_session, capabilities, control_tx);
            CodeUiRuntimeHandle::build_with_control(
                tui_adapter,
                false,
                true,
                CodeUiInitialController::LocalTui {
                    owner_label: "Terminal UI".to_string(),
                    reason: Some("The terminal UI controls this live managed session".to_string()),
                },
            )
            .await
        } else {
            runtime
        }
    } else {
        let projection_bundle = session_canonical_thread_id(&session)
            .and_then(|thread_id| Uuid::parse_str(&thread_id).ok());
        let projection_bundle = match projection_bundle {
            Some(thread_id) => {
                match load_code_ui_projection_bundle(registry.working_dir(), thread_id).await {
                    Ok(bundle) => bundle,
                    Err(error) => {
                        tracing::warn!(%thread_id, error = %error, "failed to load projection-backed code ui snapshot; falling back to session state");
                        None
                    }
                }
            }
            None => None,
        };
        build_tui_code_ui_runtime(
            &working_dir_str,
            &session,
            &provider_name,
            &model_name,
            projection_bundle.as_ref(),
            code_control_tx,
            automation_write_enabled,
        )
        .await
    };
    let code_ui_session = code_ui_runtime.adapter().session();
    let code_ui_runtime_for_app = code_ui_runtime.clone();

    let control_thread_id = session_canonical_thread_id(&session);
    let (mut web_handle, web_line) = match start_web_server(
        &params.host,
        params.port,
        registry.working_dir().to_path_buf(),
        WebServerOptions {
            code_ui: Some(code_ui_runtime),
            automation_control_token: control_runtime.token.clone(),
            audit_sink: None,
        },
    )
    .await
    {
        Ok(handle) => {
            let base_url = format!("http://{}", handle.addr);
            if let Err(error) = control_runtime.write_info_file(
                registry.working_dir(),
                base_url.clone(),
                None,
                control_thread_id.clone(),
            ) {
                handle.shutdown().await;
                if let Some(runtime) = managed_code_ui_runtime.as_ref() {
                    let _ = runtime.shutdown().await;
                }
                return Err(error);
            }
            let line = format!("Web: {base_url}");
            (Some(handle), line)
        }
        Err(err) if control_runtime.is_write() => {
            if let Some(runtime) = managed_code_ui_runtime.as_ref() {
                let _ = runtime.shutdown().await;
            }
            return Err(
                CliError::network(format!("failed to start web server: {err}"))
                    .with_detail("component", "web_server"),
            );
        }
        Err(err) => (
            None::<WebServerHandle>,
            format!("Web: failed to start ({err})"),
        ),
    };
    let control_base_url = web_handle
        .as_ref()
        .map(|handle| format!("http://{}", handle.addr));

    // Start MCP Server
    let (mcp_handle, mcp_line) =
        match start_mcp_server(&params.host, params.mcp_port, params.mcp_server.clone()).await {
            Ok(handle) => {
                let mcp_url = format!("http://{}", handle.addr);
                if let Some(base_url) = control_base_url.as_ref()
                    && let Err(error) = control_runtime.write_info_file(
                        registry.working_dir(),
                        base_url.clone(),
                        Some(mcp_url.clone()),
                        control_thread_id.clone(),
                    )
                {
                    if let Some(handle) = web_handle.take() {
                        handle.shutdown().await;
                    }
                    handle.shutdown().await;
                    if let Some(runtime) = managed_code_ui_runtime.as_ref() {
                        let _ = runtime.shutdown().await;
                    }
                    return Err(error);
                }
                let line = format!("MCP: {mcp_url}");
                (Some(handle), line)
            }
            Err(err) if control_runtime.is_write() => {
                if let Some(handle) = web_handle.take() {
                    handle.shutdown().await;
                }
                if let Some(runtime) = managed_code_ui_runtime.as_ref() {
                    let _ = runtime.shutdown().await;
                }
                return Err(
                    CliError::network(format!("failed to start MCP server: {err}"))
                        .with_detail("component", "mcp_server"),
                );
            }
            Err(err) => (None, format!("MCP: failed to start ({err})")),
        };

    let input_guidance = if managed_code_ui_runtime.is_some() {
        "Type your message and press Enter to work with the managed provider."
    } else {
        "Type a development request and press Enter to generate a reviewable plan before execution."
    };
    let welcome = format!("Welcome to Libra Code! {input_guidance}\n{web_line}\n{mcp_line}");

    // Load slash commands
    let commands = load_commands(registry.working_dir());
    let command_dispatcher = CommandDispatcher::new(commands);

    // Load agent profiles
    let profiles = load_profiles(registry.working_dir());
    let agent_router = AgentProfileRouter::new(profiles);
    let managed_runtime_for_shutdown = managed_code_ui_runtime.clone();

    // Create and run app
    let mut app = App::new(
        tui,
        model,
        registry,
        config,
        AppConfig {
            welcome_message: welcome,
            command_dispatcher,
            agent_router,
            session,
            session_store,
            user_input_rx: params.user_input_rx,
            exec_approval_rx: params.exec_approval_rx,
            model_name,
            provider_name,
            mcp_server: Some(params.mcp_server),
            code_ui_session: Some(code_ui_session),
            code_ui_runtime: Some(code_ui_runtime_for_app),
            code_control_rx,
            managed_code_ui_runtime,
            default_network_access: params.network_access,
        },
    );

    let graph_thread_hint = match app.run().await {
        Ok(exit_info) => {
            if let ExitReason::Fatal(msg) = exit_info.reason {
                return Err(
                    CliError::fatal(msg).with_stable_code(StableErrorCode::InternalInvariant)
                );
            }
            exit_info.thread_id
        }
        Err(e) => return Err(CliError::internal(format!("TUI exited unexpectedly: {e}"))),
    };

    if let Some(handle) = web_handle {
        handle.shutdown().await;
    }
    if let Some(handle) = mcp_handle {
        handle.shutdown().await;
    }
    if let Some(runtime) = managed_runtime_for_shutdown {
        let _ = runtime.shutdown().await;
    }
    if let Some(thread_id) = graph_thread_hint {
        println!("Inspect this thread graph with: libra graph {thread_id}");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// MCP server — Streamable HTTP transport via Hyper
// ---------------------------------------------------------------------------

/// Starts the MCP server using `rmcp`'s Streamable HTTP transport.
///
/// Each incoming TCP connection is handled by a Hyper service that wraps the
/// `StreamableHttpService`. Per-connection tasks are tracked in `connection_tasks`
/// so they can be aborted during shutdown, preventing task leaks.
///
/// Uses `LocalSessionManager` for session management (single-node, in-memory).
async fn start_mcp_server(
    host: &str,
    port: u16,
    mcp_server: Arc<LibraMcpServer>,
) -> anyhow::Result<McpServerHandle> {
    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let bound_addr = listener.local_addr()?;

    // Use rmcp's Streamable HTTP transport via Hyper directly
    let service = TowerToHyperService::new(StreamableHttpService::new(
        move || Ok(mcp_server.clone()),
        LocalSessionManager::default().into(),
        Default::default(),
    ));

    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
    let connection_tasks: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>> =
        Arc::new(Mutex::new(Vec::new()));
    let tracked_connections = connection_tasks.clone();

    let join = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => {
                    break;
                }
                accept = listener.accept() => {
                    match accept {
                        Ok((stream, _)) => {
                            let io = TokioIo::new(stream);
                            let service = service.clone();
                            let conn_task = tokio::spawn(async move {
                                if let Err(e) = hyper_util::server::conn::auto::Builder::new(TokioExecutor::default())
                                    .serve_connection(io, service)
                                    .await
                                {
                                    cli_error!(e, "warning: MCP connection error");
                                }
                            });
                            match tracked_connections.lock() {
                                Ok(mut tasks) => {
                                    tasks.retain(|task| !task.is_finished());
                                    tasks.push(conn_task);
                                }
                                Err(_) => conn_task.abort(),
                            }
                        }
                        Err(e) => {
                            cli_error!(e, "warning: MCP accept error");
                        }
                    }
                }
            }
        }
        Ok(())
    });

    Ok(McpServerHandle {
        addr: bound_addr,
        shutdown_tx,
        join,
        connection_tasks,
    })
}

// ---------------------------------------------------------------------------
// System prompt and runtime context construction
// ---------------------------------------------------------------------------

/// Builds the system prompt (preamble) for the AI agent, incorporating the
/// working directory context and optional operating mode (dev/review/research).
fn system_preamble(working_dir: &std::path::Path, context: Option<CodeContext>) -> String {
    let mut builder = SystemPromptBuilder::new(working_dir);
    if let Some(ctx) = context {
        let mode = match ctx {
            CodeContext::Dev => ContextMode::Dev,
            CodeContext::Review => ContextMode::Review,
            CodeContext::Research => ContextMode::Research,
        };
        builder = builder.with_context(mode);
    }
    builder.build()
}

/// Constructs the default [`ToolRuntimeContext`] for TUI mode, configuring
/// the sandbox policy based on the operating context:
///
/// - **Dev mode (or no context)**: Workspace-write sandbox allowing modifications
///   within the working directory; network access follows the developer's
///   selected policy.
/// - **Review / Research mode**: Read-only sandbox; no writes or network access.
///
/// The approval policy and its communication channel are also wired in here.
fn default_tui_runtime_context(
    working_dir: &std::path::Path,
    context: Option<CodeContext>,
    approval_policy: AskForApproval,
    allow_all_commands: bool,
    network_access: bool,
    exec_approval_tx: tokio::sync::mpsc::UnboundedSender<ExecApprovalRequest>,
) -> ToolRuntimeContext {
    let policy = match context {
        Some(CodeContext::Review | CodeContext::Research) => SandboxPolicy::ReadOnly,
        Some(CodeContext::Dev) | None => SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![working_dir.to_path_buf()],
            network_access,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        },
    };

    let mut approval_store = ApprovalStore::default();
    if allow_all_commands {
        approval_store.approve_all_commands();
    }

    ToolRuntimeContext {
        sandbox: Some(ToolSandboxContext {
            policy,
            permissions: SandboxPermissions::UseDefault,
        }),
        sandbox_runtime: None,
        approval: Some(ToolApprovalContext {
            policy: approval_policy,
            request_tx: exec_approval_tx,
            store: Arc::new(tokio::sync::Mutex::new(approval_store)),
            scope_key_prefix: None,
        }),
        max_output_bytes: None,
    }
}

// ---------------------------------------------------------------------------
// MCP server initialization — storage and database setup
// ---------------------------------------------------------------------------

/// Initializes the [`LibraMcpServer`] instance with optional history persistence.
///
/// Sets up the local object storage directory and SQLite database under the
/// `.libra/` storage root. If any step fails (directory creation, DB connection),
/// falls back to a read-only MCP server with history disabled, printing a warning.
///
/// # Side Effects
/// - Creates the local object storage directory when possible.
/// - Opens a SQLite connection for intent/run history when the DB path is usable.
/// - Prints warnings to stderr before falling back to history-disabled mode.
///
/// # Errors
/// This helper intentionally does not return errors. It converts storage/DB
/// setup failures into a read-only MCP server so AI clients can still inspect
/// files and continue a degraded session.
async fn init_mcp_server(working_dir: &std::path::Path) -> Arc<LibraMcpServer> {
    let storage_dir = resolve_storage_root(working_dir);
    let objects_dir = storage_dir.join("objects");
    let dot_libra = storage_dir;

    // Try to create the directory. If it fails, we assume read-only or permission issues.
    if let Err(e) = std::fs::create_dir_all(&objects_dir) {
        eprintln!(
            "Warning: Failed to create storage directory: {}. Running in read-only mode (history/context disabled). Error: {}",
            objects_dir.display(),
            e
        );
        return Arc::new(LibraMcpServer::new_with_working_dir(
            None,
            None,
            working_dir.to_path_buf(),
        ));
    }

    // Connect to DB
    let db_path = dot_libra.join("libra.db");
    let Some(db_path_str) = db_path.to_str() else {
        eprintln!(
            "Warning: Database path is not valid UTF-8: {}. History disabled.",
            db_path.display()
        );
        return Arc::new(LibraMcpServer::new_with_working_dir(
            None,
            None,
            working_dir.to_path_buf(),
        ));
    };

    #[cfg(target_os = "windows")]
    let db_path_string = db_path_str.replace("\\", "/");
    #[cfg(target_os = "windows")]
    let db_path_str = &db_path_string;

    let db_conn = match establish_connection(db_path_str).await {
        Ok(conn) => Arc::new(conn),
        Err(e) => {
            eprintln!(
                "Warning: Failed to connect to database: {}. History disabled.",
                e
            );
            return Arc::new(LibraMcpServer::new_with_working_dir(
                None,
                None,
                working_dir.to_path_buf(),
            ));
        }
    };

    let storage = Arc::new(ClientStorage::init(objects_dir));
    let intent_history_manager = Arc::new(HistoryManager::new(storage.clone(), dot_libra, db_conn));
    Arc::new(LibraMcpServer::new_with_working_dir(
        Some(intent_history_manager),
        Some(storage),
        working_dir.to_path_buf(),
    ))
}

/// Resolves the `.libra/` storage root for the given working directory.
///
/// Supports linked worktrees by delegating to `try_get_storage_path`, which
/// follows `.libra` symlinks to the main repository's storage. Falls back to
/// `<working_dir>/.libra` if resolution fails.
pub(crate) fn resolve_storage_root(working_dir: &std::path::Path) -> std::path::PathBuf {
    try_get_storage_path(Some(working_dir.to_path_buf()))
        .unwrap_or_else(|_| working_dir.join(".libra"))
}

// ---------------------------------------------------------------------------
// Mode: Stdio — MCP server over stdin/stdout
// ---------------------------------------------------------------------------

/// Runs the MCP server over stdin/stdout using `rmcp`'s async read/write
/// transport. This mode is designed for integration with AI clients (e.g.
/// Claude Desktop) that communicate via the Model Context Protocol over pipes.
///
/// Blocks until the MCP session ends (client disconnects or EOF on stdin).
///
/// # Side Effects
/// - Takes ownership of process stdin/stdout for the MCP transport.
/// - Initializes the same history/object-backed MCP server used by other modes.
///
/// # Errors
/// Returns [`CliError`] when working-dir resolution fails, the MCP server cannot
/// start on stdio, or the running MCP session reports an unrecoverable error.
async fn execute_stdio(args: &CodeArgs) -> CliResult<()> {
    let working_dir = resolve_code_working_dir(args)?;

    let mcp_server = init_mcp_server(&working_dir).await;

    use rmcp::{
        service::serve_server,
        transport::{async_rw::AsyncRwTransport, io::stdio},
    };

    let (stdin, stdout) = stdio();
    let transport = AsyncRwTransport::new_server(stdin, stdout);

    match serve_server(mcp_server, transport).await {
        Ok(running) => {
            if let Err(e) = running.waiting().await {
                return Err(CliError::internal(format!("MCP Stdio server error: {}", e)));
            }
        }
        Err(e) => {
            return Err(CliError::network(format!(
                "failed to start MCP Stdio server: {e}"
            )));
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// CLI argument validation
// ---------------------------------------------------------------------------

/// Validates CLI flag combinations across all three operating modes.
///
/// Enforces constraints such as:
/// - Web and MCP ports must differ (except in stdio mode).
/// - TUI-specific flags (--model, --temperature, --resume, etc.) are rejected
///   in web-only and stdio modes.
/// - Provider-specific flags are only accepted for their respective providers.
fn validate_mode_args(args: &CodeArgs, _output: &OutputConfig) -> Result<(), String> {
    if !args.stdio && args.port == args.mcp_port && args.port != 0 {
        return Err(format!(
            "--port ({}) and --mcp-port ({}) must be different",
            args.port, args.mcp_port
        ));
    }

    if args.web_only {
        reject_non_tui_flags(args, "--web")?;
    }

    if args.stdio {
        if args.control == ControlMode::Write {
            return Err(
                "--control write is not supported with `libra code --stdio` because --stdio is the MCP stdio transport; use `libra code-control --stdio` for local TUI automation"
                    .to_string(),
            );
        }
        reject_non_tui_flags(args, "--stdio")?;
        reject_mode_flag(args.host != DEFAULT_BIND_HOST, "--host", "--stdio")?;
        reject_mode_flag(args.port != DEFAULT_WEB_PORT, "--port", "--stdio")?;
        reject_mode_flag(args.mcp_port != DEFAULT_MCP_PORT, "--mcp-port", "--stdio")?;
    }

    if args.control == ControlMode::Write {
        ensure_loopback_control_host_for_validation(&args.host)?;
    }

    if args.provider != CodeProvider::Codex {
        if args.codex_port.is_some() {
            return Err("--codex-port is only supported with --provider=codex".to_string());
        }
        if args.codex_bin != DEFAULT_CODEX_BIN {
            return Err("--codex-bin is only supported with --provider=codex".to_string());
        }
        if args.plan_mode {
            return Err("--plan-mode is only supported with --provider=codex".to_string());
        }
    }

    if args.provider == CodeProvider::Codex && args.api_base.is_some() {
        return Err("--api-base is not supported with --provider=codex".to_string());
    }

    if args.provider != CodeProvider::Ollama && args.ollama_thinking.is_some() {
        return Err(
            "--ollama-thinking/--thinking is only supported with --provider=ollama".to_string(),
        );
    }

    if args.provider != CodeProvider::Ollama && args.ollama_compact_tools {
        return Err("--ollama-compact-tools is only supported with --provider=ollama".to_string());
    }

    if args.provider != CodeProvider::Deepseek && args.deepseek_thinking.is_some() {
        return Err("--deepseek-thinking is only supported with --provider=deepseek".to_string());
    }

    if args.provider != CodeProvider::Deepseek && args.deepseek_reasoning_effort.is_some() {
        return Err(
            "--deepseek-reasoning-effort is only supported with --provider=deepseek".to_string(),
        );
    }

    if args.provider != CodeProvider::Deepseek && args.deepseek_stream.is_some() {
        return Err(
            "--deepseek-stream/--stream is only supported with --provider=deepseek".to_string(),
        );
    }

    if args.provider != CodeProvider::Kimi && args.kimi_thinking.is_some() {
        return Err("--kimi-thinking is only supported with --provider=kimi".to_string());
    }

    if args.provider != CodeProvider::Kimi && args.kimi_stream.is_some() {
        return Err("--kimi-stream is only supported with --provider=kimi".to_string());
    }

    #[cfg(feature = "test-provider")]
    {
        if args.provider == CodeProvider::Fake {
            if std::env::var_os("LIBRA_ENABLE_TEST_PROVIDER").is_none() {
                return Err(
                    "--provider=fake is test-only; set LIBRA_ENABLE_TEST_PROVIDER=1 to use it"
                        .to_string(),
                );
            }
            if args.fake_fixture.is_none() {
                return Err("--fake-fixture is required with --provider=fake".to_string());
            }
        } else if args.fake_fixture.is_some() {
            return Err("--fake-fixture is only supported with --provider=fake".to_string());
        }
    }

    Ok(())
}

/// Helper: rejects a flag if it was set (`is_invalid == true`) with a
/// standardized error message indicating the flag is not supported in the given mode.
fn reject_mode_flag(is_invalid: bool, flag: &str, mode: &str) -> Result<(), String> {
    if is_invalid {
        return Err(format!("{flag} is not supported in {mode} mode"));
    }
    Ok(())
}

fn ensure_loopback_control_host_for_validation(host: &str) -> Result<(), String> {
    let normalized = host.trim().trim_matches('[').trim_matches(']');
    let is_loopback = matches!(normalized, "localhost" | "127.0.0.1" | "::1")
        || normalized
            .parse::<std::net::IpAddr>()
            .map(|addr| addr.is_loopback())
            .unwrap_or(false);

    if is_loopback {
        Ok(())
    } else {
        Err("--control write requires a loopback --host such as 127.0.0.1 or ::1".to_string())
    }
}

/// Rejects all TUI-specific flags when running in a non-TUI mode (web-only or stdio).
/// This ensures users get clear errors instead of silently ignored flags.
fn reject_non_tui_flags(args: &CodeArgs, mode: &str) -> Result<(), String> {
    reject_mode_flag(args.provider != CodeProvider::Gemini, "--provider", mode)?;
    reject_mode_flag(args.model.is_some(), "--model", mode)?;
    reject_mode_flag(args.temperature.is_some(), "--temperature", mode)?;
    reject_mode_flag(args.env_file.is_some(), "--env-file", mode)?;
    reject_mode_flag(args.ollama_thinking.is_some(), "--ollama-thinking", mode)?;
    reject_mode_flag(args.ollama_compact_tools, "--ollama-compact-tools", mode)?;
    reject_mode_flag(
        args.deepseek_thinking.is_some(),
        "--deepseek-thinking",
        mode,
    )?;
    reject_mode_flag(
        args.deepseek_reasoning_effort.is_some(),
        "--deepseek-reasoning-effort",
        mode,
    )?;
    reject_mode_flag(args.deepseek_stream.is_some(), "--deepseek-stream", mode)?;
    reject_mode_flag(args.kimi_thinking.is_some(), "--kimi-thinking", mode)?;
    reject_mode_flag(args.kimi_stream.is_some(), "--kimi-stream", mode)?;
    reject_mode_flag(args.context.is_some(), "--context", mode)?;
    reject_mode_flag(args.resume.is_some(), "--resume", mode)?;
    reject_mode_flag(
        args.approval_policy != CodeApprovalPolicy::OnRequest,
        "--approval-policy",
        mode,
    )?;
    reject_mode_flag(
        args.network_access != CodeNetworkAccess::Deny,
        "--network-access",
        mode,
    )?;
    reject_mode_flag(args.api_base.is_some(), "--api-base", mode)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use tokio::sync::mpsc::unbounded_channel;

    use super::*;

    fn base_args() -> CodeArgs {
        CodeArgs {
            web_only: false,
            port: DEFAULT_WEB_PORT,
            host: DEFAULT_BIND_HOST.to_string(),
            cwd: None,
            repo: None,
            env_file: None,
            control: ControlMode::Observe,
            control_token_file: None,
            control_info_file: None,
            provider: CodeProvider::Gemini,
            model: None,
            temperature: None,
            ollama_thinking: None,
            ollama_compact_tools: false,
            deepseek_thinking: None,
            deepseek_reasoning_effort: None,
            deepseek_stream: None,
            kimi_thinking: None,
            kimi_stream: None,
            #[cfg(feature = "test-provider")]
            fake_fixture: None,
            context: None,
            resume: None,
            approval_policy: CodeApprovalPolicy::OnRequest,
            network_access: CodeNetworkAccess::Deny,
            mcp_port: DEFAULT_MCP_PORT,
            stdio: false,
            api_base: None,
            codex_bin: DEFAULT_CODEX_BIN.to_string(),
            codex_port: None,
            plan_mode: false,
        }
    }

    #[test]
    fn rejects_same_web_and_mcp_ports() {
        let mut args = base_args();
        args.mcp_port = args.port;
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_err());
    }

    #[test]
    fn rejects_tui_flags_in_web_mode() {
        let mut args = base_args();
        args.web_only = true;
        args.model = Some("foo".to_string());
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_err());
    }

    #[test]
    fn rejects_web_flags_in_stdio_mode() {
        let mut args = base_args();
        args.stdio = true;
        args.host = "0.0.0.0".to_string();
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_err());
    }

    #[test]
    fn accepts_default_tui_mode() {
        let args = base_args();
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_ok());
    }

    #[test]
    fn accepts_control_write_in_default_tui_mode() {
        let mut args = base_args();
        args.control = ControlMode::Write;

        assert!(validate_mode_args(&args, &OutputConfig::default()).is_ok());
    }

    #[test]
    fn accepts_control_write_in_default_web_mode() {
        let args = CodeArgs::try_parse_from(["libra", "--web", "--control", "write"]).unwrap();

        assert!(args.web_only);
        assert_eq!(args.control, ControlMode::Write);
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_ok());
    }

    #[test]
    fn rejects_control_write_in_stdio_mode() {
        let mut args = base_args();
        args.stdio = true;
        args.control = ControlMode::Write;

        let err = validate_mode_args(&args, &OutputConfig::default()).unwrap_err();
        assert!(err.contains("code-control --stdio"));
    }

    #[test]
    fn rejects_control_write_with_non_loopback_host() {
        let mut args = base_args();
        args.control = ControlMode::Write;
        args.host = "0.0.0.0".to_string();

        let err = validate_mode_args(&args, &OutputConfig::default()).unwrap_err();
        assert!(err.contains("loopback"));
    }

    #[test]
    fn accepts_env_file_cli_arg_in_tui_mode() {
        let args = CodeArgs::try_parse_from(["libra", "--env-file", ".env.test"]).unwrap();

        assert_eq!(args.env_file.as_deref(), Some(Path::new(".env.test")));
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_ok());
    }

    #[test]
    fn rejects_env_file_in_web_mode() {
        let mut args = base_args();
        args.web_only = true;
        args.env_file = Some(PathBuf::from(".env.test"));

        let err = validate_mode_args(&args, &OutputConfig::default()).unwrap_err();
        assert!(err.contains("--env-file"));
    }

    #[test]
    fn parses_dotenv_style_env_file() {
        let env_file = parse_code_env_file(
            r#"
            # comments and blank lines are ignored
            export DEEPSEEK_API_KEY='deepseek-key'
            OPENAI_BASE_URL="https://example.test/v1"
            UNQUOTED=value # inline comment
            "#,
            Path::new(".env.test"),
        )
        .unwrap();

        assert_eq!(env_file.get("DEEPSEEK_API_KEY"), Some("deepseek-key"));
        assert_eq!(
            env_file.get("OPENAI_BASE_URL"),
            Some("https://example.test/v1")
        );
        assert_eq!(env_file.get("UNQUOTED"), Some("value"));
    }

    #[test]
    fn provider_env_file_value_overrides_process_lookup() {
        let env_file =
            parse_code_env_file("DEEPSEEK_API_KEY=file-key", Path::new(".env.test")).unwrap();

        let value = provider_env_value_with_lookup(&env_file, "DEEPSEEK_API_KEY", |_| {
            Some("old-key".into())
        });

        assert_eq!(value.as_deref(), Some("file-key"));
    }

    #[test]
    fn accepts_network_access_cli_arg_in_tui_mode() {
        let args = CodeArgs::try_parse_from(["libra", "--network-access", "allow"]).unwrap();

        assert_eq!(args.network_access, CodeNetworkAccess::Allow);
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_ok());
    }

    #[test]
    fn accepts_allow_all_approval_policy_in_tui_mode() {
        let args = CodeArgs::try_parse_from(["libra", "--approval-policy", "allow-all"]).unwrap();

        assert_eq!(args.approval_policy, CodeApprovalPolicy::AllowAll);
        assert!(args.approval_policy.allows_all_commands());
        assert_eq!(
            AskForApproval::from(args.approval_policy),
            AskForApproval::OnRequest
        );
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_ok());
    }

    #[test]
    fn rejects_network_access_cli_arg_with_invalid_value() {
        let result = CodeArgs::try_parse_from(["libra", "--network-access", "sometimes"]);

        assert!(result.is_err());
    }

    #[test]
    fn rejects_network_access_flag_in_web_mode() {
        let mut args = base_args();
        args.web_only = true;
        args.network_access = CodeNetworkAccess::Allow;

        let err = validate_mode_args(&args, &OutputConfig::default()).unwrap_err();
        assert!(err.contains("--network-access"));
    }

    #[test]
    fn accepts_anthropic_provider_in_tui_mode() {
        let mut args = base_args();
        args.provider = CodeProvider::Anthropic;
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_ok());
    }

    #[test]
    fn rejects_ollama_thinking_for_non_ollama_provider() {
        let mut args = base_args();
        args.ollama_thinking = Some(OllamaThinkingArg::High);
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_err());
    }

    #[test]
    fn accepts_ollama_thinking_for_ollama_provider() {
        let mut args = base_args();
        args.provider = CodeProvider::Ollama;
        args.ollama_thinking = Some(OllamaThinkingArg::High);
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_ok());
    }

    #[test]
    fn rejects_ollama_compact_tools_for_non_ollama_provider() {
        let mut args = base_args();
        args.ollama_compact_tools = true;
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_err());
    }

    #[test]
    fn accepts_ollama_compact_tools_for_ollama_provider() {
        let mut args = base_args();
        args.provider = CodeProvider::Ollama;
        args.ollama_compact_tools = true;
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_ok());
    }

    #[test]
    fn accepts_deepseek_reasoning_flags_for_deepseek_provider() {
        let args = CodeArgs::try_parse_from([
            "libra",
            "--provider",
            "deepseek",
            "--model",
            "deepseek-v4-pro",
            "--deepseek-thinking",
            "enabled",
            "--deepseek-reasoning-effort",
            "high",
            "--deepseek-stream",
            "true",
        ])
        .unwrap();

        assert_eq!(args.provider, CodeProvider::Deepseek);
        assert_eq!(args.deepseek_thinking, Some(DeepSeekThinkingArg::Enabled));
        assert_eq!(
            args.deepseek_reasoning_effort,
            Some(DeepSeekReasoningEffortArg::High)
        );
        assert_eq!(
            completion_thinking_for_args(&args),
            Some(CompletionThinking::Enabled)
        );
        assert_eq!(
            completion_reasoning_effort_for_args(&args),
            Some(CompletionReasoningEffort::High)
        );
        assert_eq!(completion_stream_for_args(&args), Some(true));
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_ok());
    }

    #[test]
    fn accepts_deepseek_max_reasoning_alias() {
        let args = CodeArgs::try_parse_from([
            "libra",
            "--provider",
            "deepseek",
            "--deepseek-reasoning-effort",
            "xhigh",
        ])
        .unwrap();

        assert_eq!(
            args.deepseek_reasoning_effort,
            Some(DeepSeekReasoningEffortArg::Max)
        );
        assert_eq!(
            completion_reasoning_effort_for_args(&args),
            Some(CompletionReasoningEffort::Max)
        );
    }

    #[test]
    fn rejects_deepseek_reasoning_flags_for_non_deepseek_provider() {
        let mut args = base_args();
        args.deepseek_thinking = Some(DeepSeekThinkingArg::Enabled);
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_err());

        let mut args = base_args();
        args.deepseek_reasoning_effort = Some(DeepSeekReasoningEffortArg::High);
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_err());

        let mut args = base_args();
        args.deepseek_stream = Some(true);
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_err());
    }

    #[test]
    fn accepts_kimi_thinking_for_kimi_provider() {
        let args = CodeArgs::try_parse_from([
            "libra",
            "--provider",
            "kimi",
            "--model",
            "kimi-k2.6",
            "--kimi-thinking",
            "disabled",
        ])
        .unwrap();

        assert_eq!(args.provider, CodeProvider::Kimi);
        assert_eq!(args.kimi_thinking, Some(KimiThinkingArg::Disabled));
        assert_eq!(
            completion_thinking_for_args(&args),
            Some(CompletionThinking::Disabled)
        );
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_ok());
    }

    #[test]
    fn defaults_kimi_stream_for_kimi_provider() {
        let args = CodeArgs::try_parse_from(["libra", "--provider", "kimi"]).unwrap();

        assert_eq!(args.provider, CodeProvider::Kimi);
        assert_eq!(args.kimi_stream, None);
        assert_eq!(completion_stream_for_args(&args), Some(true));
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_ok());
    }

    #[test]
    fn accepts_kimi_stream_override_for_kimi_provider() {
        let args =
            CodeArgs::try_parse_from(["libra", "--provider", "kimi", "--kimi-stream", "false"])
                .unwrap();

        assert_eq!(args.provider, CodeProvider::Kimi);
        assert_eq!(args.kimi_stream, Some(false));
        assert_eq!(completion_stream_for_args(&args), Some(false));
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_ok());
    }

    #[test]
    fn rejects_kimi_thinking_for_non_kimi_provider() {
        let mut args = base_args();
        args.kimi_thinking = Some(KimiThinkingArg::Enabled);

        let err = validate_mode_args(&args, &OutputConfig::default()).unwrap_err();
        assert!(err.contains("--kimi-thinking"));
    }

    #[test]
    fn rejects_kimi_stream_for_non_kimi_provider() {
        let mut args = base_args();
        args.kimi_stream = Some(true);

        let err = validate_mode_args(&args, &OutputConfig::default()).unwrap_err();
        assert!(err.contains("--kimi-stream"));
    }

    #[test]
    fn accepts_deepseek_stream_alias_for_deepseek_provider() {
        let args =
            CodeArgs::try_parse_from(["libra", "--provider", "deepseek", "--stream", "false"])
                .unwrap();

        assert_eq!(args.deepseek_stream, Some(false));
        assert_eq!(completion_stream_for_args(&args), Some(false));
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_ok());
    }

    #[test]
    fn tui_preserves_reasoning_content_for_reasoning_providers() {
        assert!(preserve_reasoning_content_for_provider(
            CodeProvider::Deepseek
        ));
        assert!(!preserve_reasoning_content_for_provider(
            CodeProvider::Gemini
        ));
        assert!(!preserve_reasoning_content_for_provider(
            CodeProvider::Ollama
        ));
        assert!(preserve_reasoning_content_for_provider(CodeProvider::Kimi));
    }

    #[test]
    fn codex_preflight_rejects_file_cwd() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cwd_file = temp_dir.path().join("README.md");
        std::fs::write(&cwd_file, "not a directory").unwrap();

        let mut args = base_args();
        args.provider = CodeProvider::Codex;
        args.cwd = Some(cwd_file.clone());

        let err = resolve_code_preflight_working_dir(&args).unwrap_err();
        assert!(
            err.to_string().contains("--cwd must point to a directory"),
            "unexpected error: {err}"
        );
        assert!(
            err.to_string().contains(&cwd_file.display().to_string()),
            "error should identify the invalid --cwd path: {err}"
        );
    }

    #[test]
    fn code_ui_runtime_uses_canonical_thread_id_metadata() {
        let mut session = SessionState::new("/tmp/workspace");
        session.id = "legacy-session".to_string();
        session.metadata.insert(
            "thread_id".to_string(),
            serde_json::json!("11111111-1111-4111-8111-111111111111"),
        );

        assert_eq!(
            session_canonical_thread_id(&session).as_deref(),
            Some("11111111-1111-4111-8111-111111111111")
        );
    }

    #[tokio::test]
    async fn tui_code_ui_runtime_prefers_projection_bundle_identity() {
        let thread_id = Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap();
        let actor = git_internal::internal::object::types::ActorRef::human("tester").unwrap();
        let bundle = ThreadBundle {
            thread: crate::internal::ai::projection::ThreadProjection {
                thread_id,
                title: Some("projection thread".to_string()),
                owner: actor.clone(),
                participants: vec![crate::internal::ai::projection::ThreadParticipant {
                    actor,
                    role: crate::internal::ai::projection::ThreadParticipantRole::Owner,
                    joined_at: Utc::now(),
                }],
                current_intent_id: None,
                latest_intent_id: None,
                intents: Vec::new(),
                metadata: None,
                archived: false,
                created_at: Utc::now(),
                updated_at: Utc::now(),
                version: 1,
            },
            scheduler: crate::internal::ai::projection::SchedulerState {
                thread_id,
                selected_plan_id: None,
                selected_plan_ids: Vec::new(),
                current_plan_heads: Vec::new(),
                active_task_id: None,
                active_run_id: None,
                live_context_window: Vec::new(),
                metadata: None,
                updated_at: Utc::now(),
                version: 1,
            },
            freshness: crate::internal::ai::runtime::contracts::ProjectionFreshness::Fresh,
        };
        let mut session = SessionState::new("/tmp/workspace");
        session.id = "legacy-session".to_string();

        let runtime = build_tui_code_ui_runtime(
            "/tmp/workspace",
            &session,
            "ollama",
            "gemma4:31b",
            Some(&bundle),
            None,
            false,
        )
        .await;
        let snapshot = runtime.snapshot().await;

        assert_eq!(snapshot.session_id, thread_id.to_string());
        assert_eq!(snapshot.thread_id, Some(thread_id.to_string()));
    }

    #[test]
    fn default_tui_runtime_context_denies_network_in_dev_mode() {
        let (tx, _rx) = unbounded_channel();
        let runtime = default_tui_runtime_context(
            Path::new("/tmp/workspace"),
            Some(CodeContext::Dev),
            AskForApproval::OnRequest,
            false,
            false,
            tx,
        );

        let sandbox = runtime.sandbox.expect("sandbox context should be present");
        assert!(matches!(
            sandbox.policy,
            SandboxPolicy::WorkspaceWrite {
                writable_roots,
                network_access,
                ..
            } if writable_roots == vec![PathBuf::from("/tmp/workspace")] && !network_access
        ));
    }

    #[test]
    fn default_tui_runtime_context_allows_network_when_requested_in_dev_mode() {
        let (tx, _rx) = unbounded_channel();
        let runtime = default_tui_runtime_context(
            Path::new("/tmp/workspace"),
            Some(CodeContext::Dev),
            AskForApproval::OnRequest,
            false,
            true,
            tx,
        );

        let sandbox = runtime.sandbox.expect("sandbox context should be present");
        assert!(matches!(
            sandbox.policy,
            SandboxPolicy::WorkspaceWrite {
                writable_roots,
                network_access,
                ..
            } if writable_roots == vec![PathBuf::from("/tmp/workspace")] && network_access
        ));
    }

    #[tokio::test]
    async fn default_tui_runtime_context_can_allow_all_commands() {
        let (tx, _rx) = unbounded_channel();
        let runtime = default_tui_runtime_context(
            Path::new("/tmp/workspace"),
            Some(CodeContext::Dev),
            AskForApproval::OnRequest,
            true,
            true,
            tx,
        );

        let approval = runtime
            .approval
            .expect("approval context should be present");
        assert!(approval.store.lock().await.allow_all_commands());
    }

    #[test]
    fn default_tui_runtime_context_is_read_only_for_review_and_research() {
        for context in [CodeContext::Review, CodeContext::Research] {
            let (tx, _rx) = unbounded_channel();
            let runtime = default_tui_runtime_context(
                Path::new("/tmp/workspace"),
                Some(context),
                AskForApproval::OnRequest,
                false,
                true,
                tx,
            );

            let sandbox = runtime.sandbox.expect("sandbox context should be present");
            assert!(matches!(sandbox.policy, SandboxPolicy::ReadOnly));
        }
    }
}
