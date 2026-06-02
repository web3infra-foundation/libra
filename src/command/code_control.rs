//! Local automation shim for driving an existing `libra code --control write`
//!
//! 本地自动化垫片，用于驱动现有的 `libra code --control write`
//! session over NDJSON JSON-RPC 2.0.
//!
//! This command is intentionally separate from `libra code --stdio`, which
//! remains the MCP stdio transport. `code-control --stdio` is a local bridge
//! from JSON-RPC lines to the loopback `/api/code/*` HTTP/SSE control surface.

use std::{
    io::{self, BufRead, Write},
    path::PathBuf,
};

use clap::Parser;
use futures_util::StreamExt;
use reqwest::{Client, RequestBuilder, StatusCode};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use url::Url;

use crate::utils::error::{CliError, CliResult};

/// `--help` examples shown in `libra code-control --help` output.
///
/// `code-control` drives a local Libra Code TUI automation control
/// session over NDJSON JSON-RPC 2.0 on stdin/stdout. The banner pins
/// the canonical `--stdio` form (the only supported form today),
/// shows how to wire it to the discovery file emitted by
/// `libra code --control write`, and demonstrates Unix-style piping
/// to feed a single JSON-RPC request through the shim. Cross-cutting
/// `--help` EXAMPLES rollout per `docs/improvement/README.md` item B.
pub const CODE_CONTROL_EXAMPLES: &str = "\
EXAMPLES:
    libra code-control --stdio --url http://127.0.0.1:3000 --token-file ./control.token
                                                  Run the JSON-RPC shim against a session at the given URL/token
    libra code-control --stdio \\
        --url $(jq -r .url .libra/code/control.json) \\
        --token-file .libra/code/control.token
                                                  Wire from the discovery file emitted by 'libra code --control write'
    echo '{\"jsonrpc\":\"2.0\",\"method\":\"controller.attach\",\"params\":{\"clientId\":\"my-script\"},\"id\":1}' | \\
        libra code-control --stdio --url http://127.0.0.1:3000 --token-file ./control.token
                                                  Send a single attach request through the shim";

#[derive(Debug, Clone, Parser)]
#[command(after_help = CODE_CONTROL_EXAMPLES)]
pub struct CodeControlArgs {
    /// Run the local automation shim on stdin/stdout as NDJSON JSON-RPC 2.0.
    #[arg(long)]
    pub stdio: bool,
    /// Base URL from `.libra/code/control.json`, e.g. http://127.0.0.1:3000.
    #[arg(long)]
    pub url: String,
    /// Path to the local process-level control token file
    #[arg(long, value_name = "PATH")]
    pub token_file: PathBuf,
}

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: Option<String>,
    method: Option<String>,
    #[serde(default)]
    params: Option<Value>,
    #[serde(default)]
    id: Option<Value>,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcErrorObject>,
    id: Value,
}

#[derive(Debug, Clone, Serialize)]
struct JsonRpcErrorObject {
    code: i64,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttachParams {
    client_id: String,
    #[serde(default)]
    kind: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DetachParams {
    client_id: String,
    controller_token: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubmitParams {
    text: String,
    controller_token: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RespondParams {
    interaction_id: String,
    controller_token: String,
    response: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CancelParams {
    controller_token: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskDispatchParams {
    agent: String,
    prompt: String,
    controller_token: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadsListParams {
    #[serde(default)]
    limit: Option<Value>,
    #[serde(default)]
    offset: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoalStartParams {
    objective: String,
    controller_token: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoalCancelParams {
    reason: String,
    controller_token: String,
}

pub async fn execute(args: CodeControlArgs) -> CliResult<()> {
    if !args.stdio {
        return Err(CliError::command_usage(
            "`libra code-control` currently supports only `--stdio`",
        ));
    }

    let base_url = Url::parse(&args.url).map_err(|error| {
        CliError::command_usage(format!(
            "--url must be a valid control endpoint base URL (got '{}': {error})",
            args.url
        ))
    })?;
    let control_token = read_control_token(&args.token_file)?;
    let client = Client::builder()
        .build()
        .map_err(|error| CliError::fatal(format!("failed to build HTTP client: {error}")))?;

    let stdin = io::stdin();
    let lines = stdin.lock().lines();
    for line in lines {
        let line = line.map_err(|error| {
            CliError::fatal(format!(
                "failed to read JSON-RPC request from stdin: {error}"
            ))
        })?;
        if line.trim().is_empty() {
            continue;
        }

        let parsed = match parse_json_rpc_request(&line) {
            Ok(request) => request,
            Err(error) => {
                write_json_rpc_response(&json_rpc_error(Value::Null, error))?;
                continue;
            }
        };
        let id = parsed.id.clone().unwrap_or(Value::Null);
        let response = dispatch_json_rpc_request(&client, &base_url, &control_token, parsed).await;
        match response {
            DispatchResult::Response(response) => write_json_rpc_response(&response)?,
            DispatchResult::NotificationOnly => {}
            DispatchResult::Subscribe { response } => {
                write_json_rpc_response(&response)?;
                stream_events(&client, &base_url).await?;
                break;
            }
            DispatchResult::Error(error) => write_json_rpc_response(&json_rpc_error(id, error))?,
        }
    }

    Ok(())
}

fn read_control_token(path: &PathBuf) -> CliResult<String> {
    let content = std::fs::read_to_string(path).map_err(|error| {
        CliError::fatal(format!(
            "failed to read local TUI control token file '{}': {error}",
            path.display()
        ))
    })?;
    let token = content.trim().to_string();
    if token.is_empty() {
        return Err(CliError::fatal(format!(
            "local TUI control token file '{}' is empty",
            path.display()
        )));
    }
    Ok(token)
}

fn parse_json_rpc_request(line: &str) -> Result<JsonRpcRequest, JsonRpcErrorObject> {
    let request: JsonRpcRequest =
        serde_json::from_str(line).map_err(|error| JsonRpcErrorObject {
            code: -32700,
            message: format!("Parse error: {error}"),
            data: None,
        })?;
    if request.jsonrpc.as_deref() != Some("2.0") || request.method.is_none() {
        return Err(JsonRpcErrorObject {
            code: -32600,
            message: "Invalid Request: expected JSON-RPC 2.0 object with method".to_string(),
            data: None,
        });
    }
    Ok(request)
}

enum DispatchResult {
    Response(JsonRpcResponse),
    NotificationOnly,
    Subscribe { response: JsonRpcResponse },
    Error(JsonRpcErrorObject),
}

async fn dispatch_json_rpc_request(
    client: &Client,
    base_url: &Url,
    control_token: &str,
    request: JsonRpcRequest,
) -> DispatchResult {
    let id = request.id.clone().unwrap_or(Value::Null);
    let Some(method) = request.method.as_deref() else {
        return DispatchResult::Error(JsonRpcErrorObject {
            code: -32600,
            message: "Invalid Request: missing method".to_string(),
            data: None,
        });
    };
    let result = match method {
        "session.get" => send_get(client, base_url, "/api/code/session").await,
        "diagnostics.get" => send_get(client, base_url, "/api/code/diagnostics").await,
        "threads.list" => {
            let params = match parse_optional_params::<ThreadsListParams>(request.params) {
                Ok(params) => params,
                Err(error) => return DispatchResult::Error(error),
            };
            let query = match thread_list_query_params(params) {
                Ok(query) => query,
                Err(error) => return DispatchResult::Error(error),
            };
            send_get_with_query(client, base_url, "/api/code/threads", query).await
        }
        "controller.attach" => {
            let params = match parse_params::<AttachParams>(request.params) {
                Ok(params) => params,
                Err(error) => return DispatchResult::Error(error),
            };
            let mut body = json!({ "clientId": params.client_id });
            if let Some(kind) = params.kind {
                body["kind"] = Value::String(kind);
            }
            send_post(
                client,
                base_url,
                "/api/code/controller/attach",
                control_token,
                None,
                body,
            )
            .await
        }
        "controller.detach" => {
            let params = match parse_params::<DetachParams>(request.params) {
                Ok(params) => params,
                Err(error) => return DispatchResult::Error(error),
            };
            send_post(
                client,
                base_url,
                "/api/code/controller/detach",
                control_token,
                Some(&params.controller_token),
                json!({ "clientId": params.client_id }),
            )
            .await
        }
        "message.submit" => {
            let params = match parse_params::<SubmitParams>(request.params) {
                Ok(params) => params,
                Err(error) => return DispatchResult::Error(error),
            };
            send_post(
                client,
                base_url,
                "/api/code/messages",
                control_token,
                Some(&params.controller_token),
                json!({ "text": params.text }),
            )
            .await
        }
        "interaction.respond" => {
            let params = match parse_params::<RespondParams>(request.params) {
                Ok(params) => params,
                Err(error) => return DispatchResult::Error(error),
            };
            let endpoint = format!("/api/code/interactions/{}", params.interaction_id);
            send_post(
                client,
                base_url,
                &endpoint,
                control_token,
                Some(&params.controller_token),
                params.response,
            )
            .await
        }
        "turn.cancel" => {
            let params = match parse_params::<CancelParams>(request.params) {
                Ok(params) => params,
                Err(error) => return DispatchResult::Error(error),
            };
            send_post(
                client,
                base_url,
                "/api/code/control/cancel",
                control_token,
                Some(&params.controller_token),
                json!({}),
            )
            .await
        }
        "events.subscribe" => {
            return DispatchResult::Subscribe {
                response: json_rpc_success(id, json!({ "subscribed": true })),
            };
        }
        "task.dispatch" => {
            let params = match parse_params::<TaskDispatchParams>(request.params) {
                Ok(params) => params,
                Err(error) => return DispatchResult::Error(error),
            };
            send_post(
                client,
                base_url,
                "/api/code/task/dispatch",
                control_token,
                Some(&params.controller_token),
                json!({ "agent": params.agent, "prompt": params.prompt }),
            )
            .await
        }
        "goal.start" => {
            // OC-Phase 6 P6.6 — Goal mode entrypoint for automation.
            // Same contract as the TUI `/goal start <objective>`
            // (parses the objective, validates shape, mints
            // `GoalEvent::Created` in the active session).
            let params = match parse_params::<GoalStartParams>(request.params) {
                Ok(params) => params,
                Err(error) => return DispatchResult::Error(error),
            };
            send_post(
                client,
                base_url,
                "/api/code/goal/start",
                control_token,
                Some(&params.controller_token),
                json!({ "objective": params.objective }),
            )
            .await
        }
        "goal.status" => {
            // Read-only observe endpoint (loopback only). No
            // controller token required at this layer.
            send_get(client, base_url, "/api/code/goal/status").await
        }
        "goal.cancel" => {
            let params = match parse_params::<GoalCancelParams>(request.params) {
                Ok(params) => params,
                Err(error) => return DispatchResult::Error(error),
            };
            send_post(
                client,
                base_url,
                "/api/code/goal/cancel",
                control_token,
                Some(&params.controller_token),
                json!({ "reason": params.reason }),
            )
            .await
        }
        _ => {
            return DispatchResult::Error(JsonRpcErrorObject {
                code: -32601,
                message: format!("Method not found: {method}"),
                data: None,
            });
        }
    };

    match result {
        Ok(result) if request.id.is_some() => {
            DispatchResult::Response(json_rpc_success(id, result))
        }
        Ok(_) => DispatchResult::NotificationOnly,
        Err(error) => DispatchResult::Error(error),
    }
}

fn parse_params<T: DeserializeOwned>(params: Option<Value>) -> Result<T, JsonRpcErrorObject> {
    let params = params.ok_or_else(|| JsonRpcErrorObject {
        code: -32602,
        message: "Invalid params: params object is required".to_string(),
        data: None,
    })?;
    serde_json::from_value(params).map_err(|error| JsonRpcErrorObject {
        code: -32602,
        message: format!("Invalid params: {error}"),
        data: None,
    })
}

fn parse_optional_params<T: Default + DeserializeOwned>(
    params: Option<Value>,
) -> Result<T, JsonRpcErrorObject> {
    match params {
        Some(params) => serde_json::from_value(params).map_err(|error| JsonRpcErrorObject {
            code: -32602,
            message: format!("Invalid params: {error}"),
            data: None,
        }),
        None => Ok(T::default()),
    }
}

fn thread_list_query_params(
    params: ThreadsListParams,
) -> Result<Vec<(&'static str, String)>, JsonRpcErrorObject> {
    let mut query = Vec::new();
    if let Some(value) = optional_query_value("limit", params.limit)? {
        query.push(("limit", value));
    }
    if let Some(value) = optional_query_value("offset", params.offset)? {
        query.push(("offset", value));
    }
    Ok(query)
}

fn optional_query_value(
    name: &str,
    value: Option<Value>,
) -> Result<Option<String>, JsonRpcErrorObject> {
    let Some(value) = value else {
        return Ok(None);
    };
    match value {
        Value::Null => Ok(None),
        Value::Number(number) => Ok(Some(number.to_string())),
        Value::String(text) => Ok(Some(text)),
        _ => Err(JsonRpcErrorObject {
            code: -32602,
            message: format!("Invalid params: {name} must be a string or number"),
            data: None,
        }),
    }
}

async fn send_get(
    client: &Client,
    base_url: &Url,
    endpoint: &str,
) -> Result<Value, JsonRpcErrorObject> {
    let url = endpoint_url(base_url, endpoint)?;
    let response = client.get(url).send().await.map_err(transport_error)?;
    response_json_or_error(response).await
}

async fn send_get_with_query(
    client: &Client,
    base_url: &Url,
    endpoint: &str,
    query: Vec<(&'static str, String)>,
) -> Result<Value, JsonRpcErrorObject> {
    let mut url = endpoint_url(base_url, endpoint)?;
    if !query.is_empty() {
        {
            let mut query_pairs = url.query_pairs_mut();
            for (key, value) in query {
                query_pairs.append_pair(key, &value);
            }
        }
    }
    let response = client.get(url).send().await.map_err(transport_error)?;
    response_json_or_error(response).await
}

async fn send_post(
    client: &Client,
    base_url: &Url,
    endpoint: &str,
    control_token: &str,
    controller_token: Option<&str>,
    body: Value,
) -> Result<Value, JsonRpcErrorObject> {
    let url = endpoint_url(base_url, endpoint)?;
    let request = client.post(url).json(&body);
    let request = apply_control_headers(request, control_token, controller_token);
    let response = request.send().await.map_err(transport_error)?;
    response_json_or_error(response).await
}

fn apply_control_headers(
    request: RequestBuilder,
    control_token: &str,
    controller_token: Option<&str>,
) -> RequestBuilder {
    let request = request.header("x-libra-control-token", control_token);
    if let Some(controller_token) = controller_token {
        request.header("x-code-controller-token", controller_token)
    } else {
        request
    }
}

async fn response_json_or_error(response: reqwest::Response) -> Result<Value, JsonRpcErrorObject> {
    let status = response.status();
    let body = response.text().await.map_err(transport_error)?;
    let parsed = if body.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str::<Value>(&body).map_err(|error| JsonRpcErrorObject {
            code: -32603,
            message: format!("HTTP response was not valid JSON: {error}"),
            data: Some(json!({ "status": status.as_u16() })),
        })?
    };

    if status.is_success() {
        return Ok(parsed);
    }

    let libra_error = parsed.get("error");
    let libra_code = libra_error
        .and_then(|value| value.get("code"))
        .and_then(Value::as_str)
        .unwrap_or("HTTP_ERROR");
    let default_message = status.canonical_reason().unwrap_or("HTTP request failed");
    let libra_message = libra_error
        .and_then(|value| value.get("message"))
        .and_then(Value::as_str)
        .unwrap_or(default_message);
    Err(JsonRpcErrorObject {
        code: -32000,
        message: libra_message.to_string(),
        data: Some(json!({
            "status": status.as_u16(),
            "code": libra_code,
        })),
    })
}

fn transport_error(error: reqwest::Error) -> JsonRpcErrorObject {
    JsonRpcErrorObject {
        code: -32001,
        message: format!("Transport error: {error}"),
        data: None,
    }
}

fn endpoint_url(base_url: &Url, endpoint: &str) -> Result<Url, JsonRpcErrorObject> {
    let mut url = base_url.clone();
    let base_path = url.path().trim_end_matches('/');
    let endpoint = endpoint.trim_start_matches('/');
    url.set_path(&format!("{base_path}/{endpoint}"));
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

async fn stream_events(client: &Client, base_url: &Url) -> CliResult<()> {
    let url = endpoint_url(base_url, "/api/code/events").map_err(|error| {
        CliError::fatal(format!(
            "failed to build events endpoint URL: {}",
            error.message
        ))
    })?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| CliError::fatal(format!("failed to subscribe to events: {error}")))?;
    if response.status() != StatusCode::OK {
        let status = response.status();
        let body = match response.text().await {
            Ok(body) => body,
            Err(error) => format!("failed to read error body: {error}"),
        };
        return Err(CliError::fatal(format!(
            "events.subscribe failed with HTTP {}: {}",
            status.as_u16(),
            body
        )));
    }

    let mut parser = SseParser::default();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| {
            CliError::fatal(format!("failed to read SSE event stream: {error}"))
        })?;
        for notification in parser.push(&chunk) {
            write_json_value(&notification)?;
        }
    }
    for notification in parser.finish() {
        write_json_value(&notification)?;
    }
    Ok(())
}

#[derive(Default)]
struct SseParser {
    pending: Vec<u8>,
    event_name: Option<String>,
    data_lines: Vec<String>,
}

impl SseParser {
    fn push(&mut self, chunk: &[u8]) -> Vec<Value> {
        self.pending.extend_from_slice(chunk);
        let mut notifications = Vec::new();
        while let Some(newline) = self.pending.iter().position(|byte| *byte == b'\n') {
            let line = self.pending.drain(..=newline).collect::<Vec<_>>();
            if let Some(notification) = self.process_line(&line) {
                notifications.push(notification);
            }
        }
        notifications
    }

    fn finish(&mut self) -> Vec<Value> {
        if !self.pending.is_empty() {
            let line = std::mem::take(&mut self.pending);
            if let Some(notification) = self.process_line(&line) {
                return vec![notification];
            }
        }
        self.dispatch_event().into_iter().collect()
    }

    fn process_line(&mut self, raw_line: &[u8]) -> Option<Value> {
        let mut line = String::from_utf8_lossy(raw_line).to_string();
        while line.ends_with('\n') || line.ends_with('\r') {
            line.pop();
        }
        if line.is_empty() {
            return self.dispatch_event();
        }
        if let Some(event) = line.strip_prefix("event:") {
            self.event_name = Some(event.trim().to_string());
        } else if let Some(data) = line.strip_prefix("data:") {
            self.data_lines.push(data.trim_start().to_string());
        }
        None
    }

    fn dispatch_event(&mut self) -> Option<Value> {
        if self.event_name.is_none() && self.data_lines.is_empty() {
            return None;
        }
        let event = self
            .event_name
            .take()
            .unwrap_or_else(|| "message".to_string());
        let data = self.data_lines.join("\n");
        self.data_lines.clear();
        let data = match serde_json::from_str::<Value>(&data) {
            Ok(value) => value,
            Err(_) => Value::String(data),
        };
        Some(json!({
            "jsonrpc": "2.0",
            "method": "events.notification",
            "params": {
                "event": event,
                "data": data,
            }
        }))
    }
}

fn json_rpc_success(id: Value, result: Value) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0",
        result: Some(result),
        error: None,
        id,
    }
}

fn json_rpc_error(id: Value, error: JsonRpcErrorObject) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0",
        result: None,
        error: Some(error),
        id,
    }
}

fn write_json_rpc_response(response: &JsonRpcResponse) -> CliResult<()> {
    write_json_value(&serde_json::to_value(response).map_err(|error| {
        CliError::fatal(format!("failed to serialize JSON-RPC response: {error}"))
    })?)
}

fn write_json_value(value: &Value) -> CliResult<()> {
    let line = serde_json::to_string(value)
        .map_err(|error| CliError::fatal(format!("failed to serialize JSON output: {error}")))?;
    let mut stdout = io::stdout().lock();
    stdout
        .write_all(line.as_bytes())
        .and_then(|_| stdout.write_all(b"\n"))
        .and_then(|_| stdout.flush())
        .map_err(|error| CliError::fatal(format!("failed to write JSON output: {error}")))
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, net::SocketAddr, sync::Arc};

    use axum::{
        Json, Router,
        extract::{Query, State},
        http::HeaderMap,
        routing::{get, post},
    };
    use tokio::sync::{Mutex, oneshot};

    use super::*;

    #[test]
    fn malformed_json_maps_to_parse_error() {
        let error = parse_json_rpc_request("{not-json").unwrap_err();

        assert_eq!(error.code, -32700);
    }

    #[test]
    fn sse_parser_emits_json_rpc_notifications() {
        let mut parser = SseParser::default();

        let output = parser
            .push(b"event: session_updated\ndata: {\"seq\":1,\"type\":\"session_updated\"}\n\n");

        assert_eq!(output.len(), 1);
        assert_eq!(output[0]["method"], "events.notification");
        assert_eq!(output[0]["params"]["event"], "session_updated");
        assert_eq!(output[0]["params"]["data"]["seq"], 1);
    }

    #[tokio::test]
    async fn json_rpc_dispatch_maps_threads_list_to_http() {
        #[derive(Default)]
        struct MockState {
            calls: Mutex<Vec<Value>>,
        }

        async fn threads(
            State(state): State<Arc<MockState>>,
            Query(query): Query<HashMap<String, String>>,
        ) -> Json<Value> {
            state
                .calls
                .lock()
                .await
                .push(json!({ "path": "threads", "query": query }));
            Json(json!({
                "items": [
                    {
                        "id": "thread-1",
                        "title": "Investigate",
                        "archived": false,
                        "currentIntentId": null,
                        "createdAt": "2026-04-30T00:00:00Z",
                        "updatedAt": "2026-04-30T00:01:00Z"
                    }
                ]
            }))
        }

        let state = Arc::new(MockState::default());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/api/code/threads", get(threads))
            .with_state(state.clone());
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await
        });
        let base_url = Url::parse(&format!("http://{addr}")).unwrap();
        let client = Client::new();

        let response = dispatch_json_rpc_request(
            &client,
            &base_url,
            "process-token",
            JsonRpcRequest {
                jsonrpc: Some("2.0".to_string()),
                method: Some("threads.list".to_string()),
                params: Some(json!({ "limit": 2, "offset": "4" })),
                id: Some(json!(1)),
            },
        )
        .await;

        match response {
            DispatchResult::Response(response) => {
                assert_eq!(response.result.unwrap()["items"][0]["id"], "thread-1");
            }
            DispatchResult::Error(error) => {
                panic!("threads.list returned error: {}", error.message)
            }
            DispatchResult::NotificationOnly | DispatchResult::Subscribe { .. } => {
                panic!("threads.list must return a JSON-RPC response")
            }
        }
        let calls = state.calls.lock().await.clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["query"]["limit"], "2");
        assert_eq!(calls[0]["query"]["offset"], "4");

        let _ = shutdown_tx.send(());
        let _ = server.await;
    }

    #[tokio::test]
    async fn json_rpc_dispatch_maps_attach_submit_and_detach_to_http() {
        #[derive(Default)]
        struct MockState {
            calls: Mutex<Vec<Value>>,
        }

        async fn attach(
            State(state): State<Arc<MockState>>,
            headers: HeaderMap,
            Json(body): Json<Value>,
        ) -> Json<Value> {
            state
                .calls
                .lock()
                .await
                .push(json!({ "path": "attach", "token": headers.get("x-libra-control-token").and_then(|value| value.to_str().ok()), "body": body }));
            Json(json!({
                "controllerToken": "lease-token",
                "leaseExpiresAt": "2026-04-30T00:00:00Z",
                "controller": { "kind": "automation", "canWrite": true, "loopbackOnly": true }
            }))
        }

        async fn messages(
            State(state): State<Arc<MockState>>,
            headers: HeaderMap,
            Json(body): Json<Value>,
        ) -> Json<Value> {
            state
                .calls
                .lock()
                .await
                .push(json!({ "path": "messages", "token": headers.get("x-libra-control-token").and_then(|value| value.to_str().ok()), "controller": headers.get("x-code-controller-token").and_then(|value| value.to_str().ok()), "body": body }));
            Json(json!({ "accepted": true }))
        }

        async fn task_dispatch(
            State(state): State<Arc<MockState>>,
            headers: HeaderMap,
            Json(body): Json<Value>,
        ) -> Json<Value> {
            state
                .calls
                .lock()
                .await
                .push(json!({ "path": "task.dispatch", "token": headers.get("x-libra-control-token").and_then(|value| value.to_str().ok()), "controller": headers.get("x-code-controller-token").and_then(|value| value.to_str().ok()), "body": body }));
            Json(json!({ "accepted": true, "result": "Task `task-1` completed" }))
        }

        async fn detach(
            State(state): State<Arc<MockState>>,
            headers: HeaderMap,
            Json(body): Json<Value>,
        ) -> Json<Value> {
            state
                .calls
                .lock()
                .await
                .push(json!({ "path": "detach", "token": headers.get("x-libra-control-token").and_then(|value| value.to_str().ok()), "controller": headers.get("x-code-controller-token").and_then(|value| value.to_str().ok()), "body": body }));
            Json(json!({ "detached": true }))
        }

        let state = Arc::new(MockState::default());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/api/code/controller/attach", post(attach))
            .route("/api/code/messages", post(messages))
            .route("/api/code/task/dispatch", post(task_dispatch))
            .route("/api/code/controller/detach", post(detach))
            .route("/api/code/session", get(|| async { Json(json!({})) }))
            .with_state(state.clone());
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await
        });
        let base_url = Url::parse(&format!("http://{addr}")).unwrap();
        let client = Client::new();

        let attach_response = dispatch_json_rpc_request(
            &client,
            &base_url,
            "process-token",
            JsonRpcRequest {
                jsonrpc: Some("2.0".to_string()),
                method: Some("controller.attach".to_string()),
                params: Some(json!({ "clientId": "test-client", "kind": "automation" })),
                id: Some(json!(1)),
            },
        )
        .await;
        assert!(matches!(attach_response, DispatchResult::Response(_)));

        let submit_response = dispatch_json_rpc_request(
            &client,
            &base_url,
            "process-token",
            JsonRpcRequest {
                jsonrpc: Some("2.0".to_string()),
                method: Some("message.submit".to_string()),
                params: Some(json!({ "text": "hello", "controllerToken": "lease-token" })),
                id: Some(json!(2)),
            },
        )
        .await;
        assert!(matches!(submit_response, DispatchResult::Response(_)));

        let task_response = dispatch_json_rpc_request(
            &client,
            &base_url,
            "process-token",
            JsonRpcRequest {
                jsonrpc: Some("2.0".to_string()),
                method: Some("task.dispatch".to_string()),
                params: Some(json!({
                    "agent": "explorer",
                    "prompt": "grep TODO src/",
                    "controllerToken": "lease-token"
                })),
                id: Some(json!(4)),
            },
        )
        .await;
        assert!(matches!(task_response, DispatchResult::Response(_)));

        let detach_response = dispatch_json_rpc_request(
            &client,
            &base_url,
            "process-token",
            JsonRpcRequest {
                jsonrpc: Some("2.0".to_string()),
                method: Some("controller.detach".to_string()),
                params: Some(
                    json!({ "clientId": "test-client", "controllerToken": "lease-token" }),
                ),
                id: Some(json!(3)),
            },
        )
        .await;
        assert!(matches!(detach_response, DispatchResult::Response(_)));

        let calls = state.calls.lock().await.clone();
        assert_eq!(calls.len(), 4);
        assert_eq!(calls[0]["path"], "attach");
        assert_eq!(calls[0]["token"], "process-token");
        assert_eq!(calls[1]["path"], "messages");
        assert_eq!(calls[1]["controller"], "lease-token");
        assert_eq!(calls[2]["path"], "task.dispatch");
        assert_eq!(calls[2]["controller"], "lease-token");
        assert_eq!(calls[2]["body"]["agent"], "explorer");
        assert_eq!(calls[2]["body"]["prompt"], "grep TODO src/");
        assert_eq!(calls[3]["path"], "detach");

        let _ = shutdown_tx.send(());
        let _ = server.await;
    }
}
