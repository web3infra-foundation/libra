#[cfg(feature = "test-provider")]
mod harness;

#[cfg(feature = "test-provider")]
use std::{
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

#[cfg(feature = "test-provider")]
use anyhow::{Result, bail};
#[cfg(feature = "test-provider")]
use harness::{CodeSession, CodeSessionOptions};
#[cfg(feature = "test-provider")]
use serial_test::serial;

#[cfg(feature = "test-provider")]
#[test]
#[serial]
fn code_session_starts_tui_and_cleans_control_files() -> Result<()> {
    let mut session = CodeSession::spawn(CodeSessionOptions::new("self", fixture("basic_chat")))?;
    let snapshot = session.snapshot()?;
    assert_eq!(snapshot["provider"]["provider"], "fake");

    let diagnostics = session.diagnostics()?;
    let diagnostics_text = diagnostics.to_string();
    assert!(!diagnostics_text.contains("control-token"));
    assert!(!diagnostics_text.contains("X-Libra-Control-Token"));

    let token_path = session.token_path().to_path_buf();
    let info_path = session.info_path().to_path_buf();
    assert!(token_path.exists());
    assert!(info_path.exists());

    session.shutdown()?;
    wait_for_absent(&token_path, Duration::from_secs(5))?;
    wait_for_absent(&info_path, Duration::from_secs(5))?;
    Ok(())
}

#[cfg(not(feature = "test-provider"))]
#[test]
fn harness_self_test_requires_test_provider_feature() {
    eprintln!("skipping harness self-test; enable --features test-provider");
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
fn wait_for_absent(path: &std::path::Path, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !path.exists() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    bail!("path still exists after shutdown: {}", path.display())
}
