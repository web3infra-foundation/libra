//! Integration tests for reflog command with filtering functionality.

use clap::Parser;
use libra::command::{commit, reflog};
use serial_test::serial;
use tempfile::tempdir;

use super::*;

#[tokio::test]
#[serial]
async fn test_reflog_show_with_filters() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    // Create some commits to generate reflog entries
    for i in 1..=3 {
        let commit_args = commit::CommitArgs {
            message: Some(format!("Test commit {}", i)),
            file: None,
            allow_empty: true,
            conventional: false,
            amend: false,
            no_edit: false,
            signoff: false,
            disable_pre: true,
            all: false,
        };
        commit::execute(commit_args).await;
    }

    // Test basic reflog show
    let args = reflog::ReflogArgs::parse_from(["reflog", "show"]);
    reflog::execute(args).await;

    // Test with --grep filter
    let args = reflog::ReflogArgs::parse_from(["reflog", "show", "--grep", "commit"]);
    reflog::execute(args).await;

    // Test with --since filter (relative date)
    let args = reflog::ReflogArgs::parse_from(["reflog", "show", "--since", "1 hour ago"]);
    reflog::execute(args).await;

    // Test with combined filters
    let args = reflog::ReflogArgs::parse_from([
        "reflog", "show",
        "--grep", "Test",
        "--since", "1 day ago"
    ]);
    reflog::execute(args).await;

    // Test with --pretty flag
    let args = reflog::ReflogArgs::parse_from([
        "reflog", "show",
        "--since", "1 day ago",
        "--pretty", "oneline"
    ]);
    reflog::execute(args).await;
}

#[tokio::test]
#[serial]
async fn test_reflog_show_invalid_date() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    // Create a commit
    let commit_args = commit::CommitArgs {
        message: Some("Test commit".to_string()),
        file: None,
        allow_empty: true,
        conventional: false,
        amend: false,
        no_edit: false,
        signoff: false,
        disable_pre: true,
        all: false,
    };
    commit::execute(commit_args).await;

    // Test with invalid date format - should show error message
    let args = reflog::ReflogArgs::parse_from([
        "reflog", "show",
        "--since", "invalid-date-format"
    ]);
    reflog::execute(args).await; // Should print error but not panic
}

#[tokio::test]
#[serial]
async fn test_reflog_show_with_author_filter() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    // Create commits with different authors
    for i in 1..=3 {
        let commit_args = commit::CommitArgs {
            message: Some(format!("Test commit {}", i)),
            file: None,
            allow_empty: true,
            conventional: false,
            amend: false,
            no_edit: false,
            signoff: false,
            disable_pre: true,
            all: false,
        };
        commit::execute(commit_args).await;
    }

    // Test with --author filter (should work without error)
    let args = reflog::ReflogArgs::parse_from([
        "reflog", "show",
        "--author", "test"
    ]);
    reflog::execute(args).await; // Should complete without panic
}

#[tokio::test]
#[serial]
async fn test_reflog_show_with_number_limit() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    // Create 5 commits
    for i in 1..=5 {
        let commit_args = commit::CommitArgs {
            message: Some(format!("Test commit {}", i)),
            file: None,
            allow_empty: true,
            conventional: false,
            amend: false,
            no_edit: false,
            signoff: false,
            disable_pre: true,
            all: false,
        };
        commit::execute(commit_args).await;
    }

    // Test with -n limit (should work without error)
    let args = reflog::ReflogArgs::parse_from([
        "reflog", "show",
        "-n", "3"
    ]);
    reflog::execute(args).await; // Should complete without panic

    // Test with --number limit
    let args = reflog::ReflogArgs::parse_from([
        "reflog", "show",
        "--number", "2"
    ]);
    reflog::execute(args).await; // Should complete without panic
}

#[tokio::test]
#[serial]
async fn test_reflog_show_with_combined_filters() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    // Create commits
    for i in 1..=3 {
        let commit_args = commit::CommitArgs {
            message: Some(format!("Test commit {}", i)),
            file: None,
            allow_empty: true,
            conventional: false,
            amend: false,
            no_edit: false,
            signoff: false,
            disable_pre: true,
            all: false,
        };
        commit::execute(commit_args).await;
    }

    // Test with combined filters: author + number
    let args = reflog::ReflogArgs::parse_from([
        "reflog", "show",
        "--author", "test",
        "-n", "2"
    ]);
    reflog::execute(args).await; // Should complete without panic

    // Test with combined filters: since + author + number
    let args = reflog::ReflogArgs::parse_from([
        "reflog", "show",
        "--since", "1 day ago",
        "--author", "test",
        "--number", "5"
    ]);
    reflog::execute(args).await; // Should complete without panic
}

#[tokio::test]
#[serial]
async fn test_reflog_show_with_patch() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    // Create commits with file changes
    for i in 1..=3 {
        let commit_args = commit::CommitArgs {
            message: Some(format!("Test commit {}", i)),
            file: None,
            allow_empty: true,
            conventional: false,
            amend: false,
            no_edit: false,
            signoff: false,
            disable_pre: true,
            all: false,
        };
        commit::execute(commit_args).await;
    }

    // Test with --patch flag (should work without error)
    let args = reflog::ReflogArgs::parse_from([
        "reflog", "show",
        "--patch"
    ]);
    reflog::execute(args).await; // Should complete without panic

    // Test with -p shorthand
    let args = reflog::ReflogArgs::parse_from([
        "reflog", "show",
        "-p"
    ]);
    reflog::execute(args).await; // Should complete without panic

    // Test with patch and number limit
    let args = reflog::ReflogArgs::parse_from([
        "reflog", "show",
        "--patch",
        "-n", "2"
    ]);
    reflog::execute(args).await; // Should complete without panic
}

#[tokio::test]
#[serial]
async fn test_reflog_show_with_stat() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    // Create commits
    for i in 1..=3 {
        let commit_args = commit::CommitArgs {
            message: Some(format!("Test commit {}", i)),
            file: None,
            allow_empty: true,
            conventional: false,
            amend: false,
            no_edit: false,
            signoff: false,
            disable_pre: true,
            all: false,
        };
        commit::execute(commit_args).await;
    }

    // Test with --stat flag (should work without error)
    let args = reflog::ReflogArgs::parse_from([
        "reflog", "show",
        "--stat"
    ]);
    reflog::execute(args).await; // Should complete without panic

    // Test with stat and number limit
    let args = reflog::ReflogArgs::parse_from([
        "reflog", "show",
        "--stat",
        "-n", "2"
    ]);
    reflog::execute(args).await; // Should complete without panic
}
