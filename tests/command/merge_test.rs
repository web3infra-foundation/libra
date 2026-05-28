//! Tests merge command scenarios including fast-forward handling and conflict reporting.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use std::path::Path;

use git_internal::internal::object::commit::Commit;
use libra::{
    command::load_object,
    internal::{branch::Branch, head::Head},
    utils::test::ChangeDirGuard,
};
use serial_test::serial;

use super::{
    assert_cli_success, create_committed_repo_via_cli, parse_cli_error_stderr, parse_json_stdout,
    run_libra_command,
};

fn commit_file(repo: &Path, file: &str, content: &str, message: &str) {
    let path = repo.join(file);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("failed to create parent directory");
    }
    std::fs::write(path, content).expect("failed to write file");
    assert_cli_success(&run_libra_command(&["add", file], repo), "add file");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", message, "--no-verify"], repo),
        "commit file",
    );
}

#[test]
fn test_merge_cli_missing_branch_returns_error_1() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["merge", "no-such"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(129));
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert!(stderr.contains("error: no-such - not something we can merge"));
}

#[test]
fn test_merge_json_fast_forward_outputs_summary() {
    let temp_repo = create_committed_repo_via_cli();
    let temp_path = temp_repo.path();

    assert_cli_success(
        &run_libra_command(&["branch", "feature"], temp_path),
        "create branch",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "feature"], temp_path),
        "checkout feature",
    );

    std::fs::write(temp_path.join("file.txt"), "Feature content").expect("failed to write file");
    assert_cli_success(&run_libra_command(&["add", "."], temp_path), "add file");
    assert_cli_success(
        &run_libra_command(
            &["commit", "-m", "Add feature content", "--no-verify"],
            temp_path,
        ),
        "commit",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "main"], temp_path),
        "checkout main",
    );

    let output = run_libra_command(&["--json", "merge", "feature"], temp_path);
    assert_cli_success(&output, "json merge feature");

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "merge");
    assert_eq!(json["data"]["strategy"], "fast-forward");
    assert_eq!(json["data"]["up_to_date"], false);
    assert_eq!(json["data"]["files_changed"], 1);
    assert!(json["data"]["old_commit"].as_str().is_some());
    assert!(json["data"]["commit"].as_str().is_some());
    assert!(output.stderr.is_empty());
}

#[test]
fn test_merge_json_already_up_to_date_outputs_summary() {
    let temp_repo = create_committed_repo_via_cli();
    let temp_path = temp_repo.path();

    assert_cli_success(
        &run_libra_command(&["branch", "feature"], temp_path),
        "create branch",
    );

    let output = run_libra_command(&["--json", "merge", "feature"], temp_path);
    assert_cli_success(&output, "json merge up to date");

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "merge");
    assert_eq!(json["data"]["strategy"], "already-up-to-date");
    assert_eq!(json["data"]["up_to_date"], true);
    assert_eq!(json["data"]["files_changed"], 0);
    assert!(json["data"]["old_commit"].as_str().is_some());
    assert!(json["data"]["commit"].is_null());
    assert!(output.stderr.is_empty());
}

#[test]
fn test_merge_machine_outputs_single_json_line() {
    let temp_repo = create_committed_repo_via_cli();
    let temp_path = temp_repo.path();

    assert_cli_success(
        &run_libra_command(&["branch", "feature"], temp_path),
        "create branch",
    );

    let output = run_libra_command(&["--machine", "merge", "feature"], temp_path);
    assert_cli_success(&output, "machine merge feature");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.lines().count(),
        1,
        "expected one JSON line, got: {stdout}"
    );
    let json: serde_json::Value = serde_json::from_str(stdout.trim()).expect("expected JSON");
    assert_eq!(json["command"], "merge");
    assert_eq!(json["data"]["strategy"], "already-up-to-date");
    assert!(output.stderr.is_empty());
}

#[test]
fn test_merge_machine_fast_forward_outputs_single_json_line() {
    let temp_repo = create_committed_repo_via_cli();
    let temp_path = temp_repo.path();

    assert_cli_success(
        &run_libra_command(&["branch", "feature"], temp_path),
        "create branch",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "feature"], temp_path),
        "checkout feature",
    );

    std::fs::write(temp_path.join("file.txt"), "Feature content").expect("failed to write file");
    assert_cli_success(&run_libra_command(&["add", "."], temp_path), "add file");
    assert_cli_success(
        &run_libra_command(
            &["commit", "-m", "Add feature content", "--no-verify"],
            temp_path,
        ),
        "commit",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "main"], temp_path),
        "checkout main",
    );

    let output = run_libra_command(&["--machine", "merge", "feature"], temp_path);
    assert_cli_success(&output, "machine merge feature");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.lines().count(),
        1,
        "expected one JSON line, got: {stdout}"
    );
    let json: serde_json::Value = serde_json::from_str(stdout.trim()).expect("expected JSON");
    assert_eq!(json["command"], "merge");
    assert_eq!(json["data"]["strategy"], "fast-forward");
    assert_eq!(json["data"]["up_to_date"], false);
    assert_eq!(json["data"]["files_changed"], 1);
    assert!(output.stderr.is_empty());
}

#[tokio::test]
/// Test fast-forward merge of local branches
async fn test_merge_fast_forward() {
    let temp_repo = create_committed_repo_via_cli();
    let temp_path = temp_repo.path();

    assert_cli_success(
        &run_libra_command(&["branch", "feature"], temp_path),
        "create branch",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "feature"], temp_path),
        "checkout feature",
    );

    // Commit changes on the feature branch
    std::fs::write(temp_path.join("file.txt"), "Feature content").expect("Failed to write file");
    assert_cli_success(&run_libra_command(&["add", "."], temp_path), "add file");
    assert_cli_success(
        &run_libra_command(
            &["commit", "-m", "Add feature content", "--no-verify"],
            temp_path,
        ),
        "commit",
    );

    // Switch back to the main branch and perform fast-forward merge
    assert_cli_success(
        &run_libra_command(&["checkout", "main"], temp_path),
        "checkout main",
    );

    let merge_output = run_libra_command(&["merge", "feature"], temp_path);
    assert!(
        merge_output.status.success(),
        "Fast-forward merge failed: {}",
        String::from_utf8_lossy(&merge_output.stderr)
    );
}

#[tokio::test]
#[serial]
/// Test merging a remote branch
async fn test_merge_remote_branch() {
    let temp_repo = create_committed_repo_via_cli();
    let temp_path = temp_repo.path();

    assert_cli_success(
        &run_libra_command(&["branch", "feature"], temp_path),
        "create branch",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "feature"], temp_path),
        "checkout feature",
    );

    std::fs::write(temp_path.join("remote.txt"), "Remote content").expect("Failed to write file");
    assert_cli_success(&run_libra_command(&["add", "."], temp_path), "add file");
    assert_cli_success(
        &run_libra_command(
            &["commit", "-m", "Add remote content", "--no-verify"],
            temp_path,
        ),
        "commit",
    );

    let _guard = ChangeDirGuard::new(temp_path);
    let feature_commit = Head::current_commit()
        .await
        .expect("feature branch should have a tip");
    Branch::update_branch("feature", &feature_commit.to_string(), Some("origin"))
        .await
        .unwrap();

    assert_cli_success(
        &run_libra_command(&["checkout", "main"], temp_path),
        "checkout main",
    );

    let merge_output = run_libra_command(&["merge", "origin/feature"], temp_path);
    assert!(
        merge_output.status.success(),
        "Merge remote branch failed: {}",
        String::from_utf8_lossy(&merge_output.stderr)
    );
}

#[tokio::test]
#[serial]
/// Test JSON output when merging a remote branch reference.
async fn test_merge_json_remote_branch_outputs_summary() {
    let temp_repo = create_committed_repo_via_cli();
    let temp_path = temp_repo.path();

    assert_cli_success(
        &run_libra_command(&["branch", "feature"], temp_path),
        "create branch",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "feature"], temp_path),
        "checkout feature",
    );

    std::fs::write(temp_path.join("remote.txt"), "Remote content").expect("Failed to write file");
    assert_cli_success(&run_libra_command(&["add", "."], temp_path), "add file");
    assert_cli_success(
        &run_libra_command(
            &["commit", "-m", "Add remote content", "--no-verify"],
            temp_path,
        ),
        "commit",
    );

    let _guard = ChangeDirGuard::new(temp_path);
    let feature_commit = Head::current_commit()
        .await
        .expect("feature branch should have a tip");
    Branch::update_branch("feature", &feature_commit.to_string(), Some("origin"))
        .await
        .unwrap();

    assert_cli_success(
        &run_libra_command(&["checkout", "main"], temp_path),
        "checkout main",
    );

    let output = run_libra_command(
        &["--json", "merge", "refs/remotes/origin/feature"],
        temp_path,
    );
    assert_cli_success(&output, "json merge remote branch");

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "merge");
    assert_eq!(json["data"]["strategy"], "fast-forward");
    assert_eq!(json["data"]["up_to_date"], false);
    assert_eq!(json["data"]["files_changed"], 1);
    assert!(json["data"]["commit"].as_str().is_some());
    assert!(output.stderr.is_empty());
}

#[tokio::test]
#[serial]
/// Test merging diverged branches with non-overlapping changes.
async fn test_merge_diverged_branch_creates_two_parent_commit() {
    let temp_repo = create_committed_repo_via_cli();
    let temp_path = temp_repo.path();

    let output = run_libra_command(&["branch", "branch1"], temp_path);
    assert!(output.status.success(), "Failed to create branch1");

    let output = run_libra_command(&["checkout", "branch1"], temp_path);
    assert!(output.status.success(), "Failed to checkout branch1");

    commit_file(
        temp_path,
        "branch1.txt",
        "Branch1 content",
        "Add branch1 content",
    );

    let output = run_libra_command(&["checkout", "main"], temp_path);
    assert!(output.status.success(), "Failed to checkout main");

    let output = run_libra_command(&["branch", "branch2"], temp_path);
    assert!(output.status.success(), "Failed to create branch2");

    let output = run_libra_command(&["checkout", "branch2"], temp_path);
    assert!(output.status.success(), "Failed to checkout branch2");

    commit_file(
        temp_path,
        "branch2.txt",
        "Branch2 content",
        "Add branch2 content",
    );

    let output = run_libra_command(&["checkout", "branch1"], temp_path);
    assert!(output.status.success(), "Failed to checkout branch1");

    let merge_output = run_libra_command(&["merge", "branch2"], temp_path);
    assert_cli_success(&merge_output, "three-way merge");
    let stdout = String::from_utf8_lossy(&merge_output.stdout);
    assert!(
        stdout.contains("Merge made by the 'three-way' strategy."),
        "merge should report three-way strategy, stdout: {stdout}"
    );
    assert_eq!(
        std::fs::read_to_string(temp_path.join("branch1.txt")).expect("read branch1"),
        "Branch1 content"
    );
    assert_eq!(
        std::fs::read_to_string(temp_path.join("branch2.txt")).expect("read branch2"),
        "Branch2 content"
    );

    let _guard = ChangeDirGuard::new(temp_path);
    let head = Head::current_commit()
        .await
        .expect("merge should create HEAD");
    let commit: Commit = load_object(&head).expect("load merge commit");
    assert_eq!(
        commit.parent_commit_ids.len(),
        2,
        "diverged merge should create a two-parent commit"
    );
}

#[test]
#[serial]
fn test_merge_diverged_nested_directory_file_survives_three_way() {
    let temp_repo = create_committed_repo_via_cli();
    let temp_path = temp_repo.path();

    assert_cli_success(
        &run_libra_command(&["branch", "feature"], temp_path),
        "create feature",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "feature"], temp_path),
        "checkout feature",
    );
    commit_file(
        temp_path,
        "nested/feature.txt",
        "feature nested\n",
        "feature nested",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "main"], temp_path),
        "checkout main",
    );
    commit_file(temp_path, "main.txt", "main\n", "main change");

    let output = run_libra_command(&["merge", "feature"], temp_path);
    assert_cli_success(&output, "nested three-way merge");
    assert_eq!(
        std::fs::read_to_string(temp_path.join("nested").join("feature.txt"))
            .expect("read nested feature file"),
        "feature nested\n"
    );
}

#[test]
#[serial]
/// Test JSON envelope for a clean three-way merge.
fn test_merge_json_diverged_branch_outputs_three_way_summary() {
    let temp_repo = create_committed_repo_via_cli();
    let temp_path = temp_repo.path();

    let output = run_libra_command(&["branch", "branch1"], temp_path);
    assert!(output.status.success(), "Failed to create branch1");

    let output = run_libra_command(&["checkout", "branch1"], temp_path);
    assert!(output.status.success(), "Failed to checkout branch1");

    commit_file(
        temp_path,
        "branch1.txt",
        "Branch1 content",
        "Add branch1 content",
    );

    let output = run_libra_command(&["checkout", "main"], temp_path);
    assert!(output.status.success(), "Failed to checkout main");

    let output = run_libra_command(&["branch", "branch2"], temp_path);
    assert!(output.status.success(), "Failed to create branch2");

    let output = run_libra_command(&["checkout", "branch2"], temp_path);
    assert!(output.status.success(), "Failed to checkout branch2");

    commit_file(
        temp_path,
        "branch2.txt",
        "Branch2 content",
        "Add branch2 content",
    );

    let output = run_libra_command(&["checkout", "branch1"], temp_path);
    assert!(output.status.success(), "Failed to checkout branch1");

    let merge_output = run_libra_command(&["--json", "merge", "branch2"], temp_path);
    assert_cli_success(&merge_output, "json three-way merge");
    assert!(merge_output.stderr.is_empty());
    let json = parse_json_stdout(&merge_output);
    assert_eq!(json["command"], "merge");
    assert_eq!(json["data"]["strategy"], "three-way");
    assert_eq!(json["data"]["up_to_date"], false);
    assert_eq!(
        json["data"]["parents"].as_array().expect("parents").len(),
        2
    );
    assert!(
        json["data"]["commit"].as_str().is_some(),
        "json should report the merge commit: {json}"
    );
}

#[test]
#[serial]
fn test_merge_conflict_writes_markers_status_hints_and_abort_restores() {
    let temp_repo = create_committed_repo_via_cli();
    let temp_path = temp_repo.path();

    assert_cli_success(
        &run_libra_command(&["branch", "feature"], temp_path),
        "create feature",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "feature"], temp_path),
        "checkout feature",
    );
    commit_file(
        temp_path,
        "tracked.txt",
        "feature change\n",
        "feature change",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "main"], temp_path),
        "checkout main",
    );
    commit_file(temp_path, "tracked.txt", "main change\n", "main change");

    let output = run_libra_command(&["merge", "feature"], temp_path);
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-CONFLICT-002");
    assert!(stderr.contains("merge has conflicts in tracked.txt"));
    assert!(
        report
            .hints
            .iter()
            .any(|hint| hint.contains("libra merge --continue")),
        "conflict error should hint continue: {:?}",
        report.hints
    );

    let conflicted = std::fs::read_to_string(temp_path.join("tracked.txt")).expect("read conflict");
    assert!(conflicted.contains("<<<<<<< HEAD"), "{conflicted}");
    assert!(conflicted.contains("======="), "{conflicted}");
    assert!(conflicted.contains(">>>>>>>"), "{conflicted}");

    let status = run_libra_command(&["status"], temp_path);
    assert_cli_success(&status, "status during merge");
    let status_stdout = String::from_utf8_lossy(&status.stdout);
    assert!(
        status_stdout.contains("You are in the middle of a merge with 'feature'."),
        "status should mention merge state, stdout: {status_stdout}"
    );
    assert!(status_stdout.contains("libra merge --continue"));
    assert!(status_stdout.contains("libra merge --abort"));

    let abort = run_libra_command(&["merge", "--abort"], temp_path);
    assert_cli_success(&abort, "merge abort");
    assert_eq!(
        std::fs::read_to_string(temp_path.join("tracked.txt")).expect("read restored file"),
        "main change\n"
    );
    assert!(
        !temp_path.join(".libra").join("merge-state.json").exists(),
        "abort should remove merge state"
    );
}

#[tokio::test]
#[serial]
async fn test_merge_continue_after_resolving_conflict_creates_two_parent_commit() {
    let temp_repo = create_committed_repo_via_cli();
    let temp_path = temp_repo.path();

    assert_cli_success(
        &run_libra_command(&["branch", "feature"], temp_path),
        "create feature",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "feature"], temp_path),
        "checkout feature",
    );
    commit_file(
        temp_path,
        "tracked.txt",
        "feature change\n",
        "feature change",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "main"], temp_path),
        "checkout main",
    );
    commit_file(temp_path, "tracked.txt", "main change\n", "main change");

    let output = run_libra_command(&["merge", "feature"], temp_path);
    assert_eq!(output.status.code(), Some(128));

    std::fs::write(temp_path.join("tracked.txt"), "resolved change\n").expect("write resolution");
    assert_cli_success(
        &run_libra_command(&["add", "tracked.txt"], temp_path),
        "stage resolution",
    );
    let status = run_libra_command(&["status"], temp_path);
    assert_cli_success(&status, "status after staged resolution");
    let status_stdout = String::from_utf8_lossy(&status.stdout);
    assert!(
        status_stdout.contains("all conflicts fixed"),
        "status should acknowledge staged conflict resolution, stdout: {status_stdout}"
    );
    let continued = run_libra_command(&["merge", "--continue"], temp_path);
    assert_cli_success(&continued, "merge continue");
    let stdout = String::from_utf8_lossy(&continued.stdout);
    assert!(stdout.contains("Merge completed."), "stdout: {stdout}");

    let _guard = ChangeDirGuard::new(temp_path);
    let head = Head::current_commit()
        .await
        .expect("merge continue should create HEAD");
    let commit: Commit = load_object(&head).expect("load continued merge commit");
    assert_eq!(commit.parent_commit_ids.len(), 2);
    assert_eq!(
        std::fs::read_to_string(temp_path.join("tracked.txt")).expect("read resolved file"),
        "resolved change\n"
    );
    assert!(!temp_path.join(".libra").join("merge-state.json").exists());
}

#[test]
#[serial]
fn test_merge_continue_refuses_unstaged_resolution_edits() {
    let temp_repo = create_committed_repo_via_cli();
    let temp_path = temp_repo.path();

    assert_cli_success(
        &run_libra_command(&["branch", "feature"], temp_path),
        "create feature",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "feature"], temp_path),
        "checkout feature",
    );
    commit_file(
        temp_path,
        "tracked.txt",
        "feature change\n",
        "feature change",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "main"], temp_path),
        "checkout main",
    );
    commit_file(temp_path, "tracked.txt", "main change\n", "main change");

    let output = run_libra_command(&["merge", "feature"], temp_path);
    assert_eq!(output.status.code(), Some(128));

    std::fs::write(temp_path.join("tracked.txt"), "staged resolution\n").expect("write resolution");
    assert_cli_success(
        &run_libra_command(&["add", "tracked.txt"], temp_path),
        "stage resolution",
    );
    std::fs::write(temp_path.join("tracked.txt"), "unstaged follow-up\n")
        .expect("write unstaged follow-up");

    let continued = run_libra_command(&["merge", "--continue"], temp_path);
    let (_stderr, report) = parse_cli_error_stderr(&continued.stderr);
    assert_eq!(continued.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-CONFLICT-002");
    assert!(report.message.contains("uncommitted changes"));
    assert_eq!(
        std::fs::read_to_string(temp_path.join("tracked.txt")).expect("read follow-up"),
        "unstaged follow-up\n"
    );
}

#[test]
#[serial]
fn test_merge_dirty_worktree_refuses_before_state() {
    let temp_repo = create_committed_repo_via_cli();
    let temp_path = temp_repo.path();

    assert_cli_success(
        &run_libra_command(&["branch", "feature"], temp_path),
        "create feature",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "feature"], temp_path),
        "checkout feature",
    );
    commit_file(temp_path, "feature.txt", "feature\n", "feature change");
    assert_cli_success(
        &run_libra_command(&["checkout", "main"], temp_path),
        "checkout main",
    );
    commit_file(temp_path, "main.txt", "main\n", "main change");
    std::fs::write(temp_path.join("tracked.txt"), "dirty\n").expect("write dirty file");

    let output = run_libra_command(&["merge", "feature"], temp_path);
    let (_stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-CONFLICT-002");
    assert!(report.message.contains("uncommitted changes"));
    assert!(
        !temp_path.join(".libra").join("merge-state.json").exists(),
        "dirty refusal should not create merge state"
    );
}

#[test]
#[serial]
fn test_merge_untracked_overwrite_refuses_before_head_update() {
    let temp_repo = create_committed_repo_via_cli();
    let temp_path = temp_repo.path();

    assert_cli_success(
        &run_libra_command(&["branch", "feature"], temp_path),
        "create feature",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "feature"], temp_path),
        "checkout feature",
    );
    commit_file(
        temp_path,
        "clobber.txt",
        "from feature\n",
        "feature clobber",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "main"], temp_path),
        "checkout main",
    );
    std::fs::write(temp_path.join("clobber.txt"), "untracked local\n")
        .expect("write untracked clobber");

    let output = run_libra_command(&["merge", "feature"], temp_path);
    let (_stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-CONFLICT-002");
    assert!(
        report
            .message
            .contains("untracked working tree file would be overwritten"),
        "message: {}",
        report.message
    );
    assert_eq!(
        std::fs::read_to_string(temp_path.join("clobber.txt")).expect("read untracked file"),
        "untracked local\n"
    );
    assert!(!temp_path.join(".libra").join("merge-state.json").exists());
}

/// `libra merge --help` surfaces the EXAMPLES banner so users see the
/// supported fast-forward / remote-ref / JSON forms before hitting the
/// `MergeNonFastForward` runtime error. Cross-cutting `--help` EXAMPLES
/// rollout per `docs/improvement/README.md` item B.
#[test]
fn test_merge_help_lists_examples_banner() {
    let repo = tempfile::tempdir().expect("tempdir for merge --help");
    let output = run_libra_command(&["merge", "--help"], repo.path());
    assert!(
        output.status.success(),
        "merge --help should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("EXAMPLES:"),
        "merge --help should include EXAMPLES banner, stdout: {stdout}"
    );
    assert!(
        stdout.contains("NOTES:"),
        "merge --help should call out the non-fast-forward limitation, stdout: {stdout}"
    );
    for invocation in [
        "libra merge feature-x",
        "libra merge origin/main",
        "libra merge --json",
    ] {
        assert!(
            stdout.contains(invocation),
            "merge --help should include `{invocation}`, stdout: {stdout}"
        );
    }
}
