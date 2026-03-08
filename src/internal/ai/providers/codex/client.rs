//! Codex WebSocket client for Libra.

use std::{fmt, sync::Arc};

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::internal::ai::client::Provider;

const CODEX_WS_URL: &str = "ws://127.0.0.1:8080";

#[derive(Clone)]
pub struct CodexProvider {
    api_key: String,
}

impl fmt::Debug for CodexProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CodexProvider")
            .field("api_key", &"***")
            .finish()
    }
}

impl CodexProvider {
    pub fn new(api_key: String) -> Self {
        Self { api_key }
    }

    pub fn api_key(&self) -> &str {
        &self.api_key
    }
}

impl Provider for CodexProvider {
    fn on_request(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        // Codex uses WebSocket, but this hook is called for shared HTTP paths.
        // Return unchanged to avoid panicking.
        request
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexMessage {
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<u64>,
    pub method: Option<String>,
    pub params: Option<serde_json::Value>,
    pub result: Option<serde_json::Value>,
    pub error: Option<serde_json::Value>,
}

impl CodexMessage {
    pub fn new_request(id: u64, method: &str, params: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: Some(id),
            method: Some(method.to_string()),
            params: Some(params),
            result: None,
            error: None,
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

pub struct CodexWebSocket {
    provider: CodexProvider,
    sender: Arc<Mutex<Option<mpsc::Sender<Message>>>>,
    responses: Arc<Mutex<std::collections::HashMap<u64, serde_json::Value>>>,
    thread_id: Arc<Mutex<Option<String>>>,
    agent_messages: Arc<Mutex<Vec<String>>>,
    completion_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    approval_tx: Arc<Mutex<Option<mpsc::Sender<serde_json::Value>>>>,
}

impl std::fmt::Debug for CodexWebSocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CodexWebSocket")
            .field("provider", &self.provider)
            .finish()
    }
}

impl CodexWebSocket {
    pub fn new(api_key: String) -> Self {
        Self {
            provider: CodexProvider::new(api_key),
            sender: Arc::new(Mutex::new(None)),
            responses: Arc::new(Mutex::new(std::collections::HashMap::new())),
            thread_id: Arc::new(Mutex::new(None)),
            agent_messages: Arc::new(Mutex::new(Vec::new())),
            completion_tx: Arc::new(Mutex::new(None)),
            approval_tx: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn connect(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (ws_stream, _) = connect_async(CODEX_WS_URL).await?;
        let (mut write, mut read) = ws_stream.split();

        let responses = self.responses.clone();
        let agent_messages = self.agent_messages.clone();
        let completion_tx = self.completion_tx.clone();
        let approval_tx = self.approval_tx.clone();
        let sender_clone = self.sender.clone();

        let (tx, mut rx) = mpsc::channel::<Message>(100);

        {
            let mut sender = self.sender.lock().await;
            *sender = Some(tx.clone());
        }

        tokio::spawn(async move {
            while let Some(message) = rx.recv().await {
                if write.send(message).await.is_err() {
                    break;
                }
            }
        });

        let responses_clone = responses;
        let agent_messages_clone = agent_messages;
        let completion_tx_clone = completion_tx;
        let _approval_tx_clone = approval_tx;
        let sender_for_approval = sender_clone.clone();
        tokio::spawn(async move {
            while let Some(msg) = read.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                            if let Some(id_val) = json.get("id") {
                                if let Some(id) = id_val.as_u64() {
                                    let mut responses = responses_clone.lock().await;
                                    responses.insert(id, json);
                                }
                            } else if let Some(method) = json.get("method") {
                                let method_str = method.as_str().unwrap_or("");
                                if method_str.contains("agent_message")
                                    && let Some(params) = json.get("params")
                                    && let Some(msg_obj) = params.get("msg")
                                    && let Some(delta) =
                                        msg_obj.get("delta").and_then(|d| d.as_str())
                                {
                                    let mut msgs = agent_messages_clone.lock().await;
                                    msgs.clear();
                                    msgs.push(delta.to_string());
                                }
                                // Handle turn completion
                                if method_str.contains("turn/completed")
                                    || method_str.contains("turnCompleted")
                                {
                                    // eprintln!("[Codex] Turn completed notification received");
                                    if let Some(tx) = completion_tx_clone.lock().await.take() {
                                        let _ = tx.send(());
                                    }
                                }
                                // Handle request approval
                                if method_str.contains("requestApproval") {
                                    // eprintln!("[Codex] Request approval received: {}", method_str);
                                    if let Some(params) = json.get("params") {
                                        let request_id = params.get("requestId").cloned();
                                        // Send approval immediately
                                        let msg = CodexMessage::new_request(
                                            0,
                                            "requestApproval/resolve",
                                            serde_json::json!({
                                                "requestId": request_id,
                                                "approved": true
                                            }),
                                        );
                                        if let Some(ref sender) = *sender_for_approval.lock().await
                                        {
                                            // eprintln!("[Codex] Sending auto-approval");
                                            let _ = sender.send(Message::Text(msg.to_json())).await;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Ok(Message::Close(_)) => {
                        break;
                    }
                    Err(_) => {
                        break;
                    }
                    _ => {}
                }
            }
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        let _init_result = self
            .send_request(
                "initialize",
                serde_json::json!({
                    "protocolVersion": "1.0",
                    "capabilities": {},
                    "clientInfo": {
                        "name": "libra",
                        "version": "1.0.0"
                    }
                }),
            )
            .await;

        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        let thread_id_clone = self.thread_id.clone();
        let thread_result = self
            .send_request("thread/start", serde_json::json!({}))
            .await;

        #[allow(clippy::collapsible_if, clippy::collapsible_else_if)]
        if let Ok(result) = thread_result {
            if let Some(result_obj) = result.get("result") {
                if let Some(thread_id_val) = result_obj
                    .get("threadId")
                    .or_else(|| result_obj.get("thread_id"))
                    .and_then(|v| v.as_str())
                {
                    let mut tid = thread_id_clone.lock().await;
                    *tid = Some(thread_id_val.to_string());
                } else {
                    if let Some(thread_obj) = result_obj.get("thread") {
                        if let Some(thread_id_val) = thread_obj.get("id") {
                            if let Some(thread_id) = thread_id_val.as_str() {
                                let mut tid = thread_id_clone.lock().await;
                                *tid = Some(thread_id.to_string());
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    pub async fn send_request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static REQUEST_ID: AtomicU64 = AtomicU64::new(1);
        let id = REQUEST_ID.fetch_add(1, Ordering::Relaxed);

        let message = CodexMessage::new_request(id, method, params);
        let json = message.to_json();

        let sender = self.sender.lock().await;
        if let Some(ref tx) = *sender {
            tx.send(Message::Text(json)).await?;
        } else {
            return Err("WebSocket not connected".into());
        }

        for _ in 0..600 {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            let mut responses = self.responses.lock().await;
            // Use remove() instead of get() to prevent memory leaks
            // Each request's response is consumed after reading
            if let Some(response) = responses.remove(&id) {
                // Check if response contains an error
                if let Some(error_obj) = response.get("error") {
                    return Err(format!("WebSocket error: {}", error_obj).into());
                }
                return Ok(response);
            }
        }

        // Return error on timeout instead of Ok with error payload
        Err("Request timeout: no response received after 60 seconds".into())
    }

    pub async fn send_request_with_thread(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
        let thread_id = {
            let guard = self.thread_id.lock().await;
            guard.clone()
        };

        let mut params_with_thread = serde_json::json!(params);
        if let Some(ref tid) = thread_id {
            params_with_thread["threadId"] = serde_json::json!(tid);
        } else {
            return Err("No thread ID available".into());
        }

        // Create oneshot channel for completion signal
        let (tx, _rx) = oneshot::channel();
        {
            let mut completion = self.completion_tx.lock().await;
            *completion = Some(tx);
        }

        // Clear previous messages
        {
            let mut msgs = self.agent_messages.lock().await;
            msgs.clear();
        }

        let result = self.send_request(method, params_with_thread).await?;

        // Wait for completion signal with timeout (120 seconds)
        // let timeout = tokio::time::timeout(tokio::time::Duration::from_secs(120), rx);
        // match timeout.await {
        //     Ok(Ok(())) => {
        //         // eprintln!("[Codex] Received completion signal, waiting for turn to complete...");
        //     }
        //     Ok(Err(_)) => {
        //         // eprintln!("[Codex] Completion channel closed without signal");
        //     }
        //     Err(_) => {
        //         // eprintln!("[Codex] Timeout waiting for completion");
        //     }
        // }

        // Poll thread/read until turn is completed (max 5 minutes)
        for _i in 0..600 {
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

            let turn_result = self
                .send_request(
                    "thread/read",
                    serde_json::json!({
                        "includeTurns": true,
                        "threadId": thread_id
                    }),
                )
                .await;

            if let Ok(turn_result) = turn_result {
                // Check result.thread.turns[-1].status (from thread/read API)
                let status = turn_result
                    .get("result")
                    .and_then(|r| r.get("thread"))
                    .and_then(|t| t.get("turns"))
                    .and_then(|arr| arr.as_array())
                    .and_then(|arr| arr.last())
                    .and_then(|last_turn| last_turn.get("status"))
                    .and_then(|s| s.as_str());

                // eprintln!("[Codex] Turn status ({}): {:?}", i, status);

                if status == Some("completed") {
                    return Ok(turn_result);
                }
            }
        }

        Ok(result)
    }

    pub async fn get_agent_messages(&self) -> Vec<String> {
        let msgs = self.agent_messages.lock().await;
        msgs.clone()
    }

    pub async fn clear_agent_messages(&self) {
        let mut msgs = self.agent_messages.lock().await;
        msgs.clear();
    }
}

pub type Client = CodexWebSocket;

impl Client {
    pub fn from_env() -> Result<Self, std::env::VarError> {
        let api_key = std::env::var("OPENAI_API_KEY")?;
        Ok(Self::new(api_key))
    }

    pub fn with_api_key(api_key: String) -> Self {
        Self::new(api_key)
    }
}

impl Clone for CodexWebSocket {
    fn clone(&self) -> Self {
        Self {
            provider: self.provider.clone(),
            sender: self.sender.clone(),
            responses: self.responses.clone(),
            thread_id: self.thread_id.clone(),
            agent_messages: self.agent_messages.clone(),
            completion_tx: self.completion_tx.clone(),
            approval_tx: self.approval_tx.clone(),
        }
    }
}
