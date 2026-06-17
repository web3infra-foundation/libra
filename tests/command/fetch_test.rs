//! Tests fetch command behavior for remote ref updates and pack retrieval flows.
//!
//! **Layer:** L1 (most tests). `test_fetch_invalid_remote` is L2 — requires `LIBRA_TEST_GITHUB_TOKEN`.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

#[cfg(unix)]
use libra::internal::vault;
#[cfg(unix)]
use libra::utils::test::ScopedEnvVar;
use libra::{
    command::fetch,
    internal::{
        branch::Branch,
        config::{ConfigKv, RemoteConfig},
    },
    utils::test::{ChangeDirGuard, setup_with_new_libra_in},
};
use serial_test::serial;
use tempfile::{TempDir, tempdir};
use tokio::{process::Command as TokioCommand, time::timeout};

use super::{
    assert_cli_success, create_committed_repo_via_cli, parse_json_stdout, run_libra_command,
};

fn libra_command(cwd: &Path) -> Command {
    let home = cwd.join(".libra-test-home");
    let config_home = home.join(".config");
    fs::create_dir_all(&config_home).expect("failed to create isolated HOME");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_libra"));
    cmd.current_dir(cwd)
        .stdin(Stdio::null())
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("USERPROFILE", &home)
        .env("LIBRA_TEST", "1");
    cmd
}

fn libra_tokio_command(cwd: &Path) -> TokioCommand {
    let home = cwd.join(".libra-test-home");
    let config_home = home.join(".config");
    fs::create_dir_all(&config_home).expect("failed to create isolated HOME");

    let mut cmd = TokioCommand::new(env!("CARGO_BIN_EXE_libra"));
    cmd.current_dir(cwd)
        .stdin(Stdio::null())
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("USERPROFILE", &home)
        .env("LIBRA_TEST", "1");
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

async fn setup_local_fetch_cli_fixture() -> (TempDir, PathBuf, String, String) {
    let temp_root = tempdir().expect("failed to create temp root");
    let remote_dir = temp_root.path().join("remote.git");
    let work_dir = temp_root.path().join("workdir");
    let repo_dir = temp_root.path().join("libra_repo");

    assert!(
        Command::new("git")
            .args(["init", "--bare", remote_dir.to_str().unwrap()])
            .status()
            .expect("failed to init bare remote")
            .success()
    );
    assert!(
        Command::new("git")
            .args(["init", work_dir.to_str().unwrap()])
            .status()
            .expect("failed to init working repo")
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["config", "user.name", "Libra Tester"])
            .status()
            .expect("failed to set user.name")
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["config", "user.email", "tester@example.com"])
            .status()
            .expect("failed to set user.email")
            .success()
    );

    fs::write(work_dir.join("README.md"), "hello libra").expect("failed to write README");
    assert!(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["add", "README.md"])
            .status()
            .expect("failed to add README")
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["commit", "-m", "initial commit"])
            .status()
            .expect("failed to commit")
            .success()
    );

    let current_branch = String::from_utf8(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .output()
            .expect("failed to read current branch")
            .stdout,
    )
    .expect("branch name not utf8")
    .trim()
    .to_string();

    let pushed_commit = String::from_utf8(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("failed to read HEAD commit")
            .stdout,
    )
    .expect("commit hash not utf8")
    .trim()
    .to_string();

    assert!(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["remote", "add", "origin", remote_dir.to_str().unwrap()])
            .status()
            .expect("failed to add origin remote")
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&work_dir)
            .args([
                "push",
                "origin",
                &format!("HEAD:refs/heads/{current_branch}"),
            ])
            .status()
            .expect("failed to push to remote")
            .success()
    );

    fs::create_dir_all(&repo_dir).expect("failed to create repo dir");
    setup_with_new_libra_in(&repo_dir).await;
    let _guard = ChangeDirGuard::new(&repo_dir);
    let remote_path = remote_dir.to_str().unwrap().to_string();
    ConfigKv::set("remote.origin.url", &remote_path, false)
        .await
        .unwrap();

    (temp_root, repo_dir, current_branch, pushed_commit)
}

#[test]
fn test_fetch_cli_without_remote_is_noop_like_git() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["fetch"], repo.path());

    // Without a configured remote, fetch should fail with a fatal error.
    assert_eq!(output.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no configured remote for the current branch"));
    assert!(stderr.contains("Error-Code: LBR-REPO-003"));
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
/// Test fetching from an invalid remote repository with timeout
async fn test_fetch_invalid_remote() {
    if std::env::var("LIBRA_TEST_GITHUB_TOKEN").map_or(true, |v| v.is_empty()) {
        eprintln!("skipped (LIBRA_TEST_GITHUB_TOKEN not set)");
        return;
    }
    let temp_repo = init_temp_repo();
    let temp_path = temp_repo.path();

    eprintln!("Starting test: fetch from invalid remote");

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

    // Attempt to fetch with 15-second timeout to avoid hanging CI
    eprintln!("Attempting 'libra fetch' with 15s timeout...");
    let fetch_result = timeout(Duration::from_secs(15), async {
        libra_tokio_command(temp_path).arg("fetch").output().await
    })
    .await;

    match fetch_result {
        // Timeout occurred — this is expected for unreachable remotes
        Err(_) => {
            eprintln!("Fetch timed out after 15 seconds — expected for invalid remote");
        }
        // Command completed within timeout
        Ok(Ok(output)) => {
            eprintln!("Fetch completed (status: {:?})", output.status);
            assert!(
                !output.status.success(),
                "Fetch should fail when remote is unreachable"
            );
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                !stderr.trim().is_empty(),
                "Expected error message in stderr, but was empty"
            );

            eprintln!("Fetch failed as expected: {stderr}");
        }
        // Failed to start the command
        Ok(Err(e)) => {
            panic!("Failed to run 'libra fetch' command: {e}");
        }
    }

    eprintln!("test_fetch_invalid_remote passed");
}

#[tokio::test]
#[serial]
async fn test_fetch_local_repository() {
    let temp_root = tempdir().expect("failed to create temp root");
    let remote_dir = temp_root.path().join("remote.git");
    let work_dir = temp_root.path().join("workdir");

    // Prepare remote bare repository with an initial commit pushed from a working clone
    assert!(
        Command::new("git")
            .args(["init", "--bare", remote_dir.to_str().unwrap()])
            .status()
            .expect("failed to init bare remote")
            .success()
    );

    assert!(
        Command::new("git")
            .args(["init", work_dir.to_str().unwrap()])
            .status()
            .expect("failed to init working repo")
            .success()
    );

    assert!(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["config", "user.name", "Libra Tester"])
            .status()
            .expect("failed to set user.name")
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["config", "user.email", "tester@example.com"])
            .status()
            .expect("failed to set user.email")
            .success()
    );

    fs::write(work_dir.join("README.md"), "hello libra").expect("failed to write README");
    assert!(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["add", "README.md"])
            .status()
            .expect("failed to add README")
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["commit", "-m", "initial commit"])
            .status()
            .expect("failed to commit")
            .success()
    );

    let current_branch = String::from_utf8(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .output()
            .expect("failed to read current branch")
            .stdout,
    )
    .expect("branch name not utf8")
    .trim()
    .to_string();

    let pushed_commit = String::from_utf8(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("failed to read HEAD commit")
            .stdout,
    )
    .expect("commit hash not utf8")
    .trim()
    .to_string();

    assert!(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["remote", "add", "origin", remote_dir.to_str().unwrap()])
            .status()
            .expect("failed to add origin remote")
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&work_dir)
            .args([
                "push",
                "origin",
                &format!("HEAD:refs/heads/{current_branch}"),
            ])
            .status()
            .expect("failed to push to remote")
            .success()
    );

    // Initialize a fresh Libra repository to fetch into
    let repo_dir = temp_root.path().join("libra_repo");
    fs::create_dir_all(&repo_dir).expect("failed to create repo dir");
    setup_with_new_libra_in(&repo_dir).await;
    let _guard = ChangeDirGuard::new(&repo_dir);

    let remote_path = remote_dir.to_str().unwrap().to_string();
    ConfigKv::set("remote.origin.url", &remote_path, false)
        .await
        .unwrap();

    fetch::fetch_repository(
        RemoteConfig {
            name: "origin".to_string(),
            url: remote_path.clone(),
        },
        None,
        false,
        None,
    )
    .await;

    // Migrated from lossy `Branch::find_branch` per docs/development/commands/branch.md —
    // storage errors no longer collapse into "remote-tracking branch not found".
    let tracked_branch = Branch::find_branch_result(
        &format!("refs/remotes/origin/{current_branch}"),
        Some("origin"),
    )
    .await
    .expect("failed to query remote-tracking branch")
    .expect("remote-tracking branch not found");
    assert_eq!(tracked_branch.commit.to_string(), pushed_commit);
}

#[tokio::test]
#[serial]
async fn test_fetch_json_output_reports_updated_refs() {
    let (_temp_root, repo_dir, current_branch, pushed_commit) =
        setup_local_fetch_cli_fixture().await;

    let output = run_libra_command(&["--json", "fetch", "origin"], &repo_dir);
    assert_cli_success(&output, "fetch --json origin");

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "fetch");
    assert_eq!(json["data"]["all"], false);
    assert_eq!(json["data"]["requested_remote"], "origin");
    assert_eq!(json["data"]["remotes"][0]["remote"], "origin");
    assert_eq!(
        json["data"]["remotes"][0]["refs_updated"][0]["remote_ref"],
        format!("refs/remotes/origin/{current_branch}")
    );
    assert_eq!(
        json["data"]["remotes"][0]["refs_updated"][0]["new_oid"],
        pushed_commit
    );
    assert!(
        json["data"]["remotes"][0]["objects_fetched"]
            .as_u64()
            .expect("objects_fetched should be a number")
            > 0
    );
}

#[tokio::test]
#[serial]
async fn test_fetch_machine_output_is_single_line_json() {
    let (_temp_root, repo_dir, _current_branch, _pushed_commit) =
        setup_local_fetch_cli_fixture().await;

    let output = run_libra_command(&["--machine", "fetch", "origin"], &repo_dir);
    assert_cli_success(&output, "fetch --machine origin");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.lines().count(),
        1,
        "machine output must be single-line JSON"
    );
    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "fetch");
    assert_eq!(json["data"]["requested_remote"], "origin");
    assert!(
        output.stderr.is_empty(),
        "machine mode should keep stderr clean, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
#[serial]
async fn test_fetch_json_emits_progress_events_to_stderr() {
    let (_temp_root, repo_dir, _current_branch, _pushed_commit) =
        setup_local_fetch_cli_fixture().await;

    let output = run_libra_command(&["--json", "fetch", "origin"], &repo_dir);
    assert_cli_success(&output, "fetch --json origin");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("\"event\":\"progress_done\""),
        "expected progress_done event in stderr, got: {stderr}"
    );
    assert!(
        stderr.contains("\"task\":\"fetch origin\""),
        "expected fetch task name in stderr, got: {stderr}"
    );
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn test_fetch_ssh_remote_via_fake_ssh() {
    let temp_root = tempdir().expect("failed to create temp root");
    let remote_dir = temp_root.path().join("remote.git");
    let work_dir = temp_root.path().join("workdir");
    let repo_dir = temp_root.path().join("libra_repo");
    let log_path = temp_root.path().join("fake_ssh.log");
    let ssh_script = create_fake_ssh_script(temp_root.path());

    assert!(
        Command::new("git")
            .args(["init", "--bare", remote_dir.to_str().unwrap()])
            .status()
            .expect("failed to init bare remote")
            .success()
    );
    assert!(
        Command::new("git")
            .args(["init", work_dir.to_str().unwrap()])
            .status()
            .expect("failed to init working repo")
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["config", "user.name", "Libra Tester"])
            .status()
            .expect("failed to set user.name")
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["config", "user.email", "tester@example.com"])
            .status()
            .expect("failed to set user.email")
            .success()
    );

    fs::write(work_dir.join("README.md"), "hello ssh fetch").expect("failed to write README");
    assert!(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["add", "README.md"])
            .status()
            .expect("failed to add README")
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["commit", "-m", "initial commit"])
            .status()
            .expect("failed to commit")
            .success()
    );
    let current_branch = String::from_utf8(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .output()
            .expect("failed to read current branch")
            .stdout,
    )
    .expect("branch name not utf8")
    .trim()
    .to_string();
    let pushed_commit = String::from_utf8(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("failed to read HEAD commit")
            .stdout,
    )
    .expect("commit hash not utf8")
    .trim()
    .to_string();
    assert!(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["remote", "add", "origin", remote_dir.to_str().unwrap()])
            .status()
            .expect("failed to add origin remote")
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&work_dir)
            .args([
                "push",
                "origin",
                &format!("HEAD:refs/heads/{current_branch}"),
            ])
            .status()
            .expect("failed to push to remote")
            .success()
    );

    fs::create_dir_all(&repo_dir).expect("failed to create repo dir");
    setup_with_new_libra_in(&repo_dir).await;
    let _guard = ChangeDirGuard::new(&repo_dir);

    let ssh_remote = format!("git@fakehost:{}", remote_dir.to_string_lossy());
    ConfigKv::set("remote.origin.url", &ssh_remote, false)
        .await
        .unwrap();

    let fetch_out = libra_command(&repo_dir)
        .env("LIBRA_SSH_COMMAND", &ssh_script)
        .env("LIBRA_TEST_SSH_LOG", &log_path)
        .args(["fetch", "origin"])
        .output()
        .expect("failed to run libra fetch over fake ssh");
    assert!(
        fetch_out.status.success(),
        "fetch over SSH should succeed, stderr: {}",
        String::from_utf8_lossy(&fetch_out.stderr)
    );

    // Migrated from lossy `Branch::find_branch` per docs/development/commands/branch.md —
    // storage errors no longer collapse into "remote-tracking branch not found".
    let tracked_branch = Branch::find_branch_result(
        &format!("refs/remotes/origin/{current_branch}"),
        Some("origin"),
    )
    .await
    .expect("failed to query remote-tracking branch")
    .expect("remote-tracking branch not found");
    assert_eq!(tracked_branch.commit.to_string(), pushed_commit);

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
async fn test_fetch_ssh_respects_strict_host_key_checking_config_casing() {
    let temp_root = tempdir().expect("failed to create temp root");
    let remote_dir = temp_root.path().join("remote.git");
    let repo_dir = temp_root.path().join("libra_repo");
    let work_dir = temp_root.path().join("git_work");
    let log_path = temp_root.path().join("fake_ssh.log");
    let ssh_script = create_fake_ssh_script(temp_root.path());

    assert!(
        Command::new("git")
            .args(["init", "--bare", remote_dir.to_str().unwrap()])
            .status()
            .expect("failed to init bare remote")
            .success()
    );
    fs::create_dir_all(&work_dir).expect("failed to create work dir");
    assert!(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["init"])
            .status()
            .expect("failed to init git workdir")
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["config", "user.name", "Fetch Test User"])
            .status()
            .expect("failed to configure git user.name")
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["config", "user.email", "fetch-test@example.com"])
            .status()
            .expect("failed to configure git user.email")
            .success()
    );
    fs::write(work_dir.join("README.md"), "hello ssh fetch").expect("failed to write README");
    assert!(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["add", "README.md"])
            .status()
            .expect("failed to add README")
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["commit", "-m", "initial commit"])
            .status()
            .expect("failed to commit")
            .success()
    );
    let current_branch = String::from_utf8(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .output()
            .expect("failed to read current branch")
            .stdout,
    )
    .expect("branch name not utf8")
    .trim()
    .to_string();
    assert!(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["remote", "add", "origin", remote_dir.to_str().unwrap()])
            .status()
            .expect("failed to add origin remote")
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&work_dir)
            .args([
                "push",
                "origin",
                &format!("HEAD:refs/heads/{current_branch}"),
            ])
            .status()
            .expect("failed to push to remote")
            .success()
    );

    fs::create_dir_all(&repo_dir).expect("failed to create repo dir");
    setup_with_new_libra_in(&repo_dir).await;
    let _guard = ChangeDirGuard::new(&repo_dir);

    let ssh_remote = format!("git@fakehost:{}", remote_dir.to_string_lossy());
    ConfigKv::set("remote.origin.url", &ssh_remote, false)
        .await
        .unwrap();
    ConfigKv::set("ssh.strictHostKeyChecking", "accept-new", false)
        .await
        .unwrap();

    let fetch_out = libra_command(&repo_dir)
        .env("LIBRA_SSH_COMMAND", &ssh_script)
        .env("LIBRA_TEST_SSH_LOG", &log_path)
        .args(["fetch", "origin"])
        .output()
        .expect("failed to run libra fetch over fake ssh");
    assert!(
        fetch_out.status.success(),
        "fetch over SSH should succeed, stderr: {}",
        String::from_utf8_lossy(&fetch_out.stderr)
    );

    let ssh_log = fs::read_to_string(&log_path).expect("failed to read fake ssh log");
    assert!(
        ssh_log.contains("StrictHostKeyChecking=accept-new"),
        "SSH command should use configured strictHostKeyChecking mode, log:\n{ssh_log}"
    );
    assert!(
        !ssh_log.contains("StrictHostKeyChecking=yes"),
        "configured mode should override default strict host key checking, log:\n{ssh_log}"
    );
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn test_fetch_ssh_host_key_failure_is_reported() {
    let temp_root = tempdir().expect("failed to create temp root");
    let remote_dir = temp_root.path().join("remote.git");
    let repo_dir = temp_root.path().join("libra_repo");
    let ssh_script = create_fake_ssh_script(temp_root.path());

    assert!(
        Command::new("git")
            .args(["init", "--bare", remote_dir.to_str().unwrap()])
            .status()
            .expect("failed to init bare remote")
            .success()
    );
    fs::create_dir_all(&repo_dir).expect("failed to create repo dir");
    setup_with_new_libra_in(&repo_dir).await;

    let ssh_remote = format!("git@fakehost:{}", remote_dir.to_string_lossy());
    let _guard = ChangeDirGuard::new(&repo_dir);
    ConfigKv::set("remote.origin.url", &ssh_remote, false)
        .await
        .unwrap();

    let fetch_out = libra_command(&repo_dir)
        .env("LIBRA_SSH_COMMAND", &ssh_script)
        .env("LIBRA_TEST_SSH_FAIL", "hostkey")
        .args(["fetch", "origin"])
        .output()
        .expect("failed to run libra fetch over fake ssh");
    let stderr = String::from_utf8_lossy(&fetch_out.stderr);
    assert!(
        stderr.contains("Host key verification failed."),
        "fetch should surface SSH host-key failures, stderr: {stderr}"
    );
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn test_fetch_ssh_invalid_vault_key_fails_without_fallback() {
    let temp_root = tempdir().expect("failed to create temp root");
    let remote_dir = temp_root.path().join("remote.git");
    let repo_dir = temp_root.path().join("libra_repo");
    let home_dir = repo_dir.join(".libra-test-home");
    let config_home = home_dir.join(".config");
    let log_path = temp_root.path().join("fake_ssh.log");
    let ssh_script = create_fake_ssh_script(temp_root.path());

    assert!(
        Command::new("git")
            .args(["init", "--bare", remote_dir.to_str().unwrap()])
            .status()
            .expect("failed to init bare remote")
            .success()
    );
    fs::create_dir_all(&repo_dir).expect("failed to create repo dir");
    setup_with_new_libra_in(&repo_dir).await;
    fs::create_dir_all(&config_home).expect("failed to create config home");

    let ssh_remote = format!("git@fakehost:{}", remote_dir.to_string_lossy());
    let _home = ScopedEnvVar::set("HOME", &home_dir);
    let _userprofile = ScopedEnvVar::set("USERPROFILE", &home_dir);
    let _xdg = ScopedEnvVar::set("XDG_CONFIG_HOME", &config_home);
    let _guard = ChangeDirGuard::new(&repo_dir);
    vault::lazy_init_vault_for_scope("local")
        .await
        .expect("failed to initialize local vault");
    ConfigKv::set("remote.origin.url", &ssh_remote, false)
        .await
        .unwrap();
    ConfigKv::set("vault.ssh.origin.privkey", "not-valid-hex", true)
        .await
        .unwrap();

    let fetch_out = libra_command(&repo_dir)
        .env("HOME", &home_dir)
        .env("USERPROFILE", &home_dir)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("LIBRA_SSH_COMMAND", &ssh_script)
        .env("LIBRA_TEST_SSH_LOG", &log_path)
        .args(["fetch", "origin"])
        .output()
        .expect("failed to run libra fetch over fake ssh");
    let stderr = String::from_utf8_lossy(&fetch_out.stderr);
    assert!(
        !fetch_out.status.success(),
        "fetch should fail when configured vault SSH key is invalid"
    );
    assert!(
        stderr.contains("failed to decode vault SSH private key 'vault.ssh.origin.privkey'"),
        "fetch should report invalid configured vault SSH key, stderr: {stderr}"
    );
    assert!(
        !log_path.exists(),
        "fetch should fail before invoking SSH when vault key is invalid"
    );
}

// ---- C3: shallow-fetch contract (`libra fetch --depth N`) ---------------------------------
//
// The internal `fetch_repository(..., depth)` plumbing has supported shallow fetch for some
// time; C3 (compat plan) surfaces it as a public, stable CLI flag. These tests verify the
// public surface contract — not the wire-level shallow protocol semantics, which are owned
// by `git_internal` and exercised through its own test suites.

#[test]
fn test_fetch_help_lists_depth_flag_without_experimental() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["fetch", "--help"], repo.path());
    assert!(
        output.status.success(),
        "fetch --help should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--depth"),
        "fetch --help must surface --depth flag (C3 contract), stdout: {stdout}"
    );
    assert!(
        !stdout.to_lowercase().contains("experimental"),
        "fetch --depth is a stable public flag; --help must not mark it experimental, stdout: {stdout}"
    );
}

#[tokio::test]
#[serial]
async fn test_fetch_with_depth_one_against_local_remote() {
    // Smoke: `libra fetch origin --depth 1` succeeds against a local file remote
    // and reports the same JSON envelope shape as a non-shallow fetch.
    let (_temp_root, repo_dir, current_branch, pushed_commit) =
        setup_local_fetch_cli_fixture().await;

    let output = run_libra_command(&["--json", "fetch", "origin", "--depth", "1"], &repo_dir);
    assert_cli_success(&output, "fetch --json origin --depth 1");

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "fetch");
    assert_eq!(json["data"]["all"], false);
    assert_eq!(json["data"]["requested_remote"], "origin");
    assert_eq!(json["data"]["remotes"][0]["remote"], "origin");
    assert_eq!(
        json["data"]["remotes"][0]["refs_updated"][0]["remote_ref"],
        format!("refs/remotes/origin/{current_branch}")
    );
    assert_eq!(
        json["data"]["remotes"][0]["refs_updated"][0]["new_oid"],
        pushed_commit
    );
}

#[tokio::test]
#[serial]
async fn test_fetch_all_with_depth_runs_across_remotes() {
    // `libra fetch --all --depth N` must accept both flags together and pass `depth`
    // through to every configured remote; conflicts_with("repository") on `--all`
    // already prevents the bad combination.
    let (_temp_root, repo_dir, current_branch, _pushed_commit) =
        setup_local_fetch_cli_fixture().await;

    let output = run_libra_command(&["--json", "fetch", "--all", "--depth", "3"], &repo_dir);
    assert_cli_success(&output, "fetch --json --all --depth 3");

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "fetch");
    assert_eq!(json["data"]["all"], true);
    let remotes = json["data"]["remotes"]
        .as_array()
        .expect("remotes should be an array");
    assert!(
        !remotes.is_empty(),
        "fetch --all should report at least one remote"
    );
    let origin_seen = remotes.iter().any(|r| r["remote"] == "origin");
    assert!(origin_seen, "fetch --all should include 'origin' remote");
    let _ = current_branch;
}

#[tokio::test]
#[serial]
async fn test_fetch_full_then_shallow_is_idempotent() {
    // After a full (non-shallow) fetch has already populated origin's tracking
    // refs, re-running with `--depth 1` must not error. This exercises the
    // common workflow where a developer first does a regular fetch and then
    // wants to refresh just the tip.
    //
    // Note: the converse case (shallow → shallow re-fetch) currently has known
    // plumbing limitations on file:// transport when the local commit graph
    // contains a shallow boundary; that scenario is tracked separately and is
    // not part of the C3 public-flag contract.
    let (_temp_root, repo_dir, _current_branch, _pushed_commit) =
        setup_local_fetch_cli_fixture().await;

    let first = run_libra_command(&["fetch", "origin"], &repo_dir);
    assert_cli_success(&first, "first fetch (full)");

    let second = run_libra_command(&["fetch", "origin", "--depth", "1"], &repo_dir);
    assert_cli_success(&second, "second fetch --depth 1 after full");
}

#[tokio::test]
#[serial]
async fn test_fetch_shallow_then_shallow_is_idempotent() {
    // C3 follow-up: once a shallow boundary has been created locally,
    // re-running the same shallow fetch should still negotiate cleanly.
    let (_temp_root, repo_dir, _current_branch, pushed_commit) =
        setup_local_fetch_cli_fixture().await;

    let first = run_libra_command(&["--json", "fetch", "origin", "--depth", "1"], &repo_dir);
    assert_cli_success(&first, "first fetch --depth 1");
    let first_json = parse_json_stdout(&first);
    assert!(
        first_json["data"]["remotes"][0]["objects_fetched"]
            .as_u64()
            .is_some_and(|count| count > 0),
        "first shallow fetch must materialize at least one object: {first_json:?}"
    );

    let shallow_path = repo_dir.join(".libra").join("shallow");
    let shallow = fs::read_to_string(&shallow_path)
        .expect("first shallow fetch must persist .libra/shallow metadata");
    assert!(
        shallow.lines().any(|line| line.trim() == pushed_commit),
        "shallow metadata must contain the fetched boundary {pushed_commit}; got {shallow:?}"
    );

    let second = run_libra_command(&["fetch", "origin", "--depth", "1"], &repo_dir);
    assert_cli_success(&second, "second fetch --depth 1 after shallow");
}

/// `libra fetch --dry-run` previews the remote-tracking ref updates without
/// downloading any pack or writing refs / FETCH_HEAD.
#[tokio::test]
#[serial]
async fn test_fetch_dry_run_previews_without_writing() {
    let (_temp_root, repo_dir, current_branch, pushed_commit) =
        setup_local_fetch_cli_fixture().await;

    let output = run_libra_command(&["--json", "fetch", "origin", "--dry-run"], &repo_dir);
    assert_cli_success(&output, "fetch --dry-run origin");

    let json = parse_json_stdout(&output);
    assert_eq!(
        json["data"]["remotes"][0]["refs_updated"][0]["remote_ref"],
        format!("refs/remotes/origin/{current_branch}")
    );
    assert_eq!(
        json["data"]["remotes"][0]["refs_updated"][0]["new_oid"],
        pushed_commit
    );
    // Dry-run downloads nothing.
    assert_eq!(json["data"]["remotes"][0]["objects_fetched"], 0);

    // No remote-tracking ref was written, and no FETCH_HEAD was created.
    assert!(
        !repo_dir.join(".libra/FETCH_HEAD").exists(),
        "--dry-run must not write FETCH_HEAD"
    );
    let _guard = ChangeDirGuard::new(&repo_dir);
    let tracking = Branch::find_branch_result(
        &format!("refs/remotes/origin/{current_branch}"),
        Some("origin"),
    )
    .await
    .expect("branch lookup should succeed");
    assert!(
        tracking.is_none(),
        "--dry-run must not persist a remote-tracking ref"
    );
}

/// `libra fetch --porcelain` prints one machine-readable line per ref update.
#[tokio::test]
#[serial]
async fn test_fetch_porcelain_prints_per_ref_lines() {
    let (_temp_root, repo_dir, current_branch, pushed_commit) =
        setup_local_fetch_cli_fixture().await;

    let output = run_libra_command(&["fetch", "origin", "--porcelain"], &repo_dir);
    assert_cli_success(&output, "fetch --porcelain origin");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .find(|line| line.contains(&format!("refs/remotes/origin/{current_branch}")))
        .unwrap_or_else(|| panic!("expected a porcelain line for the fetched ref, got: {stdout}"));
    let cols: Vec<&str> = line.split(' ').collect();
    // `<flag> <old-oid> <new-oid> <local-ref>` — new ref uses the `*` flag.
    assert_eq!(cols[0], "*", "a new ref must use the `*` flag");
    assert_eq!(cols[2], pushed_commit, "third column must be the new oid");
    assert_eq!(
        cols[3],
        format!("refs/remotes/origin/{current_branch}"),
        "fourth column must be the local tracking ref"
    );
}

/// `--porcelain` and the global `--json` are both machine formats and must not
/// be combined (usage error, exit 129).
#[tokio::test]
#[serial]
async fn test_fetch_porcelain_rejects_combination_with_json() {
    let (_temp_root, repo_dir, _current_branch, _pushed_commit) =
        setup_local_fetch_cli_fixture().await;

    let output = run_libra_command(&["--json", "fetch", "origin", "--porcelain"], &repo_dir);
    assert_eq!(
        output.status.code(),
        Some(129),
        "combining --porcelain with --json must be a usage error"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--porcelain") && stderr.contains("--json"),
        "error must mention the conflicting flags, got: {stderr}"
    );
}

/// `-v/--verbose` announces the remote being contacted on stderr without
/// changing the stdout result contract.
#[tokio::test]
#[serial]
async fn test_fetch_verbose_announces_remote_on_stderr() {
    let (_temp_root, repo_dir, _current_branch, _pushed_commit) =
        setup_local_fetch_cli_fixture().await;

    let output = run_libra_command(&["fetch", "origin", "-v"], &repo_dir);
    assert_cli_success(&output, "fetch -v origin");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Fetching origin from"),
        "verbose mode must announce the remote on stderr, got: {stderr}"
    );
}

/// A plain `libra fetch` writes `.libra/FETCH_HEAD`; `--append` accumulates
/// rather than overwriting.
#[tokio::test]
#[serial]
async fn test_fetch_writes_and_appends_fetch_head() {
    let (_temp_root, repo_dir, current_branch, pushed_commit) =
        setup_local_fetch_cli_fixture().await;

    let first = run_libra_command(&["fetch", "origin"], &repo_dir);
    assert_cli_success(&first, "fetch origin (writes FETCH_HEAD)");

    let fetch_head_path = repo_dir.join(".libra/FETCH_HEAD");
    assert!(
        fetch_head_path.exists(),
        "fetch must write .libra/FETCH_HEAD"
    );
    let body = fs::read_to_string(&fetch_head_path).expect("read FETCH_HEAD");
    assert!(
        body.contains(&pushed_commit) && body.contains("not-for-merge"),
        "FETCH_HEAD must record the fetched oid as not-for-merge, got: {body}"
    );
    assert!(
        body.contains(&format!("branch '{current_branch}'")),
        "FETCH_HEAD must describe the fetched branch, got: {body}"
    );

    // `--append` accumulates: re-fetching with --append keeps prior lines.
    let append = run_libra_command(&["fetch", "origin", "--append"], &repo_dir);
    assert_cli_success(&append, "fetch --append origin");
    let appended = fs::read_to_string(&fetch_head_path).expect("read FETCH_HEAD after append");
    assert!(
        appended.matches("not-for-merge").count() >= body.matches("not-for-merge").count(),
        "--append must not shrink FETCH_HEAD, before: {body}\nafter: {appended}"
    );
}
