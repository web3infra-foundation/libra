//! Tests push command negotiation and ref update flows against remotes.

#[cfg(unix)]
use std::path::{Path, PathBuf};
use std::{env, fs, process::Command, time::Duration};

use clap::Parser;
use libra::{
    command::push,
    internal::{config::Config, db::get_db_conn_instance, reflog::Reflog},
    utils::test::ChangeDirGuard,
};
use serial_test::serial;
use tempfile::TempDir;
use tokio::{process::Command as TokioCommand, time::timeout};

use super::{create_committed_repo_via_cli, run_libra_command};

fn libra_command(cwd: &std::path::Path) -> Command {
    let home = cwd.join(".libra-test-home");
    let config_home = home.join(".config");
    fs::create_dir_all(&config_home).expect("failed to create isolated HOME");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_libra"));
    cmd.current_dir(cwd)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("USERPROFILE", &home);
    cmd
}

fn libra_tokio_command(cwd: &std::path::Path) -> TokioCommand {
    let home = cwd.join(".libra-test-home");
    let config_home = home.join(".config");
    fs::create_dir_all(&config_home).expect("failed to create isolated HOME");

    let mut cmd = TokioCommand::new(env!("CARGO_BIN_EXE_libra"));
    cmd.current_dir(cwd)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("USERPROFILE", &home);
    cmd
}

/// Helper function: Initialize a temporary Libra repository
fn init_temp_repo() -> TempDir {
    let temp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
    let temp_path = temp_dir.path();

    eprintln!("Temporary directory created at: {temp_path:?}");
    assert!(
        temp_path.is_dir(),
        "Temporary path is not a valid directory"
    );

    let output = libra_command(temp_path)
        .args(["init", "--vault"])
        .output()
        .expect("Failed to execute libra binary");

    if !output.status.success() {
        panic!(
            "Failed to initialize libra repository: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    eprintln!("Initialized libra repo at: {temp_path:?}");
    temp_dir
}

#[cfg(unix)]
fn configure_local_identity(repo: &Path) {
    let name_out = libra_command(repo)
        .args(["config", "user.name", "Push Test User"])
        .output()
        .expect("failed to configure user.name");
    assert!(
        name_out.status.success(),
        "failed to configure user.name: {}",
        String::from_utf8_lossy(&name_out.stderr)
    );

    let email_out = libra_command(repo)
        .args(["config", "user.email", "push-test@example.com"])
        .output()
        .expect("failed to configure user.email");
    assert!(
        email_out.status.success(),
        "failed to configure user.email: {}",
        String::from_utf8_lossy(&email_out.stderr)
    );
}

#[test]
#[serial]
fn test_push_cli_without_remote_returns_fatal_128() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["push"], repo.path());
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert!(stderr.contains("fatal: no configured push destination"));
    assert!(stderr.contains("Hint:"));
}

#[cfg(unix)]
fn create_fake_ssh_script(root: &Path) -> PathBuf {
    let script_path = root.join("fake_ssh.sh");
    let script = r#"#!/bin/sh
set -eu

if [ -n "${LIBRA_TEST_SSH_LOG:-}" ]; then
  printf '%s\n' "$@" >> "$LIBRA_TEST_SSH_LOG"
  printf -- '---\n' >> "$LIBRA_TEST_SSH_LOG"
fi

if [ "${LIBRA_TEST_SSH_FAIL:-}" = "hostkey" ]; then
  echo "Host key verification failed." >&2
  exit 255
fi

remote_cmd=""
for arg in "$@"; do
  remote_cmd="$arg"
done

if [ -z "$remote_cmd" ]; then
  echo "missing remote command" >&2
  exit 2
fi

exec sh -c "$remote_cmd"
"#;
    fs::write(&script_path, script).expect("failed to write fake ssh script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&script_path)
            .expect("failed to stat fake ssh script")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms).expect("failed to chmod fake ssh script");
    }
    script_path
}

#[tokio::test]
#[serial]
async fn test_push_force_flag_parsing() {
    let temp_repo = init_temp_repo();
    let temp_path = temp_repo.path();
    let _guard = ChangeDirGuard::new(temp_path);

    // Test that --force flag is correctly parsed
    let args = push::PushArgs::parse_from(["push", "--force", "origin", "main"]);
    assert!(args.force);

    // Test that -f flag is correctly parsed
    let args = push::PushArgs::parse_from(["push", "-f", "origin", "main"]);
    assert!(args.force);
}

#[tokio::test]
#[serial]
async fn test_push_file_remote_succeeds_and_updates_tracking() {
    // Local file remotes should behave like regular Git remotes for push.
    let remote_dir = tempfile::tempdir().unwrap();
    let remote_path = remote_dir.path();
    let init_remote = Command::new("git")
        .args(["init", "--bare", remote_path.to_str().unwrap()])
        .output()
        .expect("init remote");
    assert!(
        init_remote.status.success(),
        "failed to initialize bare remote: {}",
        String::from_utf8_lossy(&init_remote.stderr)
    );

    // local repo
    let local_dir = tempfile::tempdir().unwrap();
    let local_path = local_dir.path();
    let _guard = ChangeDirGuard::new(local_path);
    let out = libra_command(local_path)
        .args(["init", "--vault"])
        .output()
        .expect("init");
    assert!(
        out.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = libra_command(local_path)
        .args(["config", "user.name", "Push Test User"])
        .output()
        .expect("set user.name");
    assert!(
        out.status.success(),
        "set user.name failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = libra_command(local_path)
        .args(["config", "user.email", "push-test@example.com"])
        .output()
        .expect("set user.email");
    assert!(
        out.status.success(),
        "set user.email failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // add file + commit
    std::fs::write(local_path.join("file.txt"), "hello").unwrap();
    let out = libra_command(local_path)
        .args(["add", "file.txt"])
        .output()
        .expect("add");
    assert!(
        out.status.success(),
        "add failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = libra_command(local_path)
        .args(["commit", "-m", "init"])
        .output()
        .expect("commit");
    assert!(
        out.status.success(),
        "commit failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // add remote (local path, will be treated as file://)
    let out = libra_command(local_path)
        .args(["remote", "add", "origin", remote_path.to_str().unwrap()])
        .output()
        .expect("remote add");
    assert!(
        out.status.success(),
        "remote add failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // push should succeed to local file remotes and write remote-tracking reflog
    let out = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(local_path)
        .args(["push", "-u", "origin", "main"])
        .output()
        .expect("push");
    assert!(
        out.status.success(),
        "push to local remote should succeed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let local_head_out = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(local_path)
        .args(["log", "-n", "1", "--oneline"])
        .output()
        .expect("read local head");
    assert!(
        local_head_out.status.success(),
        "failed to read local head: {}",
        String::from_utf8_lossy(&local_head_out.stderr)
    );
    let local_head = String::from_utf8_lossy(&local_head_out.stdout)
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string();
    assert!(
        !local_head.is_empty(),
        "local head hash should not be empty"
    );

    let remote_head_out = Command::new("git")
        .args([
            "--git-dir",
            remote_path.to_str().unwrap(),
            "rev-parse",
            "refs/heads/main",
        ])
        .output()
        .expect("read remote head");
    assert!(
        remote_head_out.status.success(),
        "failed to read remote head: {}",
        String::from_utf8_lossy(&remote_head_out.stderr)
    );
    let remote_head = String::from_utf8_lossy(&remote_head_out.stdout)
        .trim()
        .to_string();
    assert!(
        remote_head.starts_with(&local_head),
        "remote branch should point to pushed commit, remote={remote_head}, local_prefix={local_head}"
    );

    // ensure reflog entry is written for remote tracking update
    env::set_current_dir(local_path).expect("set current dir to local repo");
    let db = get_db_conn_instance().await;
    let entry = Reflog::find_one(&db, "refs/remotes/origin/main")
        .await
        .expect("query reflog");
    assert!(
        entry.is_some(),
        "reflog should be created after successful push"
    );
}

#[tokio::test]
#[serial]
async fn test_push_set_upstream_tracks_pushed_branch_when_refspec_differs() {
    let remote_dir = tempfile::tempdir().unwrap();
    let remote_path = remote_dir.path();
    let init_remote = Command::new("git")
        .args(["init", "--bare", remote_path.to_str().unwrap()])
        .output()
        .expect("init remote");
    assert!(
        init_remote.status.success(),
        "failed to initialize bare remote: {}",
        String::from_utf8_lossy(&init_remote.stderr)
    );

    let repo = create_committed_repo_via_cli();
    let repo_path = repo.path();

    let current_branch_out = run_libra_command(&["branch", "--show-current"], repo_path);
    assert!(
        current_branch_out.status.success(),
        "failed to get current branch: {}",
        String::from_utf8_lossy(&current_branch_out.stderr)
    );
    let current_branch = String::from_utf8_lossy(&current_branch_out.stdout)
        .trim()
        .to_string();
    assert!(
        !current_branch.is_empty(),
        "current branch should not be empty"
    );

    let remote_add_out = run_libra_command(
        &["remote", "add", "origin", remote_path.to_str().unwrap()],
        repo_path,
    );
    assert!(
        remote_add_out.status.success(),
        "remote add failed: {}",
        String::from_utf8_lossy(&remote_add_out.stderr)
    );

    let create_branch_out = run_libra_command(&["branch", "topic"], repo_path);
    assert!(
        create_branch_out.status.success(),
        "branch create failed: {}",
        String::from_utf8_lossy(&create_branch_out.stderr)
    );

    let push_out = run_libra_command(&["push", "-u", "origin", "topic"], repo_path);
    assert!(
        push_out.status.success(),
        "push with upstream should succeed: {}",
        String::from_utf8_lossy(&push_out.stderr)
    );

    {
        let _guard = ChangeDirGuard::new(repo_path);
        let pushed_branch_config = Config::branch_config("topic")
            .await
            .expect("pushed branch should have tracking config");
        assert_eq!(pushed_branch_config.remote, "origin");
        assert_eq!(pushed_branch_config.merge, "topic");

        assert!(
            Config::branch_config(&current_branch).await.is_none(),
            "current branch should keep its existing tracking config"
        );
    }

    let pull_out = run_libra_command(&["pull"], repo_path);
    assert_eq!(pull_out.status.code(), Some(128));
    assert!(
        String::from_utf8_lossy(&pull_out.stderr)
            .contains("no configured remote for the current branch"),
        "pull should fail on current branch without tracking after push -u to another branch: {}",
        String::from_utf8_lossy(&pull_out.stderr)
    );
}

#[test]
#[serial]
fn test_push_set_upstream_with_detached_head_returns_fatal_128() {
    let repo = create_committed_repo_via_cli();
    let repo_path = repo.path();

    let log_out = run_libra_command(&["log", "-n", "1", "--oneline"], repo_path);
    assert!(
        log_out.status.success(),
        "failed to read current commit: {}",
        String::from_utf8_lossy(&log_out.stderr)
    );
    let commit_hash = String::from_utf8_lossy(&log_out.stdout)
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string();
    assert!(!commit_hash.is_empty(), "commit hash should not be empty");

    let detach_out = run_libra_command(&["switch", "--detach", &commit_hash], repo_path);
    assert!(
        detach_out.status.success(),
        "detach should succeed: {}",
        String::from_utf8_lossy(&detach_out.stderr)
    );

    let push_out = run_libra_command(&["push", "-u", "origin", "main"], repo_path);
    assert_eq!(push_out.status.code(), Some(128));
    assert!(
        String::from_utf8_lossy(&push_out.stderr).contains("cannot set upstream: HEAD is detached")
    );
}

#[tokio::test]
#[ignore] // This test requires network connectivity
/// Test pushing to an invalid remote repository with timeout
async fn test_push_invalid_remote() {
    let temp_repo = init_temp_repo();
    let temp_path = temp_repo.path();
    let _guard = ChangeDirGuard::new(temp_path);

    eprintln!("Starting test: push to invalid remote");

    // Configure an invalid remote repository
    eprintln!("Adding invalid remote: https://invalid-url.example/repo.git");
    let remote_output = libra_tokio_command(temp_path)
        .args([
            "remote",
            "add",
            "origin",
            "https://invalid-url.example/repo.git",
        ])
        .output()
        .await
        .expect("Failed to add remote");

    assert!(
        remote_output.status.success(),
        "Failed to add remote: {}",
        String::from_utf8_lossy(&remote_output.stderr)
    );

    // Set upstream branch
    eprintln!("Setting upstream to origin/main");
    let branch_output = libra_tokio_command(temp_path)
        .args(["branch", "--set-upstream-to", "origin/main"])
        .output()
        .await
        .expect("Failed to set upstream branch");

    assert!(
        branch_output.status.success(),
        "Failed to set upstream: {}",
        String::from_utf8_lossy(&branch_output.stderr)
    );

    // Attempt to push with 15-second timeout to avoid hanging CI
    eprintln!("Attempting 'libra push' with 15s timeout...");
    let push_result = timeout(Duration::from_secs(15), async {
        libra_tokio_command(temp_path).arg("push").output().await
    })
    .await;

    match push_result {
        // Timeout occurred — this is expected for unreachable remotes
        Err(_) => {
            eprintln!("Push timed out after 15 seconds — expected for invalid remote");
        }
        // Command completed within timeout
        Ok(Ok(output)) => {
            eprintln!("Push completed (status: {:?})", output.status);
            // Push to invalid remote should fail
            assert!(
                !output.status.success(),
                "Push should fail when remote is unreachable"
            );
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                !stderr.trim().is_empty(),
                "Expected error message in stderr, but was empty"
            );

            eprintln!("Push failed as expected: {stderr}");
        }
        // Failed to start the command
        Ok(Err(e)) => {
            panic!("Failed to run 'libra push' command: {e}");
        }
    }

    eprintln!("test_push_invalid_remote passed");
}

#[tokio::test]
#[serial]
async fn test_push_force_with_local_changes() {
    // This test would verify force push functionality in a local repository setup
    // It would require setting up two repositories, making divergent changes,
    // and verifying that force push correctly overwrites the remote history

    // Note: This is a placeholder for a more comprehensive integration test
    // that would require a more complex setup with actual Git repositories
}

#[test]
#[serial]
fn test_push_unsupported_url_scheme_returns_fatal_128() {
    let repo = create_committed_repo_via_cli();
    let repo_path = repo.path();

    // Add a remote with an unsupported scheme (e.g., ftp)
    let remote_add_out = run_libra_command(
        &["remote", "add", "origin", "ftp://example.com/repo"],
        repo_path,
    );
    assert!(
        remote_add_out.status.success(),
        "remote add failed: {}",
        String::from_utf8_lossy(&remote_add_out.stderr)
    );

    // Push should fail with unsupported URL scheme error
    let push_out = run_libra_command(&["push", "origin", "main"], repo_path);
    let stderr = String::from_utf8_lossy(&push_out.stderr);

    assert_eq!(push_out.status.code(), Some(128));
    assert!(
        stderr.contains("unsupported URL scheme"),
        "expected unsupported URL scheme error, got: {stderr}"
    );
}

#[test]
#[serial]
fn test_push_ssh_url_scheme_returns_fatal_128() {
    let repo = create_committed_repo_via_cli();
    let repo_path = repo.path();

    // Add a remote with SSH protocol scheme
    let remote_add_out = run_libra_command(
        &["remote", "add", "origin", "ssh://git@example.com/repo.git"],
        repo_path,
    );
    assert!(
        remote_add_out.status.success(),
        "remote add failed: {}",
        String::from_utf8_lossy(&remote_add_out.stderr)
    );

    // Push should fail with unsupported URL scheme error
    let push_out = run_libra_command(&["push", "origin", "main"], repo_path);
    let stderr = String::from_utf8_lossy(&push_out.stderr);

    assert_eq!(push_out.status.code(), Some(128));
    assert!(
        stderr.contains("unsupported URL scheme"),
        "expected unsupported URL scheme error, got: {stderr}"
    );
}
