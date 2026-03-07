//! Tests pull command integration that combines fetch with merge or rebase behaviors.

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
