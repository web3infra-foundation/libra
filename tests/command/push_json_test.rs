//! JSON output schema validation for push command.
//!
//! **Layer:** L1 — all tests are in-process, no network required.

use serde_json::Value;

use super::{create_committed_repo_via_cli, run_libra_command};

/// Parse JSON error report from stderr.
///
/// When `--json` is active, errors are rendered as pretty-printed JSON to stderr.
/// This helper parses the entire stderr as a JSON object.
fn parse_json_error_stderr(stderr: &[u8]) -> Value {
    let text = String::from_utf8_lossy(stderr);
    serde_json::from_str(text.trim()).unwrap_or_else(|e| {
        panic!("failed to parse JSON from stderr: {e}\nstderr: {text}");
    })
}

// ---------------------------------------------------------------------------
// Error JSON: no remote configured
// ---------------------------------------------------------------------------

#[test]
fn test_push_json_error_no_remote() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["--json", "push"], repo.path());

    let report = parse_json_error_stderr(&output.stderr);

    assert_eq!(report["ok"], Value::Bool(false));
    assert_eq!(report["error_code"], "LBR-REPO-003");
    assert!(
        report["message"]
            .as_str()
            .unwrap_or("")
            .contains("no configured push destination")
    );
}

// ---------------------------------------------------------------------------
// Error JSON: invalid refspec
// ---------------------------------------------------------------------------

#[test]
fn test_push_json_error_invalid_refspec() {
    let repo = create_committed_repo_via_cli();

    let _ = run_libra_command(
        &["remote", "add", "origin", "https://example.com/repo.git"],
        repo.path(),
    );

    let output = run_libra_command(&["--json", "push", "origin", "src:"], repo.path());

    let report = parse_json_error_stderr(&output.stderr);

    assert_eq!(report["ok"], Value::Bool(false));
    assert_eq!(report["error_code"], "LBR-CLI-002");
}

// ---------------------------------------------------------------------------
// Error JSON: source ref not found
// ---------------------------------------------------------------------------

#[test]
fn test_push_json_error_source_ref_not_found() {
    let repo = create_committed_repo_via_cli();

    let _ = run_libra_command(
        &["remote", "add", "origin", "https://example.com/repo.git"],
        repo.path(),
    );

    let output = run_libra_command(&["--json", "push", "origin", "nonexistent"], repo.path());

    let report = parse_json_error_stderr(&output.stderr);

    assert_eq!(report["ok"], Value::Bool(false));
    assert_eq!(report["error_code"], "LBR-CLI-003");
}

// ---------------------------------------------------------------------------
// Error JSON: detached head
// ---------------------------------------------------------------------------

#[test]
fn test_push_json_error_detached_head() {
    let repo = create_committed_repo_via_cli();

    // Get full commit hash from log
    let log_out = run_libra_command(&["log"], repo.path());
    let stdout = String::from_utf8_lossy(&log_out.stdout);
    let hash = stdout
        .lines()
        .find(|l| l.starts_with("commit "))
        .and_then(|l| l.strip_prefix("commit "))
        .map(|h| h.trim())
        .expect("expected commit hash in log output");

    // Detach HEAD using switch --detach
    let switch_out = run_libra_command(&["switch", "--detach", hash], repo.path());
    assert!(
        switch_out.status.success(),
        "switch --detach failed: {}",
        String::from_utf8_lossy(&switch_out.stderr)
    );

    let _ = run_libra_command(
        &["remote", "add", "origin", "https://example.com/repo.git"],
        repo.path(),
    );

    let output = run_libra_command(&["--json", "push"], repo.path());

    let report = parse_json_error_stderr(&output.stderr);

    assert_eq!(report["ok"], Value::Bool(false));
    assert_eq!(report["error_code"], "LBR-REPO-003");
    assert!(
        report["message"]
            .as_str()
            .unwrap_or("")
            .contains("HEAD is detached")
    );
}

// ---------------------------------------------------------------------------
// Machine mode: error goes to stderr as single-line JSON
// ---------------------------------------------------------------------------

#[test]
fn test_push_machine_error_is_single_line_json() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["--machine", "push"], repo.path());

    let stderr = String::from_utf8_lossy(&output.stderr);

    // In machine mode, the error report should be parseable as a single JSON object
    let report: Value = serde_json::from_str(stderr.trim())
        .expect("stderr should be parseable JSON in machine mode");
    assert_eq!(report["ok"], Value::Bool(false));
}
