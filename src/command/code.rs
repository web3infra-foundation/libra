//! Code command for interactive coding sessions.
//!
//! Supports two modes:
//! - Default: Terminal UI (TUI) for interactive coding (and background web server)
//! - With `--web-only` / `--web`: Web server only

use std::{net::SocketAddr, sync::Arc};

use axum::{Router, response::Html, routing::get};
use clap::{Parser, ValueEnum};
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::internal::{
    ai::{
        agent::ToolLoopConfig,
        client::CompletionClient,
        history::HistoryManager,
        mcp::server::LibraMcpServer,
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
                ApplyPatchHandler, GrepFilesHandler, ListDirHandler, McpBridgeHandler,
                ReadFileHandler,
            },
        },
    },
    tui::{App, Tui, tui_init, tui_restore},
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
    #[arg(long, alias = "web")]
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

    /// Maximum model/tool turns
    #[arg(long, default_value_t = 8)]
    pub max_steps: usize,

    /// Sampling temperature
    #[arg(long)]
    pub temperature: Option<f64>,

    /// Port to listen on (MCP server)
    #[arg(long, default_value_t = 6789)]
    pub mcp_port: u16,
}

pub async fn execute(args: CodeArgs) {
    if args.web_only {
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
        Ok(cwd) => cwd,
        Err(e) => {
            eprintln!("Failed to get current directory: {}", e);
            return;
        }
    };
    let repo_id = load_or_create_repo_id(&cwd);
    let storage = Arc::new(crate::utils::storage::local::LocalStorage::new(
        cwd.join(".libra").join("objects"),
    ));
    let intent_history_manager = Arc::new(HistoryManager::new(storage.clone(), cwd.join(".libra")));
    let mcp_server = Arc::new(LibraMcpServer::new(
        Some(intent_history_manager),
        Some(storage),
        repo_id,
    ));

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

    let preamble = system_preamble(&working_dir);
    let temperature = args.temperature;
    let max_steps = args.max_steps;

    // Prepare MCP server instance shared between the HTTP transport and TUI bridge
    let repo_id = load_or_create_repo_id(&working_dir);
    let storage = Arc::new(crate::utils::storage::local::LocalStorage::new(
        working_dir.join(".libra").join("objects"),
    ));
    let intent_history_manager = Arc::new(HistoryManager::new(
        storage.clone(),
        working_dir.join(".libra"),
    ));
    let mcp_server = Arc::new(LibraMcpServer::new(
        Some(intent_history_manager),
        Some(storage),
        repo_id,
    ));

    // Build registry: basic file tools + MCP workflow tools
    let mut builder = ToolRegistryBuilder::with_working_dir(working_dir)
        .register("read_file", Arc::new(ReadFileHandler))
        .register("list_dir", Arc::new(ListDirHandler))
        .register("grep_files", Arc::new(GrepFilesHandler))
        .register("apply_patch", Arc::new(ApplyPatchHandler));

    for (name, handler) in McpBridgeHandler::all_handlers(mcp_server.clone()) {
        builder = builder.register(name, handler);
    }

    let registry = Arc::new(builder.build());

    let config = ToolLoopConfig {
        preamble: Some(preamble),
        temperature,
        max_steps,
    };

    // Create agent based on provider
    let registry = registry.clone();
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
            let model_type = crate::internal::tui::ModelType::Gemini(model);
            run_tui_with_model(
                args.host,
                args.port,
                args.mcp_port,
                model_type,
                registry,
                config,
                mcp_server,
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
            let model_type = crate::internal::tui::ModelType::Openai(model);
            run_tui_with_model(
                args.host,
                args.port,
                args.mcp_port,
                model_type,
                registry,
                config,
                mcp_server,
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
            let model_type = crate::internal::tui::ModelType::Anthropic(model);
            run_tui_with_model(
                args.host,
                args.port,
                args.mcp_port,
                model_type,
                registry,
                config,
                mcp_server,
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
            // Fixed model: deepseek-chat
            let model = client.completion_model("deepseek-chat");
            let model_type = crate::internal::tui::ModelType::Deepseek(model);
            run_tui_with_model(
                args.host,
                args.port,
                args.mcp_port,
                model_type,
                registry,
                config,
                mcp_server,
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
            let model_type = crate::internal::tui::ModelType::Zhipu(model);
            run_tui_with_model(
                args.host,
                args.port,
                args.mcp_port,
                model_type,
                registry,
                config,
                mcp_server,
            )
            .await;
        }
    }
}

async fn run_tui_with_model(
    host: String,
    port: u16,
    mcp_port: u16,
    model: crate::internal::tui::ModelType,
    registry: Arc<ToolRegistry>,
    config: ToolLoopConfig,
    mcp_server: Arc<LibraMcpServer>,
) {
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

    let (web_handle, web_line) = match start_web_server(&host, port).await {
        Ok(handle) => {
            let line = format!("Web: http://{}", handle.addr);
            (Some(handle), line)
        }
        Err(err) => (None, format!("Web: failed to start ({err})")),
    };

    // Start MCP Server
    let (mcp_handle, mcp_line) = match start_mcp_server(&host, mcp_port, mcp_server).await {
        Ok(handle) => {
            let line = format!("MCP: http://{}", handle.addr);
            (Some(handle), line)
        }
        Err(err) => (None, format!("MCP: failed to start ({err})")),
    };

    let version = env!("CARGO_PKG_VERSION");
    let model_name = model.name();
    let _provider = model.provider(); // Unused variable, prefixed with underscore
    let current_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("~"));
    let current_dir_display = current_dir.display();

    let welcome = format!(
        "╭─────────────────────────────────────────────────────────────────────╮\n│ >_ Libra Codex (v{})                                             │\n│                                                                     │\n│ model:     {:39} /model to change │\n│ directory: {}                    │\n│                                                                     │\n│ Project: https://github.com/web3infra-foundation/libra              │\n╰─────────────────────────────────────────────────────────────────────╯\n\nWelcome to Libra Code! Type your message and press Enter to chat with the AI assistant.\n{}\n{}",
        version, model_name, current_dir_display, web_line, mcp_line
    );

    // Create and run app
    let mut app = App::new(tui, model, registry, config, welcome);

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

fn system_preamble(working_dir: &std::path::Path) -> String {
    format!(
        "You are a coding assistant. You help with programming tasks, code review, and file operations. \
Working directory: {}. \
Be concise and helpful in your responses.",
        working_dir.display()
    )
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
