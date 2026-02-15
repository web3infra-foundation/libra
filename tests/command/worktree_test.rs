//! Tests worktree subcommands for basic add/list/lock/unlock/remove flows.

use std::fs;

use libra::{exec_async, utils::test};
use serial_test::serial;
use tempfile::tempdir;

#[tokio::test]
#[serial]
async fn test_worktree_add_creates_linked_directory() {
    let repo_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(repo_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(repo_dir.path());

    let result = exec_async(vec!["worktree", "add", "wt1"]).await;
    assert!(result.is_ok(), "worktree add failed: {:?}", result.err());

    let wt_path = repo_dir.path().join("wt1");
    assert!(wt_path.is_dir(), "worktree directory should exist");

    let link = wt_path.join(".libra");
    assert!(link.is_file(), ".libra link file should exist in worktree");

    let content = fs::read_to_string(&link).unwrap();
    assert!(
        content.trim_start().starts_with("gitdir:"),
        "link file should start with gitdir:, got: {}",
        content
    );
}

#[tokio::test]
#[serial]
async fn test_worktree_lock_unlock_and_remove() {
    let repo_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(repo_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(repo_dir.path());

    exec_async(vec!["worktree", "add", "wt2"])
        .await
        .expect("worktree add should succeed");

    exec_async(vec!["worktree", "lock", "wt2"])
        .await
        .expect("worktree lock should succeed");

    exec_async(vec!["worktree", "unlock", "wt2"])
        .await
        .expect("worktree unlock should succeed");

    exec_async(vec!["worktree", "remove", "wt2"])
        .await
        .expect("worktree remove should succeed");
}
