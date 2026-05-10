//! Mock OpenAI-compatible HTTP server for provider boot smoke
//! tests (Wave 10 / §5.2).
//!
//! Spawns a tiny `axum` router on `127.0.0.1:0` that:
//!   * Accepts `POST /chat/completions` (OpenAI-compatible
//!     providers: OpenAI, DeepSeek, Kimi, Zhipu),
//!     `POST /v1/messages` (Anthropic native shape), and
//!     `POST /api/chat` (Ollama native shape — note the leading
//!     `/api`, distinct from the OpenAI-compat path).
//!   * Captures the raw JSON body of every request so the test
//!     can assert provider-specific flag passthrough end-to-end
//!     (CompletionRequest → wire body).
//!   * Returns a configurable canned 200-OK JSON response so the
//!     CompletionModel deserialiser sees a plausible payload.
//!
//! `axum` is already a primary dependency of the crate, so this
//! helper introduces no new test-time crate; it lives here rather
//! than in the production tree so production builds don't link
//! the router definition.

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use axum::{Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::post};
use serde_json::Value;
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle};

#[derive(Default)]
struct CapturedState {
    bodies: Vec<Value>,
    canned: Option<Value>,
}

/// Test-only HTTP server that captures inbound JSON bodies and
/// replies with a configurable canned response.
///
/// The server stops on `Drop` (graceful shutdown via oneshot).
pub struct MockProviderServer {
    addr: SocketAddr,
    state: Arc<Mutex<CapturedState>>,
    shutdown: Mutex<Option<oneshot::Sender<()>>>,
    handle: Mutex<Option<JoinHandle<()>>>,
}

impl MockProviderServer {
    /// Bind to `127.0.0.1:0` and start serving. The provided
    /// `canned` value is returned as the JSON body of every
    /// successful request.
    pub async fn start(canned: Value) -> Self {
        let state = Arc::new(Mutex::new(CapturedState {
            canned: Some(canned),
            ..Default::default()
        }));
        let app = Router::new()
            .route("/chat/completions", post(handler))
            .route("/v1/messages", post(handler))
            .route("/api/chat", post(handler))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock provider server");
        let addr = listener.local_addr().expect("read mock provider address");
        let (tx, rx) = oneshot::channel::<()>();
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = rx.await;
                })
                .await;
        });
        Self {
            addr,
            state,
            shutdown: Mutex::new(Some(tx)),
            handle: Mutex::new(Some(handle)),
        }
    }

    /// `http://127.0.0.1:<port>` — pass to a provider client's
    /// `with_base_url` constructor.
    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Snapshot of every JSON body seen so far (in arrival order).
    pub fn captured_bodies(&self) -> Vec<Value> {
        self.state
            .lock()
            .expect("captured state poisoned")
            .bodies
            .clone()
    }
}

async fn handler(
    State(state): State<Arc<Mutex<CapturedState>>>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let canned = {
        let mut s = state.lock().expect("captured state poisoned");
        s.bodies.push(body);
        s.canned.clone().unwrap_or_else(|| serde_json::json!({}))
    };
    (StatusCode::OK, Json(canned))
}

impl Drop for MockProviderServer {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.shutdown.lock()
            && let Some(tx) = guard.take()
        {
            let _ = tx.send(());
        }
        if let Ok(mut guard) = self.handle.lock()
            && let Some(h) = guard.take()
        {
            h.abort();
        }
    }
}
