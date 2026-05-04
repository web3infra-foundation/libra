//! # Embedded Web Server for `libra code`
//!
//! This module serves the static Next.js bundle and the provider-agnostic
//! `/api/code/*` protocol used by the browser UI.

pub mod code_ui;

use std::{convert::Infallible, net::SocketAddr, path::PathBuf, sync::Arc};

use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{ConnectInfo, Path, Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use futures_util::stream::{self, StreamExt};
use serde::Serialize;
use tokio::sync::oneshot;
use tokio_stream::wrappers::BroadcastStream;
use uuid::Uuid;

use self::code_ui::{
    CodeUiApiError, CodeUiControllerDetachRequest, CodeUiControllerKind, CodeUiInteractionResponse,
    CodeUiMessageRequest, CodeUiRuntimeHandle, browser_controller_token_from_headers,
    ensure_session_updated_event,
};
use crate::{
    command::code::resolve_storage_root,
    internal::ai::runtime::hardening::{AuditEvent, AuditSink, SecretRedactor, TracingAuditSink},
    utils::util::get_repo_name_from_url,
};

const CODE_CONTROL_BODY_LIMIT_BYTES: usize = 256 * 1024;
const CODE_CONTROL_BODY_REJECT_DRAIN_BYTES: usize = 1024 * 1024;

#[derive(Clone)]
struct WebAppState {
    working_dir: Arc<PathBuf>,
    code_ui: Option<Arc<CodeUiRuntimeHandle>>,
    automation_control_token: Option<Arc<str>>,
    audit_sink: Arc<dyn AuditSink>,
    control_trace_id: Uuid,
}

#[derive(Clone, Default)]
pub struct WebServerOptions {
    pub code_ui: Option<Arc<CodeUiRuntimeHandle>>,
    pub automation_control_token: Option<Arc<str>>,
    pub audit_sink: Option<Arc<dyn AuditSink>>,
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
    let bound_addr = listener.local_addr()?;

    let app = build_router(WebAppState {
        working_dir: Arc::new(working_dir),
        code_ui: options.code_ui,
        automation_control_token: options.automation_control_token,
        audit_sink: options
            .audit_sink
            .unwrap_or_else(|| Arc::new(TracingAuditSink)),
        control_trace_id: Uuid::new_v4(),
    });
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    let join = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            let _ = shutdown_rx.await;
        })
        .await
        .map_err(|e| anyhow::anyhow!(e))
    });

    Ok(WebServerHandle {
        addr: bound_addr,
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
    // Auth layer matrix (matches docs/automation/local-tui-control.md):
    //   /session          -> loopback only (observe)
    //   /events           -> loopback only (observe)
    //   /diagnostics      -> loopback only (observe)
    //   /controller/attach  -> loopback; automation also needs X-Libra-Control-Token
    //   /controller/detach  -> loopback + controller-token; automation also needs control-token
    //   /messages         -> loopback + controller-token; automation also needs control-token
    //   /interactions/{id} -> loopback + controller-token; automation also needs control-token
    //   /control/cancel   -> loopback + control-token + controller-token (automation only)
    Router::new()
        .route("/session", get(code_session_handler))
        .route("/events", get(code_events_handler))
        .route("/diagnostics", get(code_diagnostics_handler))
        .route("/controller/attach", post(code_controller_attach_handler))
        .route("/controller/detach", post(code_controller_detach_handler))
        .merge(code_write_router())
}

fn code_write_router() -> Router<WebAppState> {
    Router::new()
        .route("/messages", post(code_message_handler))
        .route("/interactions/{id}", post(code_interaction_handler))
        .route("/control/cancel", post(code_cancel_handler))
        .layer(middleware::from_fn(enforce_code_write_body_limit))
}

async fn static_handler(uri: axum::http::Uri) -> impl IntoResponse {
    use crate::command::web_assets::WebAssets;

    let path = uri.path().trim_start_matches('/');
    if path.contains("..") {
        return (StatusCode::NOT_FOUND, "404 Not Found").into_response();
    }

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

async fn repo_info_handler(
    ConnectInfo(remote_addr): ConnectInfo<SocketAddr>,
    State(state): State<WebAppState>,
) -> Result<Json<serde_json::Value>, WebApiError> {
    use serde_json::json;

    use crate::internal::config::ConfigKv;

    ensure_loopback_api_request(remote_addr)?;

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

    Ok(Json(json!({
        "id": id,
        "name": name,
        "description": description.trim(),
    })))
}

async fn code_session_handler(
    ConnectInfo(remote_addr): ConnectInfo<SocketAddr>,
    State(state): State<WebAppState>,
) -> Result<Json<serde_json::Value>, WebApiError> {
    ensure_loopback_api_request(remote_addr)?;
    let runtime = code_ui_runtime(&state)?;
    Ok(Json(serde_json::to_value(runtime.snapshot().await)?))
}

async fn code_events_handler(
    ConnectInfo(remote_addr): ConnectInfo<SocketAddr>,
    State(state): State<WebAppState>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, WebApiError> {
    ensure_loopback_api_request(remote_addr)?;
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

async fn code_diagnostics_handler(
    ConnectInfo(remote_addr): ConnectInfo<SocketAddr>,
    State(state): State<WebAppState>,
) -> Result<Json<serde_json::Value>, WebApiError> {
    ensure_loopback_api_request(remote_addr)?;
    let runtime = code_ui_runtime(&state)?;
    Ok(Json(serde_json::to_value(runtime.diagnostics().await)?))
}

async fn code_controller_attach_handler(
    ConnectInfo(remote_addr): ConnectInfo<SocketAddr>,
    State(state): State<WebAppState>,
    headers: HeaderMap,
    Json(body): Json<code_ui::CodeUiControllerAttachRequest>,
) -> Result<Json<serde_json::Value>, WebApiError> {
    ensure_loopback_api_request(remote_addr)?;
    let runtime = code_ui_runtime(&state)?;
    let result = async {
        if body.kind == CodeUiControllerKind::Automation {
            ensure_automation_control_token(&headers, state.automation_control_token.as_ref())?;
        }
        runtime
            .attach_controller(body.kind, &body.client_id)
            .await
            .map_err(WebApiError::from)
    }
    .await;
    append_control_audit(
        &state,
        &runtime,
        "controller.attach",
        body.kind,
        &body.client_id,
        control_audit_outcome(&result),
    )
    .await;
    let response = result?;
    Ok(Json(serde_json::to_value(response)?))
}

async fn code_controller_detach_handler(
    ConnectInfo(remote_addr): ConnectInfo<SocketAddr>,
    State(state): State<WebAppState>,
    headers: HeaderMap,
    Json(body): Json<CodeUiControllerDetachRequest>,
) -> Result<Json<serde_json::Value>, WebApiError> {
    ensure_loopback_api_request(remote_addr)?;
    let runtime = code_ui_runtime(&state)?;
    let token = browser_controller_token_from_headers(&headers).ok_or_else(|| {
        WebApiError::from(CodeUiApiError::forbidden(
            "MISSING_CONTROLLER_TOKEN",
            "A browser controller token is required for detach",
        ))
    })?;
    let mut audit_kind = CodeUiControllerKind::None;
    let result = async {
        let lease = runtime.ensure_controller_write_access(Some(&token)).await?;
        audit_kind = lease.kind;
        if lease.kind == CodeUiControllerKind::Automation {
            ensure_automation_control_token(&headers, state.automation_control_token.as_ref())?;
        }
        runtime
            .detach_controller(lease.kind, &body.client_id, &token, false)
            .await
            .map_err(WebApiError::from)
    }
    .await;
    append_control_audit(
        &state,
        &runtime,
        "controller.detach",
        audit_kind,
        &body.client_id,
        control_audit_outcome(&result),
    )
    .await;
    result?;
    Ok(Json(serde_json::json!({ "detached": true })))
}

async fn code_message_handler(
    ConnectInfo(remote_addr): ConnectInfo<SocketAddr>,
    State(state): State<WebAppState>,
    headers: HeaderMap,
    Json(body): Json<CodeUiMessageRequest>,
) -> Result<Json<serde_json::Value>, WebApiError> {
    ensure_loopback_api_request(remote_addr)?;
    let runtime = code_ui_runtime(&state)?;
    let token = browser_controller_token_from_headers(&headers);
    let mut audit_kind = CodeUiControllerKind::None;
    let mut audit_client_id = "unknown".to_string();
    let result = async {
        let lease = runtime
            .ensure_controller_write_access(token.as_deref())
            .await?;
        audit_kind = lease.kind;
        audit_client_id = lease.client_id.clone();
        if lease.kind == CodeUiControllerKind::Automation {
            ensure_automation_control_token(&headers, state.automation_control_token.as_ref())?;
        }
        runtime
            .submit_message(token.as_deref(), body.text)
            .await
            .map_err(WebApiError::from)
    }
    .await;
    append_control_audit(
        &state,
        &runtime,
        "message.submit",
        audit_kind,
        &audit_client_id,
        control_audit_outcome(&result),
    )
    .await;
    result?;
    Ok(Json(serde_json::to_value(code_ui::CodeUiAckResponse {
        accepted: true,
    })?))
}

async fn code_interaction_handler(
    ConnectInfo(remote_addr): ConnectInfo<SocketAddr>,
    State(state): State<WebAppState>,
    Path(interaction_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CodeUiInteractionResponse>,
) -> Result<Json<serde_json::Value>, WebApiError> {
    ensure_loopback_api_request(remote_addr)?;
    let runtime = code_ui_runtime(&state)?;
    let token = browser_controller_token_from_headers(&headers);
    let mut audit_kind = CodeUiControllerKind::None;
    let mut audit_client_id = "unknown".to_string();
    let result = async {
        let lease = runtime
            .ensure_controller_write_access(token.as_deref())
            .await?;
        audit_kind = lease.kind;
        audit_client_id = lease.client_id.clone();
        if lease.kind == CodeUiControllerKind::Automation {
            ensure_automation_control_token(&headers, state.automation_control_token.as_ref())?;
        }
        runtime
            .respond_interaction(token.as_deref(), &interaction_id, body)
            .await
            .map_err(WebApiError::from)
    }
    .await;
    append_control_audit(
        &state,
        &runtime,
        "interaction.respond",
        audit_kind,
        &audit_client_id,
        control_audit_outcome(&result),
    )
    .await;
    result?;
    Ok(Json(serde_json::to_value(code_ui::CodeUiAckResponse {
        accepted: true,
    })?))
}

async fn code_cancel_handler(
    ConnectInfo(remote_addr): ConnectInfo<SocketAddr>,
    State(state): State<WebAppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, WebApiError> {
    ensure_loopback_api_request(remote_addr)?;
    let runtime = code_ui_runtime(&state)?;
    let token = browser_controller_token_from_headers(&headers);
    let mut audit_client_id = "unknown".to_string();
    let result = async {
        let lease = runtime
            .ensure_controller_write_access(token.as_deref())
            .await?;
        audit_client_id = lease.client_id.clone();
        if lease.kind != CodeUiControllerKind::Automation {
            return Err(WebApiError::from(CodeUiApiError::forbidden(
                "AUTOMATION_CONTROLLER_REQUIRED",
                "Only an automation controller can cancel through /api/code/control/cancel",
            )));
        }
        ensure_automation_control_token(&headers, state.automation_control_token.as_ref())?;
        runtime
            .cancel_turn(token.as_deref())
            .await
            .map_err(WebApiError::from)
    }
    .await;
    append_control_audit(
        &state,
        &runtime,
        "turn.cancel",
        CodeUiControllerKind::Automation,
        &audit_client_id,
        control_audit_outcome(&result),
    )
    .await;
    result?;
    Ok(Json(serde_json::to_value(code_ui::CodeUiAckResponse {
        accepted: true,
    })?))
}

async fn enforce_code_write_body_limit(request: Request, next: Next) -> Response {
    let (parts, body) = request.into_parts();
    match to_bytes(body, CODE_CONTROL_BODY_REJECT_DRAIN_BYTES).await {
        Ok(body) if body.len() <= CODE_CONTROL_BODY_LIMIT_BYTES => {
            next.run(Request::from_parts(parts, Body::from(body))).await
        }
        Ok(_) | Err(_) => code_control_body_too_large_response(),
    }
}

fn code_control_body_too_large_response() -> Response {
    WebApiError {
        status: StatusCode::PAYLOAD_TOO_LARGE,
        code: "PAYLOAD_TOO_LARGE".to_string(),
        message: "Code UI write request bodies are limited to 256KiB".to_string(),
    }
    .into_response()
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

fn ensure_loopback_api_request(remote_addr: SocketAddr) -> Result<(), WebApiError> {
    if remote_addr.ip().is_loopback() {
        return Ok(());
    }

    Err(WebApiError::forbidden(
        "LOOPBACK_REQUIRED",
        "Libra Code API requests must come from a loopback client",
    ))
}

fn ensure_automation_control_token(
    headers: &HeaderMap,
    expected: Option<&Arc<str>>,
) -> Result<(), WebApiError> {
    let Some(expected) = expected else {
        return Err(WebApiError::forbidden(
            "CONTROL_DISABLED",
            "Local TUI automation write control is not enabled; start with --control write",
        ));
    };

    let Some(actual) = headers
        .get("x-libra-control-token")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Err(WebApiError::forbidden(
            "MISSING_CONTROL_TOKEN",
            "X-Libra-Control-Token is required for automation control requests",
        ));
    };

    if actual != expected.as_ref() {
        return Err(WebApiError::forbidden(
            "INVALID_CONTROL_TOKEN",
            "X-Libra-Control-Token does not match this Libra Code session",
        ));
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControlAuditOutcome<'a> {
    Accepted,
    Error(&'a str),
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ControlAuditRecord<'a> {
    thread_id: Option<String>,
    controller_kind: &'static str,
    client_id: &'a str,
    result: &'static str,
    error_code: Option<&'a str>,
}

fn control_audit_outcome<T>(result: &Result<T, WebApiError>) -> ControlAuditOutcome<'_> {
    match result {
        Ok(_) => ControlAuditOutcome::Accepted,
        Err(error) => ControlAuditOutcome::Error(error.code.as_str()),
    }
}

async fn append_control_audit(
    state: &WebAppState,
    runtime: &CodeUiRuntimeHandle,
    action: &'static str,
    controller_kind: CodeUiControllerKind,
    client_id: &str,
    outcome: ControlAuditOutcome<'_>,
) {
    let snapshot = runtime.snapshot().await;
    let redactor = SecretRedactor::default_runtime();
    let client_id = sanitized_audit_client_id(&redactor, client_id);
    let (result, error_code) = match outcome {
        ControlAuditOutcome::Accepted => ("accepted", None),
        ControlAuditOutcome::Error(code) => ("error", Some(code)),
    };
    let record = ControlAuditRecord {
        thread_id: snapshot.thread_id.clone(),
        controller_kind: controller_kind.as_str(),
        client_id: &client_id,
        result,
        error_code,
    };
    let redacted_summary = match serde_json::to_string(&record) {
        Ok(summary) => redactor.redact(&summary),
        Err(error) => {
            tracing::warn!(error = %error, "failed to serialize local TUI control audit summary");
            return;
        }
    };
    let trace_id = snapshot
        .thread_id
        .as_deref()
        .and_then(|thread_id| Uuid::parse_str(thread_id).ok())
        .unwrap_or(state.control_trace_id);

    if let Err(error) = state
        .audit_sink
        .append(AuditEvent {
            trace_id,
            principal_id: format!(
                "local-tui-control:{}:{}",
                controller_kind.as_str(),
                client_id
            ),
            action: action.to_string(),
            policy_version: "local-tui-control/v1".to_string(),
            redacted_summary,
            at: chrono::Utc::now(),
        })
        .await
    {
        tracing::warn!(error = %error, action, "failed to append local TUI control audit event");
    }
}

fn sanitized_audit_client_id(redactor: &SecretRedactor, client_id: &str) -> String {
    let redacted = redactor.redact(client_id.trim());
    let mut sanitized = redacted
        .chars()
        .map(|ch| if ch.is_control() { '_' } else { ch })
        .collect::<String>();
    if sanitized.is_empty() {
        sanitized = "unknown".to_string();
    }
    if sanitized.chars().count() > 80 {
        sanitized = sanitized.chars().take(80).collect();
    }
    sanitized
}

struct WebApiError {
    status: StatusCode,
    code: String,
    message: String,
}

impl WebApiError {
    fn forbidden(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code: code.into(),
            message: message.into(),
        }
    }
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

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, Ipv6Addr};

    use axum::{
        body::{Body, to_bytes},
        http::{Method, Request, Uri},
    };
    use tower::ServiceExt;

    use super::*;
    use crate::internal::ai::{
        runtime::hardening::InMemoryAuditSink,
        web::code_ui::{
            CodeUiCapabilities, CodeUiInitialController, CodeUiProviderInfo, CodeUiSession,
            ReadOnlyCodeUiAdapter, initial_snapshot,
        },
    };

    #[test]
    fn loopback_api_request_allows_loopback_clients() {
        let ipv4 = SocketAddr::from((Ipv4Addr::LOCALHOST, 34567));
        let ipv6 = SocketAddr::from((Ipv6Addr::LOCALHOST, 34567));

        assert!(ensure_loopback_api_request(ipv4).is_ok());
        assert!(ensure_loopback_api_request(ipv6).is_ok());
    }

    #[test]
    fn loopback_api_request_rejects_remote_clients() {
        let remote = SocketAddr::from((Ipv4Addr::new(192, 0, 2, 10), 34567));
        let error =
            ensure_loopback_api_request(remote).expect_err("remote client must be rejected");

        assert_eq!(error.status, StatusCode::FORBIDDEN);
        assert_eq!(error.code, "LOOPBACK_REQUIRED");
    }

    #[test]
    fn code_control_auth_rejects_when_disabled() {
        let headers = HeaderMap::new();

        let error = ensure_automation_control_token(&headers, None).unwrap_err();

        assert_eq!(error.status, StatusCode::FORBIDDEN);
        assert_eq!(error.code, "CONTROL_DISABLED");
    }

    #[test]
    fn code_control_auth_requires_token_header() {
        let headers = HeaderMap::new();
        let expected: Arc<str> = Arc::from("secret");

        let error = ensure_automation_control_token(&headers, Some(&expected)).unwrap_err();

        assert_eq!(error.status, StatusCode::FORBIDDEN);
        assert_eq!(error.code, "MISSING_CONTROL_TOKEN");
    }

    #[test]
    fn code_control_auth_rejects_invalid_token() {
        let mut headers = HeaderMap::new();
        headers.insert("x-libra-control-token", "wrong".parse().unwrap());
        let expected: Arc<str> = Arc::from("secret");

        let error = ensure_automation_control_token(&headers, Some(&expected)).unwrap_err();

        assert_eq!(error.status, StatusCode::FORBIDDEN);
        assert_eq!(error.code, "INVALID_CONTROL_TOKEN");
    }

    #[test]
    fn code_control_auth_accepts_matching_token() {
        let mut headers = HeaderMap::new();
        headers.insert("x-libra-control-token", "secret".parse().unwrap());
        let expected: Arc<str> = Arc::from("secret");

        assert!(ensure_automation_control_token(&headers, Some(&expected)).is_ok());
    }

    #[tokio::test]
    async fn code_write_body_limit_returns_json_error() {
        let app = code_write_router().with_state(WebAppState {
            working_dir: Arc::new(PathBuf::from("/tmp/libra")),
            code_ui: None,
            automation_control_token: None,
            audit_sink: Arc::new(TracingAuditSink),
            control_trace_id: Uuid::new_v4(),
        });
        let oversized_text = "x".repeat(CODE_CONTROL_BODY_LIMIT_BYTES + 1);
        let body = format!(r#"{{"text":"{oversized_text}"}}"#);
        let request = Request::builder()
            .method(Method::POST)
            .uri("/messages")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::CONTENT_LENGTH, body.len().to_string())
            .body(Body::from(body))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["error"]["code"], "PAYLOAD_TOO_LARGE");
    }

    #[tokio::test]
    async fn automation_attach_appends_redacted_control_audit_event() {
        let session = CodeUiSession::new(initial_snapshot(
            "/tmp/libra",
            CodeUiProviderInfo {
                provider: "test".to_string(),
                model: Some("test-model".to_string()),
                mode: None,
                managed: false,
            },
            CodeUiCapabilities::default(),
        ));
        let runtime = CodeUiRuntimeHandle::build_with_control(
            ReadOnlyCodeUiAdapter::new(session, CodeUiCapabilities::default()),
            false,
            true,
            CodeUiInitialController::LocalTui {
                owner_label: "Terminal UI".to_string(),
                reason: None,
            },
        )
        .await;
        let audit_sink = Arc::new(InMemoryAuditSink::default());
        let app = code_router().with_state(WebAppState {
            working_dir: Arc::new(PathBuf::from("/tmp/libra")),
            code_ui: Some(runtime),
            automation_control_token: Some(Arc::from("control-token-secret")),
            audit_sink: audit_sink.clone(),
            control_trace_id: Uuid::new_v4(),
        });
        let request = Request::builder()
            .method(Method::POST)
            .uri("/controller/attach")
            .header(header::CONTENT_TYPE, "application/json")
            .header("x-libra-control-token", "control-token-secret")
            .extension(ConnectInfo(SocketAddr::from((Ipv4Addr::LOCALHOST, 4000))))
            .body(Body::from(
                r#"{"clientId":"local-script token:super-secret","kind":"automation"}"#,
            ))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let events = audit_sink.events().await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].action, "controller.attach");
        assert_eq!(events[0].policy_version, "local-tui-control/v1");
        assert!(
            events[0]
                .redacted_summary
                .contains("\"result\":\"accepted\"")
        );
        assert!(!events[0].redacted_summary.contains("super-secret"));
        assert!(!events[0].redacted_summary.contains("control-token-secret"));
    }

    #[tokio::test]
    async fn static_handler_rejects_parent_directory_segments() {
        let response = static_handler(Uri::from_static("/../index.html"))
            .await
            .into_response();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
