//! Tests fetch command behavior for remote ref updates and pack retrieval flows.
//!
//! **Layer:** L1 (most tests). `test_fetch_invalid_remote` is L2 — requires `LIBRA_TEST_GITHUB_TOKEN`.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    str::FromStr,
    time::Duration,
};

use git_internal::{hash::ObjectHash, internal::object::types::ObjectType};
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
    utils::{
        client_storage::ClientStorage,
        path,
        test::{ChangeDirGuard, setup_with_new_libra_in},
    },
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

    // Migrated from lossy `Branch::find_branch` per docs/improvement/branch.md —
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

    // Migrated from lossy `Branch::find_branch` per docs/improvement/branch.md —
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

/// `--recurse-submodules` is declared only to produce a friendly usage error
/// (exit 129) instead of a clap "unknown argument" (exit 2).
#[test]
fn test_fetch_recurse_submodules_declined_exits_129() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["fetch", "--recurse-submodules=no"], repo.path());
    assert_eq!(
        output.status.code(),
        Some(129),
        "declined flag must exit 129: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("submodule recursion"),
        "stderr should explain the decline: {stderr}"
    );
}

/// `--porcelain` and `--json` are both machine formats and must not combine
/// (a usage error, exit 129).
#[test]
fn test_fetch_porcelain_json_mutually_exclusive_exits_129() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["--json", "fetch", "--porcelain"], repo.path());
    assert_eq!(
        output.status.code(),
        Some(129),
        "--porcelain + --json must be a usage error: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// An unparseable `--shallow-since` date is a usage error (129), caught at the
/// command layer before any network activity.
#[test]
fn test_fetch_shallow_since_invalid_date_exits_129() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(
        &["fetch", "origin", "--shallow-since=definitely-not-a-date"],
        repo.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(129),
        "invalid --shallow-since must be a usage error: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_fetch_depth_and_deepen_conflict() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(
        &["fetch", "origin", "--depth", "1", "--deepen", "1"],
        repo.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(129),
        "--depth and --deepen must be rejected as a usage error: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// End-to-end `fetch --prune`: a remote-tracking branch whose remote counterpart
/// was deleted is removed locally; live branches and `refs/heads/*` are kept.
#[tokio::test]
#[serial]
async fn test_fetch_prune_removes_stale_remote_tracking() {
    let temp = tempdir().expect("temp root");
    let remote_dir = temp.path().join("remote.git");
    let work_dir = temp.path().join("work");

    let git = |dir: Option<&Path>, args: &[&str]| {
        let mut cmd = Command::new("git");
        if let Some(d) = dir {
            cmd.current_dir(d);
        }
        assert!(
            cmd.args(args).status().expect("git command").success(),
            "git {args:?} failed"
        );
    };

    git(None, &["init", "--bare", remote_dir.to_str().unwrap()]);
    git(None, &["init", work_dir.to_str().unwrap()]);
    git(Some(&work_dir), &["config", "user.name", "Libra Tester"]);
    git(
        Some(&work_dir),
        &["config", "user.email", "tester@example.com"],
    );
    fs::write(work_dir.join("README.md"), "hello").expect("write README");
    git(Some(&work_dir), &["add", "README.md"]);
    git(Some(&work_dir), &["commit", "-m", "initial"]);
    // Push two branches: main (the current branch) and feature.
    let current = String::from_utf8(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .output()
            .expect("rev-parse")
            .stdout,
    )
    .expect("utf8")
    .trim()
    .to_string();
    git(
        Some(&work_dir),
        &["remote", "add", "origin", remote_dir.to_str().unwrap()],
    );
    git(
        Some(&work_dir),
        &["push", "origin", &format!("HEAD:refs/heads/{current}")],
    );
    git(
        Some(&work_dir),
        &["push", "origin", "HEAD:refs/heads/feature"],
    );

    // Fetch into a fresh Libra repo so both tracking branches exist.
    let repo_dir = temp.path().join("libra_repo");
    fs::create_dir_all(&repo_dir).expect("repo dir");
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

    assert!(
        Branch::find_branch_result("refs/remotes/origin/feature", Some("origin"))
            .await
            .expect("query feature")
            .is_some(),
        "feature tracking branch must exist before prune"
    );

    // Delete `feature` on the remote, then prune.
    git(
        Some(&remote_dir),
        &["update-ref", "-d", "refs/heads/feature"],
    );
    let output = run_libra_command(&["fetch", "origin", "--prune"], &repo_dir);
    assert_cli_success(&output, "fetch --prune");

    assert!(
        Branch::find_branch_result("refs/remotes/origin/feature", Some("origin"))
            .await
            .expect("query feature after prune")
            .is_none(),
        "stale feature tracking branch must be pruned"
    );
    assert!(
        Branch::find_branch_result(&format!("refs/remotes/origin/{current}"), Some("origin"))
            .await
            .expect("query main after prune")
            .is_some(),
        "live tracking branch must survive prune"
    );
}

/// `fetch --dry-run` previews ref updates without writing them: the local
/// tracking ref stays at its old commit even though the remote advanced.
#[tokio::test]
#[serial]
async fn test_fetch_dry_run_makes_no_ref_writes() {
    let temp = tempdir().expect("temp root");
    let remote_dir = temp.path().join("remote.git");
    let work_dir = temp.path().join("work");

    let git = |dir: Option<&Path>, args: &[&str]| {
        let mut cmd = Command::new("git");
        if let Some(d) = dir {
            cmd.current_dir(d);
        }
        assert!(
            cmd.args(args).status().expect("git command").success(),
            "git {args:?} failed"
        );
    };
    let git_out = |dir: &Path, args: &[&str]| -> String {
        String::from_utf8(
            Command::new("git")
                .current_dir(dir)
                .args(args)
                .output()
                .expect("git output")
                .stdout,
        )
        .expect("utf8")
        .trim()
        .to_string()
    };

    git(None, &["init", "--bare", remote_dir.to_str().unwrap()]);
    git(None, &["init", work_dir.to_str().unwrap()]);
    git(Some(&work_dir), &["config", "user.name", "Libra Tester"]);
    git(
        Some(&work_dir),
        &["config", "user.email", "tester@example.com"],
    );
    fs::write(work_dir.join("a.txt"), "one").expect("write");
    git(Some(&work_dir), &["add", "a.txt"]);
    git(Some(&work_dir), &["commit", "-m", "c1"]);
    let current = git_out(&work_dir, &["rev-parse", "--abbrev-ref", "HEAD"]);
    git(
        Some(&work_dir),
        &["remote", "add", "origin", remote_dir.to_str().unwrap()],
    );
    git(
        Some(&work_dir),
        &["push", "origin", &format!("HEAD:refs/heads/{current}")],
    );

    let repo_dir = temp.path().join("libra_repo");
    fs::create_dir_all(&repo_dir).expect("repo dir");
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
    let commit1 = git_out(&work_dir, &["rev-parse", "HEAD"]);

    // Advance the remote.
    fs::write(work_dir.join("a.txt"), "two").expect("write");
    git(Some(&work_dir), &["add", "a.txt"]);
    git(Some(&work_dir), &["commit", "-m", "c2"]);
    git(
        Some(&work_dir),
        &["push", "origin", &format!("HEAD:refs/heads/{current}")],
    );

    // Dry-run must not move the tracking ref.
    let output = run_libra_command(&["fetch", "origin", "--dry-run"], &repo_dir);
    assert_cli_success(&output, "fetch --dry-run");
    let tracked =
        Branch::find_branch_result(&format!("refs/remotes/origin/{current}"), Some("origin"))
            .await
            .expect("query tracking")
            .expect("tracking exists");
    assert_eq!(
        tracked.commit.to_string(),
        commit1,
        "dry-run must not advance the tracking ref"
    );
}

/// `fetch` writes `.libra/FETCH_HEAD` by default; `--append` accumulates rather
/// than overwriting.
#[tokio::test]
#[serial]
async fn test_fetch_writes_and_appends_fetch_head() {
    let temp = tempdir().expect("temp root");
    let remote_dir = temp.path().join("remote.git");
    let work_dir = temp.path().join("work");

    let git = |dir: Option<&Path>, args: &[&str]| {
        let mut cmd = Command::new("git");
        if let Some(d) = dir {
            cmd.current_dir(d);
        }
        assert!(
            cmd.args(args).status().expect("git command").success(),
            "git {args:?} failed"
        );
    };

    git(None, &["init", "--bare", remote_dir.to_str().unwrap()]);
    git(None, &["init", work_dir.to_str().unwrap()]);
    git(Some(&work_dir), &["config", "user.name", "Libra Tester"]);
    git(
        Some(&work_dir),
        &["config", "user.email", "tester@example.com"],
    );
    fs::write(work_dir.join("a.txt"), "one").expect("write");
    git(Some(&work_dir), &["add", "a.txt"]);
    git(Some(&work_dir), &["commit", "-m", "c1"]);
    let current = String::from_utf8(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .output()
            .expect("rev-parse")
            .stdout,
    )
    .expect("utf8")
    .trim()
    .to_string();
    git(
        Some(&work_dir),
        &["remote", "add", "origin", remote_dir.to_str().unwrap()],
    );
    git(
        Some(&work_dir),
        &["push", "origin", &format!("HEAD:refs/heads/{current}")],
    );

    let repo_dir = temp.path().join("libra_repo");
    fs::create_dir_all(&repo_dir).expect("repo dir");
    setup_with_new_libra_in(&repo_dir).await;
    let remote_path = remote_dir.to_str().unwrap().to_string();
    {
        let _guard = ChangeDirGuard::new(&repo_dir);
        ConfigKv::set("remote.origin.url", &remote_path, false)
            .await
            .unwrap();
    }

    // Default fetch writes FETCH_HEAD with the fetched branch line.
    let first = run_libra_command(&["fetch", "origin"], &repo_dir);
    assert_cli_success(&first, "fetch writes FETCH_HEAD");
    let fetch_head_path = repo_dir.join(".libra").join("FETCH_HEAD");
    let contents = fs::read_to_string(&fetch_head_path).expect("FETCH_HEAD written");
    assert!(
        contents.contains("not-for-merge") && contents.contains(&format!("branch '{current}' of")),
        "FETCH_HEAD should record the fetched branch: {contents:?}"
    );
    let first_line_count = contents.lines().count();

    // A re-fetch with --append must not shrink FETCH_HEAD (appends, never
    // truncates), even when nothing changed.
    let again = run_libra_command(&["fetch", "origin", "--append"], &repo_dir);
    assert_cli_success(&again, "fetch --append");
    let appended = fs::read_to_string(&fetch_head_path).expect("FETCH_HEAD still present");
    assert!(
        appended.lines().count() >= first_line_count,
        "--append must not truncate FETCH_HEAD: before={first_line_count}, after={}",
        appended.lines().count()
    );
}

/// `--verbose`/`-v` announces the remote being contacted on stderr (printed
/// before the connection is attempted, so a bad URL still shows it).
#[tokio::test]
#[serial]
async fn test_fetch_verbose_announces_remote_on_stderr() {
    let temp = tempdir().expect("temp root");
    let repo_dir = temp.path().join("repo");
    fs::create_dir_all(&repo_dir).expect("repo dir");
    setup_with_new_libra_in(&repo_dir).await;
    {
        let _guard = ChangeDirGuard::new(&repo_dir);
        ConfigKv::set("remote.origin.url", "/nonexistent/remote.git", false)
            .await
            .unwrap();
    }

    let output = run_libra_command(&["fetch", "origin", "-v"], &repo_dir);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Fetching origin from /nonexistent/remote.git"),
        "verbose must announce the remote on stderr: {stderr}"
    );
}

/// Run a `git` command in `cwd`, asserting it succeeds.
fn git_in(cwd: &Path, args: &[&str]) {
    assert!(
        Command::new("git")
            .current_dir(cwd)
            .args(args)
            .status()
            .unwrap_or_else(|error| panic!("git {args:?} failed to spawn: {error}"))
            .success(),
        "git {args:?} failed"
    );
}

/// Run a `git` command in `cwd`, returning its trimmed stdout.
fn git_out(cwd: &Path, args: &[&str]) -> String {
    String::from_utf8(
        Command::new("git")
            .current_dir(cwd)
            .args(args)
            .output()
            .unwrap_or_else(|error| panic!("git {args:?} failed to spawn: {error}"))
            .stdout,
    )
    .expect("git output not utf8")
    .trim()
    .to_string()
}

/// A local bare git remote carrying one branch plus a lightweight and an
/// annotated tag, paired with a fresh Libra repo configured to fetch it.
struct FetchTagFixture {
    _temp_root: TempDir,
    repo_dir: PathBuf,
    work_dir: PathBuf,
    /// The commit the lightweight tag points at (`refs/tags/lightweight-tag`).
    lightweight_target: String,
    /// The tag-object hash the annotated tag points at (`refs/tags/annotated-tag`).
    annotated_target: String,
}

async fn setup_fetch_fixture_with_tags() -> FetchTagFixture {
    let temp_root = tempdir().expect("failed to create temp root");
    let remote_dir = temp_root.path().join("remote.git");
    let work_dir = temp_root.path().join("workdir");
    let repo_dir = temp_root.path().join("libra_repo");

    git_in(
        temp_root.path(),
        &["init", "--bare", remote_dir.to_str().unwrap()],
    );
    git_in(temp_root.path(), &["init", work_dir.to_str().unwrap()]);
    git_in(&work_dir, &["config", "user.name", "Libra Tester"]);
    git_in(&work_dir, &["config", "user.email", "tester@example.com"]);
    // Neutralise any host-global signing config so plain `git tag` stays
    // lightweight and annotated tags are created without a GPG key.
    git_in(&work_dir, &["config", "tag.gpgSign", "false"]);
    git_in(&work_dir, &["config", "tag.forceSignAnnotated", "false"]);
    git_in(&work_dir, &["config", "commit.gpgSign", "false"]);

    fs::write(work_dir.join("README.md"), "hello libra").expect("failed to write README");
    git_in(&work_dir, &["add", "README.md"]);
    git_in(&work_dir, &["commit", "-m", "initial commit"]);
    git_in(&work_dir, &["tag", "lightweight-tag"]);
    git_in(
        &work_dir,
        &["tag", "-a", "annotated-tag", "-m", "annotated message"],
    );

    let current_branch = git_out(&work_dir, &["rev-parse", "--abbrev-ref", "HEAD"]);
    let lightweight_target = git_out(&work_dir, &["rev-parse", "refs/tags/lightweight-tag"]);
    let annotated_target = git_out(&work_dir, &["rev-parse", "refs/tags/annotated-tag"]);

    git_in(
        &work_dir,
        &["remote", "add", "origin", remote_dir.to_str().unwrap()],
    );
    git_in(
        &work_dir,
        &[
            "push",
            "origin",
            &format!("HEAD:refs/heads/{current_branch}"),
        ],
    );
    git_in(&work_dir, &["push", "origin", "--tags"]);

    fs::create_dir_all(&repo_dir).expect("failed to create repo dir");
    setup_with_new_libra_in(&repo_dir).await;
    {
        let _guard = ChangeDirGuard::new(&repo_dir);
        ConfigKv::set("remote.origin.url", remote_dir.to_str().unwrap(), false)
            .await
            .unwrap();
    }

    FetchTagFixture {
        _temp_root: temp_root,
        repo_dir,
        work_dir,
        lightweight_target,
        annotated_target,
    }
}

/// `--tags` imports both lightweight and annotated tags into `refs/tags/*`,
/// pulling the annotated tag's object into the local store.
#[tokio::test]
#[serial]
async fn test_fetch_tags_imports_annotated_and_lightweight() {
    let fixture = setup_fetch_fixture_with_tags().await;

    let output = run_libra_command(&["fetch", "origin", "--tags"], &fixture.repo_dir);
    assert_cli_success(&output, "fetch origin --tags");

    let show = run_libra_command(&["show-ref", "--tags"], &fixture.repo_dir);
    assert_cli_success(&show, "show-ref --tags");
    let refs = String::from_utf8_lossy(&show.stdout);

    let lightweight = refs
        .lines()
        .find(|line| line.contains("refs/tags/lightweight-tag"))
        .unwrap_or_else(|| panic!("lightweight tag not imported: {refs}"));
    assert!(
        lightweight.contains(&fixture.lightweight_target),
        "lightweight tag points at the wrong object: {lightweight}"
    );

    let annotated = refs
        .lines()
        .find(|line| line.contains("refs/tags/annotated-tag"))
        .unwrap_or_else(|| panic!("annotated tag not imported: {refs}"));
    assert!(
        annotated.contains(&fixture.annotated_target),
        "annotated tag points at the wrong object: {annotated}"
    );

    // The annotated tag's object itself must have been fetched into the local
    // store (not just the ref written). Check the stored object type directly,
    // since `cat-file -t` peels an annotated tag hash to its commit.
    {
        let _guard = ChangeDirGuard::new(&fixture.repo_dir);
        let storage = ClientStorage::init(path::objects());
        let hash =
            ObjectHash::from_str(&fixture.annotated_target).expect("valid annotated tag hash");
        let obj_type = storage
            .get_object_type(&hash)
            .expect("annotated tag object must be present in the local store");
        assert_eq!(
            obj_type,
            ObjectType::Tag,
            "fetched annotated tag should be stored as a tag object"
        );
    }
}

/// `--no-tags` imports no tags even when the remote advertises them.
#[tokio::test]
#[serial]
async fn test_fetch_no_tags_skips_all_tags() {
    let fixture = setup_fetch_fixture_with_tags().await;

    let output = run_libra_command(&["fetch", "origin", "--no-tags"], &fixture.repo_dir);
    assert_cli_success(&output, "fetch origin --no-tags");

    let show = run_libra_command(&["show-ref", "--tags"], &fixture.repo_dir);
    let refs = String::from_utf8_lossy(&show.stdout);
    assert!(
        !refs.contains("refs/tags/"),
        "--no-tags must not import any tag, got: {refs}"
    );
}

/// An existing local tag is preserved on a subsequent `--tags` fetch even when
/// the remote moved the tag (tags are immutable without `--force`).
#[tokio::test]
#[serial]
async fn test_fetch_existing_tag_not_clobbered_without_force() {
    let fixture = setup_fetch_fixture_with_tags().await;

    // First fetch imports the annotated tag at its original target.
    let first = run_libra_command(&["fetch", "origin", "--tags"], &fixture.repo_dir);
    assert_cli_success(&first, "fetch origin --tags (first)");

    // Move the annotated tag on the remote to a brand-new commit.
    fs::write(fixture.work_dir.join("CHANGE.md"), "second").expect("failed to write change");
    git_in(&fixture.work_dir, &["add", "CHANGE.md"]);
    git_in(&fixture.work_dir, &["commit", "-m", "second commit"]);
    git_in(
        &fixture.work_dir,
        &["tag", "-f", "-a", "annotated-tag", "-m", "moved"],
    );
    let moved_target = git_out(&fixture.work_dir, &["rev-parse", "refs/tags/annotated-tag"]);
    assert_ne!(
        moved_target, fixture.annotated_target,
        "the remote tag should have moved to a new object"
    );
    git_in(
        &fixture.work_dir,
        &["push", "-f", "origin", "refs/tags/annotated-tag"],
    );

    // Second fetch must leave the existing local tag untouched.
    let second = run_libra_command(&["fetch", "origin", "--tags"], &fixture.repo_dir);
    assert_cli_success(&second, "fetch origin --tags (second)");

    let show = run_libra_command(&["show-ref", "--tags"], &fixture.repo_dir);
    let refs = String::from_utf8_lossy(&show.stdout);
    let annotated = refs
        .lines()
        .find(|line| line.contains("refs/tags/annotated-tag"))
        .unwrap_or_else(|| panic!("annotated tag missing after second fetch: {refs}"));
    assert!(
        annotated.contains(&fixture.annotated_target),
        "existing tag must be preserved, not clobbered: {annotated}"
    );
    assert!(
        !annotated.contains(&moved_target),
        "existing tag must not move to the remote's new target: {annotated}"
    );
}

/// `--force` overwrites an existing local tag when the remote moved it (tags
/// are immutable only without `--force`).
#[tokio::test]
#[serial]
async fn test_fetch_force_clobbers_existing_tag() {
    let fixture = setup_fetch_fixture_with_tags().await;

    // First fetch imports the annotated tag at its original target.
    let first = run_libra_command(&["fetch", "origin", "--tags"], &fixture.repo_dir);
    assert_cli_success(&first, "fetch origin --tags (first)");

    // Move the annotated tag on the remote to a brand-new commit.
    fs::write(fixture.work_dir.join("CHANGE.md"), "second").expect("failed to write change");
    git_in(&fixture.work_dir, &["add", "CHANGE.md"]);
    git_in(&fixture.work_dir, &["commit", "-m", "second commit"]);
    git_in(
        &fixture.work_dir,
        &["tag", "-f", "-a", "annotated-tag", "-m", "moved"],
    );
    let moved_target = git_out(&fixture.work_dir, &["rev-parse", "refs/tags/annotated-tag"]);
    assert_ne!(
        moved_target, fixture.annotated_target,
        "the remote tag should have moved to a new object"
    );
    git_in(
        &fixture.work_dir,
        &["push", "-f", "origin", "refs/tags/annotated-tag"],
    );

    // With --force, the second fetch clobbers the existing local tag.
    let second = run_libra_command(&["fetch", "origin", "--tags", "--force"], &fixture.repo_dir);
    assert_cli_success(&second, "fetch origin --tags --force");

    let show = run_libra_command(&["show-ref", "--tags"], &fixture.repo_dir);
    let refs = String::from_utf8_lossy(&show.stdout);
    let annotated = refs
        .lines()
        .find(|line| line.contains("refs/tags/annotated-tag"))
        .unwrap_or_else(|| panic!("annotated tag missing after forced fetch: {refs}"));
    assert!(
        annotated.contains(&moved_target),
        "--force should clobber the tag to the remote's new target: {annotated}"
    );
    assert!(
        !annotated.contains(&fixture.annotated_target),
        "the old tag target should be replaced by --force: {annotated}"
    );
}

/// `--update-shallow` is accepted and a normal (non-shallow) fetch still
/// succeeds, updating the remote-tracking ref without creating a shallow file.
#[tokio::test]
#[serial]
async fn test_fetch_update_shallow_accepts_flag() {
    let (_temp_root, repo_dir, current_branch, pushed_commit) =
        setup_local_fetch_cli_fixture().await;

    let output = run_libra_command(
        &["--json", "fetch", "origin", "--update-shallow"],
        &repo_dir,
    );
    assert_cli_success(&output, "fetch origin --update-shallow");

    let json = parse_json_stdout(&output);
    assert_eq!(
        json["data"]["remotes"][0]["refs_updated"][0]["remote_ref"],
        format!("refs/remotes/origin/{current_branch}")
    );
    assert_eq!(
        json["data"]["remotes"][0]["refs_updated"][0]["new_oid"],
        pushed_commit
    );
    assert!(
        !repo_dir.join(".libra/shallow").exists(),
        "fetching a non-shallow remote must not create a shallow boundary file"
    );
}

/// `--refmap` overrides where fetched branches are stored under
/// `refs/remotes/<remote>/`.
#[tokio::test]
#[serial]
async fn test_fetch_refmap_maps_to_custom_tracking_ref() {
    let (_temp_root, repo_dir, current_branch, pushed_commit) =
        setup_local_fetch_cli_fixture().await;

    let output = run_libra_command(
        &[
            "--json",
            "fetch",
            "origin",
            "--refmap",
            "refs/heads/*:refs/remotes/origin/mirror/*",
        ],
        &repo_dir,
    );
    assert_cli_success(&output, "fetch origin --refmap");

    let json = parse_json_stdout(&output);
    assert_eq!(
        json["data"]["remotes"][0]["refs_updated"][0]["remote_ref"],
        format!("refs/remotes/origin/mirror/{current_branch}")
    );
    assert_eq!(
        json["data"]["remotes"][0]["refs_updated"][0]["new_oid"],
        pushed_commit
    );

    let show = run_libra_command(&["show-ref"], &repo_dir);
    let refs = String::from_utf8_lossy(&show.stdout);
    assert!(
        refs.contains(&format!("refs/remotes/origin/mirror/{current_branch}")),
        "custom --refmap tracking ref should be stored: {refs}"
    );
}

/// A `--refmap` entry over the 256-byte cap is rejected with exit 129.
#[tokio::test]
#[serial]
async fn test_fetch_refmap_over_256_bytes_rejected() {
    let (_temp_root, repo_dir, _branch, _commit) = setup_local_fetch_cli_fixture().await;

    let long = format!("refs/heads/*:refs/remotes/origin/{}", "a".repeat(300));
    let output = run_libra_command(&["fetch", "origin", "--refmap", &long], &repo_dir);
    assert_eq!(
        output.status.code(),
        Some(129),
        "an over-long --refmap entry must exit 129: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A `--refmap` destination outside `refs/remotes/<remote>/` is rejected (129).
#[tokio::test]
#[serial]
async fn test_fetch_refmap_dst_outside_remote_rejected() {
    let (_temp_root, repo_dir, _branch, _commit) = setup_local_fetch_cli_fixture().await;

    let output = run_libra_command(
        &["fetch", "origin", "--refmap", "refs/heads/*:refs/heads/*"],
        &repo_dir,
    );
    assert_eq!(
        output.status.code(),
        Some(129),
        "a --refmap destination outside refs/remotes/<remote>/ must exit 129: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// `--atomic` is accepted and a normal fetch still succeeds, updating the
/// remote-tracking ref (per-remote ref updates are already transactional).
#[tokio::test]
#[serial]
async fn test_fetch_atomic_accepts_flag() {
    let (_temp_root, repo_dir, current_branch, pushed_commit) =
        setup_local_fetch_cli_fixture().await;

    let output = run_libra_command(&["--json", "fetch", "origin", "--atomic"], &repo_dir);
    assert_cli_success(&output, "fetch origin --atomic");

    let json = parse_json_stdout(&output);
    assert_eq!(
        json["data"]["remotes"][0]["refs_updated"][0]["remote_ref"],
        format!("refs/remotes/origin/{current_branch}")
    );
    assert_eq!(
        json["data"]["remotes"][0]["refs_updated"][0]["new_oid"],
        pushed_commit
    );
}
