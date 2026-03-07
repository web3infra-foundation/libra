//! Tests pull command integration that combines fetch with merge or rebase behaviors.

use super::{create_committed_repo_via_cli, run_libra_command};
use serial_test::serial;

#[test]
#[serial]
fn test_pull_cli_without_tracking_returns_error_1() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["pull"], repo.path());
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(1));
    assert!(stderr.contains("error: There is no tracking information for the current branch."));
    assert!(stderr.contains("Hint: Run 'libra branch --set-upstream-to=<remote>/<branch>'"));
    assert!(stderr.contains("Hint: Or specify a remote and branch"));
}
