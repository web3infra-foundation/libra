//! Tests `libra blame` for line-level attribution, format envelopes
//! (human/JSON/machine), and SHA-1 vs. SHA-256 repository handling.
//!
//! **Layer:** L1 — deterministic, no external dependencies.
//!
//! Fixture conventions:
//! - CLI-driven cases use `create_committed_repo_via_cli()` from `mod.rs`
//!   plus extra `add`/`commit` invocations through `run_libra_command()`.
//! - In-process cases call `setup_repo_with_hash()` to bootstrap a repo
//!   under a chosen `core.objectformat` and `prepare_history()` to lay
//!   down a known two-commit history of `foo.txt` (line2 changed in the
//!   second commit). The two returned commit hashes act as expected blame
//!   targets.

use std::{fs, io::Write};

use chrono::DateTime;
use libra::{
    command::{
        add::{self, AddArgs},
        blame::{self, BlameArgs},
        commit::{self, CommitArgs},
        get_target_commit,
        init::{self, InitArgs},
    },
    internal::config::ConfigKv,
};
use tempfile::tempdir;

use super::*;

/// Scenario: running `libra blame` outside any repo must exit 128 with a
/// "fatal: not a libra repository" message. Pins the repo-presence guard.
#[test]
fn test_blame_cli_outside_repository_returns_fatal_128() {
    let temp = tempdir().unwrap();
    let output = run_libra_command(&["blame", "some_file.txt"], temp.path());
    assert_eq!(output.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("fatal: not a libra repository"),
        "unexpected stderr: {stderr}"
    );
}

/// Scenario: `--json blame <file>` must emit the canonical envelope with
/// `command="blame"`, `data.file=<path>`, and `data.lines` as an array.
/// Schema pin for downstream JSON consumers.
#[test]
fn test_blame_json_output_includes_lines() {
    let repo = create_committed_repo_via_cli();
    std::fs::write(repo.path().join("tracked.txt"), "line1\nline2\n").unwrap();
    let add_output = run_libra_command(&["add", "tracked.txt"], repo.path());
    assert!(
        add_output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&add_output.stderr)
    );
    let commit_output = run_libra_command(
        &["commit", "-m", "update tracked", "--no-verify"],
        repo.path(),
    );
    assert!(
        commit_output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&commit_output.stderr)
    );

    let output = run_libra_command(&["--json", "blame", "tracked.txt"], repo.path());
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "blame");
    assert_eq!(json["data"]["file"], "tracked.txt");
    assert!(json["data"]["lines"].as_array().is_some());
}

/// Scenario: `--machine blame` must emit exactly one non-empty stdout
/// line of valid JSON (NDJSON-friendly). Mirrors `add_json_test`'s
/// machine-mode contract.
#[test]
fn test_blame_machine_output_is_single_line_json() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["--machine", "blame", "tracked.txt"], repo.path());
    assert_cli_success(&output, "machine blame tracked.txt");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let non_empty_lines: Vec<&str> = stdout.lines().filter(|line| !line.is_empty()).collect();
    assert_eq!(
        non_empty_lines.len(),
        1,
        "machine output should be exactly one non-empty line, got: {stdout}"
    );

    let parsed: serde_json::Value =
        serde_json::from_str(non_empty_lines[0]).expect("machine output should be valid JSON");
    assert_eq!(parsed["command"], "blame");
    assert_eq!(parsed["data"]["file"], "tracked.txt");
    assert!(parsed["data"]["lines"].as_array().is_some());
}

/// Scenario: human-readable blame output must truncate excessively long
/// (Unicode) author names with an ellipsis ("...") rather than corrupt
/// the table layout. Regression guard against char-vs-byte width bugs.
#[test]
fn test_blame_human_output_handles_long_unicode_author_names() {
    let repo = create_committed_repo_via_cli();

    let name_output = run_libra_command(
        &[
            "config",
            "user.name",
            "测试作者名字很长很长很长很长很长很长",
        ],
        repo.path(),
    );
    assert_cli_success(&name_output, "config user.name");
    let email_output = run_libra_command(
        &["config", "user.email", "unicode@example.com"],
        repo.path(),
    );
    assert_cli_success(&email_output, "config user.email");

    std::fs::write(repo.path().join("tracked.txt"), "unicode blame line\n").unwrap();
    let add_output = run_libra_command(&["add", "tracked.txt"], repo.path());
    assert_cli_success(&add_output, "add tracked.txt");
    let commit_output = run_libra_command(
        &["commit", "-m", "unicode author", "--no-verify"],
        repo.path(),
    );
    assert_cli_success(&commit_output, "commit unicode author");

    let output = run_libra_command(&["blame", "tracked.txt"], repo.path());
    assert_cli_success(&output, "blame tracked.txt");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("..."),
        "expected truncated author marker in blame output, got: {stdout}"
    );
}

/// Scenario: each line in JSON blame output must reference the commit
/// hash that introduced it. With the known 2-commit `foo.txt` history,
/// line 1 maps to the first commit and line 2 to the second. The `date`
/// field must be RFC3339-parseable.
#[tokio::test]
#[serial]
async fn test_blame_json_assigns_lines_to_introducing_commits() {
    let repo = tempdir().unwrap();
    let _guard = setup_repo_with_hash(&repo, "sha1").await;
    let (first, second) = prepare_history().await;

    let output = run_libra_command(&["--json", "blame", "foo.txt"], repo.path());
    assert_cli_success(&output, "json blame foo.txt");

    let json = parse_json_stdout(&output);
    let lines = json["data"]["lines"]
        .as_array()
        .expect("blame lines should be an array");
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["line_number"], 1);
    assert_eq!(lines[0]["hash"], first.to_string());
    assert_eq!(lines[1]["line_number"], 2);
    assert_eq!(lines[1]["hash"], second.to_string());
    let date = lines[0]["date"]
        .as_str()
        .expect("blame date should be a string");
    assert!(
        DateTime::parse_from_rfc3339(date).is_ok(),
        "expected RFC3339 blame date, got: {date}"
    );
}

/// Scenario: `-L <n>,<m>` must restrict blame output to the requested
/// line range. Asks for line 2 only and asserts the array has length 1
/// with the expected hash and content.
#[tokio::test]
#[serial]
async fn test_blame_json_line_range_filters_output() {
    let repo = tempdir().unwrap();
    let _guard = setup_repo_with_hash(&repo, "sha1").await;
    let (_first, second) = prepare_history().await;

    let output = run_libra_command(&["--json", "blame", "-L", "2,2", "foo.txt"], repo.path());
    assert_cli_success(&output, "json blame with line range");

    let json = parse_json_stdout(&output);
    let lines = json["data"]["lines"]
        .as_array()
        .expect("blame lines should be an array");
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["line_number"], 2);
    assert_eq!(lines[0]["hash"], second.to_string());
    assert_eq!(lines[0]["content"], "line2-modified");
}

/// Scenario: an out-of-bounds `-L` range must surface as a stable CLI
/// error tagged `LBR-CLI-002` (category `cli`) with exit code 129.
/// Pins the structured error envelope.
#[test]
fn test_blame_invalid_line_range_uses_stable_cli_error() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["blame", "-L", "9,10", "tracked.txt"], repo.path());
    let (_stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(129));
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert_eq!(report.category, "cli");
}

/// Bootstrap a repo with the requested hash algorithm (`"sha1"` or
/// `"sha256"`), set a stable identity, and return the
/// `ChangeDirGuard` that pins the process CWD to the repo for the
/// remainder of the test (RAII; lives to end of test).
async fn setup_repo_with_hash(
    temp: &tempfile::TempDir,
    object_format: &str,
) -> test::ChangeDirGuard {
    test::setup_clean_testing_env_in(temp.path());
    init::init(InitArgs {
        bare: false,
        initial_branch: None,
        repo_directory: temp.path().to_str().unwrap().to_string(),
        template: None,
        quiet: true,
        shared: None,
        object_format: Some(object_format.to_string()),
        ref_format: None,
        from_git_repository: None,
        vault: false,
    })
    .await
    .unwrap();
    let guard = test::ChangeDirGuard::new(temp.path());
    ConfigKv::set("user.name", "Blame Test User", false)
        .await
        .unwrap();
    ConfigKv::set("user.email", "blame-test@example.com", false)
        .await
        .unwrap();
    guard
}

/// Build a fixed two-commit history of `foo.txt`:
///   c1: "line1\nline2\n"      (first hash)
///   c2: "line1\nline2-modified\n" (second hash)
/// Returns `(first, second)` in chronological order. Assumes a
/// `ChangeDirGuard` is already active.
async fn prepare_history() -> (ObjectHash, ObjectHash) {
    // first commit
    let mut f = fs::File::create("foo.txt").unwrap();
    writeln!(f, "line1").unwrap();
    writeln!(f, "line2").unwrap();

    add::execute(AddArgs {
        pathspec: vec!["foo.txt".into()],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
    })
    .await;

    commit::execute(CommitArgs {
        message: Some("init".into()),
        ..Default::default()
    })
    .await;

    let first = get_target_commit("HEAD").await.unwrap();

    // second commit (modify line2)
    let mut f = fs::File::create("foo.txt").unwrap();
    writeln!(f, "line1").unwrap();
    writeln!(f, "line2-modified").unwrap();

    add::execute(AddArgs {
        pathspec: vec!["foo.txt".into()],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
    })
    .await;

    commit::execute(CommitArgs {
        message: Some("update".into()),
        ..Default::default()
    })
    .await;

    let second = get_target_commit("HEAD").await.unwrap();
    (first, second)
}

/// Scenario: `blame::execute` against a SHA-1 repo must complete without
/// panic. Smoke test for the SHA-1 code path.
#[tokio::test]
#[serial]
async fn blame_runs_with_sha1() {
    let repo = tempdir().unwrap();
    let _guard = setup_repo_with_hash(&repo, "sha1").await;
    prepare_history().await;

    // should not panic for SHA-1 repo
    blame::execute(BlameArgs {
        file: "foo.txt".into(),
        commit: "HEAD".into(),
        line_range: None,
    })
    .await;
}

/// Scenario: `blame::execute` against a SHA-256 repo must complete
/// without panic. Smoke test for the SHA-256 code path; pairs with the
/// SHA-1 case to guarantee both are wired through.
#[tokio::test]
#[serial]
async fn blame_runs_with_sha256() {
    let repo = tempdir().unwrap();
    let _guard = setup_repo_with_hash(&repo, "sha256").await;
    prepare_history().await;

    // should not panic for SHA-256 repo
    blame::execute(BlameArgs {
        file: "foo.txt".into(),
        commit: "HEAD".into(),
        line_range: None,
    })
    .await;
}

/// Scenario: a 40-hex (SHA-1 length) commit identifier passed against a
/// SHA-256 repo must be rejected by `get_target_commit`. Format-mismatch
/// regression guard so users do not silently get the wrong commit.
#[tokio::test]
#[serial]
async fn blame_rejects_sha1_length_on_sha256_repo() {
    let repo = tempdir().unwrap();
    let _guard = setup_repo_with_hash(&repo, "sha256").await;
    prepare_history().await;

    // Passing a 40-hex (SHA-1 length) commit id into a SHA-256 repo should be rejected.
    let res = get_target_commit("4b825dc642cb6eb9a060e54bf8d69288fbee4904").await;
    assert!(
        res.is_err(),
        "expect get_target_commit to reject SHA-1 length hash in SHA-256 repo"
    );
}
