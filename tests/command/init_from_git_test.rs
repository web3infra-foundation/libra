//! Tests `libra init --from-git-repository` for converting an existing Git repository into a Libra repo.

use std::{fs, path::Path, process::Command};

use libra::{
    internal::{branch::Branch, config::Config, head::Head},
    utils::test::ChangeDirGuard,
};
use serial_test::serial;
use tempfile::tempdir;

/// Helper to create a simple local Git repository with a single commit and return its path.
fn create_simple_git_repo() -> (tempfile::TempDir, std::path::PathBuf) {
    let temp_root = tempdir().unwrap();
    let git_dir = temp_root.path().join("git-src");
    fs::create_dir_all(&git_dir).unwrap();

    assert!(
        Command::new("git")
            .args(["init", "-b", "main", git_dir.to_str().unwrap()])
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

fn libra_command(cwd: &Path) -> Command {
    let home = cwd.join(".libra-test-home");
    let config_home = home.join(".config");
    fs::create_dir_all(&config_home).expect("failed to create isolated HOME for CLI test");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_libra"));
    cmd.current_dir(cwd)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &config_home);
    #[cfg(windows)]
    cmd.env("USERPROFILE", &home);
    cmd
}

#[tokio::test]
#[serial]
async fn test_init_from_git_repository_converts_repo() {
    let (temp_root, git_dir) = create_simple_git_repo();
    let libra_dir = temp_root.path().join("libra-repo");
    fs::create_dir_all(&libra_dir).unwrap();

    let status = libra_command(&libra_dir)
        .args(["init", "--from-git-repository", git_dir.to_str().unwrap()])
        .status()
        .expect("failed to execute libra init");
    assert!(status.success(), "libra init should succeed");

    let _guard = ChangeDirGuard::new(&libra_dir);

    let remote = Config::remote_config("origin").await;
    assert!(remote.is_some(), "origin remote should be configured");
    let remote = remote.unwrap();
    let expected_remote = git_dir.join(".git").canonicalize().unwrap();
    let actual_remote = Path::new(&remote.url).canonicalize().unwrap();
    assert_eq!(actual_remote, expected_remote);

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

#[tokio::test]
#[serial]
async fn test_init_from_git_repository_missing_source_fails() {
    let temp_root = tempdir().unwrap();
    let libra_dir = temp_root.path().join("libra-repo");
    fs::create_dir_all(&libra_dir).unwrap();

    let missing = temp_root.path().join("missing-git");

    let status = libra_command(&libra_dir)
        .args(["init", "--from-git-repository", missing.to_str().unwrap()])
        .status()
        .expect("failed to execute libra init");
    assert!(
        !status.success(),
        "libra init should fail for missing source repository"
    );
}

#[tokio::test]
#[serial]
async fn test_init_from_git_repository_non_git_path_fails() {
    let temp_root = tempdir().unwrap();
    let non_git_dir = temp_root.path().join("not-a-git");
    fs::create_dir_all(&non_git_dir).unwrap();

    let libra_dir = temp_root.path().join("libra-repo");
    fs::create_dir_all(&libra_dir).unwrap();

    let status = libra_command(&libra_dir)
        .args([
            "init",
            "--from-git-repository",
            non_git_dir.to_str().unwrap(),
        ])
        .status()
        .expect("failed to execute libra init");
    assert!(
        !status.success(),
        "libra init should fail when source path is not a git repository"
    );
}

#[tokio::test]
#[serial]
async fn test_init_from_git_repository_empty_git_repo_fails() {
    let temp_root = tempdir().unwrap();
    let git_dir = temp_root.path().join("empty-git");
    fs::create_dir_all(&git_dir).unwrap();

    assert!(
        Command::new("git")
            .args(["init", git_dir.to_str().unwrap()])
            .status()
            .unwrap()
            .success()
    );

    let libra_dir = temp_root.path().join("libra-repo");
    fs::create_dir_all(&libra_dir).unwrap();

    let status = libra_command(&libra_dir)
        .args(["init", "--from-git-repository", git_dir.to_str().unwrap()])
        .status()
        .expect("failed to execute libra init");
    assert!(
        !status.success(),
        "libra init should fail for empty git repository"
    );
}

#[tokio::test]
#[serial]
async fn test_init_from_git_repository_multiple_branches() {
    let temp_root = tempdir().unwrap();
    let git_dir = temp_root.path().join("git-src");
    fs::create_dir_all(&git_dir).unwrap();

    assert!(
        Command::new("git")
            .args(["init", "-b", "main", git_dir.to_str().unwrap()])
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

    fs::write(git_dir.join("file-main.txt"), "main branch").unwrap();
    assert!(
        Command::new("git")
            .current_dir(&git_dir)
            .args(["add", "file-main.txt"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&git_dir)
            .args(["commit", "-m", "main commit"])
            .status()
            .unwrap()
            .success()
    );

    assert!(
        Command::new("git")
            .current_dir(&git_dir)
            .args(["checkout", "-b", "feature"])
            .status()
            .unwrap()
            .success()
    );
    fs::write(git_dir.join("file-feature.txt"), "feature branch").unwrap();
    assert!(
        Command::new("git")
            .current_dir(&git_dir)
            .args(["add", "file-feature.txt"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&git_dir)
            .args(["commit", "-m", "feature commit"])
            .status()
            .unwrap()
            .success()
    );

    assert!(
        Command::new("git")
            .current_dir(&git_dir)
            .args(["checkout", "main"])
            .status()
            .unwrap()
            .success()
    );

    assert!(
        Command::new("git")
            .current_dir(&git_dir)
            .args(["pack-refs", "--all"])
            .status()
            .unwrap()
            .success()
    );

    let libra_dir = temp_root.path().join("libra-repo");
    fs::create_dir_all(&libra_dir).unwrap();

    let output = libra_command(&libra_dir)
        .args(["init", "--from-git-repository", git_dir.to_str().unwrap()])
        .output()
        .expect("failed to execute libra init");

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!("libra init failed: {stderr}");
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("Unsupported ref type during fetch: HEAD"),
        "fetch should skip symbolic HEAD without warning, got stderr: {stderr}"
    );

    let output = libra_command(&libra_dir)
        .args(["branch", "-r"])
        .output()
        .expect("failed to execute libra branch");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let remote_branches: Vec<&str> = stdout
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();
    assert!(remote_branches.len() >= 2);
    assert!(
        remote_branches.contains(&"origin/main"),
        "expected origin/main in remote branches, got: {remote_branches:?}"
    );
    assert!(
        remote_branches.contains(&"origin/feature"),
        "expected origin/feature in remote branches, got: {remote_branches:?}"
    );
    assert!(
        remote_branches.iter().all(|b| !b.contains("refs/remotes/")),
        "remote branch output should not expose internal refs/remotes paths: {remote_branches:?}"
    );
}

#[tokio::test]
#[serial]
async fn test_init_from_git_repository_with_gitlink_entry_succeeds() {
    let temp_root = tempdir().unwrap();
    let git_dir = temp_root.path().join("git-src");
    let sub_repo = temp_root.path().join("sub-src");
    let libra_dir = temp_root.path().join("libra-repo");
    fs::create_dir_all(&git_dir).unwrap();
    fs::create_dir_all(&sub_repo).unwrap();
    fs::create_dir_all(&libra_dir).unwrap();

    // Build a sub-repository commit that will be referenced as a gitlink.
    assert!(
        Command::new("git")
            .args(["init", "-b", "master", sub_repo.to_str().unwrap()])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&sub_repo)
            .args(["config", "user.name", "Libra Tester"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&sub_repo)
            .args(["config", "user.email", "tester@example.com"])
            .status()
            .unwrap()
            .success()
    );
    fs::write(sub_repo.join("sub.txt"), "submodule content").unwrap();
    assert!(
        Command::new("git")
            .current_dir(&sub_repo)
            .args(["add", "sub.txt"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&sub_repo)
            .args(["commit", "-m", "submodule commit"])
            .status()
            .unwrap()
            .success()
    );
    let sub_head = String::from_utf8(
        Command::new("git")
            .current_dir(&sub_repo)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();

    // Build the source repo with a gitlink entry (mode 160000).
    assert!(
        Command::new("git")
            .args(["init", "-b", "master", git_dir.to_str().unwrap()])
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
    fs::write(git_dir.join("README.md"), "root repo").unwrap();
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
            .args(["commit", "-m", "initial"])
            .status()
            .unwrap()
            .success()
    );

    fs::write(
        git_dir.join(".gitmodules"),
        "[submodule \"vendor/sub\"]\n\tpath = vendor/sub\n\turl = ../sub-src\n",
    )
    .unwrap();
    assert!(
        Command::new("git")
            .current_dir(&git_dir)
            .args(["add", ".gitmodules"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&git_dir)
            .args([
                "update-index",
                "--add",
                "--cacheinfo",
                "160000",
                &sub_head,
                "vendor/sub",
            ])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&git_dir)
            .args(["commit", "-m", "add gitlink entry"])
            .status()
            .unwrap()
            .success()
    );

    let output = libra_command(&libra_dir)
        .args(["init", "--from-git-repository", git_dir.to_str().unwrap()])
        .output()
        .expect("failed to execute libra init");

    assert!(
        output.status.success(),
        "libra init should succeed for source repos with gitlink entries; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
#[serial]
async fn test_init_from_git_repository_bare_source_repo() {
    let (temp_root, git_workdir) = create_simple_git_repo();
    let git_dir = temp_root.path().join("git-src-bare");
    assert!(
        Command::new("git")
            .args([
                "clone",
                "--bare",
                git_workdir.to_str().unwrap(),
                git_dir.to_str().unwrap()
            ])
            .status()
            .unwrap()
            .success()
    );

    let libra_dir = temp_root.path().join("libra-repo");
    fs::create_dir_all(&libra_dir).unwrap();

    let status = libra_command(&libra_dir)
        .args(["init", "--from-git-repository", git_dir.to_str().unwrap()])
        .status()
        .expect("failed to execute libra init");
    assert!(status.success(), "libra init should succeed for bare repo");

    let _guard = ChangeDirGuard::new(&libra_dir);

    let remote = Config::remote_config("origin").await;
    assert!(remote.is_some(), "origin remote should be configured");
}

#[tokio::test]
#[serial]
async fn test_init_from_git_repository_bare_target_repo() {
    let (temp_root, git_dir) = create_simple_git_repo();
    let libra_dir = temp_root.path().join("libra-repo-bare");
    fs::create_dir_all(&libra_dir).unwrap();

    let status = libra_command(&libra_dir)
        .args([
            "init",
            "--bare",
            "--from-git-repository",
            git_dir.to_str().unwrap(),
        ])
        .status()
        .expect("failed to execute libra init");
    assert!(status.success(), "bare libra init should succeed");

    let _guard = ChangeDirGuard::new(&libra_dir);

    let remote = Config::remote_config("origin").await;
    assert!(
        remote.is_some(),
        "origin remote should be configured for bare init"
    );
}
