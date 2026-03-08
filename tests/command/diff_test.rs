//! Tests diff command across commits, stage, and working tree with algorithm and pathspec options.

use std::{fs, io::Write};

use clap::Parser;
use libra::command::diff::{self, DiffArgs};

use super::*;

#[test]
#[serial]
fn test_diff_cli_outside_repository_returns_fatal_128() {
    let temp = tempdir().unwrap();
    let output = run_libra_command(&["diff"], temp.path());
    assert_eq!(output.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("fatal: not a libra repository"),
        "unexpected stderr: {stderr}"
    );
}

/// Helper function to create a file with content.
fn create_file(path: &str, content: &str) {
    let mut file = fs::File::create(path).unwrap();
    file.write_all(content.as_bytes()).unwrap();
}

/// Helper function to modify a file with new content.
fn modify_file(path: &str, content: &str) {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)
        .unwrap();
    file.write_all(content.as_bytes()).unwrap();
}

#[tokio::test]
#[serial]
/// Tests diff command immediately after libra init (empty repository scenario).
/// This tests the edge case where there are no commits and no staged changes.
async fn test_diff_after_init() {
    let test_dir = tempdir().unwrap();
    let output_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = ChangeDirGuard::new(test_dir.path());

    let output_file = output_dir.path().join("diff_output.txt");
    let output_str = output_file.to_str().unwrap();
    let args = DiffArgs::parse_from(["diff", "--output", output_str]);
    diff::execute(args).await;

    // Empty repo should produce no diff output (or an empty file)
    let content = fs::read_to_string(&output_file).unwrap_or_default();
    assert!(
        content.is_empty(),
        "Expected no diff output after init, got: {content}"
    );
}

#[tokio::test]
#[serial]
/// Tests the basic diff functionality between working directory and HEAD.
async fn test_basic_diff() {
    let test_dir = tempdir().unwrap();
    let output_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // Create a file and add it to index
    create_file("file1.txt", "Initial content\nLine 2\nLine 3\n");

    add::execute(AddArgs {
        pathspec: vec![String::from("file1.txt")],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
    })
    .await;

    // Create initial commit
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
    })
    .await;

    // Modify the file
    modify_file("file1.txt", "Modified content\nLine 2\nLine 3 changed\n");

    // Run diff command with output to file to avoid pager
    let output_file = output_dir.path().join("diff_output.txt");
    let output_str = output_file.to_str().unwrap();
    let args = DiffArgs::parse_from(["diff", "--algorithm", "histogram", "--output", output_str]);
    diff::execute(args).await;

    let content = fs::read_to_string(&output_file).unwrap();
    assert!(
        content.contains("diff --git"),
        "Output should contain diff header"
    );
    assert!(
        content.contains("-Initial content"),
        "Output should show removed line"
    );
    assert!(
        content.contains("+Modified content"),
        "Output should show added line"
    );
}

#[tokio::test]
#[serial]
/// Tests diff with staged changes
async fn test_diff_staged() {
    let test_dir = tempdir().unwrap();
    let output_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // Create a file and add it to index
    create_file("file1.txt", "Initial content\nLine 2\nLine 3\n");

    add::execute(AddArgs {
        pathspec: vec![String::from("file1.txt")],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
    })
    .await;

    // Create initial commit
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
    })
    .await;

    // Modify the file and stage it
    modify_file("file1.txt", "Modified content\nLine 2\nLine 3 changed\n");

    add::execute(AddArgs {
        pathspec: vec![String::from("file1.txt")],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
    })
    .await;

    // Modify the file again (so working dir differs from staged)
    modify_file(
        "file1.txt",
        "Modified content again\nLine 2\nLine 3 changed again\n",
    );

    // Run diff command with --staged flag, output to file to avoid pager
    let output_file = output_dir.path().join("diff_output.txt");
    let output_str = output_file.to_str().unwrap();
    let args = DiffArgs::parse_from([
        "diff",
        "--staged",
        "--algorithm",
        "histogram",
        "--output",
        output_str,
    ]);
    diff::execute(args).await;

    let content = fs::read_to_string(&output_file).unwrap();
    assert!(
        content.contains("diff --git"),
        "Staged diff should contain diff header"
    );
    assert!(
        content.contains("-Initial content"),
        "Staged diff should show removed line"
    );
    assert!(
        content.contains("+Modified content"),
        "Staged diff should show added line"
    );
}

#[tokio::test]
#[serial]
/// Tests diff between two specific commits
async fn test_diff_between_commits() {
    let test_dir = tempdir().unwrap();
    let output_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // Create a file and make initial commit
    create_file("file1.txt", "Initial content\nLine 2\nLine 3\n");

    add::execute(AddArgs {
        pathspec: vec![String::from("file1.txt")],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
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
    })
    .await;

    // Get the first commit hash
    let first_commit = Head::current_commit().await.unwrap();

    // Modify file and create a second commit
    modify_file("file1.txt", "Modified content\nLine 2\nLine 3 changed\n");

    add::execute(AddArgs {
        pathspec: vec![String::from("file1.txt")],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
    })
    .await;

    commit::execute(CommitArgs {
        message: Some("Second commit".to_string()),
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

    // Get the second commit hash
    let second_commit = Head::current_commit().await.unwrap();

    // Run diff command comparing the two commits, output to file to avoid pager
    let output_file = output_dir.path().join("diff_output.txt");
    let output_str = output_file.to_str().unwrap();
    let args = DiffArgs::parse_from([
        "diff",
        "--old",
        &first_commit.to_string(),
        "--new",
        &second_commit.to_string(),
        "--algorithm",
        "histogram",
        "--output",
        output_str,
    ]);
    diff::execute(args).await;

    let content = fs::read_to_string(&output_file).unwrap();
    assert!(
        content.contains("diff --git"),
        "Commit diff should contain diff header"
    );
    assert!(
        content.contains("-Initial content"),
        "Commit diff should show removed line"
    );
    assert!(
        content.contains("+Modified content"),
        "Commit diff should show added line"
    );
}

#[tokio::test]
#[serial]
/// Tests diff with specific file path
async fn test_diff_with_pathspec() {
    let test_dir = tempdir().unwrap();
    let output_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // Create multiple files and commit them
    create_file("file1.txt", "File 1 content\nLine 2\nLine 3\n");
    create_file("file2.txt", "File 2 content\nLine 2\nLine 3\n");

    add::execute(AddArgs {
        pathspec: vec![String::from(".")],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
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
    })
    .await;

    // Modify both files
    modify_file("file1.txt", "File 1 modified\nLine 2\nLine 3 changed\n");
    modify_file("file2.txt", "File 2 modified\nLine 2\nLine 3 changed\n");

    // Run diff command with specific file path, output to file to avoid pager
    let output_file = output_dir.path().join("diff_output.txt");
    let output_str = output_file.to_str().unwrap();
    let args = DiffArgs::parse_from([
        "diff",
        "--algorithm",
        "histogram",
        "--output",
        output_str,
        "file1.txt",
    ]);
    diff::execute(args).await;

    let content = fs::read_to_string(&output_file).unwrap();
    assert!(
        content.contains("diff --git"),
        "Pathspec diff should contain diff header"
    );
    assert!(
        content.contains("file1.txt"),
        "Pathspec diff should reference file1.txt"
    );
    // file2.txt should NOT appear in the output since we filtered by pathspec
    assert!(
        !content.contains("file2.txt"),
        "Pathspec diff should not contain file2.txt"
    );
}

#[tokio::test]
#[serial]
/// Tests diff with output to a file
async fn test_diff_output_to_file() {
    let test_dir = tempdir().unwrap();
    let output_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // Create a file and commit it
    create_file("file1.txt", "Initial content\nLine 2\nLine 3\n");

    add::execute(AddArgs {
        pathspec: vec![String::from("file1.txt")],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
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
    })
    .await;

    // Modify the file
    modify_file("file1.txt", "Modified content\nLine 2\nLine 3 changed\n");

    // Output file path outside the repo
    let output_file = output_dir.path().join("diff_output.txt");
    let output_str = output_file.to_str().unwrap();

    // Run diff command with output to file
    let args = DiffArgs::parse_from(["diff", "--algorithm", "histogram", "--output", output_str]);
    diff::execute(args).await;

    // Verify the output file exists
    assert!(
        fs::metadata(&output_file).is_ok(),
        "Output file should exist"
    );

    // Read the file content to make sure it contains diff output
    let content = fs::read_to_string(&output_file).unwrap();
    assert!(
        content.contains("diff --git"),
        "Output should contain diff header"
    );
}

#[tokio::test]
#[serial]
/// Tests diff with different algorithms
async fn test_diff_algorithms() {
    let test_dir = tempdir().unwrap();
    let output_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // Create a file with some content to make a non-trivial diff
    create_file(
        "file1.txt",
        "Line 1\nLine 2\nLine 3\nLine 4\nLine 5\nLine 6\nLine 7\n",
    );

    add::execute(AddArgs {
        pathspec: vec![String::from("file1.txt")],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
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
    })
    .await;

    // Make complex changes to test different algorithms
    modify_file(
        "file1.txt",
        "Line 1\nModified Line\nLine 3\nNew Line\nLine 5\nLine 6\nDeleted Line 7\n",
    );

    // Test histogram algorithm
    let histogram_file = output_dir.path().join("histogram_diff.txt");
    let histogram_str = histogram_file.to_str().unwrap();
    let args = DiffArgs::parse_from([
        "diff",
        "--algorithm",
        "histogram",
        "--output",
        histogram_str,
    ]);
    diff::execute(args).await;

    // Test myers algorithm
    let myers_file = output_dir.path().join("myers_diff.txt");
    let myers_str = myers_file.to_str().unwrap();
    let args = DiffArgs::parse_from(["diff", "--algorithm", "myers", "--output", myers_str]);
    diff::execute(args).await;

    // Test myersMinimal algorithm
    let myers_min_file = output_dir.path().join("myersMinimal_diff.txt");
    let myers_min_str = myers_min_file.to_str().unwrap();
    let args = DiffArgs::parse_from([
        "diff",
        "--algorithm",
        "myersMinimal",
        "--output",
        myers_min_str,
    ]);
    diff::execute(args).await;

    // Verify all output files exist
    assert!(
        fs::metadata(&histogram_file).is_ok(),
        "Histogram output file should exist"
    );
    assert!(
        fs::metadata(&myers_file).is_ok(),
        "Myers output file should exist"
    );
    assert!(
        fs::metadata(&myers_min_file).is_ok(),
        "MyersMinimal output file should exist"
    );
}
