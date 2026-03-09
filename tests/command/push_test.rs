//! Tests push command negotiation and ref update flows against remotes.

#[cfg(unix)]
use std::path::{Path, PathBuf};
use std::{env, fs, process::Command, time::Duration};

use clap::Parser;
use libra::{
    command::push,
    internal::{db::get_db_conn_instance, reflog::Reflog},
    utils::test::ChangeDirGuard,
};
use serial_test::serial;
use tempfile::TempDir;
use tokio::{process::Command as TokioCommand, time::timeout};

/// Helper function: Initialize a temporary Libra repository
fn init_temp_repo() -> TempDir {
    let temp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
    let temp_path = temp_dir.path();

    eprintln!("Temporary directory created at: {temp_path:?}");
    assert!(
        temp_path.is_dir(),
        "Temporary path is not a valid directory"
    );

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .arg("init")
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
async fn test_push_file_remote_fails_without_reflog() {
    // local file remotes are not supported; ensure we fail loudly and avoid reflog writes
    let remote_dir = tempfile::tempdir().unwrap();
    let remote_path = remote_dir.path();

    // local repo
    let local_dir = tempfile::tempdir().unwrap();
    let local_path = local_dir.path();
    let _guard = ChangeDirGuard::new(local_path);
    let out = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(local_path)
        .arg("init")
        .output()
        .expect("init");
    assert!(
        out.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // add file + commit
    std::fs::write(local_path.join("file.txt"), "hello").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(local_path)
        .args(["add", "file.txt"])
        .output()
        .expect("add");
    assert!(
        out.status.success(),
        "add failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(local_path)
        .args(["commit", "-m", "init"])
        .output()
        .expect("commit");
    assert!(
        out.status.success(),
        "commit failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // add remote (local path, will be treated as file://)
    let out = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(local_path)
        .args(["remote", "add", "origin", remote_path.to_str().unwrap()])
        .output()
        .expect("remote add");
    assert!(
        out.status.success(),
        "remote add failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // push should fail with clear fatal message
    let out = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(local_path)
        .args(["push", "origin", "master"])
        .output()
        .expect("push");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("pushing to local file repositories is not yet supported"),
        "stderr should mention unsupported file:// push, got: {stderr}"
    );

    // ensure no reflog entry is written
    env::set_current_dir(local_path).expect("set current dir to local repo");
    let db = get_db_conn_instance().await;
    let entry = Reflog::find_one(db, "refs/remotes/origin/master")
        .await
        .expect("query reflog");
    assert!(
        entry.is_none(),
        "reflog should not be created when push fails"
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
    let remote_output = TokioCommand::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
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
    let branch_output = TokioCommand::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
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
        TokioCommand::new(env!("CARGO_BIN_EXE_libra"))
            .current_dir(temp_path)
            .arg("push")
            .output()
            .await
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

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn test_push_ssh_remote_via_fake_ssh() {
    let temp_root = tempfile::tempdir().expect("failed to create temp root");
    let remote_dir = temp_root.path().join("remote.git");
    let local_dir = temp_root.path().join("local");
    let log_path = temp_root.path().join("fake_ssh.log");
    let ssh_script = create_fake_ssh_script(temp_root.path());

    assert!(
        Command::new("git")
            .args(["init", "--bare", remote_dir.to_str().unwrap()])
            .status()
            .expect("failed to init bare remote")
            .success()
    );

    fs::create_dir_all(&local_dir).expect("failed to create local dir");
    let init_out = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(&local_dir)
        .arg("init")
        .output()
        .expect("failed to init local libra repo");
    assert!(
        init_out.status.success(),
        "local init failed: {}",
        String::from_utf8_lossy(&init_out.stderr)
    );

    fs::write(local_dir.join("hello.txt"), "hello push ssh").expect("failed to write file");
    let add_out = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(&local_dir)
        .args(["add", "hello.txt"])
        .output()
        .expect("failed to add file");
    assert!(
        add_out.status.success(),
        "add failed: {}",
        String::from_utf8_lossy(&add_out.stderr)
    );
    let commit_out = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(&local_dir)
        .args(["commit", "-m", "initial commit"])
        .output()
        .expect("failed to commit");
    assert!(
        commit_out.status.success(),
        "commit failed: {}",
        String::from_utf8_lossy(&commit_out.stderr)
    );

    let ssh_remote = format!("git@fakehost:{}", remote_dir.to_string_lossy());
    let remote_add_out = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(&local_dir)
        .args(["remote", "add", "origin", &ssh_remote])
        .output()
        .expect("failed to add ssh remote");
    assert!(
        remote_add_out.status.success(),
        "remote add failed: {}",
        String::from_utf8_lossy(&remote_add_out.stderr)
    );

    let push_out = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(&local_dir)
        .env("LIBRA_SSH_COMMAND", &ssh_script)
        .env("LIBRA_TEST_SSH_LOG", &log_path)
        .args(["push", "origin", "master"])
        .output()
        .expect("failed to run push over fake ssh");
    assert!(
        push_out.status.success(),
        "push over SSH should succeed, stderr: {}",
        String::from_utf8_lossy(&push_out.stderr)
    );
    let stdout = String::from_utf8_lossy(&push_out.stdout);
    assert!(
        stdout.contains("Push success"),
        "push should report success, stdout: {stdout}"
    );

    let remote_head_out = Command::new("git")
        .args([
            "--git-dir",
            remote_dir.to_str().unwrap(),
            "rev-parse",
            "refs/heads/master",
        ])
        .output()
        .expect("failed to read remote head");
    assert!(
        remote_head_out.status.success(),
        "remote master branch should exist after push, stderr: {}",
        String::from_utf8_lossy(&remote_head_out.stderr)
    );

    let ssh_log = fs::read_to_string(&log_path).expect("failed to read fake ssh log");
    assert!(
        ssh_log.contains("StrictHostKeyChecking=yes"),
        "SSH command should enforce strict host key checking, log:\n{ssh_log}"
    );
    assert!(
        !ssh_log.contains("StrictHostKeyChecking=accept-new"),
        "SSH command must not use accept-new by default, log:\n{ssh_log}"
    );
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn test_push_ssh_host_key_failure_is_reported() {
    let temp_root = tempfile::tempdir().expect("failed to create temp root");
    let remote_dir = temp_root.path().join("remote.git");
    let local_dir = temp_root.path().join("local");
    let ssh_script = create_fake_ssh_script(temp_root.path());

    assert!(
        Command::new("git")
            .args(["init", "--bare", remote_dir.to_str().unwrap()])
            .status()
            .expect("failed to init bare remote")
            .success()
    );

    fs::create_dir_all(&local_dir).expect("failed to create local dir");
    let init_out = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(&local_dir)
        .arg("init")
        .output()
        .expect("failed to init local libra repo");
    assert!(
        init_out.status.success(),
        "local init failed: {}",
        String::from_utf8_lossy(&init_out.stderr)
    );

    fs::write(local_dir.join("hello.txt"), "hello push ssh fail").expect("failed to write file");
    let add_out = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(&local_dir)
        .args(["add", "hello.txt"])
        .output()
        .expect("failed to add file");
    assert!(
        add_out.status.success(),
        "add failed: {}",
        String::from_utf8_lossy(&add_out.stderr)
    );
    let commit_out = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(&local_dir)
        .args(["commit", "-m", "initial commit"])
        .output()
        .expect("failed to commit");
    assert!(
        commit_out.status.success(),
        "commit failed: {}",
        String::from_utf8_lossy(&commit_out.stderr)
    );

    let ssh_remote = format!("git@fakehost:{}", remote_dir.to_string_lossy());
    let remote_add_out = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(&local_dir)
        .args(["remote", "add", "origin", &ssh_remote])
        .output()
        .expect("failed to add ssh remote");
    assert!(
        remote_add_out.status.success(),
        "remote add failed: {}",
        String::from_utf8_lossy(&remote_add_out.stderr)
    );

    let push_out = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(&local_dir)
        .env("LIBRA_SSH_COMMAND", &ssh_script)
        .env("LIBRA_TEST_SSH_FAIL", "hostkey")
        .args(["push", "origin", "master"])
        .output()
        .expect("failed to run push over fake ssh");
    let stderr = String::from_utf8_lossy(&push_out.stderr);
    assert!(
        stderr.contains("Host key verification failed."),
        "push should surface SSH host-key failures, stderr: {stderr}"
    );

    let remote_head_out = Command::new("git")
        .args([
            "--git-dir",
            remote_dir.to_str().unwrap(),
            "rev-parse",
            "refs/heads/master",
        ])
        .output()
        .expect("failed to read remote head");
    assert!(
        !remote_head_out.status.success(),
        "remote branch should not be created when SSH host-key verification fails"
    );
}
