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
        pathspec_from_file: None,
        pathspec_file_nul: false,
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

#[test]
fn test_for_each_ref_contains_filter() {
    use super::{assert_cli_success, create_committed_repo_via_cli, run_libra_command};

    let repo = create_committed_repo_via_cli();
    let p = repo.path();

    std::fs::write(p.join("f1.txt"), "1\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "f1.txt"], p), "add f1");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "c1", "--no-verify"], p),
        "commit c1",
    );
    // `old` points at c1 and never advances.
    assert_cli_success(&run_libra_command(&["branch", "old"], p), "branch old");

    std::fs::write(p.join("f2.txt"), "2\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "f2.txt"], p), "add f2");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "c2", "--no-verify"], p),
        "commit c2",
    );

    let head = run_libra_command(&["rev-parse", "HEAD"], p);
    let c2 = String::from_utf8_lossy(&head.stdout).trim().to_string();

    // Only main (at c2) contains c2; `old` (at c1) does not.
    let out = run_libra_command(&["for-each-ref", "--heads", "--contains", &c2], p);
    assert_cli_success(&out, "for-each-ref --contains c2");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("refs/heads/main"),
        "main should contain c2: {stdout}"
    );
    assert!(
        !stdout.contains("refs/heads/old"),
        "old should NOT contain c2: {stdout}"
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
async fn test_for_each_ref_points_at_matches_direct_and_peeled_tag_targets() {
    let temp = tempdir().unwrap();
    setup_repo_with_commit(&temp).await;

    let lightweight = run_libra_command(&["tag", "lw"], temp.path());
    assert_cli_success(&lightweight, "tag lw should succeed");
    let annotated = run_libra_command(&["tag", "-m", "annotated", "ann"], temp.path());
    assert_cli_success(&annotated, "tag -m ann should succeed");

    let head_output = run_libra_command(
        &[
            "for-each-ref",
            "--points-at",
            "HEAD",
            "--format=%(refname) %(objecttype)",
        ],
        temp.path(),
    );
    assert_cli_success(&head_output, "for-each-ref --points-at HEAD should succeed");
    let head_stdout = String::from_utf8_lossy(&head_output.stdout);
    assert!(
        head_stdout.contains("refs/heads/main commit"),
        "expected main branch in --points-at HEAD output, got: {head_stdout}"
    );
    assert!(
        head_stdout.contains("refs/tags/lw commit"),
        "expected lightweight tag in --points-at HEAD output, got: {head_stdout}"
    );
    assert!(
        head_stdout.contains("refs/tags/ann tag"),
        "expected annotated tag in --points-at HEAD output, got: {head_stdout}"
    );

    let tag_object_output = run_libra_command(
        &["for-each-ref", "--points-at", "ann", "--format=%(refname)"],
        temp.path(),
    );
    assert_cli_success(
        &tag_object_output,
        "for-each-ref --points-at ann should succeed",
    );
    let tag_stdout = String::from_utf8_lossy(&tag_object_output.stdout);
    assert_eq!(
        tag_stdout.trim(),
        "refs/tags/ann",
        "expected only annotated tag object ref, got: {tag_stdout}"
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
