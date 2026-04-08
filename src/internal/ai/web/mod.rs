//! # Embedded Web Server for `libra code`
//!
//! This module serves the static Next.js bundle and the provider-agnostic
//! `/api/code/*` protocol used by the browser UI.

pub mod code_ui;

use std::{convert::Infallible, net::SocketAddr, path::PathBuf, sync::Arc};

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use futures_util::stream::{self, StreamExt};
use tokio::sync::oneshot;
use tokio_stream::wrappers::BroadcastStream;

use self::code_ui::{
    CodeUiApiError, CodeUiControllerDetachRequest, CodeUiInteractionResponse, CodeUiMessageRequest,
    CodeUiRuntimeHandle, browser_controller_token_from_headers, ensure_session_updated_event,
};
use crate::{command::code::resolve_storage_root, utils::util::get_repo_name_from_url};

#[derive(Clone)]
struct WebAppState {
    working_dir: Arc<PathBuf>,
    code_ui: Option<Arc<CodeUiRuntimeHandle>>,
}

#[derive(Clone, Default)]
pub struct WebServerOptions {
    pub code_ui: Option<Arc<CodeUiRuntimeHandle>>,
}

/// Handle to a running web server, providing its bound address and a
/// mechanism for graceful shutdown via the oneshot channel.
pub struct WebServerHandle {
    pub addr: SocketAddr,
    shutdown_tx: oneshot::Sender<()>,
    join: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl WebServerHandle {
    pub async fn shutdown(self) {
        let _ = self.shutdown_tx.send(());
        let _ = self.join.await;
    }
}

/// Binds an `axum` HTTP server to `host:port` and spawns it as a background
/// tokio task. Returns a [`WebServerHandle`] for later graceful shutdown.
pub async fn start(
    host: &str,
    port: u16,
    working_dir: PathBuf,
    options: WebServerOptions,
) -> anyhow::Result<WebServerHandle> {
    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;

    let app = build_router(WebAppState {
        working_dir: Arc::new(working_dir),
        code_ui: options.code_ui,
    });
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

fn build_router(state: WebAppState) -> Router {
    Router::new()
        .nest("/api", api_router())
        .with_state(state)
        .fallback(static_handler)
}

fn api_router() -> Router<WebAppState> {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/repo", get(repo_info_handler))
        .nest("/code", code_router())
}

fn code_router() -> Router<WebAppState> {
    Router::new()
        .route("/session", get(code_session_handler))
        .route("/events", get(code_events_handler))
        .route("/controller/attach", post(code_controller_attach_handler))
        .route("/controller/detach", post(code_controller_detach_handler))
        .route("/messages", post(code_message_handler))
        .route("/interactions/{id}", post(code_interaction_handler))
}

async fn static_handler(uri: axum::http::Uri) -> impl IntoResponse {
    use crate::command::web_assets::WebAssets;

    let path = uri.path().trim_start_matches('/');

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

async fn repo_info_handler(State(state): State<WebAppState>) -> impl IntoResponse {
    use serde_json::json;

    use crate::internal::config::ConfigKv;

    let id = ConfigKv::get("libra.repoid")
        .await
        .ok()
        .flatten()
        .map(|entry| entry.value)
        .unwrap_or_default();

    let name = match ConfigKv::get_current_remote_url().await {
        Ok(Some(url)) => get_repo_name_from_url(&url)
            .map(|value| value.to_string())
            .unwrap_or_default(),
        _ => state
            .working_dir
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default(),
    };

    let storage_root = resolve_storage_root(&state.working_dir);
    let desc_path = storage_root.join("description");
    let description = std::fs::read_to_string(&desc_path).unwrap_or_default();

    Json(json!({
        "id": id,
        "name": name,
        "description": description.trim(),
    }))
}

async fn code_session_handler(
    State(state): State<WebAppState>,
) -> Result<Json<serde_json::Value>, WebApiError> {
    let runtime = code_ui_runtime(&state)?;
    Ok(Json(serde_json::to_value(runtime.snapshot().await)?))
}

async fn code_events_handler(
    State(state): State<WebAppState>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, WebApiError> {
    let runtime = code_ui_runtime(&state)?;
    let current_snapshot = runtime.snapshot().await;
    let initial_event = ensure_session_updated_event(&current_snapshot)?;
    let receiver = runtime.subscribe();

    let initial_stream = stream::once(async move { Ok(code_ui_event_to_sse(initial_event)) });
    let updates = BroadcastStream::new(receiver).filter_map(|message| async move {
        match message {
            Ok(event) => Some(Ok(code_ui_event_to_sse(event))),
            Err(_) => None,
        }
    });

    Ok(Sse::new(initial_stream.chain(updates)).keep_alive(KeepAlive::new()))
}

async fn code_controller_attach_handler(
    State(state): State<WebAppState>,
    Json(body): Json<code_ui::CodeUiControllerAttachRequest>,
) -> Result<Json<serde_json::Value>, WebApiError> {
    let runtime = code_ui_runtime(&state)?;
    let response = runtime.attach_browser_controller(&body.client_id).await?;
    Ok(Json(serde_json::to_value(response)?))
}

async fn code_controller_detach_handler(
    State(state): State<WebAppState>,
    headers: HeaderMap,
    Json(body): Json<CodeUiControllerDetachRequest>,
) -> Result<Json<serde_json::Value>, WebApiError> {
    let runtime = code_ui_runtime(&state)?;
    let token = browser_controller_token_from_headers(&headers).ok_or_else(|| {
        WebApiError::from(CodeUiApiError::forbidden(
            "MISSING_CONTROLLER_TOKEN",
            "A browser controller token is required for detach",
        ))
    })?;
    runtime
        .detach_browser_controller(&body.client_id, &token)
        .await?;
    Ok(Json(serde_json::json!({ "detached": true })))
}

async fn code_message_handler(
    State(state): State<WebAppState>,
    headers: HeaderMap,
    Json(body): Json<CodeUiMessageRequest>,
) -> Result<Json<serde_json::Value>, WebApiError> {
    let runtime = code_ui_runtime(&state)?;
    let token = browser_controller_token_from_headers(&headers);
    runtime.submit_message(token.as_deref(), body.text).await?;
    Ok(Json(serde_json::to_value(code_ui::CodeUiAckResponse {
        accepted: true,
    })?))
}

async fn code_interaction_handler(
    State(state): State<WebAppState>,
    Path(interaction_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CodeUiInteractionResponse>,
) -> Result<Json<serde_json::Value>, WebApiError> {
    let runtime = code_ui_runtime(&state)?;
    let token = browser_controller_token_from_headers(&headers);
    runtime
        .respond_interaction(token.as_deref(), &interaction_id, body)
        .await?;
    Ok(Json(serde_json::to_value(code_ui::CodeUiAckResponse {
        accepted: true,
    })?))
}

fn code_ui_runtime(state: &WebAppState) -> Result<Arc<CodeUiRuntimeHandle>, WebApiError> {
    state
        .code_ui
        .clone()
        .ok_or_else(|| WebApiError::from(CodeUiApiError::unavailable()))
}

fn code_ui_event_to_sse(event: code_ui::CodeUiEventEnvelope) -> Event {
    Event::default()
        .event(event.event_type.clone())
        .json_data(event)
        .unwrap_or_else(|_| Event::default().event("session_updated"))
}

struct WebApiError {
    status: StatusCode,
    code: String,
    message: String,
}

impl From<CodeUiApiError> for WebApiError {
    fn from(value: CodeUiApiError) -> Self {
        Self {
            status: StatusCode::from_u16(value.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            code: value.code,
            message: value.message,
        }
    }
}

impl From<anyhow::Error> for WebApiError {
    fn from(value: anyhow::Error) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "INTERNAL_ERROR".to_string(),
            message: value.to_string(),
        }
    }
}

impl From<serde_json::Error> for WebApiError {
    fn from(value: serde_json::Error) -> Self {
        anyhow::Error::new(value).into()
    }
}

impl IntoResponse for WebApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({
                "error": {
                    "code": self.code,
                    "message": self.message,
                }
            })),
        )
            .into_response()
    }
}
