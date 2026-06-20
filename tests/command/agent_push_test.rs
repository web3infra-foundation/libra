//! Integration coverage for `libra agent push`.
//!
//! The external-agent capture plan reserves `refs/libra/agent-traces` for
//! transport. This test keeps the wrapper pinned to that private destination
//! instead of accidentally publishing `agent-traces` as a normal branch.

#[cfg(unix)]
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[cfg(unix)]
use libra::{
    internal::branch::{AGENT_TRACES_BRANCH, Branch as InternalBranch},
    utils::test::ChangeDirGuard,
};
#[cfg(unix)]
use serial_test::serial;

#[cfg(unix)]
fn libra_command(cwd: &Path) -> Command {
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

#[cfg(unix)]
fn create_fake_ssh_script(root: &Path) -> PathBuf {
    let script_path = root.join("fake_ssh.sh");
    let script = r#"#!/bin/sh
set -eu

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

    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(&script_path)
        .expect("failed to stat fake ssh script")
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script_path, perms).expect("failed to chmod fake ssh script");

    script_path
}

#[cfg(unix)]
fn init_repo_with_agent_traces_tip(local_dir: &Path) -> String {
    fs::create_dir_all(local_dir).expect("failed to create local repo dir");

    let init_out = libra_command(local_dir)
        .args(["init"])
        .output()
        .expect("failed to init local libra repo");
    assert!(
        init_out.status.success(),
        "local init failed: {}",
        String::from_utf8_lossy(&init_out.stderr)
    );

    for (key, value) in [
        ("user.name", "Agent Push Test"),
        ("user.email", "agent-push@example.com"),
    ] {
        let config_out = libra_command(local_dir)
            .args(["config", key, value])
            .output()
            .expect("failed to configure identity");
        assert!(
            config_out.status.success(),
            "config {key} failed: {}",
            String::from_utf8_lossy(&config_out.stderr)
        );
    }

    fs::write(local_dir.join("tracked.txt"), "agent traces source\n")
        .expect("failed to write tracked file");
    let add_out = libra_command(local_dir)
        .args(["add", "tracked.txt"])
        .output()
        .expect("failed to add tracked file");
    assert!(
        add_out.status.success(),
        "add failed: {}",
        String::from_utf8_lossy(&add_out.stderr)
    );

    let commit_out = libra_command(local_dir)
        .args(["commit", "-m", "base"])
        .output()
        .expect("failed to commit");
    assert!(
        commit_out.status.success(),
        "commit failed: {}",
        String::from_utf8_lossy(&commit_out.stderr)
    );

    let head_out = libra_command(local_dir)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("failed to read HEAD");
    assert!(
        head_out.status.success(),
        "rev-parse HEAD failed: {}",
        String::from_utf8_lossy(&head_out.stderr)
    );
    let head = String::from_utf8(head_out.stdout)
        .expect("HEAD hash not utf8")
        .trim()
        .to_string();

    let _guard = ChangeDirGuard::new(local_dir);
    let runtime = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    runtime
        .block_on(InternalBranch::update_branch(
            AGENT_TRACES_BRANCH,
            &head,
            None,
        ))
        .expect("failed to point agent-traces branch at HEAD");

    head
}

#[cfg(unix)]
fn add_fake_ssh_remote(local_dir: &Path, remote_dir: &Path) {
    let ssh_remote = format!("git@fakehost:{}", remote_dir.to_string_lossy());
    let remote_out = libra_command(local_dir)
        .args(["remote", "add", "origin", &ssh_remote])
        .output()
        .expect("failed to add fake ssh remote");
    assert!(
        remote_out.status.success(),
        "remote add failed: {}",
        String::from_utf8_lossy(&remote_out.stderr)
    );
}

#[cfg(unix)]
#[test]
#[serial]
fn agent_push_writes_private_agent_traces_ref() {
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

    let local_head = init_repo_with_agent_traces_tip(&local_dir);
    add_fake_ssh_remote(&local_dir, &remote_dir);

    let push_out = libra_command(&local_dir)
        .env("LIBRA_SSH_COMMAND", &ssh_script)
        .args(["agent", "push", "--remote", "origin"])
        .output()
        .expect("failed to run libra agent push");
    assert!(
        push_out.status.success(),
        "agent push failed: {}",
        String::from_utf8_lossy(&push_out.stderr)
    );

    let private_ref_out = Command::new("git")
        .args([
            "--git-dir",
            remote_dir.to_str().unwrap(),
            "rev-parse",
            "refs/libra/agent-traces",
        ])
        .output()
        .expect("failed to read remote private ref");
    assert!(
        private_ref_out.status.success(),
        "remote refs/libra/agent-traces should exist, stderr: {}",
        String::from_utf8_lossy(&private_ref_out.stderr)
    );
    let private_ref = String::from_utf8(private_ref_out.stdout)
        .expect("remote ref hash not utf8")
        .trim()
        .to_string();
    assert_eq!(private_ref, local_head);

    let public_branch_out = Command::new("git")
        .args([
            "--git-dir",
            remote_dir.to_str().unwrap(),
            "rev-parse",
            "--verify",
            "refs/heads/agent-traces",
        ])
        .output()
        .expect("failed to check public agent-traces branch");
    assert!(
        !public_branch_out.status.success(),
        "agent push must not create refs/heads/agent-traces"
    );
}
