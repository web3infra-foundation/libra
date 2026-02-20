//! Code command for interactive coding sessions.
//!
//! Supports three modes:
//! - Default: Terminal UI (TUI) for interactive coding (and background web server)
//! - Web Mode (`--web`): Web server only, suitable for browser access or remote hosting.
//! - Stdio Mode (`--stdio`): MCP server over standard input/output, designed for integration with AI clients like Claude Desktop.

use std::{net::SocketAddr, sync::Arc};
use std::{net::SocketAddr, sync::Arc};

use axum::{Router, response::Html, routing::get};
use clap::{Parser, ValueEnum};
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::internal::{
    ai::{
        client::CompletionClient,
        history::HistoryManager,
        mcp::{resource::CreateIntentParams, server::LibraMcpServer},
        mcp::{resource::CreateIntentParams, server::LibraMcpServer},
        providers::{
            anthropic::{CLAUDE_3_5_SONNET, Client as AnthropicClient},
            deepseek::client::Client as DeepSeekClient,
            gemini::{Client as GeminiClient, GEMINI_2_5_FLASH},
            openai::{Client as OpenAIClient, GPT_4O_MINI},
            zhipu::{Client as ZhipuClient, GLM_5},
        },
        tools::{
            ToolRegistry, ToolRegistryBuilder,
            handlers::{
                ApplyPatchHandler, GrepFilesHandler, ListDirHandler, McpBridgeHandler, PlanHandler,
                ReadFileHandler, RequestUserInputHandler, ShellHandler,
                ApplyPatchHandler, GrepFilesHandler, ListDirHandler, McpBridgeHandler, PlanHandler,
                ReadFileHandler, RequestUserInputHandler, ShellHandler,
            },
        },
    },
    tui::{App, AppConfig, Tui, tui_init, tui_restore},
    tui::{App, AppConfig, Tui, tui_init, tui_restore},
};

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum CodeProvider {
    Gemini,
    Openai,
    Anthropic,
    Deepseek,
    Zhipu,
}

#[derive(Parser, Debug)]
pub struct CodeArgs {
    /// Run the web server only (no TUI). Alias: `--web`.
    #[arg(long, alias = "web", conflicts_with = "stdio")]
    pub web_only: bool,

    /// Port to listen on (web server)
    #[arg(short, long, default_value = "3000")]
    pub port: u16,

    /// Host address to bind to (web server)
    #[arg(long, default_value = "127.0.0.1")]
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
    #[arg(long)]
    pub context: Option<String>,

    /// Resume the most recent session
    #[arg(long)]
    pub resume: bool,

    /// Operating context mode (dev, review, research)
    #[arg(long)]
    pub context: Option<String>,

    /// Resume the most recent session
    #[arg(long)]
    pub resume: bool,

    /// Port to listen on (MCP server)
    #[arg(long, default_value_t = 6789)]
    pub mcp_port: u16,

    /// Run the MCP server over Stdio (for Claude Desktop integration)
    #[arg(long, alias = "mcp-stdio", conflicts_with = "web_only")]
    pub stdio: bool,
}

pub async fn execute(args: CodeArgs) {
    if args.stdio {
        execute_stdio(args).await
    } else if args.web_only {
        execute_web_only(args).await
    } else {
        execute_tui(args).await
    }
}

async fn root() -> Html<&'static str> {
    Html(
        r#"<!DOCTYPE html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>Libra Code</title>
  </head>
  <body>
    <h1>Hello, Libra Code!</h1>
  </body>
</html>"#,
    )
}

struct WebHandle {
    addr: SocketAddr,
    shutdown_tx: oneshot::Sender<()>,
    join: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl WebHandle {
    async fn shutdown(self) {
        let _ = self.shutdown_tx.send(());
        let _ = self.join.await;
    }
}

async fn start_web_server(host: &str, port: u16) -> anyhow::Result<WebHandle> {
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

    Ok(WebHandle {
        addr,
        shutdown_tx,
        join,
    })
}

/// MCP write helper: create initial intent
async fn create_initial_intent(mcp_server: &Arc<LibraMcpServer>) {
    let params = CreateIntentParams {
        content: "Libra Code session started".to_string(),
        parent_id: None,
        status: Some("active".to_string()),
        task_id: None,
        commit_sha: None,
        actor_kind: Some("system".to_string()),
        actor_id: Some("libra-code".to_string()),
    };

    // Resolve actor
    let actor = match mcp_server
        .resolve_actor_from_params(params.actor_kind.as_deref(), params.actor_id.as_deref())
    {
        Ok(actor) => actor,
        Err(e) => {
            eprintln!("Failed to resolve actor: {:?}", e);
            return;
        }
    };

    // Call MCP interface to create intent
    match mcp_server.create_intent_impl(params, actor).await {
        Ok(result) => {
            if !result.is_error.unwrap_or(false) {
                // Initial intent created successfully
            } else {
                eprintln!("Failed to create initial intent: {:?}", result.content);
            }
        }
        Err(e) => {
            eprintln!("Error creating initial intent: {:?}", e);
        }
    }
}

/// MCP write helper: create initial intent
async fn create_initial_intent(mcp_server: &Arc<LibraMcpServer>) {
    let params = CreateIntentParams {
        content: "Libra Code session started".to_string(),
        parent_id: None,
        status: Some("active".to_string()),
        task_id: None,
        commit_sha: None,
        actor_kind: Some("system".to_string()),
        actor_id: Some("libra-code".to_string()),
    };

    // Resolve actor
    let actor = match mcp_server
        .resolve_actor_from_params(params.actor_kind.as_deref(), params.actor_id.as_deref())
    {
        Ok(actor) => actor,
        Err(e) => {
            eprintln!("Failed to resolve actor: {:?}", e);
            return;
        }
    };

    // Call MCP interface to create intent
    match mcp_server.create_intent_impl(params, actor).await {
        Ok(result) => {
            if !result.is_error.unwrap_or(false) {
                // Initial intent created successfully
            } else {
                eprintln!("Failed to create initial intent: {:?}", result.content);
            }
        }
        Err(e) => {
            eprintln!("Error creating initial intent: {:?}", e);
        }
    }
}

async fn execute_web_only(args: CodeArgs) {
    let addr: SocketAddr = match format!("{}:{}", args.host, args.port).parse() {
        Ok(addr) => addr,
        Err(e) => {
            eprintln!("Invalid address: {}", e);
            return;
        }
    };

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Failed to bind to {}: {}", addr, e);
            return;
        }
    };

    let app = Router::new().route("/", get(root));
    println!("Libra Code server running at http://{}", addr);

    // Prepare MCP server instance shared between the HTTP transport and TUI bridge
    let cwd = match std::env::current_dir() {
        Ok(path) => path,
        Ok(path) => path,
        Err(e) => {
            eprintln!("Failed to get current directory: {}", e);
            return;
        }
    };

    let mcp_server = init_mcp_server(&cwd, false);

    // Start MCP Server
    let (mcp_handle, mcp_line) = match start_mcp_server(&args.host, args.mcp_port, mcp_server).await
    {
        Ok(handle) => {
            let line = format!("MCP: http://{}", handle.addr);
            (Some(handle), line)
        }
        Err(err) => (None, format!("MCP: failed to start ({err})")),
    };
    println!("{}", mcp_line);

    if let Err(e) = axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
    {
        eprintln!("Server error: {}", e);
    }

    if let Some(handle) = mcp_handle {
        handle.shutdown().await;
    }
}

async fn execute_tui(args: CodeArgs) {
    let working_dir = match std::env::current_dir() {
        Ok(path) => path,
        Err(err) => {
            eprintln!("error: failed to get current working directory: {}", err);
            return;
        }
    };

    let preamble = system_preamble(&working_dir, args.context.as_deref());
    let preamble = system_preamble(&working_dir, args.context.as_deref());
    let temperature = args.temperature;
    let resume = args.resume;
    let resume = args.resume;

    // Prepare MCP server instance shared between the HTTP transport and TUI bridge
    let mcp_server = init_mcp_server(&working_dir, false);

    // Create the bridge channel for request_user_input tool <-> TUI communication.
    let (user_input_tx, user_input_rx) = tokio::sync::mpsc::unbounded_channel::<
        crate::internal::ai::tools::context::UserInputRequest,
    >();

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
        .register(
            "request_user_input",
            Arc::new(RequestUserInputHandler::new(user_input_tx)),
        );
        .register("apply_patch", Arc::new(ApplyPatchHandler))
        .register("shell", Arc::new(ShellHandler))
        .register("update_plan", Arc::new(PlanHandler))
        .register(
            "request_user_input",
            Arc::new(RequestUserInputHandler::new(user_input_tx)),
        );

    for (name, handler) in McpBridgeHandler::all_handlers(mcp_server.clone()) {
        builder = builder.register(name, handler);
    }

    let registry = Arc::new(builder.build());

    // Resolve model name before entering the provider match
    let provider_name = format!("{:?}", args.provider).to_lowercase();
    // Resolve model name before entering the provider match
    let provider_name = format!("{:?}", args.provider).to_lowercase();

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
            run_tui_with_model(
                model,
                TuiParams {
                    host: args.host,
                    port: args.port,
                    mcp_port: args.mcp_port,
                    registry: registry.clone(),
                    preamble,
                    temperature,
                    resume,
                    user_input_rx,
                    mcp_server,
                    model_name,
                    provider_name,
                },
                model,
                TuiParams {
                    host: args.host,
                    port: args.port,
                    mcp_port: args.mcp_port,
                    registry: registry.clone(),
                    preamble,
                    temperature,
                    resume,
                    user_input_rx,
                    mcp_server,
                    model_name,
                    provider_name,
                },
            )
            .await;
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
            run_tui_with_model(
                model,
                TuiParams {
                    host: args.host,
                    port: args.port,
                    mcp_port: args.mcp_port,
                    registry: registry.clone(),
                    preamble,
                    temperature,
                    resume,
                    user_input_rx,
                    mcp_server,
                    model_name,
                    provider_name,
                },
                model,
                TuiParams {
                    host: args.host,
                    port: args.port,
                    mcp_port: args.mcp_port,
                    registry: registry.clone(),
                    preamble,
                    temperature,
                    resume,
                    user_input_rx,
                    mcp_server,
                    model_name,
                    provider_name,
                },
            )
            .await;
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
            run_tui_with_model(
                model,
                TuiParams {
                    host: args.host,
                    port: args.port,
                    mcp_port: args.mcp_port,
                    registry: registry.clone(),
                    preamble,
                    temperature,
                    resume,
                    user_input_rx,
                    mcp_server,
                    model_name,
                    provider_name,
                },
                model,
                TuiParams {
                    host: args.host,
                    port: args.port,
                    mcp_port: args.mcp_port,
                    registry: registry.clone(),
                    preamble,
                    temperature,
                    resume,
                    user_input_rx,
                    mcp_server,
                    model_name,
                    provider_name,
                },
            )
            .await;
        }
        CodeProvider::Deepseek => {
            let client = match DeepSeekClient::from_env() {
                Ok(client) => client,
                Err(_) => {
                    eprintln!("error: DEEPSEEK_API_KEY is not set");
                    return;
                }
            };
            let model_name = "deepseek-chat".to_string();
            let model = client.completion_model(&model_name);
            let model_name = "deepseek-chat".to_string();
            let model = client.completion_model(&model_name);
            run_tui_with_model(
                model,
                TuiParams {
                    host: args.host,
                    port: args.port,
                    mcp_port: args.mcp_port,
                    registry: registry.clone(),
                    preamble,
                    temperature,
                    resume,
                    user_input_rx,
                    mcp_server,
                    model_name,
                    provider_name,
                },
                model,
                TuiParams {
                    host: args.host,
                    port: args.port,
                    mcp_port: args.mcp_port,
                    registry: registry.clone(),
                    preamble,
                    temperature,
                    resume,
                    user_input_rx,
                    mcp_server,
                    model_name,
                    provider_name,
                },
            )
            .await;
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
            run_tui_with_model(
                model,
                TuiParams {
                    host: args.host,
                    port: args.port,
                    mcp_port: args.mcp_port,
                    registry: registry.clone(),
                    preamble,
                    temperature,
                    resume,
                    user_input_rx,
                    mcp_server,
                    model_name,
                    provider_name,
                },
                model,
                TuiParams {
                    host: args.host,
                    port: args.port,
                    mcp_port: args.mcp_port,
                    registry: registry.clone(),
                    preamble,
                    temperature,
                    resume,
                    user_input_rx,
                    mcp_server,
                    model_name,
                    provider_name,
                },
            )
            .await;
        }
    }
}

struct TuiParams {
struct TuiParams {
    host: String,
    port: u16,
    mcp_port: u16,
    registry: Arc<ToolRegistry>,
    preamble: String,
    temperature: Option<f64>,
    resume: bool,
    user_input_rx:
        tokio::sync::mpsc::UnboundedReceiver<crate::internal::ai::tools::context::UserInputRequest>,
    preamble: String,
    temperature: Option<f64>,
    resume: bool,
    user_input_rx:
        tokio::sync::mpsc::UnboundedReceiver<crate::internal::ai::tools::context::UserInputRequest>,
    mcp_server: Arc<LibraMcpServer>,
    model_name: String,
    provider_name: String,
}

async fn run_tui_with_model<M>(model: M, params: TuiParams)
where
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
        max_steps: None, // TUI mode: unlimited tool steps
        hook_runner,
        allowed_tools: None,
    };

    model_name: String,
    provider_name: String,
}

async fn run_tui_with_model<M>(model: M, params: TuiParams)
where
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
        max_steps: None, // TUI mode: unlimited tool steps
        hook_runner,
        allowed_tools: None,
    };

    // Initialize terminal
    let terminal = match tui_init() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("Failed to initialize terminal: {}", e);
            return;
        }
    };

    // Ensure terminal is restored on exit
    let _guard = scopeguard::guard((), |_| {
        let _ = tui_restore();
    });

    let tui = Tui::new(terminal);

    let (web_handle, web_line) = match start_web_server(&params.host, params.port).await {
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

    // Create initial intent via MCP
    create_initial_intent(&params.mcp_server).await;

    let welcome = format!(
        "Welcome to Libra Code! Type your message and press Enter to chat with the AI assistant.\n{}\n{}",
        web_line, mcp_line
    );

    // Load slash commands
    let commands = crate::internal::ai::commands::load_commands(registry.working_dir());
    let command_dispatcher = crate::internal::ai::commands::CommandDispatcher::new(commands);

    // Load agent definitions
    let agents = crate::internal::ai::agents::load_agents(registry.working_dir());
    let agent_router = crate::internal::ai::agents::AgentRouter::new(agents);

    // Set up session persistence
    let working_dir_str = registry.working_dir().to_string_lossy().to_string();
    let session_store = crate::internal::ai::session::SessionStore::new(registry.working_dir());
    let session = if params.resume {
        match session_store.load_latest() {
            Ok(Some(s)) => s,
            _ => crate::internal::ai::session::SessionState::new(&working_dir_str),
        }
    } else {
        crate::internal::ai::session::SessionState::new(&working_dir_str)
    };
        "Welcome to Libra Code! Type your message and press Enter to chat with the AI assistant.\n{}\n{}",
        web_line, mcp_line
    );

    // Load slash commands
    let commands = crate::internal::ai::commands::load_commands(registry.working_dir());
    let command_dispatcher = crate::internal::ai::commands::CommandDispatcher::new(commands);

    // Load agent definitions
    let agents = crate::internal::ai::agents::load_agents(registry.working_dir());
    let agent_router = crate::internal::ai::agents::AgentRouter::new(agents);

    // Set up session persistence
    let working_dir_str = registry.working_dir().to_string_lossy().to_string();
    let session_store = crate::internal::ai::session::SessionStore::new(registry.working_dir());
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
            model_name: params.model_name,
            provider_name: params.provider_name,
            mcp_server: Some(params.mcp_server),
        },
    );

    match app.run().await {
        Ok(exit_info) => {
            if let crate::internal::tui::ExitReason::Fatal(msg) = exit_info.reason {
                eprintln!("Fatal error: {}", msg);
            }
        }
        Err(e) => {
            eprintln!("Error running TUI: {}", e);
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
) -> anyhow::Result<WebHandle> {
    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;

    // Use rmcp's Streamable HTTP transport via Hyper directly
    let service = TowerToHyperService::new(StreamableHttpService::new(
        move || Ok(mcp_server.clone()),
        LocalSessionManager::default().into(),
        Default::default(),
    ));

    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();

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
                            tokio::spawn(async move {
                                if let Err(e) = hyper_util::server::conn::auto::Builder::new(TokioExecutor::default())
                                    .serve_connection(io, service)
                                    .await
                                {
                                    eprintln!("MCP connection error: {:?}", e);
                                }
                            });
                        }
                        Err(e) => {
                            eprintln!("MCP accept error: {:?}", e);
                        }
                    }
                }
            }
        }
        Ok(())
    });

    Ok(WebHandle {
        addr,
        shutdown_tx,
        join,
    })
}

fn system_preamble(working_dir: &std::path::Path, context: Option<&str>) -> String {
    let mut builder = crate::internal::ai::prompt::SystemPromptBuilder::new(working_dir);
    if let Some(ctx_str) = context {
        if let Ok(mode) = ctx_str.parse::<crate::internal::ai::prompt::ContextMode>() {
            builder = builder.with_context(mode);
        } else {
            tracing::warn!(context = ctx_str, "unknown context mode, ignoring");
        }
    }
    builder.build()
fn system_preamble(working_dir: &std::path::Path, context: Option<&str>) -> String {
    let mut builder = crate::internal::ai::prompt::SystemPromptBuilder::new(working_dir);
    if let Some(ctx_str) = context {
        if let Ok(mode) = ctx_str.parse::<crate::internal::ai::prompt::ContextMode>() {
            builder = builder.with_context(mode);
        } else {
            tracing::warn!(context = ctx_str, "unknown context mode, ignoring");
        }
    }
    builder.build()
}

/// Load the repo UUID from `.libra/repo_id`, or create one if not present.
fn load_or_create_repo_id(working_dir: &std::path::Path) -> Uuid {
    let repo_id_path = working_dir.join(".libra").join("repo_id");
    if let Ok(content) = std::fs::read_to_string(&repo_id_path)
        && let Ok(id) = content.trim().parse::<Uuid>()
    {
        return id;
    }
    let id = Uuid::new_v4();
    // Best-effort persist; ignore errors (e.g. .libra dir missing)
    let _ = std::fs::create_dir_all(repo_id_path.parent().unwrap());
    let _ = std::fs::write(&repo_id_path, id.to_string());
    id
}

fn init_mcp_server(working_dir: &std::path::Path, is_stdio: bool) -> Arc<LibraMcpServer> {
    // Determine storage paths based on mode
    let (objects_dir, dot_libra, repo_id) = if is_stdio {
        // Stdio mode (e.g. Claude Desktop): Use ~/.libra/mcp/<repo_id>/ per-repo namespace
        // to avoid sandbox permission issues and isolate concurrent sessions.
        let repo_id = load_or_create_repo_id(working_dir);
        let home_dir = dirs::home_dir().unwrap_or_else(std::env::temp_dir);
        let libra_home = home_dir.join(".libra");
        let mcp_root = libra_home.join("mcp").join(repo_id.to_string());
        (mcp_root.join("objects"), mcp_root, repo_id)
    } else {
        // TUI/Web mode: Use the resolved .libra storage directory for isolation,
        // supporting linked worktrees via try_get_storage_path.
        let storage_dir = crate::utils::util::try_get_storage_path(Some(working_dir.to_path_buf()))
            .unwrap_or_else(|_| working_dir.join(".libra"));
        let repo_id = load_or_create_repo_id(working_dir);
        (storage_dir.join("objects"), storage_dir, repo_id)
    };

    // Try to create the directory. If it fails, we assume read-only or permission issues.
    if let Err(e) = std::fs::create_dir_all(&objects_dir) {
        eprintln!(
            "Warning: Failed to create storage directory: {}. Running in read-only mode (history/context disabled). Error: {}",
            objects_dir.display(),
            e
        );
        return Arc::new(LibraMcpServer::new(None, None, repo_id));
    }

    let storage = Arc::new(crate::utils::storage::local::LocalStorage::new(objects_dir));
    let intent_history_manager = Arc::new(HistoryManager::new(storage.clone(), dot_libra));
    Arc::new(LibraMcpServer::new(
        Some(intent_history_manager),
        Some(storage),
        repo_id,
    ))
}

async fn execute_stdio(_args: CodeArgs) {
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => {
            eprintln!("Failed to get current directory: {}", e);
            return;
        }
    };

    let mcp_server = init_mcp_server(&cwd, true);

    use rmcp::{
        service::serve_server,
        transport::{async_rw::AsyncRwTransport, io::stdio},
    };

    let (stdin, stdout) = stdio();
    let transport = AsyncRwTransport::new_server(stdin, stdout);

    match serve_server(mcp_server, transport).await {
        Ok(running) => {
            let result = running.waiting().await;
            if let Err(e) = result {
                eprintln!("MCP Stdio server error: {}", e);
            }
        }
        Err(e) => {
            eprintln!("Failed to start MCP Stdio server: {}", e);
        }
    }
}
