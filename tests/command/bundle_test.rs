//! Integration tests for `libra bundle`.
//!
//! Layer: L1 (deterministic; tempdir + isolated HOME, no network).

use std::fs;

use tempfile::tempdir;

use super::{create_committed_repo_via_cli, run_libra_command};

#[test]
fn bundle_create_writes_a_v2_bundle() {
    let repo = create_committed_repo_via_cli();
    let path = repo.path().join("out.bundle");
    let result = run_libra_command(
        &["bundle", "create", path.to_str().unwrap(), "HEAD"],
        repo.path(),
    );
    assert_eq!(
        result.status.code(),
        Some(0),
        "bundle create failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let bytes = fs::read(&path).unwrap();
    assert!(
        bytes.starts_with(b"# v2 git bundle\n"),
        "missing v2 signature"
    );
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("refs/heads/"), "missing a head ref line");
    // The pack follows the blank line that terminates the header.
    assert!(
        bytes.windows(6).any(|w| w == b"\n\nPACK"),
        "missing PACK after header"
    );
}

#[test]
fn bundle_list_heads_prints_refs() {
    let repo = create_committed_repo_via_cli();
    let path = repo.path().join("out.bundle");
    assert_eq!(
        run_libra_command(
            &["bundle", "create", path.to_str().unwrap(), "HEAD"],
            repo.path()
        )
        .status
        .code(),
        Some(0)
    );
    let result = run_libra_command(
        &["bundle", "list-heads", path.to_str().unwrap()],
        repo.path(),
    );
    assert_eq!(result.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&result.stdout).contains("refs/heads/"));
}

#[test]
fn bundle_verify_accepts_a_created_bundle() {
    let repo = create_committed_repo_via_cli();
    let path = repo.path().join("out.bundle");
    assert_eq!(
        run_libra_command(
            &["bundle", "create", path.to_str().unwrap(), "HEAD"],
            repo.path()
        )
        .status
        .code(),
        Some(0)
    );
    let result = run_libra_command(&["bundle", "verify", path.to_str().unwrap()], repo.path());
    assert_eq!(
        result.status.code(),
        Some(0),
        "verify failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("is okay"));
}

#[test]
fn bundle_verify_rejects_a_non_bundle() {
    let repo = create_committed_repo_via_cli();
    let path = repo.path().join("not.bundle");
    fs::write(&path, b"this is not a bundle\n").unwrap();
    let result = run_libra_command(&["bundle", "verify", path.to_str().unwrap()], repo.path());
    // An invalid bundle format is a verification failure (exit 1), like
    // `git bundle verify`; exit 128 is reserved for usage errors.
    assert_eq!(result.status.code(), Some(1));
}

#[test]
fn bundle_create_bad_rev_is_an_error() {
    let repo = create_committed_repo_via_cli();
    let path = repo.path().join("out.bundle");
    let result = run_libra_command(
        &["bundle", "create", path.to_str().unwrap(), "no-such-rev"],
        repo.path(),
    );
    assert_eq!(result.status.code(), Some(128));
    assert!(!path.exists(), "no half-written bundle should remain");
}

#[test]
fn bundle_outside_repository_is_an_error() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("out.bundle");
    let result = run_libra_command(
        &["bundle", "create", path.to_str().unwrap(), "HEAD"],
        dir.path(),
    );
    assert_eq!(result.status.code(), Some(128));
}
