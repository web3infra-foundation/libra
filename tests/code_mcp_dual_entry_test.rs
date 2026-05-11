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
//!     entry points share a process. Full write-and-observe
//!     consistency (one entry writes, the other observes via
//!     SSE) needs richer harness wiring and is split out.
//!
//! Coverage deferred (still §5.14 P1 work):
//!   * Item 3 full — dual-entry consistency where MCP
//!     `tools/call` and web `/messages` both touch the same
//!     thread and SSE observers see both writes. Needs parallel
//!     write/observe coordination on the same CodeSession.

#[cfg(feature = "test-provider")]
mod harness;

#[cfg(feature = "test-provider")]
use std::{path::PathBuf, process::Command};

#[cfg(feature = "test-provider")]
use anyhow::{Context, Result, bail};
#[cfg(feature = "test-provider")]
use harness::{CodeSession, CodeSessionOptions};
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
/// proving both surfaces are reachable on the SAME process,
/// which is the §5.14 item 3 reachability promise.
///
/// Full write-and-observe (MCP write → web SSE observes) is
/// the deferred multi-PR follow-up.
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

#[cfg(not(feature = "test-provider"))]
#[test]
fn mcp_dual_entry_test_requires_test_provider_feature() {
    eprintln!("skipping mcp dual entry test; enable --features test-provider");
}
