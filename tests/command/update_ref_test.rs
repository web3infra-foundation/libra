//! Integration tests for `libra update-ref`.
//!
//! Layer: L1 (deterministic; tempdir + isolated HOME, no network).

use std::{fs, process::Output};

use tempfile::TempDir;

use super::{create_committed_repo_via_cli, parse_json_stdout, run_libra_command};

const ZERO_SHA1: &str = "0000000000000000000000000000000000000000";

fn stdout_trimmed(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn rev_parse(repo: &TempDir, rev: &str) -> Output {
    run_libra_command(&["rev-parse", rev], repo.path())
}

/// A repo with two distinct commits; returns `(repo, first_oid, second_oid)`.
fn repo_with_two_commits() -> (TempDir, String, String) {
    let repo = create_committed_repo_via_cli();
    let c1 = stdout_trimmed(&rev_parse(&repo, "HEAD"));

    fs::write(repo.path().join("tracked.txt"), "second\n").unwrap();
    let add = run_libra_command(&["add", "tracked.txt"], repo.path());
    assert!(add.status.success());
    let commit = run_libra_command(&["commit", "-m", "second", "--no-verify"], repo.path());
    assert!(
        commit.status.success(),
        "second commit failed: {}",
        String::from_utf8_lossy(&commit.stderr)
    );
    let c2 = stdout_trimmed(&rev_parse(&repo, "HEAD"));
    assert_ne!(c1, c2, "expected two distinct commits");
    (repo, c1, c2)
}

#[test]
fn creates_a_new_branch_ref() {
    let (repo, c1, _c2) = repo_with_two_commits();
    let out = run_libra_command(&["update-ref", "refs/heads/feature", &c1], repo.path());
    assert_eq!(
        out.status.code(),
        Some(0),
        "create failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(stdout_trimmed(&rev_parse(&repo, "feature")), c1);
}

#[test]
fn updates_an_existing_branch_ref() {
    let (repo, c1, c2) = repo_with_two_commits();
    run_libra_command(&["update-ref", "refs/heads/feature", &c1], repo.path());
    let out = run_libra_command(&["update-ref", "refs/heads/feature", &c2], repo.path());
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout_trimmed(&rev_parse(&repo, "feature")), c2);
}

#[test]
fn compare_and_swap_succeeds_when_old_matches() {
    let (repo, c1, c2) = repo_with_two_commits();
    run_libra_command(&["update-ref", "refs/heads/feature", &c1], repo.path());
    let out = run_libra_command(&["update-ref", "refs/heads/feature", &c2, &c1], repo.path());
    assert_eq!(
        out.status.code(),
        Some(0),
        "CAS should succeed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(stdout_trimmed(&rev_parse(&repo, "feature")), c2);
}

#[test]
fn compare_and_swap_fails_when_old_mismatches() {
    let (repo, c1, c2) = repo_with_two_commits();
    run_libra_command(&["update-ref", "refs/heads/feature", &c1], repo.path());
    // Current is c1, but we claim it is c2.
    let out = run_libra_command(&["update-ref", "refs/heads/feature", &c2, &c2], repo.path());
    assert_eq!(out.status.code(), Some(128), "CAS mismatch must fail");
    // The ref is unchanged.
    assert_eq!(stdout_trimmed(&rev_parse(&repo, "feature")), c1);
}

#[test]
fn zero_old_value_creates_only_when_absent() {
    let (repo, c1, _c2) = repo_with_two_commits();
    let create = run_libra_command(
        &["update-ref", "refs/heads/fresh", &c1, ZERO_SHA1],
        repo.path(),
    );
    assert_eq!(
        create.status.code(),
        Some(0),
        "create-only should succeed when absent: {}",
        String::from_utf8_lossy(&create.stderr)
    );
    // Now it exists; a second create-only must fail.
    let again = run_libra_command(
        &["update-ref", "refs/heads/fresh", &c1, ZERO_SHA1],
        repo.path(),
    );
    assert_eq!(
        again.status.code(),
        Some(128),
        "create-only must fail when present"
    );
}

#[test]
fn deletes_a_branch_ref() {
    let (repo, c1, _c2) = repo_with_two_commits();
    run_libra_command(&["update-ref", "refs/heads/feature", &c1], repo.path());
    let del = run_libra_command(&["update-ref", "-d", "refs/heads/feature"], repo.path());
    assert_eq!(
        del.status.code(),
        Some(0),
        "delete failed: {}",
        String::from_utf8_lossy(&del.stderr)
    );
    assert_ne!(
        rev_parse(&repo, "feature").status.code(),
        Some(0),
        "deleted ref should no longer resolve"
    );
}

#[test]
fn delete_with_mismatched_old_fails() {
    let (repo, c1, c2) = repo_with_two_commits();
    run_libra_command(&["update-ref", "refs/heads/feature", &c1], repo.path());
    let del = run_libra_command(
        &["update-ref", "-d", "refs/heads/feature", &c2],
        repo.path(),
    );
    assert_eq!(
        del.status.code(),
        Some(128),
        "delete CAS mismatch must fail"
    );
    // Still present.
    assert_eq!(stdout_trimmed(&rev_parse(&repo, "feature")), c1);
}

#[test]
fn deleting_a_missing_ref_fails() {
    let (repo, _c1, _c2) = repo_with_two_commits();
    let del = run_libra_command(&["update-ref", "-d", "refs/heads/ghost"], repo.path());
    assert_eq!(del.status.code(), Some(128));
}

#[test]
fn rejects_head() {
    let (repo, c1, _c2) = repo_with_two_commits();
    let out = run_libra_command(&["update-ref", "HEAD", &c1], repo.path());
    assert_eq!(out.status.code(), Some(128), "HEAD must be rejected");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("HEAD"),
        "error should mention HEAD: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn rejects_non_heads_namespace() {
    let (repo, c1, _c2) = repo_with_two_commits();
    let out = run_libra_command(&["update-ref", "refs/tags/v1", &c1], repo.path());
    assert_eq!(
        out.status.code(),
        Some(128),
        "refs/tags/* must be rejected in v1"
    );
}

#[test]
fn rejects_invalid_object_id() {
    let (repo, _c1, _c2) = repo_with_two_commits();
    let out = run_libra_command(
        &["update-ref", "refs/heads/feature", "deadbeef"],
        repo.path(),
    );
    assert_eq!(out.status.code(), Some(128));
}

#[test]
fn rejects_nonexistent_new_object() {
    let (repo, _c1, _c2) = repo_with_two_commits();
    // A syntactically valid id that is not present in the object store: Git's
    // update-ref refuses to create such a dangling ref.
    let ghost = "a".repeat(40);
    let out = run_libra_command(&["update-ref", "refs/heads/feature", &ghost], repo.path());
    assert_eq!(
        out.status.code(),
        Some(128),
        "update-ref to a nonexistent object must fail: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn rejects_symbolic_ref_value() {
    let (repo, _c1, _c2) = repo_with_two_commits();
    let out = run_libra_command(
        &["update-ref", "refs/heads/feature", "ref:refs/heads/main"],
        repo.path(),
    );
    assert_eq!(out.status.code(), Some(128), "ref: values must be rejected");
}

#[test]
fn json_output_reports_old_and_new() {
    let (repo, c1, c2) = repo_with_two_commits();
    run_libra_command(&["update-ref", "refs/heads/feature", &c1], repo.path());
    let out = run_libra_command(
        &["--json", "update-ref", "refs/heads/feature", &c2],
        repo.path(),
    );
    assert_eq!(out.status.code(), Some(0));
    let json = parse_json_stdout(&out);
    assert_eq!(json["data"]["ref"].as_str(), Some("refs/heads/feature"));
    assert_eq!(json["data"]["old"].as_str(), Some(c1.as_str()));
    assert_eq!(json["data"]["new"].as_str(), Some(c2.as_str()));
    assert_eq!(json["data"]["deleted"].as_bool(), Some(false));
}

#[test]
fn outside_repository_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_libra_command(&["update-ref", "refs/heads/x", ZERO_SHA1], dir.path());
    assert_eq!(out.status.code(), Some(128));
}
