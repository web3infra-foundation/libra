//! Code command for interactive coding sessions.
//!
//! Supports two modes:
//! - Default: Terminal UI (TUI) for interactive coding (and background web server)
//! - With `--web-only` / `--web`: Web server only

use std::{net::SocketAddr, sync::Arc};

use axum::{Router, response::Html, routing::get};
use clap::{Parser, ValueEnum};
use tokio::sync::oneshot;

use crate::internal::{
    ai::{
        client::CompletionClient,
        providers::{
            anthropic::{CLAUDE_3_5_SONNET, Client as AnthropicClient},
            gemini::{Client as GeminiClient, GEMINI_2_5_FLASH},
            openai::{Client as OpenAIClient, GPT_4O_MINI},
        },
        tools::{
            ToolRegistry, ToolRegistryBuilder,
            handlers::{
                ApplyPatchHandler, GrepFilesHandler, ListDirHandler, ReadFileHandler,
                ShellHandler,
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

    if let Err(e) = axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
    {
        eprintln!("Server error: {}", e);
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

    let registry = Arc::new(
        ToolRegistryBuilder::with_working_dir(working_dir)
            .register("read_file", Arc::new(ReadFileHandler))
            .register("list_dir", Arc::new(ListDirHandler))
            .register("grep_files", Arc::new(GrepFilesHandler))
            .register("apply_patch", Arc::new(ApplyPatchHandler))
            .register("shell", Arc::new(ShellHandler))
            .build(),
    );

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
                args.host,
                args.port,
                model,
                registry.clone(),
                preamble,
                temperature,
                max_steps,
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
                args.host,
                args.port,
                model,
                registry.clone(),
                preamble,
                temperature,
                max_steps,
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
                args.host,
                args.port,
                model,
                registry.clone(),
                preamble,
                temperature,
                max_steps,
            )
            .await;
        }
    }
}

async fn run_tui_with_model<M>(
    host: String,
    port: u16,
    model: M,
    registry: Arc<ToolRegistry>,
    preamble: String,
    temperature: Option<f64>,
    max_steps: usize,
) where
    M: crate::internal::ai::completion::CompletionModel + 'static,
{
    let config = crate::internal::ai::agent::ToolLoopConfig {
        preamble: Some(preamble),
        temperature,
        max_steps,
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

    let (web_handle, web_line) = match start_web_server(&host, port).await {
        Ok(handle) => {
            let line = format!("Web: http://{}", handle.addr);
            (Some(handle), line)
        }
        Err(err) => (None, format!("Web: failed to start ({err})")),
    };

    let welcome = format!(
        "Welcome to Libra Code! Type your message and press Enter to chat with the AI assistant.\n{}",
        web_line
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
}

fn system_preamble(working_dir: &std::path::Path) -> String {
    format!(
        "You are a coding assistant. You help with programming tasks, code review, and file operations. \
Working directory: {}. \
Be concise and helpful in your responses.",
        working_dir.display()
    )
}
