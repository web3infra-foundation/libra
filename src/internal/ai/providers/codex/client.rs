//! Codex WebSocket client for Libra.

use std::fmt;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, Mutex};
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
    fn on_request(&self, _request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        unimplemented!("WebSocket client does not use reqwest")
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
}

impl std::fmt::Debug for CodexWebSocket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
        }
    }

    pub async fn connect(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (ws_stream, _) = connect_async(CODEX_WS_URL).await?;
        let (mut write, mut read) = ws_stream.split();

        let responses = self.responses.clone();
        let agent_messages = self.agent_messages.clone();

        let (tx, mut rx) = mpsc::channel::<Message>(100);

        {
            let mut sender = self.sender.lock().await;
            *sender = Some(tx.clone());
        }

        let mut write = write;
        tokio::spawn(async move {
            while let Some(message) = rx.recv().await {
                if write.send(message).await.is_err() {
                    break;
                }
            }
        });

        let responses_clone = responses;
        let agent_messages_clone = agent_messages;
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
                                if method_str.contains("agent_message") {
                                    if let Some(params) = json.get("params") {
                                        if let Some(msg_obj) = params.get("msg") {
                                            if let Some(delta) = msg_obj.get("delta").and_then(|d| d.as_str()) {
                                                eprintln!("[Codex] Got agent message: {}", delta);
                                                let mut msgs = agent_messages_clone.lock().await;
                                                msgs.push(delta.to_string());
                                            }
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

        let _init_result = self.send_request("initialize", serde_json::json!({
            "protocolVersion": "1.0",
            "capabilities": {},
            "clientInfo": {
                "name": "libra",
                "version": "1.0.0"
            }
        })).await;

        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        let thread_id_clone = self.thread_id.clone();
        let thread_result = self.send_request("thread/start", serde_json::json!({})).await;

        if let Ok(result) = thread_result {
            if let Some(result_obj) = result.get("result") {
                if let Some(thread_id_val) = result_obj.get("threadId").or_else(|| result_obj.get("thread_id")).and_then(|v| v.as_str()) {
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

        for _ in 0..100 {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            let responses = self.responses.lock().await;
            if let Some(response) = responses.get(&id) {
                return Ok(response.clone());
            }
        }

        Ok(serde_json::json!({"error": "timeout"}))
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

        let result = self.send_request(method, params_with_thread).await?;

        tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

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
        }
    }
}
