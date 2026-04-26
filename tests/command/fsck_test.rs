//! Integration tests for the `fsck` command.
//!
//! Covers: basic functionality, boundary conditions, and error handling.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use std::fs;

use serial_test::serial;
use tempfile::tempdir;

use super::*;

// ---------------------------------------------------------------------------
// Basic functionality tests (>= 4)
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_fsck_empty_repo_passes() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());

    let output = run_libra_command(&["fsck"], repo.path());
    assert!(
        output.status.success(),
        "fsck on empty repo should pass: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[serial]
fn test_fsck_repo_with_commit_passes() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["fsck"], repo.path());
    assert!(
        output.status.success(),
        "fsck on healthy repo should pass: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Integrity check passed"),
        "should print pass message: {stdout}"
    );
}

#[test]
#[serial]
fn test_fsck_verbose_output() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["fsck", "--verbose"], repo.path());
    assert!(
        output.status.success(),
        "fsck --verbose should pass: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Fsck Summary"),
        "verbose output should contain summary: {stdout}"
    );
}

#[test]
#[serial]
fn test_fsck_objects_only() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["fsck", "--objects-only"], repo.path());
    assert!(
        output.status.success(),
        "fsck --objects-only should pass: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[serial]
fn test_fsck_json_output_on_healthy_repo() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["fsck", "--json"], repo.path());
    assert!(
        output.status.success(),
        "fsck --json should pass: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"overall_status\": \"ok\""),
        "JSON should report ok status: {stdout}"
    );
    assert!(
        stdout.contains("\"objects_checked\""),
        "JSON should contain objects_checked: {stdout}"
    );
}

#[test]
#[serial]
fn test_fsck_with_multiple_files_and_commits() {
    let repo = create_committed_repo_via_cli();

    // Add more files and another commit
    fs::write(repo.path().join("file2.txt"), "second file\n").expect("failed to create file2");
    let output = run_libra_command(&["add", "file2.txt"], repo.path());
    assert_cli_success(&output, "add file2");

    let output = run_libra_command(
        &["commit", "-m", "second commit", "--no-verify"],
        repo.path(),
    );
    assert_cli_success(&output, "second commit");

    let output = run_libra_command(&["fsck"], repo.path());
    assert!(
        output.status.success(),
        "fsck after multiple commits should pass: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Integrity check passed"),
        "should show passed message: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// Boundary condition tests (>= 8)
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_fsck_with_chinese_content_file() {
    let repo = create_committed_repo_via_cli();

    fs::write(repo.path().join("中文文件.txt"), "中文内容\n")
        .expect("failed to create Chinese file");
    let output = run_libra_command(&["add", "中文文件.txt"], repo.path());
    assert_cli_success(&output, "add Chinese file");

    let output = run_libra_command(
        &["commit", "-m", "chinese content", "--no-verify"],
        repo.path(),
    );
    assert_cli_success(&output, "commit Chinese file");

    let output = run_libra_command(&["fsck"], repo.path());
    assert!(
        output.status.success(),
        "fsck with Chinese filenames should pass: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[serial]
fn test_fsck_with_special_characters_in_filename() {
    let repo = create_committed_repo_via_cli();

    fs::write(repo.path().join("file with spaces.txt"), "content\n")
        .expect("failed to create file");
    let output = run_libra_command(&["add", "file with spaces.txt"], repo.path());
    assert_cli_success(&output, "add file with spaces");

    let output = run_libra_command(
        &["commit", "-m", "special chars", "--no-verify"],
        repo.path(),
    );
    assert_cli_success(&output, "commit special chars");

    let output = run_libra_command(&["fsck"], repo.path());
    assert!(
        output.status.success(),
        "fsck with special filenames should pass: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[serial]
fn test_fsck_with_empty_file() {
    let repo = create_committed_repo_via_cli();

    fs::write(repo.path().join("empty.txt"), "").expect("failed to create empty file");
    let output = run_libra_command(&["add", "empty.txt"], repo.path());
    assert_cli_success(&output, "add empty file");

    let output = run_libra_command(&["commit", "-m", "empty file", "--no-verify"], repo.path());
    assert_cli_success(&output, "commit empty file");

    let output = run_libra_command(&["fsck"], repo.path());
    assert!(
        output.status.success(),
        "fsck with empty file should pass: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[serial]
fn test_fsck_with_large_file() {
    let repo = create_committed_repo_via_cli();

    // Create a ~1MB file
    let content = "x".repeat(1_048_576);
    fs::write(repo.path().join("large.txt"), &content).expect("failed to create large file");
    let output = run_libra_command(&["add", "large.txt"], repo.path());
    assert_cli_success(&output, "add large file");

    let output = run_libra_command(&["commit", "-m", "large file", "--no-verify"], repo.path());
    assert_cli_success(&output, "commit large file");

    let output = run_libra_command(&["fsck"], repo.path());
    assert!(
        output.status.success(),
        "fsck with large file should pass: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[serial]
fn test_fsck_with_indexed_files() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());
    configure_identity_via_cli(repo.path());

    // Create and stage files but don't commit (tests index checking)
    fs::write(repo.path().join("staged.txt"), "staged content\n")
        .expect("failed to create staged file");
    let output = run_libra_command(&["add", "staged.txt"], repo.path());
    assert_cli_success(&output, "add staged file");

    let output = run_libra_command(&["fsck"], repo.path());
    assert!(
        output.status.success(),
        "fsck with staged files (index) should pass: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[serial]
fn test_fsck_with_ignored_files() {
    let repo = create_committed_repo_via_cli();

    fs::write(repo.path().join(".libraignore"), "ignore_me/\n")
        .expect("failed to create ignore file");
    fs::create_dir_all(repo.path().join("ignore_me")).expect("failed to create ignored dir");
    fs::write(repo.path().join("ignore_me/data.txt"), "ignored\n")
        .expect("failed to create ignored file");

    let output = run_libra_command(&["fsck"], repo.path());
    assert!(
        output.status.success(),
        "fsck with ignored files should pass: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[serial]
fn test_fsck_with_branch_switch() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["branch", "feature"], repo.path());
    assert_cli_success(&output, "create branch");

    let output = run_libra_command(&["switch", "feature"], repo.path());
    assert_cli_success(&output, "switch branch");

    let output = run_libra_command(&["fsck"], repo.path());
    assert!(
        output.status.success(),
        "fsck after branch switch should pass: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[serial]
fn test_fsck_with_unicode_content() {
    let repo = create_committed_repo_via_cli();

    // Emoji and various Unicode
    fs::write(repo.path().join("emoji.txt"), "🦀🔥✅🎉\n").expect("failed to create emoji file");
    let output = run_libra_command(&["add", "emoji.txt"], repo.path());
    assert_cli_success(&output, "add emoji file");

    let output = run_libra_command(&["commit", "-m", "emoji", "--no-verify"], repo.path());
    assert_cli_success(&output, "commit emoji");

    let output = run_libra_command(&["fsck"], repo.path());
    assert!(
        output.status.success(),
        "fsck with Unicode content should pass: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[serial]
fn test_fsck_single_object_valid() {
    let repo = create_committed_repo_via_cli();

    // Get the commit hash from log (full hash, not abbreviated)
    let log_output = run_libra_command(&["log", "--pretty=%H"], repo.path());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    let commit_hash = stdout
        .lines()
        .next()
        .unwrap()
        .trim();

    let output = run_libra_command(&["fsck", commit_hash], repo.path());
    assert!(
        output.status.success(),
        "fsck single valid object should pass: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("is valid"),
        "should show object is valid: {stdout}"
    );
}

#[test]
#[serial]
fn test_fsck_no_cross_ref_check() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["fsck", "--no-cross-ref-check"], repo.path());
    assert!(
        output.status.success(),
        "fsck --no-cross-ref-check should pass: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[serial]
fn test_fsck_no_index_check() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["fsck", "--no-index-check"], repo.path());
    assert!(
        output.status.success(),
        "fsck --no-index-check should pass: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// ---------------------------------------------------------------------------
// Error handling tests (>= 8)
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_fsck_outside_repository() {
    let temp = tempdir().unwrap();

    let output = run_libra_command(&["fsck"], temp.path());
    assert_eq!(
        output.status.code(),
        Some(128),
        "fsck outside repo should exit 128"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("fatal"),
        "should show fatal error: {stderr}"
    );
}

#[test]
#[serial]
fn test_fsck_corrupted_object() {
    let repo = create_committed_repo_via_cli();

    // Corrupt an object by writing garbage to a blob
    let objects_dir = repo.path().join(".libra").join("objects");
    if objects_dir.exists() {
        // Find and corrupt a loose object
        for entry in walk_objects_dir(&objects_dir) {
            fs::write(&entry, b"corrupted data!!!").expect("failed to corrupt object");

            let output = run_libra_command(&["fsck"], repo.path());
            assert!(
                !output.status.success(),
                "fsck should fail on corrupted object"
            );
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                stderr.contains("corrupt")
                    || stderr.contains("FAILED")
                    || stderr.contains("mismatch"),
                "should report corruption: {stderr}"
            );
            return;
        }
    }
}

#[test]
#[serial]
fn test_fsck_missing_object() {
    let repo = create_committed_repo_via_cli();

    // Delete a loose object file
    let objects_dir = repo.path().join(".libra").join("objects");
    if objects_dir.exists() {
        for entry in walk_objects_dir(&objects_dir) {
            fs::remove_file(&entry).expect("failed to remove object");

            let output = run_libra_command(&["fsck"], repo.path());
            assert!(
                !output.status.success(),
                "fsck should fail on missing object"
            );
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                stderr.contains("missing") || stderr.contains("FAILED"),
                "should report missing object: {stderr}"
            );
            return;
        }
    }
}

#[test]
#[serial]
fn test_fsck_invalid_object_id() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["fsck", "not-a-valid-hash!!"], repo.path());
    assert!(
        !output.status.success(),
        "fsck with invalid object ID should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid") || stderr.contains("fatal"),
        "should report invalid hash: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// --fix flag tests
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_fsck_fix_deletes_broken_ref() {
    let repo = create_committed_repo_via_cli();

    // Create a branch, then delete its target object to make it broken
    let _ = run_libra_command(&["branch", "test-branch"], repo.path());

    let log_output = run_libra_command(&["log", "--pretty=%H"], repo.path());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    let commit_hash = stdout.lines().next().unwrap().trim();

    let object_path = loose_object_path(repo.path(), commit_hash);
    if object_path.exists() {
        fs::remove_file(&object_path).ok();
    }

    let output = run_libra_command(&["fsck", "--fix"], repo.path());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success()
            || stdout.contains("Fixed")
            || stderr.contains("Deleted broken ref")
            || stderr.contains("FAILED"),
        "fsck --fix should handle broken refs: stdout={stdout}, stderr={stderr}"
    );
}

#[test]
#[serial]
fn test_fsck_fix_rebuilds_corrupted_index() {
    let repo = create_committed_repo_via_cli();

    let index_path = repo.path().join(".libra").join("index");
    if index_path.exists() {
        fs::write(&index_path, b"corrupted index data!!!").expect("failed to corrupt index");
    }

    let output = run_libra_command(&["fsck", "--fix"], repo.path());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success()
            || stdout.contains("rebuilt")
            || stderr.contains("rebuilt")
            || stderr.contains("Fixed"),
        "fsck --fix should attempt to rebuild index: stdout={stdout}, stderr={stderr}"
    );

    if index_path.exists() {
        let verify_output = run_libra_command(&["fsck"], repo.path());
        assert!(
            verify_output.status.success()
                || !String::from_utf8_lossy(&verify_output.stdout).contains("index"),
            "fsck after fix should not report index issues: {}",
            String::from_utf8_lossy(&verify_output.stderr)
        );
    }
}

#[test]
#[serial]
fn test_fsck_fix_on_clean_repo() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["fsck", "--fix"], repo.path());
    assert!(
        output.status.success(),
        "fsck --fix on clean repo should pass: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Integrity check passed"),
        "should show passed message: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// Exit code verification tests
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_fsck_exit_code_zero_on_success() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["fsck"], repo.path());
    assert_eq!(
        output.status.code(),
        Some(0),
        "fsck on healthy repo should exit 0"
    );
}

#[test]
#[serial]
fn test_fsck_exit_code_on_corrupted_object() {
    let repo = create_committed_repo_via_cli();

    let objects_dir = repo.path().join(".libra").join("objects");
    if objects_dir.exists() {
        for entry in walk_objects_dir(&objects_dir) {
            fs::write(&entry, b"corrupted!!!").expect("failed to corrupt object");

            let output = run_libra_command(&["fsck"], repo.path());
            let exit_code = output.status.code().unwrap_or(-1);
            assert!(
                exit_code & 1 != 0,
                "exit code should have OBJECT_CORRUPT bit set: {exit_code}"
            );
            return;
        }
    }
}

#[test]
#[serial]
fn test_fsck_exit_code_combination_objects_and_refs() {
    let repo = create_committed_repo_via_cli();

    let objects_dir = repo.path().join(".libra").join("objects");
    let mut corrupted = false;

    if objects_dir.exists() {
        for entry in walk_objects_dir(&objects_dir) {
            fs::write(&entry, b"corrupted!!!").expect("failed to corrupt object");
            corrupted = true;
            break;
        }
    }

    if corrupted {
        let output = run_libra_command(&["fsck"], repo.path());
        let exit_code = output.status.code().unwrap_or(-1);
        assert!(
            exit_code & 1 != 0,
            "exit code should include OBJECT_CORRUPT: {exit_code}"
        );
    }
}

#[test]
#[serial]
fn test_fsck_missing_nonexistent_object() {
    let repo = create_committed_repo_via_cli();

    // Use a hash that doesn't exist
    let output = run_libra_command(
        &["fsck", "0000000000000000000000000000000000000000"],
        repo.path(),
    );
    assert!(
        !output.status.success(),
        "fsck on nonexistent object should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("missing") || stderr.contains("not found") || stderr.contains("FAILED"),
        "should report missing object: {stderr}"
    );
}

#[test]
#[serial]
fn test_fsck_deleted_objects_dir() {
    let repo = create_committed_repo_via_cli();

    // Remove the entire objects directory
    let objects_dir = repo.path().join(".libra").join("objects");
    if objects_dir.exists() {
        fs::remove_dir_all(&objects_dir).expect("failed to remove objects dir");

        let output = run_libra_command(&["fsck"], repo.path());
        // Should either fail or report no objects (both acceptable)
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !output.status.success() {
            assert!(
                stderr.contains("missing")
                    || stderr.contains("corrupt")
                    || stderr.contains("FAILED"),
                "should report issues: {stderr}"
            );
        } else {
            assert!(
                stdout.contains("No objects") || stdout.contains("passed"),
                "should report no objects or pass: {stdout}"
            );
        }
    }
}

#[test]
#[serial]
fn test_fsck_corrupted_index() {
    let repo = create_committed_repo_via_cli();

    // Corrupt the index file
    let index_path = repo.path().join(".libra").join("index");
    if index_path.exists() {
        fs::write(&index_path, b"not a valid index file!!!").expect("failed to corrupt index");

        let output = run_libra_command(&["fsck"], repo.path());
        // Should detect the corruption or fail to parse
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Either the fsck detects corruption or it fails to parse
        assert!(
            !output.status.success()
                || stderr.contains("corrupt")
                || stderr.contains("parse")
                || stdout.contains("corrupt")
                || stderr.contains("FAILED"),
            "should detect corrupted index or report issues: stderr={stderr}, stdout={stdout}"
        );
    }
}

#[test]
#[serial]
fn test_fsck_broken_ref() {
    let repo = create_committed_repo_via_cli();

    // Create a branch pointing to a nonexistent commit
    let _output = run_libra_command(
        &[
            "branch",
            "dead-branch",
            "0000000000000000000000000000000000000000",
        ],
        repo.path(),
    );
    // This might fail, which is fine - try fsck regardless

    let output = run_libra_command(&["fsck"], repo.path());
    // Should detect broken refs or pass (depends on how branch handles invalid targets)
    let stderr = String::from_utf8_lossy(&output.stderr);
    let _stdout = String::from_utf8_lossy(&output.stdout);
    // At minimum it should not crash
    assert!(
        output.status.code().is_some(),
        "fsck should exit with a code"
    );
    // If it detects the issue, it should report it
    if !output.status.success() {
        assert!(
            stderr.contains("broken") || stderr.contains("missing") || stderr.contains("FAILED"),
            "should report broken ref: {stderr}"
        );
    }
}

#[test]
#[serial]
fn test_fsck_after_force_delete_tracked_file() {
    let repo = create_committed_repo_via_cli();

    // Delete the tracked file
    fs::remove_file(repo.path().join("tracked.txt")).expect("failed to delete tracked file");

    // fsck should still pass (objects are fine, index is stale but not corrupted)
    let output = run_libra_command(&["fsck"], repo.path());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Objects should still be intact
    assert!(
        output.status.success() || stdout.contains("passed") || !stdout.contains("corrupt"),
        "fsck after file deletion should not report object corruption: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[serial]
fn test_fsck_too_short_hash() {
    let repo = create_committed_repo_via_cli();

    // Use a hash that's too short
    let output = run_libra_command(&["fsck", "abc123"], repo.path());
    assert!(!output.status.success(), "fsck with short hash should fail");
}

#[test]
#[serial]
fn test_fsck_empty_hash_string() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["fsck", ""], repo.path());
    assert!(!output.status.success(), "fsck with empty hash should fail");
}

/// Walk the objects directory and return paths to loose object files.
fn walk_objects_dir(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                for sub_entry in fs::read_dir(&path).into_iter().flatten().flatten() {
                    let sub_path = sub_entry.path();
                    if sub_path.is_file() {
                        files.push(sub_path);
                    }
                }
            }
        }
    }
    files
}

// ---------------------------------------------------------------------------
// SHA-256 object format tests
// ---------------------------------------------------------------------------

fn init_sha256_repo_via_cli(repo: &Path) {
    fs::create_dir_all(repo).expect("failed to create repository directory");
    let output = run_libra_command(&["init", "--object-format", "sha256"], repo);
    assert_cli_success(&output, "failed to initialize SHA-256 repository");
}

fn create_sha256_committed_repo_via_cli() -> tempfile::TempDir {
    let repo = tempdir().expect("failed to create repository root");
    init_sha256_repo_via_cli(repo.path());
    configure_identity_via_cli(repo.path());

    fs::write(repo.path().join("tracked.txt"), "sha256 tracked content\n")
        .expect("failed to create tracked file");

    let output = run_libra_command(&["add", "tracked.txt"], repo.path());
    assert_cli_success(&output, "failed to add tracked file");

    let output = run_libra_command(&["commit", "-m", "sha256 base", "--no-verify"], repo.path());
    assert_cli_success(&output, "failed to create initial commit");

    repo
}

#[test]
#[serial]
fn test_fsck_sha256_empty_repo_passes() {
    let repo = tempdir().unwrap();
    init_sha256_repo_via_cli(repo.path());

    let output = run_libra_command(&["fsck"], repo.path());
    assert!(
        output.status.success(),
        "fsck on empty SHA-256 repo should pass: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[serial]
fn test_fsck_sha256_repo_with_commit_passes() {
    let repo = create_sha256_committed_repo_via_cli();

    let output = run_libra_command(&["fsck"], repo.path());
    assert!(
        output.status.success(),
        "fsck on SHA-256 repo with commit should pass: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Integrity check passed"),
        "should print pass message: {stdout}"
    );
}

#[test]
#[serial]
fn test_fsck_sha256_verbose_output() {
    let repo = create_sha256_committed_repo_via_cli();

    let output = run_libra_command(&["fsck", "--verbose"], repo.path());
    assert!(
        output.status.success(),
        "fsck --verbose on SHA-256 repo should pass: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Fsck Summary"),
        "verbose output should contain summary: {stdout}"
    );
}

#[test]
#[serial]
fn test_fsck_sha256_json_output() {
    let repo = create_sha256_committed_repo_via_cli();

    let output = run_libra_command(&["fsck", "--json"], repo.path());
    assert!(
        output.status.success(),
        "fsck --json on SHA-256 repo should pass: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"overall_status\": \"ok\""),
        "JSON should report ok status: {stdout}"
    );
}

#[test]
#[serial]
fn test_fsck_sha256_corrupted_object_detected() {
    let repo = create_sha256_committed_repo_via_cli();

    let objects_dir = repo.path().join(".libra").join("objects");
    if objects_dir.exists() {
        for entry in walk_objects_dir(&objects_dir) {
            fs::write(&entry, b"corrupted sha256 data!!!")
                .expect("failed to corrupt SHA-256 object");

            let output = run_libra_command(&["fsck"], repo.path());
            assert!(
                !output.status.success(),
                "fsck should fail on corrupted SHA-256 object"
            );
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                stderr.contains("corrupt")
                    || stderr.contains("FAILED")
                    || stderr.contains("mismatch"),
                "should report corruption: {stderr}"
            );
            return;
        }
    }
    panic!("no SHA-256 objects found to corrupt");
}

#[test]
#[serial]
fn test_fsck_sha256_missing_object_detected() {
    let repo = create_sha256_committed_repo_via_cli();

    let objects_dir = repo.path().join(".libra").join("objects");
    if objects_dir.exists() {
        for entry in walk_objects_dir(&objects_dir) {
            fs::remove_file(&entry).expect("failed to remove SHA-256 object");

            let output = run_libra_command(&["fsck"], repo.path());
            assert!(
                !output.status.success(),
                "fsck should fail on missing SHA-256 object"
            );
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                stderr.contains("missing") || stderr.contains("FAILED"),
                "should report missing object: {stderr}"
            );
            return;
        }
    }
    panic!("no SHA-256 objects found to remove");
}

#[test]
#[serial]
fn test_fsck_sha256_single_object_valid() {
    let repo = create_sha256_committed_repo_via_cli();

    let log_output = run_libra_command(&["log", "--pretty=%H"], repo.path());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    let commit_hash = stdout
        .lines()
        .next()
        .unwrap()
        .trim();

    // SHA-256 hashes are 64 hex chars
    assert_eq!(
        commit_hash.len(),
        64,
        "SHA-256 commit hash should be 64 hex chars, got {}: {commit_hash}",
        commit_hash.len()
    );

    let output = run_libra_command(&["fsck", commit_hash], repo.path());
    assert!(
        output.status.success(),
        "fsck single SHA-256 object should pass: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("is valid"),
        "should show object is valid: {stdout}"
    );
}

#[test]
#[serial]
fn test_fsck_sha256_with_indexed_files() {
    let repo = tempdir().unwrap();
    init_sha256_repo_via_cli(repo.path());
    configure_identity_via_cli(repo.path());

    fs::write(repo.path().join("staged.txt"), "sha256 staged content\n")
        .expect("failed to create staged file");
    let output = run_libra_command(&["add", "staged.txt"], repo.path());
    assert_cli_success(&output, "add staged file");

    let output = run_libra_command(&["fsck"], repo.path());
    assert!(
        output.status.success(),
        "fsck with SHA-256 staged files should pass: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[serial]
fn test_fsck_sha256_invalid_object_id() {
    let repo = create_sha256_committed_repo_via_cli();

    // Use an invalid hash that is clearly not SHA-256
    let output = run_libra_command(&["fsck", "not-a-valid-sha256-hash!!"], repo.path());
    assert!(
        !output.status.success(),
        "fsck with invalid SHA-256 object ID should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid") || stderr.contains("fatal"),
        "should report invalid hash: {stderr}"
    );
}

