//! Wave 9 / PR 9 — `libra code` MCP entry-point coverage (§5.14,
//! partial).
//!
//! Coverage included here:
//!   * **Item 1 — automation discovery**: after `libra code`
//!     starts, the runtime writes the MCP server URL into
//!     `--control-info-file` so a downstream automation client
//!     can discover the MCP endpoint without scraping logs.
//!     The harness now parses `mcpUrl` from `control.json` and
//!     this test asserts (a) the field is populated, (b) it
//!     points at a loopback `http://127.0.0.1:<port>/mcp`-style
//!     URL, (c) the `<port>` differs from the web port (the
//!     runtime requires the two to be distinct outside `--stdio`
//!     mode, see `code.rs:3354` "Web and MCP ports must differ").
//!   * **Item 2 — `--stdio` mutex**: clap-level mutual exclusion
//!     of `--stdio` and `--web-only`. Pins that the conflict is
//!     surfaced as a usage error before any runtime work runs.
//!
//!   * **Item 3 — dual-reachability smoke**: same `libra code`
//!     process responds on BOTH the web HTTP transport
//!     (`/api/code/session`) AND the MCP Streamable HTTP
//!     transport (`<mcpUrl>` POST `initialize`). Proves the two
//!     entry points share a process.
//!   * **Item 3 — web→MCP consistency**: a message submitted
//!     through web `/messages` is observed by a live web SSE
//!     subscriber and is then visible through MCP `tools/call`
//!     `list_tasks` on the same process.
//!
//! Coverage deferred (still §5.14 P1 work):
//!   * Item 3 remaining direction — MCP-originated `tools/call`
//!     writes are not currently wired into Code UI transcript
//!     broadcasting, so MCP write → web SSE observe remains a
//!     roadmap-sized follow-up.

#[cfg(feature = "test-provider")]
mod harness;

#[cfg(feature = "test-provider")]
use std::{
    path::PathBuf,
    process::Command,
    thread,
    time::{Duration, Instant},
};

#[cfg(feature = "test-provider")]
use anyhow::{Context, Result, bail};
#[cfg(feature = "test-provider")]
use harness::{CodeSession, CodeSessionOptions};
#[cfg(feature = "test-provider")]
use reqwest::StatusCode;
#[cfg(feature = "test-provider")]
use serde_json::{Value, json};
#[cfg(feature = "test-provider")]
use serial_test::serial;

#[cfg(feature = "test-provider")]
fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/code_ui/basic_chat.json")
}

#[cfg(feature = "test-provider")]
fn libra_bin_path() -> PathBuf {
    std::env::var_os("CARGO_BIN_EXE_libra")
        .map(PathBuf::from)
        .expect("CARGO_BIN_EXE_libra is set for integration tests")
}

#[cfg(feature = "test-provider")]
fn parse_sse_data(sse_text: &str) -> Vec<String> {
    sse_text
        .lines()
        .filter_map(|line| {
            line.strip_prefix("data:")
                .or_else(|| line.strip_prefix("data: "))
                .map(|d| d.trim().to_string())
        })
        .filter(|d| !d.is_empty())
        .collect()
}

#[cfg(feature = "test-provider")]
fn mcp_post(
    client: &reqwest::blocking::Client,
    url: &str,
    session_id: Option<&str>,
    body: &Value,
) -> Result<(StatusCode, String)> {
    let mut request = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "text/event-stream, application/json");
    if let Some(session_id) = session_id {
        request = request.header("Mcp-Session-Id", session_id);
    }

    let response = request
        .json(body)
        .send()
        .with_context(|| format!("MCP POST to {url} failed"))?;
    let status = response.status();
    let body = response
        .text()
        .context("failed to read MCP response body")?;
    Ok((status, body))
}

#[cfg(feature = "test-provider")]
fn first_json_rpc_sse_body(method: &str, body: &str) -> Result<Value> {
    let data = parse_sse_data(body);
    let first = data
        .first()
        .ok_or_else(|| anyhow::anyhow!("MCP {method} response had no SSE data lines: {body}"))?;
    serde_json::from_str(first)
        .with_context(|| format!("failed to parse MCP {method} JSON-RPC result: {first}"))
}

#[cfg(feature = "test-provider")]
fn mcp_initialize(client: &reqwest::blocking::Client, mcp_url: &str) -> Result<String> {
    let initialize = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "libra-code-mcp-dual-entry", "version": "0.0.0" }
        }
    });
    let response = client
        .post(mcp_url)
        .header("Content-Type", "application/json")
        .header("Accept", "text/event-stream, application/json")
        .json(&initialize)
        .send()
        .with_context(|| format!("MCP initialize POST to {mcp_url} failed"))?;
    let status = response.status();
    let session_id = response
        .headers()
        .get("Mcp-Session-Id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let body = response
        .text()
        .context("failed to read MCP initialize body")?;
    if !status.is_success() {
        bail!("MCP initialize returned non-success status {status}: {body}");
    }
    let session_id = session_id.ok_or_else(|| {
        anyhow::anyhow!("MCP initialize did not return Mcp-Session-Id header: {body}")
    })?;
    if session_id.is_empty() {
        bail!("MCP initialize returned an empty Mcp-Session-Id header");
    }

    let init_result = first_json_rpc_sse_body("initialize", &body)?;
    if init_result.get("id") != Some(&Value::from(1)) || init_result.get("result").is_none() {
        bail!("MCP initialize returned malformed JSON-RPC result: {init_result}");
    }

    let initialized = json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized",
        "params": {}
    });
    let (status, body) = mcp_post(client, mcp_url, Some(&session_id), &initialized)
        .context("failed to send MCP initialized notification")?;
    if !status.is_success() {
        bail!("MCP initialized notification failed with {status}: {body}");
    }

    Ok(session_id)
}

#[cfg(feature = "test-provider")]
fn mcp_call_tool(
    client: &reqwest::blocking::Client,
    mcp_url: &str,
    session_id: &str,
    request_id: u64,
    name: &str,
    arguments: Value,
) -> Result<Value> {
    let request = json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": name,
            "arguments": arguments,
        },
        "id": request_id,
    });
    let (status, body) = mcp_post(client, mcp_url, Some(session_id), &request)
        .with_context(|| format!("failed to call MCP tool {name}"))?;
    if !status.is_success() {
        bail!("MCP tools/call {name} failed with {status}: {body}");
    }
    let value = first_json_rpc_sse_body(name, &body)?;
    if value.get("id") != Some(&Value::from(request_id)) || value.get("result").is_none() {
        bail!("MCP tools/call {name} returned malformed JSON-RPC result: {value}");
    }
    Ok(value)
}

#[cfg(feature = "test-provider")]
fn mcp_result_text(value: &Value) -> String {
    value
        .pointer("/result/content")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

#[cfg(feature = "test-provider")]
fn event_payload_transcript_contains(payload: &Value, needle: &str) -> bool {
    payload
        .pointer("/data/transcript")
        .and_then(Value::as_array)
        .is_some_and(|transcript| {
            transcript.iter().any(|entry| {
                let matches = |key: &str| {
                    entry
                        .get(key)
                        .and_then(Value::as_str)
                        .is_some_and(|value| value.contains(needle))
                };
                matches("content") || matches("title")
            })
        })
}

#[cfg(feature = "test-provider")]
fn wait_for_sse_transcript(
    events: &mut harness::EventStream,
    needle: &str,
    timeout: Duration,
) -> Result<Value> {
    let deadline = Instant::now() + timeout;
    let mut last_event = "<none>".to_string();
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let Some(event) = events.next_event(remaining.min(Duration::from_secs(1)))? else {
            continue;
        };
        last_event = format!("event={} data={}", event.event, event.data);
        if event.event != "session_updated" {
            continue;
        }
        let payload: Value = serde_json::from_str(&event.data)
            .with_context(|| format!("failed to parse SSE payload: {}", event.data))?;
        if event_payload_transcript_contains(&payload, needle) {
            return Ok(payload);
        }
    }
    bail!("timed out waiting for SSE transcript to contain {needle:?}; last event: {last_event}")
}

#[cfg(feature = "test-provider")]
fn wait_for_mcp_task(
    client: &reqwest::blocking::Client,
    mcp_url: &str,
    session_id: &str,
    needle: &str,
    timeout: Duration,
) -> Result<String> {
    let deadline = Instant::now() + timeout;
    let mut last_text = String::new();
    let mut request_id = 10_u64;
    while Instant::now() < deadline {
        let value = mcp_call_tool(
            client,
            mcp_url,
            session_id,
            request_id,
            "list_tasks",
            json!({ "limit": 20 }),
        )?;
        let text = mcp_result_text(&value);
        if text.contains(needle) {
            return Ok(text);
        }
        last_text = text;
        request_id += 1;
        thread::sleep(Duration::from_millis(200));
    }
    bail!("timed out waiting for MCP list_tasks to contain {needle:?}; last tasks:\n{last_text}")
}

/// Wave 9 §5.14 item 1 — automation MCP discovery.
///
/// After spawning `libra code`, `control.json` (the file the CLI
/// writes when `--control-info-file` is set) must contain the
/// MCP server's URL so an automation client can find it without
/// log scraping. The harness now parses `mcpUrl` from the
/// runtime-emitted JSON; this test pins that:
///   * The field is populated for a normal spawn (the runtime
///     starts the MCP server alongside the web server).
///   * The URL is a loopback `http://127.0.0.1:<port>/mcp`-style
///     string (the harness already pins `host=127.0.0.1` and the
///     code runtime appends `/mcp` to the bind address).
///   * The MCP port is distinct from the web port — `code.rs`
///     enforces "Web and MCP ports must differ" outside `--stdio`
///     mode, so a regression that collapses them would silently
///     break automation.
#[cfg(feature = "test-provider")]
#[test]
#[serial]
fn libra_code_writes_mcp_url_into_control_info_file() -> Result<()> {
    let session = CodeSession::spawn(CodeSessionOptions::new(
        "code-mcp-control-info",
        fixture_path(),
    ))?;
    let mcp_url = session
        .mcp_url()
        .ok_or_else(|| {
            anyhow::anyhow!("control.json did not surface mcpUrl after libra code spawn")
        })?
        .to_string();

    assert!(
        mcp_url.starts_with("http://127.0.0.1:"),
        "mcpUrl must point at the loopback bind; got {mcp_url:?}",
    );

    // Extract the port segment from `http://127.0.0.1:<port>/...`.
    let after_scheme = mcp_url
        .strip_prefix("http://127.0.0.1:")
        .expect("checked by the assert above");
    let mcp_port_str: String = after_scheme
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    let mcp_port: u16 = mcp_port_str
        .parse()
        .with_context(|| format!("could not parse MCP port from {mcp_url:?}"))?;
    let base_url = session.matrix_attach_url();
    let web_port: u16 = base_url
        .strip_prefix("http://127.0.0.1:")
        .and_then(|tail| tail.split('/').next())
        .and_then(|p| p.parse().ok())
        .ok_or_else(|| anyhow::anyhow!("could not parse web port from base url {base_url}"))?;
    assert_ne!(
        mcp_port, web_port,
        "Web and MCP ports must differ outside --stdio mode (code.rs:3354); both were {mcp_port}",
    );
    Ok(())
}

/// Wave 9 §5.14 item 2 — `--stdio` + `--web-only` mutual
/// exclusion.
///
/// `code.rs:439` declares `pub web_only: bool` with
/// `conflicts_with = "stdio"`. This test pins clap surfaces that
/// conflict as a usage error before the runtime starts, so a
/// future refactor that drops the `conflicts_with` attribute
/// silently breaks the documented mutex.
///
/// Driven via `Command` (no PTY) because the conflict is
/// resolved during arg parsing — neither mode actually starts.
#[cfg(feature = "test-provider")]
#[test]
fn libra_code_stdio_web_only_combo_is_rejected_at_arg_parse() -> Result<()> {
    let output = Command::new(libra_bin_path())
        .args(["code", "--stdio", "--web-only"])
        .output()
        .context("failed to spawn libra code --stdio --web-only")?;
    if output.status.success() {
        bail!(
            "expected --stdio + --web-only to fail at arg parse, but exit was successful;\nstdout: {}\nstderr: {}",
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
        combined.contains("--stdio") && combined.contains("--web-only"),
        "clap conflict error must reference both flags; got:\n{combined}",
    );
    // clap's conflict-resolution error commonly includes the
    // phrase "cannot be used with" or "the argument ... cannot be
    // used with"; assert the keyword "cannot" so any future clap
    // wording change still passes as long as the conflict is
    // reported.
    assert!(
        combined.contains("cannot") || combined.contains("conflicts"),
        "expected a conflict-style error mentioning the mutex; got:\n{combined}",
    );
    Ok(())
}

/// Wave 9 §5.14 item 3 smoke — dual-reachability. After spawn,
/// the same `libra code` process must respond on BOTH:
///   * the web HTTP transport (proven via the existing
///     `session.snapshot()` GET `/api/code/session`), AND
///   * the MCP Streamable HTTP transport (proven via a fresh
///     reqwest POST to `<mcpUrl>` with a JSON-RPC `initialize`
///     payload).
///
/// The MCP transport is gated on the `Mcp-Session-Id` header
/// pattern from `tests/e2e_mcp_flow.rs`: initialize must succeed
/// (status `200 OK` + the response carries an `Mcp-Session-Id`
/// response header). This does NOT walk the full handshake
/// (notifications/initialized + tools/list) — that's covered by
/// `e2e_mcp_flow.rs` already; this test's contribution is
/// proving both surfaces are reachable on the SAME process.
#[cfg(feature = "test-provider")]
#[test]
#[serial]
fn libra_code_serves_both_web_and_mcp_transports_on_same_process() -> Result<()> {
    let session = CodeSession::spawn(CodeSessionOptions::new(
        "code-mcp-dual-reachability",
        fixture_path(),
    ))?;

    // 1. Web reachability — drive the existing snapshot accessor
    //    so the failure mode is identical to other tests that
    //    rely on web HTTP.
    let snapshot = session.snapshot().context("web /api/code/session probe")?;
    assert!(
        snapshot.get("sessionId").and_then(|v| v.as_str()).is_some(),
        "web /api/code/session must surface a sessionId after spawn; got {snapshot:?}",
    );

    // 2. MCP reachability — POST a JSON-RPC initialize to the
    //    Streamable HTTP transport on the same process. The
    //    Mcp-Session-Id response header is the success contract
    //    (per `tests/e2e_mcp_flow.rs:291` "Server did not return
    //    Mcp-Session-Id header on initialize").
    let mcp_url = session
        .mcp_url()
        .ok_or_else(|| anyhow::anyhow!("control.json did not surface mcpUrl after spawn"))?
        .to_string();
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .context("build mcp probe client")?;
    let init_payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "libra-dual-reach-probe", "version": "0.0.0" }
        }
    });
    let response = client
        .post(&mcp_url)
        .header("Content-Type", "application/json")
        .header("Accept", "text/event-stream, application/json")
        .json(&init_payload)
        .send()
        .with_context(|| format!("MCP initialize POST to {mcp_url} failed"))?;
    let status = response.status();
    let session_id = response
        .headers()
        .get("Mcp-Session-Id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    if !status.is_success() {
        let body = response.text().unwrap_or_default();
        bail!("MCP initialize returned non-success status {status}: {body}");
    }
    assert!(
        session_id.is_some_and(|id| !id.is_empty()),
        "MCP initialize must return a non-empty Mcp-Session-Id header so a downstream automation client can continue the handshake",
    );
    Ok(())
}

/// Wave 9 §5.14 item 3 consistency — web write → web SSE +
/// MCP observe.
///
/// The same `libra code` process exposes web `/messages`, web
/// `/events`, and MCP Streamable HTTP. This test drives all three:
///
///   1. initialize an MCP client against the runtime's `mcpUrl`;
///   2. subscribe to web SSE before writing;
///   3. submit a message through the web automation endpoint;
///   4. assert the SSE stream observes that transcript update;
///   5. poll MCP `tools/call list_tasks` until it sees the TUI
///      turn-tracking Task created from the same user text.
///
/// This pins the currently implemented consistency direction.
/// MCP-originated tool writes are still not broadcast into Code UI
/// transcript state, so that opposite direction remains explicit
/// roadmap work rather than an overclaimed test assertion.
#[cfg(feature = "test-provider")]
#[test]
#[serial]
fn web_message_turn_is_observable_through_sse_and_mcp_task_list() -> Result<()> {
    let mut session = CodeSession::spawn(CodeSessionOptions::new(
        "code-mcp-web-message-consistency",
        fixture_path(),
    ))?;
    let mcp_url = session
        .mcp_url()
        .ok_or_else(|| anyhow::anyhow!("control.json did not surface mcpUrl after spawn"))?
        .to_string();
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .context("build MCP consistency client")?;
    let mcp_session_id = mcp_initialize(&client, &mcp_url)?;

    let mut events = session.open_event_stream()?;
    session.attach_automation("code-mcp-web-message-consistency")?;

    let marker = "mcp-dual-web-observe-marker";
    let user_text = format!("/chat {marker}");
    session.submit_message(&user_text)?;
    let _payload = wait_for_sse_transcript(&mut events, marker, Duration::from_secs(10))?;
    let tasks_text = wait_for_mcp_task(
        &client,
        &mcp_url,
        &mcp_session_id,
        marker,
        Duration::from_secs(10),
    )?;
    assert!(
        tasks_text.contains(&format!("TUI: {user_text}")) || tasks_text.contains(marker),
        "MCP list_tasks must expose the web-submitted turn text; got:\n{tasks_text}",
    );
    Ok(())
}

#[cfg(not(feature = "test-provider"))]
#[test]
fn mcp_dual_entry_test_requires_test_provider_feature() {
    eprintln!("skipping mcp dual entry test; enable --features test-provider");
}
