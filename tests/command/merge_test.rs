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
    assert!(
        commit.message.starts_with('\n'),
        "merge commit body must retain Git's blank-line separator before the message"
    );
}

#[test]
fn test_merge_custom_message_via_dash_m() {
    let temp_repo = create_committed_repo_via_cli();
    let p = temp_repo.path();

    assert!(
        run_libra_command(&["checkout", "-b", "feat"], p)
            .status
            .success(),
        "create+checkout feat"
    );
    commit_file(p, "feat.txt", "feat content", "feat commit");
    assert!(
        run_libra_command(&["checkout", "main"], p).status.success(),
        "checkout main"
    );
    commit_file(p, "main.txt", "main content", "main commit");

    let merge = run_libra_command(&["merge", "-m", "MY CUSTOM MERGE MSG", "feat"], p);
    assert_cli_success(&merge, "merge -m custom feat");

    // The merge commit (HEAD) should carry the custom subject.
    let log = run_libra_command(&["log", "-n", "1", "--pretty=%s"], p);
    assert_cli_success(&log, "log -n 1 --pretty=%s");
    let subject = String::from_utf8_lossy(&log.stdout);
    assert!(
        subject.contains("MY CUSTOM MERGE MSG"),
        "merge commit subject should be the -m message, got: {subject}"
    );
}

#[test]
fn test_merge_squash_stages_without_committing() {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();

    assert!(
        run_libra_command(&["checkout", "-b", "feat"], p)
            .status
            .success(),
        "checkout -b feat"
    );
    commit_file(p, "feat.txt", "feat content", "feat commit");
    assert!(
        run_libra_command(&["checkout", "main"], p).status.success(),
        "checkout main"
    );
    commit_file(p, "main.txt", "main content", "main commit");

    let before = run_libra_command(&["rev-parse", "HEAD"], p);
    let before_head = String::from_utf8_lossy(&before.stdout).trim().to_string();

    let merge = run_libra_command(&["merge", "--squash", "feat"], p);
    assert_cli_success(&merge, "merge --squash feat");
    let merge_out = String::from_utf8_lossy(&merge.stdout);
    assert!(
        merge_out.contains("Squash commit"),
        "expected squash message, got: {merge_out}"
    );

    // --squash must NOT move HEAD, but the merged file must be in the worktree.
    let after = run_libra_command(&["rev-parse", "HEAD"], p);
    assert_eq!(
        String::from_utf8_lossy(&after.stdout).trim(),
        before_head,
        "--squash must not move HEAD"
    );
    assert!(
        p.join("feat.txt").exists(),
        "merged file should be staged into the worktree"
    );

    // The staged result is finalized with a normal commit, which advances HEAD.
    let commit = run_libra_command(&["commit", "-m", "squashed merge", "--no-verify"], p);
    assert_cli_success(&commit, "commit after squash");
    let final_head = run_libra_command(&["rev-parse", "HEAD"], p);
    assert_ne!(
        String::from_utf8_lossy(&final_head.stdout).trim(),
        before_head,
        "HEAD should advance after committing the squashed result"
    );
}

#[test]
fn test_merge_no_commit_then_continue() {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();

    assert!(
        run_libra_command(&["checkout", "-b", "feat"], p)
            .status
            .success(),
        "checkout -b feat"
    );
    commit_file(p, "feat.txt", "feat content", "feat commit");
    assert!(
        run_libra_command(&["checkout", "main"], p).status.success(),
        "checkout main"
    );
    commit_file(p, "main.txt", "main content", "main commit");

    let before = run_libra_command(&["rev-parse", "HEAD"], p);
    let before_head = String::from_utf8_lossy(&before.stdout).trim().to_string();

    // --no-commit stages the merge but does not move HEAD.
    let merge = run_libra_command(&["merge", "--no-commit", "feat"], p);
    assert_cli_success(&merge, "merge --no-commit feat");
    assert!(
        String::from_utf8_lossy(&merge.stdout).contains("stopped before committing"),
        "expected the no-commit message, got: {}",
        String::from_utf8_lossy(&merge.stdout)
    );
    let mid = run_libra_command(&["rev-parse", "HEAD"], p);
    assert_eq!(
        String::from_utf8_lossy(&mid.stdout).trim(),
        before_head,
        "--no-commit must not move HEAD"
    );
    assert!(
        p.join("feat.txt").exists(),
        "merged file should be staged into the worktree"
    );

    // merge --continue finalizes the two-parent commit and advances HEAD.
    let cont = run_libra_command(&["merge", "--continue"], p);
    assert_cli_success(&cont, "merge --continue");
    let after = run_libra_command(&["rev-parse", "HEAD"], p);
    assert_ne!(
        String::from_utf8_lossy(&after.stdout).trim(),
        before_head,
        "HEAD should advance after merge --continue"
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
/// rollout per `docs/development/commands/_general.md` item B.
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

#[test]
fn test_merge_no_edit_accepts_default_message() {
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

    // `--no-edit` accepts the auto-generated merge message without an editor
    // (Libra never opens one, so this behaves like a plain three-way merge).
    let output = run_libra_command(&["merge", "feature", "--no-edit"], temp_path);
    assert_cli_success(&output, "merge feature --no-edit");
    let log = run_libra_command(&["log", "--oneline", "-n", "1"], temp_path);
    assert!(
        String::from_utf8_lossy(&log.stdout).contains("Merge feature into main"),
        "merge commit landed with the default message: {:?}",
        String::from_utf8_lossy(&log.stdout)
    );
}

#[test]
fn test_merge_no_stat_short_n_and_long_are_accepted() {
    // `-n`/`--no-stat` suppress Git's post-merge diffstat. Libra's merge never
    // prints a diffstat, so both are accepted no-ops that produce a normal merge.
    for flag in ["-n", "--no-stat"] {
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

        let output = run_libra_command(&["merge", "feature", flag], temp_path);
        assert_cli_success(&output, &format!("merge feature {flag}"));
        // No diffstat is printed (Libra never shows one); the merge still happens.
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            !stdout.contains(" | ")
                && !stdout.contains("file changed")
                && !stdout.contains("files changed"),
            "merge {flag} prints no diffstat: {stdout}"
        );
        let log = run_libra_command(&["log", "--oneline", "-n", "1"], temp_path);
        assert!(
            String::from_utf8_lossy(&log.stdout)
                .to_lowercase()
                .contains("merge"),
            "merge {flag} created a merge commit"
        );
    }
}

#[test]
fn test_merge_no_progress_is_accepted_noop() {
    // `--no-progress` suppresses a progress meter. Libra's merge never renders
    // one, so it is an accepted no-op that produces a normal merge.
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

    let output = run_libra_command(&["merge", "feature", "--no-progress"], temp_path);
    assert_cli_success(&output, "merge feature --no-progress");
    let log = run_libra_command(&["log", "--oneline", "-n", "1"], temp_path);
    assert!(
        String::from_utf8_lossy(&log.stdout)
            .to_lowercase()
            .contains("merge"),
        "merge --no-progress created a merge commit"
    );
}

#[test]
fn test_merge_no_verify_signatures_is_accepted_noop() {
    // `--no-verify-signatures` skips GPG signature verification of the merged
    // commits. Libra's merge never verifies signatures, so it is an accepted
    // no-op that produces a normal merge.
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

    let output = run_libra_command(&["merge", "feature", "--no-verify-signatures"], temp_path);
    assert_cli_success(&output, "merge feature --no-verify-signatures");
    let log = run_libra_command(&["log", "--oneline", "-n", "1"], temp_path);
    assert!(
        String::from_utf8_lossy(&log.stdout)
            .to_lowercase()
            .contains("merge"),
        "merge --no-verify-signatures created a merge commit"
    );
}

#[test]
fn test_merge_no_rerere_autoupdate_is_accepted_noop() {
    // `--no-rerere-autoupdate` skips updating the rerere index. Libra has no
    // rerere, so it is an accepted no-op that produces a normal merge.
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

    let output = run_libra_command(&["merge", "feature", "--no-rerere-autoupdate"], temp_path);
    assert_cli_success(&output, "merge feature --no-rerere-autoupdate");
    let log = run_libra_command(&["log", "--oneline", "-n", "1"], temp_path);
    assert!(
        String::from_utf8_lossy(&log.stdout)
            .to_lowercase()
            .contains("merge"),
        "merge --no-rerere-autoupdate created a merge commit"
    );
}

#[test]
fn test_merge_stat_prints_diffstat_for_three_way() {
    // `--stat` prints a diffstat of what the merge brought in. Three-way setup:
    // feature.txt on `feature`, main.txt on `main`, so merging `feature` adds
    // feature.txt relative to the pre-merge main tip.
    let temp_repo = create_committed_repo_via_cli();
    let p = temp_repo.path();
    assert_cli_success(
        &run_libra_command(&["branch", "feature"], p),
        "branch feature",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "feature"], p),
        "checkout feature",
    );
    commit_file(p, "feature.txt", "feature line\n", "feature change");
    assert_cli_success(
        &run_libra_command(&["checkout", "main"], p),
        "checkout main",
    );
    commit_file(p, "main.txt", "main line\n", "main change");

    let out = run_libra_command(&["merge", "--stat", "feature"], p);
    assert_cli_success(&out, "merge --stat feature");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("feature.txt"),
        "diffstat must name the merged-in file: {stdout}"
    );
    assert!(
        stdout.contains(" | "),
        "diffstat must have a per-file bar line: {stdout}"
    );
    assert!(
        stdout.contains("file changed") || stdout.contains("files changed"),
        "diffstat must have a summary line: {stdout}"
    );
}

#[test]
fn test_merge_stat_prints_diffstat_for_fast_forward() {
    // Fast-forward: `main` is strictly behind `feature`, so merging fast-forwards
    // and `--stat` reports the files feature added.
    let temp_repo = create_committed_repo_via_cli();
    let p = temp_repo.path();
    assert_cli_success(
        &run_libra_command(&["branch", "feature"], p),
        "branch feature",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "feature"], p),
        "checkout feature",
    );
    commit_file(p, "ff.txt", "ff line\n", "ff change");
    assert_cli_success(
        &run_libra_command(&["checkout", "main"], p),
        "checkout main",
    );

    let out = run_libra_command(&["merge", "--stat", "feature"], p);
    assert_cli_success(&out, "merge --stat feature (ff)");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Fast-forward"),
        "expected a fast-forward: {stdout}"
    );
    assert!(
        stdout.contains("ff.txt") && stdout.contains(" | "),
        "fast-forward --stat must print the diffstat: {stdout}"
    );
}

#[test]
fn test_merge_stat_no_stat_toggle_last_wins() {
    // `--stat`/`--no-stat` is a last-one-wins toggle.
    let make = || -> tempfile::TempDir {
        let repo = create_committed_repo_via_cli();
        let p = repo.path();
        assert_cli_success(
            &run_libra_command(&["branch", "feature"], p),
            "branch feature",
        );
        assert_cli_success(
            &run_libra_command(&["checkout", "feature"], p),
            "checkout feature",
        );
        commit_file(p, "feature.txt", "feature line\n", "feature change");
        assert_cli_success(
            &run_libra_command(&["checkout", "main"], p),
            "checkout main",
        );
        commit_file(p, "main.txt", "main line\n", "main change");
        repo
    };

    // `--no-stat --stat` → stat wins → diffstat printed.
    let repo = make();
    let out = run_libra_command(&["merge", "--no-stat", "--stat", "feature"], repo.path());
    assert_cli_success(&out, "merge --no-stat --stat");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("file changed")
            || String::from_utf8_lossy(&out.stdout).contains("files changed"),
        "last --stat wins → diffstat printed"
    );

    // `--stat --no-stat` → no-stat wins → no diffstat.
    let repo = make();
    let out = run_libra_command(&["merge", "--stat", "--no-stat", "feature"], repo.path());
    assert_cli_success(&out, "merge --stat --no-stat");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains(" | ") && !stdout.contains("file changed"),
        "last --no-stat wins → no diffstat: {stdout}"
    );
}

#[test]
fn test_merge_stat_suppressed_in_json_machine_and_quiet_modes() {
    // `--stat` must never corrupt structured (`--json`/`--machine`) output or
    // break `--quiet` silence: the diffstat is human-only.
    let setup = || -> tempfile::TempDir {
        let repo = create_committed_repo_via_cli();
        let p = repo.path();
        assert_cli_success(
            &run_libra_command(&["branch", "feature"], p),
            "branch feature",
        );
        assert_cli_success(
            &run_libra_command(&["checkout", "feature"], p),
            "checkout feature",
        );
        commit_file(p, "feature.txt", "feature line\n", "feature change");
        assert_cli_success(
            &run_libra_command(&["checkout", "main"], p),
            "checkout main",
        );
        commit_file(p, "main.txt", "main line\n", "main change");
        repo
    };
    let no_stat_text =
        |s: &str| !s.contains(" | ") && !s.contains("file changed") && !s.contains("files changed");

    // `--json --stat`: stdout is a single parseable JSON envelope, no diffstat text.
    let repo = setup();
    let out = run_libra_command(&["--json", "merge", "--stat", "feature"], repo.path());
    assert_cli_success(&out, "--json merge --stat");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let json: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("--json stdout must be a single JSON record");
    assert_eq!(json["command"], "merge");
    assert!(
        no_stat_text(&stdout),
        "no diffstat text in JSON stdout: {stdout}"
    );

    // `--machine --stat`: NDJSON stays clean (machine implies json + quiet).
    let repo = setup();
    let out = run_libra_command(&["--machine", "merge", "--stat", "feature"], repo.path());
    assert_cli_success(&out, "--machine merge --stat");
    assert!(
        no_stat_text(&String::from_utf8_lossy(&out.stdout)),
        "no diffstat text in machine stdout"
    );

    // `--quiet --stat`: stdout stays empty.
    let repo = setup();
    let out = run_libra_command(&["--quiet", "merge", "--stat", "feature"], repo.path());
    assert_cli_success(&out, "--quiet merge --stat");
    assert!(
        String::from_utf8_lossy(&out.stdout).trim().is_empty(),
        "quiet must suppress the diffstat"
    );
}

#[test]
fn test_merge_verify_signatures_accepts_signed_rejects_unsigned() {
    // `merge --verify-signatures` validates the merged tip's PGP signature
    // against the local vault key, aborting if it is unsigned (or invalid).
    let repo = create_committed_repo_via_cli();
    let p = repo.path();

    // dev: a branch whose tip is a SIGNED commit (vault PGP signing on; `libra
    // init` already provisioned the vault key, so enabling the config is enough).
    assert_cli_success(
        &run_libra_command(&["config", "vault.signing", "true"], p),
        "enable vault signing",
    );
    assert_cli_success(&run_libra_command(&["branch", "dev"], p), "branch dev");
    assert_cli_success(&run_libra_command(&["checkout", "dev"], p), "checkout dev");
    std::fs::write(p.join("dev.txt"), "dev\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "dev.txt"], p), "add dev.txt");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "dev-signed", "--no-verify"], p),
        "signed dev commit",
    );

    // dev2: a branch (from the original base) whose tip is UNSIGNED.
    assert_cli_success(
        &run_libra_command(&["config", "vault.signing", "false"], p),
        "disable vault signing",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "main"], p),
        "checkout main",
    );
    assert_cli_success(&run_libra_command(&["branch", "dev2"], p), "branch dev2");
    assert_cli_success(
        &run_libra_command(&["checkout", "dev2"], p),
        "checkout dev2",
    );
    std::fs::write(p.join("dev2.txt"), "dev2\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "dev2.txt"], p), "add dev2.txt");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "dev2-unsigned", "--no-verify"], p),
        "unsigned dev2 commit",
    );

    // Signed tip → merge --verify-signatures succeeds (proves the signed-content
    // reconstruction round-trips against the vault key).
    assert_cli_success(
        &run_libra_command(&["checkout", "main"], p),
        "checkout main again",
    );
    assert_cli_success(
        &run_libra_command(&["merge", "--verify-signatures", "dev"], p),
        "merge of a signed tip",
    );

    // Unsigned tip → aborts before merging.
    let bad = run_libra_command(&["merge", "--verify-signatures", "dev2"], p);
    assert!(
        !bad.status.success(),
        "merge of an unsigned tip must abort: {}",
        String::from_utf8_lossy(&bad.stdout)
    );
    assert!(
        String::from_utf8_lossy(&bad.stderr).contains("does not have a GPG signature"),
        "unsigned-merge error should name the missing signature: {}",
        String::from_utf8_lossy(&bad.stderr)
    );

    // Without verification, the unsigned tip merges fine.
    assert_cli_success(
        &run_libra_command(&["merge", "--no-verify-signatures", "dev2"], p),
        "unsigned tip merges without verification",
    );

    // A signed commit whose message starts with whitespace (preserved via
    // --cleanup=verbatim) must still verify: the signed-content reconstruction
    // takes the message verbatim, not trimmed.
    assert_cli_success(
        &run_libra_command(&["config", "vault.signing", "true"], p),
        "re-enable vault signing",
    );
    assert_cli_success(&run_libra_command(&["branch", "dev3"], p), "branch dev3");
    assert_cli_success(
        &run_libra_command(&["checkout", "dev3"], p),
        "checkout dev3",
    );
    std::fs::write(p.join("dev3.txt"), "dev3\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "dev3.txt"], p), "add dev3.txt");
    assert_cli_success(
        &run_libra_command(
            &[
                "commit",
                "--cleanup=verbatim",
                "-m",
                "  leading-space subject",
                "--no-verify",
            ],
            p,
        ),
        "signed commit with leading-whitespace message",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "main"], p),
        "checkout main for dev3",
    );
    assert_cli_success(
        &run_libra_command(&["merge", "--verify-signatures", "dev3"], p),
        "signed leading-whitespace-message tip verifies (message taken verbatim)",
    );

    // A signed message whose body itself contains the signature END-marker text
    // must still verify: the body is located by the signature block's offset, not
    // by searching for the marker (which would mis-select the body copy).
    assert_cli_success(&run_libra_command(&["branch", "dev4"], p), "branch dev4");
    assert_cli_success(
        &run_libra_command(&["checkout", "dev4"], p),
        "checkout dev4",
    );
    std::fs::write(p.join("dev4.txt"), "dev4\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "dev4.txt"], p), "add dev4.txt");
    assert_cli_success(
        &run_libra_command(
            &[
                "commit",
                "--cleanup=verbatim",
                "-m",
                "body mentions -----END PGP SIGNATURE----- inline",
                "--no-verify",
            ],
            p,
        ),
        "signed commit whose body contains the END marker text",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "main"], p),
        "checkout main for dev4",
    );
    assert_cli_success(
        &run_libra_command(&["merge", "--verify-signatures", "dev4"], p),
        "signed tip whose message contains the END marker still verifies",
    );
}
