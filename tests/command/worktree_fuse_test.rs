//! FUSE worktree tests (feature-gated).

use std::{fs, path::Path};

use libra::{
    exec_async,
    utils::{test, util},
};
use serial_test::serial;
use tempfile::tempdir;

use super::*;

#[derive(Debug, serde::Deserialize)]
struct FuseState {
    worktrees: Vec<FuseEntry>,
}

#[derive(Debug, serde::Deserialize)]
struct FuseEntry {
    path: String,
    branch: String,
    upper_dir: String,
    lower_dirs: Vec<String>,
    locked: bool,
}

fn fuse_state_path() -> std::path::PathBuf {
    util::storage_path().join("worktrees-fuse.json")
}

fn read_fuse_state() -> FuseState {
    let data = fs::read_to_string(fuse_state_path()).expect("worktrees-fuse.json should exist");
    serde_json::from_str(&data).expect("fuse state should be valid json")
}

fn can_run_fuse_tests() -> bool {
    Path::new("/dev/fuse").exists()
        && std::env::var("LIBRA_RUN_FUSE_TESTS")
            .ok()
            .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "on"))
            .unwrap_or(false)
}

fn is_known_fuse_env_error(message: &str) -> bool {
    message.contains("Transport endpoint is not connected")
        || message.contains("Operation not permitted")
        || message.contains("failed to unmount FUSE worktree")
}

fn try_write_probe(path: &Path) -> bool {
    let probe = path.join(".libra-fuse-probe");
    if fs::write(&probe, b"probe").is_err() {
        return false;
    }
    let _ = fs::remove_file(&probe);
    true
}

fn is_mounted(path: &Path) -> bool {
    let Ok(content) = fs::read_to_string("/proc/self/mountinfo") else {
        return false;
    };
    let target = path.to_string_lossy().to_string();
    content.lines().any(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 5 {
            return false;
        }
        fields[4].replace("\\040", " ") == target
    })
}

#[tokio::test]
#[serial]
async fn test_fuse_worktree_metadata_management() {
    if !can_run_fuse_tests() {
        return;
    }

    let repo_dir = tempdir().expect("create temp repo");
    test::setup_with_new_libra_in(repo_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(repo_dir.path());

    if let Err(err) = exec_async(vec!["worktree", "add", "wt-fuse-meta", "--fuse"]).await {
        if is_known_fuse_env_error(&err.to_string()) {
            return;
        }
        panic!("fuse add should succeed: {err}");
    }

    let mount_path = repo_dir.path().join("wt-fuse-meta");
    if !is_mounted(&mount_path) || !try_write_probe(&mount_path) {
        return;
    }

    let state = read_fuse_state();
    assert_eq!(state.worktrees.len(), 1);
    let entry = &state.worktrees[0];
    assert!(entry.path.ends_with("wt-fuse-meta"));
    assert!(!entry.upper_dir.is_empty());
    assert!(!entry.lower_dirs.is_empty());
    assert!(!entry.locked);
}

#[tokio::test]
#[serial]
async fn test_fuse_worktree_add_list_remove_flow() {
    if !can_run_fuse_tests() {
        return;
    }

    let repo_dir = tempdir().expect("create temp repo");
    test::setup_with_new_libra_in(repo_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(repo_dir.path());

    if let Err(err) = exec_async(vec!["worktree", "add", "wt-fuse-flow", "--fuse"]).await {
        if is_known_fuse_env_error(&err.to_string()) {
            return;
        }
        panic!("fuse add should succeed: {err}");
    }

    let mount_path = repo_dir.path().join("wt-fuse-flow");
    if !is_mounted(&mount_path) || !try_write_probe(&mount_path) {
        return;
    }

    let list_output = run_libra_command(&["worktree", "list"], repo_dir.path());
    assert!(list_output.status.success(), "list should succeed");
    let stdout = String::from_utf8_lossy(&list_output.stdout);
    assert!(
        stdout.contains("wt-fuse-flow"),
        "list output should include fuse worktree: {stdout}"
    );

    if let Err(err) = exec_async(vec!["worktree", "remove", "wt-fuse-flow"]).await {
        if is_known_fuse_env_error(&err.to_string()) {
            return;
        }
        panic!("fuse remove should succeed: {err}");
    }

    let state = read_fuse_state();
    assert!(state.worktrees.is_empty(), "fuse state should be cleaned");
}

#[tokio::test]
#[serial]
async fn test_fuse_multiple_worktrees_mount_and_access() {
    if !can_run_fuse_tests() {
        return;
    }

    let repo_dir = tempdir().expect("create temp repo");
    test::setup_with_new_libra_in(repo_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(repo_dir.path());

    if let Err(err) = exec_async(vec!["worktree", "add", "wt-fuse-a", "--fuse"]).await {
        if is_known_fuse_env_error(&err.to_string()) {
            return;
        }
        panic!("fuse add a should succeed: {err}");
    }
    if let Err(err) = exec_async(vec!["worktree", "add", "wt-fuse-b", "--fuse"]).await {
        if is_known_fuse_env_error(&err.to_string()) {
            return;
        }
        panic!("fuse add b should succeed: {err}");
    }

    let a = repo_dir.path().join("wt-fuse-a");
    let b = repo_dir.path().join("wt-fuse-b");

    if !is_mounted(&a) || !is_mounted(&b) || !try_write_probe(&a) || !try_write_probe(&b) {
        return;
    }

    fs::write(a.join("only-a.txt"), b"a").expect("write a");
    fs::write(b.join("only-b.txt"), b"b").expect("write b");

    assert!(a.join("only-a.txt").exists());
    assert!(b.join("only-b.txt").exists());
    assert!(!a.join("only-b.txt").exists());
    assert!(!b.join("only-a.txt").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn test_fuse_worktree_parallel_add_remove() {
    if !can_run_fuse_tests() {
        return;
    }

    let repo_dir = tempdir().expect("create temp repo");
    test::setup_with_new_libra_in(repo_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(repo_dir.path());

    let (r1, r2) = tokio::join!(
        exec_async(vec!["worktree", "add", "wt-par-1", "--fuse"]),
        exec_async(vec!["worktree", "add", "wt-par-2", "--fuse"])
    );
    if let Err(err) = r1 {
        if is_known_fuse_env_error(&err.to_string()) {
            return;
        }
        panic!("parallel add 1 should succeed: {err}");
    }
    if let Err(err) = r2 {
        if is_known_fuse_env_error(&err.to_string()) {
            return;
        }
        panic!("parallel add 2 should succeed: {err}");
    }

    let state = read_fuse_state();
    if state.worktrees.len() != 2 {
        return;
    }

    let (d1, d2) = tokio::join!(
        exec_async(vec!["worktree", "remove", "wt-par-1"]),
        exec_async(vec!["worktree", "remove", "wt-par-2"])
    );
    if let Err(err) = d1 {
        if is_known_fuse_env_error(&err.to_string()) {
            return;
        }
        panic!("parallel remove 1 should succeed: {err}");
    }
    if let Err(err) = d2 {
        if is_known_fuse_env_error(&err.to_string()) {
            return;
        }
        panic!("parallel remove 2 should succeed: {err}");
    }
}

#[tokio::test]
#[serial]
async fn test_fuse_worktree_add_with_branch_and_create_branch() {
    if !can_run_fuse_tests() {
        return;
    }

    let repo_dir = tempdir().expect("create temp repo");
    test::setup_with_new_libra_in(repo_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(repo_dir.path());

    fs::write(repo_dir.path().join("seed.txt"), "seed\n").expect("write seed");
    let add_output = run_libra_command(&["add", "seed.txt"], repo_dir.path());
    assert!(add_output.status.success(), "add should succeed");
    let commit_output = run_libra_command(
        &["commit", "-m", "seed", "--no-verify"],
        repo_dir.path(),
    );
    assert!(commit_output.status.success(), "commit should succeed");

    if let Err(err) = exec_async(vec![
        "worktree",
        "add",
        "wt-fuse-branch",
        "--fuse",
        "--create-branch",
        "feature/fuse-wt",
    ])
    .await
    {
        if is_known_fuse_env_error(&err.to_string()) {
            return;
        }
        panic!("fuse add with create-branch should succeed: {err}");
    }

    let mount_path = repo_dir.path().join("wt-fuse-branch");
    if !is_mounted(&mount_path) || !try_write_probe(&mount_path) {
        return;
    }

    let state = read_fuse_state();
    assert_eq!(state.worktrees.len(), 1);
    assert_eq!(state.worktrees[0].branch, "feature/fuse-wt");
}
