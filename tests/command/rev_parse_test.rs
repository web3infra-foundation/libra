//! Integration tests for `rev-parse` command.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use super::*;

#[test]
fn test_rev_parse_head_resolves_commit() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["rev-parse", "HEAD"], repo.path());
    assert_cli_success(&output, "rev-parse HEAD");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let value = stdout.trim();
    assert_eq!(value.len(), 40, "expected full hash, got: {value}");
    assert!(value.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn test_rev_parse_short_head_returns_non_ambiguous_hash() {
    let repo = create_committed_repo_via_cli();

    let full = run_libra_command(&["rev-parse", "HEAD"], repo.path());
    assert_cli_success(&full, "rev-parse HEAD (full)");
    let full_hash = String::from_utf8_lossy(&full.stdout).trim().to_string();

    let output = run_libra_command(&["rev-parse", "--short", "HEAD"], repo.path());
    assert_cli_success(&output, "rev-parse --short HEAD");

    let short_hash = String::from_utf8_lossy(&output.stdout).trim().to_string();
    assert!(
        short_hash.len() >= 7,
        "expected abbreviated hash, got: {short_hash}"
    );
    assert!(short_hash.len() <= full_hash.len());
    assert!(full_hash.starts_with(&short_hash));

    let resolved = run_libra_command(&["rev-parse", short_hash.as_str()], repo.path());
    assert_cli_success(&resolved, "rev-parse <short-hash>");
    assert_eq!(String::from_utf8_lossy(&resolved.stdout).trim(), full_hash);
}

#[test]
fn test_rev_parse_abbrev_ref_head_returns_branch_name() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["rev-parse", "--abbrev-ref", "HEAD"], repo.path());
    assert_cli_success(&output, "rev-parse --abbrev-ref HEAD");

    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "main");
}

#[tokio::test]
#[serial]
async fn test_rev_parse_abbrev_ref_remote_tracking_ref_returns_short_name() {
    let repo = tempdir().expect("failed to create repository root");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = ChangeDirGuard::new(repo.path());

    commit::execute(CommitArgs {
        message: Some("base".to_string()),
        allow_empty: true,
        disable_pre: true,
        no_verify: false,
        ..Default::default()
    })
    .await;

    let head = Head::current_commit().await.expect("expected HEAD commit");
    Branch::update_branch(
        "refs/remotes/origin/main",
        &head.to_string(),
        Some("origin"),
    )
    .await
    .expect("failed to create remote-tracking ref");

    let output = run_libra_command(&["rev-parse", "--abbrev-ref", "origin/main"], repo.path());
    assert_cli_success(&output, "rev-parse --abbrev-ref origin/main");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "origin/main"
    );
}

#[test]
fn test_rev_parse_show_toplevel_rejects_spec() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["rev-parse", "--show-toplevel", "HEAD"], repo.path());

    assert!(!output.status.success(), "command unexpectedly succeeded");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot be used with") || stderr.contains("unexpected argument"),
        "stderr: {stderr}"
    );
}

#[test]
fn test_rev_parse_invalid_target_returns_cli_error_code() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["rev-parse", "badref"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(129));
    assert!(stderr.contains("not a valid object name: 'badref'"));
    assert_eq!(report.error_code, "LBR-CLI-003");
}

#[test]
fn test_rev_parse_json_returns_envelope() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["--json", "rev-parse", "HEAD"], repo.path());
    assert_cli_success(&output, "json rev-parse HEAD");

    let json = parse_json_stdout(&output);
    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "rev-parse");
    assert_eq!(json["data"]["mode"], "resolve");
    assert_eq!(json["data"]["input"], "HEAD");
    assert!(json["data"]["value"].as_str().is_some());
}

#[test]
fn test_rev_parse_machine_returns_single_json_line() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["--machine", "rev-parse", "HEAD"], repo.path());
    assert_cli_success(&output, "machine rev-parse HEAD");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.lines().count(),
        1,
        "expected one JSON line, got: {stdout}"
    );

    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).expect("expected JSON");
    assert_eq!(parsed["command"], "rev-parse");
    assert_eq!(parsed["data"]["mode"], "resolve");
}
