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
async fn hash_object_stdin_paths_hashes_each_listed_file() {
    let repo = tempfile::tempdir().expect("create temp repo");
    init_repo_via_cli(repo.path());
    fs::write(repo.path().join("a.txt"), b"hello").expect("write first fixture");
    fs::write(repo.path().join("b.txt"), b"world").expect("write second fixture");

    let output = run_libra_command_with_stdin(
        &["hash-object", "--stdin-paths"],
        repo.path(),
        "a.txt\nb.txt\n",
    );
    assert_cli_success(&output, "hash-object --stdin-paths should succeed");

    let hashes = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    assert_eq!(
        hashes,
        vec![
            "b6fc4c620b67d95f953a5c1c1230aaab5db5a1b0",
            "04fea06420ca60892f73becee3614f6d023a4b7f",
        ]
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

    let output = run_libra_command(&["hash-object", "-t", "bogus", "hello.txt"], repo.path());
    assert_eq!(
        output.status.code(),
        Some(129),
        "an unsupported object type is a usage error"
    );

    let (human, report) = parse_cli_error_stderr(&output.stderr);
    assert!(
        human.contains("unsupported object type 'bogus'"),
        "human stderr should explain unsupported type: {human}"
    );
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert!(
        report
            .hints
            .iter()
            .any(|hint| hint.contains("blob, commit, tree, tag")),
        "hint should list supported types: {:?}",
        report.hints
    );
}

/// Run `git hash-object -t <type>` on a file and return the hash it computes.
fn git_hash_object(object_type: &str, path: &std::path::Path) -> String {
    let output = std::process::Command::new("git")
        .args(["hash-object", "-t", object_type, "--literally"])
        .arg(path)
        .output()
        .expect("run git hash-object");
    assert!(
        output.status.success(),
        "git hash-object failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

const VALID_COMMIT_BODY: &str = "tree 4b825dc642cb6eb9a060e54bf8d69288fbee4904\nauthor Test <test@example.com> 1700000000 +0000\ncommitter Test <test@example.com> 1700000000 +0000\n\nhello\n";

/// `-t commit` computes the same hash Git does for an identical commit body.
#[tokio::test]
async fn hash_object_commit_matches_git() {
    let repo = tempfile::tempdir().expect("create temp repo");
    init_repo_via_cli(repo.path());
    fs::write(repo.path().join("commit.txt"), VALID_COMMIT_BODY).expect("write commit body");

    let expected = git_hash_object("commit", &repo.path().join("commit.txt"));
    let output = run_libra_command(&["hash-object", "-t", "commit", "commit.txt"], repo.path());
    assert_cli_success(&output, "hash-object -t commit should succeed");
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), expected);
}

/// A malformed commit is rejected (exit 128) without `--literally`.
#[tokio::test]
async fn hash_object_corrupt_commit_without_literally_exits_128() {
    let repo = tempfile::tempdir().expect("create temp repo");
    init_repo_via_cli(repo.path());
    fs::write(repo.path().join("bad.txt"), b"not a commit\n").expect("write");

    let output = run_libra_command(&["hash-object", "-t", "commit", "bad.txt"], repo.path());
    assert_eq!(
        output.status.code(),
        Some(128),
        "corrupt commit should exit 128"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("corrupt commit object"),
        "should report corruption: {stderr}"
    );
    assert!(!stderr.contains("panic"), "must not leak a panic: {stderr}");
}

/// `--literally` hashes malformed content as-is (exit 0).
#[tokio::test]
async fn hash_object_literally_skips_validation() {
    let repo = tempfile::tempdir().expect("create temp repo");
    init_repo_via_cli(repo.path());
    fs::write(repo.path().join("bad.txt"), b"not a commit\n").expect("write");

    let output = run_libra_command(
        &["hash-object", "-t", "commit", "--literally", "bad.txt"],
        repo.path(),
    );
    assert_cli_success(&output, "hash-object --literally should accept any content");
    let expected = git_hash_object("commit", &repo.path().join("bad.txt"));
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), expected);
}

/// `-t commit -w` persists the object so `cat-file -t` reads it back as a commit.
#[tokio::test]
async fn hash_object_write_commit_persists_for_cat_file() {
    let repo = tempfile::tempdir().expect("create temp repo");
    init_repo_via_cli(repo.path());
    fs::write(repo.path().join("commit.txt"), VALID_COMMIT_BODY).expect("write commit body");

    let output = run_libra_command(
        &["hash-object", "-t", "commit", "-w", "commit.txt"],
        repo.path(),
    );
    assert_cli_success(&output, "hash-object -t commit -w should succeed");
    let oid = String::from_utf8_lossy(&output.stdout).trim().to_string();

    let cat = run_libra_command(&["cat-file", "-t", &oid], repo.path());
    assert_cli_success(&cat, "cat-file -t should read the written commit");
    assert_eq!(String::from_utf8_lossy(&cat.stdout).trim(), "commit");
}

/// The `--json` output reports the resolved object type.
#[tokio::test]
async fn hash_object_json_object_type_reflects_resolved_type() {
    let repo = tempfile::tempdir().expect("create temp repo");
    init_repo_via_cli(repo.path());
    fs::write(repo.path().join("commit.txt"), VALID_COMMIT_BODY).expect("write commit body");

    let output = run_libra_command(
        &[
            "--json=compact",
            "hash-object",
            "-t",
            "commit",
            "commit.txt",
        ],
        repo.path(),
    );
    assert_cli_success(&output, "hash-object --json -t commit should succeed");
    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["object_type"], "commit");
}

/// `--path` sets the reported source label for `--stdin` input.
#[tokio::test]
async fn hash_object_path_flag_used_as_source_label() {
    let repo = tempfile::tempdir().expect("create temp repo");
    init_repo_via_cli(repo.path());

    let output = run_libra_command_with_stdin(
        &[
            "--json=compact",
            "hash-object",
            "--stdin",
            "--path",
            "foo.txt",
        ],
        repo.path(),
        "hello",
    );
    assert_cli_success(&output, "hash-object --stdin --path should succeed");
    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["objects"][0]["source"], "foo.txt");
}

/// `--no-filters` is a no-op: content is hashed verbatim (no CRLF conversion).
#[tokio::test]
async fn hash_object_no_filters_is_noop() {
    let repo = tempfile::tempdir().expect("create temp repo");
    init_repo_via_cli(repo.path());

    let plain =
        run_libra_command_with_stdin(&["hash-object", "--stdin"], repo.path(), "a\r\nb\r\n");
    let filtered = run_libra_command_with_stdin(
        &["hash-object", "--stdin", "--no-filters"],
        repo.path(),
        "a\r\nb\r\n",
    );
    assert_cli_success(&plain, "plain stdin hash");
    assert_cli_success(&filtered, "--no-filters stdin hash");
    assert_eq!(
        String::from_utf8_lossy(&plain.stdout),
        String::from_utf8_lossy(&filtered.stdout),
        "--no-filters must not change the hash"
    );
}

/// `--path` conflicts with `--no-filters` and `--stdin-paths` at parse time.
#[tokio::test]
async fn hash_object_path_conflicts_are_rejected() {
    let repo = tempfile::tempdir().expect("create temp repo");
    init_repo_via_cli(repo.path());

    for argv in [
        vec!["hash-object", "--stdin", "--path", "f.txt", "--no-filters"],
        vec!["hash-object", "--stdin-paths", "--path", "f.txt"],
    ] {
        let output = run_libra_command(&argv, repo.path());
        assert_eq!(
            output.status.code(),
            Some(129),
            "conflicting --path usage {argv:?} should be rejected"
        );
    }
}
