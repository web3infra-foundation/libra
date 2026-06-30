//! Integration tests for `libra replace`.
//!
//! Layer: L1 (deterministic; tempdir + isolated HOME, no network). These run
//! the CLI as subprocesses, so each `libra` invocation rebuilds the
//! `refs/replace` peel cache from scratch.

use std::fs;

use tempfile::{TempDir, tempdir};

use super::{assert_cli_success, create_committed_repo_via_cli, run_libra_command};

/// A repo with a root commit "base" (HEAD~1) and a child commit "second" (HEAD).
fn two_commit_repo() -> TempDir {
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

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

/// The core peel: replacing HEAD (the "second" commit) with HEAD~1 ("base")
/// makes `log` read the replacement through `load_object`.
#[test]
fn replace_makes_log_read_the_replacement() {
    let repo = two_commit_repo();

    // Sanity: before replacing, log shows "second".
    let before = run_libra_command(&["log", "-1"], repo.path());
    assert!(
        stdout(&before).contains("second"),
        "pre-state: {}",
        stdout(&before)
    );

    let created = run_libra_command(&["replace", "HEAD", "HEAD~1"], repo.path());
    assert_eq!(
        created.status.code(),
        Some(0),
        "replace failed: {}",
        String::from_utf8_lossy(&created.stderr)
    );

    // HEAD's commit now reads back as "base" (its replacement, HEAD~1).
    let after = run_libra_command(&["log", "-1"], repo.path());
    let text = stdout(&after);
    assert!(text.contains("base"), "peel did not apply: {text}");
    assert!(!text.contains("second"), "replacement was not used: {text}");
}

#[test]
fn replace_list_shows_the_replaced_object() {
    let repo = two_commit_repo();
    let head = run_libra_command(&["rev-parse", "HEAD"], repo.path());
    let head_oid = stdout(&head).trim().to_string();

    assert_eq!(
        run_libra_command(&["replace", "HEAD", "HEAD~1"], repo.path())
            .status
            .code(),
        Some(0)
    );
    let listed = run_libra_command(&["replace", "-l"], repo.path());
    assert_eq!(listed.status.code(), Some(0));
    assert!(
        stdout(&listed).contains(&head_oid),
        "list should contain {head_oid}: {}",
        stdout(&listed)
    );
}

#[test]
fn replace_delete_restores_the_original() {
    let repo = two_commit_repo();
    assert_eq!(
        run_libra_command(&["replace", "HEAD", "HEAD~1"], repo.path())
            .status
            .code(),
        Some(0)
    );
    let deleted = run_libra_command(&["replace", "-d", "HEAD"], repo.path());
    assert_eq!(deleted.status.code(), Some(0));

    // With the replacement gone, log reads "second" again.
    let after = run_libra_command(&["log", "-1"], repo.path());
    assert!(
        stdout(&after).contains("second"),
        "delete did not restore: {}",
        stdout(&after)
    );
}

#[test]
fn replace_existing_without_force_is_rejected() {
    let repo = two_commit_repo();
    assert_eq!(
        run_libra_command(&["replace", "HEAD", "HEAD~1"], repo.path())
            .status
            .code(),
        Some(0)
    );
    let again = run_libra_command(&["replace", "HEAD", "HEAD~1"], repo.path());
    assert_eq!(again.status.code(), Some(128), "overwrite should need -f");
    let forced = run_libra_command(&["replace", "-f", "HEAD", "HEAD~1"], repo.path());
    assert_eq!(forced.status.code(), Some(0), "-f should overwrite");
}

#[test]
fn replace_delete_missing_is_an_error() {
    let repo = two_commit_repo();
    let result = run_libra_command(&["replace", "-d", "HEAD"], repo.path());
    assert_eq!(result.status.code(), Some(128));
}

#[test]
fn replace_bad_object_is_an_error() {
    let repo = two_commit_repo();
    let result = run_libra_command(&["replace", "no-such-rev", "HEAD"], repo.path());
    assert_eq!(result.status.code(), Some(128));
}

#[test]
fn replace_outside_repository_is_an_error() {
    let dir = tempdir().unwrap();
    let result = run_libra_command(&["replace", "-l"], dir.path());
    assert_eq!(result.status.code(), Some(128));
}
