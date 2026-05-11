//! Wave 9 §5.13 foundation — minimal Codex app-server WebSocket
//! mock for runtime tests.
//!
//! The `libra code --provider codex` runtime opens a WebSocket
//! connection to the Codex app-server and speaks JSON-RPC. The
//! protocol surface is large (initialize, thread/start, turn/start,
//! many notifications), but the smallest meaningful contract is:
//!
//!   1. Accept a WebSocket handshake on `127.0.0.1:0`.
//!   2. Read one JSON-RPC request.
//!   3. Echo back a success response with the same `id`.
//!
//! This helper covers that minimum so a `tokio-tungstenite` client
//! (or `wait_for_codex_ready`-style probe) can connect and complete
//! a single round trip without an actual Codex binary.
//!
//! Future §5.13 work (full handshake + thread/start + notification
//! replay + persistence assertions) builds on this same helper by
//! extending the request matcher; the on-disk format is small enough
//! that this single file should remain the only mock infrastructure
//! for the entire §5.13 closure.

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle};
use tokio_tungstenite::tungstenite::Message;

/// Configurable behaviour for the mock server's responses.
#[derive(Clone, Default)]
pub struct MockCodexWsConfig {
    /// Optional canned thread id returned by `thread/start`. Defaults to
    /// `"libra-mock-thread"` when not set.
    pub thread_id: Option<String>,
}

/// Test-only WebSocket server mimicking the Codex app-server handshake.
///
/// Accepts WS connections on `127.0.0.1:0`. For each incoming JSON-RPC
/// request it captures the request body, then sends back a minimal
/// JSON-RPC success response keyed off the request `method`:
///
///   * `initialize` → `{ "jsonrpc": "2.0", "id": <id>, "result": {} }`
///   * `thread/start` → success with `result.thread.id = "<thread_id>"`
///   * any other method → empty `result: {}` success
///
/// Captured requests are exposed via `captured_requests()` so tests can
/// assert the libra→codex client emitted the expected payload shape.
pub struct MockCodexWsServer {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<Value>>>,
    handle: Option<JoinHandle<()>>,
}

impl MockCodexWsServer {
    /// Bind to `127.0.0.1:0`, start serving with the supplied config.
    pub async fn start(config: MockCodexWsConfig) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .context("bind mock codex ws server")?;
        let addr = listener
            .local_addr()
            .context("read mock codex ws address")?;
        let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
        let requests_for_task = requests.clone();
        let thread_id = config
            .thread_id
            .unwrap_or_else(|| "libra-mock-thread".to_string());
        let handle = tokio::spawn(async move {
            loop {
                let (tcp_stream, _peer) = match listener.accept().await {
                    Ok(pair) => pair,
                    Err(_) => break,
                };
                let requests = requests_for_task.clone();
                let thread_id = thread_id.clone();
                tokio::spawn(async move {
                    let ws_stream = match tokio_tungstenite::accept_async(tcp_stream).await {
                        Ok(stream) => stream,
                        Err(_) => return,
                    };
                    let (mut write, mut read) = ws_stream.split();
                    while let Some(message) = read.next().await {
                        let text = match message {
                            Ok(Message::Text(text)) => text.to_string(),
                            Ok(Message::Close(_)) => break,
                            Ok(_) => continue,
                            Err(_) => break,
                        };
                        let request: Value = match serde_json::from_str(&text) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };
                        if let Ok(mut guard) = requests.lock() {
                            guard.push(request.clone());
                        }
                        let id = request.get("id").cloned().unwrap_or(Value::Null);
                        let method = request
                            .get("method")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        let result = match method.as_str() {
                            "initialize" => json!({}),
                            "thread/start" => json!({ "thread": { "id": thread_id } }),
                            _ => json!({}),
                        };
                        let response = json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": result,
                        });
                        if write
                            .send(Message::Text(response.to_string().into()))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                });
            }
        });
        Ok(Self {
            addr,
            requests,
            handle: Some(handle),
        })
    }

    /// `ws://127.0.0.1:<port>` — pass to `connect_async` or to
    /// `libra code --provider codex --codex-port <port>`.
    pub fn ws_url(&self) -> String {
        format!("ws://{}", self.addr)
    }

    /// Local TCP port the mock is listening on.
    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// Snapshot of every JSON-RPC request body the mock has received
    /// (in arrival order, across all connections).
    pub fn captured_requests(&self) -> Vec<Value> {
        self.requests
            .lock()
            .expect("captured requests mutex poisoned")
            .clone()
    }
}

impl Drop for MockCodexWsServer {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}
