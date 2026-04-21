//! Integration coverage for `libra graph` CLI argument handling.

use std::fs;

use tempfile::tempdir;

use super::{assert_cli_success, parse_cli_error_stderr, run_libra_command};

#[test]
fn graph_rejects_non_uuid_thread_id_before_opening_tui() {
    let repo = tempdir().expect("failed to create temporary directory");
    let init = run_libra_command(&["init"], repo.path());
    assert_cli_success(&init, "failed to initialize repository");

    let output = run_libra_command(&["graph", "not-a-thread"], repo.path());

    assert!(!output.status.success());
    let (_stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert!(
        report
            .message
            .contains("graph expects a canonical thread_id UUID"),
        "expected graph UUID validation error, got {:?}",
        report
    );
}

#[test]
fn graph_repo_flag_uses_target_repo_when_passed_after_thread_id() {
    let root = tempdir().expect("failed to create temporary directory");
    let repo = root.path().join("linked");
    let outside = root.path().join("outside");
    fs::create_dir_all(&repo).expect("failed to create repository directory");
    fs::create_dir_all(&outside).expect("failed to create outside directory");

    let init = run_libra_command(&["init"], &repo);
    assert_cli_success(&init, "failed to initialize repository");

    let repo_arg = repo
        .to_str()
        .expect("temporary repository path should be valid UTF-8");
    let output = run_libra_command(
        &[
            "graph",
            "019d9c35-5e95-7901-9625-65abdf797165",
            "--repo",
            repo_arg,
        ],
        &outside,
    );

    assert!(!output.status.success());
    let (_stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-REPO-002");
    assert!(
        report.message.contains("failed to load thread graph"),
        "expected graph load failure after accepting --repo, got {:?}",
        report
    );
    assert!(
        !report.message.contains("unexpected argument"),
        "graph should accept --repo after the thread id, got {:?}",
        report
    );
    assert!(
        !report.message.contains("not a libra repository"),
        "graph should use the --repo target instead of the process cwd, got {:?}",
        report
    );
}
