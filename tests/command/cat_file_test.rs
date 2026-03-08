//! Tests for the `cat-file` command, verifying object type, size, content display,
//! existence checks, and AI object inspection.

use std::process::Command;

use serial_test::serial;

/// Initialize a temporary repository using CLI.
fn init_temp_repo() -> tempfile::TempDir {
    let temp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
    let temp_path = temp_dir.path();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .arg("init")
        .output()
        .expect("Failed to execute libra binary");

    if !output.status.success() {
        panic!(
            "Failed to initialize libra repository: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    temp_dir
}

/// Configure user identity for commits using CLI.
fn configure_user_identity(temp_path: &std::path::Path) {
    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["config", "user.name", "Test User"])
        .output()
        .expect("Failed to configure user.name");
    assert!(output.status.success(), "Failed to configure user.name");

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["config", "user.email", "test@example.com"])
        .output()
        .expect("Failed to configure user.email");
    assert!(output.status.success(), "Failed to configure user.email");
}

/// Create a commit with a file.
fn create_commit(temp_path: &std::path::Path, filename: &str, content: &str, message: &str) {
    std::fs::write(temp_path.join(filename), content).expect("Failed to create file");

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["add", filename])
        .output()
        .expect("Failed to add file");
    assert!(
        output.status.success(),
        "Failed to add file: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["commit", "-m", message, "--no-verify"])
        .output()
        .expect("Failed to commit");
    assert!(
        output.status.success(),
        "Failed to commit: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Test `cat-file -t` prints the object type for a commit.
#[tokio::test]
#[serial]
async fn test_cat_file_type_commit() {
    let temp_dir = init_temp_repo();
    let temp_path = temp_dir.path();

    configure_user_identity(temp_path);
    create_commit(temp_path, "hello.txt", "hello world\n", "first commit");

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["cat-file", "-t", "HEAD"])
        .output()
        .expect("Failed to execute cat-file");

    assert!(
        output.status.success(),
        "cat-file -t failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.trim(),
        "commit",
        "Expected type 'commit', got '{}'",
        stdout.trim()
    );
}

/// Test `cat-file -s` prints the object size for a commit.
#[tokio::test]
#[serial]
async fn test_cat_file_size_commit() {
    let temp_dir = init_temp_repo();
    let temp_path = temp_dir.path();

    configure_user_identity(temp_path);
    create_commit(temp_path, "hello.txt", "hello world\n", "first commit");

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["cat-file", "-s", "HEAD"])
        .output()
        .expect("Failed to execute cat-file");

    assert!(
        output.status.success(),
        "cat-file -s failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let size: usize = stdout.trim().parse().expect("Expected a numeric size");
    assert!(size > 0, "Commit object size should be > 0, got {}", size);
}

/// Test `cat-file -p` pretty-prints a commit object.
#[tokio::test]
#[serial]
async fn test_cat_file_pretty_commit() {
    let temp_dir = init_temp_repo();
    let temp_path = temp_dir.path();

    configure_user_identity(temp_path);
    create_commit(temp_path, "hello.txt", "hello world\n", "first commit");

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["cat-file", "-p", "HEAD"])
        .output()
        .expect("Failed to execute cat-file");

    assert!(
        output.status.success(),
        "cat-file -p failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("tree "),
        "Commit pretty-print should contain 'tree': {}",
        stdout
    );
    assert!(
        stdout.contains("author "),
        "Commit pretty-print should contain 'author': {}",
        stdout
    );
    assert!(
        stdout.contains("committer "),
        "Commit pretty-print should contain 'committer': {}",
        stdout
    );
    assert!(
        stdout.contains("first commit"),
        "Commit pretty-print should contain message: {}",
        stdout
    );
}

/// Test `cat-file -p` pretty-prints a tree object given a commit's tree hash.
#[tokio::test]
#[serial]
async fn test_cat_file_pretty_tree() {
    let temp_dir = init_temp_repo();
    let temp_path = temp_dir.path();

    configure_user_identity(temp_path);
    create_commit(temp_path, "file.txt", "content\n", "add file");

    // Get the tree hash from the commit
    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["cat-file", "-p", "HEAD"])
        .output()
        .expect("Failed to execute cat-file");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let tree_hash = stdout
        .lines()
        .find(|l| l.starts_with("tree "))
        .expect("should have tree line")
        .strip_prefix("tree ")
        .unwrap()
        .trim();

    // Now cat-file -p the tree
    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["cat-file", "-p", tree_hash])
        .output()
        .expect("Failed to execute cat-file on tree");
    assert!(
        output.status.success(),
        "cat-file -p tree failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("blob"),
        "Tree pretty-print should contain 'blob': {}",
        stdout
    );
    assert!(
        stdout.contains("file.txt"),
        "Tree pretty-print should contain 'file.txt': {}",
        stdout
    );

    // cat-file -t the tree should return "tree"
    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["cat-file", "-t", tree_hash])
        .output()
        .expect("Failed to execute cat-file -t on tree");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "tree");
}

/// Test `cat-file -p` pretty-prints a blob object.
#[tokio::test]
#[serial]
async fn test_cat_file_pretty_blob() {
    let temp_dir = init_temp_repo();
    let temp_path = temp_dir.path();

    configure_user_identity(temp_path);
    create_commit(temp_path, "readme.txt", "Hello, Libra!\n", "init readme");

    // Get tree hash, then blob hash from tree
    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["cat-file", "-p", "HEAD"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let tree_hash = stdout
        .lines()
        .find(|l| l.starts_with("tree "))
        .unwrap()
        .strip_prefix("tree ")
        .unwrap()
        .trim();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["cat-file", "-p", tree_hash])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    // tree line format: "100644 blob <hash>\t<name>"
    let blob_line = stdout
        .lines()
        .find(|l| l.contains("readme.txt"))
        .expect("should find readme.txt in tree");
    let blob_hash = blob_line
        .split_whitespace()
        .nth(2)
        .unwrap()
        // remove the tab and filename suffix: the hash may be followed by \t
        .split('\t')
        .next()
        .unwrap();

    // cat-file -p the blob
    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["cat-file", "-p", blob_hash])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "cat-file -p blob failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout, "Hello, Libra!\n", "Blob content should match");

    // cat-file -t the blob should return "blob"
    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["cat-file", "-t", blob_hash])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "blob");

    // cat-file -s the blob should be 14 bytes ("Hello, Libra!\n" = 14)
    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["cat-file", "-s", blob_hash])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "14");
}

/// Test `cat-file` panic handling for corrupted/invalid objects.
#[tokio::test]
#[serial]
async fn test_cat_file_panic_handling() {
    let temp_dir = init_temp_repo();
    let temp_path = temp_dir.path();

    // Test that the command reports an error (exit 128) rather than panicking
    // when accessing a non-existent object in a valid repository.
    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["cat-file", "-p", "0000000000000000000000000000000000000000"])
        .output()
        .expect("Failed to execute cat-file");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("fatal:"));
}

/// Test `cat-file -e` exits 0 for existing objects and non-zero for missing objects.
#[tokio::test]
#[serial]
async fn test_cat_file_exist_check() {
    let temp_dir = init_temp_repo();
    let temp_path = temp_dir.path();

    configure_user_identity(temp_path);
    create_commit(temp_path, "f.txt", "data", "commit");

    // HEAD exists
    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["cat-file", "-e", "HEAD"])
        .output()
        .expect("Failed to execute cat-file -e");
    assert!(
        output.status.success(),
        "cat-file -e HEAD should succeed for existing object"
    );

    // Non-existent hash
    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["cat-file", "-e", "0000000000000000000000000000000000000000"])
        .output()
        .expect("Failed to execute cat-file -e");
    assert!(
        !output.status.success(),
        "cat-file -e should fail for non-existent object"
    );
}

/// Test that mutually exclusive flags are enforced.
#[tokio::test]
#[serial]
async fn test_cat_file_mutual_exclusion() {
    let temp_dir = init_temp_repo();
    let temp_path = temp_dir.path();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["cat-file", "-t", "-s", "HEAD"])
        .output()
        .expect("Failed to execute cat-file");

    assert!(
        !output.status.success(),
        "cat-file -t -s should fail (mutual exclusion)"
    );
}

/// Test `cat-file -p` with multiple files in a tree.
#[tokio::test]
#[serial]
async fn test_cat_file_tree_multiple_files() {
    let temp_dir = init_temp_repo();
    let temp_path = temp_dir.path();

    configure_user_identity(temp_path);

    // Create multiple files
    std::fs::write(temp_path.join("a.txt"), "aaa\n").unwrap();
    std::fs::write(temp_path.join("b.txt"), "bbb\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["add", "."])
        .output()
        .unwrap();
    assert!(output.status.success());

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["commit", "-m", "two files", "--no-verify"])
        .output()
        .unwrap();
    assert!(output.status.success());

    // Get tree hash
    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["cat-file", "-p", "HEAD"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let tree_hash = stdout
        .lines()
        .find(|l| l.starts_with("tree "))
        .unwrap()
        .strip_prefix("tree ")
        .unwrap()
        .trim();

    // Pretty-print tree
    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["cat-file", "-p", tree_hash])
        .output()
        .unwrap();
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("a.txt"), "Should list a.txt: {}", stdout);
    assert!(stdout.contains("b.txt"), "Should list b.txt: {}", stdout);
}

/// Test `cat-file` with a non-existent reference.
#[tokio::test]
#[serial]
async fn test_cat_file_nonexistent_ref() {
    let temp_dir = init_temp_repo();
    let temp_path = temp_dir.path();

    configure_user_identity(temp_path);
    create_commit(temp_path, "f.txt", "data", "commit");

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["cat-file", "-t", "nonexistent-branch"])
        .output()
        .expect("Failed to execute cat-file");

    assert!(
        !output.status.success(),
        "cat-file should fail for non-existent ref"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// AI object tests
// ═══════════════════════════════════════════════════════════════════════

/// Test `cat-file --ai-list-types` on a fresh repo (no AI objects yet).
#[tokio::test]
#[serial]
async fn test_cat_file_ai_list_types_empty() {
    let temp_dir = init_temp_repo();
    let temp_path = temp_dir.path();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["cat-file", "--ai-list-types"])
        .output()
        .expect("Failed to execute cat-file --ai-list-types");

    assert!(
        output.status.success(),
        "cat-file --ai-list-types should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Fresh repo has no AI objects, output should be empty or minimal
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Since there are no AI objects, none of the types should appear with counts
    assert!(
        !stdout.contains("(0 objects)"),
        "Should not show types with zero objects"
    );
}

/// Test `cat-file --ai-list <type>` on a fresh repo.
#[tokio::test]
#[serial]
async fn test_cat_file_ai_list_empty_type() {
    let temp_dir = init_temp_repo();
    let temp_path = temp_dir.path();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["cat-file", "--ai-list", "intent"])
        .output()
        .expect("Failed to execute cat-file --ai-list");

    assert!(
        output.status.success(),
        "cat-file --ai-list intent should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("No intent objects found"),
        "Should report no objects: {}",
        stdout
    );
}

/// Test `cat-file --ai-list <invalid_type>` fails.
#[tokio::test]
#[serial]
async fn test_cat_file_ai_list_invalid_type() {
    let temp_dir = init_temp_repo();
    let temp_path = temp_dir.path();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["cat-file", "--ai-list", "foobar"])
        .output()
        .expect("Failed to execute cat-file --ai-list");

    assert!(
        !output.status.success(),
        "cat-file --ai-list foobar should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown AI object type"),
        "Should report unknown type: {}",
        stderr
    );
}

/// Test `cat-file --ai <uuid>` with a non-existent UUID.
#[tokio::test]
#[serial]
async fn test_cat_file_ai_nonexistent_uuid() {
    let temp_dir = init_temp_repo();
    let temp_path = temp_dir.path();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["cat-file", "--ai", "00000000-0000-0000-0000-000000000000"])
        .output()
        .expect("Failed to execute cat-file --ai");

    assert!(
        !output.status.success(),
        "cat-file --ai with non-existent UUID should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("AI object not found"),
        "Should report not found: {}",
        stderr
    );
}

/// Test `cat-file --ai-type <uuid>` with a non-existent UUID.
#[tokio::test]
#[serial]
async fn test_cat_file_ai_type_nonexistent() {
    let temp_dir = init_temp_repo();
    let temp_path = temp_dir.path();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args([
            "cat-file",
            "--ai-type",
            "00000000-0000-0000-0000-000000000000",
        ])
        .output()
        .expect("Failed to execute cat-file --ai-type");

    assert!(
        !output.status.success(),
        "cat-file --ai-type with non-existent UUID should fail"
    );
}

/// Test that AI flags and Git flags are mutually exclusive.
#[tokio::test]
#[serial]
async fn test_cat_file_ai_git_mutual_exclusion() {
    let temp_dir = init_temp_repo();
    let temp_path = temp_dir.path();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["cat-file", "--ai-list-types", "-t", "HEAD"])
        .output()
        .expect("Failed to execute cat-file");

    assert!(
        !output.status.success(),
        "AI and Git flags should be mutually exclusive"
    );
}

/// Running `cat-file` outside a repository should return exit code 128.
#[test]
#[serial]
fn test_cat_file_cli_outside_repository_returns_fatal_128() {
    let temp = tempfile::tempdir().unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp.path())
        .args(["cat-file", "-t", "HEAD"])
        .output()
        .expect("Failed to execute cat-file");

    assert_eq!(output.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("fatal: not a libra repository"),
        "unexpected stderr: {stderr}"
    );
}
