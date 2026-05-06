#[cfg(feature = "test-provider")]
mod harness;

#[cfg(feature = "test-provider")]
use std::{path::PathBuf, time::Duration};

#[cfg(feature = "test-provider")]
use anyhow::Result;
#[cfg(feature = "test-provider")]
use harness::{CodeSession, CodeSessionOptions, Scenario};
#[cfg(feature = "test-provider")]
use reqwest::StatusCode;
#[cfg(feature = "test-provider")]
use serde_json::Value;
#[cfg(feature = "test-provider")]
use serial_test::serial;

#[cfg(feature = "test-provider")]
#[test]
#[serial]
fn basic_chat_submit_updates_transcript() -> Result<()> {
    let mut session = CodeSession::spawn(CodeSessionOptions::new("basic", fixture("basic_chat")))?;
    {
        let mut scenario = Scenario::new("basic_chat", &mut session);
        scenario
            .step("attach automation")
            .attach_automation("scenario-basic")?
            .expect_controller_kind("automation")?;
        scenario
            .step("submit direct chat")
            .submit("/chat hello")?
            .expect_transcript_contains("fake assistant: hello from the PTY harness")?
            .expect_status_eq("idle")?;
    }

    session.shutdown()
}

#[cfg(feature = "test-provider")]
#[test]
#[serial]
fn automation_reclaim_returns_control_to_tui() -> Result<()> {
    let mut session =
        CodeSession::spawn(CodeSessionOptions::new("reclaim", fixture("basic_chat")))?;
    session.attach_automation("scenario-reclaim")?;
    session.wait_for_snapshot(Duration::from_secs(10), |snapshot| {
        controller_kind(snapshot) == Some("automation")
    })?;

    session.write_tui_line("/control reclaim")?;
    session.wait_for_snapshot(Duration::from_secs(10), |snapshot| {
        controller_kind(snapshot) == Some("tui")
    })?;

    let (status, body) = session.submit_message_expect_error("/chat hello")?;
    assert!(!status.is_success());
    assert!(matches!(
        error_code(&body),
        Some("INVALID_CONTROLLER_TOKEN" | "CONTROLLER_CONFLICT")
    ));

    session.shutdown()
}

#[cfg(feature = "test-provider")]
#[test]
#[serial]
fn cancel_running_turn_returns_session_to_idle() -> Result<()> {
    let mut session =
        CodeSession::spawn(CodeSessionOptions::new("cancel", fixture("delayed_chat")))?;
    session.attach_automation("scenario-cancel")?;
    session.submit_message("/chat slow")?;
    session.wait_for_snapshot(Duration::from_secs(10), |snapshot| {
        status(snapshot) == Some("thinking")
    })?;

    session.cancel_turn()?;
    session.wait_for_snapshot(Duration::from_secs(10), |snapshot| {
        status(snapshot) == Some("idle")
    })?;

    session.shutdown()
}

#[cfg(feature = "test-provider")]
#[test]
#[serial]
fn oversized_message_is_rejected_before_reaching_tui() -> Result<()> {
    let mut session =
        CodeSession::spawn(CodeSessionOptions::new("oversize", fixture("basic_chat")))?;
    session.attach_automation("scenario-oversize")?;
    let (status, body) = session.submit_large_message(300 * 1024)?;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(error_code(&body), Some("PAYLOAD_TOO_LARGE"));
    session.shutdown()
}

#[cfg(feature = "test-provider")]
#[test]
#[serial]
fn unknown_interaction_id_is_rejected_without_state_change() -> Result<()> {
    let mut session = CodeSession::spawn(CodeSessionOptions::new(
        "unknown-interaction",
        fixture("basic_chat"),
    ))?;
    session.attach_automation("scenario-unknown-interaction")?;
    let before = session.snapshot()?;

    let (http_status, body) = session.respond_interaction_expect_error("missing-interaction")?;

    assert_eq!(http_status, StatusCode::CONFLICT);
    assert_eq!(error_code(&body), Some("INTERACTION_NOT_ACTIVE"));
    let after = session.snapshot()?;
    assert_eq!(status(&before), status(&after));
    assert_eq!(controller_kind(&after), Some("automation"));
    session.shutdown()
}

#[cfg(feature = "test-provider")]
#[test]
#[serial]
fn default_control_paths_reject_second_live_instance() -> Result<()> {
    let mut session = CodeSession::spawn(
        CodeSessionOptions::new("multi-instance", fixture("basic_chat"))
            .with_default_control_paths(),
    )?;
    let output = session.run_default_control_conflict()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}\n{stderr}");

    assert!(!output.status.success());
    assert!(combined.contains("CONTROL_INSTANCE_CONFLICT"));
    assert!(combined.contains("baseUrl") || combined.contains("http://127.0.0.1:"));

    session.shutdown()
}

/// Browser-controller end-to-end smoke. Spawns `libra code` with
/// `--browser-control loopback`, attaches as a browser (no automation
/// control token), submits a chat through the browser write surface, and
/// confirms the snapshot reflects the browser ownership + transcript turn.
/// Ends with a clean detach.
#[cfg(feature = "test-provider")]
#[test]
#[serial]
fn browser_controller_attach_submit_detach_roundtrip() -> Result<()> {
    let mut session = CodeSession::spawn(
        CodeSessionOptions::new("browser-roundtrip", fixture("basic_chat"))
            .with_browser_control_loopback(),
    )?;

    let token = session.attach_browser("scenario-browser-roundtrip")?;
    session.wait_for_snapshot(Duration::from_secs(10), |snapshot| {
        controller_kind(snapshot) == Some("browser")
    })?;

    let (status, body) = session.browser_submit_message(&token, "/chat hello")?;
    assert!(
        status.is_success(),
        "browser submit must succeed, got {status}: {body}",
    );

    session.wait_for_snapshot(Duration::from_secs(10), |snapshot| {
        status_eq(snapshot, "idle")
            && transcript_contains(snapshot, "fake assistant: hello from the PTY harness")
    })?;

    let (detach_status, _) = session.browser_detach(&token, "scenario-browser-roundtrip")?;
    assert!(detach_status.is_success());

    session.shutdown()
}

/// `--browser-control` defaults to `off` for the harness's TUI fixture, so
/// without `with_browser_control_loopback()` an attach must come back with
/// `BROWSER_CONTROL_DISABLED` and the runtime stays controlled by the TUI.
#[cfg(feature = "test-provider")]
#[test]
#[serial]
fn browser_attach_rejected_when_control_disabled() -> Result<()> {
    let mut session = CodeSession::spawn(CodeSessionOptions::new(
        "browser-disabled",
        fixture("basic_chat"),
    ))?;

    let (http_status, body) = session.attach_browser_expect_error("scenario-browser-disabled")?;
    assert_eq!(http_status, StatusCode::FORBIDDEN);
    assert_eq!(error_code(&body), Some("BROWSER_CONTROL_DISABLED"));

    let snapshot = session.snapshot()?;
    assert_ne!(controller_kind(&snapshot), Some("browser"));

    session.shutdown()
}

/// Once a browser holds the lease, a second browser attempting to attach
/// with a different `clientId` must trip `CONTROLLER_CONFLICT` instead of
/// kicking the first writer out — the lease must be released or expire
/// first. Mirrors the multi-tab scenario the frontend has to defend against.
#[cfg(feature = "test-provider")]
#[test]
#[serial]
fn second_browser_attach_with_different_client_returns_conflict() -> Result<()> {
    let mut session = CodeSession::spawn(
        CodeSessionOptions::new("browser-conflict", fixture("basic_chat"))
            .with_browser_control_loopback(),
    )?;

    let _first_token = session.attach_browser("scenario-browser-first")?;
    session.wait_for_snapshot(Duration::from_secs(10), |snapshot| {
        controller_kind(snapshot) == Some("browser")
    })?;

    let (http_status, body) = session.attach_browser_expect_error("scenario-browser-second")?;
    assert_eq!(http_status, StatusCode::CONFLICT);
    assert_eq!(error_code(&body), Some("CONTROLLER_CONFLICT"));

    session.shutdown()
}

#[cfg(not(feature = "test-provider"))]
#[test]
fn code_ui_scenarios_require_test_provider_feature() {
    eprintln!("skipping code UI scenarios; enable --features test-provider");
}

#[cfg(feature = "test-provider")]
fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("code_ui")
        .join(format!("{name}.json"))
}

#[cfg(feature = "test-provider")]
fn status(snapshot: &Value) -> Option<&str> {
    snapshot.get("status").and_then(Value::as_str)
}

#[cfg(feature = "test-provider")]
fn controller_kind(snapshot: &Value) -> Option<&str> {
    snapshot
        .get("controller")
        .and_then(|controller| controller.get("kind"))
        .and_then(Value::as_str)
}

#[cfg(feature = "test-provider")]
fn error_code(body: &Value) -> Option<&str> {
    body.get("error")
        .and_then(|error| error.get("code"))
        .or_else(|| body.get("code"))
        .and_then(Value::as_str)
}

#[cfg(feature = "test-provider")]
fn status_eq(snapshot: &Value, expected: &str) -> bool {
    status(snapshot) == Some(expected)
}

#[cfg(feature = "test-provider")]
fn transcript_contains(snapshot: &Value, needle: &str) -> bool {
    let Some(transcript) = snapshot.get("transcript").and_then(Value::as_array) else {
        return false;
    };
    transcript.iter().any(|entry| {
        entry
            .get("content")
            .and_then(Value::as_str)
            .is_some_and(|content| content.contains(needle))
    })
}
