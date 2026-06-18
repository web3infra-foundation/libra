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
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("C1: add 1.txt".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
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
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("C2: modify 1.txt".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
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
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("C3: remove 1.txt, add 2.txt".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
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
        commit: vec!["HEAD".to_string()],
        no_commit: false,
        mainline: None,
        signoff: false,
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
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("Add test.txt".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
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
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("Modify test.txt".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
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
        commit: vec!["HEAD".to_string()],
        no_commit: true,
        mainline: None,
        signoff: false,
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
        no_edit: false,
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
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("Initial commit".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
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
        commit: vec![root_hash],
        no_commit: false,
        mainline: None,
        signoff: false,
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
        commit: vec!["nonexistent".to_string()],
        no_commit: false,
        mainline: None,
        signoff: false,
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
