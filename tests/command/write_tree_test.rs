//! Integration tests for `libra write-tree` (and its `read-tree` round-trip).
//!
//! Layer: L1 (deterministic; tempdir + isolated HOME, no network).

use std::fs;

use tempfile::tempdir;

use super::{parse_json_stdout, run_libra_command};

/// The canonical empty tree object id for a SHA-1 repository.
const EMPTY_TREE_SHA1: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

fn init_repo() -> tempfile::TempDir {
    let repo = tempdir().expect("tempdir");
    let init = run_libra_command(&["init"], repo.path());
    assert!(
        init.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );
    repo
}

fn stdout_trimmed(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
fn empty_index_writes_the_empty_tree() {
    let repo = init_repo();
    let out = run_libra_command(&["write-tree"], repo.path());
    assert_eq!(
        out.status.code(),
        Some(0),
        "write-tree on an empty index: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(stdout_trimmed(&out), EMPTY_TREE_SHA1);
}

#[test]
fn write_tree_builds_nested_directories() {
    let repo = init_repo();
    // `a/b/c.txt` has no sibling file directly in `a` or `a/b` — the case the
    // old per-command tree builders dropped. The shared builder must keep it.
    fs::create_dir_all(repo.path().join("a/b")).unwrap();
    fs::write(repo.path().join("top.txt"), "top").unwrap();
    fs::write(repo.path().join("a/b/c.txt"), "deep").unwrap();
    let add = run_libra_command(&["add", "."], repo.path());
    assert!(
        add.status.success(),
        "add failed: {}",
        String::from_utf8_lossy(&add.stderr)
    );

    let tree = run_libra_command(&["write-tree"], repo.path());
    assert_eq!(tree.status.code(), Some(0));
    let root = stdout_trimmed(&tree);
    assert_eq!(root.len(), 40, "SHA-1 tree id: {root}");
    assert_ne!(
        root, EMPTY_TREE_SHA1,
        "a populated index is not the empty tree"
    );

    // Round-trip: read the tree back into a fresh index and re-write it — a
    // correct nested build is stable, and the deep file must survive.
    fs::write(repo.path().join("scratch.txt"), "dirty the index").unwrap();
    run_libra_command(&["add", "scratch.txt"], repo.path());
    let read = run_libra_command(&["read-tree", &root], repo.path());
    assert_eq!(
        read.status.code(),
        Some(0),
        "read-tree failed: {}",
        String::from_utf8_lossy(&read.stderr)
    );
    let rewritten = run_libra_command(&["write-tree"], repo.path());
    assert_eq!(
        stdout_trimmed(&rewritten),
        root,
        "read-tree then write-tree must reproduce the original tree id"
    );
}

#[test]
fn write_tree_json_reports_the_tree_id() {
    let repo = init_repo();
    fs::write(repo.path().join("f.txt"), "x").unwrap();
    run_libra_command(&["add", "f.txt"], repo.path());
    let out = run_libra_command(&["--json", "write-tree"], repo.path());
    assert_eq!(out.status.code(), Some(0));
    let json = parse_json_stdout(&out);
    let tree = json["data"]["tree"].as_str().expect("tree field");
    assert_eq!(tree.len(), 40, "SHA-1 tree id in JSON: {tree}");
}

#[test]
fn write_tree_outside_repository_is_an_error() {
    let dir = tempdir().expect("tempdir");
    let out = run_libra_command(&["write-tree"], dir.path());
    assert_eq!(
        out.status.code(),
        Some(128),
        "write-tree outside a repository is fatal: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
