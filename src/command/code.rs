//! Code command for interactive coding sessions.
//!
//! Supports three modes:
//! - Default: Terminal UI (TUI) for interactive coding (and background web server)
//! - Web Mode (`--web`): Web server only, suitable for browser access or remote hosting.
//! - Stdio Mode (`--stdio`): MCP server over standard input/output, designed for integration with AI clients like Claude Desktop.

use std::{net::SocketAddr, sync::Arc};

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
        mcp::{resource::CreateIntentParams, server::LibraMcpServer},
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

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum CodeProvider {
    Gemini,
    Openai,
    Anthropic,
    Deepseek,
    Zhipu,
    Ollama,
}

impl CodeProvider {
    fn as_str(self) -> &'static str {
        match self {
            Self::Gemini => "gemini",
            Self::Openai => "openai",
            Self::Anthropic => "anthropic",
            Self::Deepseek => "deepseek",
            Self::Zhipu => "zhipu",
            Self::Ollama => "ollama",
        }
    }
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

    /// Port to listen on (MCP server)
    #[arg(long, default_value_t = 6789)]
    pub mcp_port: u16,

    /// Run the MCP server over Stdio (for Claude Desktop integration)
    #[arg(long, alias = "mcp-stdio", conflicts_with = "web_only")]
    pub stdio: bool,

    /// Provider API base URL (e.g. http://remote-host:11434/v1 for remote Ollama)
    #[arg(long)]
    pub api_base: Option<String>,
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
    Html(browse_page_html())
}

fn browse_page_html() -> &'static str {
    include_str!("code/browse_page.html")
}

fn browse_router() -> Router {
    Router::new()
        .route("/", get(root))
        .route("/browse-page.html", get(root))
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
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    let join = tokio::spawn(async move {
        axum::serve(listener, browse_router())
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
        structured_content: None,
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
            cli_error!(e, "error: failed to resolve actor");
            return;
        }
    };

    // Call MCP interface to create intent
    match mcp_server.create_intent_impl(params, actor).await {
        Ok(result) => {
            if !result.is_error.unwrap_or(false) {
                // Initial intent created successfully
            } else {
                eprintln!("error: failed to create initial intent");
            }
        }
        Err(e) => {
            cli_error!(e, "error: failed to create initial intent");
        }
    }
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
    let (mcp_handle, mcp_line) =
        match start_mcp_server(&args.host, args.mcp_port, mcp_server.clone()).await {
            Ok(handle) => {
                let line = format!("MCP: http://{}", handle.addr);
                (Some(handle), line)
            }
            Err(err) => (None, format!("MCP: failed to start ({err})")),
        };

    // Create initial intent via MCP
    create_initial_intent(&mcp_server).await;

    println!("{}", mcp_line);

    let _ = tokio::signal::ctrl_c().await;

    web_handle.shutdown().await;
    if let Some(handle) = mcp_handle {
        handle.shutdown().await;
    }
}

async fn execute_tui(args: CodeArgs) {
    // Use repository working directory to ensure correct initialization of .libra resources.
    let working_dir = crate::utils::util::working_dir();
    let CodeArgs {
        web_only: _,
        port,
        host,
        provider,
        model,
        temperature,
        context,
        resume,
        mcp_port,
        stdio: _,
        api_base,
    } = args;

    // Validate --api-base: only honored for Ollama via CLI flag. Other providers
    // accept custom base URLs through their respective environment variables.
    if api_base.is_some() && provider != CodeProvider::Ollama {
        eprintln!(
            "warning: --api-base is only honored for the ollama provider; \
             use provider-specific env vars (e.g. OPENAI_BASE_URL) for others; ignoring"
        );
    } else if let Some(ref base_url) = api_base {
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

    let preamble = system_preamble(&working_dir, context.as_deref());

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
    let bootstrap = TuiBootstrap {
        host,
        port,
        mcp_port,
        registry,
        preamble,
        temperature,
        resume,
        user_input_rx,
        mcp_server,
    };

    // Create agent based on provider
    match provider {
        CodeProvider::Gemini => {
            let client = match GeminiClient::from_env() {
                Ok(client) => client,
                Err(_) => {
                    eprintln!("error: GEMINI_API_KEY is not set");
                    return;
                }
            };
            let model_name = resolve_model_name(model, GEMINI_2_5_FLASH);
            let model = client.completion_model(&model_name);
            launch_tui_model(model, bootstrap, CodeProvider::Gemini, model_name).await;
        }
        CodeProvider::Openai => {
            let client = match OpenAIClient::from_env() {
                Ok(client) => client,
                Err(_) => {
                    eprintln!("error: OPENAI_API_KEY is not set");
                    return;
                }
            };
            let model_name = resolve_model_name(model, GPT_4O_MINI);
            let model = client.completion_model(&model_name);
            launch_tui_model(model, bootstrap, CodeProvider::Openai, model_name).await;
        }
        CodeProvider::Anthropic => {
            let client = match AnthropicClient::from_env() {
                Ok(client) => client,
                Err(_) => {
                    eprintln!("error: ANTHROPIC_API_KEY is not set");
                    return;
                }
            };
            let model_name = resolve_model_name(model, CLAUDE_3_5_SONNET);
            let model = client.completion_model(&model_name);
            launch_tui_model(model, bootstrap, CodeProvider::Anthropic, model_name).await;
        }
        CodeProvider::Deepseek => {
            let client = match DeepSeekClient::from_env() {
                Ok(client) => client,
                Err(_) => {
                    eprintln!("error: DEEPSEEK_API_KEY is not set");
                    return;
                }
            };
            let model_name = resolve_model_name(model, "deepseek-chat");
            let model = client.completion_model(&model_name);
            launch_tui_model(model, bootstrap, CodeProvider::Deepseek, model_name).await;
        }
        CodeProvider::Zhipu => {
            let client = match ZhipuClient::from_env() {
                Ok(client) => client,
                Err(_) => {
                    eprintln!("error: ZHIPU_API_KEY is not set");
                    return;
                }
            };
            let model_name = resolve_model_name(model, GLM_5);
            let model = client.completion_model(&model_name);
            launch_tui_model(model, bootstrap, CodeProvider::Zhipu, model_name).await;
        }
        CodeProvider::Ollama => {
            let client = if let Some(base_url) = &api_base {
                OllamaClient::with_base_url(base_url)
            } else {
                OllamaClient::from_env()
            };
            let model_name = match model {
                Some(m) => m,
                None => {
                    eprintln!(
                        "error: --model is required when using --provider ollama (e.g. --model llama3.2)"
                    );
                    return;
                }
            };
            let model = client.completion_model(&model_name);
            launch_tui_model(model, bootstrap, CodeProvider::Ollama, model_name).await;
        }
    }
}

struct TuiBootstrap {
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

impl TuiBootstrap {
    fn into_params(self, provider: CodeProvider, model_name: String) -> TuiParams {
        TuiParams {
            host: self.host,
            port: self.port,
            mcp_port: self.mcp_port,
            registry: self.registry,
            preamble: self.preamble,
            temperature: self.temperature,
            resume: self.resume,
            user_input_rx: self.user_input_rx,
            mcp_server: self.mcp_server,
            model_name,
            provider_name: provider.as_str().to_string(),
        }
    }
}

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
    mcp_server: Arc<LibraMcpServer>,
    model_name: String,
    provider_name: String,
}

fn resolve_model_name(model: Option<String>, default_model: &str) -> String {
    model.unwrap_or_else(|| default_model.to_string())
}

async fn launch_tui_model<M>(
    model: M,
    bootstrap: TuiBootstrap,
    provider: CodeProvider,
    model_name: String,
) where
    M: crate::internal::ai::completion::CompletionModel + 'static,
{
    run_tui_with_model(model, bootstrap.into_params(provider, model_name)).await;
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

    // Create initial intent via MCP
    create_initial_intent(&params.mcp_server).await;

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
                                    cli_error!(e, "warning: MCP connection error");
                                }
                            });
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
}

async fn init_mcp_server(working_dir: &std::path::Path) -> Arc<LibraMcpServer> {
    // Use the resolved .libra storage directory for isolation, supporting
    // linked worktrees via try_get_storage_path.
    let storage_dir = crate::utils::util::try_get_storage_path(Some(working_dir.to_path_buf()))
        .unwrap_or_else(|_| working_dir.join(".libra"));
    let (objects_dir, dot_libra) = (storage_dir.join("objects"), storage_dir);

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
    let db_path_string = db_path.to_string_lossy().into_owned();

    #[cfg(target_os = "windows")]
    let db_path_string = db_path_string.replace("\\", "/");
    #[cfg(target_os = "windows")]
    let db_path_str = &db_path_string;
    #[cfg(not(target_os = "windows"))]
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

async fn execute_stdio(_args: CodeArgs) {
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
            let result = running.waiting().await;
            if let Err(e) = result {
                eprintln!("MCP Stdio server error: {}", e);
            }
        }
        Err(e) => {
            cli_error!(e, "fatal: failed to start MCP Stdio server");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CodeProvider, browse_page_html, resolve_model_name};

    #[test]
    fn provider_names_match_cli_contract() {
        assert_eq!(CodeProvider::Gemini.as_str(), "gemini");
        assert_eq!(CodeProvider::Openai.as_str(), "openai");
        assert_eq!(CodeProvider::Anthropic.as_str(), "anthropic");
        assert_eq!(CodeProvider::Deepseek.as_str(), "deepseek");
        assert_eq!(CodeProvider::Zhipu.as_str(), "zhipu");
        assert_eq!(CodeProvider::Ollama.as_str(), "ollama");
    }

    #[test]
    fn resolve_model_name_prefers_user_override() {
        assert_eq!(
            resolve_model_name(Some("deepseek-reasoner".into()), "deepseek-chat"),
            "deepseek-reasoner"
        );
        assert_eq!(resolve_model_name(None, "deepseek-chat"), "deepseek-chat");
    }

    #[test]
    fn browse_page_is_embedded() {
        let page = browse_page_html();
        assert!(page.contains("Libra Browse"));
        assert!(page.contains("Ctrl + K to focus the AI bar"));
    }
}
