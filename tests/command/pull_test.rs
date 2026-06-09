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
#[serial]
fn test_pull_ff_only_conflicts_with_rebase() {
    // `--ff-only` is a merge-only flag; combining it with `--rebase` is a
    // runtime usage error. (`--rebase=false --ff-only` is allowed, so this is a
    // runtime guard rather than a clap conflict — see `rebase_false_allows_*`.)
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["pull", "--ff-only", "--rebase"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(129));
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert!(
        stderr.contains("cannot be used with")
            && stderr.contains("--ff-only")
            && stderr.contains("--rebase"),
        "pull should reject conflicting integration modes: {stderr}"
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

// ─────────────────────────────────────────────────────────────────────────
// Batch 0/1: flag-compatibility and config gates (fail before fetch; a
// committed repo is enough — no remote needed).
// ─────────────────────────────────────────────────────────────────────────

fn assert_incompatible(output: &std::process::Output, needles: &[&str]) {
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(output.status.code(), Some(129), "stderr: {stderr}");
    assert_eq!(report.error_code, "LBR-CLI-002", "stderr: {stderr}");
    for needle in needles {
        assert!(stderr.contains(needle), "expected `{needle}` in: {stderr}");
    }
}

#[test]
#[serial]
fn test_pull_squash_conflicts_with_rebase() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["pull", "--rebase", "--squash"], repo.path());
    assert_incompatible(&output, &["--rebase", "--squash"]);
}

#[test]
#[serial]
fn test_pull_no_commit_conflicts_with_rebase() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["pull", "--rebase", "--no-commit"], repo.path());
    assert_incompatible(&output, &["--rebase", "--no-commit"]);
}

#[test]
#[serial]
fn test_pull_rebase_autostash_rejected() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["pull", "--rebase", "--autostash"], repo.path());
    assert_incompatible(&output, &["--rebase", "--autostash"]);
}

#[test]
#[serial]
fn test_pull_squash_autostash_rejected_upfront() {
    // Tracking IS configured so the squash guard (which runs after rebase
    // resolution but before fetch) is the failure, not a tracking error.
    let repo = create_committed_repo_via_cli();
    run_libra_command(&["config", "branch.main.remote", "origin"], repo.path());
    run_libra_command(
        &["config", "branch.main.merge", "refs/heads/main"],
        repo.path(),
    );
    let output = run_libra_command(&["pull", "--squash", "--autostash"], repo.path());
    assert_incompatible(&output, &["--squash", "autostash"]);
}

#[test]
#[serial]
fn test_pull_rebase_false_allows_squash() {
    // `--rebase=false` forces the merge path and must NOT trip the rebase guard;
    // it falls through to the (here unconfigured) tracking lookup instead.
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["pull", "--rebase=false", "--squash"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(output.status.code(), Some(128), "stderr: {stderr}");
    assert_eq!(report.error_code, "LBR-REPO-003");
    assert!(
        !stderr.contains("cannot be used with"),
        "--rebase=false --squash must not be rejected as incompatible: {stderr}"
    );
    assert!(stderr.contains("no tracking information"));
}

#[test]
#[serial]
fn test_pull_rebase_merges_rejected() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["pull", "--rebase=merges"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(output.status.code(), Some(129), "stderr: {stderr}");
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert!(stderr.contains("not supported"), "stderr: {stderr}");
}

#[test]
#[serial]
fn test_pull_rebase_interactive_rejected() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["pull", "--rebase=interactive"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(output.status.code(), Some(129), "stderr: {stderr}");
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert!(stderr.contains("not supported"), "stderr: {stderr}");
}

#[test]
#[serial]
fn test_pull_rebase_config_merges_rejected() {
    let repo = create_committed_repo_via_cli();
    run_libra_command(&["config", "pull.rebase", "merges"], repo.path());
    let output = run_libra_command(&["pull"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(output.status.code(), Some(129), "stderr: {stderr}");
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert!(stderr.contains("not supported"), "stderr: {stderr}");
}

#[test]
#[serial]
fn test_pull_rebase_config_invalid_value_rejected() {
    let repo = create_committed_repo_via_cli();
    run_libra_command(&["config", "pull.rebase", "bogus"], repo.path());
    let output = run_libra_command(&["pull"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(output.status.code(), Some(129), "stderr: {stderr}");
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert!(stderr.contains("pull.rebase"), "stderr: {stderr}");
}

#[test]
#[serial]
fn test_pull_squash_with_merge_autostash_config_rejected() {
    let repo = create_committed_repo_via_cli();
    run_libra_command(&["config", "branch.main.remote", "origin"], repo.path());
    run_libra_command(
        &["config", "branch.main.merge", "refs/heads/main"],
        repo.path(),
    );
    run_libra_command(&["config", "merge.autoStash", "true"], repo.path());

    // merge.autoStash=true resolves autostash on, so --squash is rejected.
    let rejected = run_libra_command(&["pull", "--squash"], repo.path());
    assert_incompatible(&rejected, &["--squash", "autostash"]);

    // --no-autostash forces autostash off, so the squash guard passes and we
    // fall through to target resolution, which fails with "remote not found"
    // (LBR-CLI-003) — distinct from the squash guard's LBR-CLI-002. Both are
    // exit 129, so distinguish by stable code.
    let allowed = run_libra_command(&["pull", "--squash", "--no-autostash"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&allowed.stderr);
    assert_eq!(
        report.error_code, "LBR-CLI-003",
        "--no-autostash should clear the squash/autostash conflict and reach \
         target resolution, not be rejected as incompatible: {stderr}"
    );
    assert!(
        !stderr.contains("autostash"),
        "cleared squash/autostash conflict must not mention autostash: {stderr}"
    );
}

#[test]
#[serial]
fn test_pull_squash_with_pull_ff_false_config_rejected() {
    let repo = create_committed_repo_via_cli();
    run_libra_command(&["config", "branch.main.remote", "origin"], repo.path());
    run_libra_command(
        &["config", "branch.main.merge", "refs/heads/main"],
        repo.path(),
    );
    run_libra_command(&["config", "pull.ff", "false"], repo.path());

    let output = run_libra_command(&["pull", "--squash"], repo.path());
    assert_incompatible(&output, &["--squash", "no-fast-forward"]);
}

// ─────────────────────────────────────────────────────────────────────────
// Batch 0/1: behavioral forwarding against a real local remote.
// ─────────────────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn test_pull_no_ff_forces_merge_commit() {
    let (_temp_root, remote_dir, work_dir, branch) = create_remote_fixture();
    let local_repo = tempdir().expect("local repo");
    init_repo_via_cli(local_repo.path());
    configure_identity_via_cli(local_repo.path());
    configure_pull_tracking(local_repo.path(), &remote_dir, &branch);
    assert_cli_success(
        &run_libra_command(&["pull"], local_repo.path()),
        "initial pull",
    );

    push_remote_commit(
        &work_dir,
        &branch,
        "remote.txt",
        "remote\n",
        "remote update",
    );

    let output = run_libra_command(&["pull", "--no-ff"], local_repo.path());
    assert_cli_success(&output, "pull --no-ff");

    let _guard = ChangeDirGuard::new(local_repo.path());
    let head = Head::current_commit().await.expect("merge commit");
    let commit: Commit = load_object(&head).expect("load merge commit");
    assert_eq!(
        commit.parent_commit_ids.len(),
        2,
        "--no-ff must force a merge commit even when fast-forward is possible"
    );
}

#[tokio::test]
#[serial]
async fn test_pull_squash_stages_without_commit() {
    let (_temp_root, remote_dir, work_dir, branch) = create_remote_fixture();
    let local_repo = tempdir().expect("local repo");
    init_repo_via_cli(local_repo.path());
    configure_identity_via_cli(local_repo.path());
    configure_pull_tracking(local_repo.path(), &remote_dir, &branch);
    assert_cli_success(
        &run_libra_command(&["pull"], local_repo.path()),
        "initial pull",
    );

    push_remote_commit(
        &work_dir,
        &branch,
        "remote.txt",
        "remote\n",
        "remote update",
    );
    fs::write(local_repo.path().join("local.txt"), "local\n").expect("write local");
    assert_cli_success(
        &run_libra_command(&["add", "local.txt"], local_repo.path()),
        "stage local",
    );
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "local", "--no-verify"], local_repo.path()),
        "commit local",
    );

    let head_before = {
        let _g = ChangeDirGuard::new(local_repo.path());
        Head::current_commit().await.expect("HEAD before squash")
    };

    let output = run_libra_command(&["--json", "pull", "--squash"], local_repo.path());
    assert_cli_success(&output, "pull --squash");
    let json = parse_json_stdout(&output);
    assert!(
        json["data"]["merge"]["commit"].is_null(),
        "squash must not record a merge commit: {json}"
    );

    let head_after = {
        let _g = ChangeDirGuard::new(local_repo.path());
        Head::current_commit().await.expect("HEAD after squash")
    };
    assert_eq!(head_before, head_after, "squash must not move HEAD");
    assert!(
        local_repo.path().join("remote.txt").exists(),
        "squash stages the remote change into the worktree"
    );
}

#[test]
#[serial]
fn test_pull_squash_human_output_does_not_report_fast_forward() {
    let (_temp_root, remote_dir, work_dir, branch) = create_remote_fixture();
    let local_repo = tempdir().expect("local repo");
    init_repo_via_cli(local_repo.path());
    configure_identity_via_cli(local_repo.path());
    configure_pull_tracking(local_repo.path(), &remote_dir, &branch);
    assert_cli_success(
        &run_libra_command(&["pull"], local_repo.path()),
        "initial pull",
    );

    push_remote_commit(
        &work_dir,
        &branch,
        "remote.txt",
        "remote\n",
        "remote update",
    );
    fs::write(local_repo.path().join("local.txt"), "local\n").expect("write local");
    assert_cli_success(
        &run_libra_command(&["add", "local.txt"], local_repo.path()),
        "stage local",
    );
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "local", "--no-verify"], local_repo.path()),
        "commit local",
    );

    let output = run_libra_command(&["pull", "--squash"], local_repo.path());
    assert_cli_success(&output, "pull --squash");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Squash commit -- not updating HEAD."),
        "squash pull should explain that HEAD was not updated: {stdout}"
    );
    assert!(
        !stdout.lines().any(|line| line == "Fast-forward"),
        "squash pull must not report the integration as a fast-forward: {stdout}"
    );
}

#[test]
#[serial]
fn test_pull_no_commit_human_output_leaves_merge_state_without_fast_forward_label() {
    let (_temp_root, remote_dir, work_dir, branch) = create_remote_fixture();
    let local_repo = tempdir().expect("local repo");
    init_repo_via_cli(local_repo.path());
    configure_identity_via_cli(local_repo.path());
    configure_pull_tracking(local_repo.path(), &remote_dir, &branch);
    assert_cli_success(
        &run_libra_command(&["pull"], local_repo.path()),
        "initial pull",
    );

    push_remote_commit(
        &work_dir,
        &branch,
        "remote.txt",
        "remote\n",
        "remote update",
    );
    fs::write(local_repo.path().join("local.txt"), "local\n").expect("write local");
    assert_cli_success(
        &run_libra_command(&["add", "local.txt"], local_repo.path()),
        "stage local",
    );
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "local", "--no-verify"], local_repo.path()),
        "commit local",
    );

    let output = run_libra_command(&["pull", "--no-commit"], local_repo.path());
    assert_cli_success(&output, "pull --no-commit");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Automatic merge went well; stopped before committing as requested."),
        "no-commit pull should explain that it stopped before committing: {stdout}"
    );
    assert!(
        !stdout.lines().any(|line| line == "Fast-forward"),
        "no-commit pull must not report the integration as a fast-forward: {stdout}"
    );
    assert!(
        local_repo
            .path()
            .join(".libra")
            .join("merge-state.json")
            .exists(),
        "--no-commit pull should leave merge state for an explicit commit/continue"
    );
}

#[tokio::test]
#[serial]
async fn test_pull_autostash_clean_after_up_to_date() {
    let (_temp_root, remote_dir, _work_dir, branch) = create_remote_fixture();
    let local_repo = tempdir().expect("local repo");
    init_repo_via_cli(local_repo.path());
    configure_identity_via_cli(local_repo.path());
    configure_pull_tracking(local_repo.path(), &remote_dir, &branch);
    assert_cli_success(
        &run_libra_command(&["pull"], local_repo.path()),
        "initial pull",
    );

    // Dirty a tracked file; remote is unchanged so the pull is up to date.
    fs::write(local_repo.path().join("README.md"), "dirty local edit\n").expect("dirty edit");

    let output = run_libra_command(&["pull", "--autostash"], local_repo.path());
    assert_cli_success(&output, "pull --autostash up-to-date");

    assert_eq!(
        fs::read_to_string(local_repo.path().join("README.md")).expect("read README"),
        "dirty local edit\n",
        "autostash must restore the dirty edit after integrating"
    );
    let stash_list = run_libra_command(&["stash", "list"], local_repo.path());
    assert_cli_success(&stash_list, "stash list");
    assert!(
        String::from_utf8_lossy(&stash_list.stdout)
            .trim()
            .is_empty(),
        "autostash must not leave a stash entry behind"
    );
}

#[tokio::test]
#[serial]
async fn test_pull_depth_forwarded_to_fetch_local_remote() {
    let (_temp_root, remote_dir, work_dir, branch) = create_remote_fixture();
    push_remote_commit(&work_dir, &branch, "c2.txt", "c2\n", "second");
    push_remote_commit(&work_dir, &branch, "c3.txt", "c3\n", "third");

    let local_repo = tempdir().expect("local repo");
    init_repo_via_cli(local_repo.path());
    configure_identity_via_cli(local_repo.path());
    let remote_add = run_libra_command(
        &["remote", "add", "origin", remote_dir.to_str().unwrap()],
        local_repo.path(),
    );
    assert_cli_success(&remote_add, "remote add");

    let output = run_libra_command(
        &["pull", "origin", &branch, "--depth", "1"],
        local_repo.path(),
    );
    assert_cli_success(&output, "pull origin <branch> --depth 1");

    assert!(
        local_repo.path().join(".libra").join("shallow").exists(),
        "a depth-limited pull must record the .libra/shallow boundary, proving --depth reached fetch"
    );
}
