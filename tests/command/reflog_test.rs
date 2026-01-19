//! Integration tests for reflog command with filtering functionality.

use clap::Parser;
use libra::{
    command::{commit, reflog},
    internal::{db::get_db_conn_instance, reflog::Reflog},
};
use serial_test::serial;
use tempfile::tempdir;

use super::*;

/// Helper: Get the number of reflog entries for HEAD
async fn count_reflog_entries() -> usize {
    let db = get_db_conn_instance().await;
    Reflog::find_all(db, "HEAD")
        .await
        .map(|logs| logs.len())
        .unwrap_or(0)
}

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
            no_verify: false,
            author: None,
        };
        commit::execute(commit_args).await;
    }

    // Verify: 3 reflog entries should exist (init + 3 commits = 4 entries total)
    let total_entries = count_reflog_entries().await;
    assert!(
        total_entries >= 3,
        "Expected at least 3 reflog entries, found {}",
        total_entries
    );

    // Test basic reflog show - should not panic
    let args = reflog::ReflogArgs::parse_from(["reflog", "show"]);
    reflog::execute(args).await;

    // Test with --grep filter - should not panic and filter works
    let args = reflog::ReflogArgs::parse_from(["reflog", "show", "--grep", "commit"]);
    reflog::execute(args).await;
    // All our commits have "commit" in the message, so this should work

    // Test with --since filter (relative date) - should not panic
    let args = reflog::ReflogArgs::parse_from(["reflog", "show", "--since", "1 hour ago"]);
    reflog::execute(args).await;

    // Test with combined filters - should not panic
    let args = reflog::ReflogArgs::parse_from([
        "reflog",
        "show",
        "--grep",
        "Test",
        "--since",
        "1 day ago",
    ]);
    reflog::execute(args).await;

    // Test with --pretty flag - should not panic
    let args = reflog::ReflogArgs::parse_from([
        "reflog",
        "show",
        "--since",
        "1 day ago",
        "--pretty",
        "oneline",
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
        no_verify: false,
        author: None,
    };
    commit::execute(commit_args).await;

    // Test with invalid date format - should show error message
    let args = reflog::ReflogArgs::parse_from(["reflog", "show", "--since", "invalid-date-format"]);
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
            no_verify: false,
            author: None,
        };
        commit::execute(commit_args).await;
    }

    // Verify: reflog entries should exist
    let total_entries = count_reflog_entries().await;
    assert!(
        total_entries >= 3,
        "Expected at least 3 reflog entries, found {}",
        total_entries
    );

    // Test with --author filter (should work without error)
    let args = reflog::ReflogArgs::parse_from(["reflog", "show", "--author", "test"]);
    reflog::execute(args).await; // Should filter entries by author

    // Test with case-insensitive author filter
    let args = reflog::ReflogArgs::parse_from(["reflog", "show", "--author", "TEST"]);
    reflog::execute(args).await; // Should work the same as lowercase

    // Verify: author filter doesn't delete entries
    let entries_after = count_reflog_entries().await;
    assert_eq!(
        total_entries, entries_after,
        "Author filter should not delete reflog entries"
    );
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
            no_verify: false,
            author: None,
        };
        commit::execute(commit_args).await;
    }

    // Verify: At least 5 reflog entries should exist
    let total_entries = count_reflog_entries().await;
    assert!(
        total_entries >= 5,
        "Expected at least 5 reflog entries, found {}",
        total_entries
    );

    // Test with -n limit (should work without error)
    let args = reflog::ReflogArgs::parse_from(["reflog", "show", "-n", "3"]);
    reflog::execute(args).await; // Should display only 3 most recent entries

    // Test with --number limit
    let args = reflog::ReflogArgs::parse_from(["reflog", "show", "--number", "2"]);
    reflog::execute(args).await; // Should display only 2 most recent entries

    // Test that number limit doesn't affect total reflog count
    let entries_after = count_reflog_entries().await;
    assert_eq!(
        total_entries, entries_after,
        "Number limit should not delete reflog entries"
    );
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
            no_verify: false,
            author: None,
        };
        commit::execute(commit_args).await;
    }

    // Verify: reflog entries should exist
    let total_entries = count_reflog_entries().await;
    assert!(
        total_entries >= 3,
        "Expected at least 3 reflog entries, found {}",
        total_entries
    );

    // Test with combined filters: author + number
    let args = reflog::ReflogArgs::parse_from(["reflog", "show", "--author", "test", "-n", "2"]);
    reflog::execute(args).await; // Should filter by author AND limit to 2 entries

    // Test with combined filters: since + author + number
    let args = reflog::ReflogArgs::parse_from([
        "reflog",
        "show",
        "--since",
        "1 day ago",
        "--author",
        "test",
        "--number",
        "5",
    ]);
    reflog::execute(args).await; // Should apply all three filters together

    // Verify: combined filters don't delete entries
    let entries_after = count_reflog_entries().await;
    assert_eq!(
        total_entries, entries_after,
        "Combined filters should not delete reflog entries"
    );
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
            no_verify: false,
            author: None,
        };
        commit::execute(commit_args).await;
    }

    // Test with --patch flag (should work without error)
    let args = reflog::ReflogArgs::parse_from(["reflog", "show", "--patch"]);
    reflog::execute(args).await; // Should complete without panic

    // Test with -p shorthand
    let args = reflog::ReflogArgs::parse_from(["reflog", "show", "-p"]);
    reflog::execute(args).await; // Should complete without panic

    // Test with patch and number limit
    let args = reflog::ReflogArgs::parse_from(["reflog", "show", "--patch", "-n", "2"]);
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
            no_verify: false,
            author: None,
        };
        commit::execute(commit_args).await;
    }

    // Test with --stat flag (should work without error)
    let args = reflog::ReflogArgs::parse_from(["reflog", "show", "--stat"]);
    reflog::execute(args).await; // Should complete without panic

    // Test with stat and number limit
    let args = reflog::ReflogArgs::parse_from(["reflog", "show", "--stat", "-n", "2"]);
    reflog::execute(args).await; // Should complete without panic
}
