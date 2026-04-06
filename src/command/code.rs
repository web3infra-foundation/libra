//! Code command for interactive coding sessions.
//!
//! Supports three modes:
//! - Default: Terminal UI (TUI) for interactive coding (and background web server)
//! - Web Mode (`--web`): Web server only, suitable for browser access or remote hosting.
//! - Stdio Mode (`--stdio`): MCP server over standard input/output, designed for integration with AI clients like Claude Desktop.

use std::{
    io::IsTerminal,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
};

use axum::{Router, response::IntoResponse, routing::get};
use clap::{Parser, ValueEnum};
use tokio::{
    process::{Child, Command},
    sync::oneshot,
    time::{Duration, Instant, sleep},
};
use tokio_tungstenite::connect_async;
use url::Url;

// use uuid::Uuid;
use crate::internal::{
    ai::{
        claudecode as agent_claudecode,
        client::CompletionClient,
        codex as agent_codex,
        history::HistoryManager,
        mcp::server::LibraMcpServer,
        providers::{
            anthropic::{CLAUDE_3_5_SONNET, Client as AnthropicClient},
            deepseek::client::Client as DeepSeekClient,
            gemini::{Client as GeminiClient, GEMINI_2_5_FLASH},
            ollama::Client as OllamaClient,
            openai::{Client as OpenAIClient, GPT_4O_MINI},
            zhipu::{Client as ZhipuClient, GLM_5},
        },
        sandbox::{
            ApprovalStore, AskForApproval, ExecApprovalRequest, SandboxPermissions, SandboxPolicy,
            ToolApprovalContext, ToolRuntimeContext, ToolSandboxContext,
        },
        tools::{
            ToolRegistry, ToolRegistryBuilder,
            handlers::{
                ApplyPatchHandler, GrepFilesHandler, ListDirHandler, McpBridgeHandler, PlanHandler,
                ReadFileHandler, RequestUserInputHandler, ShellHandler, SubmitIntentDraftHandler,
            },
        },
    },
    tui::{App, AppConfig, Tui, tui_init, tui_restore},
};
use crate::{
    cli_error,
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::OutputConfig,
    },
};

const DEFAULT_WEB_PORT: u16 = 3000;
const DEFAULT_MCP_PORT: u16 = 6789;
const DEFAULT_BIND_HOST: &str = "127.0.0.1";
const DEFAULT_CODEX_BIN: &str = "codex";
const CODEX_STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
const CODEX_STARTUP_POLL_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum CodeProvider {
    Gemini,
    Openai,
    Anthropic,
    Claudecode,
    Deepseek,
    Zhipu,
    Ollama,
    Codex,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum CodeContext {
    #[value(alias = "development")]
    Dev,
    #[value(alias = "code-review")]
    Review,
    #[value(alias = "explore")]
    Research,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum CodeApprovalPolicy {
    /// Never prompt; dangerous commands are rejected.
    Never,
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

impl From<CodeApprovalPolicy> for AskForApproval {
    fn from(value: CodeApprovalPolicy) -> Self {
        match value {
            CodeApprovalPolicy::Never => AskForApproval::Never,
            CodeApprovalPolicy::OnFailure => AskForApproval::OnFailure,
            CodeApprovalPolicy::OnRequest => AskForApproval::OnRequest,
            CodeApprovalPolicy::Untrusted => AskForApproval::UnlessTrusted,
        }
    }
}

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

    /// AI provider backend
    #[arg(long, value_enum, default_value_t = CodeProvider::Gemini)]
    pub provider: CodeProvider,

    /// Model id (provider-specific)
    #[arg(long)]
    pub model: Option<String>,

    /// Sampling temperature
    #[arg(long)]
    pub temperature: Option<f64>,

    /// Operating context mode (dev, review, research)
    #[arg(long, value_enum)]
    pub context: Option<CodeContext>,

    /// Resume the most recent session
    #[arg(long)]
    pub resume: bool,

    /// Resume a specific Claude Code managed provider session UUID.
    #[arg(long)]
    pub resume_session: Option<String>,

    /// Fork into a new Claude Code managed session when resuming a provider session.
    #[arg(long, default_value_t = false)]
    pub fork_session: bool,

    /// Use an explicit Claude Code managed session UUID on the first turn.
    #[arg(long)]
    pub session_id: Option<String>,

    /// Resume only up to and including a specific Claude Code assistant message UUID.
    #[arg(long)]
    pub resume_at: Option<String>,

    /// Tool approval policy:
    /// - `never`: no prompts, dangerous commands are rejected
    /// - `on-failure`: prompt only for retry outside sandbox after sandbox denial
    /// - `on-request`: run sandboxed by default; prompt for escalation/policy-required cases
    /// - `untrusted`: prompt for non-trusted operations, auto-allow known-safe reads
    #[arg(long, value_enum, default_value_t = CodeApprovalPolicy::OnRequest)]
    pub approval_policy: CodeApprovalPolicy,

    /// Port to listen on (MCP server)
    #[arg(long, default_value_t = DEFAULT_MCP_PORT)]
    pub mcp_port: u16,

    /// Run the MCP server over Stdio (for Claude Desktop integration)
    #[arg(long, alias = "mcp-stdio", conflicts_with = "web_only")]
    pub stdio: bool,

    /// Provider API base URL (e.g. http://remote-host:11434/v1 for remote Ollama)
    #[arg(long)]
    pub api_base: Option<String>,

    /// Optional custom Claude Code managed helper path.
    #[arg(long, hide = true)]
    pub helper_path: Option<PathBuf>,

    /// Python executable used by the Claude Code managed helper.
    #[arg(long, hide = true)]
    pub python_binary: Option<String>,

    /// Override the Claude Code managed helper timeout in seconds.
    #[arg(long, hide = true)]
    pub timeout_seconds: Option<u64>,

    /// Override the Claude Code managed helper permission mode.
    #[arg(long, hide = true)]
    pub permission_mode: Option<String>,

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

/// Serve embedded static assets from the Next.js export (`web/out/`).
///
/// Lookup order:
/// 1. Exact path match (e.g. `_next/static/chunks/main.js`)
/// 2. Directory index (`path/index.html`) — works with `trailingSlash: true`
/// 3. SPA fallback → `index.html`
/// 4. 404
async fn static_handler(uri: axum::http::Uri) -> impl IntoResponse {
    use axum::http::{StatusCode, header};

    use super::web_assets::WebAssets;

    let path = uri.path().trim_start_matches('/');

    // Try exact path, then directory index, then SPA fallback.
    // Track the resolved filename so MIME detection uses the actual file extension.
    let resolved = if WebAssets::get(path).is_some() {
        Some(path.to_string())
    } else {
        let index_path = format!("{}/index.html", path.trim_end_matches('/'));
        if WebAssets::get(&index_path).is_some() {
            Some(index_path)
        } else if WebAssets::get("index.html").is_some() {
            Some("index.html".to_string())
        } else {
            None
        }
    };

    match resolved {
        Some(resolved_path) => match WebAssets::get(&resolved_path) {
            Some(content) => {
                let mime = mime_guess::from_path(&resolved_path).first_or_octet_stream();
                (
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, mime.as_ref().to_string())],
                    content.data.to_vec(),
                )
                    .into_response()
            }
            None => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "embedded asset lookup became inconsistent",
            )
                .into_response(),
        },
        None => (StatusCode::NOT_FOUND, "404 Not Found").into_response(),
    }
}

/// Placeholder API router — extend with endpoints as needed.
fn api_router() -> Router {
    Router::new().route("/health", get(|| async { "ok" }))
}

/// Build the web router: API routes under `/api`, everything else served from
/// the embedded Next.js static export.
fn build_web_router() -> Router {
    Router::new()
        .nest("/api", api_router())
        .fallback(static_handler)
}

struct WebServerHandle {
    addr: SocketAddr,
    shutdown_tx: oneshot::Sender<()>,
    join: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl WebServerHandle {
    async fn shutdown(self) {
        let _ = self.shutdown_tx.send(());
        let _ = self.join.await;
    }
}

struct McpServerHandle {
    addr: SocketAddr,
    shutdown_tx: oneshot::Sender<()>,
    join: tokio::task::JoinHandle<anyhow::Result<()>>,
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

async fn start_web_server(host: &str, port: u16) -> anyhow::Result<WebServerHandle> {
    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;

    let app = build_web_router();
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    let join = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .await
            .map_err(|e| anyhow::anyhow!(e))
    });

    Ok(WebServerHandle {
        addr,
        shutdown_tx,
        join,
    })
}

async fn execute_web_only(args: &CodeArgs) -> CliResult<()> {
    let web_handle = match start_web_server(&args.host, args.port).await {
        Ok(handle) => handle,
        Err(err) => {
            return Err(
                CliError::network(format!("failed to start web server: {err}"))
                    .with_detail("component", "web_server"),
            );
        }
    };
    println!("Libra Code server running at http://{}", web_handle.addr);

    let working_dir = resolve_code_working_dir(args)?;

    let mcp_server = init_mcp_server(&working_dir).await;

    // Start MCP Server
    let mcp_handle = match start_mcp_server(&args.host, args.mcp_port, mcp_server.clone()).await {
        Ok(handle) => {
            println!("MCP: http://{}", handle.addr);
            handle
        }
        Err(err) => {
            web_handle.shutdown().await;
            return Err(
                CliError::network(format!("failed to start MCP server: {err}"))
                    .with_detail("component", "mcp_server"),
            );
        }
    };

    let _ = tokio::signal::ctrl_c().await;
    web_handle.shutdown().await;
    mcp_handle.shutdown().await;
    Ok(())
}

async fn execute_tui(args: CodeArgs) -> CliResult<()> {
    let working_dir = resolve_code_working_dir(&args)?;

    if args.provider == CodeProvider::Codex {
        return execute_codex_mode(args, working_dir).await;
    }

    // Validate --api-base: only honored for Ollama via CLI flag. Other providers
    // accept custom base URLs through their respective environment variables.
    if args.api_base.is_some() && args.provider != CodeProvider::Ollama {
        eprintln!(
            "warning: --api-base is only honored for the ollama provider; \
             use provider-specific env vars (e.g. OPENAI_BASE_URL) for others; ignoring"
        );
    } else if let Some(ref base_url) = args.api_base {
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
    let resume = args.resume;
    let stdout_is_terminal = std::io::stdout().is_terminal();
    let host = args.host.clone();

    // Prepare MCP server instance shared between the HTTP transport and TUI bridge
    let mcp_server = init_mcp_server(&working_dir).await;

    // Create the bridge channel for request_user_input tool <-> TUI communication.
    let (user_input_tx, user_input_rx) = tokio::sync::mpsc::unbounded_channel::<
        crate::internal::ai::tools::context::UserInputRequest,
    >();
    let (exec_approval_tx, exec_approval_rx) =
        tokio::sync::mpsc::unbounded_channel::<ExecApprovalRequest>();

    // Build registry: basic file tools + MCP workflow tools
    let mut builder = ToolRegistryBuilder::with_working_dir(working_dir.clone())
        .register("read_file", Arc::new(ReadFileHandler))
        .register("list_dir", Arc::new(ListDirHandler))
        .register("grep_files", Arc::new(GrepFilesHandler))
        .register("apply_patch", Arc::new(ApplyPatchHandler))
        .register("shell", Arc::new(ShellHandler))
        .register("update_plan", Arc::new(PlanHandler))
        .register("submit_intent_draft", Arc::new(SubmitIntentDraftHandler))
        .register(
            "request_user_input",
            Arc::new(RequestUserInputHandler::new(user_input_tx.clone())),
        );

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
        context: args.context,
        resume,
        approval_policy: args.approval_policy.into(),
        user_input_tx,
        user_input_rx,
        exec_approval_rx,
        exec_approval_tx,
        mcp_server,
        managed_claudecode: None,
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
        CodeProvider::Claudecode => {
            if !stdout_is_terminal {
                return execute_claudecode_mode(args, working_dir).await;
            }

            let claudecode_args = build_claudecode_code_args(&args, working_dir);
            validate_claudecode_code_args(&claudecode_args)?;

            let runtime = agent_claudecode::prepare_tui_runtime(claudecode_args)
                .await
                .map_err(map_claudecode_cli_error)?;

            let model_name = runtime.model_name().to_string();
            let mut launch_config = launch_config;
            launch_config.managed_claudecode = Some(runtime);
            run_tui_with_model(
                UnsupportedCompletionModel,
                launch_config,
                model_name,
                provider_name,
            )
            .await?;
        }
        CodeProvider::Deepseek => {
            let client = match DeepSeekClient::from_env() {
                Ok(client) => client,
                Err(_) => return Err(CliError::auth("DEEPSEEK_API_KEY is not set")),
            };
            let model_name = args.model.unwrap_or_else(|| "deepseek-chat".to_string());
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
            let client = if let Some(base_url) = &args.api_base {
                OllamaClient::with_base_url(base_url)
            } else {
                OllamaClient::from_env()
            };
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
        CodeProvider::Codex => {
            return Err(CliError::internal(
                "codex provider reached unexpected TUI provider dispatch",
            ));
        }
    }

    Ok(())
}

struct ManagedCodexServer {
    ws_url: String,
    child: Child,
}

impl ManagedCodexServer {
    async fn shutdown(&mut self) {
        if self.child.id().is_none() {
            return;
        }
        let _ = self.child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(5), self.child.wait()).await;
    }
}

async fn execute_codex_mode(args: CodeArgs, working_dir: PathBuf) -> CliResult<()> {
    let mut server =
        start_managed_codex_server(&args.codex_bin, args.codex_port, &working_dir).await?;
    println!("Starting Libra Code with Codex provider");
    println!("Working directory: {}", working_dir.display());
    println!("Codex WebSocket: {}", server.ws_url);
    println!("Codex app-server: auto-started");
    if args.plan_mode {
        println!("Plan Mode: enabled (plan required before execution)");
    }

    let agent_args = agent_codex::AgentCodexArgs {
        url: server.ws_url.clone(),
        cwd: working_dir.to_string_lossy().to_string(),
        approval: approval_policy_to_codex(args.approval_policy).to_string(),
        model_provider: None,
        service_tier: None,
        personality: None,
        model: args.model.clone(),
        plan_mode: args.plan_mode,
        debug: false,
    };

    let result = agent_codex::execute(agent_args)
        .await
        .map_err(|e| CliError::fatal(e.to_string()));

    server.shutdown().await;
    result
}

async fn execute_claudecode_mode(args: CodeArgs, working_dir: PathBuf) -> CliResult<()> {
    println!("Starting Libra Code with Claude Code managed provider");
    println!("Working directory: {}", working_dir.display());
    if args.resume {
        println!("Claude Code session continuity: resume latest provider session in cwd");
    } else if let Some(resume_session) = &args.resume_session {
        println!("Claude Code session continuity: resume {}", resume_session);
    }

    let claudecode_args = build_claudecode_code_args(&args, working_dir);
    validate_claudecode_code_args(&claudecode_args)?;

    agent_claudecode::execute(claudecode_args)
        .await
        .map_err(map_claudecode_cli_error)
}

fn build_claudecode_code_args(
    args: &CodeArgs,
    working_dir: PathBuf,
) -> agent_claudecode::ClaudecodeCodeArgs {
    agent_claudecode::ClaudecodeCodeArgs {
        working_dir,
        model: args.model.clone(),
        python_binary: args.python_binary.clone(),
        helper_path: args.helper_path.clone(),
        timeout_seconds: args.timeout_seconds,
        interactive_approvals: approval_policy_enables_claudecode_interactive_approvals(
            args.approval_policy,
            args.permission_mode.as_deref(),
        ),
        permission_mode: Some(args.permission_mode.clone().unwrap_or_else(|| {
            approval_policy_to_claudecode_managed_permission_mode(args.approval_policy).to_string()
        })),
        continue_session: args.resume,
        resume: args.resume_session.clone(),
        fork_session: args.fork_session,
        session_id: args.session_id.clone(),
        resume_session_at: args.resume_at.clone(),
    }
}

fn validate_claudecode_code_args(
    args: &agent_claudecode::ClaudecodeCodeArgs,
) -> Result<(), CliError> {
    agent_claudecode::validate_code_args(args, &OutputConfig::default())
        .map_err(|error| CliError::command_usage(error.to_string()))
}

fn approval_policy_to_codex(policy: CodeApprovalPolicy) -> &'static str {
    match policy {
        CodeApprovalPolicy::Never => "accept",
        CodeApprovalPolicy::OnFailure
        | CodeApprovalPolicy::OnRequest
        | CodeApprovalPolicy::Untrusted => "ask",
    }
}

fn approval_policy_to_claudecode_managed_permission_mode(
    policy: CodeApprovalPolicy,
) -> &'static str {
    match policy {
        CodeApprovalPolicy::Never => "acceptEdits",
        CodeApprovalPolicy::OnFailure
        | CodeApprovalPolicy::OnRequest
        | CodeApprovalPolicy::Untrusted => "plan",
    }
}

fn approval_policy_enables_claudecode_interactive_approvals(
    policy: CodeApprovalPolicy,
    permission_mode_override: Option<&str>,
) -> bool {
    !matches!(policy, CodeApprovalPolicy::Never)
        && !matches!(permission_mode_override, Some("bypassPermissions"))
}

fn map_claudecode_cli_error(error: anyhow::Error) -> CliError {
    if agent_claudecode::is_auth_error(&error) {
        CliError::auth(error.to_string())
    } else {
        CliError::fatal(error.to_string())
    }
}

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

fn resolve_code_working_dir(args: &CodeArgs) -> CliResult<PathBuf> {
    let working_dir = args
        .cwd
        .clone()
        .unwrap_or_else(crate::utils::util::working_dir);
    if !working_dir.exists() {
        return Err(CliError::command_usage(format!(
            "--cwd path does not exist: {}",
            working_dir.display()
        )));
    }
    if !working_dir.is_dir() {
        return Err(CliError::command_usage(format!(
            "--cwd must point to a directory: {}",
            working_dir.display()
        )));
    }
    Ok(working_dir)
}

struct TuiLaunchConfig {
    host: String,
    port: u16,
    mcp_port: u16,
    registry: Arc<ToolRegistry>,
    preamble: String,
    temperature: Option<f64>,
    context: Option<CodeContext>,
    resume: bool,
    approval_policy: AskForApproval,
    user_input_rx:
        tokio::sync::mpsc::UnboundedReceiver<crate::internal::ai::tools::context::UserInputRequest>,
    user_input_tx:
        tokio::sync::mpsc::UnboundedSender<crate::internal::ai::tools::context::UserInputRequest>,
    exec_approval_rx: tokio::sync::mpsc::UnboundedReceiver<ExecApprovalRequest>,
    exec_approval_tx: tokio::sync::mpsc::UnboundedSender<ExecApprovalRequest>,
    mcp_server: Arc<LibraMcpServer>,
    managed_claudecode: Option<agent_claudecode::ClaudecodeTuiRuntime>,
}

#[derive(Clone, Debug, Default)]
struct UnsupportedCompletionModel;

impl crate::internal::ai::completion::CompletionModel for UnsupportedCompletionModel {
    type Response = serde_json::Value;

    async fn completion(
        &self,
        _request: crate::internal::ai::completion::CompletionRequest,
    ) -> Result<
        crate::internal::ai::completion::CompletionResponse<Self::Response>,
        crate::internal::ai::completion::CompletionError,
    > {
        Err(
            crate::internal::ai::completion::CompletionError::NotImplemented(
                "generic completion workflows are not available for the active managed provider"
                    .to_string(),
            ),
        )
    }
}

async fn run_tui_with_model<M>(
    model: M,
    params: TuiLaunchConfig,
    model_name: String,
    provider_name: String,
) -> CliResult<()>
where
    M: crate::internal::ai::completion::CompletionModel + Clone + 'static,
{
    let registry = params.registry;
    let hook_runner = {
        let runner = crate::internal::ai::hooks::HookRunner::load(registry.working_dir());
        if runner.has_hooks() {
            Some(std::sync::Arc::new(runner))
        } else {
            None
        }
    };

    let config = crate::internal::ai::agent::ToolLoopConfig {
        preamble: Some(params.preamble),
        temperature: params.temperature,
        hook_runner,
        allowed_tools: None,
        runtime_context: Some(default_tui_runtime_context(
            registry.working_dir(),
            params.context,
            params.approval_policy,
            params.exec_approval_tx.clone(),
        )),
        max_turns: None,
    };

    // Initialize terminal
    let terminal = match tui_init() {
        Ok(t) => t,
        Err(e) => return Err(CliError::io(format!("failed to initialize terminal: {e}"))),
    };

    // Ensure terminal is restored on exit
    let _guard = scopeguard::guard((), |_| {
        let _ = tui_restore();
    });

    let tui = Tui::new(terminal);

    let (web_handle, web_line) = match start_web_server(&params.host, params.port).await {
        Ok(handle) => {
            let line = format!("Web: http://{}", handle.addr);
            (Some(handle), line)
        }
        Err(err) => (None, format!("Web: failed to start ({err})")),
    };

    // Start MCP Server
    let (mcp_handle, mcp_line) =
        match start_mcp_server(&params.host, params.mcp_port, params.mcp_server.clone()).await {
            Ok(handle) => {
                let line = format!("MCP: http://{}", handle.addr);
                (Some(handle), line)
            }
            Err(err) => (None, format!("MCP: failed to start ({err})")),
        };

    let welcome = format!(
        "Welcome to Libra Code! Type your message and press Enter to chat with the AI assistant.\n{}\n{}",
        web_line, mcp_line
    );

    // Load slash commands
    let commands = crate::internal::ai::commands::load_commands(registry.working_dir());
    let command_dispatcher = crate::internal::ai::commands::CommandDispatcher::new(commands);

    // Load agent profiles
    let profiles = crate::internal::ai::agent::profile::load_profiles(registry.working_dir());
    let agent_router = crate::internal::ai::agent::profile::AgentProfileRouter::new(profiles);

    // Set up session persistence
    let working_dir_str = registry.working_dir().to_string_lossy().to_string();
    let storage_root = resolve_storage_root(registry.working_dir());
    let session_store =
        crate::internal::ai::session::SessionStore::from_storage_path(&storage_root);
    let session = if params.resume {
        match session_store.load_latest_for_working_dir(&working_dir_str) {
            Ok(Some(s)) => s,
            _ => crate::internal::ai::session::SessionState::new(&working_dir_str),
        }
    } else {
        crate::internal::ai::session::SessionState::new(&working_dir_str)
    };

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
            user_input_tx: params.user_input_tx,
            exec_approval_rx: params.exec_approval_rx,
            exec_approval_tx: params.exec_approval_tx,
            model_name,
            provider_name,
            mcp_server: Some(params.mcp_server),
            managed_claudecode: params.managed_claudecode,
        },
    );

    match app.run().await {
        Ok(exit_info) => {
            if let crate::internal::tui::ExitReason::Fatal(msg) = exit_info.reason {
                return Err(
                    CliError::fatal(msg).with_stable_code(StableErrorCode::InternalInvariant)
                );
            }
        }
        Err(e) => return Err(CliError::internal(format!("TUI exited unexpectedly: {e}"))),
    }

    if let Some(handle) = web_handle {
        handle.shutdown().await;
    }
    if let Some(handle) = mcp_handle {
        handle.shutdown().await;
    }

    Ok(())
}

use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    service::TowerToHyperService,
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpService, session::local::LocalSessionManager,
};

async fn start_mcp_server(
    host: &str,
    port: u16,
    mcp_server: Arc<LibraMcpServer>,
) -> anyhow::Result<McpServerHandle> {
    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;

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
        addr,
        shutdown_tx,
        join,
        connection_tasks,
    })
}

fn system_preamble(working_dir: &std::path::Path, context: Option<CodeContext>) -> String {
    let mut builder = crate::internal::ai::prompt::SystemPromptBuilder::new(working_dir);
    if let Some(ctx) = context {
        let mode = match ctx {
            CodeContext::Dev => crate::internal::ai::prompt::ContextMode::Dev,
            CodeContext::Review => crate::internal::ai::prompt::ContextMode::Review,
            CodeContext::Research => crate::internal::ai::prompt::ContextMode::Research,
        };
        builder = builder.with_context(mode);
    }
    builder.build()
}

fn default_tui_runtime_context(
    working_dir: &std::path::Path,
    context: Option<CodeContext>,
    approval_policy: AskForApproval,
    exec_approval_tx: tokio::sync::mpsc::UnboundedSender<ExecApprovalRequest>,
) -> ToolRuntimeContext {
    let policy = match context {
        Some(CodeContext::Review | CodeContext::Research) => SandboxPolicy::ReadOnly,
        Some(CodeContext::Dev) | None => SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![working_dir.to_path_buf()],
            network_access: false,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        },
    };

    ToolRuntimeContext {
        sandbox: Some(ToolSandboxContext {
            policy,
            permissions: SandboxPermissions::UseDefault,
        }),
        sandbox_runtime: None,
        approval: Some(ToolApprovalContext {
            policy: approval_policy,
            request_tx: exec_approval_tx,
            store: Arc::new(tokio::sync::Mutex::new(ApprovalStore::default())),
        }),
        max_output_bytes: None,
    }
}

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
        return Arc::new(LibraMcpServer::new(None, None));
    }

    // Connect to DB
    let db_path = dot_libra.join("libra.db");
    let Some(db_path_str) = db_path.to_str() else {
        eprintln!(
            "Warning: Database path is not valid UTF-8: {}. History disabled.",
            db_path.display()
        );
        return Arc::new(LibraMcpServer::new(None, None));
    };

    #[cfg(target_os = "windows")]
    let db_path_string = db_path_str.replace("\\", "/");
    #[cfg(target_os = "windows")]
    let db_path_str = &db_path_string;

    let db_conn = match crate::internal::db::establish_connection(db_path_str).await {
        Ok(conn) => Arc::new(conn),
        Err(e) => {
            eprintln!(
                "Warning: Failed to connect to database: {}. History disabled.",
                e
            );
            return Arc::new(LibraMcpServer::new(None, None));
        }
    };

    let storage = Arc::new(crate::utils::storage::local::LocalStorage::new(objects_dir));
    let intent_history_manager = Arc::new(HistoryManager::new(storage.clone(), dot_libra, db_conn));
    Arc::new(LibraMcpServer::new(
        Some(intent_history_manager),
        Some(storage),
    ))
}

fn resolve_storage_root(working_dir: &std::path::Path) -> std::path::PathBuf {
    // Use the resolved .libra storage directory for isolation, supporting
    // linked worktrees via try_get_storage_path.
    crate::utils::util::try_get_storage_path(Some(working_dir.to_path_buf()))
        .unwrap_or_else(|_| working_dir.join(".libra"))
}

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

fn validate_mode_args(args: &CodeArgs, output: &OutputConfig) -> Result<(), String> {
    if !args.stdio && args.port == args.mcp_port {
        return Err(format!(
            "--port ({}) and --mcp-port ({}) must be different",
            args.port, args.mcp_port
        ));
    }

    if args.web_only {
        reject_non_tui_flags(args, "--web")?;
    }

    if args.stdio {
        reject_non_tui_flags(args, "--stdio")?;
        reject_mode_flag(args.host != DEFAULT_BIND_HOST, "--host", "--stdio")?;
        reject_mode_flag(args.port != DEFAULT_WEB_PORT, "--port", "--stdio")?;
        reject_mode_flag(args.mcp_port != DEFAULT_MCP_PORT, "--mcp-port", "--stdio")?;
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

    if args.provider != CodeProvider::Claudecode && has_claudecode_managed_flags(args) {
        return Err(
            "Claude Code managed runtime flags are only supported with --provider=claudecode"
                .to_string(),
        );
    }

    if args.provider == CodeProvider::Codex && args.api_base.is_some() {
        return Err("--api-base is not supported with --provider=codex".to_string());
    }

    if args.provider == CodeProvider::Claudecode {
        reject_mode_flag(
            args.temperature.is_some(),
            "--temperature",
            "--provider=claudecode",
        )?;
        reject_mode_flag(args.context.is_some(), "--context", "--provider=claudecode")?;
        reject_mode_flag(
            args.api_base.is_some(),
            "--api-base",
            "--provider=claudecode",
        )?;
        if output.is_json() || output.quiet {
            return Err(
                "--json, --machine, and --quiet are not supported with --provider=claudecode"
                    .to_string(),
            );
        }
        validate_claudecode_managed_flags(args)?;
    }

    Ok(())
}

fn has_claudecode_managed_flags(args: &CodeArgs) -> bool {
    args.resume_session.is_some()
        || args.fork_session
        || args.session_id.is_some()
        || args.resume_at.is_some()
        || args.helper_path.is_some()
        || args.python_binary.is_some()
        || args.timeout_seconds.is_some()
        || args.permission_mode.is_some()
}

fn validate_claudecode_managed_flags(args: &CodeArgs) -> Result<(), String> {
    if args.resume && args.resume_session.is_some() {
        return Err("--resume cannot be combined with --resume-session".to_string());
    }
    if args.resume && args.fork_session {
        return Err("--fork-session requires --resume-session".to_string());
    }
    if args.resume && args.resume_at.is_some() {
        return Err("--resume-at requires --resume-session".to_string());
    }
    if args.resume && args.session_id.is_some() {
        return Err("--session-id requires --resume-session when resuming".to_string());
    }
    if args.resume_at.is_some() && args.resume_session.is_none() {
        return Err("--resume-at requires --resume-session".to_string());
    }
    if args.fork_session && args.resume_session.is_none() {
        return Err("--fork-session requires --resume-session".to_string());
    }
    if args.session_id.is_some() && args.resume_session.is_some() && !args.fork_session {
        return Err(
            "--session-id requires --fork-session when combined with --resume-session".to_string(),
        );
    }

    if let Some(permission_mode) = args.permission_mode.as_deref() {
        match permission_mode {
            "default" | "acceptEdits" | "plan" | "bypassPermissions" => {}
            _ => {
                return Err(format!(
                    "--permission-mode must be one of default, acceptEdits, plan, bypassPermissions (got {permission_mode})"
                ));
            }
        }
    }

    Ok(())
}

fn reject_mode_flag(is_invalid: bool, flag: &str, mode: &str) -> Result<(), String> {
    if is_invalid {
        return Err(format!("{flag} is not supported in {mode} mode"));
    }
    Ok(())
}

fn reject_non_tui_flags(args: &CodeArgs, mode: &str) -> Result<(), String> {
    reject_mode_flag(args.provider != CodeProvider::Gemini, "--provider", mode)?;
    reject_mode_flag(args.model.is_some(), "--model", mode)?;
    reject_mode_flag(args.temperature.is_some(), "--temperature", mode)?;
    reject_mode_flag(args.context.is_some(), "--context", mode)?;
    reject_mode_flag(args.resume, "--resume", mode)?;
    reject_mode_flag(args.resume_session.is_some(), "--resume-session", mode)?;
    reject_mode_flag(args.fork_session, "--fork-session", mode)?;
    reject_mode_flag(args.session_id.is_some(), "--session-id", mode)?;
    reject_mode_flag(args.resume_at.is_some(), "--resume-at", mode)?;
    reject_mode_flag(
        args.approval_policy != CodeApprovalPolicy::OnRequest,
        "--approval-policy",
        mode,
    )?;
    reject_mode_flag(args.api_base.is_some(), "--api-base", mode)?;
    reject_mode_flag(args.helper_path.is_some(), "--helper-path", mode)?;
    reject_mode_flag(args.python_binary.is_some(), "--python-binary", mode)?;
    reject_mode_flag(args.timeout_seconds.is_some(), "--timeout-seconds", mode)?;
    reject_mode_flag(args.permission_mode.is_some(), "--permission-mode", mode)?;
    Ok(())
}

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
            provider: CodeProvider::Gemini,
            model: None,
            temperature: None,
            context: None,
            resume: false,
            resume_session: None,
            fork_session: false,
            session_id: None,
            resume_at: None,
            approval_policy: CodeApprovalPolicy::OnRequest,
            mcp_port: DEFAULT_MCP_PORT,
            stdio: false,
            api_base: None,
            helper_path: None,
            python_binary: None,
            timeout_seconds: None,
            permission_mode: None,
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
    fn accepts_anthropic_provider_in_tui_mode() {
        let mut args = base_args();
        args.provider = CodeProvider::Anthropic;
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_ok());
    }

    #[test]
    fn maps_never_approval_to_accept_edits_for_claudecode_managed_runtime() {
        assert_eq!(
            approval_policy_to_claudecode_managed_permission_mode(CodeApprovalPolicy::Never),
            "acceptEdits"
        );
        assert_eq!(
            approval_policy_to_claudecode_managed_permission_mode(CodeApprovalPolicy::OnRequest),
            "plan"
        );
        assert_eq!(
            approval_policy_to_claudecode_managed_permission_mode(CodeApprovalPolicy::Untrusted),
            "plan"
        );
    }

    #[test]
    fn claudecode_interactive_approvals_follow_approval_policy() {
        assert!(!approval_policy_enables_claudecode_interactive_approvals(
            CodeApprovalPolicy::Never,
            None
        ));
        assert!(approval_policy_enables_claudecode_interactive_approvals(
            CodeApprovalPolicy::OnRequest,
            None
        ));
        assert!(!approval_policy_enables_claudecode_interactive_approvals(
            CodeApprovalPolicy::OnRequest,
            Some("bypassPermissions")
        ));
    }

    #[test]
    fn accepts_claudecode_provider_in_tui_mode() {
        let mut args = base_args();
        args.provider = CodeProvider::Claudecode;
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_ok());
    }

    #[test]
    fn rejects_resume_and_resume_session_together_for_claudecode() {
        let mut args = base_args();
        args.provider = CodeProvider::Claudecode;
        args.resume = true;
        args.resume_session = Some("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_string());
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_err());
    }

    #[test]
    fn rejects_resume_at_without_resume_session_for_claudecode() {
        let mut args = base_args();
        args.provider = CodeProvider::Claudecode;
        args.resume_at = Some("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb".to_string());
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_err());
    }

    #[test]
    fn rejects_fork_without_resume_session_for_claudecode() {
        let mut args = base_args();
        args.provider = CodeProvider::Claudecode;
        args.fork_session = true;
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_err());
    }

    #[test]
    fn rejects_session_id_without_fork_when_resuming_explicit_claudecode_session() {
        let mut args = base_args();
        args.provider = CodeProvider::Claudecode;
        args.resume_session = Some("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_string());
        args.session_id = Some("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb".to_string());
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_err());
    }

    #[test]
    fn rejects_claudecode_managed_flags_for_non_claudecode_provider() {
        let mut args = base_args();
        args.resume_session = Some("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_string());
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_err());
    }

    #[test]
    fn rejects_json_output_for_claudecode() {
        let mut args = base_args();
        args.provider = CodeProvider::Claudecode;
        let output = OutputConfig {
            json_format: Some(crate::utils::output::JsonFormat::Pretty),
            ..OutputConfig::default()
        };
        assert!(validate_mode_args(&args, &output).is_err());
    }

    #[test]
    fn rejects_quiet_output_for_claudecode() {
        let mut args = base_args();
        args.provider = CodeProvider::Claudecode;
        let output = OutputConfig {
            quiet: true,
            ..OutputConfig::default()
        };
        assert!(validate_mode_args(&args, &output).is_err());
    }

    #[test]
    fn invalid_claudecode_resume_session_is_reported_as_command_usage() {
        let mut args = base_args();
        args.provider = CodeProvider::Claudecode;
        args.resume_session = Some("not-a-uuid".to_string());

        let cli = validate_claudecode_code_args(&build_claudecode_code_args(
            &args,
            PathBuf::from("/tmp/libra-claudecode"),
        ))
        .expect_err("invalid resume UUID should be rejected");

        assert_eq!(cli.kind(), crate::utils::error::CliErrorKind::CommandUsage);
        assert_eq!(
            cli.stable_code(),
            crate::utils::error::StableErrorCode::CliInvalidArguments
        );
        assert!(
            cli.message().contains("--resume must be a valid UUID"),
            "unexpected usage message: {}",
            cli.message()
        );
    }

    #[test]
    fn default_tui_runtime_context_denies_network_in_dev_mode() {
        let (tx, _rx) = unbounded_channel();
        let runtime = default_tui_runtime_context(
            Path::new("/tmp/workspace"),
            Some(CodeContext::Dev),
            AskForApproval::OnRequest,
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
    fn default_tui_runtime_context_is_read_only_for_review_and_research() {
        for context in [CodeContext::Review, CodeContext::Research] {
            let (tx, _rx) = unbounded_channel();
            let runtime = default_tui_runtime_context(
                Path::new("/tmp/workspace"),
                Some(context),
                AskForApproval::OnRequest,
                tx,
            );

            let sandbox = runtime.sandbox.expect("sandbox context should be present");
            assert!(matches!(sandbox.policy, SandboxPolicy::ReadOnly));
        }
    }
}
