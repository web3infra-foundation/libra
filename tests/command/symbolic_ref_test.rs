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

#[tokio::test]
#[serial]
async fn symbolic_ref_set_with_reason_records_head_reflog() {
    use libra::internal::reflog::Reflog;

    let repo = create_committed_repo_via_cli();
    let branch_output = run_libra_command(&["branch", "feature"], repo.path());
    assert_cli_success(&branch_output, "branch feature");

    let reason = "manual branch move from symbolic-ref";
    let output = run_libra_command(
        &["symbolic-ref", "-m", reason, "HEAD", "refs/heads/feature"],
        repo.path(),
    );
    assert_cli_success(&output, "symbolic-ref -m HEAD refs/heads/feature");

    let _guard = ChangeDirGuard::new(repo.path());
    let db = libra::internal::db::get_db_conn_instance().await;
    let entries = Reflog::find_all(&db, "HEAD")
        .await
        .expect("read HEAD reflog");
    assert!(
        entries
            .iter()
            .any(|entry| entry.action == "switch" && entry.message == reason),
        "expected HEAD reflog to contain reason `{reason}`, entries: {entries:?}"
    );
}

#[tokio::test]
#[serial]
async fn symbolic_ref_set_without_reason_records_default_reflog_message() {
    use libra::internal::reflog::Reflog;

    let repo = create_committed_repo_via_cli();
    let branch_output = run_libra_command(&["branch", "feature"], repo.path());
    assert_cli_success(&branch_output, "branch feature");

    let output = run_libra_command(&["symbolic-ref", "HEAD", "refs/heads/feature"], repo.path());
    assert_cli_success(&output, "symbolic-ref HEAD refs/heads/feature");

    let _guard = ChangeDirGuard::new(repo.path());
    let db = libra::internal::db::get_db_conn_instance().await;
    let entries = Reflog::find_all(&db, "HEAD")
        .await
        .expect("read HEAD reflog");
    assert!(
        entries
            .iter()
            .any(|entry| entry.action == "switch"
                && entry.message == "moving from main to feature"),
        "expected HEAD reflog to contain default switch message, entries: {entries:?}"
    );
}

#[tokio::test]
#[serial]
async fn symbolic_ref_set_unborn_target_skips_reflog_but_updates_head() {
    use libra::internal::reflog::Reflog;

    let repo = create_committed_repo_via_cli();
    let _guard = ChangeDirGuard::new(repo.path());
    let db = libra::internal::db::get_db_conn_instance().await;
    let before = Reflog::find_all(&db, "HEAD")
        .await
        .expect("read initial HEAD reflog")
        .len();
    drop(db);
    drop(_guard);

    let output = run_libra_command(
        &["symbolic-ref", "HEAD", "refs/heads/unborn-topic"],
        repo.path(),
    );
    assert_cli_success(&output, "symbolic-ref HEAD refs/heads/unborn-topic");

    let current = run_libra_command(&["symbolic-ref", "HEAD"], repo.path());
    assert_cli_success(&current, "symbolic-ref HEAD after unborn set");
    assert_eq!(
        String::from_utf8_lossy(&current.stdout).trim(),
        "refs/heads/unborn-topic"
    );

    let _guard = ChangeDirGuard::new(repo.path());
    let db = libra::internal::db::get_db_conn_instance().await;
    let after = Reflog::find_all(&db, "HEAD")
        .await
        .expect("read final HEAD reflog")
        .len();
    assert_eq!(
        after, before,
        "unborn target must not write a reflog entry with a missing new_oid"
    );
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
fn symbolic_ref_json_quiet_detached_head_reports_structured_error() {
    let repo = create_committed_repo_via_cli();

    let detach = run_libra_command(&["switch", "--detach", "HEAD"], repo.path());
    assert_cli_success(&detach, "switch --detach HEAD");

    let output = run_libra_command(&["--json", "symbolic-ref", "--quiet", "HEAD"], repo.path());
    assert_eq!(
        output.status.code(),
        Some(1),
        "json quiet detached HEAD should preserve the quiet exit code"
    );
    assert!(
        output.stdout.is_empty(),
        "json quiet detached HEAD should not write stdout, got: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    let (human, report) = parse_cli_error_stderr(&output.stderr);
    assert!(
        human.is_empty(),
        "json mode should emit only the structured error envelope, got: {human}"
    );
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert_eq!(report.exit_code, 1);
    assert_eq!(report.message, "HEAD is not a symbolic ref");
    assert!(
        report.hints.is_empty(),
        "quiet mode should suppress guidance hints"
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

/// `libra symbolic-ref --help` surfaces the EXAMPLES banner so users
/// see the read, short-read, set, quiet, and JSON forms without
/// reading the design doc. Cross-cutting `--help` EXAMPLES rollout
/// per `docs/improvement/README.md` item B.
#[test]
fn test_symbolic_ref_help_lists_examples_banner() {
    let repo = tempdir().expect("tempdir for symbolic-ref --help");
    let output = run_libra_command(&["symbolic-ref", "--help"], repo.path());
    assert!(
        output.status.success(),
        "symbolic-ref --help should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("EXAMPLES:"),
        "symbolic-ref --help should include EXAMPLES banner, stdout: {stdout}"
    );
    for invocation in [
        "libra symbolic-ref HEAD",
        "libra symbolic-ref --short HEAD",
        "libra symbolic-ref HEAD refs/heads/main",
        "libra symbolic-ref -m \"manual move\" HEAD refs/heads/main",
        "libra symbolic-ref -q HEAD",
        "libra symbolic-ref --json HEAD",
    ] {
        assert!(
            stdout.contains(invocation),
            "symbolic-ref --help should include `{invocation}`, stdout: {stdout}"
        );
    }
}

#[test]
#[serial]
fn test_symbolic_ref_delete_is_rejected() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["symbolic-ref", "-d", "HEAD"], repo.path());
    assert_eq!(output.status.code(), Some(128));
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert!(
        stderr.contains("delete symbolic ref is intentionally unsupported"),
        "unexpected stderr: {stderr}"
    );
    assert_eq!(report.error_code, "LBR-CONFLICT-002");
    assert_eq!(report.exit_code, 128);
    assert!(
        report
            .hints
            .iter()
            .any(|hint| hint.contains("libra switch") && hint.contains("libra checkout")),
        "delete rejection should include switch/checkout hint: {report:?}"
    );
}

#[test]
#[serial]
fn test_symbolic_ref_delete_non_head_is_rejected() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["symbolic-ref", "-d", "refs/syms/topic"], repo.path());
    assert_eq!(output.status.code(), Some(128));
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert!(
        stderr.contains("delete symbolic ref is intentionally unsupported"),
        "unexpected stderr: {stderr}"
    );
    assert_eq!(report.error_code, "LBR-CONFLICT-002");
}
