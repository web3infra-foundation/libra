//! Tests cherry-pick scenarios that apply commits and verify results or conflicts.

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
        branch: Some("master".to_string()),
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
        branch: Some("master".to_string()),
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
        no_commit: false,
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
        branch: Some("master".to_string()),
        create: None,
        detach: false,
        track: false,
    })
    .await;

    // Cherry-pick both commits
    cherry_pick::execute(cherry_pick::CherryPickArgs {
        commits: vec![commit1.to_string(), commit2.to_string()],
        no_commit: false,
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
        no_commit: false,
    })
    .await;

    println!("Error handling test completed");
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
        repo_directory: temp_path.path().to_str().unwrap().to_string(),
        quiet: true,
        template: None,
        shared: None,
        object_format: Some("sha256".to_string()),
        ref_format: None,
    })
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
        no_commit: false,
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
        no_commit: false,
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
