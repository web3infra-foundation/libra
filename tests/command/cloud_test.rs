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
