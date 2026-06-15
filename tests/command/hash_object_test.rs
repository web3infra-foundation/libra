//! Tests for `hash-object`, covering Git-compatible blob hashing, stdin input,
//! object writes, and structured output.

use std::fs;

use super::{
    assert_cli_success, init_repo_via_cli, parse_cli_error_stderr, parse_json_stdout,
    run_libra_command, run_libra_command_with_stdin,
};

#[tokio::test]
async fn hash_object_file_matches_git_blob_hash() {
    let repo = tempfile::tempdir().expect("create temp repo");
    init_repo_via_cli(repo.path());
    fs::write(repo.path().join("hello.txt"), b"hello world\n").expect("write fixture");

    let output = run_libra_command(&["hash-object", "hello.txt"], repo.path());
    assert_cli_success(&output, "hash-object file should succeed");

    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "3b18e512dba79e4c8300dd08aeb37f8e728b8dad"
    );
    assert!(
        output.stderr.is_empty(),
        "stderr should stay clean: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn hash_object_stdin_matches_git_blob_hash() {
    let repo = tempfile::tempdir().expect("create temp repo");
    init_repo_via_cli(repo.path());

    let output = run_libra_command_with_stdin(&["hash-object", "--stdin"], repo.path(), "hello");
    assert_cli_success(&output, "hash-object --stdin should succeed");

    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "b6fc4c620b67d95f953a5c1c1230aaab5db5a1b0"
    );
}

#[tokio::test]
async fn hash_object_read_only_uses_sha256_repository_format() {
    let repo = tempfile::tempdir().expect("create temp repo");
    let init = run_libra_command(&["init", "--object-format", "sha256"], repo.path());
    assert_cli_success(&init, "failed to initialize sha256 repository");
    fs::write(repo.path().join("hello.txt"), b"hello world\n").expect("write fixture");

    let output = run_libra_command(&["hash-object", "hello.txt"], repo.path());
    assert_cli_success(
        &output,
        "read-only hash-object should use repository object format",
    );

    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "0bd69098bd9b9cc5934a610ab65da429b525361147faa7b5b922919e9a23143d"
    );
}

#[tokio::test]
async fn hash_object_file_works_outside_repository() {
    let dir = tempfile::tempdir().expect("create temp dir");
    fs::write(dir.path().join("hello.txt"), b"hello world\n").expect("write fixture");

    let output = run_libra_command(&["hash-object", "hello.txt"], dir.path());
    assert_cli_success(&output, "read-only hash-object should not require repo");

    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "3b18e512dba79e4c8300dd08aeb37f8e728b8dad"
    );
}

#[tokio::test]
async fn hash_object_stdin_works_outside_repository() {
    let dir = tempfile::tempdir().expect("create temp dir");

    let output = run_libra_command_with_stdin(&["hash-object", "--stdin"], dir.path(), "hello");
    assert_cli_success(
        &output,
        "read-only hash-object --stdin should not require repo",
    );

    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "b6fc4c620b67d95f953a5c1c1230aaab5db5a1b0"
    );
}

#[tokio::test]
async fn hash_object_no_filters_matches_default_hash() {
    let repo = tempfile::tempdir().expect("create temp repo");
    init_repo_via_cli(repo.path());
    fs::write(repo.path().join("hello.txt"), b"hello").expect("write fixture");

    let output = run_libra_command(&["hash-object", "--no-filters", "hello.txt"], repo.path());
    assert_cli_success(&output, "hash-object --no-filters should succeed");

    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "b6fc4c620b67d95f953a5c1c1230aaab5db5a1b0"
    );
}

#[tokio::test]
async fn hash_object_stdin_path_matches_raw_hash_and_reports_source_label() {
    let repo = tempfile::tempdir().expect("create temp repo");
    init_repo_via_cli(repo.path());

    let output = run_libra_command_with_stdin(
        &[
            "hash-object",
            "--stdin",
            "--path=virtual/input.txt",
            "--json",
        ],
        repo.path(),
        "hello",
    );
    assert_cli_success(&output, "hash-object --stdin --path should succeed");

    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["objects"][0]["source"], "virtual/input.txt");
    assert_eq!(
        json["data"]["objects"][0]["oid"],
        "b6fc4c620b67d95f953a5c1c1230aaab5db5a1b0"
    );
}

#[tokio::test]
async fn hash_object_path_conflicts_with_no_filters() {
    let repo = tempfile::tempdir().expect("create temp repo");
    init_repo_via_cli(repo.path());

    let output = run_libra_command_with_stdin(
        &[
            "hash-object",
            "--stdin",
            "--path=virtual/input.txt",
            "--no-filters",
        ],
        repo.path(),
        "hello",
    );

    assert_eq!(output.status.code(), Some(129));
    let (human, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert!(
        human.contains("cannot be used with"),
        "expected clap conflict message, got: {human}"
    );
}

#[tokio::test]
async fn hash_object_write_still_requires_repository() {
    let dir = tempfile::tempdir().expect("create temp dir");
    fs::write(dir.path().join("persist.txt"), b"persist me").expect("write fixture");

    let output = run_libra_command(&["hash-object", "-w", "persist.txt"], dir.path());
    assert!(
        !output.status.success(),
        "hash-object -w outside repo should fail"
    );

    let (human, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-REPO-001");
    assert!(
        human.contains("not a libra repository"),
        "error should explain repo requirement: {human}"
    );
}

#[tokio::test]
async fn hash_object_write_persists_blob_for_cat_file() {
    let repo = tempfile::tempdir().expect("create temp repo");
    init_repo_via_cli(repo.path());
    fs::write(repo.path().join("persist.txt"), b"persist me").expect("write fixture");

    let output = run_libra_command(&["hash-object", "-w", "persist.txt"], repo.path());
    assert_cli_success(&output, "hash-object -w should succeed");
    let oid = String::from_utf8_lossy(&output.stdout).trim().to_string();

    let type_output = run_libra_command(&["cat-file", "-t", &oid], repo.path());
    assert_cli_success(&type_output, "cat-file should find written blob");
    assert_eq!(String::from_utf8_lossy(&type_output.stdout).trim(), "blob");

    let pretty_output = run_libra_command(&["cat-file", "-p", &oid], repo.path());
    assert_cli_success(&pretty_output, "cat-file -p should print written blob");
    assert_eq!(String::from_utf8_lossy(&pretty_output.stdout), "persist me");
}

#[tokio::test]
async fn hash_object_batch_prints_successes_before_later_failure() {
    let repo = tempfile::tempdir().expect("create temp repo");
    init_repo_via_cli(repo.path());
    fs::write(repo.path().join("first.txt"), b"first").expect("write fixture");

    let output = run_libra_command(&["hash-object", "first.txt", "missing.txt"], repo.path());

    assert!(
        !output.status.success(),
        "missing trailing input should fail the command"
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "fe4f02ad058b43f6ed467fdf65b935107529564b"
    );

    let (human, report) = parse_cli_error_stderr(&output.stderr);
    assert!(
        human.contains("failed to read 'missing.txt'"),
        "human stderr should explain unreadable input: {human}"
    );
    assert_eq!(report.error_code, "LBR-IO-001");
}

#[tokio::test]
async fn hash_object_json_reports_source_size_and_write_mode() {
    let repo = tempfile::tempdir().expect("create temp repo");
    init_repo_via_cli(repo.path());

    let output =
        run_libra_command_with_stdin(&["hash-object", "--stdin", "--json"], repo.path(), "hello");
    assert_cli_success(&output, "hash-object --json should succeed");

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "hash-object");
    assert_eq!(json["data"]["object_type"], "blob");
    assert_eq!(json["data"]["write"], false);
    assert_eq!(json["data"]["objects"][0]["source"], "-");
    assert_eq!(json["data"]["objects"][0]["size"], 5);
    assert_eq!(
        json["data"]["objects"][0]["oid"],
        "b6fc4c620b67d95f953a5c1c1230aaab5db5a1b0"
    );
    assert_eq!(json["data"]["objects"][0]["written"], false);
}

#[tokio::test]
async fn hash_object_rejects_unsupported_object_type() {
    let repo = tempfile::tempdir().expect("create temp repo");
    init_repo_via_cli(repo.path());
    fs::write(repo.path().join("hello.txt"), b"hello").expect("write fixture");

    let output = run_libra_command(&["hash-object", "-t", "tree", "hello.txt"], repo.path());
    assert!(
        !output.status.success(),
        "unsupported object type should fail"
    );

    let (human, report) = parse_cli_error_stderr(&output.stderr);
    assert!(
        human.contains("unsupported object type 'tree'"),
        "human stderr should explain unsupported type: {human}"
    );
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert!(
        report
            .hints
            .iter()
            .any(|hint| hint.contains("supports only blob objects")),
        "hint should describe supported type: {:?}",
        report.hints
    );
}
