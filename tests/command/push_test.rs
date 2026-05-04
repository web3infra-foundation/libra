//! Tests push command negotiation and ref update flows against remotes.
//!
//! **Layer:** L1 (most tests). `test_push_invalid_remote` and `test_push_force_with_local_changes`
//! are L2 — require `LIBRA_TEST_GITHUB_TOKEN` or are `#[cfg(unix)]`.

#[cfg(unix)]
use std::path::{Path, PathBuf};
use std::{env, fs, process::Command, time::Duration};

use clap::Parser;
use libra::{
    command::push,
    internal::{db::get_db_conn_instance, reflog::Reflog},
    utils::test::ChangeDirGuard,
};
#[cfg(unix)]
use serde_json::Value;
use serial_test::serial;
use tempfile::TempDir;
use tokio::{process::Command as TokioCommand, time::timeout};

use super::{create_committed_repo_via_cli, parse_cli_error_stderr, run_libra_command};

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
        .args(["init"])
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

#[cfg(unix)]
fn init_local_repo_with_commit(local_dir: &Path, file_name: &str, content: &str, message: &str) {
    fs::create_dir_all(local_dir).expect("failed to create local dir");
    let init_out = libra_command(local_dir)
        .args(["init"])
        .output()
        .expect("failed to init local libra repo");
    assert!(
        init_out.status.success(),
        "local init failed: {}",
        String::from_utf8_lossy(&init_out.stderr)
    );
    configure_local_identity(local_dir);

    fs::write(local_dir.join(file_name), content).expect("failed to write file");
    let add_out = libra_command(local_dir)
        .args(["add", file_name])
        .output()
        .expect("failed to add file");
    assert!(
        add_out.status.success(),
        "add failed: {}",
        String::from_utf8_lossy(&add_out.stderr)
    );
    let commit_out = libra_command(local_dir)
        .args(["commit", "-m", message])
        .output()
        .expect("failed to commit");
    assert!(
        commit_out.status.success(),
        "commit failed: {}",
        String::from_utf8_lossy(&commit_out.stderr)
    );
}

#[cfg(unix)]
fn current_branch_name(repo: &Path) -> String {
    String::from_utf8(
        libra_command(repo)
            .args(["branch", "--show-current"])
            .output()
            .expect("failed to read current branch")
            .stdout,
    )
    .expect("branch name not utf8")
    .trim()
    .to_string()
}

#[cfg(unix)]
fn add_fake_ssh_remote(repo: &Path, remote_dir: &Path) {
    let ssh_remote = format!("git@fakehost:{}", remote_dir.to_string_lossy());
    let remote_add_out = libra_command(repo)
        .args(["remote", "add", "origin", &ssh_remote])
        .output()
        .expect("failed to add ssh remote");
    assert!(
        remote_add_out.status.success(),
        "remote add failed: {}",
        String::from_utf8_lossy(&remote_add_out.stderr)
    );
}

#[test]
fn test_push_cli_without_remote_returns_fatal_128() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["push"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-REPO-003");
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
async fn test_push_file_remote_fails_without_reflog() {
    // local file remotes are not supported; ensure we fail loudly and avoid reflog writes
    let remote_dir = tempfile::tempdir().unwrap();
    let remote_path = remote_dir.path();

    // local repo
    let local_dir = tempfile::tempdir().unwrap();
    let local_path = local_dir.path();
    let _guard = ChangeDirGuard::new(local_path);
    let out = libra_command(local_path)
        .args(["init"])
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

    // push should fail with clear fatal message
    let out = libra_command(local_path)
        .args(["push", "origin", "main"])
        .output()
        .expect("push");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("pushing to local file repositories is not supported"),
        "stderr should mention unsupported file:// push, got: {stderr}"
    );

    // ensure no reflog entry is written
    let db = get_db_conn_instance().await;
    let entry = Reflog::find_one(&db, "refs/remotes/origin/master")
        .await
        .expect("query reflog");
    assert!(
        entry.is_none(),
        "reflog should not be created when push fails"
    );
}

#[tokio::test]
/// Test pushing to an invalid remote repository with timeout
async fn test_push_invalid_remote() {
    if std::env::var("LIBRA_TEST_GITHUB_TOKEN").map_or(true, |v| v.is_empty()) {
        eprintln!("skipped (LIBRA_TEST_GITHUB_TOKEN not set)");
        return;
    }
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

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn test_push_force_with_local_changes() {
    let temp_root = tempfile::tempdir().expect("failed to create temp root");
    let remote_dir = temp_root.path().join("remote.git");
    let local_dir = temp_root.path().join("local");
    let ssh_script = create_fake_ssh_script(temp_root.path());

    // Create a bare remote repository
    assert!(
        Command::new("git")
            .args(["init", "--bare", remote_dir.to_str().unwrap()])
            .status()
            .expect("failed to init bare remote")
            .success()
    );

    // Set up local repo with initial commit and push
    fs::create_dir_all(&local_dir).expect("failed to create local dir");
    let init_out = libra_command(&local_dir)
        .args(["init"])
        .output()
        .expect("failed to init local libra repo");
    assert!(
        init_out.status.success(),
        "local init failed: {}",
        String::from_utf8_lossy(&init_out.stderr)
    );
    configure_local_identity(&local_dir);

    fs::write(local_dir.join("file.txt"), "initial content").expect("failed to write file");
    let add_out = libra_command(&local_dir)
        .args(["add", "file.txt"])
        .output()
        .expect("failed to add file");
    assert!(add_out.status.success());
    let commit_out = libra_command(&local_dir)
        .args(["commit", "-m", "initial commit"])
        .output()
        .expect("failed to commit");
    assert!(commit_out.status.success());

    let current_branch = String::from_utf8(
        libra_command(&local_dir)
            .args(["branch", "--show-current"])
            .output()
            .expect("failed to read current branch")
            .stdout,
    )
    .expect("branch name not utf8")
    .trim()
    .to_string();

    let ssh_remote = format!("git@fakehost:{}", remote_dir.to_string_lossy());
    let remote_add_out = libra_command(&local_dir)
        .args(["remote", "add", "origin", &ssh_remote])
        .output()
        .expect("failed to add ssh remote");
    assert!(remote_add_out.status.success());

    let push_out = libra_command(&local_dir)
        .env("LIBRA_SSH_COMMAND", &ssh_script)
        .args(["push", "origin", &current_branch])
        .output()
        .expect("failed to push");
    assert!(
        push_out.status.success(),
        "initial push should succeed, stderr: {}",
        String::from_utf8_lossy(&push_out.stderr)
    );

    // Record the initial remote HEAD
    let initial_head = String::from_utf8(
        Command::new("git")
            .args([
                "--git-dir",
                remote_dir.to_str().unwrap(),
                "rev-parse",
                &format!("refs/heads/{current_branch}"),
            ])
            .output()
            .expect("failed to read remote head")
            .stdout,
    )
    .expect("hash not utf8")
    .trim()
    .to_string();

    // Amend the commit locally to create divergent history
    fs::write(local_dir.join("file.txt"), "force pushed content").expect("failed to overwrite");
    let add_out = libra_command(&local_dir)
        .args(["add", "file.txt"])
        .output()
        .expect("failed to add");
    assert!(add_out.status.success());
    let commit_out = libra_command(&local_dir)
        .args(["commit", "-m", "divergent commit"])
        .output()
        .expect("failed to commit");
    assert!(commit_out.status.success());

    // Force push should succeed and update the remote
    let force_push_out = libra_command(&local_dir)
        .env("LIBRA_SSH_COMMAND", &ssh_script)
        .args(["push", "--force", "origin", &current_branch])
        .output()
        .expect("failed to force push");
    assert!(
        force_push_out.status.success(),
        "force push should succeed, stderr: {}",
        String::from_utf8_lossy(&force_push_out.stderr)
    );

    // Verify that the remote HEAD changed
    let final_head = String::from_utf8(
        Command::new("git")
            .args([
                "--git-dir",
                remote_dir.to_str().unwrap(),
                "rev-parse",
                &format!("refs/heads/{current_branch}"),
            ])
            .output()
            .expect("failed to read remote head")
            .stdout,
    )
    .expect("hash not utf8")
    .trim()
    .to_string();

    assert_ne!(
        initial_head, final_head,
        "force push should have updated the remote HEAD"
    );
}

#[cfg(unix)]
#[test]
#[serial]
fn test_push_explicit_refspec_uses_destination_branch_name() {
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

    init_local_repo_with_commit(
        &local_dir,
        "tracked.txt",
        "initial content",
        "initial commit",
    );
    add_fake_ssh_remote(&local_dir, &remote_dir);

    let feature_out = libra_command(&local_dir)
        .args(["branch", "feature"])
        .output()
        .expect("failed to create feature branch");
    assert!(
        feature_out.status.success(),
        "feature branch creation failed: {}",
        String::from_utf8_lossy(&feature_out.stderr)
    );
    let release_out = libra_command(&local_dir)
        .args(["branch", "release"])
        .output()
        .expect("failed to create release branch");
    assert!(
        release_out.status.success(),
        "release branch creation failed: {}",
        String::from_utf8_lossy(&release_out.stderr)
    );

    let set_remote_out = libra_command(&local_dir)
        .args(["config", "branch.release.remote", "origin"])
        .output()
        .expect("failed to configure branch.release.remote");
    assert!(
        set_remote_out.status.success(),
        "config branch.release.remote failed: {}",
        String::from_utf8_lossy(&set_remote_out.stderr)
    );
    let set_merge_out = libra_command(&local_dir)
        .args(["config", "branch.release.merge", "refs/heads/stable"])
        .output()
        .expect("failed to configure branch.release.merge");
    assert!(
        set_merge_out.status.success(),
        "config branch.release.merge failed: {}",
        String::from_utf8_lossy(&set_merge_out.stderr)
    );

    let push_out = libra_command(&local_dir)
        .env("LIBRA_SSH_COMMAND", &ssh_script)
        .args(["push", "origin", "feature:release"])
        .output()
        .expect("failed to push explicit refspec");
    assert!(
        push_out.status.success(),
        "explicit refspec push failed: {}",
        String::from_utf8_lossy(&push_out.stderr)
    );

    let release_ref_out = Command::new("git")
        .args([
            "--git-dir",
            remote_dir.to_str().unwrap(),
            "rev-parse",
            "refs/heads/release",
        ])
        .output()
        .expect("failed to read remote release ref");
    assert!(
        release_ref_out.status.success(),
        "remote release ref should exist, stderr: {}",
        String::from_utf8_lossy(&release_ref_out.stderr)
    );

    let stable_ref_out = Command::new("git")
        .args([
            "--git-dir",
            remote_dir.to_str().unwrap(),
            "rev-parse",
            "refs/heads/stable",
        ])
        .output()
        .expect("failed to read remote stable ref");
    assert!(
        !stable_ref_out.status.success(),
        "explicit feature:release push should not create refs/heads/stable"
    );
}

#[cfg(unix)]
#[test]
#[serial]
fn test_push_json_with_set_upstream_keeps_structured_output_clean() {
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

    init_local_repo_with_commit(
        &local_dir,
        "tracked.txt",
        "initial content",
        "initial commit",
    );
    add_fake_ssh_remote(&local_dir, &remote_dir);
    let current_branch = current_branch_name(&local_dir);

    let push_out = libra_command(&local_dir)
        .env("LIBRA_SSH_COMMAND", &ssh_script)
        .args(["--json", "push", "-u", "origin", &current_branch])
        .output()
        .expect("failed to run json push");
    assert!(
        push_out.status.success(),
        "json push failed: {}",
        String::from_utf8_lossy(&push_out.stderr)
    );

    let stdout = String::from_utf8_lossy(&push_out.stdout);
    let parsed: Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("json push should emit valid JSON, got: {stdout}\nerror: {e}"));
    assert_eq!(parsed["ok"], Value::Bool(true));
    assert_eq!(parsed["command"], Value::String("push".to_string()));
    assert_eq!(
        parsed["data"]["upstream_set"],
        Value::String(format!("origin/{current_branch}"))
    );
    assert_eq!(
        parsed["data"]["updates"][0]["remote_ref"],
        Value::String(format!("refs/heads/{current_branch}"))
    );

    let stderr = String::from_utf8_lossy(&push_out.stderr);
    assert!(
        stderr.trim().is_empty(),
        "json push success should keep stderr clean, got: {stderr}"
    );
}

#[cfg(unix)]
#[test]
#[serial]
fn test_push_machine_success_is_single_json_line() {
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

    init_local_repo_with_commit(
        &local_dir,
        "tracked.txt",
        "initial content",
        "initial commit",
    );
    add_fake_ssh_remote(&local_dir, &remote_dir);
    let current_branch = current_branch_name(&local_dir);

    let push_out = libra_command(&local_dir)
        .env("LIBRA_SSH_COMMAND", &ssh_script)
        .args(["--machine", "push", "origin", &current_branch])
        .output()
        .expect("failed to run machine push");
    assert!(
        push_out.status.success(),
        "machine push failed: {}",
        String::from_utf8_lossy(&push_out.stderr)
    );

    let stdout = String::from_utf8_lossy(&push_out.stdout);
    let non_empty_lines: Vec<_> = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert_eq!(
        non_empty_lines.len(),
        1,
        "machine push should emit exactly one JSON line, got: {non_empty_lines:?}"
    );
    let parsed: Value = serde_json::from_str(non_empty_lines[0]).unwrap_or_else(|e| {
        panic!(
            "machine push should emit valid JSON, got: {}\nerror: {e}",
            non_empty_lines[0]
        )
    });
    assert_eq!(parsed["ok"], Value::Bool(true));

    let stderr = String::from_utf8_lossy(&push_out.stderr);
    assert!(
        stderr.trim().is_empty(),
        "machine push success should keep stderr clean, got: {stderr}"
    );
}

#[cfg(unix)]
#[test]
#[serial]
fn test_push_quiet_force_still_emits_warning_and_warning_exit_code() {
    let temp_root = tempfile::tempdir().expect("failed to create temp root");
    let remote_dir = temp_root.path().join("remote.git");
    let local_dir = temp_root.path().join("local");
    let other_dir = temp_root.path().join("other");
    let ssh_script = create_fake_ssh_script(temp_root.path());

    assert!(
        Command::new("git")
            .args(["init", "--bare", remote_dir.to_str().unwrap()])
            .status()
            .expect("failed to init bare remote")
            .success()
    );

    init_local_repo_with_commit(
        &local_dir,
        "tracked.txt",
        "initial content",
        "initial commit",
    );
    add_fake_ssh_remote(&local_dir, &remote_dir);
    let current_branch = current_branch_name(&local_dir);

    let initial_push_out = libra_command(&local_dir)
        .env("LIBRA_SSH_COMMAND", &ssh_script)
        .args(["push", "origin", &current_branch])
        .output()
        .expect("failed initial push");
    assert!(
        initial_push_out.status.success(),
        "initial push failed: {}",
        String::from_utf8_lossy(&initial_push_out.stderr)
    );

    assert!(
        Command::new("git")
            .args([
                "clone",
                "--branch",
                &current_branch,
                remote_dir.to_str().unwrap(),
                other_dir.to_str().unwrap(),
            ])
            .status()
            .expect("failed to clone remote")
            .success()
    );
    assert!(
        Command::new("git")
            .args([
                "-C",
                other_dir.to_str().unwrap(),
                "config",
                "user.name",
                "Git User"
            ])
            .status()
            .expect("failed to configure git user.name")
            .success()
    );
    assert!(
        Command::new("git")
            .args([
                "-C",
                other_dir.to_str().unwrap(),
                "config",
                "user.email",
                "git-user@example.com",
            ])
            .status()
            .expect("failed to configure git user.email")
            .success()
    );
    fs::write(other_dir.join("remote.txt"), "remote change").expect("failed to write remote file");
    assert!(
        Command::new("git")
            .args(["-C", other_dir.to_str().unwrap(), "add", "remote.txt"])
            .status()
            .expect("failed to add remote file")
            .success()
    );
    assert!(
        Command::new("git")
            .args([
                "-C",
                other_dir.to_str().unwrap(),
                "commit",
                "-m",
                "remote change"
            ])
            .status()
            .expect("failed to commit remote change")
            .success()
    );
    assert!(
        Command::new("git")
            .args([
                "-C",
                other_dir.to_str().unwrap(),
                "push",
                "origin",
                &current_branch
            ])
            .status()
            .expect("failed to push remote change")
            .success()
    );
    let remote_diverged_head = String::from_utf8(
        Command::new("git")
            .args([
                "--git-dir",
                remote_dir.to_str().unwrap(),
                "rev-parse",
                &format!("refs/heads/{current_branch}"),
            ])
            .output()
            .expect("failed to read diverged remote head")
            .stdout,
    )
    .expect("remote head not utf8")
    .trim()
    .to_string();

    fs::write(local_dir.join("tracked.txt"), "local divergent change")
        .expect("failed to write local divergent file");
    let add_out = libra_command(&local_dir)
        .args(["add", "tracked.txt"])
        .output()
        .expect("failed to add local divergent change");
    assert!(
        add_out.status.success(),
        "failed to add local divergent change: {}",
        String::from_utf8_lossy(&add_out.stderr)
    );
    let commit_out = libra_command(&local_dir)
        .args(["commit", "-m", "local divergent change"])
        .output()
        .expect("failed to commit local divergent change");
    assert!(
        commit_out.status.success(),
        "failed to commit local divergent change: {}",
        String::from_utf8_lossy(&commit_out.stderr)
    );

    let force_push_out = libra_command(&local_dir)
        .env("LIBRA_SSH_COMMAND", &ssh_script)
        .args([
            "--quiet",
            "--exit-code-on-warning",
            "push",
            "--force",
            "origin",
            &current_branch,
        ])
        .output()
        .expect("failed to force push quietly");
    assert_eq!(
        force_push_out.status.code(),
        Some(9),
        "force push with warning exit code should return 9, stderr: {}",
        String::from_utf8_lossy(&force_push_out.stderr)
    );
    assert!(
        force_push_out.stdout.is_empty(),
        "quiet force push should suppress stdout, got: {}",
        String::from_utf8_lossy(&force_push_out.stdout)
    );
    let stderr = String::from_utf8_lossy(&force_push_out.stderr);
    assert!(
        stderr.contains("warning: force push overwrites remote history"),
        "quiet force push should preserve warning output, got: {stderr}"
    );

    let final_remote_head = String::from_utf8(
        Command::new("git")
            .args([
                "--git-dir",
                remote_dir.to_str().unwrap(),
                "rev-parse",
                &format!("refs/heads/{current_branch}"),
            ])
            .output()
            .expect("failed to read final remote head")
            .stdout,
    )
    .expect("final remote head not utf8")
    .trim()
    .to_string();
    assert_ne!(
        remote_diverged_head, final_remote_head,
        "force push should still update the remote ref"
    );
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
    let init_out = libra_command(&local_dir)
        .args(["init"])
        .output()
        .expect("failed to init local libra repo");
    assert!(
        init_out.status.success(),
        "local init failed: {}",
        String::from_utf8_lossy(&init_out.stderr)
    );
    configure_local_identity(&local_dir);

    fs::write(local_dir.join("hello.txt"), "hello push ssh").expect("failed to write file");
    let add_out = libra_command(&local_dir)
        .args(["add", "hello.txt"])
        .output()
        .expect("failed to add file");
    assert!(
        add_out.status.success(),
        "add failed: {}",
        String::from_utf8_lossy(&add_out.stderr)
    );
    let commit_out = libra_command(&local_dir)
        .args(["commit", "-m", "initial commit"])
        .output()
        .expect("failed to commit");
    assert!(
        commit_out.status.success(),
        "commit failed: {}",
        String::from_utf8_lossy(&commit_out.stderr)
    );
    let current_branch = String::from_utf8(
        libra_command(&local_dir)
            .args(["branch", "--show-current"])
            .output()
            .expect("failed to read current branch")
            .stdout,
    )
    .expect("branch name not utf8")
    .trim()
    .to_string();
    assert!(
        !current_branch.is_empty(),
        "current branch should not be empty"
    );

    let ssh_remote = format!("git@fakehost:{}", remote_dir.to_string_lossy());
    let remote_add_out = libra_command(&local_dir)
        .args(["remote", "add", "origin", &ssh_remote])
        .output()
        .expect("failed to add ssh remote");
    assert!(
        remote_add_out.status.success(),
        "remote add failed: {}",
        String::from_utf8_lossy(&remote_add_out.stderr)
    );

    let push_out = libra_command(&local_dir)
        .env("LIBRA_SSH_COMMAND", &ssh_script)
        .env("LIBRA_TEST_SSH_LOG", &log_path)
        .args(["push", "origin", &current_branch])
        .output()
        .expect("failed to run push over fake ssh");
    assert!(
        push_out.status.success(),
        "push over SSH should succeed, stderr: {}",
        String::from_utf8_lossy(&push_out.stderr)
    );
    let stdout = String::from_utf8_lossy(&push_out.stdout);
    assert!(
        stdout.contains("To ") && stdout.contains("->"),
        "push should report success with ref update summary, stdout: {stdout}"
    );

    let remote_head_out = Command::new("git")
        .args([
            "--git-dir",
            remote_dir.to_str().unwrap(),
            "rev-parse",
            &format!("refs/heads/{current_branch}"),
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
    let init_out = libra_command(&local_dir)
        .args(["init"])
        .output()
        .expect("failed to init local libra repo");
    assert!(
        init_out.status.success(),
        "local init failed: {}",
        String::from_utf8_lossy(&init_out.stderr)
    );
    configure_local_identity(&local_dir);

    fs::write(local_dir.join("hello.txt"), "hello push ssh fail").expect("failed to write file");
    let add_out = libra_command(&local_dir)
        .args(["add", "hello.txt"])
        .output()
        .expect("failed to add file");
    assert!(
        add_out.status.success(),
        "add failed: {}",
        String::from_utf8_lossy(&add_out.stderr)
    );
    let commit_out = libra_command(&local_dir)
        .args(["commit", "-m", "initial commit"])
        .output()
        .expect("failed to commit");
    assert!(
        commit_out.status.success(),
        "commit failed: {}",
        String::from_utf8_lossy(&commit_out.stderr)
    );
    let current_branch = String::from_utf8(
        libra_command(&local_dir)
            .args(["branch", "--show-current"])
            .output()
            .expect("failed to read current branch")
            .stdout,
    )
    .expect("branch name not utf8")
    .trim()
    .to_string();
    assert!(
        !current_branch.is_empty(),
        "current branch should not be empty"
    );

    let ssh_remote = format!("git@fakehost:{}", remote_dir.to_string_lossy());
    let remote_add_out = libra_command(&local_dir)
        .args(["remote", "add", "origin", &ssh_remote])
        .output()
        .expect("failed to add ssh remote");
    assert!(
        remote_add_out.status.success(),
        "remote add failed: {}",
        String::from_utf8_lossy(&remote_add_out.stderr)
    );

    let push_out = libra_command(&local_dir)
        .env("LIBRA_SSH_COMMAND", &ssh_script)
        .env("LIBRA_TEST_SSH_FAIL", "hostkey")
        .args(["push", "origin", &current_branch])
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
            &format!("refs/heads/{current_branch}"),
        ])
        .output()
        .expect("failed to read remote head");
    assert!(
        !remote_head_out.status.success(),
        "remote branch should not be created when SSH host-key verification fails"
    );
}
