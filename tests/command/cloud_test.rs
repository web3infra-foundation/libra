//! CLI-level tests for the `cloud` command.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use super::*;

/// Running `cloud sync` outside a repository should return exit code 128.
#[test]
fn test_cloud_cli_outside_repository_returns_fatal_128() {
    let temp = tempdir().unwrap();

    let output = run_libra_command(&["cloud", "sync"], temp.path());
    assert_eq!(output.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("fatal: not a libra repository"),
        "unexpected stderr: {stderr}"
    );
}

/// `cloud status` is local-only and supports structured output without requiring
/// Cloudflare credentials.
#[test]
fn test_cloud_status_json_output_empty_repo() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());

    let output = run_libra_command(&["--json", "cloud", "status"], repo.path());
    assert_cli_success(&output, "cloud status --json failed");
    assert!(output.stderr.is_empty());

    let json = parse_json_stdout(&output);
    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "cloud.status");
    assert!(
        json["data"]["repo_id"]
            .as_str()
            .is_some_and(|repo_id| !repo_id.is_empty()),
        "repo_id should be populated: {json}"
    );
    assert_eq!(json["data"]["total_objects"], 0);
    assert_eq!(json["data"]["synced"], 0);
    assert_eq!(json["data"]["pending"], 0);
    assert_eq!(json["data"]["synced_percent"], 0);
    assert_eq!(json["data"]["by_type"].as_array().unwrap().len(), 0);
    assert!(json["data"].get("unsynced_objects").is_none());
}

/// `--machine` emits the same status envelope as one NDJSON record.
#[test]
fn test_cloud_status_machine_output_empty_repo() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());

    let output = run_libra_command(&["--machine", "cloud", "status"], repo.path());
    assert_cli_success(&output, "cloud status --machine failed");
    assert!(output.stderr.is_empty());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.lines().count(), 1, "unexpected stdout: {stdout}");
    let json: serde_json::Value =
        serde_json::from_str(stdout.trim_end()).expect("machine stdout should be JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "cloud.status");
    assert_eq!(json["data"]["total_objects"], 0);
}
