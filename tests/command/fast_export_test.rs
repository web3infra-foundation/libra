//! Integration tests for `libra fast-export`.
//!
//! Layer: L1 (deterministic; tempdir + isolated HOME, no network).

use std::{fs, process::Output};

use tempfile::{TempDir, tempdir};

use super::{assert_cli_success, create_committed_repo_via_cli, run_libra_command};

fn out(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

#[test]
fn fast_export_emits_a_fast_import_stream() {
    let repo = create_committed_repo_via_cli();
    let result = run_libra_command(&["fast-export"], repo.path());
    assert_eq!(
        result.status.code(),
        Some(0),
        "fast-export failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stream = out(&result);
    assert!(stream.contains("blob\n"), "should emit blobs: {stream}");
    assert!(
        stream.contains("data "),
        "should emit data sections: {stream}"
    );
    assert!(
        stream.contains("commit refs/heads/"),
        "should emit a commit under a branch ref: {stream}"
    );
    assert!(
        stream.contains("deleteall\n"),
        "should reconstruct the tree: {stream}"
    );
    assert!(
        stream.contains("M 100644 :") && stream.contains("tracked.txt"),
        "should emit the tracked file with a mark: {stream}"
    );
    assert!(
        stream.contains("committer ") && stream.contains("author "),
        "should emit author/committer: {stream}"
    );
}

/// A second commit must reference its parent with `from :<mark>` (proving the
/// topological ordering emits parents first).
#[test]
fn fast_export_chains_parents() {
    let repo = repo_with_two_commits();
    let result = run_libra_command(&["fast-export"], repo.path());
    assert_eq!(result.status.code(), Some(0));
    assert!(
        out(&result).contains("from :"),
        "the child commit should chain to its parent: {}",
        out(&result)
    );
}

#[test]
fn fast_export_accepts_an_explicit_rev() {
    let repo = create_committed_repo_via_cli();
    let result = run_libra_command(&["fast-export", "HEAD"], repo.path());
    assert_eq!(result.status.code(), Some(0));
    assert!(out(&result).contains("commit refs/heads/"));
}

#[test]
fn fast_export_bad_rev_is_an_error() {
    let repo = create_committed_repo_via_cli();
    let result = run_libra_command(&["fast-export", "not-a-rev"], repo.path());
    assert_eq!(result.status.code(), Some(128));
}

#[test]
fn fast_export_outside_repository_is_an_error() {
    let dir = tempdir().unwrap();
    let result = run_libra_command(&["fast-export"], dir.path());
    assert_eq!(result.status.code(), Some(128));
}

fn repo_with_two_commits() -> TempDir {
    let repo = create_committed_repo_via_cli();
    fs::write(repo.path().join("tracked.txt"), "second\n").unwrap();
    assert_cli_success(
        &run_libra_command(&["add", "tracked.txt"], repo.path()),
        "stage second",
    );
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "second", "--no-verify"], repo.path()),
        "second commit",
    );
    repo
}
