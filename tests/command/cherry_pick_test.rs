//! Tests cherry-pick scenarios that apply commits and verify results or conflicts.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use std::{fs, path::PathBuf};

use libra::{
    command::{
        add, cherry_pick, cherry_pick::CherryPickArgs, commit, init, switch, switch::SwitchArgs,
    },
    internal::head::Head,
};
use serial_test::serial;
use tempfile::tempdir;

use super::*;

#[test]
fn test_cherry_pick_cli_outside_repository_returns_fatal_128() {
    let temp = tempdir().unwrap();
    let output = run_libra_command(&["cherry-pick", "abc123"], temp.path());
    assert_eq!(output.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("fatal: not a libra repository"),
        "unexpected stderr: {stderr}"
    );
}

/// Test basic cherry-pick functionality
/// This test follows the workflow:
/// 1. Create a common ancestor commit (C1)
/// 2. Create a feature branch and add commits (C2, C3)
/// 3. Switch back to master branch
/// 4. Cherry-pick feature commits to master
#[tokio::test]
#[serial]
async fn test_basic_cherry_pick() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    println!("===== SCENARIO: BASIC CHERRY-PICK TEST =====");

    // --- 1. Create common ancestor commit (C1) ---
    fs::write("base.txt", "base").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["base.txt".to_string()],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
        ..Default::default()
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("C1: Initial commit, our common ancestor".to_string()),
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
    })
    .await;
    println!("C1: Created common ancestor.");

    // --- 2. Create and switch to feature branch ---
    switch::execute(SwitchArgs {
        branch: None,
        create: Some("feature".to_string()),
        detach: false,
        track: false,
    })
    .await;
    println!("Switched to new branch 'feature'.");

    // --- 3. Create two commits on feature branch ---
    // Commit C2: First target to cherry-pick
    fs::write("feature_a.txt", "feature A").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["feature_a.txt".to_string()],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
        ..Default::default()
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("C2: Add feature_a.txt".to_string()),
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
    })
    .await;
    println!("C2: Added feature_a.txt on feature branch.");

    // Get C2 commit hash for cherry-picking later
    let c2_commit = Head::current_commit()
        .await
        .expect("Should have current commit");

    // Commit C3: Second target to cherry-pick
    fs::write("feature_b.txt", "feature B").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["feature_b.txt".to_string()],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
        ..Default::default()
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("C3: Add feature_b.txt".to_string()),
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
    })
    .await;
    println!("C3: Added feature_b.txt on feature branch.");

    // --- 4. Switch back to master branch ---
    switch::execute(SwitchArgs {
        branch: Some("main".to_string()),
        create: None,
        detach: false,
        track: false,
    })
    .await;
    println!("Switched back to master.");

    // --- 5. Verify initial state on master ---
    println!("\nCherry-pick test repo is ready. Current state:");
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

    // Should only have base.txt on master
    assert!(
        PathBuf::from("base.txt").exists(),
        "base.txt should exist on master"
    );
    assert!(
        !PathBuf::from("feature_a.txt").exists(),
        "feature_a.txt should not exist on master before cherry-pick"
    );
    assert!(
        !PathBuf::from("feature_b.txt").exists(),
        "feature_b.txt should not exist on master before cherry-pick"
    );

    // --- 6. Cherry-pick C2 (feature_a.txt) with --no-commit flag ---
    println!("\n--- Cherry-picking C2 with --no-commit ---");
    cherry_pick::execute(cherry_pick::CherryPickArgs {
        commits: vec![c2_commit.to_string()],
        no_commit: true,
        ..Default::default()
    })
    .await;

    // --- 7. Verify state after cherry-pick --no-commit ---
    println!("Files after cherry-pick --no-commit:");
    let files_after_cherry_pick: Vec<_> = fs::read_dir(".")
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
    for file in &files_after_cherry_pick {
        println!("{file}");
    }

    // Should now have both base.txt and feature_a.txt
    assert!(
        PathBuf::from("base.txt").exists(),
        "base.txt should still exist"
    );
    assert!(
        PathBuf::from("feature_a.txt").exists(),
        "feature_a.txt should exist after cherry-pick"
    );
    assert!(
        !PathBuf::from("feature_b.txt").exists(),
        "feature_b.txt should not exist (not cherry-picked)"
    );

    // Verify content of cherry-picked file
    let feature_a_content = fs::read_to_string("feature_a.txt").unwrap();
    assert_eq!(
        feature_a_content, "feature A",
        "feature_a.txt should have correct content"
    );

    // Check that changes are staged but not committed (no new commit created)
    let _ = Head::current_commit().await.expect("Should have HEAD");

    // The head should still be the same as before cherry-pick since we used --no-commit
    // In a real test, we might want to check the index status here

    println!("Cherry-pick --no-commit test passed");

    println!("\nAll cherry-pick tests completed successfully!");
}

/// Test cherry-pick with automatic commit
#[tokio::test]
#[serial]
async fn test_cherry_pick_with_commit() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    // Create base commit
    fs::write("base.txt", "base content").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["base.txt".to_string()],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
        ..Default::default()
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("Base commit".to_string()),
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
    })
    .await;

    // Create feature branch and commit
    switch::execute(SwitchArgs {
        branch: None,
        create: Some("feature".to_string()),
        detach: false,
        track: false,
    })
    .await;

    fs::write("feature.txt", "feature content").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["feature.txt".to_string()],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
        ..Default::default()
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("Feature commit".to_string()),
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
    })
    .await;

    let feature_commit = Head::current_commit()
        .await
        .expect("Should have current commit");

    // Switch back to master
    switch::execute(SwitchArgs {
        branch: Some("main".to_string()),
        create: None,
        detach: false,
        track: false,
    })
    .await;

    let head_before = Head::current_commit()
        .await
        .expect("Should have HEAD before cherry-pick");

    // Cherry-pick with automatic commit
    cherry_pick::execute(cherry_pick::CherryPickArgs {
        commits: vec![feature_commit.to_string()],
        ..Default::default()
    })
    .await;

    // Verify new commit was created
    let head_after = Head::current_commit()
        .await
        .expect("Should have HEAD after cherry-pick");
    assert_ne!(
        head_before, head_after,
        "A new commit should have been created"
    );

    // Verify file was cherry-picked
    assert!(
        PathBuf::from("feature.txt").exists(),
        "feature.txt should exist after cherry-pick"
    );
    let content = fs::read_to_string("feature.txt").unwrap();
    assert_eq!(
        content, "feature content",
        "feature.txt should have correct content"
    );

    println!("Cherry-pick with commit test passed");
}

/// Test cherry-pick multiple commits
#[tokio::test]
#[serial]
async fn test_cherry_pick_multiple_commits() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    // Create base commit
    fs::write("base.txt", "base").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["base.txt".to_string()],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
        ..Default::default()
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("Base commit".to_string()),
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
    })
    .await;

    // Create feature branch
    switch::execute(SwitchArgs {
        branch: None,
        create: Some("feature".to_string()),
        detach: false,
        track: false,
    })
    .await;

    // Create first feature commit
    fs::write("file1.txt", "content1").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["file1.txt".to_string()],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
        ..Default::default()
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("Feature commit 1".to_string()),
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
    })
    .await;
    let commit1 = Head::current_commit().await.expect("Should have commit1");

    // Create second feature commit
    fs::write("file2.txt", "content2").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["file2.txt".to_string()],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
        ..Default::default()
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("Feature commit 2".to_string()),
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
    })
    .await;
    let commit2 = Head::current_commit().await.expect("Should have commit2");

    // Switch back to master
    switch::execute(SwitchArgs {
        branch: Some("main".to_string()),
        create: None,
        detach: false,
        track: false,
    })
    .await;

    // Cherry-pick both commits
    cherry_pick::execute(cherry_pick::CherryPickArgs {
        commits: vec![commit1.to_string(), commit2.to_string()],
        ..Default::default()
    })
    .await;

    // Verify both files exist
    assert!(
        PathBuf::from("file1.txt").exists(),
        "file1.txt should exist"
    );
    assert!(
        PathBuf::from("file2.txt").exists(),
        "file2.txt should exist"
    );

    let content1 = fs::read_to_string("file1.txt").unwrap();
    let content2 = fs::read_to_string("file2.txt").unwrap();
    assert_eq!(
        content1, "content1",
        "file1.txt should have correct content"
    );
    assert_eq!(
        content2, "content2",
        "file2.txt should have correct content"
    );

    println!("Multiple commits cherry-pick test passed");
}

/// Test error cases for cherry-pick
#[tokio::test]
#[serial]
async fn test_cherry_pick_errors() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    // Test cherry-picking non-existent commit should fail gracefully
    cherry_pick::execute(cherry_pick::CherryPickArgs {
        commits: vec!["nonexistent".to_string()],
        ..Default::default()
    })
    .await;

    println!("Error handling test completed");
}

#[test]
#[serial]
fn test_cherry_pick_invalid_commit_returns_cli_invalid_target() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["cherry-pick", "nonexistent"], repo.path());
    assert_eq!(output.status.code(), Some(129));

    let (human, report) = parse_cli_error_stderr(&output.stderr);
    assert!(
        human.contains("fatal: failed to resolve commit reference 'nonexistent'"),
        "unexpected stderr: {human}"
    );
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert_eq!(report.exit_code, 129);
}

#[tokio::test]
#[serial]
async fn test_cherry_pick_merge_commit_rejection_uses_invalid_arguments_code() {
    let repo = create_committed_repo_via_cli();
    let _guard = ChangeDirGuard::new(repo.path());

    let head = Head::current_commit().await.expect("expected HEAD commit");
    let head_commit: Commit = load_object(&head).expect("failed to load HEAD commit");
    let merge_commit = Commit::from_tree_id(
        head_commit.tree_id,
        vec![head, head],
        &format_commit_msg("synthetic merge commit", None),
    );
    save_object(&merge_commit, &merge_commit.id).expect("failed to save synthetic merge commit");

    let output = run_libra_command(&["cherry-pick", &merge_commit.id.to_string()], repo.path());
    assert_eq!(output.status.code(), Some(129));

    let (human, report) = parse_cli_error_stderr(&output.stderr);
    assert!(
        human.contains("fatal: cherry-picking merge commits is not supported"),
        "unexpected stderr: {human}"
    );
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert_eq!(report.exit_code, 129);
}

#[tokio::test]
#[serial]
async fn test_cherry_pick_json_output() {
    let repo = create_committed_repo_via_cli();
    let _guard = ChangeDirGuard::new(repo.path());

    let output = run_libra_command(&["switch", "-c", "feature"], repo.path());
    assert_cli_success(&output, "switch -c feature should succeed");

    fs::write("feature.txt", "feature content\n").unwrap();
    let output = run_libra_command(&["add", "feature.txt"], repo.path());
    assert_cli_success(&output, "add feature.txt should succeed");

    let output = run_libra_command(
        &["commit", "-m", "Feature commit", "--no-verify"],
        repo.path(),
    );
    assert_cli_success(&output, "feature commit should succeed");

    let feature_commit = Head::current_commit()
        .await
        .expect("expected feature commit");

    let output = run_libra_command(&["switch", "main"], repo.path());
    assert_cli_success(&output, "switch main should succeed");

    let output = run_libra_command(
        &["cherry-pick", "--json", &feature_commit.to_string()],
        repo.path(),
    );
    assert_cli_success(&output, "cherry-pick --json should succeed");

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "cherry-pick");
    assert_eq!(json["data"]["no_commit"], false);
    assert_eq!(json["data"]["picked"].as_array().unwrap().len(), 1);
    assert_eq!(
        json["data"]["picked"][0]["source_commit"],
        feature_commit.to_string()
    );
    assert!(json["data"]["picked"][0]["new_commit"].as_str().is_some());
}

#[tokio::test]
#[serial]
/// Verify cherry-pick behavior under SHA-256: accepts 64-hex commit ids, rejects SHA-1 length.
async fn test_cherry_pick_sha256_hash_handling() {
    let temp_path = tempdir().unwrap();
    test::setup_clean_testing_env_in(temp_path.path());
    let _guard = ChangeDirGuard::new(temp_path.path());

    // init repo with sha256
    init::init(init::InitArgs {
        bare: false,
        initial_branch: Some("main".to_string()),
        template: None,
        repo_directory: temp_path.path().to_str().unwrap().to_string(),
        quiet: true,
        shared: None,
        object_format: Some("sha256".to_string()),
        ref_format: None,
        from_git_repository: None,
        vault: false,
    })
    .await
    .unwrap();
    libra::internal::config::ConfigKv::set("user.name", "Cherry Test User", false)
        .await
        .unwrap();
    libra::internal::config::ConfigKv::set("user.email", "cherry-test@example.com", false)
        .await
        .unwrap();

    // base commit on main
    fs::write("base.txt", "base").unwrap();
    add::execute(add::AddArgs {
        pathspec: vec!["base.txt".into()],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
        ..Default::default()
    })
    .await;
    commit::execute(commit::CommitArgs {
        message: Some("base".into()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: true,
        all: false,
        no_verify: false,
        author: None,
    })
    .await;

    // feature branch with one commit
    switch::execute(SwitchArgs {
        branch: None,
        create: Some("feature".into()),
        detach: false,
        track: false,
    })
    .await;
    fs::write("feature.txt", "feature").unwrap();
    add::execute(add::AddArgs {
        pathspec: vec!["feature.txt".into()],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
        ..Default::default()
    })
    .await;
    commit::execute(commit::CommitArgs {
        message: Some("feature".into()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: true,
        all: false,
        no_verify: false,
        author: None,
    })
    .await;
    let feature_commit = Head::current_commit().await.expect("need feature commit");
    assert_eq!(feature_commit.to_string().len(), 64);

    // back to main
    switch::execute(SwitchArgs {
        branch: Some("main".into()),
        create: None,
        detach: false,
        track: false,
    })
    .await;
    let head_before = Head::current_commit().await.unwrap();

    // attempt cherry-pick with SHA-1 length hash: should no-op and not create file
    cherry_pick::execute(CherryPickArgs {
        commits: vec!["4b825dc642cb6eb9a060e54bf8d69288fbee4904".into()],
        ..Default::default()
    })
    .await;
    let head_after_invalid = Head::current_commit().await.unwrap();
    assert_eq!(
        head_before, head_after_invalid,
        "invalid hash must not advance HEAD"
    );
    assert!(
        !PathBuf::from("feature.txt").exists(),
        "invalid hash must not apply changes"
    );

    // cherry-pick with valid SHA-256 commit should succeed
    cherry_pick::execute(CherryPickArgs {
        commits: vec![feature_commit.to_string()],
        ..Default::default()
    })
    .await;
    let head_after_valid = Head::current_commit().await.unwrap();
    assert_ne!(
        head_before, head_after_valid,
        "valid cherry-pick should create new commit"
    );
    assert!(
        PathBuf::from("feature.txt").exists(),
        "feature.txt should be present after valid cherry-pick"
    );
}

// ── Batch 0: commit-modifier flags (-x / -s / -e / --allow-empty*) ──

/// `libra rev-parse <rev>` → trimmed OID string (panics on failure).
fn cp_rev_parse(repo: &std::path::Path, rev: &str) -> String {
    let out = run_libra_command(&["rev-parse", rev], repo);
    assert_cli_success(&out, "rev-parse");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Raw `cat-file -p HEAD` body (includes the commit message).
fn cp_head_message(repo: &std::path::Path) -> String {
    let out = run_libra_command(&["cat-file", "-p", "HEAD"], repo);
    assert_cli_success(&out, "cat-file -p HEAD");
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// Fresh repo with a `feature` branch holding one commit that adds `file`=`content`
/// (message `msg`). Returns `(repo, feature_oid)` with HEAD back on `main`.
fn repo_with_feature_commit(file: &str, content: &str, msg: &str) -> (tempfile::TempDir, String) {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();
    assert_cli_success(
        &run_libra_command(&["switch", "-c", "feature"], p),
        "switch -c feature",
    );
    std::fs::write(p.join(file), content).unwrap();
    assert_cli_success(&run_libra_command(&["add", file], p), "add feature file");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", msg, "--no-verify"], p),
        "feature commit",
    );
    let oid = cp_rev_parse(p, "HEAD");
    assert_cli_success(&run_libra_command(&["switch", "main"], p), "switch main");
    (repo, oid)
}

/// Default cherry-pick (no `-x`) must NOT append the cherry-picked-from line
/// (behavior reversal — previously always appended).
#[test]
fn cherry_pick_default_omits_cherry_picked_from_line() {
    let (repo, oid) = repo_with_feature_commit("f.txt", "feat\n", "feature work");
    assert_cli_success(
        &run_libra_command(&["cherry-pick", &oid], repo.path()),
        "cherry-pick default",
    );
    let msg = cp_head_message(repo.path());
    assert!(
        !msg.contains("(cherry picked from commit"),
        "default cherry-pick must not append the origin line, got: {msg}"
    );
    assert!(msg.contains("feature work"), "message: {msg}");
}

/// `-x` appends the cherry-picked-from line (and only once).
#[test]
fn cherry_pick_dash_x_appends_origin_line() {
    let (repo, oid) = repo_with_feature_commit("f.txt", "feat\n", "feature work");
    assert_cli_success(
        &run_libra_command(&["cherry-pick", "-x", &oid], repo.path()),
        "cherry-pick -x",
    );
    let msg = cp_head_message(repo.path());
    let needle = format!("(cherry picked from commit {oid})");
    assert_eq!(
        msg.matches(&needle).count(),
        1,
        "origin line must appear exactly once, got: {msg}"
    );
}

/// `-s` appends a Signed-off-by trailer.
#[test]
fn cherry_pick_signoff_appends_trailer() {
    let (repo, oid) = repo_with_feature_commit("f.txt", "feat\n", "feature work");
    assert_cli_success(
        &run_libra_command(&["cherry-pick", "-s", &oid], repo.path()),
        "cherry-pick -s",
    );
    let msg = cp_head_message(repo.path());
    assert!(
        msg.contains("Signed-off-by:"),
        "signoff trailer missing, got: {msg}"
    );
}

/// `-x -s` ordering: the cherry-picked-from line precedes Signed-off-by.
#[test]
fn cherry_pick_x_and_signoff_ordering() {
    let (repo, oid) = repo_with_feature_commit("f.txt", "feat\n", "feature work");
    assert_cli_success(
        &run_libra_command(&["cherry-pick", "-x", "-s", &oid], repo.path()),
        "cherry-pick -x -s",
    );
    let msg = cp_head_message(repo.path());
    let x_pos = msg
        .find("(cherry picked from commit")
        .expect("origin line present");
    let s_pos = msg.find("Signed-off-by:").expect("signoff present");
    assert!(
        x_pos < s_pos,
        "cherry-picked-from must precede Signed-off-by, got: {msg}"
    );
}

/// `-n c1 c2` no longer errors and accumulates both changes into the index.
#[test]
fn cherry_pick_multiple_with_no_commit_accumulates_index() {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();
    assert_cli_success(
        &run_libra_command(&["switch", "-c", "feature"], p),
        "switch -c feature",
    );
    std::fs::write(p.join("a.txt"), "aaa\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "a.txt"], p), "add a");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "add a", "--no-verify"], p),
        "commit a",
    );
    let c1 = cp_rev_parse(p, "HEAD");
    std::fs::write(p.join("b.txt"), "bbb\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "b.txt"], p), "add b");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "add b", "--no-verify"], p),
        "commit b",
    );
    let c2 = cp_rev_parse(p, "HEAD");
    assert_cli_success(&run_libra_command(&["switch", "main"], p), "switch main");
    let head_before = cp_rev_parse(p, "HEAD");

    let out = run_libra_command(&["cherry-pick", "-n", &c1, &c2], p);
    assert_cli_success(&out, "cherry-pick -n c1 c2 must not error");

    // HEAD unchanged (no commits made), both files staged.
    assert_eq!(
        cp_rev_parse(p, "HEAD"),
        head_before,
        "HEAD must not advance"
    );
    let status = run_libra_command(&["status"], p);
    let body = String::from_utf8_lossy(&status.stdout);
    assert!(body.contains("a.txt"), "a.txt staged: {body}");
    assert!(body.contains("b.txt"), "b.txt staged: {body}");
}

/// A commit whose own change set is empty is blocked without `--allow-empty`.
#[test]
fn cherry_pick_originally_empty_blocked_without_allow_empty() {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();
    assert_cli_success(
        &run_libra_command(&["switch", "-c", "feature"], p),
        "switch -c feature",
    );
    assert_cli_success(
        &run_libra_command(
            &["commit", "--allow-empty", "-m", "empty feat", "--no-verify"],
            p,
        ),
        "empty feature commit",
    );
    let empty_oid = cp_rev_parse(p, "HEAD");
    assert_cli_success(&run_libra_command(&["switch", "main"], p), "switch main");

    let out = run_libra_command(&["cherry-pick", &empty_oid], p);
    assert_eq!(out.status.code(), Some(129), "empty commit blocked");
    let (_h, report) = parse_cli_error_stderr(&out.stderr);
    assert_eq!(report.error_code, "LBR-CLI-002");
}

/// `--allow-empty` lets an originally-empty commit through.
#[test]
fn cherry_pick_allow_empty_creates_commit() {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();
    assert_cli_success(
        &run_libra_command(&["switch", "-c", "feature"], p),
        "switch -c feature",
    );
    assert_cli_success(
        &run_libra_command(
            &["commit", "--allow-empty", "-m", "empty feat", "--no-verify"],
            p,
        ),
        "empty feature commit",
    );
    let empty_oid = cp_rev_parse(p, "HEAD");
    assert_cli_success(&run_libra_command(&["switch", "main"], p), "switch main");
    let head_before = cp_rev_parse(p, "HEAD");

    assert_cli_success(
        &run_libra_command(&["cherry-pick", "--allow-empty", &empty_oid], p),
        "cherry-pick --allow-empty",
    );
    assert_ne!(
        cp_rev_parse(p, "HEAD"),
        head_before,
        "an empty commit should still create a new commit under --allow-empty"
    );
}

/// A commit that becomes redundant after replay is blocked by default, kept with
/// `--keep-redundant-commits`.
#[test]
fn cherry_pick_redundant_blocked_then_kept() {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();
    // feature adds dup.txt=same
    assert_cli_success(
        &run_libra_command(&["switch", "-c", "feature"], p),
        "switch -c feature",
    );
    std::fs::write(p.join("dup.txt"), "same\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "dup.txt"], p), "add dup");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "feat dup", "--no-verify"], p),
        "feature commit",
    );
    let feat = cp_rev_parse(p, "HEAD");
    // main independently adds the identical dup.txt=same
    assert_cli_success(&run_libra_command(&["switch", "main"], p), "switch main");
    std::fs::write(p.join("dup.txt"), "same\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "dup.txt"], p), "add dup main");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "main dup", "--no-verify"], p),
        "main commit",
    );
    let head_before = cp_rev_parse(p, "HEAD");

    // default: redundant → blocked, HEAD unchanged.
    let blocked = run_libra_command(&["cherry-pick", &feat], p);
    assert_eq!(blocked.status.code(), Some(129), "redundant blocked");
    let (_h, report) = parse_cli_error_stderr(&blocked.stderr);
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert_eq!(
        cp_rev_parse(p, "HEAD"),
        head_before,
        "HEAD unchanged on block"
    );

    // --keep-redundant-commits: kept.
    assert_cli_success(
        &run_libra_command(&["cherry-pick", "--keep-redundant-commits", &feat], p),
        "cherry-pick --keep-redundant-commits",
    );
    assert_ne!(
        cp_rev_parse(p, "HEAD"),
        head_before,
        "redundant commit kept advances HEAD"
    );
}

/// Unsupported Git options are rejected with LBR-UNSUPPORTED-001 / exit 128.
#[test]
fn cherry_pick_unsupported_flags_rejected() {
    let (repo, oid) = repo_with_feature_commit("f.txt", "feat\n", "feature work");
    let cases: Vec<Vec<&str>> = vec![
        vec!["cherry-pick", "--empty", "drop", &oid],
        vec!["cherry-pick", "--cleanup", "strip", &oid],
        vec!["cherry-pick", "--rerere-autoupdate", &oid],
        vec!["cherry-pick", "--commit", &oid],
    ];
    for args in cases {
        let out = run_libra_command(&args, repo.path());
        assert_eq!(
            out.status.code(),
            Some(128),
            "{args:?} should be unsupported: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let (_h, report) = parse_cli_error_stderr(&out.stderr);
        assert_eq!(report.error_code, "LBR-UNSUPPORTED-001", "args: {args:?}");
    }
}

/// `-e` in machine mode (no TTY) degrades to the assembled message without
/// launching an editor or panicking.
#[test]
fn cherry_pick_edit_no_tty_falls_back() {
    let (repo, oid) = repo_with_feature_commit("f.txt", "feat\n", "feature work");
    let out = run_libra_command(&["cherry-pick", "--machine", "-e", &oid], repo.path());
    assert_eq!(
        out.status.code(),
        Some(0),
        "machine -e should succeed without an editor: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `--machine` emits machine JSON (NDJSON) rather than suppressing stdout.
#[test]
fn cherry_pick_machine_emits_ndjson() {
    let (repo, oid) = repo_with_feature_commit("f.txt", "feat\n", "feature work");
    let out = run_libra_command(&["cherry-pick", "--machine", &oid], repo.path());
    assert_cli_success(&out, "cherry-pick --machine");
    let json = parse_json_stdout(&out);
    assert_eq!(json["command"], "cherry-pick");
    assert_eq!(json["data"]["picked"].as_array().unwrap().len(), 1);
}

// ── Batch 1a: cherry_pick_state SQLite sequencer facade ──

/// `CherryPickState` round-trips through the SQLite `cherry_pick_state` table
/// and clears cleanly (mirrors `RebaseState`).
#[tokio::test]
#[serial]
async fn cherry_pick_state_roundtrip_persists_and_clears() {
    use std::str::FromStr;

    use git_internal::hash::ObjectHash;
    use libra::command::cherry_pick::CherryPickState;

    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;
    let _guard = ChangeDirGuard::new(temp.path());

    assert!(
        !CherryPickState::is_in_progress().await.unwrap(),
        "a fresh repo has no in-progress cherry-pick"
    );

    let orig = ObjectHash::from_str(&"a".repeat(40)).unwrap();
    let current = ObjectHash::from_str(&"b".repeat(40)).unwrap();
    let next = ObjectHash::from_str(&"c".repeat(40)).unwrap();
    let state = CherryPickState {
        head_name: "main".to_string(),
        head_orig: orig,
        current_oid: current,
        todo: std::collections::VecDeque::from(vec![next]),
        opts_json: "{\"x\":true}".to_string(),
    };
    state.save().await.unwrap();

    assert!(CherryPickState::is_in_progress().await.unwrap());
    let loaded = CherryPickState::load()
        .await
        .unwrap()
        .expect("state present after save");
    assert_eq!(loaded.head_name, "main");
    assert_eq!(loaded.head_orig, orig);
    assert_eq!(loaded.current_oid, current);
    assert_eq!(loaded.todo, std::collections::VecDeque::from(vec![next]));
    assert_eq!(loaded.opts_json, "{\"x\":true}");

    CherryPickState::clear().await.unwrap();
    assert!(!CherryPickState::is_in_progress().await.unwrap());
    assert!(CherryPickState::load().await.unwrap().is_none());
}
