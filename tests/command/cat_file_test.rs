//! Tests for the `cat-file` command, verifying object type, size, content
//! display, existence checks, AI object inspection, and structured-error
//! envelopes.
//!
//! **Layer:** L1 — deterministic, no external dependencies.
//!
//! Fixture conventions: each test uses `init_temp_repo()` to spawn a
//! fresh `libra init` repo in a tempdir, optionally calls
//! `configure_user_identity()` and `create_commit()` to lay down a known
//! object graph, and runs `libra cat-file ...` through `Command`. The
//! tests cross-reference object hashes by parsing the human-readable
//! output (`tree <hash>`, tree entries `mode blob <hash>\t<name>`); these
//! parsers must therefore stay in sync with the cat-file pretty-printer.

use std::{
    io::{Read, Write},
    process::Command,
};

use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};

use super::{loose_object_path, parse_cli_error_stderr, parse_json_stdout};

/// Spawn `libra init` in a fresh tempdir and return the `TempDir` (kept
/// alive by the caller for RAII cleanup).
fn init_temp_repo() -> tempfile::TempDir {
    let temp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
    let temp_path = temp_dir.path();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["init"])
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

/// Configure `user.name` / `user.email` through the CLI so subsequent
/// commits can be authored. Required before `create_commit()`.
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

/// Write `content` to `filename`, stage it, and create a commit through
/// the CLI. Skips the pre-commit hook with `--no-verify` so the test does
/// not rely on hook availability.
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

/// Scenario: `cat-file -t HEAD` against a commit must print exactly
/// `commit` on stdout. Pins the canonical object-type vocabulary.
#[tokio::test]
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

/// Scenario: `cat-file -s HEAD` must emit a positive numeric size.
/// Smoke test for the size pathway; the exact bytes are commit-shape
/// dependent so only `> 0` is asserted.
#[tokio::test]
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

/// Scenario: `cat-file -t --json HEAD` must emit
/// `command="cat-file"`, `data.mode="type"`, `data.object="HEAD"` and
/// `data.object_type="commit"`. Schema pin for the type-mode envelope.
#[tokio::test]
async fn test_cat_file_type_json_output() {
    let temp_dir = init_temp_repo();
    let temp_path = temp_dir.path();

    configure_user_identity(temp_path);
    create_commit(temp_path, "hello.txt", "hello world\n", "first commit");

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["cat-file", "-t", "HEAD", "--json"])
        .output()
        .expect("Failed to execute cat-file");

    assert!(
        output.status.success(),
        "cat-file -t --json failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "cat-file");
    assert_eq!(json["data"]["mode"], "type");
    assert_eq!(json["data"]["object"], "HEAD");
    assert_eq!(json["data"]["object_type"], "commit");
}

/// Scenario: `cat-file -p HEAD` on a commit must include `tree `,
/// `author `, `committer `, and the commit message. Locks the
/// commit-pretty-printer's stable headers so other tests can grep for
/// them (e.g. tree-hash extraction in subsequent cases).
#[tokio::test]
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

/// Scenario: end-to-end commit → tree path. Extracts the tree hash from
/// the commit's pretty output, then verifies `cat-file -p <tree>` lists
/// the blob entry (`blob` + filename) and `cat-file -t <tree>` returns
/// `tree`. Pins both the tree-pretty format and tree type tagging.
#[tokio::test]
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

/// Scenario: end-to-end commit → tree → blob. Resolves the blob hash by
/// parsing the tree entry line, then asserts:
/// - `cat-file -p <blob>` echoes the original file content verbatim,
/// - `cat-file -t <blob>` returns `blob`,
/// - `cat-file -s <blob>` returns `14` (matching `"Hello, Libra!\n"`).
/// Pins type/size/content invariants for blob objects.
#[tokio::test]
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

/// Scenario: a syntactically-valid but unknown 40-zero hash must surface
/// a structured `LBR-CLI-003` error (exit 129, `fatal:` on stderr). The
/// command must NOT panic when an object is missing — regression guard
/// against unwrap-on-load bugs.
#[tokio::test]
async fn test_cat_file_panic_handling() {
    let temp_dir = init_temp_repo();
    let temp_path = temp_dir.path();

    // Test that the command reports a structured invalid-target error rather than panicking
    // when accessing a non-existent object in a valid repository.
    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["cat-file", "-p", "0000000000000000000000000000000000000000"])
        .output()
        .expect("Failed to execute cat-file");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(129));
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert!(stderr.contains("fatal:"));
}

#[tokio::test]
async fn test_cat_file_json_invalid_object_returns_cli_003() {
    let temp_dir = init_temp_repo();
    let temp_path = temp_dir.path();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args([
            "cat-file",
            "-p",
            "0000000000000000000000000000000000000000",
            "--json",
        ])
        .output()
        .expect("Failed to execute cat-file");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(129));
    let (_stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-003");
}

/// Scenario: `cat-file -e <object>` must be silent in both directions —
/// existing object → exit 0 with empty stderr; missing object → exit 1
/// with empty stderr. Pins Git-compatible status-only semantics so
/// scripts can `if libra cat-file -e $hash; then ...`.
#[tokio::test]
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
    assert!(
        output.stderr.is_empty(),
        "cat-file -e HEAD should not print stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Non-existent hash
    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["cat-file", "-e", "0000000000000000000000000000000000000000"])
        .output()
        .expect("Failed to execute cat-file -e");
    assert_eq!(output.status.code(), Some(1));
    assert!(
        output.stderr.is_empty(),
        "cat-file -e missing object should stay silent: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Scenario: `-t -s` together must be rejected — clap's mutual-exclusion
/// guards prevent ambiguous output. Confirms the CLI grammar.
#[tokio::test]
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

#[tokio::test]
async fn test_cat_file_ai_list_invalid_type_json_returns_cli_003() {
    let temp_dir = init_temp_repo();
    let temp_path = temp_dir.path();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["cat-file", "--ai-list", "foobar", "--json"])
        .output()
        .expect("Failed to execute cat-file");

    assert!(!output.status.success());
    let (_, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-003");
}

#[tokio::test]
async fn test_cat_file_json_pretty_print_io_read_failed_when_object_body_corrupted() {
    let temp_dir = init_temp_repo();
    let temp_path = temp_dir.path();

    configure_user_identity(temp_path);
    create_commit(temp_path, "hello.txt", "hello world\n", "first commit");

    let head_output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("Failed to execute rev-parse");
    assert!(
        head_output.status.success(),
        "rev-parse HEAD failed: {}",
        String::from_utf8_lossy(&head_output.stderr)
    );
    let head = String::from_utf8_lossy(&head_output.stdout)
        .trim()
        .to_string();

    let object_path = loose_object_path(temp_path, &head);
    let raw_data = std::fs::read(&object_path).expect("Failed to read commit object file");

    let mut decoder = ZlibDecoder::new(raw_data.as_slice());
    let mut decompressed = Vec::new();
    decoder
        .read_to_end(&mut decompressed)
        .expect("Failed to decode commit object payload");
    let header_end = decompressed
        .iter()
        .position(|&b| b == b'\0')
        .expect("Malformed object payload");
    let mut corrupted = Vec::with_capacity(header_end + 1 + 5);
    corrupted.extend_from_slice(&decompressed[..=header_end]);
    corrupted.extend_from_slice(b"\xff\xff");

    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder
        .write_all(&corrupted)
        .expect("Failed to re-encode corrupted commit object");
    let encoded = encoder
        .finish()
        .expect("Failed to finish corrupted commit object encoding");
    std::fs::write(&object_path, encoded).expect("Failed to write corrupted commit object");

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["cat-file", "-p", &head, "--json"])
        .output()
        .expect("Failed to execute cat-file");

    assert!(!output.status.success());
    let (_stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-REPO-002");
}

/// Test `cat-file --ai <uuid>` with a non-existent UUID.
#[tokio::test]
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
fn test_cat_file_cli_outside_repository_returns_fatal_128() {
    let temp = tempfile::tempdir().unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp.path())
        .args(["cat-file", "-t", "HEAD"])
        .output()
        .expect("Failed to execute cat-file");

    assert_eq!(output.status.code(), Some(128));
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-REPO-001");
    assert!(
        stderr.contains("fatal: not a libra repository"),
        "unexpected stderr: {stderr}"
    );
}

// ════════════════════════════════════════════════════════════════════════
//  Batch 0 — `--batch-check` streaming engine, -Z, modifier/contract guards
// ════════════════════════════════════════════════════════════════════════

/// Spawn `libra <args>` in `temp_path`, feed `stdin_data`, and collect output.
fn run_cat_file_with_stdin(
    temp_path: &std::path::Path,
    args: &[&str],
    stdin_data: &[u8],
) -> std::process::Output {
    use std::process::Stdio;
    let mut child = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn libra");
    child
        .stdin
        .take()
        .expect("child stdin")
        .write_all(stdin_data)
        .expect("write stdin");
    child.wait_with_output().expect("wait for libra")
}

/// Resolve `HEAD` to its full commit hash via `rev-parse`.
fn head_commit_hash(temp_path: &std::path::Path) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("rev-parse HEAD");
    assert!(out.status.success(), "rev-parse failed");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Resolve the first blob hash reachable from HEAD's tree.
fn head_first_blob_hash(temp_path: &std::path::Path) -> String {
    let commit = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["cat-file", "-p", "HEAD"])
        .output()
        .expect("cat-file -p HEAD");
    let commit_stdout = String::from_utf8_lossy(&commit.stdout);
    let tree = commit_stdout
        .lines()
        .find(|l| l.starts_with("tree "))
        .and_then(|l| l.split_whitespace().nth(1))
        .expect("tree line")
        .to_string();
    let tree_out = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["cat-file", "-p", &tree])
        .output()
        .expect("cat-file -p tree");
    let tree_stdout = String::from_utf8_lossy(&tree_out.stdout);
    tree_stdout
        .lines()
        .find(|l| l.contains(" blob "))
        .and_then(|l| l.split_whitespace().nth(2))
        .expect("blob entry")
        .to_string()
}

fn batch_repo() -> tempfile::TempDir {
    let repo = init_temp_repo();
    configure_user_identity(repo.path());
    create_commit(repo.path(), "hello.txt", "hello world\n", "first commit");
    repo
}

/// A full blob OID resolves to `<oid> blob <size>`.
#[tokio::test]
async fn batch_check_resolves_valid_oid() {
    let repo = batch_repo();
    let blob = head_first_blob_hash(repo.path());
    let out = run_cat_file_with_stdin(
        repo.path(),
        &["cat-file", "--batch-check"],
        format!("{blob}\n").as_bytes(),
    );
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout.lines().next().expect("one output line");
    let parts: Vec<&str> = line.split_whitespace().collect();
    assert_eq!(parts[0], blob, "objectname echoes the OID");
    assert_eq!(parts[1], "blob", "blob type");
    assert!(parts[2].parse::<u64>().is_ok(), "size is numeric: {line}");
}

/// A full commit hash resolves to `<hash> commit <size>`.
#[tokio::test]
async fn batch_check_resolves_commit() {
    let repo = batch_repo();
    let commit = head_commit_hash(repo.path());
    let out = run_cat_file_with_stdin(
        repo.path(),
        &["cat-file", "--batch-check"],
        format!("{commit}\n").as_bytes(),
    );
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let parts: Vec<&str> = stdout.split_whitespace().collect();
    assert_eq!(parts[0], commit);
    assert_eq!(parts[1], "commit");
}

/// `HEAD` resolves to the current commit metadata.
#[tokio::test]
async fn batch_check_resolves_head_ref() {
    let repo = batch_repo();
    let commit = head_commit_hash(repo.path());
    let out = run_cat_file_with_stdin(repo.path(), &["cat-file", "--batch-check"], b"HEAD\n");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(&commit),
        "HEAD resolves to commit hash: {stdout}"
    );
    assert!(stdout.contains(" commit "), "type is commit: {stdout}");
}

/// An unresolvable token prints `<input> missing` and the process exits 0.
#[tokio::test]
async fn batch_check_missing_object() {
    let repo = batch_repo();
    let out = run_cat_file_with_stdin(
        repo.path(),
        &["cat-file", "--batch-check"],
        b"INVALIDOBJECT\n",
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "missing must not change exit code"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout.trim_end(), "INVALIDOBJECT missing");
}

/// A short SHA that matches ≥2 objects prints `<input> ambiguous` (exit 0).
#[tokio::test]
async fn batch_check_ambiguous_short_sha() {
    let repo = init_temp_repo();
    configure_user_identity(repo.path());
    // Lay down enough commits that two object hashes share a 1-hex prefix.
    let mut hashes = Vec::new();
    for i in 0..20 {
        create_commit(
            repo.path(),
            &format!("f{i}.txt"),
            &format!("content number {i}\n"),
            &format!("commit {i}"),
        );
        hashes.push(head_commit_hash(repo.path()));
    }
    // Find the shortest prefix shared by ≥2 collected hashes.
    let mut ambiguous_prefix: Option<String> = None;
    'outer: for len in 1..=8 {
        for a in 0..hashes.len() {
            let pfx = &hashes[a][..len];
            let count = hashes.iter().filter(|h| h.starts_with(pfx)).count();
            if count >= 2 {
                ambiguous_prefix = Some(pfx.to_string());
                break 'outer;
            }
        }
    }
    let prefix = ambiguous_prefix.expect("two commit hashes should share a short prefix");
    let out = run_cat_file_with_stdin(
        repo.path(),
        &["cat-file", "--batch-check"],
        format!("{prefix}\n").as_bytes(),
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "ambiguous must not change exit code"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        stdout.trim_end(),
        format!("{prefix} ambiguous"),
        "stdout: {stdout}"
    );
}

/// A `\r\n`-terminated record is trimmed and resolves normally.
#[tokio::test]
async fn batch_check_trims_crlf() {
    let repo = batch_repo();
    let out = run_cat_file_with_stdin(repo.path(), &["cat-file", "--batch-check"], b"HEAD\r\n");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(" commit "),
        "CRLF trimmed, resolves commit: {stdout}"
    );
    assert!(
        !stdout.contains("missing"),
        "must not be treated as missing: {stdout}"
    );
}

/// Under `-Z`, only NUL separates records and stdout records are NUL-terminated;
/// a bare `\n` does not split.
#[tokio::test]
async fn batch_check_z_uses_nul_separator() {
    let repo = batch_repo();
    // Two NUL-separated HEAD tokens -> two commit records, each NUL-terminated.
    let out = run_cat_file_with_stdin(
        repo.path(),
        &["cat-file", "--batch-check", "-Z"],
        b"HEAD\0HEAD\0",
    );
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let nul_count = out.stdout.iter().filter(|&&b| b == 0).count();
    assert_eq!(nul_count, 2, "two NUL-terminated records");
    assert!(
        !out.stdout.contains(&b'\n'),
        "no LF used as separator under -Z"
    );

    // A bare LF is NOT a separator under -Z: the whole thing is one token.
    let out = run_cat_file_with_stdin(
        repo.path(),
        &["cat-file", "--batch-check", "-Z"],
        b"HEAD\nHEAD",
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("missing"),
        "LF-joined token is one unresolved token: {stdout}"
    );
}

/// A custom `--batch-check=<format>` renders the requested atoms.
#[tokio::test]
async fn batch_check_custom_format() {
    let repo = batch_repo();
    let commit = head_commit_hash(repo.path());
    let out = run_cat_file_with_stdin(
        repo.path(),
        &["cat-file", "--batch-check=%(objectname) - %(objecttype)"],
        b"HEAD\n",
    );
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout.trim_end(), format!("{commit} - commit"));
}

/// EOF on empty stdin exits 0 with no output.
#[tokio::test]
async fn batch_check_eof_exits_zero() {
    let repo = batch_repo();
    let out = run_cat_file_with_stdin(repo.path(), &["cat-file", "--batch-check"], b"");
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty(), "no output for empty stdin");
}

/// An over-long (>4KiB) input record is rejected with LBR-CLI-003 (129).
#[tokio::test]
async fn batch_check_oversize_line_rejected() {
    let repo = batch_repo();
    let oversize = vec![b'a'; 5000]; // no terminator
    let out = run_cat_file_with_stdin(repo.path(), &["cat-file", "--batch-check"], &oversize);
    assert_eq!(
        out.status.code(),
        Some(129),
        "oversize line is a usage error"
    );
    let (_human, report) = parse_cli_error_stderr(&out.stderr);
    assert_eq!(report.error_code, "LBR-CLI-003");
}

/// A closed downstream pipe (BrokenPipe) terminates the stream quietly (exit 0).
#[tokio::test]
async fn batch_check_brokenpipe_exits_zero() {
    use std::process::Stdio;
    let repo = batch_repo();
    let mut child = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(repo.path())
        .args(["cat-file", "--batch-check"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn cat-file");
    let mut stdin = child.stdin.take().expect("stdin");
    // Feed a long stream of unresolved tokens from a thread; ignore write
    // errors (the child closes its stdin when it exits).
    let writer = std::thread::spawn(move || {
        for _ in 0..200_000 {
            if stdin.write_all(b"INVALIDOBJECT\n").is_err() {
                break;
            }
        }
    });
    // Read a little, then close the read end so the child's next write fails.
    let mut stdout = child.stdout.take().expect("stdout");
    let mut buf = [0u8; 16];
    let _ = stdout.read(&mut buf);
    drop(stdout);
    let status = child.wait().expect("wait");
    let _ = writer.join();
    assert_eq!(status.code(), Some(0), "BrokenPipe must exit 0");
}

/// `--batch-check -p` is a mode-group conflict. Libra surfaces clap parse
/// conflicts through its usage path (coarse exit 129), not clap's native 2.
#[tokio::test]
async fn batch_mode_conflicts_with_single_mode() {
    let repo = batch_repo();
    let out = run_cat_file_with_stdin(repo.path(), &["cat-file", "--batch-check", "-p"], b"");
    assert_eq!(
        out.status.code(),
        Some(129),
        "mode conflict is a usage error"
    );
}

/// An unknown format placeholder is rejected (LBR-CLI-002), even on empty stdin.
#[tokio::test]
async fn batch_check_unknown_placeholder() {
    let repo = batch_repo();
    let out = run_cat_file_with_stdin(repo.path(), &["cat-file", "--batch-check=%(bogus)"], b"");
    assert_eq!(out.status.code(), Some(129));
    let (_human, report) = parse_cli_error_stderr(&out.stderr);
    assert_eq!(report.error_code, "LBR-CLI-002");
}

/// `--textconv` is explicitly unsupported (LBR-UNSUPPORTED-001, 128).
#[tokio::test]
async fn textconv_flag_rejected_unsupported() {
    let repo = batch_repo();
    let out = run_cat_file_with_stdin(repo.path(), &["cat-file", "--textconv", "HEAD"], b"");
    assert_eq!(out.status.code(), Some(128));
    let (_human, report) = parse_cli_error_stderr(&out.stderr);
    assert_eq!(report.error_code, "LBR-UNSUPPORTED-001");
}

/// `--filters` is explicitly unsupported (LBR-UNSUPPORTED-001, 128).
#[tokio::test]
async fn filters_flag_rejected_unsupported() {
    let repo = batch_repo();
    let out = run_cat_file_with_stdin(repo.path(), &["cat-file", "--filters", "HEAD"], b"");
    assert_eq!(out.status.code(), Some(128));
    let (_human, report) = parse_cli_error_stderr(&out.stderr);
    assert_eq!(report.error_code, "LBR-UNSUPPORTED-001");
}

/// Batch modes do not support `--json`/`--machine` (LBR-CLI-002, 129).
#[tokio::test]
async fn batch_with_json_rejected() {
    let repo = batch_repo();
    let out = run_cat_file_with_stdin(repo.path(), &["--json", "cat-file", "--batch-check"], b"");
    assert_eq!(out.status.code(), Some(129));
    let (_human, report) = parse_cli_error_stderr(&out.stderr);
    assert_eq!(report.error_code, "LBR-CLI-002");
}

/// Batch modes read from stdin; a positional OBJECT is rejected (LBR-CLI-002).
#[tokio::test]
async fn batch_with_positional_object_rejected() {
    let repo = batch_repo();
    let out = run_cat_file_with_stdin(repo.path(), &["cat-file", "--batch-check", "HEAD"], b"");
    assert_eq!(out.status.code(), Some(129));
    let (_human, report) = parse_cli_error_stderr(&out.stderr);
    assert_eq!(report.error_code, "LBR-CLI-002");
}

/// A bare `-Z` modifier without a batch mode is rejected (LBR-CLI-002).
#[tokio::test]
async fn z_modifier_without_batch_rejected() {
    let repo = batch_repo();
    let out = run_cat_file_with_stdin(repo.path(), &["cat-file", "-Z"], b"");
    assert_eq!(out.status.code(), Some(129));
    let (_human, report) = parse_cli_error_stderr(&out.stderr);
    assert_eq!(report.error_code, "LBR-CLI-002");
}

// ════════════════════════════════════════════════════════════════════════
//  Batch 1 — `--batch` full content output + `--buffer`
// ════════════════════════════════════════════════════════════════════════

/// Stage and commit `bytes` (possibly non-UTF-8) under `filename`.
fn commit_bytes(temp_path: &std::path::Path, filename: &str, bytes: &[u8]) {
    std::fs::write(temp_path.join(filename), bytes).expect("write file");
    let add = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["add", filename])
        .output()
        .expect("add");
    assert!(
        add.status.success(),
        "add: {}",
        String::from_utf8_lossy(&add.stderr)
    );
    let commit = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["commit", "-m", "binary", "--no-verify"])
        .output()
        .expect("commit");
    assert!(
        commit.status.success(),
        "commit: {}",
        String::from_utf8_lossy(&commit.stderr)
    );
}

/// `--batch` emits `<oid> SP blob SP <size> LF <content> LF` byte-for-byte.
#[tokio::test]
async fn batch_output_format_exact() {
    let repo = batch_repo();
    let blob = head_first_blob_hash(repo.path());
    let out = run_cat_file_with_stdin(
        repo.path(),
        &["cat-file", "--batch"],
        format!("{blob}\n").as_bytes(),
    );
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let expected = format!("{blob} blob 12\nhello world\n\n");
    assert_eq!(out.stdout, expected.as_bytes(), "exact batch output bytes");
}

/// `--batch -Z` terminates both the metadata line and the content block with NUL.
#[tokio::test]
async fn batch_output_z_uses_nul_record_sep() {
    let repo = batch_repo();
    let blob = head_first_blob_hash(repo.path());
    let out = run_cat_file_with_stdin(
        repo.path(),
        &["cat-file", "--batch", "-Z"],
        format!("{blob}\0").as_bytes(),
    );
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mut expected = format!("{blob} blob 12").into_bytes();
    expected.push(0);
    expected.extend_from_slice(b"hello world\n");
    expected.push(0);
    assert_eq!(out.stdout, expected, "NUL-terminated records under -Z");
}

/// Binary (non-UTF-8) blob content is written through verbatim.
#[tokio::test]
async fn batch_binary_blob_passthrough() {
    let repo = init_temp_repo();
    configure_user_identity(repo.path());
    let payload: &[u8] = &[0xFF, 0xFE, 0x00, 0x01, 0x80, 0x7F, 0x0A, 0xC0];
    commit_bytes(repo.path(), "bin.dat", payload);
    let blob = head_first_blob_hash(repo.path());

    let out = run_cat_file_with_stdin(
        repo.path(),
        &["cat-file", "--batch"],
        format!("{blob}\n").as_bytes(),
    );
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // stdout = "<oid> blob <len>\n" + <payload> + "\n"
    let nl = out
        .stdout
        .iter()
        .position(|&b| b == b'\n')
        .expect("metadata terminator");
    let content = &out.stdout[nl + 1..out.stdout.len() - 1];
    assert_eq!(content, payload, "binary content preserved byte-for-byte");
}

/// Multiple input tokens produce records in input order.
#[tokio::test]
async fn batch_multiple_objects_in_order() {
    let repo = batch_repo();
    let blob = head_first_blob_hash(repo.path());
    let commit = head_commit_hash(repo.path());
    let out = run_cat_file_with_stdin(
        repo.path(),
        &["cat-file", "--batch-check"],
        format!("{blob}\n{commit}\n").as_bytes(),
    );
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert!(
        lines[0].starts_with(&blob),
        "first record is the blob: {stdout}"
    );
    assert!(lines[0].contains(" blob "));
    assert!(
        lines[1].starts_with(&commit),
        "second record is the commit: {stdout}"
    );
    assert!(lines[1].contains(" commit "));
}

/// A missing object in `--batch` prints only `<object> SP missing LF` with no
/// trailing content block.
#[tokio::test]
async fn batch_missing_no_trailing_content() {
    let repo = batch_repo();
    let out = run_cat_file_with_stdin(repo.path(), &["cat-file", "--batch"], b"NOPE\n");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        out.stdout, b"NOPE missing\n",
        "missing has no content block"
    );
}

/// An empty blob (size 0) emits the metadata line then a 0-byte content block.
#[tokio::test]
async fn batch_empty_blob_object() {
    let repo = init_temp_repo();
    configure_user_identity(repo.path());
    commit_bytes(repo.path(), "empty.txt", b"");
    let blob = head_first_blob_hash(repo.path());
    let out = run_cat_file_with_stdin(
        repo.path(),
        &["cat-file", "--batch"],
        format!("{blob}\n").as_bytes(),
    );
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let expected = format!("{blob} blob 0\n\n");
    assert_eq!(
        out.stdout,
        expected.as_bytes(),
        "empty blob: meta + empty content"
    );
}

/// A corrupt (undecodable) object surfaces an I/O read failure (LBR-IO-001, 128).
#[tokio::test]
async fn batch_read_failure_maps_io_error() {
    let repo = batch_repo();
    let blob = head_first_blob_hash(repo.path());
    // Overwrite the loose object with bytes that are not a valid zlib stream.
    let object_path = loose_object_path(repo.path(), &blob);
    std::fs::write(&object_path, b"not a valid zlib object stream at all").expect("corrupt object");

    let out = run_cat_file_with_stdin(
        repo.path(),
        &["cat-file", "--batch"],
        format!("{blob}\n").as_bytes(),
    );
    assert_eq!(out.status.code(), Some(128), "corrupt object read is fatal");
    let (_human, report) = parse_cli_error_stderr(&out.stderr);
    assert_eq!(report.error_code, "LBR-IO-001");
}

/// A bare `--buffer` without a batch mode is rejected (LBR-CLI-002, 129).
#[tokio::test]
async fn buffer_without_batch_rejected() {
    let repo = batch_repo();
    let out = run_cat_file_with_stdin(repo.path(), &["cat-file", "--buffer"], b"");
    assert_eq!(out.status.code(), Some(129));
    let (_human, report) = parse_cli_error_stderr(&out.stderr);
    assert_eq!(report.error_code, "LBR-CLI-002");
}

/// Drive a `--batch[ --buffer]` child with stdin held open after one token, and
/// report whether the first record reaches stdout *before* EOF. Without
/// `--buffer` the per-object flush makes it visible; with `--buffer` it stays
/// buffered until EOF.
fn batch_first_record_visible_before_eof(extra: &[&str]) -> bool {
    use std::{process::Stdio, sync::mpsc, time::Duration};
    let repo = batch_repo();
    let mut argv = vec!["cat-file", "--batch"];
    argv.extend_from_slice(extra);
    let mut child = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(repo.path())
        .args(&argv)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn cat-file");
    let mut stdin = child.stdin.take().expect("stdin");
    stdin.write_all(b"HEAD\n").expect("write token");
    // Intentionally keep `stdin` open so the child does not see EOF yet.
    let mut stdout = child.stdout.take().expect("stdout");
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut buf = [0u8; 64];
        let n = stdout.read(&mut buf).unwrap_or(0);
        let _ = tx.send(n);
    });
    let visible = matches!(rx.recv_timeout(Duration::from_secs(3)), Ok(n) if n > 0);
    drop(stdin); // let the child reach EOF and exit
    let _ = child.wait();
    let _ = reader.join();
    visible
}

/// Without `--buffer`, each object's record is flushed immediately.
#[tokio::test]
async fn batch_no_buffer_flushes_per_object() {
    assert!(
        batch_first_record_visible_before_eof(&[]),
        "no --buffer should flush the first record before EOF"
    );
}

/// With `--buffer`, output is coalesced and not flushed until EOF.
#[tokio::test]
async fn batch_buffer_flag_coalesces_writes() {
    assert!(
        !batch_first_record_visible_before_eof(&["--buffer"]),
        "--buffer should defer output until EOF"
    );
}
