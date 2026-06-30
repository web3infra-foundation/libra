//! Integration tests for `libra rerere`.
//!
//! Layer: L1 (deterministic; tempdir + isolated HOME, no network).

use std::{fs, process::Output};

use tempfile::{TempDir, tempdir};

use super::{assert_cli_success, create_committed_repo_via_cli, run_libra_command};

const CONFLICT: &str = "line1\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> other\nline3\n";
const RESOLVED: &str = "line1\nRESOLVED\nline3\n";

fn out(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

/// A committed repo with `tracked.txt` overwritten in the working tree with a
/// conflict (the file stays tracked, which is what `rerere` keys on).
fn repo_with_conflict() -> TempDir {
    let repo = create_committed_repo_via_cli();
    fs::write(repo.path().join("tracked.txt"), CONFLICT).unwrap();
    repo
}

#[test]
fn rerere_records_resolves_and_replays() {
    let repo = repo_with_conflict();
    let file = repo.path().join("tracked.txt");

    // 1. Record the preimage.
    assert_cli_success(&run_libra_command(&["rerere"], repo.path()), "record");
    let status = run_libra_command(&["rerere", "status"], repo.path());
    assert!(
        out(&status).contains("tracked.txt"),
        "status should list the tracked conflict: {}",
        out(&status)
    );

    // 2. Resolve it and let rerere record the postimage.
    fs::write(&file, RESOLVED).unwrap();
    assert_cli_success(
        &run_libra_command(&["rerere"], repo.path()),
        "record resolution",
    );

    // 3. The same conflict reappears; rerere must replay the resolution.
    fs::write(&file, CONFLICT).unwrap();
    assert_cli_success(&run_libra_command(&["rerere"], repo.path()), "replay");
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        RESOLVED,
        "rerere should have replayed the recorded resolution"
    );
}

#[test]
fn rerere_forget_drops_the_recording() {
    let repo = repo_with_conflict();
    run_libra_command(&["rerere"], repo.path());
    let forget = run_libra_command(&["rerere", "forget", "tracked.txt"], repo.path());
    assert_eq!(forget.status.code(), Some(0));
    let status = run_libra_command(&["rerere", "status"], repo.path());
    assert!(
        !out(&status).contains("tracked.txt"),
        "forget should remove the tracked conflict: {}",
        out(&status)
    );
}

#[test]
fn rerere_forget_unknown_path_is_an_error() {
    let repo = repo_with_conflict();
    run_libra_command(&["rerere"], repo.path());
    let forget = run_libra_command(&["rerere", "forget", "nope.txt"], repo.path());
    assert_eq!(forget.status.code(), Some(128));
}

#[test]
fn rerere_clear_stops_tracking() {
    let repo = repo_with_conflict();
    run_libra_command(&["rerere"], repo.path());
    let clear = run_libra_command(&["rerere", "clear"], repo.path());
    assert_eq!(clear.status.code(), Some(0));
    let status = run_libra_command(&["rerere", "status"], repo.path());
    assert!(out(&status).trim().is_empty(), "clear should empty status");
}

#[test]
fn rerere_diff_shows_changes_since_preimage() {
    let repo = repo_with_conflict();
    run_libra_command(&["rerere"], repo.path());
    // Edit the conflicted file, then diff against the recorded preimage.
    fs::write(repo.path().join("tracked.txt"), RESOLVED).unwrap();
    let diff = run_libra_command(&["rerere", "diff"], repo.path());
    assert_eq!(diff.status.code(), Some(0));
    assert!(
        out(&diff).contains("RESOLVED") || out(&diff).contains("tracked.txt"),
        "diff should show the change: {}",
        out(&diff)
    );
}

#[test]
fn rerere_gc_is_a_noop_for_fresh_entries() {
    let repo = repo_with_conflict();
    run_libra_command(&["rerere"], repo.path());
    let gc = run_libra_command(&["rerere", "gc"], repo.path());
    assert_eq!(gc.status.code(), Some(0));
    // The fresh (unresolved) entry is well under the TTL, so it survives.
    let status = run_libra_command(&["rerere", "status"], repo.path());
    assert!(out(&status).contains("tracked.txt"));
}

#[test]
fn rerere_outside_repository_is_an_error() {
    let dir = tempdir().unwrap();
    let out = run_libra_command(&["rerere", "status"], dir.path());
    assert_eq!(out.status.code(), Some(128));
}
