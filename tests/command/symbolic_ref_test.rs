//! Integration tests for `symbolic-ref`.

use super::*;

#[test]
fn symbolic_ref_head_prints_full_ref() {
    let repo = tempdir().expect("failed to create repository root");
    init_repo_via_cli(repo.path());

    let output = run_libra_command(&["symbolic-ref", "HEAD"], repo.path());
    assert_cli_success(&output, "symbolic-ref HEAD");

    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "refs/heads/main"
    );
}

#[test]
fn symbolic_ref_short_head_prints_branch_name() {
    let repo = tempdir().expect("failed to create repository root");
    init_repo_via_cli(repo.path());

    let output = run_libra_command(&["symbolic-ref", "--short", "HEAD"], repo.path());
    assert_cli_success(&output, "symbolic-ref --short HEAD");

    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "main");
}

#[test]
fn symbolic_ref_json_reports_read_target() {
    let repo = tempdir().expect("failed to create repository root");
    init_repo_via_cli(repo.path());

    let output = run_libra_command(&["--json", "symbolic-ref", "HEAD"], repo.path());
    assert_cli_success(&output, "symbolic-ref --json HEAD");

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "symbolic-ref");
    assert_eq!(json["data"]["action"], "read");
    assert_eq!(json["data"]["name"], "HEAD");
    assert_eq!(json["data"]["target"], "refs/heads/main");
    assert_eq!(json["data"]["short"], "main");
}

#[test]
fn symbolic_ref_set_head_to_existing_branch() {
    let repo = create_committed_repo_via_cli();

    let branch_output = run_libra_command(&["branch", "feature"], repo.path());
    assert_cli_success(&branch_output, "branch feature");

    let output = run_libra_command(&["symbolic-ref", "HEAD", "refs/heads/feature"], repo.path());
    assert_cli_success(&output, "symbolic-ref HEAD refs/heads/feature");
    assert!(
        output.stdout.is_empty(),
        "set form should be silent on success, got: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    let current = run_libra_command(&["symbolic-ref", "--short", "HEAD"], repo.path());
    assert_cli_success(&current, "symbolic-ref --short HEAD after set");
    assert_eq!(String::from_utf8_lossy(&current.stdout).trim(), "feature");
}

#[test]
fn symbolic_ref_detached_head_returns_invalid_target() {
    let repo = create_committed_repo_via_cli();

    let detach = run_libra_command(&["switch", "--detach", "HEAD"], repo.path());
    assert_cli_success(&detach, "switch --detach HEAD");

    let output = run_libra_command(&["symbolic-ref", "HEAD"], repo.path());
    assert!(!output.status.success(), "detached HEAD should fail");

    let (human, report) = parse_cli_error_stderr(&output.stderr);
    assert!(
        human.contains("HEAD is not a symbolic ref"),
        "unexpected stderr: {human}"
    );
    assert_eq!(report.error_code, "LBR-CLI-003");
}

#[test]
fn symbolic_ref_quiet_detached_head_exits_silently() {
    let repo = create_committed_repo_via_cli();

    let detach = run_libra_command(&["switch", "--detach", "HEAD"], repo.path());
    assert_cli_success(&detach, "switch --detach HEAD");

    let output = run_libra_command(&["symbolic-ref", "--quiet", "HEAD"], repo.path());
    assert_eq!(
        output.status.code(),
        Some(1),
        "quiet detached HEAD should exit with status 1"
    );
    assert!(
        output.stdout.is_empty(),
        "quiet detached HEAD should not write stdout, got: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        output.stderr.is_empty(),
        "quiet detached HEAD should not write stderr, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn symbolic_ref_outside_repo_reports_repo_not_found() {
    let dir = tempdir().expect("failed to create non-repo directory");

    let output = run_libra_command(&["symbolic-ref", "HEAD"], dir.path());
    assert!(
        !output.status.success(),
        "symbolic-ref outside repo should fail"
    );

    let (human, report) = parse_cli_error_stderr(&output.stderr);
    assert!(
        human.contains("not a libra repository"),
        "unexpected stderr: {human}"
    );
    assert_eq!(report.error_code, "LBR-REPO-001");
}
