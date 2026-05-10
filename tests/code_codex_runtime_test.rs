//! Wave 9 / PR 9 — `libra code --provider codex` runtime
//! coverage (§5.13, partial — CLI surface only).
//!
//! What this test pins:
//!   * `--codex-port 0` is rejected at arg validation with the
//!     documented "must be a non-zero TCP port" error message.
//!     The runtime calls `resolve_codex_ws_url(Some(0))` from
//!     `src/command/code.rs:2212` which surfaces this error
//!     before any WebSocket connection attempt.
//!
//! Coverage deferred (full §5.13 closure):
//!   * End-to-end boot smoke against a `tokio-tungstenite`-based
//!     WebSocket mock that completes the JSON-RPC handshake,
//!     emits Codex notifications, and asserts persistence to
//!     `.libra/objects/`. This needs a substantial WS+JSON-RPC
//!     mock helper alongside the existing
//!     `tests/helpers/mock_codex.rs` (which is raw NDJSON, not
//!     WS-aware), so the §5.13 doc explicitly tags it
//!     "roadmap-sized" — a multi-PR effort split out of this
//!     loop.
//!   * Codex `--plan-mode true` plan-approve gate enforcement.
//!   * Codex disconnect / reconnect resilience.

use std::{path::PathBuf, process::Command};

use anyhow::{Context, Result, bail};

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
