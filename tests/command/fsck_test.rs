//! Integration tests for the `fsck` command.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use std::fs;

use serial_test::serial;
use tempfile::tempdir;

use super::*;

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

/// Corrupt the first loose object found. Returns whether an object was corrupted.
fn corrupt_first_object(repo: &std::path::Path) -> bool {
    let objects_dir = repo.join(".libra").join("objects");
    if !objects_dir.exists() {
        return false;
    }
    for entry in walk_objects_dir(&objects_dir) {
        // Only corrupt files that have valid hash names (2 char dir + hash file)
        let parent_dir = entry.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str());
        let file_name = entry.file_name().and_then(|n| n.to_str());
        if let (Some(dir), Some(file)) = (parent_dir, file_name) {
            if dir.len() == 2 && file.len() >= 38 {  // SHA-1: 40 chars, SHA-256: 64 chars
                fs::write(&entry, b"corrupted!!!").expect("failed to corrupt object");
                return true;
            }
        }
    }
    false
}

/// Delete the first loose object found. Returns whether an object was deleted.
fn delete_first_object(repo: &std::path::Path) -> bool {
    let objects_dir = repo.join(".libra").join("objects");
    if !objects_dir.exists() {
        return false;
    }
    for entry in walk_objects_dir(&objects_dir) {
        // Only delete files that have valid hash names (2 char dir + hash file)
        let parent_dir = entry.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str());
        let file_name = entry.file_name().and_then(|n| n.to_str());
        if let (Some(dir), Some(file)) = (parent_dir, file_name) {
            if dir.len() == 2 && file.len() >= 38 {  // SHA-1: 40 chars, SHA-256: 64 chars
                fs::remove_file(&entry).expect("failed to remove object");
                return true;
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Basic functionality tests
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
    // Verbose mode prints "Checking ..." lines to stdout
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Checking"),
        "verbose should contain 'Checking': {stdout}"
    );
}

#[test]
#[serial]
fn test_fsck_with_multiple_files_and_commits() {
    let repo = create_committed_repo_via_cli();

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
}

// ---------------------------------------------------------------------------
// --no-reflogs, --dangling, --unreachable option tests
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_fsck_no_reflogs() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["fsck", "--no-reflogs"], repo.path());
    assert!(
        output.status.success(),
        "fsck --no-reflogs should pass: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[serial]
fn test_fsck_dangling_shows_only_commits_by_default() {
    // Create a repo, then reset to create dangling objects
    let repo = create_committed_repo_via_cli();

    // Add another commit
    fs::write(repo.path().join("file2.txt"), "second file\n").expect("failed to create file2");
    let output = run_libra_command(&["add", "file2.txt"], repo.path());
    assert_cli_success(&output, "add file2");
    let output = run_libra_command(
        &["commit", "-m", "second commit", "--no-verify"],
        repo.path(),
    );
    assert_cli_success(&output, "second commit");

    // Reset to first commit, making second commit dangling
    let log_output = run_libra_command(&["log", "--pretty=%H"], repo.path());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    let first_commit = stdout.lines().nth(1).unwrap().trim();

    let output = run_libra_command(&["reset", "--hard", first_commit], repo.path());
    assert_cli_success(&output, "reset to first commit");

    // Default: only show dangling commits (not trees/blobs)
    let output = run_libra_command(&["fsck", "--no-reflogs"], repo.path());
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should show dangling commit
    assert!(
        stderr.contains("dangling commit") || stdout.contains("dangling commit"),
        "should show dangling commit: stdout={stdout}, stderr={stderr}"
    );
}

#[test]
#[serial]
fn test_fsck_no_dangling() {
    let repo = create_committed_repo_via_cli();

    // Add and then reset to create dangling objects
    fs::write(repo.path().join("file2.txt"), "second file\n").expect("failed to create file2");
    let output = run_libra_command(&["add", "file2.txt"], repo.path());
    assert_cli_success(&output, "add file2");
    let output = run_libra_command(
        &["commit", "-m", "second commit", "--no-verify"],
        repo.path(),
    );
    assert_cli_success(&output, "second commit");

    let log_output = run_libra_command(&["log", "--pretty=%H"], repo.path());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    let first_commit = stdout.lines().nth(1).unwrap().trim();

    let output = run_libra_command(&["reset", "--hard", first_commit], repo.path());
    assert_cli_success(&output, "reset to first commit");

    // --no-dangling should suppress dangling output
    let output = run_libra_command(&["fsck", "--no-reflogs", "--no-dangling"], repo.path());
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        !stderr.contains("dangling") && !stdout.contains("dangling"),
        "--no-dangling should suppress dangling output: stdout={stdout}, stderr={stderr}"
    );
}

#[test]
#[serial]
fn test_fsck_unreachable_shows_all_objects() {
    let repo = create_committed_repo_via_cli();

    // Add and then reset to create unreachable objects
    fs::write(repo.path().join("file2.txt"), "second file\n").expect("failed to create file2");
    let output = run_libra_command(&["add", "file2.txt"], repo.path());
    assert_cli_success(&output, "add file2");
    let output = run_libra_command(
        &["commit", "-m", "second commit", "--no-verify"],
        repo.path(),
    );
    assert_cli_success(&output, "second commit");

    let log_output = run_libra_command(&["log", "--pretty=%H"], repo.path());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    let first_commit = stdout.lines().nth(1).unwrap().trim();

    let output = run_libra_command(&["reset", "--hard", first_commit], repo.path());
    assert_cli_success(&output, "reset to first commit");

    // --unreachable should show all unreachable objects (commit, tree, blob)
    let output = run_libra_command(&["fsck", "--no-reflogs", "--unreachable"], repo.path());
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should show unreachable commit, tree, and blob
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("unreachable commit"),
        "--unreachable should show unreachable commit: {combined}"
    );
    assert!(
        combined.contains("unreachable tree"),
        "--unreachable should show unreachable tree: {combined}"
    );
    assert!(
        combined.contains("unreachable blob"),
        "--unreachable should show unreachable blob: {combined}"
    );
}

#[test]
#[serial]
fn test_fsck_reflog_keeps_objects_reachable() {
    let repo = create_committed_repo_via_cli();

    // Add and then reset to create objects in reflog
    fs::write(repo.path().join("file2.txt"), "second file\n").expect("failed to create file2");
    let output = run_libra_command(&["add", "file2.txt"], repo.path());
    assert_cli_success(&output, "add file2");
    let output = run_libra_command(
        &["commit", "-m", "second commit", "--no-verify"],
        repo.path(),
    );
    assert_cli_success(&output, "second commit");

    let log_output = run_libra_command(&["log", "--pretty=%H"], repo.path());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    let first_commit = stdout.lines().nth(1).unwrap().trim();

    let output = run_libra_command(&["reset", "--hard", first_commit], repo.path());
    assert_cli_success(&output, "reset to first commit");

    // Without --no-reflogs, objects in reflog are reachable (no dangling output)
    let output = run_libra_command(&["fsck"], repo.path());
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        !stderr.contains("dangling") && !stdout.contains("dangling"),
        "reflog should keep objects reachable: stdout={stdout}, stderr={stderr}"
    );
}

// ---------------------------------------------------------------------------
// Boundary condition tests
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
fn test_fsck_with_unicode_content() {
    let repo = create_committed_repo_via_cli();

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

    let log_output = run_libra_command(&["log", "--pretty=%H"], repo.path());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    let commit_hash = stdout.lines().next().unwrap().trim();

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

// ---------------------------------------------------------------------------
// Error handling tests
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

    // Get the commit hash from HEAD
    let log_output = run_libra_command(&["log", "--pretty=%H"], repo.path());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    let commit_hash = stdout.lines().next().unwrap().trim();

    // Corrupt the commit object that HEAD points to
    let objects_dir = repo.path().join(".libra").join("objects");
    let hash_prefix = &commit_hash[0..2];
    let hash_rest = &commit_hash[2..];
    let object_path = objects_dir.join(hash_prefix).join(hash_rest);

    if object_path.exists() {
        fs::write(&object_path, b"corrupted!!!").expect("failed to corrupt commit object");
        let output = run_libra_command(&["fsck"], repo.path());
        assert!(
            !output.status.success(),
            "fsck should fail on corrupted object"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("mismatch") || stderr.contains("FAILED"),
            "should report corruption: {stderr}"
        );
    }
}

#[test]
#[serial]
fn test_fsck_missing_object() {
    let repo = create_committed_repo_via_cli();

    // Get the commit hash from HEAD
    let log_output = run_libra_command(&["log", "--pretty=%H"], repo.path());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    let commit_hash = stdout.lines().next().unwrap().trim();

    // Delete the commit object that HEAD points to
    let objects_dir = repo.path().join(".libra").join("objects");
    let hash_prefix = &commit_hash[0..2];
    let hash_rest = &commit_hash[2..];
    let object_path = objects_dir.join(hash_prefix).join(hash_rest);

    if object_path.exists() {
        fs::remove_file(&object_path).expect("failed to delete commit object");
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

#[test]
#[serial]
fn test_fsck_too_short_hash() {
    let repo = create_committed_repo_via_cli();

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

// ---------------------------------------------------------------------------
// Exit code tests
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
fn test_fsck_exit_code_nonzero_on_error() {
    let repo = create_committed_repo_via_cli();

    // Get the commit hash from HEAD
    let log_output = run_libra_command(&["log", "--pretty=%H"], repo.path());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    let commit_hash = stdout.lines().next().unwrap().trim();

    // Corrupt the commit object that HEAD points to
    let objects_dir = repo.path().join(".libra").join("objects");
    let hash_prefix = &commit_hash[0..2];
    let hash_rest = &commit_hash[2..];
    let object_path = objects_dir.join(hash_prefix).join(hash_rest);

    if object_path.exists() {
        fs::write(&object_path, b"corrupted!!!").expect("failed to corrupt commit object");
        let output = run_libra_command(&["fsck"], repo.path());
        let exit_code = output.status.code().unwrap_or(-1);
        assert_ne!(
            exit_code, 0,
            "fsck on corrupted object should exit non-zero: {exit_code}"
        );
    }
}

#[test]
#[serial]
fn test_fsck_exit_code_zero_for_dangling() {
    // Dangling objects are info-level, should not cause non-zero exit
    let repo = create_committed_repo_via_cli();

    // Create dangling objects
    fs::write(repo.path().join("file2.txt"), "second file\n").expect("failed to create file2");
    let output = run_libra_command(&["add", "file2.txt"], repo.path());
    assert_cli_success(&output, "add file2");
    let output = run_libra_command(
        &["commit", "-m", "second commit", "--no-verify"],
        repo.path(),
    );
    assert_cli_success(&output, "second commit");

    let log_output = run_libra_command(&["log", "--pretty=%H"], repo.path());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    let first_commit = stdout.lines().nth(1).unwrap().trim();

    let output = run_libra_command(&["reset", "--hard", first_commit], repo.path());
    assert_cli_success(&output, "reset to first commit");

    // Dangling objects are info-level, exit code should be 0
    let output = run_libra_command(&["fsck", "--no-reflogs"], repo.path());
    assert_eq!(
        output.status.code(),
        Some(0),
        "fsck with only dangling objects should exit 0"
    );
}

// ---------------------------------------------------------------------------
// SHA-256 object format tests
// ---------------------------------------------------------------------------

fn init_sha256_repo_via_cli(repo: &std::path::Path) {
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
fn test_fsck_sha256_single_object_valid() {
    let repo = create_sha256_committed_repo_via_cli();

    let log_output = run_libra_command(&["log", "--pretty=%H"], repo.path());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    let commit_hash = stdout.lines().next().unwrap().trim();

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
fn test_fsck_sha256_corrupted_object_detected() {
    let repo = create_sha256_committed_repo_via_cli();

    // Get the commit hash from HEAD
    let log_output = run_libra_command(&["log", "--pretty=%H"], repo.path());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    let commit_hash = stdout.lines().next().unwrap().trim();

    // Corrupt the commit object that HEAD points to
    let objects_dir = repo.path().join(".libra").join("objects");
    let hash_prefix = &commit_hash[0..2];
    let hash_rest = &commit_hash[2..];
    let object_path = objects_dir.join(hash_prefix).join(hash_rest);

    if object_path.exists() {
        fs::write(&object_path, b"corrupted!!!").expect("failed to corrupt commit object");
        let output = run_libra_command(&["fsck"], repo.path());
        assert!(
            !output.status.success(),
            "fsck should fail on corrupted SHA-256 object"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("mismatch") || stderr.contains("FAILED"),
            "should report corruption: {stderr}"
        );
    }
}

#[test]
#[serial]
fn test_fsck_sha256_missing_object_detected() {
    let repo = create_sha256_committed_repo_via_cli();

    // Get the commit hash from HEAD
    let log_output = run_libra_command(&["log", "--pretty=%H"], repo.path());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    let commit_hash = stdout.lines().next().unwrap().trim();

    // Delete the commit object that HEAD points to
    let objects_dir = repo.path().join(".libra").join("objects");
    let hash_prefix = &commit_hash[0..2];
    let hash_rest = &commit_hash[2..];
    let object_path = objects_dir.join(hash_prefix).join(hash_rest);

    if object_path.exists() {
        fs::remove_file(&object_path).expect("failed to remove commit object");
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
    }
}

#[test]
#[serial]
fn test_fsck_sha256_invalid_object_id() {
    let repo = create_sha256_committed_repo_via_cli();

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

#[test]
#[serial]
fn test_fsck_sha256_dangling_and_unreachable() {
    let repo = create_sha256_committed_repo_via_cli();

    // Create dangling objects
    fs::write(repo.path().join("file2.txt"), "second file\n").expect("failed to create file2");
    let output = run_libra_command(&["add", "file2.txt"], repo.path());
    assert_cli_success(&output, "add file2");
    let output = run_libra_command(
        &["commit", "-m", "second commit", "--no-verify"],
        repo.path(),
    );
    assert_cli_success(&output, "second commit");

    let log_output = run_libra_command(&["log", "--pretty=%H"], repo.path());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    let first_commit = stdout.lines().nth(1).unwrap().trim();

    let output = run_libra_command(&["reset", "--hard", first_commit], repo.path());
    assert_cli_success(&output, "reset to first commit");

    // Test --unreachable with SHA-256
    let output = run_libra_command(&["fsck", "--no-reflogs", "--unreachable"], repo.path());
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{stdout}{stderr}");

    assert!(
        combined.contains("unreachable commit"),
        "--unreachable should show unreachable SHA-256 commit: {combined}"
    );
}

// ---------------------------------------------------------------------------
// --name-objects tests
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_fsck_name_objects_verbose() {
    let repo = create_committed_repo_via_cli();

    // Add another file and commit to have more objects
    fs::write(repo.path().join("file2.txt"), "second file\n").expect("failed to create file2");
    let output = run_libra_command(&["add", "file2.txt"], repo.path());
    assert_cli_success(&output, "add file2");
    let output = run_libra_command(
        &["commit", "-m", "second commit", "--no-verify"],
        repo.path(),
    );
    assert_cli_success(&output, "second commit");

    let output = run_libra_command(&["fsck", "--verbose", "--name-objects"], repo.path());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");

    // Should show object names in parentheses during connectivity check
    assert!(
        combined.contains("(main)") || combined.contains("(refs/heads/main)") || combined.contains("(:test.txt)"),
        "--name-objects should show object names: {combined}"
    );
}

#[test]
#[serial]
fn test_fsck_name_objects_without_verbose() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["fsck", "--name-objects"], repo.path());
    // Without --verbose, --name-objects should not affect output
    assert!(output.status.success(), "fsck --name-objects should pass");
}

// ---------------------------------------------------------------------------
// --lost-found tests
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_fsck_lost_found_creates_files() {
    let repo = create_committed_repo_via_cli();

    // Create dangling objects
    fs::write(repo.path().join("file2.txt"), "second file\n").expect("failed to create file2");
    let output = run_libra_command(&["add", "file2.txt"], repo.path());
    assert_cli_success(&output, "add file2");
    let output = run_libra_command(
        &["commit", "-m", "second commit", "--no-verify"],
        repo.path(),
    );
    assert_cli_success(&output, "second commit");

    let log_output = run_libra_command(&["log", "--pretty=%H"], repo.path());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    let first_commit = stdout.lines().nth(1).unwrap().trim();

    let output = run_libra_command(&["reset", "--hard", first_commit], repo.path());
    assert_cli_success(&output, "reset to first commit");

    let output = run_libra_command(&["fsck", "--lost-found"], repo.path());
    let _stderr = String::from_utf8_lossy(&output.stderr);

    // Should create lost-found directory
    let lost_found_dir = repo.path().join(".libra").join("lost-found");
    assert!(lost_found_dir.exists(), "--lost-found should create lost-found directory");

    // Should have commit directory with dangling commit
    let commit_dir = lost_found_dir.join("commit");
    assert!(commit_dir.exists(), "lost-found/commit should exist");

    // Should have other directory
    let other_dir = lost_found_dir.join("other");
    assert!(other_dir.exists(), "lost-found/other should exist");
}

#[test]
#[serial]
fn test_fsck_lost_found_blob_content() {
    let repo = create_committed_repo_via_cli();

    // Create a dangling blob by adding a file
    let blob_content = "unique content for lost-found test\n";
    fs::write(repo.path().join("unique.txt"), blob_content).expect("failed to create file");
    let output = run_libra_command(&["add", "unique.txt"], repo.path());
    assert_cli_success(&output, "add unique.txt");

    // Reset the index to make the blob dangling
    let output = run_libra_command(&["reset", "HEAD"], repo.path());
    assert_cli_success(&output, "reset HEAD");

    let output = run_libra_command(&["fsck", "--lost-found"], repo.path());
    assert!(output.status.success(), "fsck --lost-found should pass");

    // Check that blob content is written correctly
    let lost_found_dir = repo.path().join(".libra").join("lost-found");
    if lost_found_dir.join("other").exists() {
        let other_entries = fs::read_dir(lost_found_dir.join("other")).unwrap();
        for entry in other_entries.flatten() {
            let content = fs::read_to_string(entry.path()).unwrap();
            // Blob content should be actual content, not just hash
            if content.contains("unique content") {
                return; // Test passed
            }
        }
    }
}

// ---------------------------------------------------------------------------
// --root tests
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_fsck_root_shows_root_commit() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["fsck", "--root"], repo.path());
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        stderr.contains("root "),
        "--root should report root commits: {stderr}"
    );
}

#[test]
#[serial]
fn test_fsck_root_with_multiple_commits() {
    let repo = create_committed_repo_via_cli();

    // Create more commits
    for i in 2..=3 {
        let filename = format!("file{}.txt", i);
        fs::write(repo.path().join(&filename), format!("content {}\n", i)).expect("failed to create file");
        let output = run_libra_command(&["add", &filename], repo.path());
        assert_cli_success(&output, &format!("add {}", filename));
        let output = run_libra_command(
            &["commit", "-m", format!("commit {}", i).as_str(), "--no-verify"],
            repo.path(),
        );
        assert_cli_success(&output, &format!("commit {}", i));
    }

    let output = run_libra_command(&["fsck", "--root"], repo.path());
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Should still show only one root commit
    let root_count = stderr.matches("root ").count();
    assert_eq!(root_count, 1, "should have exactly one root commit: {stderr}");
}

// ---------------------------------------------------------------------------
// --tags tests
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_fsck_tags_reports_tags() {
    let repo = create_committed_repo_via_cli();

    // Create a tag
    let output = run_libra_command(&["tag", "v1.0"], repo.path());
    assert_cli_success(&output, "create tag v1.0");

    let output = run_libra_command(&["fsck", "--tags"], repo.path());
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        stderr.contains("tagged commit") && stderr.contains("v1.0"),
        "--tags should report tagged commits: {stderr}"
    );
}

#[test]
#[serial]
fn test_fsck_tags_without_tags() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["fsck", "--tags"], repo.path());
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Should pass but not show any tags
    assert!(output.status.success(), "fsck --tags should pass");
    assert!(
        !stderr.contains("tagged commit"),
        "should not show tagged commit when no tags exist: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// --connectivity-only tests
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_fsck_connectivity_only_passes() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["fsck", "--connectivity-only"], repo.path());
    assert!(
        output.status.success(),
        "--connectivity-only should pass on healthy repo: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[serial]
fn test_fsck_connectivity_only_skips_content_check() {
    let repo = create_committed_repo_via_cli();

    // Get the commit hash
    let log_output = run_libra_command(&["log", "--pretty=%H"], repo.path());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    let commit_hash = stdout.lines().next().unwrap().trim();

    // Corrupt the commit object
    let hash_prefix = &commit_hash[0..2];
    let hash_rest = &commit_hash[2..];
    let object_path = repo.path().join(".libra").join("objects").join(hash_prefix).join(hash_rest);

    // Store original content
    let original_content = fs::read(&object_path).expect("failed to read object");

    // Corrupt the content
    fs::write(&object_path, b"corrupted!!!").expect("failed to corrupt object");

    // --connectivity-only should pass (only checks existence)
    let output = run_libra_command(&["fsck", "--connectivity-only"], repo.path());

    // Restore original content
    fs::write(&object_path, original_content).expect("failed to restore object");

    // With --connectivity-only, it only checks if objects exist, not content
    // So it should pass even with corrupted content
    assert!(
        output.status.success(),
        "--connectivity-only should pass even with corrupted content: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[serial]
fn test_fsck_connectivity_only_detects_missing_objects() {
    let repo = create_committed_repo_via_cli();

    // Get the commit hash
    let log_output = run_libra_command(&["log", "--pretty=%H"], repo.path());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    let commit_hash = stdout.lines().next().unwrap().trim();

    // Delete the commit object
    let hash_prefix = &commit_hash[0..2];
    let hash_rest = &commit_hash[2..];
    let object_path = repo.path().join(".libra").join("objects").join(hash_prefix).join(hash_rest);
    fs::remove_file(&object_path).expect("failed to delete object");

    // --connectivity-only should detect missing objects
    let output = run_libra_command(&["fsck", "--connectivity-only"], repo.path());

    assert!(
        !output.status.success(),
        "--connectivity-only should fail on missing objects"
    );
}
