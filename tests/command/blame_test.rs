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
    let lines = json["data"]["lines"].as_array().expect("lines array");
    assert!(!lines.is_empty(), "expected blamed lines");
    // Backward-compatible schema: existing fields plus the appended ones.
    let first = &lines[0];
    assert!(first["line_number"].is_number());
    assert!(first["hash"].is_string());
    assert!(
        first["email"].is_string(),
        "appended email field must be present"
    );
    assert!(
        first["timestamp"].is_number(),
        "appended timestamp field present"
    );
    assert!(first["original_line_number"].is_number());
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

/// Scenario: a missing path in JSON mode must use the stable invalid-target
/// code so agents can distinguish user input errors from repository failures.
#[test]
fn test_blame_json_file_not_found_uses_stable_error() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["--json", "blame", "missing.txt"], repo.path());
    let (_stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(129));
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert_eq!(report.category, "cli");
    assert!(report.message.contains("file 'missing.txt' not found"));
}

/// Scenario: an invalid revision in JSON mode must be reported as a stable
/// invalid-target error rather than a generic fatal failure.
#[test]
fn test_blame_json_invalid_revision_uses_stable_error() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(
        &["--json", "blame", "tracked.txt", "no-such-rev"],
        repo.path(),
    );
    let (_stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(129));
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert_eq!(report.category, "cli");
    assert!(report.message.contains("invalid revision: 'no-such-rev'"));
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

/// Scenario: all documented `-L` syntaxes must work in JSON mode: a single
/// line and a relative `START,+COUNT` range.
#[tokio::test]
#[serial]
async fn test_blame_json_line_range_single_and_relative_forms() {
    let repo = tempdir().unwrap();
    let _guard = setup_repo_with_hash(&repo, "sha1").await;
    let (first, second) = prepare_history().await;

    let output = run_libra_command(&["--json", "blame", "-L", "1", "foo.txt"], repo.path());
    assert_cli_success(&output, "json blame single line range");
    let json = parse_json_stdout(&output);
    let lines = json["data"]["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["line_number"], 1);
    assert_eq!(lines[0]["hash"], first.to_string());

    let output = run_libra_command(&["--json", "blame", "-L", "1,+2", "foo.txt"], repo.path());
    assert_cli_success(&output, "json blame relative line range");
    let json = parse_json_stdout(&output);
    let lines = json["data"]["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["line_number"], 1);
    assert_eq!(lines[0]["hash"], first.to_string());
    assert_eq!(lines[1]["line_number"], 2);
    assert_eq!(lines[1]["hash"], second.to_string());
}

/// Scenario: empty files should be valid JSON results with an empty `lines`
/// array, not a special-case error.
#[test]
fn test_blame_json_empty_file_returns_empty_lines() {
    let repo = create_committed_repo_via_cli();
    fs::write(repo.path().join("empty.txt"), "").unwrap();
    let output = run_libra_command(&["add", "empty.txt"], repo.path());
    assert_cli_success(&output, "add empty file");
    let output = run_libra_command(&["commit", "-m", "empty file", "--no-verify"], repo.path());
    assert_cli_success(&output, "commit empty file");

    let output = run_libra_command(&["--json", "blame", "empty.txt"], repo.path());
    assert_cli_success(&output, "json blame empty file");
    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["file"], "empty.txt");
    assert_eq!(json["data"]["lines"].as_array().unwrap().len(), 0);
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
        ..Default::default()
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
        ..Default::default()
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
        ..Default::default()
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
        ..Default::default()
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

/// Build a committed repo containing `tracked.txt` (3 lines) for human-render tests.
fn repo_with_tracked_file() -> tempfile::TempDir {
    let repo = create_committed_repo_via_cli();
    std::fs::write(repo.path().join("tracked.txt"), "line1\nline2\nline3\n").unwrap();
    let add = run_libra_command(&["add", "tracked.txt"], repo.path());
    assert!(
        add.status.success(),
        "add: {}",
        String::from_utf8_lossy(&add.stderr)
    );
    let commit = run_libra_command(&["commit", "-m", "add tracked", "--no-verify"], repo.path());
    assert!(
        commit.status.success(),
        "commit: {}",
        String::from_utf8_lossy(&commit.stderr)
    );
    repo
}

/// `-s` suppresses the author/date columns; the line number follows the open
/// paren directly (`(<num>)`), which never happens when metadata is present.
#[test]
fn test_blame_human_suppress_metadata() {
    let repo = repo_with_tracked_file();
    let out = run_libra_command(&["blame", "-s", "tracked.txt"], repo.path());
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("(1)") && stdout.contains("line1"),
        "-s output should put the line number right after '(' : {stdout}"
    );
}

/// `-l` shows the full-length commit hash (40 hex for SHA-1, 64 for SHA-256).
#[test]
fn test_blame_human_long_rev() {
    let repo = repo_with_tracked_file();
    let out = run_libra_command(&["blame", "-l", "tracked.txt"], repo.path());
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.split_whitespace().any(|t| {
            (t.len() == 40 || t.len() == 64) && t.chars().all(|c| c.is_ascii_hexdigit())
        }),
        "-l output should contain a full-length hash: {stdout}"
    );
}

/// `-t` shows the raw epoch timestamp (a long digit run) instead of a date.
#[test]
fn test_blame_human_raw_timestamp() {
    let repo = repo_with_tracked_file();
    let out = run_libra_command(&["blame", "-t", "tracked.txt"], repo.path());
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let has_epoch = stdout
        .split(|c: char| !c.is_ascii_digit())
        .any(|run| run.len() >= 9);
    assert!(
        has_epoch,
        "-t output should contain a raw epoch timestamp: {stdout}"
    );
}

/// `-e` shows the author email (contains `@`) instead of the name.
#[test]
fn test_blame_human_show_email() {
    let repo = repo_with_tracked_file();
    let out = run_libra_command(&["blame", "-e", "tracked.txt"], repo.path());
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains('@'),
        "-e output should show an email: {stdout}"
    );
}

/// `-n` shows the original (pre-image) line number: a line that shifted down
/// because a line was prepended keeps its original number.
#[test]
fn test_blame_human_show_number() {
    let repo = create_committed_repo_via_cli();
    std::fs::write(repo.path().join("t.txt"), "alpha\nbeta\n").unwrap();
    assert!(
        run_libra_command(&["add", "t.txt"], repo.path())
            .status
            .success()
    );
    assert!(
        run_libra_command(&["commit", "-m", "c1", "--no-verify"], repo.path())
            .status
            .success()
    );
    // Prepend a line so alpha/beta shift from original lines 1/2 to final 2/3.
    std::fs::write(repo.path().join("t.txt"), "zero\nalpha\nbeta\n").unwrap();
    assert!(
        run_libra_command(&["add", "t.txt"], repo.path())
            .status
            .success()
    );
    assert!(
        run_libra_command(&["commit", "-m", "c2", "--no-verify"], repo.path())
            .status
            .success()
    );

    let out = run_libra_command(&["blame", "-n", "t.txt"], repo.path());
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // With -n, alpha shows its ORIGINAL line number 1 (not final 2).
    assert!(
        stdout.contains("1) alpha") && stdout.contains("2) beta"),
        "-n should show original line numbers: {stdout}"
    );
}

/// `-l -n -e` combine: full hash + original line numbers + email all present.
#[test]
fn test_blame_human_combined_flags() {
    let repo = repo_with_tracked_file();
    let out = run_libra_command(&["blame", "-l", "-n", "-e", "tracked.txt"], repo.path());
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains('@'), "combined -e: {stdout}");
    assert!(
        stdout.split_whitespace().any(|t| {
            (t.len() == 40 || t.len() == 64) && t.chars().all(|c| c.is_ascii_hexdigit())
        }),
        "combined -l: {stdout}"
    );
}

/// `-M`/`-C` (incl. `-M=50`) parse and fall back to same-file blame (parsed only).
#[test]
fn test_blame_accepts_move_copy_flags() {
    let repo = repo_with_tracked_file();
    let plain = run_libra_command(&["--json", "blame", "tracked.txt"], repo.path());
    assert!(plain.status.success());
    let moved = run_libra_command(&["--json", "blame", "-M=50", "tracked.txt"], repo.path());
    assert!(
        moved.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&moved.stderr)
    );
    let copied = run_libra_command(&["--json", "blame", "-C", "tracked.txt"], repo.path());
    assert!(
        copied.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&copied.stderr)
    );
    // Same-file fallback: attribution identical to the no-flag run.
    assert_eq!(
        parse_json_stdout(&plain)["data"]["lines"],
        parse_json_stdout(&moved)["data"]["lines"]
    );
}

/// `-M`/`-C` must not swallow the FILE positional (require_equals contract).
#[test]
fn test_blame_move_copy_flag_does_not_swallow_file() {
    use clap::Parser;
    let a = BlameArgs::try_parse_from(["blame", "-M", "file.txt"]).expect("bare -M parses");
    assert_eq!(a.file, "file.txt");
    assert_eq!(a.detect_moved, Some(0));
    let b = BlameArgs::try_parse_from(["blame", "-C", "file.txt"]).expect("bare -C parses");
    assert_eq!(b.file, "file.txt");
    assert_eq!(b.detect_copied, Some(0));
    let c = BlameArgs::try_parse_from(["blame", "-M=50", "file.txt"]).expect("-M=50 parses");
    assert_eq!(c.file, "file.txt");
    assert_eq!(c.detect_moved, Some(50));
    let d = BlameArgs::try_parse_from(["blame", "-C=50", "file.txt"]).expect("-C=50 parses");
    assert_eq!(d.file, "file.txt");
    assert_eq!(d.detect_copied, Some(50));
}

/// An unknown flag is rejected with the default coarse usage exit code 129.
#[test]
fn test_blame_bogus_flag_exits_129() {
    let repo = repo_with_tracked_file();
    let out = run_libra_command(&["blame", "--bogus-flag", "tracked.txt"], repo.path());
    assert_eq!(
        out.status.code(),
        Some(129),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Build a committed repo containing a 10-line `ten.txt` for `-L` range tests.
fn repo_with_ten_line_file() -> tempfile::TempDir {
    let repo = create_committed_repo_via_cli();
    let content: String = (1..=10).map(|i| format!("line{i}\n")).collect();
    std::fs::write(repo.path().join("ten.txt"), content).unwrap();
    assert!(
        run_libra_command(&["add", "ten.txt"], repo.path())
            .status
            .success()
    );
    assert!(
        run_libra_command(&["commit", "-m", "ten", "--no-verify"], repo.path())
            .status
            .success()
    );
    repo
}

fn porcelain_first_token_is_full_hash(stdout: &str) -> bool {
    stdout
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().next())
        .is_some_and(|t| {
            (t.len() == 40 || t.len() == 64) && t.chars().all(|c| c.is_ascii_hexdigit())
        })
}

/// `--porcelain` emits a `<hash> <orig> <final> [count]` header, `author ` KV
/// lines, and Tab-prefixed content lines.
#[test]
fn test_blame_porcelain() {
    let repo = repo_with_tracked_file();
    let out = run_libra_command(&["blame", "--porcelain", "tracked.txt"], repo.path());
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        porcelain_first_token_is_full_hash(&stdout),
        "header hash: {stdout}"
    );
    assert!(
        stdout.lines().any(|l| l.starts_with("author ")),
        "author KV: {stdout}"
    );
    assert!(
        stdout.lines().any(|l| l.starts_with("author-mail <")),
        "author-mail KV: {stdout}"
    );
    assert!(
        stdout.lines().any(|l| l.starts_with('\t')),
        "tab content: {stdout}"
    );
}

/// `-p` is an exact alias of `--porcelain`.
#[test]
fn test_blame_porcelain_p_alias() {
    let repo = repo_with_tracked_file();
    let p = run_libra_command(&["blame", "-p", "tracked.txt"], repo.path());
    let long = run_libra_command(&["blame", "--porcelain", "tracked.txt"], repo.path());
    assert!(p.status.success() && long.status.success());
    assert_eq!(
        p.stdout, long.stdout,
        "-p and --porcelain must be byte-identical"
    );
}

/// Porcelain hash header is always full-length, unaffected by `-l`/`-s`.
#[test]
fn test_blame_porcelain_hash_full_length() {
    let repo = repo_with_tracked_file();
    for extra in [["-l"], ["-s"]] {
        let mut argv = vec!["blame", "--porcelain"];
        argv.extend_from_slice(&extra);
        argv.push("tracked.txt");
        let out = run_libra_command(&argv, repo.path());
        assert!(
            out.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            porcelain_first_token_is_full_hash(&stdout),
            "porcelain hash must stay full-length with {extra:?}: {stdout}"
        );
    }
}

/// `-L 5,9999` on a 10-line file clamps the end to 10 (lines 5..=10), exit 0.
#[test]
fn test_blame_line_range_truncates_overlong_end() {
    let repo = repo_with_ten_line_file();
    let out = run_libra_command(&["--json", "blame", "-L", "5,9999", "ten.txt"], repo.path());
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json = parse_json_stdout(&out);
    let lines = json["data"]["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 6, "lines 5..=10 expected");
}

/// `-L 1,+100` on a 10-line file returns all 10 lines (relative end clamped).
#[test]
fn test_blame_line_range_relative_truncation() {
    let repo = repo_with_ten_line_file();
    let out = run_libra_command(&["--json", "blame", "-L", "1,+100", "ten.txt"], repo.path());
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json = parse_json_stdout(&out);
    assert_eq!(json["data"]["lines"].as_array().unwrap().len(), 10);
}

/// `-L 9999,10000` (start past EOF) is an error, not an empty result.
#[test]
fn test_blame_line_range_start_overflow_errors() {
    let repo = repo_with_ten_line_file();
    let out = run_libra_command(&["blame", "-L", "9999,10000", "ten.txt"], repo.path());
    assert_eq!(
        out.status.code(),
        Some(129),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `-L 1,+<usize::MAX>` must error via checked arithmetic, not panic.
#[test]
fn test_blame_line_range_offset_arithmetic_overflow() {
    let repo = repo_with_ten_line_file();
    let out = run_libra_command(
        &["blame", "-L", "1,+18446744073709551615", "ten.txt"],
        repo.path(),
    );
    assert_eq!(
        out.status.code(),
        Some(129),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A non-UTF-8 blob renders under `--porcelain` via lossy decoding without panic.
#[test]
fn test_blame_porcelain_non_utf8_lossy() {
    let repo = create_committed_repo_via_cli();
    std::fs::write(repo.path().join("bin.txt"), [0xFFu8, 0xFE, b'\n']).unwrap();
    assert!(
        run_libra_command(&["add", "bin.txt"], repo.path())
            .status
            .success()
    );
    assert!(
        run_libra_command(&["commit", "-m", "bin", "--no-verify"], repo.path())
            .status
            .success()
    );
    let out = run_libra_command(&["blame", "--porcelain", "bin.txt"], repo.path());
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `--json` takes precedence over `--porcelain` (root-global JSON wins).
#[test]
fn test_blame_json_overrides_porcelain() {
    let repo = repo_with_tracked_file();
    let out = run_libra_command(&["--json", "blame", "-p", "tracked.txt"], repo.path());
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json = parse_json_stdout(&out);
    assert_eq!(json["command"], "blame");
    assert!(json["data"]["lines"].as_array().is_some());
}

/// Porcelain `<orig-lineno>` reflects the parent-side line number: after a line
/// is prepended, an inherited line keeps its original (pre-image) number.
#[test]
fn test_blame_porcelain_orig_lineno_simple_insert() {
    let repo = create_committed_repo_via_cli();
    std::fs::write(repo.path().join("ins.txt"), "alpha\nbeta\n").unwrap();
    assert!(
        run_libra_command(&["add", "ins.txt"], repo.path())
            .status
            .success()
    );
    assert!(
        run_libra_command(&["commit", "-m", "c1", "--no-verify"], repo.path())
            .status
            .success()
    );
    std::fs::write(repo.path().join("ins.txt"), "zero\nalpha\nbeta\n").unwrap();
    assert!(
        run_libra_command(&["add", "ins.txt"], repo.path())
            .status
            .success()
    );
    assert!(
        run_libra_command(&["commit", "-m", "c2", "--no-verify"], repo.path())
            .status
            .success()
    );

    let out = run_libra_command(&["blame", "--porcelain", "ins.txt"], repo.path());
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // alpha: final line 2, original (pre-image) line 1 -> a header "<hash> 1 2 ...".
    assert!(
        stdout.lines().any(|l| {
            let t: Vec<&str> = l.split_whitespace().collect();
            t.len() >= 3 && t[0].len() >= 40 && t[1] == "1" && t[2] == "2"
        }),
        "expected a porcelain header with orig=1 final=2 for the shifted line: {stdout}"
    );
}

fn first_line_hash(out: &std::process::Output) -> String {
    parse_json_stdout(out)["data"]["lines"][0]["hash"]
        .as_str()
        .expect("line hash")
        .to_string()
}

/// `-w` attributes a whitespace-only (indent) change to the OLDER commit, while
/// the default byte-exact comparison attributes it to the indent changer. Also
/// confirms `-w` works through the `--json` path. (Batch 2 tests 1, 2, 6.)
#[test]
fn test_blame_ignore_whitespace_attributes_to_older() {
    let repo = create_committed_repo_via_cli();
    std::fs::write(repo.path().join("f.txt"), "    foo\n").unwrap();
    assert!(
        run_libra_command(&["add", "f.txt"], repo.path())
            .status
            .success()
    );
    assert!(
        run_libra_command(&["commit", "-m", "c1", "--no-verify"], repo.path())
            .status
            .success()
    );
    let c1 = first_line_hash(&run_libra_command(
        &["--json", "blame", "f.txt"],
        repo.path(),
    ));

    // C2: whitespace-only indent change (4 spaces -> 2 spaces).
    std::fs::write(repo.path().join("f.txt"), "  foo\n").unwrap();
    assert!(
        run_libra_command(&["add", "f.txt"], repo.path())
            .status
            .success()
    );
    assert!(
        run_libra_command(&["commit", "-m", "c2", "--no-verify"], repo.path())
            .status
            .success()
    );

    let with_w = first_line_hash(&run_libra_command(
        &["--json", "blame", "-w", "f.txt"],
        repo.path(),
    ));
    assert_eq!(
        with_w, c1,
        "-w should attribute the indent-only change to C1"
    );

    let without_w = first_line_hash(&run_libra_command(
        &["--json", "blame", "f.txt"],
        repo.path(),
    ));
    assert_ne!(
        without_w, c1,
        "without -w the indent changer (C2) owns the line"
    );
}

/// `-w` treats an all-whitespace (blank) line whose indentation changed as
/// unchanged, attributing it to the older commit.
#[test]
fn test_blame_ignore_whitespace_blank_line() {
    let repo = create_committed_repo_via_cli();
    std::fs::write(repo.path().join("b.txt"), "alpha\n    \n").unwrap();
    assert!(
        run_libra_command(&["add", "b.txt"], repo.path())
            .status
            .success()
    );
    assert!(
        run_libra_command(&["commit", "-m", "c1", "--no-verify"], repo.path())
            .status
            .success()
    );
    let c1 = first_line_hash(&run_libra_command(
        &["--json", "blame", "b.txt"],
        repo.path(),
    ));

    // Change only the amount of whitespace on the blank line.
    std::fs::write(repo.path().join("b.txt"), "alpha\n  \n").unwrap();
    assert!(
        run_libra_command(&["add", "b.txt"], repo.path())
            .status
            .success()
    );
    assert!(
        run_libra_command(&["commit", "-m", "c2", "--no-verify"], repo.path())
            .status
            .success()
    );

    let json = parse_json_stdout(&run_libra_command(
        &["--json", "blame", "-w", "b.txt"],
        repo.path(),
    ));
    let line2 = json["data"]["lines"][1]["hash"]
        .as_str()
        .expect("line 2 hash");
    assert_eq!(
        line2, c1,
        "-w should attribute the blank-line whitespace change to C1"
    );
}

/// BFS early-exit correctness: on a known 2-commit history, line 1 stays
/// attributed to its introducing commit (C1) and is never mis-folded into a
/// later commit by the early-exit. Line 2 (changed in C2) is a different commit.
#[test]
fn test_blame_bfs_early_exit_correctness() {
    let repo = create_committed_repo_via_cli();
    std::fs::write(repo.path().join("h.txt"), "a\nb\n").unwrap();
    assert!(
        run_libra_command(&["add", "h.txt"], repo.path())
            .status
            .success()
    );
    assert!(
        run_libra_command(&["commit", "-m", "c1", "--no-verify"], repo.path())
            .status
            .success()
    );
    let c1 = first_line_hash(&run_libra_command(
        &["--json", "blame", "h.txt"],
        repo.path(),
    ));

    std::fs::write(repo.path().join("h.txt"), "a\nB\n").unwrap();
    assert!(
        run_libra_command(&["add", "h.txt"], repo.path())
            .status
            .success()
    );
    assert!(
        run_libra_command(&["commit", "-m", "c2", "--no-verify"], repo.path())
            .status
            .success()
    );

    let json = parse_json_stdout(&run_libra_command(
        &["--json", "blame", "h.txt"],
        repo.path(),
    ));
    let lines = json["data"]["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 2);
    assert_eq!(
        lines[0]["hash"].as_str().unwrap(),
        c1,
        "line 1 must stay attributed to C1"
    );
    assert_ne!(
        lines[1]["hash"].as_str().unwrap(),
        c1,
        "line 2 (changed in C2) must not be attributed to C1"
    );
}
