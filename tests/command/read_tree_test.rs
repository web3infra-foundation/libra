//! Integration tests for `libra read-tree`.
//!
//! Layer: L1 (deterministic; tempdir + isolated HOME, no network).

use std::fs;

use tempfile::tempdir;

use super::{create_committed_repo_via_cli, run_libra_command};

fn stdout_trimmed(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
fn read_tree_head_replaces_the_index() {
    let repo = create_committed_repo_via_cli();

    // The committed index's tree.
    let head_tree = run_libra_command(&["write-tree"], repo.path());
    assert_eq!(head_tree.status.code(), Some(0));
    let head_tree = stdout_trimmed(&head_tree);

    // Dirty the index with a new staged file.
    fs::write(repo.path().join("extra.txt"), "extra").unwrap();
    let add = run_libra_command(&["add", "extra.txt"], repo.path());
    assert!(
        add.status.success(),
        "{}",
        String::from_utf8_lossy(&add.stderr)
    );
    let dirty = stdout_trimmed(&run_libra_command(&["write-tree"], repo.path()));
    assert_ne!(dirty, head_tree, "staging a file changes the index tree");

    // read-tree HEAD resets the index back to HEAD's tree.
    let read = run_libra_command(&["read-tree", "HEAD"], repo.path());
    assert_eq!(
        read.status.code(),
        Some(0),
        "read-tree HEAD failed: {}",
        String::from_utf8_lossy(&read.stderr)
    );
    let restored = stdout_trimmed(&run_libra_command(&["write-tree"], repo.path()));
    assert_eq!(
        restored, head_tree,
        "read-tree HEAD restores the committed tree"
    );
}

#[test]
fn read_tree_by_explicit_tree_id() {
    let repo = create_committed_repo_via_cli();
    let tree_id = stdout_trimmed(&run_libra_command(&["write-tree"], repo.path()));

    // Stage a change, then read the captured tree id back.
    fs::write(repo.path().join("more.txt"), "more").unwrap();
    run_libra_command(&["add", "more.txt"], repo.path());
    let read = run_libra_command(&["read-tree", &tree_id], repo.path());
    assert_eq!(
        read.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&read.stderr)
    );
    assert_eq!(
        stdout_trimmed(&run_libra_command(&["write-tree"], repo.path())),
        tree_id,
        "reading a tree id restores that exact tree"
    );
}

#[test]
fn read_tree_json_reports_tree_and_entry_count() {
    let repo = create_committed_repo_via_cli();
    let out = run_libra_command(&["--json", "read-tree", "HEAD"], repo.path());
    assert_eq!(out.status.code(), Some(0));
    let json = super::parse_json_stdout(&out);
    assert_eq!(json["data"]["tree"].as_str().map(str::len), Some(40));
    assert!(
        json["data"]["entries"].as_u64().is_some_and(|n| n >= 1),
        "HEAD has at least one entry: {json}"
    );
}

#[test]
fn read_tree_invalid_treeish_is_an_error() {
    let repo = create_committed_repo_via_cli();
    let out = run_libra_command(&["read-tree", "no-such-ref"], repo.path());
    assert_eq!(
        out.status.code(),
        Some(128),
        "an invalid tree-ish is fatal: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn read_tree_missing_argument_is_a_usage_error() {
    let repo = create_committed_repo_via_cli();
    let out = run_libra_command(&["read-tree"], repo.path());
    assert!(
        !out.status.success(),
        "read-tree requires a tree-ish argument"
    );
}

#[test]
fn read_tree_outside_repository_is_an_error() {
    let dir = tempdir().expect("tempdir");
    let out = run_libra_command(&["read-tree", "HEAD"], dir.path());
    assert_eq!(
        out.status.code(),
        Some(128),
        "read-tree outside a repository is fatal: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
