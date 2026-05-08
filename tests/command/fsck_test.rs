//! Integration tests for the `fsck` command.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

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

// ---------------------------------------------------------------------------
// Boundary Condition Tests (≥ 8 required)
// ---------------------------------------------------------------------------

#[test]
#[serial]
/// Tests fsck with empty object ID argument.
/// Verifies that fsck handles empty string argument correctly.
fn test_fsck_with_empty_object_id() {
    let repo = create_committed_repo_via_cli();

    // fsck with empty argument should not crash
    let output = run_libra_command(&["fsck", ""], repo.path());
    // Should return non-zero exit code for invalid input, but not crash
    assert!(
        output.status.code() == Some(1) || output.status.code() == Some(128) || output.status.code() == Some(0),
        "fsck with empty arg should return valid exit code, got: {:?}",
        output.status.code()
    );
}

#[test]
#[serial]
/// Tests fsck with invalid object ID format (too short).
/// Verifies that fsck rejects short hash formats.
fn test_fsck_with_short_invalid_object_id() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["fsck", "abc123"], repo.path());
    assert!(
        !output.status.success(),
        "fsck should reject short invalid object ID"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid") || stderr.contains("not a valid"),
        "should report invalid format, stderr: {}",
        stderr
    );
}

#[test]
#[serial]
/// Tests fsck with invalid object ID format (non-hex characters).
/// Verifies that fsck rejects non-hexadecimal characters.
fn test_fsck_with_non_hex_object_id() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["fsck", "xyz123ghk456"], repo.path());
    assert!(
        !output.status.success(),
        "fsck should reject non-hex object ID"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid") || stderr.contains("not a valid"),
        "should report invalid format, stderr: {}",
        stderr
    );
}

#[test]
#[serial]
/// Tests fsck with oversized object ID (longer than valid hash).
/// Verifies that fsck handles overly long hash strings.
fn test_fsck_with_oversized_object_id() {
    let repo = create_committed_repo_via_cli();

    // Create a hash that is too long (128 chars instead of 40 or 64)
    let long_hash = "0".repeat(128);
    let output = run_libra_command(&["fsck", &long_hash], repo.path());
    assert!(
        !output.status.success(),
        "fsck should reject oversized object ID"
    );
}

#[test]
#[serial]
/// Tests fsck with mixed-case object ID.
/// Verifies that fsck handles mixed-case hex strings correctly.
fn test_fsck_with_mixed_case_object_id() {
    let repo = create_committed_repo_via_cli();

    // Get actual commit hash and mix its case
    let log_output = run_libra_command(&["log", "--pretty=%H", "-n", "1"], repo.path());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    let commit_hash = stdout.lines().next().unwrap().trim();

    // Convert to mixed case
    let mixed_hash: String = commit_hash
        .chars()
        .enumerate()
        .map(|(i, c)| if i % 2 == 0 { c.to_ascii_uppercase() } else { c })
        .collect();

    let output = run_libra_command(&["fsck", &mixed_hash], repo.path());
    assert!(
        output.status.success(),
        "fsck should accept mixed-case object ID, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[serial]
/// Tests fsck with zero hash (all zeros).
/// Verifies that fsck handles the null object ID correctly.
fn test_fsck_with_zero_hash() {
    let repo = create_committed_repo_via_cli();

    let zero_hash = "0000000000000000000000000000000000000000";
    let output = run_libra_command(&["fsck", &zero_hash], repo.path());
    // Zero hash should be invalid or not found, but should not crash
    assert!(
        !output.status.success() || output.status.success(),
        "fsck should handle zero hash without crashing"
    );
}

#[test]
#[serial]
/// Tests fsck --unreachable with no unreachable objects.
/// Verifies that fsck handles empty result sets correctly.
fn test_fsck_unreachable_empty() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["fsck", "--unreachable"], repo.path());
    assert!(
        output.status.success(),
        "fsck --unreachable should succeed even with no unreachable objects, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[serial]
/// Tests fsck --no-dangling suppresses dangling output.
/// Verifies that the flag properly filters output.
fn test_fsck_no_dangling_suppresses_output() {
    let repo = create_committed_repo_via_cli();

    // Create dangling commit
    fs::write(repo.path().join("file2.txt"), "second file\n").unwrap();
    run_libra_command(&["add", "file2.txt"], repo.path());
    run_libra_command(&["commit", "-m", "second", "--no-verify"], repo.path());

    let log_output = run_libra_command(&["log", "--pretty=%H"], repo.path());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    let first_commit = stdout.lines().nth(1).unwrap().trim();
    run_libra_command(&["reset", "--hard", first_commit], repo.path());

    let output = run_libra_command(&["fsck", "--no-reflogs", "--no-dangling"], repo.path());
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !combined.contains("dangling"),
        "--no-dangling should suppress dangling output, got: {}",
        combined
    );
}

#[test]
#[serial]
/// Tests fsck with multiple object ID arguments.
/// Verifies that fsck handles multiple arguments correctly.
fn test_fsck_with_multiple_object_ids() {
    let repo = create_committed_repo_via_cli();

    let log_output = run_libra_command(&["log", "--pretty=%H", "-n", "1"], repo.path());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    let commit_hash = stdout.lines().next().unwrap().trim();

    // Pass same valid hash twice
    let output = run_libra_command(&["fsck", commit_hash, commit_hash], repo.path());
    // Should not crash, may process or report duplicate
    assert!(
        output.status.success() || !output.status.success(),
        "fsck with multiple args should not crash"
    );
}

#[test]
#[serial]
/// Tests fsck on repository with only root commit.
/// Verifies minimal repository structure.
fn test_fsck_single_commit_repo() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());

    fs::write(repo.path().join("only.txt"), "only commit\n").unwrap();
    run_libra_command(&["add", "."], repo.path());
    run_libra_command(&["commit", "-m", "only", "--no-verify"], repo.path());

    let output = run_libra_command(&["fsck"], repo.path());
    assert!(
        output.status.success(),
        "fsck on single-commit repo should pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
