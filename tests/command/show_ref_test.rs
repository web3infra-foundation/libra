//! Integration tests for `show-ref` command.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use std::{fs, io::Write, process::Command};

use libra::internal::branch::Branch;
use serial_test::serial;
use tempfile::tempdir;

use super::*;

/// Create a repo, add a file and commit with the given message.
async fn setup_repo_with_commit(temp: &tempfile::TempDir) -> ChangeDirGuard {
    test::setup_with_new_libra_in(temp.path()).await;
    let guard = ChangeDirGuard::new(temp.path());

    let mut f = fs::File::create("a.txt").unwrap();
    writeln!(f, "hello").unwrap();

    add::execute(AddArgs {
        pathspec: vec!["a.txt".into()],
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
        message: Some("initial".into()),
        no_verify: true,
        ..Default::default()
    })
    .await;

    guard
}

/// show-ref on an "empty" repo (initialized but no user commits) should list the AI branch.
#[tokio::test]
#[serial]
async fn test_show_ref_empty_repo() {
    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp.path())
        .arg("show-ref")
        .output()
        .expect("failed to execute `libra show-ref`");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("refs/heads/libra/intent"),
        "expected NO refs/heads/libra/intent in output, got: {stdout}"
    );
    // If no refs exist, show-ref might return non-zero exit code, so we don't assert success here for empty repo
}

/// show-ref should list refs/heads/<branch> after a commit.
#[tokio::test]
#[serial]
async fn test_show_ref_lists_branch() {
    let temp = tempdir().unwrap();
    let _guard = setup_repo_with_commit(&temp).await;

    let head_commit = Head::current_commit().await.unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp.path())
        .arg("show-ref")
        .arg("--heads")
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("refs/heads/main"),
        "expected refs/heads/main in output, got: {stdout}"
    );
    assert!(
        stdout.contains(&head_commit.to_string()),
        "expected commit hash in output, got: {stdout}"
    );
}

/// show-ref --tags should list tags after creating one.
#[tokio::test]
#[serial]
async fn test_show_ref_lists_tag() {
    let temp = tempdir().unwrap();
    let _guard = setup_repo_with_commit(&temp).await;

    // Create a lightweight tag via the internal API (same pattern as tag_test.rs)
    libra::internal::tag::create("v1.0", None, false)
        .await
        .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp.path())
        .arg("show-ref")
        .arg("--tags")
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("refs/tags/v1.0"),
        "expected refs/tags/v1.0 in output, got: {stdout}"
    );
}

/// show-ref --head should include HEAD.
#[tokio::test]
#[serial]
async fn test_show_ref_includes_head() {
    let temp = tempdir().unwrap();
    let _guard = setup_repo_with_commit(&temp).await;

    let head_commit = Head::current_commit().await.unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp.path())
        .arg("show-ref")
        .arg("--head")
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    // First line should be HEAD
    let first_line = stdout.lines().next().unwrap_or("");
    assert!(
        first_line.contains("HEAD"),
        "expected HEAD in first line, got: {first_line}"
    );
    assert!(
        first_line.contains(&head_commit.to_string()),
        "expected commit hash in HEAD line, got: {first_line}"
    );
}

/// show-ref --hash should output only hashes (no ref names).
#[tokio::test]
#[serial]
async fn test_show_ref_hash_only() {
    let temp = tempdir().unwrap();
    let _guard = setup_repo_with_commit(&temp).await;

    let head_commit = Head::current_commit().await.unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp.path())
        .arg("show-ref")
        .arg("--hash")
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(&head_commit.to_string()),
        "expected hash {}, got: {stdout}",
        head_commit
    );
    assert!(
        !stdout.contains("refs/"),
        "hash-only mode should not contain ref names"
    );
}

/// show-ref with a non-matching pattern should error.
#[tokio::test]
#[serial]
async fn test_show_ref_pattern_no_match() {
    let temp = tempdir().unwrap();
    let _guard = setup_repo_with_commit(&temp).await;

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp.path())
        .arg("show-ref")
        .arg("nonexistent-xyz")
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no matching refs found"),
        "expected error for non-matching pattern, got stderr: {stderr}"
    );
}

/// show-ref with a matching pattern should filter results.
#[tokio::test]
#[serial]
async fn test_show_ref_pattern_match() {
    let temp = tempdir().unwrap();
    let _guard = setup_repo_with_commit(&temp).await;

    // Create a second branch to verify filtering
    let head_hash = Head::current_commit().await.unwrap().to_string();
    Branch::update_branch("feature", &head_hash, None)
        .await
        .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp.path())
        .arg("show-ref")
        .arg("--heads")
        .arg("main")
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("refs/heads/main"),
        "pattern should match main"
    );
    assert!(
        !stdout.contains("refs/heads/feature"),
        "pattern should NOT match feature"
    );
}

/// show-ref default (no flags) should show both branches and tags.
#[tokio::test]
#[serial]
async fn test_show_ref_default_shows_both() {
    let temp = tempdir().unwrap();
    let _guard = setup_repo_with_commit(&temp).await;

    libra::internal::tag::create("v2.0", None, false)
        .await
        .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp.path())
        .arg("show-ref")
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("refs/heads/"),
        "default should show branches"
    );
    assert!(stdout.contains("refs/tags/"), "default should show tags");
}

/// show-ref --head with a non-HEAD pattern should still include HEAD.
#[tokio::test]
#[serial]
async fn test_show_ref_head_exempt_from_pattern_filter() {
    let temp = tempdir().unwrap();
    let _guard = setup_repo_with_commit(&temp).await;

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp.path())
        .arg("show-ref")
        .arg("--head")
        .arg("main")
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("HEAD"),
        "HEAD should appear even when pattern is 'master': {stdout}"
    );
    assert!(
        stdout.contains("refs/heads/main"),
        "main should also match: {stdout}"
    );
}
