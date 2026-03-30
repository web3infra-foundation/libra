//! Tests pull command integration that combines fetch with merge or rebase behaviors.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use libra::{internal::head::Head, utils::test::ChangeDirGuard};
use serial_test::serial;
use tempfile::{TempDir, tempdir};

use super::{
    assert_cli_success, configure_identity_via_cli, create_committed_repo_via_cli,
    init_repo_via_cli, parse_cli_error_stderr, run_libra_command,
};

fn git(args: &[&str], cwd: &Path) {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("failed to execute git");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_stdout(args: &[&str], cwd: &Path) -> String {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("failed to execute git");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("git output should be utf8")
        .trim()
        .to_string()
}

fn create_remote_fixture() -> (TempDir, PathBuf, PathBuf, String) {
    let temp_root = tempdir().expect("failed to create temp root");
    let remote_dir = temp_root.path().join("remote.git");
    let work_dir = temp_root.path().join("workdir");

    git(
        &["init", "--bare", remote_dir.to_str().unwrap()],
        temp_root.path(),
    );
    git(&["init", work_dir.to_str().unwrap()], temp_root.path());
    git(&["config", "user.name", "Libra Tester"], &work_dir);
    git(&["config", "user.email", "tester@example.com"], &work_dir);

    fs::write(work_dir.join("README.md"), "hello libra\n").expect("failed to write README");
    git(&["add", "README.md"], &work_dir);
    git(&["commit", "-m", "initial commit"], &work_dir);

    let branch = git_stdout(&["rev-parse", "--abbrev-ref", "HEAD"], &work_dir);
    git(
        &["remote", "add", "origin", remote_dir.to_str().unwrap()],
        &work_dir,
    );
    git(
        &["push", "origin", &format!("HEAD:refs/heads/{branch}")],
        &work_dir,
    );

    (temp_root, remote_dir, work_dir, branch)
}

fn push_remote_commit(
    work_dir: &Path,
    branch: &str,
    file: &str,
    content: &str,
    message: &str,
) -> String {
    fs::write(work_dir.join(file), content).expect("failed to write remote file");
    git(&["add", file], work_dir);
    git(&["commit", "-m", message], work_dir);
    git(
        &["push", "origin", &format!("HEAD:refs/heads/{branch}")],
        work_dir,
    );
    git_stdout(&["rev-parse", "HEAD"], work_dir)
}

fn configure_pull_tracking(repo: &Path, remote_dir: &Path, branch: &str) {
    let remote_output = run_libra_command(
        &["remote", "add", "origin", remote_dir.to_str().unwrap()],
        repo,
    );
    assert_cli_success(&remote_output, "remote add");

    let branch_remote = run_libra_command(&["config", "branch.main.remote", "origin"], repo);
    assert_cli_success(&branch_remote, "set branch.main.remote");

    let merge_ref = format!("refs/heads/{branch}");
    let branch_merge = run_libra_command(&["config", "branch.main.merge", &merge_ref], repo);
    assert_cli_success(&branch_merge, "set branch.main.merge");
}

fn parse_json_stderr(stderr: &[u8]) -> serde_json::Value {
    serde_json::from_str(String::from_utf8_lossy(stderr).trim())
        .expect("stderr should contain a JSON error report")
}

#[test]
#[serial]
fn test_pull_cli_without_tracking_returns_repo_exit_code() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["pull"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-REPO-003");
    assert!(stderr.contains("there is no tracking information for the current branch"));
}

#[test]
#[serial]
fn test_pull_cli_remote_not_found_returns_cli_exit_code() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["pull", "origin", "main"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(129));
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert!(stderr.contains("remote 'origin' not found"));
}

#[tokio::test]
#[serial]
async fn test_pull_fast_forward_updates_head_from_tracking_remote() {
    let (_temp_root, remote_dir, work_dir, branch) = create_remote_fixture();
    let remote_head = git_stdout(&["rev-parse", "HEAD"], &work_dir);

    let local_repo = tempdir().expect("failed to create local repo");
    init_repo_via_cli(local_repo.path());
    configure_identity_via_cli(local_repo.path());
    configure_pull_tracking(local_repo.path(), &remote_dir, &branch);

    let output = run_libra_command(&["pull"], local_repo.path());
    assert_cli_success(&output, "pull fast-forward");

    let _guard = ChangeDirGuard::new(local_repo.path());
    let head = Head::current_commit()
        .await
        .expect("pull should update HEAD to the fetched commit");
    assert_eq!(head.to_string(), remote_head);
    assert!(
        local_repo.path().join("README.md").exists(),
        "pull should restore the fetched worktree"
    );
}

#[tokio::test]
#[serial]
async fn test_pull_manual_merge_required_returns_conflict_code() {
    let (_temp_root, remote_dir, work_dir, branch) = create_remote_fixture();

    let local_repo = tempdir().expect("failed to create local repo");
    init_repo_via_cli(local_repo.path());
    configure_identity_via_cli(local_repo.path());
    configure_pull_tracking(local_repo.path(), &remote_dir, &branch);

    let first_pull = run_libra_command(&["pull"], local_repo.path());
    assert_cli_success(&first_pull, "initial pull");

    let _remote_head = push_remote_commit(
        &work_dir,
        &branch,
        "remote.txt",
        "remote change\n",
        "remote update",
    );

    fs::write(local_repo.path().join("local.txt"), "local change\n").expect("write local change");
    let add = run_libra_command(&["add", "local.txt"], local_repo.path());
    assert_cli_success(&add, "stage local change");
    let commit = run_libra_command(
        &["commit", "-m", "local update", "--no-verify"],
        local_repo.path(),
    );
    assert_cli_success(&commit, "commit local change");

    let output = run_libra_command(&["pull"], local_repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-CONFLICT-002");
    assert!(stderr.contains("pull requires a non-fast-forward merge"));
}

#[tokio::test]
#[serial]
async fn test_pull_detached_head_returns_repo_exit_code() {
    let repo = create_committed_repo_via_cli();
    let _guard = ChangeDirGuard::new(repo.path());

    let head = Head::current_commit().await.expect("repo should have HEAD");
    Head::update(Head::Detached(head), None).await;

    let output = run_libra_command(&["pull"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-REPO-003");
    assert!(stderr.contains("you are not currently on a branch"));
}

#[test]
#[serial]
fn test_pull_quiet_suppresses_stdout() {
    let (_temp_root, remote_dir, _work_dir, branch) = create_remote_fixture();

    let local_repo = tempdir().expect("failed to create local repo");
    init_repo_via_cli(local_repo.path());
    configure_identity_via_cli(local_repo.path());
    configure_pull_tracking(local_repo.path(), &remote_dir, &branch);

    let output = run_libra_command(&["--quiet", "pull"], local_repo.path());
    assert_cli_success(&output, "quiet pull");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.trim().is_empty(),
        "quiet pull should suppress stdout, got: {stdout}"
    );
}

#[test]
#[serial]
fn test_pull_human_output_reports_update_range_after_follow_up_fast_forward() {
    let (_temp_root, remote_dir, work_dir, branch) = create_remote_fixture();

    let local_repo = tempdir().expect("failed to create local repo");
    init_repo_via_cli(local_repo.path());
    configure_identity_via_cli(local_repo.path());
    configure_pull_tracking(local_repo.path(), &remote_dir, &branch);

    let first_pull = run_libra_command(&["pull"], local_repo.path());
    assert_cli_success(&first_pull, "initial pull");

    let new_head = push_remote_commit(
        &work_dir,
        &branch,
        "next.txt",
        "next change\n",
        "remote follow-up",
    );
    let previous_head = git_stdout(&["rev-parse", "HEAD~1"], &work_dir);

    let output = run_libra_command(&["pull"], local_repo.path());
    assert_cli_success(&output, "follow-up pull");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("From "),
        "pull should include the fetched remote, stdout: {stdout}"
    );
    assert!(
        stdout.contains(&format!(
            "Updating {}..{}",
            &previous_head[..7],
            &new_head[..7]
        )),
        "pull should report the fast-forward range, stdout: {stdout}"
    );
    assert!(
        stdout.contains("Fast-forward"),
        "pull should report the merge strategy, stdout: {stdout}"
    );
    assert!(
        stdout.contains("1 file changed"),
        "pull should summarize changed files, stdout: {stdout}"
    );
}

#[test]
#[serial]
fn test_pull_json_fetch_error_includes_phase_detail() {
    let repo = create_committed_repo_via_cli();

    let missing_remote = repo.path().join("missing-remote.git");
    let missing_remote_str = missing_remote.to_string_lossy().to_string();

    let remote_output = run_libra_command(
        &["remote", "add", "origin", &missing_remote_str],
        repo.path(),
    );
    assert_cli_success(&remote_output, "remote add");
    let branch_remote = run_libra_command(&["config", "branch.main.remote", "origin"], repo.path());
    assert_cli_success(&branch_remote, "set branch.main.remote");
    let branch_merge = run_libra_command(
        &["config", "branch.main.merge", "refs/heads/main"],
        repo.path(),
    );
    assert_cli_success(&branch_merge, "set branch.main.merge");

    let output = run_libra_command(&["--json", "pull"], repo.path());
    let report = parse_json_stderr(&output.stderr);

    assert_eq!(report["ok"], false);
    assert_eq!(report["details"]["phase"], "fetch");
}

#[test]
#[serial]
fn test_pull_json_manual_merge_error_includes_phase_detail() {
    let (_temp_root, remote_dir, work_dir, branch) = create_remote_fixture();

    let local_repo = tempdir().expect("failed to create local repo");
    init_repo_via_cli(local_repo.path());
    configure_identity_via_cli(local_repo.path());
    configure_pull_tracking(local_repo.path(), &remote_dir, &branch);

    let first_pull = run_libra_command(&["pull"], local_repo.path());
    assert_cli_success(&first_pull, "initial pull");

    let _remote_head = push_remote_commit(
        &work_dir,
        &branch,
        "remote.txt",
        "remote change\n",
        "remote update",
    );

    fs::write(local_repo.path().join("local.txt"), "local change\n").expect("write local change");
    let add = run_libra_command(&["add", "local.txt"], local_repo.path());
    assert_cli_success(&add, "stage local change");
    let commit = run_libra_command(
        &["commit", "-m", "local update", "--no-verify"],
        local_repo.path(),
    );
    assert_cli_success(&commit, "commit local change");

    let output = run_libra_command(&["--json", "pull"], local_repo.path());
    let report = parse_json_stderr(&output.stderr);

    assert_eq!(report["ok"], false);
    assert_eq!(report["error_code"], "LBR-CONFLICT-002");
    assert_eq!(report["details"]["phase"], "merge");
}
