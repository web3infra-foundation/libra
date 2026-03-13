//! Code command for interactive coding sessions.
//!
//! Supports three modes:
//! - Default: Terminal UI (TUI) for interactive coding (and background web server)
//! - Web Mode (`--web`): Web server only, suitable for browser access or remote hosting.
//! - Stdio Mode (`--stdio`): MCP server over standard input/output, designed for integration with AI clients like Claude Desktop.

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use axum::{Router, response::Html, routing::get};
use clap::{Parser, ValueEnum};
use tokio::sync::oneshot;
use url::Url;

use crate::cli_error;
// use uuid::Uuid;
use crate::internal::{
    ai::{
        client::CompletionClient,
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

const DEFAULT_WEB_PORT: u16 = 3000;
const DEFAULT_MCP_PORT: u16 = 6789;
const DEFAULT_BIND_HOST: &str = "127.0.0.1";
const BROWSE_PAGE_HTML: &str = include_str!("code/index.html");

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum CodeProvider {
    Gemini,
    Openai,
    Anthropic,
    Deepseek,
    Zhipu,
    Ollama,
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

    /// Port to listen on (MCP server)
    #[arg(long, default_value_t = DEFAULT_MCP_PORT)]
    pub mcp_port: u16,

    /// Run the MCP server over Stdio (for Claude Desktop integration)
    #[arg(long, alias = "mcp-stdio", conflicts_with = "web_only")]
    pub stdio: bool,

    /// Provider API base URL (e.g. http://remote-host:11434/v1 for remote Ollama)
    #[arg(long)]
    pub api_base: Option<String>,
}

pub async fn execute(args: CodeArgs) {
    if let Err(err) = validate_mode_args(&args) {
        eprintln!("error: {err}");
        return;
    }
    if args.stdio {
        execute_stdio().await
    } else if args.web_only {
        execute_web_only(args).await
    } else {
        execute_tui(args).await
    }
}

async fn root() -> Html<&'static str> {
    Html(BROWSE_PAGE_HTML)
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

    let app = Router::new().route("/", get(root));
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

async fn execute_web_only(args: CodeArgs) {
    let web_handle = match start_web_server(&args.host, args.port).await {
        Ok(handle) => handle,
        Err(err) => {
            cli_error!(err, "fatal: failed to start web server");
            return;
        }
    };
    println!("Libra Code server running at http://{}", web_handle.addr);

    // Prepare MCP server instance shared between the HTTP transport and TUI bridge
    // Use repository working directory to ensure correct initialization of .libra resources.
    let working_dir = crate::utils::util::working_dir();

    let mcp_server = init_mcp_server(&working_dir).await;

    // Start MCP Server
    let mcp_handle = match start_mcp_server(&args.host, args.mcp_port, mcp_server.clone()).await {
        Ok(handle) => {
            println!("MCP: http://{}", handle.addr);
            handle
        }
        Err(err) => {
            cli_error!(err, "fatal: failed to start MCP server");
            web_handle.shutdown().await;
            return;
        }
    };

    let _ = tokio::signal::ctrl_c().await;
    web_handle.shutdown().await;
    mcp_handle.shutdown().await;
}

async fn execute_tui(args: CodeArgs) {
    // Use repository working directory to ensure correct initialization of .libra resources.
    let working_dir = crate::utils::util::working_dir();

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
                eprintln!(
                    "error: --api-base must use http or https (got {})",
                    u.scheme()
                );
                return;
            }
            Err(e) => {
                eprintln!("error: --api-base is not a valid URL: {e}");
                return;
            }
        }
    }

    let preamble = system_preamble(&working_dir, args.context);
    let temperature = args.temperature;
    let resume = args.resume;

    // Prepare MCP server instance shared between the HTTP transport and TUI bridge
    let mcp_server = init_mcp_server(&working_dir).await;

    // Create the bridge channel for request_user_input tool <-> TUI communication.
    let (user_input_tx, user_input_rx) = tokio::sync::mpsc::unbounded_channel::<
        crate::internal::ai::tools::context::UserInputRequest,
    >();

    // Build registry: basic file tools + MCP workflow tools
    let mut builder = ToolRegistryBuilder::with_working_dir(working_dir)
        .register("read_file", Arc::new(ReadFileHandler))
        .register("list_dir", Arc::new(ListDirHandler))
        .register("grep_files", Arc::new(GrepFilesHandler))
        .register("apply_patch", Arc::new(ApplyPatchHandler))
        .register("shell", Arc::new(ShellHandler))
        .register("update_plan", Arc::new(PlanHandler))
        .register("submit_intent_draft", Arc::new(SubmitIntentDraftHandler))
        .register(
            "request_user_input",
            Arc::new(RequestUserInputHandler::new(user_input_tx)),
        );

    for (name, handler) in McpBridgeHandler::all_handlers(mcp_server.clone()) {
        builder = builder.register(name, handler);
    }

    let registry = Arc::new(builder.build());

    let provider_name = format!("{:?}", args.provider).to_lowercase();
    let launch_config = TuiLaunchConfig {
        host: args.host,
        port: args.port,
        mcp_port: args.mcp_port,
        registry,
        preamble,
        temperature,
        resume,
        user_input_rx,
        mcp_server,
    };

    // Create agent based on provider
    match args.provider {
        CodeProvider::Gemini => {
            let client = match GeminiClient::from_env() {
                Ok(client) => client,
                Err(_) => {
                    eprintln!("error: GEMINI_API_KEY is not set");
                    return;
                }
            };
            let model_name = args.model.unwrap_or_else(|| GEMINI_2_5_FLASH.to_string());
            let model = client.completion_model(&model_name);
            run_tui_with_model(model, launch_config, model_name, provider_name).await;
        }
        CodeProvider::Openai => {
            let client = match OpenAIClient::from_env() {
                Ok(client) => client,
                Err(_) => {
                    eprintln!("error: OPENAI_API_KEY is not set");
                    return;
                }
            };
            let model_name = args.model.unwrap_or_else(|| GPT_4O_MINI.to_string());
            let model = client.completion_model(&model_name);
            run_tui_with_model(model, launch_config, model_name, provider_name).await;
        }
        CodeProvider::Anthropic => {
            let client = match AnthropicClient::from_env() {
                Ok(client) => client,
                Err(_) => {
                    eprintln!("error: ANTHROPIC_API_KEY is not set");
                    return;
                }
            };
            let model_name = args.model.unwrap_or_else(|| CLAUDE_3_5_SONNET.to_string());
            let model = client.completion_model(&model_name);
            run_tui_with_model(model, launch_config, model_name, provider_name).await;
        }
        CodeProvider::Deepseek => {
            let client = match DeepSeekClient::from_env() {
                Ok(client) => client,
                Err(_) => {
                    eprintln!("error: DEEPSEEK_API_KEY is not set");
                    return;
                }
            };
            let model_name = args.model.unwrap_or_else(|| "deepseek-chat".to_string());
            let model = client.completion_model(&model_name);
            run_tui_with_model(model, launch_config, model_name, provider_name).await;
        }
        CodeProvider::Zhipu => {
            let client = match ZhipuClient::from_env() {
                Ok(client) => client,
                Err(_) => {
                    eprintln!("error: ZHIPU_API_KEY is not set");
                    return;
                }
            };
            let model_name = args.model.unwrap_or_else(|| GLM_5.to_string());
            let model = client.completion_model(&model_name);
            run_tui_with_model(model, launch_config, model_name, provider_name).await;
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
                    eprintln!(
                        "error: --model is required when using --provider ollama (e.g. --model llama3.2)"
                    );
                    return;
                }
            };
            let model = client.completion_model(&model_name);
            run_tui_with_model(model, launch_config, model_name, provider_name).await;
        }
    }
}

struct TuiLaunchConfig {
    host: String,
    port: u16,
    mcp_port: u16,
    registry: Arc<ToolRegistry>,
    preamble: String,
    temperature: Option<f64>,
    resume: bool,
    user_input_rx:
        tokio::sync::mpsc::UnboundedReceiver<crate::internal::ai::tools::context::UserInputRequest>,
    mcp_server: Arc<LibraMcpServer>,
}

async fn run_tui_with_model<M>(
    model: M,
    params: TuiLaunchConfig,
    model_name: String,
    provider_name: String,
) where
    M: crate::internal::ai::completion::CompletionModel + 'static,
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
    };

    // Initialize terminal
    let terminal = match tui_init() {
        Ok(t) => t,
        Err(e) => {
            cli_error!(e, "fatal: failed to initialize terminal");
            return;
        }
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
        match session_store.load_latest() {
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
            model_name,
            provider_name,
            mcp_server: Some(params.mcp_server),
        },
    );

    match app.run().await {
        Ok(exit_info) => {
            if let crate::internal::tui::ExitReason::Fatal(msg) = exit_info.reason {
                eprintln!("fatal: {}", msg);
            }
        }
        Err(e) => {
            cli_error!(e, "fatal: TUI exited unexpectedly");
        }
    }

    if let Some(handle) = web_handle {
        handle.shutdown().await;
    }
    if let Some(handle) = mcp_handle {
        handle.shutdown().await;
    }
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

async fn execute_stdio() {
    // Use repository working directory to ensure correct initialization of .libra resources.
    let working_dir = crate::utils::util::working_dir();

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
                eprintln!("MCP Stdio server error: {}", e);
            }
        }
        Err(e) => {
            cli_error!(e, "fatal: failed to start MCP Stdio server");
        }
    }
}

fn validate_mode_args(args: &CodeArgs) -> Result<(), String> {
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
    reject_mode_flag(args.api_base.is_some(), "--api-base", mode)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_args() -> CodeArgs {
        CodeArgs {
            web_only: false,
            port: DEFAULT_WEB_PORT,
            host: DEFAULT_BIND_HOST.to_string(),
            provider: CodeProvider::Gemini,
            model: None,
            temperature: None,
            context: None,
            resume: false,
            mcp_port: DEFAULT_MCP_PORT,
            stdio: false,
            api_base: None,
        }
    }

    #[test]
    fn rejects_same_web_and_mcp_ports() {
        let mut args = base_args();
        args.mcp_port = args.port;
        assert!(validate_mode_args(&args).is_err());
    }

    #[test]
    fn rejects_tui_flags_in_web_mode() {
        let mut args = base_args();
        args.web_only = true;
        args.model = Some("foo".to_string());
        assert!(validate_mode_args(&args).is_err());
    }

    #[test]
    fn rejects_web_flags_in_stdio_mode() {
        let mut args = base_args();
        args.stdio = true;
        args.host = "0.0.0.0".to_string();
        assert!(validate_mode_args(&args).is_err());
    }

    #[test]
    fn accepts_default_tui_mode() {
        let args = base_args();
        assert!(validate_mode_args(&args).is_ok());
    }
}
