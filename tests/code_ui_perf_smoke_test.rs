//! Wave 12 / PR 12 — performance smoke tests for the Code UI
//! HTTP surface (§5.18).
//!
//! These tests are `#[ignore]` so a normal `cargo test` skips
//! them; they are intended to be run on demand with:
//!
//! ```bash
//! LIBRA_RUN_PERF=1 cargo test --features test-provider \
//!   --test code_ui_perf_smoke_test -- --ignored --test-threads=1
//! ```
//!
//! Coverage included here:
//!   * `/threads?limit=1` under 10 concurrent in-process clients
//!     completes within a 2-second wall-clock ceiling — pins that
//!     the read path is not serialised behind a coarse lock.
//!
//! Coverage deferred:
//!   * 100 k-line transcript snapshot timing (§5.18 first bullet)
//!     — needs a fixture that can populate a chat session with
//!     synthetic transcript entries; out of scope for this
//!     wave, follow-up needs either a `/admin/seed-transcript`
//!     test-only route or direct `CodeUiSession::push_transcript`
//!     access from outside the crate.
//!   * 5-minute SSE stability soak (§5.18 second bullet) — too
//!     long for an inline `#[ignore]` test; tracked as a
//!     separate nightly job.

#[cfg(feature = "test-provider")]
mod harness;

#[cfg(feature = "test-provider")]
use std::{
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

#[cfg(feature = "test-provider")]
use anyhow::{Context, Result, bail};
#[cfg(feature = "test-provider")]
use harness::{CodeSession, CodeSessionOptions};
#[cfg(feature = "test-provider")]
use serial_test::serial;

#[cfg(feature = "test-provider")]
fn fixture_path() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/code_ui/basic_chat.json")
}

#[cfg(feature = "test-provider")]
fn perf_mode_enabled() -> bool {
    std::env::var("LIBRA_RUN_PERF")
        .ok()
        .as_deref()
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false)
}

/// Drive 10 in-process readers against `/threads?limit=1` and
/// pin a 2 s wall-clock ceiling for ALL of them to complete.
/// The reader path holds no per-request lock that would
/// serialise these against each other; this smoke catches a
/// regression that would silently introduce one.
#[cfg(feature = "test-provider")]
#[test]
#[ignore = "perf smoke; run with LIBRA_RUN_PERF=1"]
#[serial]
fn perf_threads_endpoint_handles_10_concurrent_readers_within_2s() -> Result<()> {
    if !perf_mode_enabled() {
        // The `#[ignore]` already skips by default, but check
        // again so the test fails loud if the runner override
        // pulled it in without the env opt-in.
        bail!(
            "LIBRA_RUN_PERF=1 must be set to run the perf smoke; rerun with `LIBRA_RUN_PERF=1 cargo test --features test-provider --test code_ui_perf_smoke_test -- --ignored --test-threads=1`",
        );
    }
    let session = CodeSession::spawn(CodeSessionOptions::new(
        "perf-threads-concurrent",
        fixture_path(),
    ))?;
    // Per-thread blocking client — `CodeSession` is `!Sync`
    // (PTY writer Box<dyn Write + Send>), so threads can't share
    // the harness's HTTP client; build their own. Same pattern
    // the parallel-attach state case uses.
    let url = format!(
        "{}/threads?limit=1",
        session
            .matrix_attach_url()
            .strip_suffix("/controller/attach")
            .map(str::to_string)
            .unwrap_or_else(|| session.matrix_attach_url())
    );
    let url_arc = Arc::new(url);
    let started = Instant::now();
    let handles: Vec<_> = (0..10)
        .map(|i| {
            let url = url_arc.clone();
            thread::spawn(move || -> Result<()> {
                let client = reqwest::blocking::Client::builder()
                    .timeout(Duration::from_secs(5))
                    .build()
                    .context("build per-thread client")?;
                let response = client
                    .get(url.as_str())
                    .send()
                    .with_context(|| format!("reader {i} GET /threads"))?;
                let status = response.status();
                if !status.is_success() {
                    bail!("reader {i} got non-success status {status}");
                }
                Ok(())
            })
        })
        .collect();
    for (i, h) in handles.into_iter().enumerate() {
        h.join()
            .map_err(|err| anyhow::anyhow!("perf reader thread {i} panicked: {err:?}"))??;
    }
    let elapsed = started.elapsed();
    if elapsed >= Duration::from_secs(2) {
        bail!(
            "10 concurrent /threads readers took {elapsed:?} (>= 2s ceiling); regressed locking?",
        );
    }
    drop(session);
    Ok(())
}

#[cfg(not(feature = "test-provider"))]
#[test]
fn perf_smoke_requires_test_provider_feature() {
    eprintln!("skipping perf smoke; enable --features test-provider");
}
