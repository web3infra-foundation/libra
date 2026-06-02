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
    run_libra_command, run_libra_command_with_stdin_and_env,
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
    assert!(
        commit.message.starts_with('\n'),
        "merge commit body must retain Git's blank-line separator before the message"
    );
}

#[tokio::test]
#[serial]
async fn test_merge_same_file_non_overlapping_edits_merges_without_conflict() {
    let temp_repo = create_committed_repo_via_cli();
    let temp_path = temp_repo.path();

    commit_file(
        temp_path,
        "tracked.txt",
        "line 1\nline 2\nline 3\nline 4\nline 5\n",
        "Prepare shared merge fixture",
    );

    let output = run_libra_command(&["branch", "feature"], temp_path);
    assert_cli_success(&output, "create feature");

    let output = run_libra_command(&["checkout", "feature"], temp_path);
    assert_cli_success(&output, "checkout feature");

    commit_file(
        temp_path,
        "tracked.txt",
        "line 1\nline 2\nline 3\nline 4\nline 5 from feature\n",
        "Edit last line on feature",
    );

    let output = run_libra_command(&["checkout", "main"], temp_path);
    assert_cli_success(&output, "checkout main");

    commit_file(
        temp_path,
        "tracked.txt",
        "line 1 from main\nline 2\nline 3\nline 4\nline 5\n",
        "Edit first line on main",
    );

    let merge_output = run_libra_command(&["merge", "feature"], temp_path);
    assert_cli_success(&merge_output, "non-overlapping same-file merge");

    let merged = std::fs::read_to_string(temp_path.join("tracked.txt")).expect("read merged file");
    assert_eq!(
        merged, "line 1 from main\nline 2\nline 3\nline 4\nline 5 from feature\n",
        "non-overlapping same-file edits should merge without conflict markers"
    );
    assert!(
        !merged.contains("<<<<<<<") && !merged.contains("=======") && !merged.contains(">>>>>>>"),
        "clean same-file merge must not leave conflict markers: {merged}"
    );
    assert!(
        !temp_path.join(".libra").join("merge-state.json").exists(),
        "clean same-file merge must not leave merge state"
    );

    let _guard = ChangeDirGuard::new(temp_path);
    let head = Head::current_commit()
        .await
        .expect("merge should create HEAD");
    let commit: Commit = load_object(&head).expect("load merge commit");
    assert_eq!(
        commit.parent_commit_ids.len(),
        2,
        "clean same-file merge should create a two-parent commit"
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

#[tokio::test]
#[serial]
async fn test_merge_ff_only_refuses_diverged_branch() {
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

    let output = run_libra_command(&["merge", "--ff-only", "feature"], temp_path);
    let (_stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-CONFLICT-002");
    assert!(report.message.contains("non-fast-forward merge refused"));
}

#[tokio::test]
#[serial]
async fn test_merge_no_ff_creates_merge_commit_for_fast_forwardable_branch() {
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

    let output = run_libra_command(&["merge", "--no-ff", "feature"], temp_path);
    assert_cli_success(&output, "no-ff merge");

    let _guard = ChangeDirGuard::new(temp_path);
    let head = Head::current_commit()
        .await
        .expect("merge should create HEAD");
    let commit: Commit = load_object(&head).expect("load merge commit");
    assert_eq!(commit.parent_commit_ids.len(), 2);
}

#[tokio::test]
#[serial]
async fn test_merge_message_file_and_signoff_set_merge_commit_message() {
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
    std::fs::write(temp_path.join("merge-message.txt"), "Custom merge\n")
        .expect("write message file");

    let output = run_libra_command(
        &["merge", "-F", "merge-message.txt", "--signoff", "feature"],
        temp_path,
    );
    assert_cli_success(&output, "merge with message file and signoff");

    let _guard = ChangeDirGuard::new(temp_path);
    let head = Head::current_commit()
        .await
        .expect("merge should create HEAD");
    let commit: Commit = load_object(&head).expect("load merge commit");
    assert!(
        commit.message.contains("Custom merge"),
        "{}",
        commit.message
    );
    assert!(
        commit
            .message
            .contains("Signed-off-by: Test User <test@example.com>"),
        "{}",
        commit.message
    );
}

#[test]
#[serial]
fn test_merge_strategy_ours_keeps_our_conflicting_file() {
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

    let output = run_libra_command(&["merge", "-s", "ours", "feature"], temp_path);
    assert_cli_success(&output, "ours strategy merge");

    assert_eq!(
        std::fs::read_to_string(temp_path.join("tracked.txt")).expect("read tracked"),
        "main change\n"
    );
}

#[test]
#[serial]
fn test_merge_strategy_option_theirs_resolves_text_conflict() {
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

    let output = run_libra_command(&["merge", "-X", "theirs", "feature"], temp_path);
    assert_cli_success(&output, "theirs strategy option merge");

    assert_eq!(
        std::fs::read_to_string(temp_path.join("tracked.txt")).expect("read tracked"),
        "feature change\n"
    );
}

#[test]
#[serial]
fn test_merge_quit_forgets_state_without_restoring_conflict_file() {
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
    let conflicted = std::fs::read_to_string(temp_path.join("tracked.txt")).expect("read conflict");
    assert!(conflicted.contains("<<<<<<< HEAD"));

    let quit = run_libra_command(&["merge", "--quit"], temp_path);
    assert_cli_success(&quit, "merge quit");

    assert!(!temp_path.join(".libra").join("merge-state.json").exists());
    let after_quit = std::fs::read_to_string(temp_path.join("tracked.txt")).expect("read conflict");
    assert!(after_quit.contains("<<<<<<< HEAD"));
}

#[test]
#[serial]
fn test_merge_binary_conflict_does_not_run_text_auto_merge() {
    let temp_repo = create_committed_repo_via_cli();
    let temp_path = temp_repo.path();

    std::fs::write(temp_path.join("binary.dat"), b"base\0data\n").expect("write base binary");
    assert_cli_success(
        &run_libra_command(&["add", "binary.dat"], temp_path),
        "add binary",
    );
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "binary base", "--no-verify"], temp_path),
        "commit binary base",
    );
    assert_cli_success(
        &run_libra_command(&["branch", "feature"], temp_path),
        "create feature",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "feature"], temp_path),
        "checkout feature",
    );
    std::fs::write(temp_path.join("binary.dat"), b"feature\0data\n").expect("write feature binary");
    assert_cli_success(
        &run_libra_command(&["add", "binary.dat"], temp_path),
        "add feature binary",
    );
    assert_cli_success(
        &run_libra_command(
            &["commit", "-m", "feature binary", "--no-verify"],
            temp_path,
        ),
        "commit feature binary",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "main"], temp_path),
        "checkout main",
    );
    std::fs::write(temp_path.join("binary.dat"), b"main\0data\n").expect("write main binary");
    assert_cli_success(
        &run_libra_command(&["add", "binary.dat"], temp_path),
        "add main binary",
    );
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "main binary", "--no-verify"], temp_path),
        "commit main binary",
    );

    let output = run_libra_command(&["merge", "feature"], temp_path);
    let (_stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-CONFLICT-002");
    let conflicted = std::fs::read_to_string(temp_path.join("binary.dat")).expect("read marker");
    assert!(conflicted.contains("[binary content,"), "{conflicted}");
}

#[tokio::test]
#[serial]
async fn test_merge_squash_updates_index_and_worktree_without_moving_head() {
    let temp_repo = create_committed_repo_via_cli();
    let temp_path = temp_repo.path();

    let _guard = ChangeDirGuard::new(temp_path);
    let original_head = Head::current_commit()
        .await
        .expect("base repository should have HEAD");
    drop(_guard);

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

    let output = run_libra_command(&["merge", "--squash", "feature"], temp_path);
    assert_cli_success(&output, "squash merge");

    assert_eq!(
        std::fs::read_to_string(temp_path.join("feature.txt")).expect("read squash result"),
        "feature\n"
    );
    assert!(!temp_path.join(".libra").join("merge-state.json").exists());
    let _guard = ChangeDirGuard::new(temp_path);
    let head_after = Head::current_commit()
        .await
        .expect("repository should still have HEAD");
    assert_eq!(head_after, original_head);
}

#[test]
#[serial]
fn test_merge_squash_no_ff_is_invalid() {
    let temp_repo = create_committed_repo_via_cli();
    let temp_path = temp_repo.path();

    let output = run_libra_command(&["merge", "--squash", "--no-ff", "main"], temp_path);
    let (_stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(129));
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert!(
        report
            .message
            .contains("--squash cannot be combined with --no-ff")
    );
}

#[tokio::test]
#[serial]
async fn test_merge_no_commit_no_ff_leaves_state_and_continue_commits() {
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
    let _guard = ChangeDirGuard::new(temp_path);
    let original_head = Head::current_commit().await.expect("main should have HEAD");
    drop(_guard);

    let output = run_libra_command(&["merge", "--no-ff", "--no-commit", "feature"], temp_path);
    assert_cli_success(&output, "no-commit merge");
    assert!(temp_path.join(".libra").join("merge-state.json").exists());
    assert_eq!(
        std::fs::read_to_string(temp_path.join("feature.txt")).expect("read no-commit result"),
        "feature\n"
    );
    let _guard = ChangeDirGuard::new(temp_path);
    let uncommitted_head = Head::current_commit()
        .await
        .expect("main should still have HEAD");
    assert_eq!(uncommitted_head, original_head);
    drop(_guard);

    let continued = run_libra_command(&["merge", "--continue"], temp_path);
    assert_cli_success(&continued, "continue no-commit merge");
    let _guard = ChangeDirGuard::new(temp_path);
    let new_head = Head::current_commit()
        .await
        .expect("continue should create HEAD");
    let commit: Commit = load_object(&new_head).expect("load merge commit");
    assert_eq!(commit.parent_commit_ids.len(), 2);
}

#[tokio::test]
#[serial]
async fn test_merge_ff_false_config_forces_merge_commit_and_cli_ff_only_overrides() {
    let temp_repo = create_committed_repo_via_cli();
    let temp_path = temp_repo.path();

    assert_cli_success(
        &run_libra_command(&["config", "merge.ff", "false"], temp_path),
        "set merge.ff false",
    );
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

    let ff_only = run_libra_command(&["merge", "--ff-only", "feature"], temp_path);
    assert_cli_success(&ff_only, "ff-only overrides merge.ff false");
    let _guard = ChangeDirGuard::new(temp_path);
    let head = Head::current_commit()
        .await
        .expect("ff-only merge should leave HEAD");
    let commit: Commit = load_object(&head).expect("load fast-forward commit");
    assert_eq!(commit.parent_commit_ids.len(), 1);
}

#[tokio::test]
#[serial]
async fn test_merge_log_appends_shortlog_to_merge_commit_message() {
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
        "feature.txt",
        "feature\n",
        "feat: feature change",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "main"], temp_path),
        "checkout main",
    );
    commit_file(temp_path, "main.txt", "main\n", "main change");

    let output = run_libra_command(&["merge", "--log=1", "feature"], temp_path);
    assert_cli_success(&output, "merge with shortlog");

    let _guard = ChangeDirGuard::new(temp_path);
    let head = Head::current_commit()
        .await
        .expect("merge should create HEAD");
    let commit: Commit = load_object(&head).expect("load merge commit");
    assert!(
        commit.message.contains("feat: feature change"),
        "{}",
        commit.message
    );
}

#[test]
#[serial]
fn test_merge_conflict_diff3_includes_base_content() {
    let temp_repo = create_committed_repo_via_cli();
    let temp_path = temp_repo.path();

    commit_file(temp_path, "tracked.txt", "base\n", "base tracked");
    assert_cli_success(
        &run_libra_command(&["branch", "feature"], temp_path),
        "create feature",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "feature"], temp_path),
        "checkout feature",
    );
    commit_file(temp_path, "tracked.txt", "feature\n", "feature change");
    assert_cli_success(
        &run_libra_command(&["checkout", "main"], temp_path),
        "checkout main",
    );
    commit_file(temp_path, "tracked.txt", "main\n", "main change");

    let output = run_libra_command(&["merge", "--conflict", "diff3", "feature"], temp_path);
    assert_eq!(output.status.code(), Some(128));
    let conflicted = std::fs::read_to_string(temp_path.join("tracked.txt")).expect("read conflict");
    assert!(conflicted.contains("|||||||"), "{conflicted}");
    assert!(conflicted.contains("base"), "{conflicted}");
}

#[test]
fn test_merge_help_accepts_stat_flags() {
    let repo = tempfile::tempdir().expect("tempdir for merge --help");
    let output = run_libra_command(&["merge", "--help"], repo.path());
    assert_cli_success(&output, "merge help");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--stat"), "{stdout}");
    assert!(stdout.contains("--no-stat"), "{stdout}");
}

#[tokio::test]
#[serial]
async fn test_merge_octopus_clean_disjoint_changes_creates_n_parent_commit() {
    let temp_repo = create_committed_repo_via_cli();
    let temp_path = temp_repo.path();

    assert_cli_success(
        &run_libra_command(&["branch", "left"], temp_path),
        "create left",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "left"], temp_path),
        "checkout left",
    );
    commit_file(temp_path, "left.txt", "left\n", "left change");
    assert_cli_success(
        &run_libra_command(&["checkout", "main"], temp_path),
        "checkout main",
    );
    assert_cli_success(
        &run_libra_command(&["branch", "right"], temp_path),
        "create right",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "right"], temp_path),
        "checkout right",
    );
    commit_file(temp_path, "right.txt", "right\n", "right change");
    assert_cli_success(
        &run_libra_command(&["checkout", "main"], temp_path),
        "checkout main",
    );

    let output = run_libra_command(&["merge", "left", "right"], temp_path);
    assert_cli_success(&output, "octopus merge");
    assert_eq!(
        std::fs::read_to_string(temp_path.join("left.txt")).expect("read left"),
        "left\n"
    );
    assert_eq!(
        std::fs::read_to_string(temp_path.join("right.txt")).expect("read right"),
        "right\n"
    );

    let _guard = ChangeDirGuard::new(temp_path);
    let head = Head::current_commit()
        .await
        .expect("octopus should create HEAD");
    let commit: Commit = load_object(&head).expect("load octopus merge commit");
    assert_eq!(commit.parent_commit_ids.len(), 3);
}

#[test]
#[serial]
fn test_merge_directory_file_conflict_is_refused_before_writing_merge_state() {
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
        "path/file.txt",
        "feature\n",
        "feature directory path",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "main"], temp_path),
        "checkout main",
    );
    commit_file(temp_path, "path", "main file\n", "main file path");

    let output = run_libra_command(&["merge", "feature"], temp_path);
    let (_stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-CONFLICT-002");
    assert!(
        report.message.contains("directory/file conflict"),
        "{}",
        report.message
    );
    assert!(!temp_path.join(".libra").join("merge-state.json").exists());
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
    assert!(
        commit.message.starts_with('\n'),
        "merge --continue commit body must retain Git's blank-line separator before the message"
    );
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

#[tokio::test]
#[serial]
async fn test_merge_into_name_overrides_message_branch_name() {
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

    let output = run_libra_command(
        &["merge", "--into-name", "release-1.0", "feature"],
        temp_path,
    );
    assert_cli_success(&output, "merge with --into-name");

    let _guard = ChangeDirGuard::new(temp_path);
    let head = Head::current_commit()
        .await
        .expect("merge should create HEAD");
    let commit: Commit = load_object(&head).expect("load merge commit");
    assert!(
        commit.message.contains("into release-1.0"),
        "merge message should honor --into-name: {}",
        commit.message
    );
}

#[test]
fn test_merge_diff_algorithm_rejects_unknown_value() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["merge", "--diff-algorithm", "bogus", "main"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(129));
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert!(
        stderr.contains("unknown diff algorithm 'bogus'"),
        "{stderr}"
    );
}

#[test]
fn test_merge_diff_algorithm_accepts_known_value() {
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

    let output = run_libra_command(
        &["merge", "--diff-algorithm", "histogram", "feature"],
        temp_path,
    );
    assert_cli_success(&output, "merge with --diff-algorithm histogram");
}

#[test]
fn test_merge_cleanup_rejects_unknown_mode() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["merge", "--cleanup", "bogus", "main"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(129));
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert!(stderr.contains("unknown cleanup mode 'bogus'"), "{stderr}");
}

#[test]
fn test_merge_help_accepts_git_compat_flags() {
    let repo = tempfile::tempdir().expect("tempdir for merge --help");
    let output = run_libra_command(&["merge", "--help"], repo.path());
    assert_cli_success(&output, "merge help");
    let stdout = String::from_utf8_lossy(&output.stdout);
    for flag in [
        "--into-name",
        "--no-log",
        "--no-signoff",
        "--no-squash",
        "--diff-algorithm",
        "--cleanup",
        "--no-verify",
        "--overwrite-ignore",
        "--no-overwrite-ignore",
        "--rerere-autoupdate",
        "--no-rerere-autoupdate",
    ] {
        assert!(
            stdout.contains(flag),
            "merge --help should list {flag}, stdout: {stdout}"
        );
    }
}

#[test]
fn test_merge_stat_reports_diffstat() {
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
    commit_file(temp_path, "feature.txt", "alpha\nbeta\n", "feat change");
    assert_cli_success(
        &run_libra_command(&["checkout", "main"], temp_path),
        "checkout main",
    );
    commit_file(temp_path, "main.txt", "main\n", "main change");

    let output = run_libra_command(&["merge", "--stat", "feature"], temp_path);
    assert_cli_success(&output, "merge --stat");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("feature.txt"),
        "stat should list feature.txt: {stdout}"
    );
    assert!(
        stdout.contains("file") && stdout.contains("changed"),
        "stat should show a summary line: {stdout}"
    );
    assert!(
        stdout.contains("insertion"),
        "stat should report insertions: {stdout}"
    );
}

#[test]
fn test_merge_summary_alias_reports_diffstat() {
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
    commit_file(temp_path, "feature.txt", "alpha\n", "feat change");
    assert_cli_success(
        &run_libra_command(&["checkout", "main"], temp_path),
        "checkout main",
    );
    commit_file(temp_path, "main.txt", "main\n", "main change");

    let output = run_libra_command(&["merge", "--summary", "feature"], temp_path);
    assert_cli_success(&output, "merge --summary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("changed"),
        "--summary alias should print a diffstat summary: {stdout}"
    );
}

#[test]
fn test_merge_no_stat_default_omits_diffstat() {
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
    commit_file(temp_path, "feature.txt", "alpha\n", "feat change");
    assert_cli_success(
        &run_libra_command(&["checkout", "main"], temp_path),
        "checkout main",
    );
    commit_file(temp_path, "main.txt", "main\n", "main change");

    let output = run_libra_command(&["merge", "feature"], temp_path);
    assert_cli_success(&output, "merge without --stat");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("changed,"),
        "default merge output should not include a diffstat: {stdout}"
    );
}

#[test]
fn test_merge_autostash_preserves_local_changes_on_fast_forward() {
    let temp_repo = create_committed_repo_via_cli();
    let temp_path = temp_repo.path();

    commit_file(temp_path, "tracked.txt", "base\n", "base tracked");
    assert_cli_success(
        &run_libra_command(&["branch", "feature"], temp_path),
        "create feature",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "feature"], temp_path),
        "checkout feature",
    );
    commit_file(temp_path, "feature.txt", "feature\n", "feat add");
    assert_cli_success(
        &run_libra_command(&["checkout", "main"], temp_path),
        "checkout main",
    );

    // Uncommitted local change that would normally block the merge.
    std::fs::write(temp_path.join("tracked.txt"), "base\nlocal-dirty\n").expect("write dirty");

    let output = run_libra_command(&["merge", "--autostash", "feature"], temp_path);
    assert_cli_success(&output, "merge --autostash");

    assert!(
        temp_path.join("feature.txt").exists(),
        "fast-forward should bring in feature.txt"
    );
    let tracked = std::fs::read_to_string(temp_path.join("tracked.txt")).expect("read tracked");
    assert!(
        tracked.contains("local-dirty"),
        "autostash should reapply local changes: {tracked}"
    );
}

#[tokio::test]
#[serial]
async fn test_merge_autostash_preserves_local_changes_on_three_way() {
    let temp_repo = create_committed_repo_via_cli();
    let temp_path = temp_repo.path();

    commit_file(temp_path, "tracked.txt", "base\n", "base tracked");
    assert_cli_success(
        &run_libra_command(&["branch", "feature"], temp_path),
        "create feature",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "feature"], temp_path),
        "checkout feature",
    );
    commit_file(temp_path, "feature.txt", "feature\n", "feat add");
    assert_cli_success(
        &run_libra_command(&["checkout", "main"], temp_path),
        "checkout main",
    );
    commit_file(temp_path, "main.txt", "main\n", "main change");

    std::fs::write(temp_path.join("tracked.txt"), "base\nlocal-dirty\n").expect("write dirty");

    let output = run_libra_command(&["merge", "--autostash", "feature"], temp_path);
    assert_cli_success(&output, "merge --autostash three-way");

    let tracked = std::fs::read_to_string(temp_path.join("tracked.txt")).expect("read tracked");
    assert!(
        tracked.contains("local-dirty"),
        "three-way autostash should reapply local changes: {tracked}"
    );

    let _guard = ChangeDirGuard::new(temp_path);
    let head = Head::current_commit()
        .await
        .expect("merge should create HEAD");
    let commit: Commit = load_object(&head).expect("load merge commit");
    assert_eq!(
        commit.parent_commit_ids.len(),
        2,
        "three-way merge should produce a two-parent commit"
    );
}

#[test]
fn test_merge_ignore_all_space_resolves_whitespace_only_side() {
    let temp_repo = create_committed_repo_via_cli();
    let temp_path = temp_repo.path();

    commit_file(temp_path, "code.txt", "fn main() {}\n", "base code");
    assert_cli_success(
        &run_libra_command(&["branch", "feature"], temp_path),
        "create feature",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "feature"], temp_path),
        "checkout feature",
    );
    // theirs: a real semantic change
    commit_file(
        temp_path,
        "code.txt",
        "fn main() { run(); }\n",
        "real change",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "main"], temp_path),
        "checkout main",
    );
    // ours: whitespace-only reformatting of the same line
    commit_file(temp_path, "code.txt", "fn  main()  {}\n", "whitespace only");

    let output = run_libra_command(&["merge", "--ignore-all-space", "feature"], temp_path);
    assert_cli_success(&output, "merge --ignore-all-space");

    let merged = std::fs::read_to_string(temp_path.join("code.txt")).expect("read merged code");
    assert!(
        merged.contains("run();"),
        "the real change should win over whitespace-only edits: {merged}"
    );
    assert!(
        !merged.contains("<<<<<<<"),
        "whitespace-only side should not conflict: {merged}"
    );
}

#[test]
fn test_merge_help_lists_whitespace_flags() {
    let repo = tempfile::tempdir().expect("tempdir for merge --help");
    let output = run_libra_command(&["merge", "--help"], repo.path());
    assert_cli_success(&output, "merge help");
    let stdout = String::from_utf8_lossy(&output.stdout);
    for flag in [
        "--ignore-space-change",
        "--ignore-all-space",
        "--ignore-space-at-eol",
        "--ignore-cr-at-eol",
        "--autostash",
        "--gpg-sign",
        "--no-gpg-sign",
        "--verify-signatures",
        "--no-verify-signatures",
        "--find-renames",
        "--no-renames",
    ] {
        assert!(
            stdout.contains(flag),
            "merge --help should list {flag}: {stdout}"
        );
    }
}

#[test]
fn test_merge_verify_signatures_rejects_unsigned_target() {
    let temp_repo = create_committed_repo_via_cli();
    let temp_path = temp_repo.path();

    // Disable vault signing so the feature commit is unsigned.
    assert_cli_success(
        &run_libra_command(&["config", "set", "vault.signing", "false"], temp_path),
        "disable signing",
    );
    assert_cli_success(
        &run_libra_command(&["branch", "feature"], temp_path),
        "create feature",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "feature"], temp_path),
        "checkout feature",
    );
    commit_file(temp_path, "feature.txt", "feature\n", "feat add");
    assert_cli_success(
        &run_libra_command(&["checkout", "main"], temp_path),
        "checkout main",
    );

    let output = run_libra_command(&["merge", "--verify-signatures", "feature"], temp_path);
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-REPO-003");
    assert!(
        stderr.contains("not signed"),
        "verify-signatures should reject unsigned target: {stderr}"
    );
}

#[tokio::test]
#[serial]
async fn test_merge_gpg_sign_produces_signed_merge_commit() {
    let temp_repo = create_committed_repo_via_cli();
    let temp_path = temp_repo.path();

    // Turn vault signing off so only `-S` can produce a signature.
    assert_cli_success(
        &run_libra_command(&["config", "set", "vault.signing", "false"], temp_path),
        "disable signing",
    );
    assert_cli_success(
        &run_libra_command(&["branch", "feature"], temp_path),
        "create feature",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "feature"], temp_path),
        "checkout feature",
    );
    commit_file(temp_path, "feature.txt", "feature\n", "feat add");
    assert_cli_success(
        &run_libra_command(&["checkout", "main"], temp_path),
        "checkout main",
    );
    commit_file(temp_path, "main.txt", "main\n", "main change");

    let output = run_libra_command(&["merge", "-S", "feature"], temp_path);
    assert_cli_success(&output, "merge -S");

    let _guard = ChangeDirGuard::new(temp_path);
    let head = Head::current_commit()
        .await
        .expect("merge should create HEAD");
    let commit: Commit = load_object(&head).expect("load merge commit");
    assert!(
        commit.message.contains("PGP SIGNATURE"),
        "merge -S should embed a PGP signature: {}",
        commit.message
    );
}

#[test]
fn test_merge_help_lists_edit_flags() {
    let repo = tempfile::tempdir().expect("tempdir for merge --help");
    let output = run_libra_command(&["merge", "--help"], repo.path());
    assert_cli_success(&output, "merge help");
    let stdout = String::from_utf8_lossy(&output.stdout);
    for flag in ["--edit", "--no-edit"] {
        assert!(
            stdout.contains(flag),
            "merge --help should list {flag}: {stdout}"
        );
    }
}

#[test]
fn test_merge_rename_with_edit_on_other_side_merges_cleanly() {
    let temp_repo = create_committed_repo_via_cli();
    let temp_path = temp_repo.path();

    commit_file(temp_path, "old.txt", "line1\nline2\nline3\n", "base file");
    assert_cli_success(
        &run_libra_command(&["branch", "feature"], temp_path),
        "create feature",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "feature"], temp_path),
        "checkout feature",
    );
    // theirs: edit the file in place
    commit_file(
        temp_path,
        "old.txt",
        "line1\nline2-EDITED\nline3\n",
        "edit on feature",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "main"], temp_path),
        "checkout main",
    );
    // ours: rename old.txt -> new.txt with identical content
    std::fs::write(temp_path.join("new.txt"), "line1\nline2\nline3\n").expect("write new.txt");
    assert_cli_success(
        &run_libra_command(&["add", "new.txt"], temp_path),
        "add new",
    );
    assert_cli_success(&run_libra_command(&["rm", "old.txt"], temp_path), "rm old");
    assert_cli_success(
        &run_libra_command(
            &["commit", "-m", "rename on main", "--no-verify"],
            temp_path,
        ),
        "commit rename",
    );

    let output = run_libra_command(&["merge", "feature"], temp_path);
    assert_cli_success(&output, "merge rename+edit");

    assert!(
        !temp_path.join("old.txt").exists(),
        "renamed-away source should be gone"
    );
    let merged = std::fs::read_to_string(temp_path.join("new.txt")).expect("read new.txt");
    assert!(
        merged.contains("line2-EDITED"),
        "the edit should follow the rename: {merged}"
    );
    assert!(
        !merged.contains("<<<<<<<"),
        "rename + edit should not conflict: {merged}"
    );
}

#[test]
fn test_merge_no_renames_falls_back_to_conflict() {
    let temp_repo = create_committed_repo_via_cli();
    let temp_path = temp_repo.path();

    commit_file(temp_path, "old.txt", "line1\nline2\nline3\n", "base file");
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
        "old.txt",
        "line1\nline2-EDITED\nline3\n",
        "edit on feature",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "main"], temp_path),
        "checkout main",
    );
    std::fs::write(temp_path.join("new.txt"), "line1\nline2\nline3\n").expect("write new.txt");
    assert_cli_success(
        &run_libra_command(&["add", "new.txt"], temp_path),
        "add new",
    );
    assert_cli_success(&run_libra_command(&["rm", "old.txt"], temp_path), "rm old");
    assert_cli_success(
        &run_libra_command(
            &["commit", "-m", "rename on main", "--no-verify"],
            temp_path,
        ),
        "commit rename",
    );

    // With rename detection disabled, the delete/modify pair conflicts.
    let output = run_libra_command(&["merge", "--no-renames", "feature"], temp_path);
    assert_eq!(
        output.status.code(),
        Some(128),
        "--no-renames should surface the delete/modify conflict: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn test_merge_edit_rewrites_message_via_editor() {
    use std::os::unix::fs::PermissionsExt;

    let temp_repo = create_committed_repo_via_cli();
    let temp_path = temp_repo.path();

    // A non-interactive "editor" that rewrites the merge message file.
    let editor = temp_path.join("editor.sh");
    std::fs::write(
        &editor,
        "#!/bin/sh\nprintf 'EDITED MERGE MESSAGE\\n' > \"$1\"\n",
    )
    .expect("write editor script");
    std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o755))
        .expect("chmod editor");

    assert_cli_success(
        &run_libra_command(&["branch", "feature"], temp_path),
        "create feature",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "feature"], temp_path),
        "checkout feature",
    );
    commit_file(temp_path, "feature.txt", "feature\n", "feat add");
    assert_cli_success(
        &run_libra_command(&["checkout", "main"], temp_path),
        "checkout main",
    );
    commit_file(temp_path, "main.txt", "main\n", "main change");

    let output = run_libra_command_with_stdin_and_env(
        &["merge", "--edit", "feature"],
        temp_path,
        "",
        &[("GIT_EDITOR", editor.to_str().expect("editor path"))],
    );
    assert_cli_success(&output, "merge --edit");

    let _guard = ChangeDirGuard::new(temp_path);
    let head = Head::current_commit()
        .await
        .expect("merge should create HEAD");
    let commit: Commit = load_object(&head).expect("load merge commit");
    assert!(
        commit.message.contains("EDITED MERGE MESSAGE"),
        "--edit should apply the editor's message: {}",
        commit.message
    );
}
