//! Integration tests for the `fsck` command.

use std::fs;

use serial_test::serial;
use tempfile::tempdir;

use super::*;

// ---------------------------------------------------------------------------
// Basic functionality tests
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_fsck_empty_repo_passes() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());

    let output = run_libra_command(&["fsck"], repo.path());
    assert!(output.status.success(), "fsck on empty repo should pass");
}

#[test]
#[serial]
fn test_fsck_repo_with_commit_passes() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["fsck"], repo.path());
    assert!(output.status.success(), "fsck on healthy repo should pass");
}

#[test]
#[serial]
fn test_fsck_verbose_output() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["fsck", "--verbose"], repo.path());
    assert!(output.status.success(), "fsck --verbose should pass");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Checking"), "verbose should contain 'Checking'");
}

// ---------------------------------------------------------------------------
// Option tests: --no-reflogs, --dangling, --unreachable
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_fsck_no_reflogs() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["fsck", "--no-reflogs"], repo.path());
    assert!(output.status.success(), "fsck --no-reflogs should pass");
}

#[test]
#[serial]
fn test_fsck_dangling_shows_only_commits() {
    let repo = create_committed_repo_via_cli();

    // Create dangling objects
    fs::write(repo.path().join("file2.txt"), "second file\n").unwrap();
    run_libra_command(&["add", "file2.txt"], repo.path());
    run_libra_command(&["commit", "-m", "second", "--no-verify"], repo.path());

    let log_output = run_libra_command(&["log", "--pretty=%H"], repo.path());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    let first_commit = stdout.lines().nth(1).unwrap().trim();
    run_libra_command(&["reset", "--hard", first_commit], repo.path());

    let output = run_libra_command(&["fsck", "--no-reflogs"], repo.path());
    let combined = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(combined.contains("dangling commit"), "should show dangling commit");
}

#[test]
#[serial]
fn test_fsck_no_dangling() {
    let repo = create_committed_repo_via_cli();

    fs::write(repo.path().join("file2.txt"), "second file\n").unwrap();
    run_libra_command(&["add", "file2.txt"], repo.path());
    run_libra_command(&["commit", "-m", "second", "--no-verify"], repo.path());

    let log_output = run_libra_command(&["log", "--pretty=%H"], repo.path());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    let first_commit = stdout.lines().nth(1).unwrap().trim();
    run_libra_command(&["reset", "--hard", first_commit], repo.path());

    let output = run_libra_command(&["fsck", "--no-reflogs", "--no-dangling"], repo.path());
    let combined = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(!combined.contains("dangling"), "--no-dangling should suppress output");
}

#[test]
#[serial]
fn test_fsck_unreachable() {
    let repo = create_committed_repo_via_cli();

    fs::write(repo.path().join("file2.txt"), "second file\n").unwrap();
    run_libra_command(&["add", "file2.txt"], repo.path());
    run_libra_command(&["commit", "-m", "second", "--no-verify"], repo.path());

    let log_output = run_libra_command(&["log", "--pretty=%H"], repo.path());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    let first_commit = stdout.lines().nth(1).unwrap().trim();
    run_libra_command(&["reset", "--hard", first_commit], repo.path());

    let output = run_libra_command(&["fsck", "--no-reflogs", "--unreachable"], repo.path());
    let combined = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(combined.contains("unreachable commit"), "--unreachable should show all objects");
}

// ---------------------------------------------------------------------------
// Error handling tests
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_fsck_outside_repository() {
    let temp = tempdir().unwrap();
    let output = run_libra_command(&["fsck"], temp.path());
    assert_eq!(output.status.code(), Some(128), "should exit 128");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("fatal"), "should show fatal error");
}

#[test]
#[serial]
fn test_fsck_corrupted_object() {
    let repo = create_committed_repo_via_cli();

    let log_output = run_libra_command(&["log", "--pretty=%H"], repo.path());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    let commit_hash = stdout.lines().next().unwrap().trim();

    let objects_dir = repo.path().join(".libra").join("objects");
    let object_path = objects_dir.join(&commit_hash[0..2]).join(&commit_hash[2..]);

    if object_path.exists() {
        fs::write(&object_path, b"corrupted!!!").unwrap();
        let output = run_libra_command(&["fsck"], repo.path());
        assert!(!output.status.success(), "should fail on corrupted object");
        let combined = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        assert!(combined.contains("unknown") || combined.contains("bad"), "should report corruption");
    }
}

#[test]
#[serial]
fn test_fsck_missing_object() {
    let repo = create_committed_repo_via_cli();

    let log_output = run_libra_command(&["log", "--pretty=%H"], repo.path());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    let commit_hash = stdout.lines().next().unwrap().trim();

    let objects_dir = repo.path().join(".libra").join("objects");
    let object_path = objects_dir.join(&commit_hash[0..2]).join(&commit_hash[2..]);

    if object_path.exists() {
        fs::remove_file(&object_path).unwrap();
        let output = run_libra_command(&["fsck"], repo.path());
        assert!(!output.status.success(), "should fail on missing object");
        let combined = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        assert!(combined.contains("missing"), "should report missing");
    }
}

#[test]
#[serial]
fn test_fsck_invalid_object_id() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["fsck", "not-a-valid-hash"], repo.path());
    assert!(!output.status.success(), "should fail with invalid hash");
}

// ---------------------------------------------------------------------------
// Exit code tests
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_fsck_exit_code_zero_on_success() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["fsck"], repo.path());
    assert_eq!(output.status.code(), Some(0), "should exit 0");
}

#[test]
#[serial]
fn test_fsck_exit_code_zero_for_dangling() {
    let repo = create_committed_repo_via_cli();

    fs::write(repo.path().join("file2.txt"), "second file\n").unwrap();
    run_libra_command(&["add", "file2.txt"], repo.path());
    run_libra_command(&["commit", "-m", "second", "--no-verify"], repo.path());

    let log_output = run_libra_command(&["log", "--pretty=%H"], repo.path());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    let first_commit = stdout.lines().nth(1).unwrap().trim();
    run_libra_command(&["reset", "--hard", first_commit], repo.path());

    let output = run_libra_command(&["fsck", "--no-reflogs"], repo.path());
    assert_eq!(output.status.code(), Some(0), "dangling should not cause failure");
}

// ---------------------------------------------------------------------------
// --root and --tags tests
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_fsck_root_shows_root_commit() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["fsck", "--root"], repo.path());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("root "), "--root should report root commits");
}

#[test]
#[serial]
fn test_fsck_tags_reports_tags() {
    let repo = create_committed_repo_via_cli();
    run_libra_command(&["tag", "v1.0"], repo.path());
    let output = run_libra_command(&["fsck", "--tags"], repo.path());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("tagged commit") && stdout.contains("v1.0"), "--tags should report tags");
}

// ---------------------------------------------------------------------------
// --connectivity-only tests
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_fsck_connectivity_only_passes() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["fsck", "--connectivity-only"], repo.path());
    assert!(output.status.success(), "--connectivity-only should pass");
}

#[test]
#[serial]
fn test_fsck_connectivity_only_detects_missing() {
    let repo = create_committed_repo_via_cli();

    let log_output = run_libra_command(&["log", "--pretty=%H"], repo.path());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    let commit_hash = stdout.lines().next().unwrap().trim();

    let objects_dir = repo.path().join(".libra").join("objects");
    let object_path = objects_dir.join(&commit_hash[0..2]).join(&commit_hash[2..]);
    fs::remove_file(&object_path).unwrap();

    let output = run_libra_command(&["fsck", "--connectivity-only"], repo.path());
    assert!(!output.status.success(), "--connectivity-only should detect missing");
    let combined = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(combined.contains("missing"), "should report missing");
}

// ---------------------------------------------------------------------------
// --lost-found tests
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_fsck_lost_found_creates_files() {
    let repo = create_committed_repo_via_cli();

    fs::write(repo.path().join("file2.txt"), "second file\n").unwrap();
    run_libra_command(&["add", "file2.txt"], repo.path());
    run_libra_command(&["commit", "-m", "second", "--no-verify"], repo.path());

    let log_output = run_libra_command(&["log", "--pretty=%H"], repo.path());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    let first_commit = stdout.lines().nth(1).unwrap().trim();
    run_libra_command(&["reset", "--hard", first_commit], repo.path());

    run_libra_command(&["fsck", "--lost-found"], repo.path());

    let lost_found_dir = repo.path().join(".libra").join("lost-found");
    assert!(lost_found_dir.exists(), "lost-found should exist");
    assert!(lost_found_dir.join("commit").exists(), "lost-found/commit should exist");
    assert!(lost_found_dir.join("other").exists(), "lost-found/other should exist");
}

// ---------------------------------------------------------------------------
// SHA-256 tests
// ---------------------------------------------------------------------------

fn create_sha256_repo() -> tempfile::TempDir {
    let repo = tempdir().unwrap();
    run_libra_command(&["init", "--object-format", "sha256"], repo.path());
    configure_identity_via_cli(repo.path());
    fs::write(repo.path().join("file.txt"), "content\n").unwrap();
    run_libra_command(&["add", "file.txt"], repo.path());
    run_libra_command(&["commit", "-m", "init", "--no-verify"], repo.path());
    repo
}

#[test]
#[serial]
fn test_fsck_sha256_passes() {
    let repo = create_sha256_repo();
    let output = run_libra_command(&["fsck"], repo.path());
    assert!(output.status.success(), "SHA-256 repo should pass");
}

#[test]
#[serial]
fn test_fsck_sha256_missing_object() {
    let repo = create_sha256_repo();

    let log_output = run_libra_command(&["log", "--pretty=%H"], repo.path());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    let commit_hash = stdout.lines().next().unwrap().trim();

    let objects_dir = repo.path().join(".libra").join("objects");
    let object_path = objects_dir.join(&commit_hash[0..2]).join(&commit_hash[2..]);
    fs::remove_file(&object_path).unwrap();

    let output = run_libra_command(&["fsck"], repo.path());
    assert!(!output.status.success(), "should fail on missing SHA-256 object");
}
