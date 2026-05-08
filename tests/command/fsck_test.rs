//! Integration tests for the `fsck` command.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

// ---------------------------------------------------------------------------
// Basic Functionality Tests (≥ 4 required)
// ---------------------------------------------------------------------------

use std::fs;

use serial_test::serial;
use tempfile::tempdir;

use super::*;

// ---------------------------------------------------------------------------
// Basic Functionality Tests (≥ 4 required)
// ---------------------------------------------------------------------------

#[test]
#[serial]
/// Tests fsck on an empty repository passes successfully.
/// Verifies the basic happy path for newly initialized repositories.
fn test_fsck_empty_repo_passes() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());

    let output = run_libra_command(&["fsck"], repo.path());
    assert!(
        output.status.success(),
        "fsck on empty repo should pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[serial]
/// Tests fsck on a repository with commits passes successfully.
/// Verifies the basic happy path for normal repositories.
fn test_fsck_repo_with_commit_passes() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["fsck"], repo.path());
    assert!(
        output.status.success(),
        "fsck on healthy repo should pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[serial]
/// Tests fsck --verbose outputs progress information.
/// Verifies that the verbose flag produces expected output.
fn test_fsck_verbose_output() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["fsck", "--verbose"], repo.path());
    assert!(
        output.status.success(),
        "fsck --verbose should pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Checking"),
        "verbose output should contain 'Checking', got: {}",
        stdout
    );
}

#[test]
#[serial]
/// Tests fsck --root reports root commits.
/// Verifies that the --root flag correctly identifies root commits.
fn test_fsck_root_shows_root_commit() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["fsck", "--root"], repo.path());
    assert!(
        output.status.success(),
        "fsck --root should pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("root"),
        "--root should report root commits, got: {}",
        stdout
    );
}

#[test]
#[serial]
/// Tests fsck --tags reports tagged commits.
/// Verifies that the --tags flag correctly lists tags.
fn test_fsck_tags_reports_tags() {
    let repo = create_committed_repo_via_cli();

    let tag_output = run_libra_command(&["tag", "v1.0"], repo.path());
    assert!(
        tag_output.status.success(),
        "tag creation should succeed, stderr: {}",
        String::from_utf8_lossy(&tag_output.stderr)
    );

    let output = run_libra_command(&["fsck", "--tags"], repo.path());
    assert!(
        output.status.success(),
        "fsck --tags should pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("v1.0"),
        "--tags should report tag names, got: {}",
        stdout
    );
}

#[test]
#[serial]
/// Tests fsck --dangling detects dangling commits.
/// Verifies that dangling objects are properly detected.
fn test_fsck_dangling_shows_only_commits() {
    let repo = create_committed_repo_via_cli();

    // Create a second commit
    fs::write(repo.path().join("file2.txt"), "second file\n").unwrap();
    run_libra_command(&["add", "file2.txt"], repo.path());
    run_libra_command(&["commit", "-m", "second", "--no-verify"], repo.path());

    // Reset to first commit, making the second commit dangling
    let log_output = run_libra_command(&["log", "--pretty=%H"], repo.path());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    let first_commit = stdout.lines().nth(1).unwrap().trim();
    run_libra_command(&["reset", "--hard", first_commit], repo.path());

    let output = run_libra_command(&["fsck", "--no-reflogs"], repo.path());
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("dangling commit"),
        "should show dangling commit, got: {}",
        combined
    );
}

#[test]
#[serial]
/// Tests fsck --connectivity-only validates object graph.
/// Verifies that connectivity check passes on healthy repos.
fn test_fsck_connectivity_only_passes() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["fsck", "--connectivity-only"], repo.path());
    assert!(
        output.status.success(),
        "--connectivity-only should pass on healthy repo, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[serial]
/// Tests fsck returns exit code 0 on success.
/// Verifies the correct exit code for successful validation.
fn test_fsck_exit_code_zero_on_success() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["fsck"], repo.path());
    assert_eq!(
        output.status.code(),
        Some(0),
        "fsck should exit 0 on success, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
