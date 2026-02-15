//! Tests worktree subcommands for core success paths and important error branches.

use std::fs;

use libra::{
    exec_async,
    utils::{test, util},
};
use serde::{Deserialize, Serialize};
use serial_test::serial;
use tempfile::tempdir;

use super::*;

#[derive(Clone, Deserialize, Serialize)]
struct TestWorktreeEntry {
    path: String,
    is_main: bool,
    locked: bool,
    lock_reason: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct TestWorktreeState {
    worktrees: Vec<TestWorktreeEntry>,
}

fn read_worktree_state() -> TestWorktreeState {
    let state_path = util::storage_path().join("worktrees.json");
    let data = fs::read_to_string(state_path).expect("worktrees.json should exist");
    serde_json::from_str(&data).expect("worktrees.json should be valid JSON")
}

fn worktree_paths() -> Vec<String> {
    read_worktree_state()
        .worktrees
        .into_iter()
        .map(|w| w.path)
        .collect()
}

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

#[tokio::test]
#[serial]
async fn test_worktree_add_does_not_reset_index() {
    let repo_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(repo_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(repo_dir.path());

    test::ensure_file("tracked.txt", Some("v1"));
    add::execute(AddArgs {
        pathspec: vec!["tracked.txt".to_string()],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
    })
    .await;

    exec_async(vec!["commit", "-m", "initial"])
        .await
        .expect("initial commit should succeed");

    test::ensure_file("tracked.txt", Some("v2"));
    add::execute(AddArgs {
        pathspec: vec!["tracked.txt".to_string()],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
    })
    .await;

    let staged_before = changes_to_be_committed().await;
    assert!(
        staged_before
            .modified
            .iter()
            .any(|p| p.to_str().unwrap() == "tracked.txt"),
        "tracked.txt should be staged before worktree add"
    );

    exec_async(vec!["worktree", "add", "wt_index"])
        .await
        .expect("worktree add should succeed even when index has staged changes");

    let staged_after = changes_to_be_committed().await;
    assert!(
        staged_after
            .modified
            .iter()
            .any(|p| p.to_str().unwrap() == "tracked.txt"),
        "tracked.txt should remain staged after worktree add"
    );
}

#[tokio::test]
#[serial]
async fn test_worktree_list_includes_main_and_added_worktrees() {
    let repo_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(repo_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(repo_dir.path());

    exec_async(vec!["worktree", "add", "wt_list"])
        .await
        .expect("worktree add should succeed");

    let before_paths = worktree_paths();
    assert!(
        before_paths.iter().any(|p| p.ends_with("wt_list")),
        "state should contain the added worktree"
    );

    exec_async(vec!["worktree", "list"])
        .await
        .expect("worktree list should succeed");

    let after_paths = worktree_paths();
    assert_eq!(
        before_paths.len(),
        after_paths.len(),
        "worktree list should not mutate state"
    );
}

#[tokio::test]
#[serial]
async fn test_worktree_move_moves_unlocked_non_main_worktree() {
    let repo_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(repo_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(repo_dir.path());

    let src = repo_dir.path().join("wt_move_src");
    let dest = repo_dir.path().join("wt_move_dest");
    exec_async(vec!["worktree", "add", "wt_move_src"])
        .await
        .expect("worktree add should succeed");

    assert!(
        src.is_dir(),
        "source directory should exist after worktree add"
    );
    assert!(!dest.exists());

    let src_canonical = src.canonicalize().unwrap();

    exec_async(vec!["worktree", "move", "wt_move_src", "wt_move_dest"])
        .await
        .expect("worktree move should succeed");

    assert!(!src.exists(), "source directory should be moved away");
    assert!(dest.is_dir(), "destination directory should be created");

    let dest_canonical = dest.canonicalize().unwrap();
    let paths = worktree_paths();
    assert!(
        paths
            .iter()
            .any(|p| p == dest_canonical.to_string_lossy().as_ref()),
        "state should contain moved worktree path"
    );
    assert!(
        !paths
            .iter()
            .any(|p| p == src_canonical.to_string_lossy().as_ref()),
        "state should not contain old worktree path"
    );
}

#[tokio::test]
#[serial]
async fn test_worktree_move_main_is_rejected_without_side_effects() {
    let repo_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(repo_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(repo_dir.path());

    exec_async(vec!["worktree", "list"])
        .await
        .expect("worktree list should initialize worktree state");

    let before_paths = worktree_paths();
    let main_path = repo_dir.path().canonicalize().unwrap();
    assert!(
        before_paths
            .iter()
            .any(|p| p == main_path.to_string_lossy().as_ref()),
        "state should contain main worktree entry"
    );

    let dest = repo_dir.path().join("moved_main");
    assert!(!dest.exists());

    exec_async(vec!["worktree", "move", ".", "moved_main"])
        .await
        .expect("worktree move command itself should not fail");

    assert!(
        !dest.exists(),
        "moving main worktree should not create destination directory"
    );

    let after_paths = worktree_paths();
    assert!(
        after_paths
            .iter()
            .any(|p| p == main_path.to_string_lossy().as_ref()),
        "main worktree should still be present after failed move"
    );
    assert!(
        !after_paths
            .iter()
            .any(|p| p == dest.to_string_lossy().as_ref()),
        "failed move should not register destination as worktree"
    );
}

#[tokio::test]
#[serial]
async fn test_worktree_move_locked_is_rejected_without_side_effects() {
    let repo_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(repo_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(repo_dir.path());

    let src = repo_dir.path().join("wt_locked");
    let dest = repo_dir.path().join("wt_locked_moved");
    exec_async(vec!["worktree", "add", "wt_locked"])
        .await
        .expect("worktree add should succeed");

    exec_async(vec!["worktree", "lock", "wt_locked"])
        .await
        .expect("worktree lock should succeed");

    assert!(src.is_dir());
    assert!(!dest.exists());

    let src_canonical = src.canonicalize().unwrap();

    exec_async(vec!["worktree", "move", "wt_locked", "wt_locked_moved"])
        .await
        .expect("worktree move command itself should not fail");

    assert!(
        src.is_dir(),
        "locked worktree directory should remain at original location"
    );
    assert!(
        !dest.exists(),
        "locked worktree move should not create destination directory"
    );

    let state = read_worktree_state();
    let locked_entry = state
        .worktrees
        .into_iter()
        .find(|w| w.path == src_canonical.to_string_lossy())
        .expect("locked worktree entry should still exist");
    assert!(locked_entry.locked, "worktree should remain locked");
}

#[tokio::test]
#[serial]
async fn test_worktree_move_rejects_duplicate_destination() {
    let repo_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(repo_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(repo_dir.path());

    exec_async(vec!["worktree", "add", "wt_a"])
        .await
        .expect("first worktree add should succeed");
    exec_async(vec!["worktree", "add", "wt_b"])
        .await
        .expect("second worktree add should succeed");

    let src = repo_dir.path().join("wt_a");
    let dest = repo_dir.path().join("wt_b");
    assert!(src.is_dir());
    assert!(dest.is_dir());

    let before_paths = worktree_paths();

    exec_async(vec!["worktree", "move", "wt_a", "wt_b"])
        .await
        .expect("worktree move command itself should not fail");

    assert!(
        src.is_dir(),
        "move to existing worktree should keep source directory"
    );
    assert!(dest.is_dir(), "destination directory should remain");

    let after_paths = worktree_paths();
    assert_eq!(
        before_paths.len(),
        after_paths.len(),
        "duplicate-destination move should not change number of registered worktrees"
    );
}

#[tokio::test]
#[serial]
async fn test_worktree_prune_removes_missing_non_main_worktrees() {
    let repo_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(repo_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(repo_dir.path());

    exec_async(vec!["worktree", "add", "wt_prune"])
        .await
        .expect("worktree add should succeed");

    let wt_path = repo_dir.path().join("wt_prune");
    assert!(wt_path.is_dir());

    fs::remove_dir_all(&wt_path).expect("failed to remove worktree directory");
    assert!(
        !wt_path.exists(),
        "worktree directory should be removed before prune"
    );

    let before_paths = worktree_paths();

    exec_async(vec!["worktree", "prune"])
        .await
        .expect("worktree prune should succeed");

    let after_paths = worktree_paths();
    assert!(
        after_paths.len() < before_paths.len(),
        "prune should remove missing non-main worktrees"
    );
}

#[tokio::test]
#[serial]
async fn test_worktree_remove_locked_is_rejected_without_side_effects() {
    let repo_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(repo_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(repo_dir.path());

    exec_async(vec!["worktree", "add", "wt_for_remove"])
        .await
        .expect("worktree add should succeed");
    exec_async(vec!["worktree", "lock", "wt_for_remove"])
        .await
        .expect("worktree lock should succeed");

    let wt_path = repo_dir.path().join("wt_for_remove");
    assert!(wt_path.is_dir());

    let before_paths = worktree_paths();

    exec_async(vec!["worktree", "remove", "wt_for_remove"])
        .await
        .expect("worktree remove command itself should not fail");

    assert!(
        wt_path.is_dir(),
        "locked worktree directory should still exist after failed remove"
    );

    let after_paths = worktree_paths();
    assert_eq!(
        before_paths.len(),
        after_paths.len(),
        "removing locked worktree should not change number of registered worktrees"
    );
}

#[tokio::test]
#[serial]
async fn test_worktree_repair_deduplicates_entries() {
    let repo_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(repo_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(repo_dir.path());

    exec_async(vec!["worktree", "add", "wt_repair"])
        .await
        .expect("worktree add should succeed");

    let mut state = read_worktree_state();
    let duplicate = state
        .worktrees
        .iter()
        .find(|w| w.path.ends_with("wt_repair"))
        .cloned()
        .expect("expected worktree entry for wt_repair");
    state.worktrees.push(duplicate);

    let state_path = util::storage_path().join("worktrees.json");
    let data = serde_json::to_string_pretty(&state)
        .expect("failed to serialize duplicated worktree state");
    fs::write(&state_path, data).expect("failed to overwrite worktrees.json with duplicates");

    exec_async(vec!["worktree", "repair"])
        .await
        .expect("worktree repair should succeed");

    let repaired = read_worktree_state();
    let paths: Vec<String> = repaired.worktrees.iter().map(|w| w.path.clone()).collect();
    let unique_paths = paths
        .iter()
        .cloned()
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(
        unique_paths.len(),
        paths.len(),
        "repair should remove duplicate worktree entries"
    );
}

#[tokio::test]
#[serial]
async fn test_worktree_main_flag_remains_single_and_stable() {
    let repo_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(repo_dir.path()).await;

    let _guard = test::ChangeDirGuard::new(repo_dir.path());
    exec_async(vec!["worktree", "add", "wt_main"])
        .await
        .expect("worktree add should succeed");

    let repo_main = repo_dir.path().canonicalize().unwrap();

    let wt_path = repo_dir.path().join("wt_main");
    let _guard_wt = test::ChangeDirGuard::new(&wt_path);
    exec_async(vec!["worktree", "list"])
        .await
        .expect("worktree list from linked worktree should succeed");

    let state = read_worktree_state();
    let main_entries: Vec<_> = state.worktrees.iter().filter(|w| w.is_main).collect();
    assert_eq!(
        main_entries.len(),
        1,
        "there should be exactly one main worktree entry"
    );
    assert_eq!(
        main_entries[0].path,
        repo_main.to_string_lossy(),
        "main worktree entry should remain the original repo directory"
    );
}
