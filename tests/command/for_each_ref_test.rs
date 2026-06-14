//! Integration tests for `libra for-each-ref`.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use std::{fs, io::Write};

use serial_test::serial;
use tempfile::tempdir;

use super::*;

/// Create a repo, add a file and commit with the given message.
async fn setup_repo_with_commit(temp: &tempfile::TempDir) {
    test::setup_with_new_libra_in(temp.path()).await;
    let _guard = ChangeDirGuard::new(temp.path());

    let mut f = fs::File::create("a.txt").unwrap();
    writeln!(f, "hello").unwrap();

    add::execute(AddArgs {
        pathspec: vec!["a.txt".into()],
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
        message: Some("initial".into()),
        ..Default::default()
    })
    .await;
}

#[tokio::test]
#[serial]
async fn test_for_each_ref_lists_heads() {
    let temp = tempdir().unwrap();
    setup_repo_with_commit(&temp).await;

    let output = run_libra_command(&["for-each-ref", "--heads"], temp.path());
    assert_cli_success(&output, "for-each-ref --heads should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("refs/heads/main"),
        "expected refs/heads/main in output, got: {stdout}"
    );
}

#[tokio::test]
#[serial]
async fn test_for_each_ref_format_and_json() {
    let temp = tempdir().unwrap();
    setup_repo_with_commit(&temp).await;

    let output = run_libra_command(
        &["--json", "for-each-ref", "--heads", "--format=%(refname)"],
        temp.path(),
    );
    assert_cli_success(&output, "for-each-ref --json should succeed");

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "for-each-ref");
    let entries = json["data"].as_array().expect("data should be an array");
    assert!(
        entries
            .iter()
            .any(|entry| entry["refname"] == "refs/heads/main"),
        "expected refs/heads/main in JSON output"
    );
}

#[tokio::test]
#[serial]
async fn test_for_each_ref_sort_and_count() {
    let temp = tempdir().unwrap();
    setup_repo_with_commit(&temp).await;

    let output = run_libra_command(&["for-each-ref", "--count=1"], temp.path());
    assert_cli_success(&output, "for-each-ref --count should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.lines().count(),
        1,
        "expected exactly one line, got: {stdout}"
    );
}

#[tokio::test]
#[serial]
async fn test_for_each_ref_unknown_sort_rejects() {
    let temp = tempdir().unwrap();
    setup_repo_with_commit(&temp).await;

    let output = run_libra_command(&["for-each-ref", "--sort=unknown"], temp.path());
    assert!(
        !output.status.success(),
        "expected failure for unsupported sort key"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unsupported for-each-ref sort key"),
        "got: {stderr}"
    );
}
