//! Tests pull command integration that combines fetch with merge or rebase behaviors.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use git_internal::internal::object::commit::Commit;
use libra::{command::load_object, internal::head::Head, utils::test::ChangeDirGuard};
use serial_test::serial;
use tempfile::{TempDir, tempdir};

use super::{
    assert_cli_success, configure_identity_via_cli, create_committed_repo_via_cli,
    init_repo_via_cli, parse_cli_error_stderr, parse_json_stdout, run_libra_command,
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

#[test]
fn test_pull_ff_only_conflicts_with_rebase_at_parse_time() {
    let repo = tempdir().expect("failed to create local repo");

    let output = run_libra_command(&["pull", "--ff-only", "--rebase"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(129));
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert!(
        stderr.contains("cannot be used with")
            && stderr.contains("--ff-only")
            && stderr.contains("--rebase"),
        "pull should reject conflicting integration modes before repo preflight: {stderr}"
    );
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
async fn test_pull_ff_only_fast_forward_updates_head_from_tracking_remote() {
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
        "remote.txt",
        "remote change\n",
        "remote update",
    );

    let output = run_libra_command(&["pull", "--ff-only"], local_repo.path());
    assert_cli_success(&output, "pull --ff-only fast-forward");

    let _guard = ChangeDirGuard::new(local_repo.path());
    let head = Head::current_commit()
        .await
        .expect("pull --ff-only should update HEAD to the fetched commit");
    assert_eq!(head.to_string(), new_head);
    assert!(
        local_repo.path().join("remote.txt").exists(),
        "pull --ff-only should restore the fetched worktree"
    );
}

#[tokio::test]
#[serial]
async fn test_pull_diverged_remote_creates_three_way_merge() {
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
    assert_cli_success(&output, "pull three-way merge");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Merge made by the 'three-way' strategy."),
        "pull should report three-way strategy, stdout: {stdout}"
    );

    let _guard = ChangeDirGuard::new(local_repo.path());
    let head = Head::current_commit()
        .await
        .expect("pull should create a merge commit");
    let commit: Commit = load_object(&head).expect("load pull merge commit");
    assert_eq!(commit.parent_commit_ids.len(), 2);
    assert!(
        commit.message.starts_with('\n'),
        "pull merge commit body must retain Git's blank-line separator before the message"
    );
    assert!(local_repo.path().join("remote.txt").exists());
    assert!(local_repo.path().join("local.txt").exists());
}

#[tokio::test]
#[serial]
async fn test_pull_ff_only_diverged_remote_rejects_without_changing_head_or_worktree() {
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

    let guard = ChangeDirGuard::new(local_repo.path());
    let local_head = Head::current_commit()
        .await
        .expect("local commit should leave HEAD");
    drop(guard);

    let output = run_libra_command(&["--json", "pull", "--ff-only"], local_repo.path());
    let report = parse_json_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report["ok"], false);
    assert_eq!(report["error_code"], "LBR-CONFLICT-002");
    assert_eq!(report["details"]["phase"], "merge");
    assert!(
        report["message"]
            .as_str()
            .is_some_and(|text| text.contains("non-fast-forward")),
        "pull --ff-only should explain the rejected merge: {report}"
    );
    assert!(
        report["hints"]
            .as_array()
            .expect("hints")
            .iter()
            .any(|hint| hint
                .as_str()
                .is_some_and(|text| text.contains("without --ff-only"))),
        "pull --ff-only should hint how to allow a merge commit: {report}"
    );

    let _guard = ChangeDirGuard::new(local_repo.path());
    let head_after = Head::current_commit()
        .await
        .expect("failed pull --ff-only should leave HEAD unchanged");
    assert_eq!(head_after, local_head);
    assert!(
        local_repo.path().join("local.txt").exists(),
        "local worktree file must remain"
    );
    assert!(
        !local_repo.path().join("remote.txt").exists(),
        "ff-only rejection must not apply remote worktree changes"
    );
    assert!(
        !local_repo
            .path()
            .join(".libra")
            .join("merge-state.json")
            .exists(),
        "ff-only rejection must not create merge state"
    );
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
        stdout.lines().any(|line| line == "Fast-forward"),
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
fn test_pull_json_diverged_remote_reports_three_way_merge() {
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
    assert_cli_success(&output, "json pull three-way merge");
    assert!(output.stderr.is_empty());
    let report = parse_json_stdout(&output);

    assert_eq!(report["ok"], true);
    assert_eq!(report["command"], "pull");
    assert_eq!(report["data"]["merge"]["strategy"], "three-way");
    assert_eq!(
        report["data"]["merge"]["parents"]
            .as_array()
            .expect("parents")
            .len(),
        2
    );
}

#[test]
#[serial]
fn test_pull_conflict_error_includes_merge_phase_and_hints() {
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
        "README.md",
        "remote change\n",
        "remote update",
    );

    fs::write(local_repo.path().join("README.md"), "local change\n").expect("write local change");
    let add = run_libra_command(&["add", "README.md"], local_repo.path());
    assert_cli_success(&add, "stage local change");
    let commit = run_libra_command(
        &["commit", "-m", "local update", "--no-verify"],
        local_repo.path(),
    );
    assert_cli_success(&commit, "commit local change");

    let output = run_libra_command(&["--json", "pull"], local_repo.path());
    let report = parse_json_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report["ok"], false);
    assert_eq!(report["error_code"], "LBR-CONFLICT-002");
    assert_eq!(report["details"]["phase"], "merge");
    assert!(
        report["hints"]
            .as_array()
            .expect("hints")
            .iter()
            .any(|hint| hint
                .as_str()
                .is_some_and(|text| text.contains("merge --continue"))),
        "pull conflict should hint merge --continue: {report}"
    );

    let conflicted =
        fs::read_to_string(local_repo.path().join("README.md")).expect("read conflict markers");
    assert!(conflicted.contains("<<<<<<< HEAD"), "{conflicted}");
    assert!(conflicted.contains(">>>>>>>"), "{conflicted}");
}

/// `libra pull --rebase` replays the local-only commit on top of the
/// freshly-fetched upstream tip when the histories have diverged.
#[tokio::test]
#[serial]
async fn test_pull_rebase_replays_local_commit_onto_diverged_upstream() {
    let (_temp_root, remote_dir, work_dir, branch) = create_remote_fixture();

    let local_repo = tempdir().expect("failed to create local repo");
    init_repo_via_cli(local_repo.path());
    configure_identity_via_cli(local_repo.path());
    configure_pull_tracking(local_repo.path(), &remote_dir, &branch);

    let first_pull = run_libra_command(&["pull"], local_repo.path());
    assert_cli_success(&first_pull, "initial pull");

    let remote_head = push_remote_commit(
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

    let output = run_libra_command(&["--json", "pull", "--rebase"], local_repo.path());
    assert_cli_success(&output, "rebase pull");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("expected JSON on stdout, got: {stdout}\nerror: {e}"));
    let data = &parsed["data"];

    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["command"], "pull");
    assert!(data["merge"].is_null());
    assert_eq!(data["rebase"]["status"], "completed");
    assert_eq!(data["rebase"]["replay_count"], 1);
    assert_eq!(data["rebase"]["up_to_date"], false);
    assert!(data["rebase"]["commit"].is_string());
    assert!(data["rebase"]["old_commit"].is_string());
    assert!(
        local_repo.path().join("remote.txt").exists(),
        "rebase should have brought in remote.txt"
    );
    assert!(
        local_repo.path().join("local.txt").exists(),
        "rebase should keep local.txt"
    );

    let new_commit = data["rebase"]["commit"].as_str().expect("commit string");
    assert_ne!(
        new_commit, remote_head,
        "rebased commit must be a child of upstream, not the upstream tip itself"
    );
}

#[tokio::test]
#[serial]
async fn test_pull_rebase_already_up_to_date_reports_noop() {
    let (_temp_root, remote_dir, _work_dir, branch) = create_remote_fixture();

    let local_repo = tempdir().expect("failed to create local repo");
    init_repo_via_cli(local_repo.path());
    configure_identity_via_cli(local_repo.path());
    configure_pull_tracking(local_repo.path(), &remote_dir, &branch);

    let first_pull = run_libra_command(&["pull"], local_repo.path());
    assert_cli_success(&first_pull, "initial pull");

    let output = run_libra_command(&["--json", "pull", "--rebase"], local_repo.path());
    assert_cli_success(&output, "rebase pull (no-op)");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("expected JSON on stdout, got: {stdout}\nerror: {e}"));
    let data = &parsed["data"];

    assert_eq!(data["rebase"]["replay_count"], 0);
    assert_eq!(data["rebase"]["up_to_date"], true);
    let old = data["rebase"]["old_commit"]
        .as_str()
        .expect("old_commit string");
    let new_commit = data["rebase"]["commit"].as_str().expect("commit string");
    assert_eq!(
        old, new_commit,
        "HEAD must not move when there is nothing to rebase"
    );
}
