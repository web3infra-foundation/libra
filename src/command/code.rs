//! Web-based code editor server command.
//! Starts an HTTP server providing a browser-accessible code editing interface.

use std::net::SocketAddr;

use axum::{response::Html, routing::get, Router};
use clap::Parser;
use tower_http::services::ServeDir;

#[derive(Parser, Debug)]
pub struct CodeArgs {
    /// Port to listen on
    #[arg(short, long, default_value = "3000")]
    pub port: u16,

    /// Host address to bind to
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,
}

pub async fn execute(args: CodeArgs) {
    let addr: SocketAddr = match format!("{}:{}", args.host, args.port).parse() {
        Ok(addr) => addr,
        Err(e) => {
            eprintln!("Invalid address: {}", e);
            return;
        }
    };

    let app = Router::new()
        .route("/", get(root))
        // Serve static files from current directory (for future assets)
        .nest_service("/static", ServeDir::new("."));

    println!("Libra Code server running at http://{}", addr);

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Failed to bind to {}: {}", addr, e);
            return;
        }
    };

    if let Err(e) = axum::serve(listener, app).await {
        eprintln!("Server error: {}", e);
    }
}

async fn root() -> Html<&'static str> {
    Html(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Libra Code</title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            display: flex;
            justify-content: center;
            align-items: center;
            height: 100vh;
            margin: 0;
            background: #1a1a2e;
            color: #eee;
        }
        h1 { font-size: 3rem; }
    </style>
</head>
<body>
    <h1>Hello, Libra Code!</h1>
</body>
</html>"#,
    )
}
