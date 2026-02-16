//! Tests worktree subcommands for core success paths and important error branches.

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

use libra::{
    exec_async,
    utils::{test, util},
};
use serde::{Deserialize, Serialize};
use serial_test::serial;
use tempfile::tempdir;

use super::*;

/// Mirror of the on-disk `WorktreeEntry` used only in tests.
///
/// This type allows tests to deserialize `worktrees.json` without depending
/// on internal, non-public structs from the main crate.
#[derive(Clone, Deserialize, Serialize)]
struct TestWorktreeEntry {
    path: String,
    is_main: bool,
    locked: bool,
    lock_reason: Option<String>,
}

/// Mirror of the on-disk `WorktreeState` used only in tests.
#[derive(Deserialize, Serialize)]
struct TestWorktreeState {
    worktrees: Vec<TestWorktreeEntry>,
}

/// Loads the current `worktrees.json` into a test-friendly `TestWorktreeState`.
fn read_worktree_state() -> TestWorktreeState {
    let state_path = util::storage_path().join("worktrees.json");
    let data = fs::read_to_string(state_path).expect("worktrees.json should exist");
    serde_json::from_str(&data).expect("worktrees.json should be valid JSON")
}

/// Returns all worktree paths from the persisted test state.
fn worktree_paths() -> Vec<String> {
    read_worktree_state()
        .worktrees
        .into_iter()
        .map(|w| w.path)
        .collect()
}

#[tokio::test]
#[serial]
/// `worktree add` creates a linked directory with a `.libra` link file.
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
/// `worktree add` stores a stable canonical path even if input uses a missing parent plus `..`.
async fn test_worktree_add_normalizes_missing_parent_with_dotdot() {
    let repo_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(repo_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(repo_dir.path());

    exec_async(vec!["worktree", "add", "missing_parent/../wt_norm"])
        .await
        .expect("worktree add should succeed");

    let expected = repo_dir.path().join("wt_norm").canonicalize().unwrap();
    let state = read_worktree_state();
    let entry = state
        .worktrees
        .iter()
        .find(|w| w.path.ends_with("wt_norm"))
        .expect("state should contain the added worktree");
    assert_eq!(
        entry.path,
        expected.to_string_lossy().as_ref(),
        "stored worktree path should be canonical and normalized"
    );

    exec_async(vec!["worktree", "lock", "wt_norm"])
        .await
        .expect("worktree lock should succeed");
    exec_async(vec!["worktree", "unlock", "wt_norm"])
        .await
        .expect("worktree unlock should succeed");
    exec_async(vec!["worktree", "remove", "wt_norm"])
        .await
        .expect("worktree remove should succeed");
}

#[tokio::test]
#[serial]
/// Adding with `../` must still allow later `lock/unlock/remove .` from inside that worktree.
async fn test_worktree_add_parent_relative_then_operate_with_dot_from_linked_worktree() {
    let root_dir = tempdir().unwrap();
    let repo_path = root_dir.path().join("repo");
    fs::create_dir_all(&repo_path).unwrap();
    test::setup_with_new_libra_in(&repo_path).await;

    let _guard_repo = test::ChangeDirGuard::new(&repo_path);
    exec_async(vec!["worktree", "add", "../wt_lock_dot"])
        .await
        .expect("worktree add with parent-relative path should succeed");

    let linked = root_dir.path().join("wt_lock_dot");
    let _guard_linked = test::ChangeDirGuard::new(&linked);
    exec_async(vec!["worktree", "lock", "."])
        .await
        .expect("worktree lock with '.' should resolve the registered entry");
    exec_async(vec!["worktree", "unlock", "."])
        .await
        .expect("worktree unlock with '.' should resolve the registered entry");
    exec_async(vec!["worktree", "remove", "."])
        .await
        .expect("worktree remove with '.' should resolve the registered entry");
}

#[tokio::test]
#[serial]
/// Adding the same path via `../...` and absolute form should deduplicate to one canonical entry.
async fn test_worktree_add_parent_relative_and_absolute_path_are_equivalent() {
    let root_dir = tempdir().unwrap();
    let repo_path = root_dir.path().join("repo");
    fs::create_dir_all(&repo_path).unwrap();
    test::setup_with_new_libra_in(&repo_path).await;
    let _guard = test::ChangeDirGuard::new(&repo_path);

    exec_async(vec!["worktree", "add", "../wt_rel_abs"])
        .await
        .expect("first worktree add should succeed");

    let abs_target = root_dir.path().join("wt_rel_abs").canonicalize().unwrap();
    let abs_target_str = abs_target.to_string_lossy().to_string();

    exec_async(vec!["worktree", "add", abs_target_str.as_str()])
        .await
        .expect("second worktree add with absolute path should succeed");

    let paths = worktree_paths();
    let matches = paths
        .iter()
        .filter(|p| p.as_str() == abs_target.to_string_lossy().as_ref())
        .count();
    assert_eq!(
        matches, 1,
        "same worktree path should only be registered once"
    );
}

#[cfg(unix)]
#[tokio::test]
#[serial]
/// Symlink inputs should be canonicalized to the real target path.
async fn test_worktree_add_symlink_path_is_canonicalized_to_real_path() {
    let root_dir = tempdir().unwrap();
    let repo_path = root_dir.path().join("repo");
    fs::create_dir_all(&repo_path).unwrap();
    test::setup_with_new_libra_in(&repo_path).await;
    let _guard = test::ChangeDirGuard::new(&repo_path);

    let real_target = root_dir.path().join("wt_real");
    fs::create_dir_all(&real_target).unwrap();
    let symlink_path = repo_path.join("wt_link");
    symlink(&real_target, &symlink_path).expect("failed to create symlink for test");

    exec_async(vec!["worktree", "add", "wt_link"])
        .await
        .expect("worktree add through symlink should succeed");

    let real_canonical = real_target.canonicalize().unwrap();
    let symlink_abs = symlink_path.canonicalize().unwrap();
    assert_eq!(
        real_canonical, symlink_abs,
        "sanity check: symlink should resolve to real target"
    );

    let paths = worktree_paths();
    assert!(
        paths
            .iter()
            .any(|p| p.as_str() == real_canonical.to_string_lossy().as_ref()),
        "state should store canonical real path instead of symlink path"
    );

    exec_async(vec!["worktree", "lock", "wt_link"])
        .await
        .expect("lock by symlink path should resolve the registered entry");
    exec_async(vec!["worktree", "unlock", "wt_link"])
        .await
        .expect("unlock by symlink path should resolve the registered entry");
    exec_async(vec!["worktree", "remove", "wt_link"])
        .await
        .expect("remove by symlink path should resolve the registered entry");
}

#[cfg(unix)]
#[tokio::test]
#[serial]
/// Adding once through a symlinked parent and once through the real path should not create duplicates.
async fn test_worktree_add_symlink_and_real_path_are_deduplicated() {
    let root_dir = tempdir().unwrap();
    let repo_path = root_dir.path().join("repo");
    fs::create_dir_all(&repo_path).unwrap();
    test::setup_with_new_libra_in(&repo_path).await;
    let _guard = test::ChangeDirGuard::new(&repo_path);

    let real_parent = root_dir.path().join("real_parent");
    fs::create_dir_all(&real_parent).unwrap();
    let alias_parent = root_dir.path().join("alias_parent");
    symlink(&real_parent, &alias_parent).expect("failed to create symlink parent");

    let via_symlink = alias_parent.join("wt_dup_sym");
    let via_real = real_parent.join("wt_dup_sym");
    let via_symlink_str = via_symlink.to_string_lossy().to_string();
    let via_real_str = via_real.to_string_lossy().to_string();

    exec_async(vec!["worktree", "add", via_symlink_str.as_str()])
        .await
        .expect("add via symlinked parent should succeed");
    exec_async(vec!["worktree", "add", via_real_str.as_str()])
        .await
        .expect("add via real parent should not fail");

    let canonical = via_real.canonicalize().unwrap();
    let paths = worktree_paths();
    let matches = paths
        .iter()
        .filter(|p| p.as_str() == canonical.to_string_lossy().as_ref())
        .count();
    assert_eq!(
        matches, 1,
        "symlink and real paths should deduplicate to one canonical worktree entry"
    );
}

#[tokio::test]
#[serial]
/// Adding into an existing non-empty directory is rejected and preserves local files.
async fn test_worktree_add_rejects_existing_non_empty_directory() {
    let repo_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(repo_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(repo_dir.path());

    test::ensure_file("a.txt", Some("repo-version"));
    add::execute(AddArgs {
        pathspec: vec!["a.txt".to_string()],
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

    let wt_path = repo_dir.path().join("wt_non_empty");
    fs::create_dir_all(&wt_path).expect("failed to create pre-existing worktree target");
    fs::write(wt_path.join("a.txt"), b"local-data")
        .expect("failed to seed pre-existing target content");

    exec_async(vec!["worktree", "add", "wt_non_empty"])
        .await
        .expect("worktree add command itself should not fail");

    assert!(
        !wt_path.join(".libra").exists(),
        "rejected add should not create .libra link in non-empty target"
    );
    let preserved =
        fs::read_to_string(wt_path.join("a.txt")).expect("target file should still exist");
    assert_eq!(
        preserved, "local-data",
        "rejected add should preserve existing directory contents"
    );

    let canonical_target = wt_path.canonicalize().unwrap();
    let paths = worktree_paths();
    assert!(
        !paths
            .iter()
            .any(|p| p == canonical_target.to_string_lossy().as_ref()),
        "rejected add should not register the non-empty target as a worktree"
    );
}

#[tokio::test]
#[serial]
/// Duplicate `worktree add` should not recreate a missing directory when the path is already registered.
async fn test_worktree_add_duplicate_registered_path_does_not_create_directory() {
    let repo_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(repo_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(repo_dir.path());

    exec_async(vec!["worktree", "add", "wt_dup"])
        .await
        .expect("initial worktree add should succeed");

    let wt_path = repo_dir.path().join("wt_dup");
    assert!(wt_path.is_dir());

    fs::remove_dir_all(&wt_path).expect("failed to remove existing worktree directory");
    assert!(!wt_path.exists(), "worktree directory should be missing");

    let before_paths = worktree_paths();
    exec_async(vec!["worktree", "add", "wt_dup"])
        .await
        .expect("duplicate worktree add command itself should not fail");
    let after_paths = worktree_paths();

    assert_eq!(
        before_paths, after_paths,
        "duplicate add should not mutate registered worktree state"
    );
    assert!(
        !wt_path.exists(),
        "duplicate add should not create a new directory for an already registered path"
    );
}

#[tokio::test]
#[serial]
/// If population fails after writing the link file, `worktree add` rolls back partial artifacts.
async fn test_worktree_add_rolls_back_link_on_restore_failure() {
    let repo_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(repo_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(repo_dir.path());

    test::ensure_file("conflict/file.txt", Some("v1"));
    add::execute(AddArgs {
        pathspec: vec!["conflict/file.txt".to_string()],
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

    let wt_path = repo_dir.path().join("wt_restore_fail");
    fs::create_dir_all(&wt_path).expect("failed to create existing target directory");
    fs::write(wt_path.join("conflict"), b"blocking file")
        .expect("failed to create conflicting path in target");

    exec_async(vec!["worktree", "add", "wt_restore_fail"])
        .await
        .expect("worktree add command itself should not fail");

    assert!(
        !wt_path.join(".libra").exists(),
        "failed restore should remove the partial .libra link"
    );

    let canonical_target = wt_path.canonicalize().unwrap();
    let paths = worktree_paths();
    assert!(
        !paths
            .iter()
            .any(|p| p == canonical_target.to_string_lossy().as_ref()),
        "failed restore should not register the worktree in state"
    );
    assert!(
        wt_path.join("conflict").is_file(),
        "pre-existing target content should be preserved on rollback"
    );
}

#[cfg(unix)]
#[tokio::test]
#[serial]
/// If state persistence fails after restore, rollback removes partially restored files in an existing target.
async fn test_worktree_add_rolls_back_populated_files_when_state_save_fails() {
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

    let wt_path = repo_dir.path().join("wt_state_save_fail");
    fs::create_dir_all(&wt_path).expect("failed to create existing empty target");

    exec_async(vec!["worktree", "list"])
        .await
        .expect("worktree list should initialize worktree state");
    assert!(
        util::storage_path().join("worktrees.json").exists(),
        "worktrees.json should exist before forcing save_state failure"
    );

    let storage_dir = util::storage_path();
    let original_mode = fs::metadata(&storage_dir)
        .expect("failed to stat storage directory")
        .permissions()
        .mode();
    let mut read_only = fs::metadata(&storage_dir)
        .expect("failed to stat storage directory")
        .permissions();
    read_only.set_mode(original_mode & !0o222);
    fs::set_permissions(&storage_dir, read_only)
        .expect("failed to set storage directory read-only");

    exec_async(vec!["worktree", "add", "wt_state_save_fail"])
        .await
        .expect("worktree add command itself should not fail");

    let mut restore_mode = fs::metadata(&storage_dir)
        .expect("failed to stat storage directory")
        .permissions();
    restore_mode.set_mode(original_mode);
    fs::set_permissions(&storage_dir, restore_mode)
        .expect("failed to restore storage directory permissions");

    assert!(
        !wt_path.join(".libra").exists(),
        "failed save_state should remove the partial .libra link"
    );
    assert!(
        fs::read_dir(&wt_path)
            .expect("target directory should still exist")
            .next()
            .is_none(),
        "rollback should clear partially restored files from existing target directory"
    );

    let canonical_target = wt_path.canonicalize().unwrap();
    let paths = worktree_paths();
    assert!(
        !paths
            .iter()
            .any(|p| p == canonical_target.to_string_lossy().as_ref()),
        "failed save_state should not register the worktree in state"
    );
}

#[cfg(unix)]
#[tokio::test]
#[serial]
/// Cross-filesystem moves should fail cleanly and keep registry/state unchanged when test env provides separate devices.
async fn test_worktree_move_across_filesystems_rolls_back_when_supported() {
    let repo_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(repo_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(repo_dir.path());

    exec_async(vec!["worktree", "add", "wt_cross_src"])
        .await
        .expect("worktree add should succeed");
    let src_path = repo_dir.path().join("wt_cross_src");
    assert!(src_path.is_dir());

    let other_fs_dir = match tempfile::tempdir_in("/tmp") {
        Ok(d) => d,
        Err(_) => return, // Environment does not allow creating this probe directory.
    };

    let repo_dev = fs::metadata(repo_dir.path()).unwrap().dev();
    let other_dev = fs::metadata(other_fs_dir.path()).unwrap().dev();
    if repo_dev == other_dev {
        return; // Not a cross-filesystem setup on this machine; skip.
    }

    let dest_path = other_fs_dir.path().join("wt_cross_dest");
    let dest_str = dest_path.to_string_lossy().to_string();
    let before_paths = worktree_paths();

    exec_async(vec!["worktree", "move", "wt_cross_src", dest_str.as_str()])
        .await
        .expect("worktree move command itself should not fail");

    let after_paths = worktree_paths();
    assert_eq!(
        before_paths, after_paths,
        "failed cross-filesystem move should keep worktree registry unchanged"
    );
    assert!(
        src_path.exists(),
        "source directory should remain after failed cross-filesystem move"
    );
    assert!(
        !dest_path.exists(),
        "destination directory should not be created by failed cross-filesystem move"
    );
}

#[tokio::test]
#[serial]
/// Corrupted `worktrees.json` should fail commands gracefully without mutating state or creating directories.
async fn test_worktree_corrupted_state_file_is_handled_without_side_effects() {
    let repo_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(repo_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(repo_dir.path());

    exec_async(vec!["worktree", "list"])
        .await
        .expect("worktree list should initialize state first");

    let state_path = util::storage_path().join("worktrees.json");
    fs::write(&state_path, b"{ invalid json").expect("failed to corrupt state file");
    let before = fs::read_to_string(&state_path).unwrap();

    exec_async(vec!["worktree", "list"])
        .await
        .expect("worktree list command itself should not fail on corrupted state");

    let after = fs::read_to_string(&state_path).unwrap();
    assert_eq!(
        before, after,
        "failed state load should not rewrite corrupted worktree state"
    );

    let new_path = repo_dir.path().join("wt_from_corrupt");
    assert!(!new_path.exists());
    exec_async(vec!["worktree", "add", "wt_from_corrupt"])
        .await
        .expect("worktree add command itself should not fail on corrupted state");
    assert!(
        !new_path.exists(),
        "add should not create target directory when worktree state cannot be loaded"
    );
}

#[tokio::test]
#[serial]
/// Basic lock/unlock/remove happy path for a non-main worktree.
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
/// Creating a worktree must not disturb existing staged changes in the index.
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
/// New worktree population should use `HEAD` content instead of staged index-only updates.
async fn test_worktree_add_populates_from_head_not_staged_index() {
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

    exec_async(vec!["worktree", "add", "wt_head_content"])
        .await
        .expect("worktree add should succeed");

    let linked_content =
        fs::read_to_string(repo_dir.path().join("wt_head_content").join("tracked.txt")).unwrap();
    assert_eq!(
        linked_content, "v1",
        "new worktree should be populated from HEAD, not staged index updates"
    );
}

#[tokio::test]
#[serial]
/// `worktree list` should include both main and added worktrees and be read-only.
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
/// Moving an unlocked, non-main worktree updates both the filesystem and state.
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
/// Moving the main worktree is rejected without creating or registering a destination.
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
/// Moving a locked worktree is rejected without changing its path or lock state.
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
/// Moving a worktree onto an existing worktree path is rejected without mutation.
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
/// Moving a worktree into `.libra` storage is rejected without mutating filesystem or state.
async fn test_worktree_move_rejects_destination_inside_storage() {
    let repo_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(repo_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(repo_dir.path());

    exec_async(vec!["worktree", "add", "wt_storage_src"])
        .await
        .expect("worktree add should succeed");

    let src = repo_dir.path().join("wt_storage_src");
    let blocked_dest = repo_dir.path().join(".libra").join("moved_inside_storage");
    assert!(src.is_dir());
    assert!(!blocked_dest.exists());

    let before_paths = worktree_paths();

    exec_async(vec![
        "worktree",
        "move",
        "wt_storage_src",
        ".libra/moved_inside_storage",
    ])
    .await
    .expect("worktree move command itself should not fail");

    assert!(
        src.is_dir(),
        "source directory should remain after rejected move into storage"
    );
    assert!(
        !blocked_dest.exists(),
        "destination inside storage should not be created"
    );

    let after_paths = worktree_paths();
    assert_eq!(
        before_paths, after_paths,
        "rejected move into storage should not mutate worktree registry"
    );
}

#[tokio::test]
#[serial]
/// `worktree prune` removes missing non-main worktrees from the registry.
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
/// Removing a locked worktree is rejected without changing state or directory.
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
/// `worktree repair` removes duplicate entries that point to the same path.
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
/// `worktree repair` persists main-flag fixes even when there are no duplicate paths.
async fn test_worktree_repair_persists_main_flag_fix_without_duplicates() {
    let repo_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(repo_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(repo_dir.path());

    exec_async(vec!["worktree", "add", "wt_main_fix"])
        .await
        .expect("worktree add should succeed");

    let mut state = read_worktree_state();
    for w in &mut state.worktrees {
        w.is_main = false;
    }

    let state_path = util::storage_path().join("worktrees.json");
    let data = serde_json::to_string_pretty(&state)
        .expect("failed to serialize worktree state with broken main flags");
    fs::write(&state_path, data).expect("failed to overwrite worktrees.json");

    exec_async(vec!["worktree", "repair"])
        .await
        .expect("worktree repair should succeed");

    let repaired = read_worktree_state();
    let main_entries: Vec<_> = repaired.worktrees.iter().filter(|w| w.is_main).collect();
    assert_eq!(
        main_entries.len(),
        1,
        "repair should persist exactly one main worktree flag"
    );
    assert_eq!(
        main_entries[0].path,
        repo_dir.path().canonicalize().unwrap().to_string_lossy(),
        "repair should persist the original repository root as main"
    );
}

#[tokio::test]
#[serial]
/// The main worktree flag remains unique and anchored to the original repo root.
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

#[tokio::test]
#[serial]
/// With `--separate-libra-dir`, the main worktree entry should stay anchored to the original workdir.
async fn test_worktree_main_flag_stable_with_separate_libra_dir() {
    let temp_root = tempdir().unwrap();
    let workdir = temp_root.path().join("work");
    let storage = temp_root.path().join("storage");
    fs::create_dir_all(&workdir).unwrap();

    init(InitArgs {
        bare: false,
        template: None,
        initial_branch: None,
        repo_directory: workdir.to_string_lossy().to_string(),
        quiet: true,
        shared: None,
        object_format: None,
        ref_format: None,
        separate_libra_dir: Some(storage.to_string_lossy().to_string()),
    })
    .await
    .expect("init with separate-libra-dir should succeed");

    let _guard = test::ChangeDirGuard::new(&workdir);
    exec_async(vec!["worktree", "add", "wt_sep_main"])
        .await
        .expect("worktree add should succeed");

    let original_main = workdir.canonicalize().unwrap();
    let wt_path = workdir.join("wt_sep_main");
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
        original_main.to_string_lossy(),
        "main worktree entry should remain the original workdir with separate storage"
    );
}

#[tokio::test]
#[serial]
/// In `--separate-libra-dir` layout, main designation stays stable when commands run from both worktrees.
async fn test_worktree_main_flag_stable_across_both_worktrees_in_separate_layout() {
    let temp_root = tempdir().unwrap();
    let workdir = temp_root.path().join("work");
    let storage = temp_root.path().join("storage");
    fs::create_dir_all(&workdir).unwrap();

    init(InitArgs {
        bare: false,
        template: None,
        initial_branch: None,
        repo_directory: workdir.to_string_lossy().to_string(),
        quiet: true,
        shared: None,
        object_format: None,
        ref_format: None,
        separate_libra_dir: Some(storage.to_string_lossy().to_string()),
    })
    .await
    .expect("init with separate-libra-dir should succeed");

    let expected_main = workdir.canonicalize().unwrap();
    let assert_main_stable = || {
        let state = read_worktree_state();
        let main_entries: Vec<_> = state.worktrees.iter().filter(|w| w.is_main).collect();
        assert_eq!(
            main_entries.len(),
            1,
            "there should be exactly one main entry"
        );
        assert_eq!(
            main_entries[0].path,
            expected_main.to_string_lossy(),
            "main entry should always be the original workdir"
        );
    };

    let _guard_main = test::ChangeDirGuard::new(&workdir);
    exec_async(vec!["worktree", "add", "wt_sep_dual"])
        .await
        .expect("worktree add should succeed");
    exec_async(vec!["worktree", "list"])
        .await
        .expect("worktree list from main worktree should succeed");
    assert_main_stable();

    let linked = workdir.join("wt_sep_dual");
    {
        let _guard_linked = test::ChangeDirGuard::new(&linked);
        exec_async(vec!["worktree", "list"])
            .await
            .expect("worktree list from linked worktree should succeed");
        exec_async(vec!["worktree", "lock", "."])
            .await
            .expect("worktree lock from linked worktree should succeed");
        exec_async(vec!["worktree", "unlock", "."])
            .await
            .expect("worktree unlock from linked worktree should succeed");
        assert_main_stable();
    }

    exec_async(vec!["worktree", "list"])
        .await
        .expect("worktree list from main worktree should still succeed");
    assert_main_stable();
}

#[tokio::test]
#[serial]
/// In `--separate-libra-dir` layout, removing the real main worktree from a linked worktree is rejected.
async fn test_worktree_remove_main_is_rejected_with_separate_libra_dir() {
    let temp_root = tempdir().unwrap();
    let workdir = temp_root.path().join("work");
    let storage = temp_root.path().join("storage");
    fs::create_dir_all(&workdir).unwrap();

    init(InitArgs {
        bare: false,
        template: None,
        initial_branch: None,
        repo_directory: workdir.to_string_lossy().to_string(),
        quiet: true,
        shared: None,
        object_format: None,
        ref_format: None,
        separate_libra_dir: Some(storage.to_string_lossy().to_string()),
    })
    .await
    .expect("init with separate-libra-dir should succeed");

    let _guard = test::ChangeDirGuard::new(&workdir);
    exec_async(vec!["worktree", "add", "wt_sep_guard"])
        .await
        .expect("worktree add should succeed");

    let main_path = workdir.canonicalize().unwrap();
    let main_path_str = main_path.to_string_lossy().to_string();

    let wt_path = workdir.join("wt_sep_guard");
    let _guard_wt = test::ChangeDirGuard::new(&wt_path);
    let before_paths = worktree_paths();

    exec_async(vec!["worktree", "remove", main_path_str.as_str()])
        .await
        .expect("worktree remove command itself should not fail");

    let after_paths = worktree_paths();
    assert_eq!(
        before_paths, after_paths,
        "remove main should not mutate worktree state in separate-libra-dir layout"
    );

    let state = read_worktree_state();
    let main_entries: Vec<_> = state.worktrees.iter().filter(|w| w.is_main).collect();
    assert_eq!(main_entries.len(), 1);
    assert_eq!(main_entries[0].path, main_path.to_string_lossy());
}

#[tokio::test]
#[serial]
/// If main is corrupted to storage parent in separate-libra-dir layout, loading from linked worktree repairs it.
async fn test_worktree_recovers_invalid_main_storage_parent_in_separate_layout() {
    let temp_root = tempdir().unwrap();
    let workdir = temp_root.path().join("work");
    let storage = temp_root.path().join("storage");
    fs::create_dir_all(&workdir).unwrap();

    init(InitArgs {
        bare: false,
        template: None,
        initial_branch: None,
        repo_directory: workdir.to_string_lossy().to_string(),
        quiet: true,
        shared: None,
        object_format: None,
        ref_format: None,
        separate_libra_dir: Some(storage.to_string_lossy().to_string()),
    })
    .await
    .expect("init with separate-libra-dir should succeed");

    let _guard = test::ChangeDirGuard::new(&workdir);
    exec_async(vec!["worktree", "add", "wt_sep_recover"])
        .await
        .expect("worktree add should succeed");

    let original_main = workdir.canonicalize().unwrap();
    let linked = workdir.join("wt_sep_recover").canonicalize().unwrap();
    let storage_parent = storage.parent().unwrap().canonicalize().unwrap();

    let mut state = read_worktree_state();
    state.worktrees.push(TestWorktreeEntry {
        path: storage_parent.to_string_lossy().to_string(),
        is_main: true,
        locked: false,
        lock_reason: None,
    });
    for w in &mut state.worktrees {
        if w.path != storage_parent.to_string_lossy() {
            w.is_main = false;
        }
    }

    let state_path = util::storage_path().join("worktrees.json");
    let data = serde_json::to_string_pretty(&state).unwrap();
    fs::write(&state_path, data).unwrap();

    let _guard_wt = test::ChangeDirGuard::new(&linked);
    exec_async(vec!["worktree", "list"])
        .await
        .expect("worktree list should repair invalid main marker");

    let repaired = read_worktree_state();
    let main_entries: Vec<_> = repaired.worktrees.iter().filter(|w| w.is_main).collect();
    assert_eq!(
        main_entries.len(),
        1,
        "there should be one repaired main entry"
    );
    assert_eq!(
        main_entries[0].path,
        original_main.to_string_lossy(),
        "main should be repaired back to the original workdir"
    );
}
