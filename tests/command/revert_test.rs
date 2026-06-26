//! Tests revert command for reversing commits with and without auto-commit.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use std::{fs, path::PathBuf};

use libra::command::revert;
use serial_test::serial;
use tempfile::tempdir;

use super::*;

#[test]
#[serial]
fn test_revert_cli_outside_repository_returns_fatal_128() {
    let temp = tempdir().unwrap();
    let output = run_libra_command(&["revert", "HEAD"], temp.path());
    assert_eq!(output.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("fatal: not a libra repository"),
        "unexpected stderr: {stderr}"
    );
}

/// Test basic revert functionality with file additions, modifications, and deletions
/// This test follows the workflow:
/// 1. C1: Add 1.txt with content1
/// 2. C2: Modify 1.txt (append content2)
/// 3. C3: Remove 1.txt, Add 2.txt
/// 4. Revert HEAD (C3) - should restore 1.txt and remove 2.txt
/// 5. Find C2 and revert it - should restore 1.txt to original content
#[tokio::test]
#[serial]
async fn test_basic_revert() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    println!("===== SCENARIO 1: BASIC REVERT TEST =====");

    // --- 1. C1: Add 1.txt ---
    fs::write("1.txt", "content1").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["1.txt".to_string()],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,

        pathspec_from_file: None,
        pathspec_file_nul: false,
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("C1: add 1.txt".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
        no_verify: false,
        author: None,
        ..Default::default()
    })
    .await;
    println!("C1: Added 1.txt");

    // --- 2. C2: Modify 1.txt ---
    fs::write("1.txt", "content1\ncontent2").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["1.txt".to_string()],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,

        pathspec_from_file: None,
        pathspec_file_nul: false,
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("C2: modify 1.txt".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
        no_verify: false,
        author: None,
        ..Default::default()
    })
    .await;
    println!("C2: Modified 1.txt");

    // --- 3. C3: Remove 1.txt, Add 2.txt ---
    fs::remove_file("1.txt").unwrap();
    fs::write("2.txt", "content3").unwrap();
    add::execute(AddArgs {
        pathspec: vec![],
        all: true,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,

        pathspec_from_file: None,
        pathspec_file_nul: false,
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("C3: remove 1.txt, add 2.txt".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
        no_verify: false,
        author: None,
        ..Default::default()
    })
    .await;
    println!("C3: Removed 1.txt, Added 2.txt");

    // --- 4. Show initial state ---
    println!("\nBasic test repo is ready. Files before revert:");
    let files: Vec<_> = fs::read_dir(".")
        .unwrap()
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with('.') && name.ends_with(".txt") {
                Some(name)
            } else {
                None
            }
        })
        .collect();
    for file in &files {
        println!("{file}");
    }

    // --- 5. Test 1: Revert HEAD (C3) ---
    println!("\n--- Test 1: Revert HEAD (C3) ---");
    revert::execute(revert::RevertArgs {
        no_rerere_autoupdate: false,
        commit: vec!["HEAD".to_string()],
        no_commit: false,
        mainline: None,
        signoff: false,
        continue_revert: false,
        abort: false,
        edit: false,
        no_edit: false,
    })
    .await;

    // Verify state after reverting C3
    println!("Files after reverting HEAD:");
    let files_after_revert: Vec<_> = fs::read_dir(".")
        .unwrap()
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with('.') && name.ends_with(".txt") {
                Some(name)
            } else {
                None
            }
        })
        .collect();
    for file in &files_after_revert {
        println!("{file}");
    }

    // Should have 1.txt back (modified version) and 2.txt should be gone
    assert!(
        PathBuf::from("1.txt").exists(),
        "1.txt should exist after reverting C3"
    );
    assert!(
        !PathBuf::from("2.txt").exists(),
        "2.txt should not exist after reverting C3"
    );

    // Check content of 1.txt should be the modified version
    let content = fs::read_to_string("1.txt").unwrap();
    assert_eq!(
        content, "content1\ncontent2",
        "1.txt should have modified content"
    );

    println!("Test 1 passed: HEAD revert successful");

    println!("\nAll basic revert tests passed!");
}

/// Test revert with no-commit flag
/// This test verifies that the --no-commit flag stages changes without creating a commit
#[tokio::test]
#[serial]
async fn test_revert_no_commit() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    // Create initial commits
    fs::write("test.txt", "original").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["test.txt".to_string()],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,

        pathspec_from_file: None,
        pathspec_file_nul: false,
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("Add test.txt".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
        no_verify: false,
        author: None,
        ..Default::default()
    })
    .await;

    fs::write("test.txt", "modified").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["test.txt".to_string()],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,

        pathspec_from_file: None,
        pathspec_file_nul: false,
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("Modify test.txt".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
        no_verify: false,
        author: None,
        ..Default::default()
    })
    .await;

    // Test revert with no-commit flag
    revert::execute(revert::RevertArgs {
        no_rerere_autoupdate: false,
        commit: vec!["HEAD".to_string()],
        no_commit: true,
        mainline: None,
        signoff: false,
        continue_revert: false,
        abort: false,
        edit: false,
        no_edit: false,
    })
    .await;

    // File should be reverted but not committed
    let content = fs::read_to_string("test.txt").unwrap();
    assert_eq!(
        content, "original",
        "File should be reverted to original content"
    );

    // Check that we can still commit the staged changes
    commit::execute(CommitArgs {
        message: Some("Manual revert commit".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
        no_verify: false,
        author: None,
        ..Default::default()
    })
    .await;

    println!("No-commit revert test passed");
}

/// Test reverting root commit
/// Root commits have no parents, so reverting them should create an empty repository state
#[tokio::test]
#[serial]
async fn test_revert_root_commit() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    // Create initial commit
    fs::write("initial.txt", "initial content").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["initial.txt".to_string()],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,

        pathspec_from_file: None,
        pathspec_file_nul: false,
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("Initial commit".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
        no_verify: false,
        author: None,
        ..Default::default()
    })
    .await;

    // Get the root commit hash - we need to implement this differently
    // since we can't call external libra command in tests
    let head = Head::current_commit()
        .await
        .expect("Should have current commit");
    let root_hash = head.to_string();

    // Revert root commit
    revert::execute(revert::RevertArgs {
        no_rerere_autoupdate: false,
        commit: vec![root_hash],
        no_commit: false,
        mainline: None,
        signoff: false,
        continue_revert: false,
        abort: false,
        edit: false,
        no_edit: false,
    })
    .await;

    // All files should be removed
    let files: Vec<_> = fs::read_dir(".")
        .unwrap()
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with('.') {
                Some(name)
            } else {
                None
            }
        })
        .collect();

    assert!(
        files.is_empty(),
        "No files should exist after reverting root commit"
    );
    println!("Root commit revert test passed");
}

#[test]
#[serial]
fn test_revert_json_output_reports_files_changed() {
    let repo = create_committed_repo_via_cli();
    let tracked_path = repo.path().join("tracked.txt");

    fs::write(&tracked_path, "updated\n").unwrap();
    let output = run_libra_command(&["add", "tracked.txt"], repo.path());
    assert_cli_success(&output, "failed to stage modified tracked.txt");
    let output = run_libra_command(
        &["commit", "-m", "update tracked", "--no-verify"],
        repo.path(),
    );
    assert_cli_success(&output, "failed to commit modified tracked.txt");

    let output = run_libra_command(&["revert", "--json", "HEAD"], repo.path());
    assert_cli_success(&output, "revert --json should succeed");

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "revert");
    assert_eq!(json["data"]["no_commit"], false);
    assert_eq!(json["data"]["files_changed"], 1);
    assert!(json["data"]["reverted_commit"].as_str().is_some());
    assert!(json["data"]["new_commit"].as_str().is_some());
    assert_eq!(
        fs::read_to_string(&tracked_path).unwrap(),
        "tracked\n",
        "revert should restore the previous file content"
    );
}

#[test]
#[serial]
fn test_revert_no_rerere_autoupdate_is_accepted_noop() {
    let repo = create_committed_repo_via_cli();
    let tracked_path = repo.path().join("tracked.txt");
    fs::write(&tracked_path, "updated\n").unwrap();
    assert_cli_success(
        &run_libra_command(&["add", "tracked.txt"], repo.path()),
        "stage modified tracked.txt",
    );
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "update", "--no-verify"], repo.path()),
        "commit modified tracked.txt",
    );

    // `--no-rerere-autoupdate` is accepted and a no-op: Libra has no rerere, so
    // the revert proceeds and creates a revert commit normally.
    let out = run_libra_command(&["revert", "--no-rerere-autoupdate", "HEAD"], repo.path());
    assert_cli_success(&out, "revert --no-rerere-autoupdate HEAD");
}

#[test]
#[serial]
fn test_revert_signoff_adds_trailer() {
    let repo = create_committed_repo_via_cli();
    let tracked_path = repo.path().join("tracked.txt");

    fs::write(&tracked_path, "updated\n").unwrap();
    assert_cli_success(
        &run_libra_command(&["add", "tracked.txt"], repo.path()),
        "stage modified tracked.txt",
    );
    assert_cli_success(
        &run_libra_command(
            &["commit", "-m", "update tracked", "--no-verify"],
            repo.path(),
        ),
        "commit modified tracked.txt",
    );

    let out = run_libra_command(&["revert", "-s", "HEAD"], repo.path());
    assert_cli_success(&out, "revert -s HEAD");

    // The revert commit message should carry the Signed-off-by trailer.
    let show = run_libra_command(&["cat-file", "-p", "HEAD"], repo.path());
    assert_cli_success(&show, "cat-file -p HEAD");
    let body = String::from_utf8_lossy(&show.stdout);
    assert!(
        body.contains("Signed-off-by:"),
        "revert -s should append a Signed-off-by trailer: {body}"
    );
    assert!(
        body.contains("This reverts commit"),
        "revert message body should be present: {body}"
    );
}

#[test]
#[serial]
fn test_revert_multiple_commits_in_one_invocation() {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();

    fs::write(p.join("a.txt"), "a\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "a.txt"], p), "add a");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "c1 add a", "--no-verify"], p),
        "commit c1",
    );
    let c1 = String::from_utf8_lossy(&run_libra_command(&["rev-parse", "HEAD"], p).stdout)
        .trim()
        .to_string();

    fs::write(p.join("b.txt"), "b\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "b.txt"], p), "add b");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "c2 add b", "--no-verify"], p),
        "commit c2",
    );
    let c2 = String::from_utf8_lossy(&run_libra_command(&["rev-parse", "HEAD"], p).stdout)
        .trim()
        .to_string();

    // Revert both commits in one invocation (newest first).
    let out = run_libra_command(&["revert", c2.as_str(), c1.as_str()], p);
    assert_cli_success(&out, "revert c2 c1");
    assert!(
        !p.join("b.txt").exists(),
        "reverting c2 should remove b.txt"
    );
    assert!(
        !p.join("a.txt").exists(),
        "reverting c1 should remove a.txt"
    );
}

#[test]
#[serial]
fn test_revert_multiple_commits_rejects_no_commit() {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();
    // --no-commit with multiple commits needs the sequencer; it is rejected.
    let out = run_libra_command(&["revert", "--no-commit", "HEAD", "HEAD~1"], p);
    assert!(
        !out.status.success(),
        "revert --no-commit with multiple commits should be rejected"
    );
}

/// Build a repo where reverting `c2` conflicts with a later change in `c3`,
/// returning (repo, c2_hash).
fn setup_revert_conflict() -> (tempfile::TempDir, String) {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();
    fs::write(p.join("f.txt"), "line1\nline2\nline3\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "f.txt"], p), "add c1");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "c1", "--no-verify"], p),
        "commit c1",
    );
    fs::write(p.join("f.txt"), "line1\nCHANGED\nline3\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "f.txt"], p), "add c2");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "c2", "--no-verify"], p),
        "commit c2",
    );
    let c2 = String::from_utf8_lossy(&run_libra_command(&["rev-parse", "HEAD"], p).stdout)
        .trim()
        .to_string();
    fs::write(p.join("f.txt"), "line1\nDIVERGED\nline3\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "f.txt"], p), "add c3");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "c3", "--no-verify"], p),
        "commit c3",
    );
    (repo, c2)
}

#[test]
#[serial]
fn test_revert_conflict_then_continue() {
    let (repo, c2) = setup_revert_conflict();
    let p = repo.path();

    // Reverting c2 conflicts with c3's overlapping change.
    let out = run_libra_command(&["revert", c2.as_str()], p);
    assert!(
        !out.status.success(),
        "conflicting revert should fail and pause"
    );
    assert!(
        p.join(".libra/revert-state.json").exists(),
        "revert state should be recorded"
    );
    assert!(
        fs::read_to_string(p.join("f.txt"))
            .unwrap()
            .contains("<<<<<<<"),
        "worktree should carry conflict markers"
    );

    // Resolve and continue.
    fs::write(p.join("f.txt"), "line1\nRESOLVED\nline3\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "f.txt"], p), "add resolved");
    let cont = run_libra_command(&["revert", "--continue"], p);
    assert_cli_success(&cont, "revert --continue");
    assert!(
        !p.join(".libra/revert-state.json").exists(),
        "state should be cleared after --continue"
    );
    assert_eq!(
        fs::read_to_string(p.join("f.txt")).unwrap(),
        "line1\nRESOLVED\nline3\n"
    );
}

#[test]
#[serial]
fn test_revert_conflict_then_abort() {
    let (repo, c2) = setup_revert_conflict();
    let p = repo.path();

    let out = run_libra_command(&["revert", c2.as_str()], p);
    assert!(!out.status.success(), "conflicting revert should pause");
    assert!(p.join(".libra/revert-state.json").exists());

    let ab = run_libra_command(&["revert", "--abort"], p);
    assert_cli_success(&ab, "revert --abort");
    assert!(
        !p.join(".libra/revert-state.json").exists(),
        "state should be cleared after --abort"
    );
    assert_eq!(
        fs::read_to_string(p.join("f.txt")).unwrap(),
        "line1\nDIVERGED\nline3\n",
        "--abort should restore the pre-revert content"
    );
}

#[tokio::test]
#[serial]
async fn test_revert_json_output_skips_noop_paths_in_files_changed() {
    let repo = create_committed_repo_via_cli();
    let _guard = ChangeDirGuard::new(repo.path());

    fs::write("added.txt", "temporary\n").unwrap();
    assert_cli_success(
        &run_libra_command(&["add", "added.txt"], repo.path()),
        "failed to stage added.txt",
    );
    assert_cli_success(
        &run_libra_command(
            &["commit", "-m", "add temporary", "--no-verify"],
            repo.path(),
        ),
        "failed to commit added.txt",
    );
    let added_commit = Head::current_commit()
        .await
        .expect("expected added.txt commit");

    fs::remove_file("added.txt").unwrap();
    assert_cli_success(
        &run_libra_command(&["add", "-A"], repo.path()),
        "failed to stage added.txt removal",
    );
    assert_cli_success(
        &run_libra_command(
            &["commit", "-m", "remove temporary", "--no-verify"],
            repo.path(),
        ),
        "failed to commit added.txt removal",
    );

    let output = run_libra_command(
        &["revert", "--json", &added_commit.to_string()],
        repo.path(),
    );
    assert_cli_success(
        &output,
        "revert of already-removed add commit should succeed",
    );

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "revert");
    assert_eq!(json["data"]["files_changed"], 0);
    assert!(json["data"]["new_commit"].as_str().is_some());
    assert!(
        !repo.path().join("added.txt").exists(),
        "reverting an already-undone add should keep the file absent"
    );
}

/// Test error cases for revert command
/// This ensures the command handles invalid input gracefully
#[tokio::test]
#[serial]
async fn test_revert_errors() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    // Test reverting non-existent commit should fail gracefully
    revert::execute(revert::RevertArgs {
        no_rerere_autoupdate: false,
        commit: vec!["nonexistent".to_string()],
        no_commit: false,
        mainline: None,
        signoff: false,
        continue_revert: false,
        abort: false,
        edit: false,
        no_edit: false,
    })
    .await;

    println!("Error handling test completed");
}

// ---------------------------------------------------------------------------
// Merge-commit revert via -m/--mainline.
// ---------------------------------------------------------------------------

fn commit_file_revert(repo: &std::path::Path, file: &str, content: &str, msg: &str) {
    fs::write(repo.join(file), content).expect("write file");
    assert_cli_success(&run_libra_command(&["add", file], repo), "add file");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", msg, "--no-verify"], repo),
        "commit file",
    );
}

/// HEAD on main is a 2-parent merge of `feature` (added feature.txt) into main
/// (added mainfile.txt): parent 1 = main pre-merge, parent 2 = feature.
fn build_revert_merge_repo() -> tempfile::TempDir {
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
    commit_file_revert(p, "feature.txt", "feature\n", "feature");
    assert_cli_success(
        &run_libra_command(&["checkout", "main"], p),
        "checkout main",
    );
    commit_file_revert(p, "mainfile.txt", "main\n", "main change");
    assert_cli_success(
        &run_libra_command(&["merge", "feature"], p),
        "merge feature",
    );
    repo
}

#[test]
#[serial]
fn test_revert_merge_without_mainline_errors_128() {
    let repo = build_revert_merge_repo();
    let out = run_libra_command(&["revert", "HEAD"], repo.path());
    assert_eq!(out.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("is a merge but no -m option was given"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
#[serial]
fn test_revert_merge_with_mainline_removes_feature_side() {
    let repo = build_revert_merge_repo();
    let p = repo.path();
    assert!(p.join("feature.txt").exists());
    assert!(p.join("mainfile.txt").exists());
    let out = run_libra_command(&["revert", "-m", "1", "HEAD"], p);
    assert_cli_success(&out, "revert -m 1 HEAD");
    // Reverting relative to parent 1 (main pre-merge) undoes feature's addition.
    assert!(
        !p.join("feature.txt").exists(),
        "feature.txt should be reverted away"
    );
    assert!(p.join("mainfile.txt").exists(), "mainfile.txt stays");
}

#[test]
#[serial]
fn test_revert_mainline_on_non_merge_errors_128() {
    let repo = create_committed_repo_via_cli();
    let out = run_libra_command(&["revert", "-m", "1", "HEAD"], repo.path());
    assert_eq!(out.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("is not a merge"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
#[serial]
fn test_revert_mainline_out_of_range_errors_128() {
    let repo = build_revert_merge_repo();
    let out = run_libra_command(&["revert", "-m", "5", "HEAD"], repo.path());
    assert_eq!(out.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("does not have a parent number 5"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn revert_no_edit_is_accepted() {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();
    std::fs::write(p.join("rev.txt"), "base\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "rev.txt"], p), "add base");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "base", "--no-verify"], p),
        "commit base",
    );
    std::fs::write(p.join("rev.txt"), "change\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "rev.txt"], p), "stage change");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "the change", "--no-verify"], p),
        "commit change",
    );

    // `--no-edit` is accepted (Libra never opens an editor for revert) and the
    // revert is applied normally.
    let revert = run_libra_command(&["revert", "HEAD", "--no-edit"], p);
    assert_cli_success(&revert, "revert HEAD --no-edit");
    assert_eq!(
        std::fs::read_to_string(p.join("rev.txt")).unwrap(),
        "base\n",
        "revert restored the file content"
    );
}

/// `revert --edit` opens the configured editor on the generated revert message
/// and commits the edited result; `--edit` and `--no-edit` are mutually
/// exclusive. (Uses `core.editor` so no process-global env is touched.)
#[test]
#[serial]
fn test_revert_edit_opens_editor() {
    let repo = tempdir().expect("repo dir");
    let p = repo.path();
    assert!(run_libra_command(&["init"], p).status.success(), "init");
    run_libra_command(&["config", "set", "user.name", "t"], p);
    run_libra_command(&["config", "set", "user.email", "t@t"], p);
    fs::write(p.join("f.txt"), "one\n").expect("write f");
    assert!(
        run_libra_command(&["add", "f.txt"], p).status.success(),
        "add"
    );
    assert!(
        run_libra_command(&["commit", "-m", "first", "--no-verify"], p)
            .status
            .success(),
        "commit first"
    );
    fs::write(p.join("f.txt"), "two\n").expect("modify f");
    assert!(
        run_libra_command(&["add", "f.txt"], p).status.success(),
        "add 2"
    );
    assert!(
        run_libra_command(&["commit", "-m", "second", "--no-verify"], p)
            .status
            .success(),
        "commit second"
    );

    // An editor script that replaces the revert message with a fixed line.
    let editor = p.join("fake-editor.sh");
    fs::write(
        &editor,
        "#!/bin/sh\necho 'EDITED revert subject' > \"$1\"\n",
    )
    .expect("write editor");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&editor, fs::Permissions::from_mode(0o755)).expect("chmod editor");
    }
    run_libra_command(
        &["config", "set", "core.editor", editor.to_str().unwrap()],
        p,
    );

    let out = run_libra_command(&["revert", "HEAD", "--edit"], p);
    assert!(
        out.status.success(),
        "revert --edit should succeed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let subject = run_libra_command(&["log", "-1", "--pretty=%s"], p);
    assert_eq!(
        String::from_utf8_lossy(&subject.stdout).trim(),
        "EDITED revert subject",
        "the edited message is committed"
    );

    // `--edit` and `--no-edit` are mutually exclusive (clap conflict).
    let conflict = run_libra_command(&["revert", "HEAD", "--edit", "--no-edit"], p);
    assert!(
        !conflict.status.success(),
        "--edit conflicts with --no-edit"
    );
}

/// `revert --edit` is carried through a conflict: after resolving and running
/// `revert --continue`, the editor opens again (via `RevertState.edit`) and the
/// edited message is committed.
#[test]
#[serial]
fn test_revert_edit_carried_through_continue() {
    let repo = tempdir().expect("repo dir");
    let p = repo.path();
    assert!(run_libra_command(&["init"], p).status.success(), "init");
    run_libra_command(&["config", "set", "user.name", "t"], p);
    run_libra_command(&["config", "set", "user.email", "t@t"], p);
    let commit = |msg: &str, body: &str| {
        fs::write(p.join("f.txt"), body).expect("write f");
        assert!(
            run_libra_command(&["add", "f.txt"], p).status.success(),
            "add"
        );
        assert!(
            run_libra_command(&["commit", "-m", msg, "--no-verify"], p)
                .status
                .success(),
            "commit {msg}"
        );
    };
    commit("c1", "a\nb\nc\n");
    commit("c2", "a\nB\nc\n"); // changes line 2
    commit("c3", "a\nZ\nc\n"); // changes line 2 again → reverting c2 will conflict

    let editor = p.join("fake-editor.sh");
    fs::write(&editor, "#!/bin/sh\necho 'EDITED via continue' > \"$1\"\n").expect("write editor");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&editor, fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    run_libra_command(
        &["config", "set", "core.editor", editor.to_str().unwrap()],
        p,
    );

    // Reverting c2 (HEAD~1) conflicts with c3's change to line 2.
    let conflicted = run_libra_command(&["revert", "HEAD~1", "--edit"], p);
    assert!(
        !conflicted.status.success(),
        "reverting HEAD~1 should conflict: {}",
        String::from_utf8_lossy(&conflicted.stdout)
    );

    // Resolve and continue: the editor opens (RevertState carried `--edit`).
    fs::write(p.join("f.txt"), "a\nRESOLVED\nc\n").expect("resolve");
    assert!(
        run_libra_command(&["add", "f.txt"], p).status.success(),
        "add resolved"
    );
    let cont = run_libra_command(&["revert", "--continue"], p);
    assert!(
        cont.status.success(),
        "revert --continue should succeed: {}",
        String::from_utf8_lossy(&cont.stderr)
    );
    let subject = run_libra_command(&["log", "-1", "--pretty=%s"], p);
    assert_eq!(
        String::from_utf8_lossy(&subject.stdout).trim(),
        "EDITED via continue",
        "the edited message is committed after --continue"
    );
}

/// A failing/empty editor on a CLEAN `revert --edit` must leave the working
/// tree and HEAD unchanged (the message is resolved before the worktree is
/// mutated), and must NOT leave a stray in-progress revert.
#[test]
#[serial]
fn test_revert_edit_failure_leaves_worktree_clean() {
    let repo = tempdir().expect("repo dir");
    let p = repo.path();
    assert!(run_libra_command(&["init"], p).status.success(), "init");
    run_libra_command(&["config", "set", "user.name", "t"], p);
    run_libra_command(&["config", "set", "user.email", "t@t"], p);
    fs::write(p.join("f.txt"), "one\n").expect("write f");
    assert!(
        run_libra_command(&["add", "f.txt"], p).status.success(),
        "add"
    );
    assert!(
        run_libra_command(&["commit", "-m", "first", "--no-verify"], p)
            .status
            .success(),
        "commit first"
    );
    fs::write(p.join("f.txt"), "two\n").expect("modify f");
    assert!(
        run_libra_command(&["add", "f.txt"], p).status.success(),
        "add 2"
    );
    assert!(
        run_libra_command(&["commit", "-m", "second", "--no-verify"], p)
            .status
            .success(),
        "commit second"
    );

    // An editor that exits non-zero (failure).
    let editor = p.join("bad-editor.sh");
    fs::write(&editor, "#!/bin/sh\nexit 1\n").expect("write editor");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&editor, fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    run_libra_command(
        &["config", "set", "core.editor", editor.to_str().unwrap()],
        p,
    );

    let out = run_libra_command(&["revert", "HEAD", "--edit"], p);
    assert!(!out.status.success(), "a failing editor aborts the revert");
    // The working tree is untouched (revert was not applied), HEAD is unchanged,
    // and there is no in-progress revert to clean up.
    assert_eq!(
        fs::read_to_string(p.join("f.txt")).unwrap(),
        "two\n",
        "worktree unchanged"
    );
    assert!(
        !p.join(".libra/revert-state.json").exists(),
        "no stray revert state on a clean-path editor failure"
    );
    let subject = run_libra_command(&["log", "-1", "--pretty=%s"], p);
    assert_eq!(
        String::from_utf8_lossy(&subject.stdout).trim(),
        "second",
        "HEAD unchanged (no revert commit)"
    );
}

/// A failing editor during `revert --continue` must leave `revert-state.json`
/// in place so the revert stays recoverable (`--abort`/retry).
#[test]
#[serial]
fn test_revert_edit_failure_during_continue_keeps_state() {
    let repo = tempdir().expect("repo dir");
    let p = repo.path();
    assert!(run_libra_command(&["init"], p).status.success(), "init");
    run_libra_command(&["config", "set", "user.name", "t"], p);
    run_libra_command(&["config", "set", "user.email", "t@t"], p);
    let commit = |msg: &str, body: &str| {
        fs::write(p.join("f.txt"), body).expect("write f");
        assert!(
            run_libra_command(&["add", "f.txt"], p).status.success(),
            "add"
        );
        assert!(
            run_libra_command(&["commit", "-m", msg, "--no-verify"], p)
                .status
                .success(),
            "commit {msg}"
        );
    };
    commit("c1", "a\nb\nc\n");
    commit("c2", "a\nB\nc\n");
    commit("c3", "a\nZ\nc\n");

    let editor = p.join("bad-editor.sh");
    fs::write(&editor, "#!/bin/sh\nexit 1\n").expect("write editor");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&editor, fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    run_libra_command(
        &["config", "set", "core.editor", editor.to_str().unwrap()],
        p,
    );

    // Conflicting revert with --edit (editor not reached yet at conflict time).
    let conflicted = run_libra_command(&["revert", "HEAD~1", "--edit"], p);
    assert!(!conflicted.status.success(), "revert should conflict");
    assert!(
        p.join(".libra/revert-state.json").exists(),
        "conflict records state"
    );

    // Resolve, then --continue: the editor runs and FAILS.
    fs::write(p.join("f.txt"), "a\nRESOLVED\nc\n").expect("resolve");
    assert!(
        run_libra_command(&["add", "f.txt"], p).status.success(),
        "add resolved"
    );
    let cont = run_libra_command(&["revert", "--continue"], p);
    assert!(!cont.status.success(), "a failing editor aborts --continue");
    // State persists so the user can retry or --abort.
    assert!(
        p.join(".libra/revert-state.json").exists(),
        "revert state remains after a failed --continue editor"
    );
}
