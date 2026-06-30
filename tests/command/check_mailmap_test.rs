//! Integration tests for `libra check-mailmap`.
//!
//! Layer: L1 (deterministic; tempdir + isolated HOME, no network).

use std::{fs, process::Output};

use tempfile::{TempDir, tempdir};

use super::{parse_json_stdout, run_libra_command, run_libra_command_with_stdin};

fn init_repo_with_mailmap(mailmap: &str) -> TempDir {
    let repo = tempdir().unwrap();
    assert!(run_libra_command(&["init"], repo.path()).status.success());
    fs::write(repo.path().join(".mailmap"), mailmap).unwrap();
    repo
}

fn out(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
fn check_mailmap_resolves_commit_email() {
    let repo = init_repo_with_mailmap("Proper Name <proper@x.com> <commit@x.com>\n");
    let result = run_libra_command(&["check-mailmap", "Whoever <commit@x.com>"], repo.path());
    assert_eq!(
        result.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(out(&result), "Proper Name <proper@x.com>");
}

#[test]
fn check_mailmap_unmatched_passes_through() {
    let repo = init_repo_with_mailmap("Proper <p@x> <c@x>\n");
    let result = run_libra_command(&["check-mailmap", "Nobody <nobody@x>"], repo.path());
    assert_eq!(result.status.code(), Some(0));
    assert_eq!(out(&result), "Nobody <nobody@x>");
}

#[test]
fn check_mailmap_reads_stdin() {
    let repo = init_repo_with_mailmap("Proper Name <proper@x.com> <commit@x.com>\n");
    let result = run_libra_command_with_stdin(
        &["check-mailmap", "--stdin"],
        repo.path(),
        "Whoever <commit@x.com>\n",
    );
    assert_eq!(result.status.code(), Some(0));
    assert_eq!(out(&result), "Proper Name <proper@x.com>");
}

#[test]
fn check_mailmap_json_lists_contacts() {
    let repo = init_repo_with_mailmap("Proper <p@x> <c@x>\n");
    let result = run_libra_command(&["--json", "check-mailmap", "Whoever <c@x>"], repo.path());
    assert_eq!(result.status.code(), Some(0));
    let json = parse_json_stdout(&result);
    assert_eq!(json["data"]["contacts"][0].as_str(), Some("Proper <p@x>"));
}

#[test]
fn check_mailmap_no_contacts_is_a_usage_error() {
    let repo = init_repo_with_mailmap("");
    let result = run_libra_command(&["check-mailmap"], repo.path());
    assert_eq!(result.status.code(), Some(128));
}

#[test]
fn check_mailmap_invalid_contact_is_an_error() {
    let repo = init_repo_with_mailmap("");
    let result = run_libra_command(&["check-mailmap", "no-email-here"], repo.path());
    assert_eq!(result.status.code(), Some(128));
}

#[test]
fn check_mailmap_outside_repository_is_an_error() {
    let dir = tempdir().unwrap();
    let result = run_libra_command(&["check-mailmap", "X <x@x>"], dir.path());
    assert_eq!(result.status.code(), Some(128));
}
