//! Wave 9 / PR 9 — `libra code --provider codex` runtime
//! coverage (§5.13).
//!
//! What this test pins:
//!   * `--codex-port 0` is rejected at arg validation with the
//!     documented "must be a non-zero TCP port" error message.
//!     The runtime calls `resolve_codex_ws_url(Some(0))` from
//!     `src/command/code.rs:2212` which surfaces this error
//!     before any WebSocket connection attempt.
//!   * The shared `MockCodexWsServer` helper (lives at
//!     `tests/helpers/mock_codex_ws_server.rs`) accepts the
//!     `tokio-tungstenite` WebSocket handshake, parses incoming
//!     JSON-RPC requests, and replies with a method-aware
//!     success envelope (`initialize` → `{}`, `thread/start` →
//!     `{ thread: { id: ... } }`). This is the foundation a
//!     follow-up PR will use to drive a full
//!     `libra code --provider codex` boot end-to-end against the
//!     mock; landing the helper now de-risks the protocol
//!     plumbing for that future work.
//!
//! Coverage still deferred (extends the same helper):
//!   * End-to-end `libra code --provider codex --codex-port
//!     <mock>` boot that completes the handshake, receives
//!     notifications, and asserts persistence to
//!     `.libra/objects/`. Needs the binary's
//!     `start_managed_codex_server` indirection swapped for the
//!     mock — a separate PR worth.
//!   * Codex `--plan-mode true` plan-approve gate enforcement.
//!   * Codex disconnect / reconnect resilience.

mod helpers;

use std::{path::PathBuf, process::Command, time::Duration};

use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use helpers::mock_codex_ws_server::{MockCodexWsConfig, MockCodexWsServer};
use serde_json::json;
use tokio_tungstenite::tungstenite::Message;

fn libra_bin_path() -> PathBuf {
    std::env::var_os("CARGO_BIN_EXE_libra")
        .map(PathBuf::from)
        .expect("CARGO_BIN_EXE_libra is set for integration tests")
}

/// Initialise a libra repo in a tempdir so `libra code` does not
/// trip the LBR-REPO-001 precondition before reaching the
/// `--codex-port` validator.
fn init_libra_repo() -> Result<tempfile::TempDir> {
    let temp = tempfile::Builder::new()
        .prefix("code-codex-runtime-")
        .tempdir()
        .context("failed to create codex test tempdir")?;
    let repo_dir = temp.path().to_path_buf();
    let output = Command::new(libra_bin_path())
        .args(["init", "--vault=false", "--quiet"])
        .arg(&repo_dir)
        .output()
        .context("failed to run 'libra init'")?;
    if !output.status.success() {
        bail!(
            "libra init failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
    Ok(temp)
}

/// Wave 9 §5.13 partial — `--codex-port 0` is rejected at arg
/// validation. Driven via `Command::new(libra)` because the
/// rejection happens during arg parsing / preflight, before any
/// runtime work runs (no PTY, no WebSocket connection attempt).
///
/// Pins the documented error message at
/// `src/command/code.rs:2216` ("--codex-port must be a non-zero
/// TCP port; omit it to auto-select a free port") so a future
/// validator change cannot silently downgrade the user-facing
/// guidance.
#[test]
fn libra_code_codex_port_zero_is_rejected_at_arg_validation() -> Result<()> {
    let repo = init_libra_repo()?;
    let output = Command::new(libra_bin_path())
        .arg("code")
        .arg("--cwd")
        .arg(repo.path())
        .args(["--provider", "codex", "--codex-port", "0"])
        .output()
        .context("failed to spawn libra code --provider codex --codex-port 0")?;
    if output.status.success() {
        bail!(
            "expected --codex-port 0 to fail validation, but exit was successful;\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        combined.contains("--codex-port") && combined.contains("non-zero"),
        "expected the documented '--codex-port must be a non-zero TCP port' error; got:\n{combined}",
    );
    Ok(())
}

/// Wave 9 §5.13 foundation — `MockCodexWsServer` round-trip
/// smoke.
///
/// Spawns the mock on `127.0.0.1:0`, opens a `tokio-tungstenite`
/// WebSocket client (mirroring how libra's Codex client calls
/// `connect_async`), and:
///
///   1. Sends a JSON-RPC `initialize` request and asserts the
///      mock responds with a success envelope keyed off the
///      request id.
///   2. Sends a JSON-RPC `thread/start` request and asserts the
///      response carries the configured thread id at
///      `result.thread.id` (matching the real Codex server's
///      shape that libra's runtime extracts via the
///      `thread.id` / `threadId` / `thread_id` fallback chain
///      at `src/internal/ai/codex/mod.rs:6200`).
///   3. Asserts both requests appear in `captured_requests()`
///      so a follow-up PR can drive the libra binary against
///      this same mock and verify the protocol payload shape.
#[tokio::test]
async fn mock_codex_ws_server_handles_initialize_and_thread_start_round_trip() -> Result<()> {
    let server = MockCodexWsServer::start(MockCodexWsConfig {
        thread_id: Some("wave-9-§5-13-thread".to_string()),
    })
    .await?;
    let url = server.ws_url();
    let (ws_stream, _response) = tokio::time::timeout(
        Duration::from_secs(5),
        tokio_tungstenite::connect_async(url.as_str()),
    )
    .await
    .context("ws connect timed out after 5s")??;
    let (mut write, mut read) = ws_stream.split();

    // initialize round trip.
    let init_payload = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "clientInfo": { "name": "libra-test", "version": "0.0.0" },
            "cwd": "/tmp/wave-9-codex"
        }
    });
    write
        .send(Message::Text(init_payload.to_string().into()))
        .await
        .context("send initialize")?;
    let init_resp_text = match tokio::time::timeout(Duration::from_secs(5), read.next())
        .await
        .context("await initialize response")?
    {
        Some(Ok(Message::Text(text))) => text.to_string(),
        Some(Ok(other)) => bail!("unexpected initialize response frame: {other:?}"),
        Some(Err(err)) => bail!("ws error reading initialize response: {err}"),
        None => bail!("mock closed before responding to initialize"),
    };
    let init_resp: serde_json::Value =
        serde_json::from_str(&init_resp_text).context("parse initialize response")?;
    assert_eq!(
        init_resp.get("id").and_then(|v| v.as_u64()),
        Some(1),
        "initialize response must echo the request id; got {init_resp}",
    );
    assert!(
        init_resp.get("result").is_some(),
        "initialize response must carry a result envelope; got {init_resp}",
    );

    // thread/start round trip.
    let thread_payload = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "thread/start",
        "params": {
            "cwd": "/tmp/wave-9-codex",
            "approvalPolicy": "on-request",
        }
    });
    write
        .send(Message::Text(thread_payload.to_string().into()))
        .await
        .context("send thread/start")?;
    let thread_resp_text = match tokio::time::timeout(Duration::from_secs(5), read.next())
        .await
        .context("await thread/start response")?
    {
        Some(Ok(Message::Text(text))) => text.to_string(),
        Some(Ok(other)) => bail!("unexpected thread/start response frame: {other:?}"),
        Some(Err(err)) => bail!("ws error reading thread/start response: {err}"),
        None => bail!("mock closed before responding to thread/start"),
    };
    let thread_resp: serde_json::Value =
        serde_json::from_str(&thread_resp_text).context("parse thread/start response")?;
    assert_eq!(
        thread_resp
            .pointer("/result/thread/id")
            .and_then(|v| v.as_str()),
        Some("wave-9-§5-13-thread"),
        "thread/start response must surface result.thread.id matching the configured value; got {thread_resp}",
    );

    // Captured requests for downstream-PR assertions.
    let captured = server.captured_requests();
    assert_eq!(captured.len(), 2, "expected exactly two captured requests");
    assert_eq!(
        captured[0].get("method").and_then(|v| v.as_str()),
        Some("initialize"),
    );
    assert_eq!(
        captured[1].get("method").and_then(|v| v.as_str()),
        Some("thread/start"),
    );
    Ok(())
}
