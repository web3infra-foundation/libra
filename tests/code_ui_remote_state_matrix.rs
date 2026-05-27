//! Wave 7 / PR 7 — `code_ui_remote_state` matrix runner.
//!
//! Loads `tests/data/code_ui_remote/state_cases.json` and runs the
//! P1 concurrency / size-limit cases through a real `libra code`
//! PTY session:
//!
//! 1. serial detach → re-attach issues a fresh controllerToken,
//! 2. parallel attach yields exactly one 200 + one 409
//!    `CONTROLLER_CONFLICT`,
//! 3. submit-while-thinking surfaces 409 `SESSION_BUSY`,
//! 4. cancel-while-idle surfaces 409 `SESSION_BUSY`,
//! 5. body-size boundary cases — 256 KiB accepted, 257 KiB and
//!    1 MiB rejected with `PAYLOAD_TOO_LARGE` and no hang.
//!
//! P2 case (`tool_call_fixture_reaches_tool_phase_or_deferred_l1`)
//! is intentionally not wired here — the doc tags it as P2 and
//! defers it pending a stable tool-phase fixture; the runner stays
//! a one-line addition once that lands.

#[cfg(feature = "test-provider")]
mod harness;

#[cfg(feature = "test-provider")]
use std::{path::PathBuf, time::Duration};

#[cfg(feature = "test-provider")]
use anyhow::Result;
#[cfg(feature = "test-provider")]
use harness::matrix::{Case, CaseFile, build_session_options, find_case, load_case_file};
#[cfg(feature = "test-provider")]
use harness::{CodeSession, CodeSessionOptions};
#[cfg(feature = "test-provider")]
use serial_test::serial;

#[cfg(feature = "test-provider")]
const CASE_FILE_PATH: &str = "tests/data/code_ui_remote/state_cases.json";

#[cfg(feature = "test-provider")]
fn run_state_case(case_name: &str) -> Result<()> {
    let file_path = harness::matrix::data_path(CASE_FILE_PATH);
    let file: CaseFile = load_case_file(&file_path)?;
    let case: Case = find_case(&file, case_name)?;
    let options = build_session_options(&file, &case);
    let mut session = CodeSession::spawn(options)?;
    let outcome = harness::matrix::run_case(&mut session, &case);
    let shutdown = session.shutdown();
    outcome?;
    shutdown
}

#[cfg(feature = "test-provider")]
macro_rules! state_case {
    ($name:ident) => {
        #[test]
        #[serial]
        fn $name() -> Result<()> {
            run_state_case(stringify!($name))
        }
    };
}

// Wave 7 — full P1 state matrix. The P2 tool-phase case is
// deferred per the JSON `notes` block; flip it on once the tool
// fixture stabilises.
#[cfg(feature = "test-provider")]
state_case!(state_two_clients_attach_serial_ok_after_detach);
#[cfg(feature = "test-provider")]
state_case!(state_two_clients_attach_parallel_one_wins_one_conflict);
#[cfg(feature = "test-provider")]
state_case!(state_submit_while_thinking_returns_session_busy);
#[cfg(feature = "test-provider")]
state_case!(state_cancel_when_idle_returns_session_busy);
#[cfg(feature = "test-provider")]
state_case!(state_payload_at_256_kib_boundary_is_accepted);
#[cfg(feature = "test-provider")]
state_case!(state_payload_at_257_kib_returns_413);
#[cfg(feature = "test-provider")]
state_case!(state_payload_at_drain_limit_1_mib_returns_413_without_hanging);

#[cfg(feature = "test-provider")]
#[test]
#[serial]
fn state_detach_while_thinking_allows_turn_to_settle() -> Result<()> {
    let client_id = "state-detach-thinking";
    let mut session = CodeSession::spawn(CodeSessionOptions::new(
        "state-detach-thinking",
        fixture("delayed_chat"),
    ))?;
    session.attach_automation(client_id)?;
    session.submit_message("/chat slow")?;
    session.wait_for_snapshot(Duration::from_secs(10), |snapshot| {
        snapshot.get("status").and_then(|v| v.as_str()) == Some("thinking")
    })?;

    session.detach_automation(client_id)?;
    session.wait_for_snapshot(Duration::from_secs(15), |snapshot| {
        let status_idle = snapshot.get("status").and_then(|v| v.as_str()) == Some("idle");
        let controller_released = snapshot
            .pointer("/controller/kind")
            .and_then(|v| v.as_str())
            .is_some_and(|kind| kind == "tui" || kind == "none");
        let transcript = snapshot
            .get("transcript")
            .and_then(|v| serde_json::to_string(v).ok())
            .unwrap_or_default();
        status_idle
            && controller_released
            && transcript.contains("fake assistant: delayed response")
    })?;

    session.shutdown()
}

#[cfg(not(feature = "test-provider"))]
#[test]
fn state_matrix_requires_test_provider_feature() {
    eprintln!("skipping state matrix; enable --features test-provider");
}

#[cfg(feature = "test-provider")]
fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("code_ui")
        .join(format!("{name}.json"))
}
