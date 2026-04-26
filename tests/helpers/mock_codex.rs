//! Mock Codex HTTP server helpers shared by AI integration tests.
//!
//! Scenario focus: deterministic streaming responses, tool-call events, and provider
//! edge cases without relying on live network services.

use std::{net::SocketAddr, sync::Arc};

use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MockToolCall {
    pub name: String,
    pub arguments_json: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MockCodexTurn {
    pub plan_text: Option<String>,
    pub patch_diff: Option<String>,
    pub tool_calls: Vec<MockToolCall>,
}

pub struct MockCodexServer {
    addr: SocketAddr,
    handle: JoinHandle<()>,
}

impl MockCodexServer {
    pub async fn start(script: Vec<MockCodexTurn>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock codex server");
        let addr = listener.local_addr().expect("read mock codex address");
        let script = Arc::new(script);
        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _peer)) = listener.accept().await else {
                    break;
                };
                let script = Arc::clone(&script);
                tokio::spawn(async move {
                    let mut request = [0_u8; 1024];
                    let _ = stream.read(&mut request).await;
                    for turn in script.iter() {
                        let line = serde_json::to_vec(turn).expect("serialize mock codex turn");
                        if stream.write_all(&line).await.is_err() {
                            break;
                        }
                        if stream.write_all(b"\n").await.is_err() {
                            break;
                        }
                    }
                    let _ = stream.shutdown().await;
                });
            }
        });

        Self { addr, handle }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn ws_url(&self) -> String {
        format!("ws://{}", self.addr)
    }
}

impl Drop for MockCodexServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}
