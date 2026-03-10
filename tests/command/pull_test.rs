//! Tests pull command integration that combines fetch with merge or rebase behaviors.

use std::process::Command;

use serial_test::serial;

use super::{create_committed_repo_via_cli, run_libra_command};

#[test]
#[serial]
fn test_pull_cli_without_tracking_returns_error_1() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["pull"], repo.path());
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Without a configured remote, fetch fails before the tracking check.
    assert_eq!(output.status.code(), Some(128));
    assert!(stderr.contains("no configured remote for the current branch"));
}

#[test]
#[serial]
fn test_pull_cli_with_tracking_from_local_remote_succeeds() {
    let repo = create_committed_repo_via_cli();
    let remote = tempfile::tempdir().expect("failed to create remote dir");
    let remote_path = remote.path();

    let init_remote = Command::new("git")
        .args(["init", "--bare", remote_path.to_str().unwrap()])
        .output()
        .expect("failed to init bare remote");
    assert!(
        init_remote.status.success(),
        "failed to init bare remote: {}",
        String::from_utf8_lossy(&init_remote.stderr)
    );

    let branch_out = run_libra_command(&["branch", "--show-current"], repo.path());
    assert!(
        branch_out.status.success(),
        "failed to get current branch: {}",
        String::from_utf8_lossy(&branch_out.stderr)
    );
    let branch = String::from_utf8_lossy(&branch_out.stdout).trim().to_string();
    assert!(!branch.is_empty(), "current branch should not be empty");

    let remote_add = run_libra_command(
        &["remote", "add", "origin", remote_path.to_str().unwrap()],
        repo.path(),
    );
    assert!(
        remote_add.status.success(),
        "failed to add remote: {}",
        String::from_utf8_lossy(&remote_add.stderr)
    );

    let push = run_libra_command(&["push", "-u", "origin", &branch], repo.path());
    assert!(
        push.status.success(),
        "failed to set upstream with push: {}",
        String::from_utf8_lossy(&push.stderr)
    );

    let pull = run_libra_command(&["pull"], repo.path());
    assert!(
        pull.status.success(),
        "pull with configured tracking should succeed: {}",
        String::from_utf8_lossy(&pull.stderr)
    );
}
