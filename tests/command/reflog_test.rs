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
