//! Tests `libra init --from-git-repository` for converting an existing Git repository into a Libra repo.

use std::{fs, process::Command};

use libra::{
    internal::{branch::Branch, config::Config, head::Head},
    utils::test::ChangeDirGuard,
};
use serial_test::serial;
use tempfile::tempdir;

use super::*;

/// Helper to create a simple local Git repository with a single commit and return its path.
fn create_simple_git_repo() -> (tempfile::TempDir, std::path::PathBuf) {
    let temp_root = tempdir().unwrap();
    let git_dir = temp_root.path().join("git-src");
    fs::create_dir_all(&git_dir).unwrap();

    assert!(
        Command::new("git")
            .args(["init", git_dir.to_str().unwrap()])
            .status()
            .unwrap()
            .success()
    );

    assert!(
        Command::new("git")
            .current_dir(&git_dir)
            .args(["config", "user.name", "Libra Tester"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&git_dir)
            .args(["config", "user.email", "tester@example.com"])
            .status()
            .unwrap()
            .success()
    );

    fs::write(git_dir.join("README.md"), "hello from git").unwrap();
    assert!(
        Command::new("git")
            .current_dir(&git_dir)
            .args(["add", "README.md"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&git_dir)
            .args(["commit", "-m", "initial from git"])
            .status()
            .unwrap()
            .success()
    );

    (temp_root, git_dir)
}

#[tokio::test]
#[serial]
async fn test_init_from_git_repository_converts_repo() {
    let (temp_root, git_dir) = create_simple_git_repo();
    let libra_dir = temp_root.path().join("libra-repo");
    fs::create_dir_all(&libra_dir).unwrap();

    let status = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(&libra_dir)
        .args(["init", "--from-git-repository", git_dir.to_str().unwrap()])
        .status()
        .expect("failed to execute libra init");
    assert!(status.success(), "libra init should succeed");

    let _guard = ChangeDirGuard::new(&libra_dir);

    let remote = Config::remote_config("origin").await;
    assert!(remote.is_some(), "origin remote should be configured");
    let remote = remote.unwrap();
    assert_eq!(
        remote.url,
        git_dir.to_str().unwrap(),
        "origin url should match source Git repository path"
    );

    let head = Head::current().await;
    let branch_name = match head {
        Head::Branch(name) => name,
        _ => panic!("HEAD should point to a branch after conversion"),
    };
    let local_branches = Branch::list_branches(None).await;
    assert!(
        local_branches.iter().any(|b| b.name == branch_name),
        "local branch created from source Git repository should exist"
    );
}
