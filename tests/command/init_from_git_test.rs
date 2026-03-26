//! Tests `libra init --from-git-repository` for converting an existing Git repository into a Libra repo.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use std::{fs, path::Path, process::Command};

use tempfile::tempdir;

use super::parse_cli_error_stderr;

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
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .env("LIBRA_TEST", "1");
    cmd
}

#[test]
fn test_init_from_git_repository_converts_repo() {
    let (temp_root, git_dir) = create_simple_git_repo();
    let libra_dir = temp_root.path().join("libra-repo");
    fs::create_dir_all(&libra_dir).unwrap();

    let status = libra_command(&libra_dir)
        .args(["init", "--from-git-repository", git_dir.to_str().unwrap()])
        .status()
        .expect("failed to execute libra init");
    assert!(status.success(), "libra init should succeed");

    // Verify origin remote is configured and points at the source .git directory
    let remote_out = libra_command(&libra_dir)
        .args(["remote", "-v"])
        .output()
        .expect("failed to run remote -v");
    let remote_stdout = String::from_utf8_lossy(&remote_out.stdout);
    let expected_remote = git_dir.join(".git").canonicalize().unwrap();
    assert!(
        remote_stdout.contains(expected_remote.to_str().unwrap()),
        "origin should point at {}, got: {remote_stdout}",
        expected_remote.display()
    );

    // Verify HEAD points to a branch
    let branch_out = libra_command(&libra_dir)
        .args(["branch", "--show-current"])
        .output()
        .expect("failed to run branch --show-current");
    let branch_name = String::from_utf8_lossy(&branch_out.stdout)
        .trim()
        .to_string();
    assert!(
        !branch_name.is_empty(),
        "HEAD should point to a branch after conversion"
    );

    // Verify that branch exists in the local branch list
    let list_out = libra_command(&libra_dir)
        .args(["branch"])
        .output()
        .expect("failed to run branch");
    let branches = String::from_utf8_lossy(&list_out.stdout);
    assert!(
        branches.contains(&branch_name),
        "local branch '{branch_name}' should exist in branch list: {branches}"
    );
}

#[tokio::test]
async fn test_init_from_git_repository_missing_source_fails() {
    let temp_root = tempdir().unwrap();
    let libra_dir = temp_root.path().join("libra-repo");
    fs::create_dir_all(&libra_dir).unwrap();

    let missing = temp_root.path().join("missing-git");

    let output = libra_command(&libra_dir)
        .args(["init", "--from-git-repository", missing.to_str().unwrap()])
        .output()
        .expect("failed to execute libra init");
    assert!(
        !output.status.success(),
        "libra init should fail for missing source repository"
    );

    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert!(
        stderr.contains("source git repository"),
        "expected missing-source error, got: {stderr}"
    );
    assert_eq!(report.error_code, "LBR-IO-001");
    assert_eq!(report.exit_code, 128);
}

#[tokio::test]
async fn test_init_from_git_repository_non_git_path_fails() {
    let temp_root = tempdir().unwrap();
    let non_git_dir = temp_root.path().join("not-a-git");
    fs::create_dir_all(&non_git_dir).unwrap();

    let libra_dir = temp_root.path().join("libra-repo");
    fs::create_dir_all(&libra_dir).unwrap();

    let output = libra_command(&libra_dir)
        .args([
            "init",
            "--from-git-repository",
            non_git_dir.to_str().unwrap(),
        ])
        .output()
        .expect("failed to execute libra init");
    assert!(
        !output.status.success(),
        "libra init should fail when source path is not a git repository"
    );

    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert!(
        stderr.contains("is not a valid Git repository"),
        "expected invalid-git error, got: {stderr}"
    );
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert_eq!(report.exit_code, 129);
    assert_eq!(
        report.hints,
        vec!["a valid Git repository must contain HEAD, config, and objects.".to_string()]
    );
}

#[tokio::test]
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

    let output = libra_command(&libra_dir)
        .args(["init", "--from-git-repository", git_dir.to_str().unwrap()])
        .output()
        .expect("failed to execute libra init");
    assert!(
        !output.status.success(),
        "libra init should fail for empty git repository"
    );

    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert!(
        stderr.contains("no refs fetched from source git repository"),
        "expected empty-git conversion failure, got: {stderr}"
    );
    assert_eq!(report.error_code, "LBR-REPO-003");
    assert_eq!(report.exit_code, 128);
}

#[tokio::test]
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

#[test]
fn test_init_from_git_repository_bare_source_repo() {
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

    let remote_out = libra_command(&libra_dir)
        .args(["remote", "-v"])
        .output()
        .expect("failed to run remote -v");
    let remote_stdout = String::from_utf8_lossy(&remote_out.stdout);
    assert!(
        remote_stdout.contains("origin"),
        "origin remote should be configured, got: {remote_stdout}"
    );
}

#[test]
fn test_init_from_git_repository_bare_target_repo() {
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

    let remote_out = libra_command(&libra_dir)
        .args(["remote", "-v"])
        .output()
        .expect("failed to run remote -v");
    let remote_stdout = String::from_utf8_lossy(&remote_out.stdout);
    assert!(
        remote_stdout.contains("origin"),
        "origin remote should be configured for bare init, got: {remote_stdout}"
    );
}

#[test]
fn test_init_from_git_repository_json_reports_converted_from_without_stderr_noise() {
    let (temp_root, git_dir) = create_simple_git_repo();
    let libra_dir = temp_root.path().join("libra-repo-json");
    fs::create_dir_all(&libra_dir).unwrap();

    let output = libra_command(&libra_dir)
        .args([
            "--json",
            "init",
            "--vault",
            "false",
            "--from-git-repository",
            git_dir.to_str().unwrap(),
        ])
        .output()
        .expect("failed to execute libra init");
    assert!(
        output.status.success(),
        "json init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.trim().is_empty(),
        "json init should not leak fetch progress to stderr, got: {stderr}"
    );

    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|error| panic!("expected JSON output, got: {stdout}\nerror: {error}"));
    let converted_from = parsed["data"]["converted_from"]
        .as_str()
        .expect("converted_from should be a string");
    assert!(
        converted_from.ends_with("/.git"),
        "converted_from should point at the canonical .git directory, got: {converted_from}"
    );
}

#[test]
fn test_init_from_git_repository_human_progress_is_only_init_stage_text() {
    let (temp_root, git_dir) = create_simple_git_repo();
    let libra_dir = temp_root.path().join("libra-repo-human");
    fs::create_dir_all(&libra_dir).unwrap();
    let canonical_git_dir = git_dir.join(".git").canonicalize().unwrap();

    let output = libra_command(&libra_dir)
        .args([
            "init",
            "--vault",
            "false",
            "--from-git-repository",
            git_dir.to_str().unwrap(),
        ])
        .output()
        .expect("failed to execute libra init");
    assert!(
        output.status.success(),
        "human init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!(
            "Converting from Git repository at {} ...",
            canonical_git_dir.display()
        )),
        "expected init-owned conversion progress, got: {stderr}"
    );
    assert!(
        !stderr.contains("Receiving objects:")
            && !stderr.contains("remote:")
            && !stderr.contains("\"ok\""),
        "nested fetch output should stay suppressed during init, got: {stderr}"
    );
}
