//! Integration tests for `rev-list` command.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use std::{fs, thread, time::Duration};

use super::*;

fn create_two_commit_repo_via_cli() -> tempfile::TempDir {
    let repo = create_committed_repo_via_cli();

    thread::sleep(Duration::from_secs(1));
    fs::write(repo.path().join("tracked.txt"), "tracked\nsecond\n")
        .expect("failed to update tracked file");

    let output = run_libra_command(&["add", "tracked.txt"], repo.path());
    assert_cli_success(&output, "failed to add second commit changes");

    let output = run_libra_command(&["commit", "-m", "second", "--no-verify"], repo.path());
    assert_cli_success(&output, "failed to create second commit");

    repo
}

#[test]
fn test_rev_list_defaults_to_head() {
    let repo = create_committed_repo_via_cli();

    let implicit = run_libra_command(&["rev-list"], repo.path());
    assert_cli_success(&implicit, "rev-list");

    let explicit = run_libra_command(&["rev-list", "HEAD"], repo.path());
    assert_cli_success(&explicit, "rev-list HEAD");

    assert_eq!(implicit.stdout, explicit.stdout);
}

#[test]
fn test_rev_list_head_lists_reachable_commits_newest_first() {
    let repo = create_two_commit_repo_via_cli();

    let head = run_libra_command(&["rev-parse", "HEAD"], repo.path());
    assert_cli_success(&head, "rev-parse HEAD");
    let head_hash = String::from_utf8_lossy(&head.stdout).trim().to_string();

    let parent = run_libra_command(&["rev-parse", "HEAD~1"], repo.path());
    assert_cli_success(&parent, "rev-parse HEAD~1");
    let parent_hash = String::from_utf8_lossy(&parent.stdout).trim().to_string();

    let output = run_libra_command(&["rev-list", "HEAD"], repo.path());
    assert_cli_success(&output, "rev-list HEAD");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(lines, vec![head_hash.as_str(), parent_hash.as_str()]);
}

#[test]
fn test_rev_list_supports_revision_navigation() {
    let repo = create_two_commit_repo_via_cli();

    let parent = run_libra_command(&["rev-parse", "HEAD~1"], repo.path());
    assert_cli_success(&parent, "rev-parse HEAD~1");
    let parent_hash = String::from_utf8_lossy(&parent.stdout).trim().to_string();

    let output = run_libra_command(&["rev-list", "HEAD~1"], repo.path());
    assert_cli_success(&output, "rev-list HEAD~1");

    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), parent_hash);
}

#[test]
fn test_rev_list_invalid_target_returns_cli_error_code() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["rev-list", "badref"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(129));
    assert!(stderr.contains("not a valid object name: 'badref'"));
    assert_eq!(report.error_code, "LBR-CLI-003");
}

#[test]
fn test_rev_list_json_returns_envelope() {
    let repo = create_two_commit_repo_via_cli();

    let output = run_libra_command(&["--json", "rev-list", "HEAD"], repo.path());
    assert_cli_success(&output, "json rev-list HEAD");

    let json = parse_json_stdout(&output);
    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "rev-list");
    assert_eq!(json["data"]["input"], "HEAD");
    assert_eq!(json["data"]["total"], 2);
    assert_eq!(json["data"]["commits"].as_array().map(Vec::len), Some(2));
}

#[test]
fn test_rev_list_machine_returns_single_json_line() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["--machine", "rev-list", "HEAD"], repo.path());
    assert_cli_success(&output, "machine rev-list HEAD");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.lines().count(),
        1,
        "expected one JSON line, got: {stdout}"
    );

    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).expect("expected JSON");
    assert_eq!(parsed["command"], "rev-list");
    assert_eq!(parsed["data"]["input"], "HEAD");
}

#[test]
fn test_rev_list_quiet_suppresses_stdout() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["--quiet", "rev-list", "HEAD"], repo.path());
    assert_cli_success(&output, "quiet rev-list HEAD");

    assert!(
        output.stdout.is_empty(),
        "unexpected stdout: {:?}",
        output.stdout
    );
}
