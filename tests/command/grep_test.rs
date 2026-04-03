//! Integration tests for the grep command.

use std::fs;

use clap::Parser;
use libra::{
    command::{
        add::{self, AddArgs},
        branch::{self, BranchArgs},
        commit::{self, CommitArgs},
    },
    utils::{output::OutputConfig, test},
};
use serde_json::Value;
use serial_test::serial;
use tempfile::tempdir;

use super::{assert_cli_success, parse_cli_error_stderr, parse_json_stdout, run_libra_command};

async fn add_and_commit(message: &str, pathspec: Vec<String>) {
    add::execute_safe(
        AddArgs {
            pathspec,
            all: false,
            update: false,
            refresh: false,
            verbose: false,
            force: false,
            dry_run: false,
            ignore_errors: false,
        },
        &OutputConfig::default(),
    )
    .await
    .expect("failed to add files");

    commit::execute_safe(
        CommitArgs {
            message: Some(message.to_string()),
            allow_empty: false,
            conventional: false,
            amend: false,
            no_edit: false,
            signoff: false,
            disable_pre: false,
            all: false,
            no_verify: true,
            author: None,
            file: None,
        },
        &OutputConfig::default(),
    )
    .await
    .expect("failed to commit files");
}

#[tokio::test]
#[serial]
async fn test_grep_working_tree_searches_only_tracked_files() {
    let repo = tempdir().expect("failed to create repo dir");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = test::ChangeDirGuard::new(repo.path());

    fs::write("tracked.txt", "needle in tracked file\n").expect("failed to write tracked file");
    add_and_commit("add tracked file", vec!["tracked.txt".to_string()]).await;

    fs::write("tracked.txt", "needle in tracked file\nupdated needle\n")
        .expect("failed to update tracked file");
    fs::write("untracked.txt", "needle in untracked file\n")
        .expect("failed to write untracked file");

    let output = run_libra_command(&["--json=compact", "grep", "needle"], repo.path());
    assert_cli_success(&output, "grep should succeed for tracked files only");

    let json = parse_json_stdout(&output);
    let matches = json["data"]["matches"]
        .as_array()
        .expect("expected grep matches array");
    let paths: Vec<&str> = matches
        .iter()
        .map(|entry| entry["path"].as_str().expect("expected match path"))
        .collect();

    assert!(paths.iter().all(|path| *path == "tracked.txt"));
    assert!(
        matches
            .iter()
            .any(|entry| entry["line"] == "updated needle")
    );
    assert!(!paths.contains(&"untracked.txt"));
}

#[tokio::test]
#[serial]
async fn test_grep_tree_head_searches_committed_content() {
    let repo = tempdir().expect("failed to create repo dir");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = test::ChangeDirGuard::new(repo.path());

    fs::write("history.txt", "present in head\n").expect("failed to write file");
    add_and_commit("add history file", vec!["history.txt".to_string()]).await;

    fs::write("history.txt", "working tree only\n").expect("failed to update file");

    let output = run_libra_command(
        &["--json=compact", "grep", "--tree", "HEAD", "present"],
        repo.path(),
    );
    assert_cli_success(&output, "grep --tree HEAD should succeed");

    let json = parse_json_stdout(&output);
    let matches = json["data"]["matches"]
        .as_array()
        .expect("expected grep matches array");

    assert_eq!(
        json["data"]["context"],
        Value::String("tree:HEAD".to_string())
    );
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0]["path"], Value::String("history.txt".to_string()));
    assert_eq!(
        matches[0]["line"],
        Value::String("present in head".to_string())
    );
}

#[tokio::test]
#[serial]
async fn test_grep_tree_accepts_branch_revisions() {
    let repo = tempdir().expect("failed to create repo dir");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = test::ChangeDirGuard::new(repo.path());

    fs::write("branch.txt", "main branch text\n").expect("failed to write file");
    add_and_commit("base commit", vec!["branch.txt".to_string()]).await;

    branch::execute_safe(
        BranchArgs {
            new_branch: Some("feature/grep-tree".to_string()),
            commit_hash: None,
            list: false,
            delete: None,
            delete_safe: None,
            set_upstream_to: None,
            show_current: false,
            rename: Vec::new(),
            remotes: false,
            all: false,
            contains: Vec::new(),
            no_contains: Vec::new(),
        },
        &OutputConfig::default(),
    )
    .await
    .expect("failed to create branch");

    fs::write("branch.txt", "feature branch text\n").expect("failed to update file");
    add_and_commit("update current branch", vec!["branch.txt".to_string()]).await;

    let output = run_libra_command(
        &[
            "--json=compact",
            "grep",
            "--tree",
            "feature/grep-tree",
            "main",
        ],
        repo.path(),
    );
    assert_cli_success(&output, "grep should resolve branch revisions");

    let json = parse_json_stdout(&output);
    let matches = json["data"]["matches"]
        .as_array()
        .expect("expected grep matches array");

    assert_eq!(matches.len(), 1);
    assert_eq!(
        matches[0]["line"],
        Value::String("main branch text".to_string())
    );
}

#[tokio::test]
#[serial]
async fn test_grep_word_regexp_preserves_regex_semantics() {
    let temp_path = tempdir().expect("failed to create temp dir");
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    fs::write("file.txt", "foo\nbar\nfoobar\nbarista\n").expect("failed to write file");

    add::execute_safe(
        AddArgs {
            pathspec: vec!["file.txt".to_string()],
            all: false,
            update: false,
            refresh: false,
            verbose: false,
            force: false,
            dry_run: false,
            ignore_errors: false,
        },
        &OutputConfig::default(),
    )
    .await
    .expect("failed to add file");

    let grep_args = libra::command::grep::GrepArgs::parse_from(["libra", "grep", "-w", "foo|bar"]);
    let result = libra::command::grep::execute_safe(grep_args, &OutputConfig::default()).await;

    assert!(
        result.is_ok(),
        "-w should accept regex alternation patterns"
    );
}

#[tokio::test]
#[serial]
async fn test_grep_tree_reports_invalid_revision() {
    let repo = tempdir().expect("failed to create repo dir");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = test::ChangeDirGuard::new(repo.path());

    fs::write("tracked.txt", "tracked\n").expect("failed to write file");
    add_and_commit("add tracked file", vec!["tracked.txt".to_string()]).await;

    let output = run_libra_command(
        &["--json=compact", "grep", "--tree", "missing-ref", "tracked"],
        repo.path(),
    );
    assert!(
        !output.status.success(),
        "grep with invalid revision should fail"
    );

    let (_human, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert!(report.message.contains("invalid revision: missing-ref"));
}

#[tokio::test]
#[serial]
async fn test_grep_byte_offset_reports_zero_based_match_offset() {
    let repo = tempdir().expect("failed to create repo dir");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = test::ChangeDirGuard::new(repo.path());

    fs::write("offset.txt", "alpha\nfoo bar\n").expect("failed to write file");
    add_and_commit("add offset file", vec!["offset.txt".to_string()]).await;

    let output = run_libra_command(&["--json=compact", "grep", "-b", "bar"], repo.path());
    assert_cli_success(&output, "grep -b should succeed");

    let json = parse_json_stdout(&output);
    let matches = json["data"]["matches"]
        .as_array()
        .expect("expected grep matches array");

    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0]["path"], Value::String("offset.txt".to_string()));
    assert_eq!(matches[0]["byte_offset"], Value::from(4));
}

#[tokio::test]
#[serial]
async fn test_grep_tree_skips_large_blob_files() {
    let repo = tempdir().expect("failed to create repo dir");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = test::ChangeDirGuard::new(repo.path());

    let large_content = "needle\n".repeat(90_000);
    fs::write("large.txt", large_content).expect("failed to write large file");
    add_and_commit("add large file", vec!["large.txt".to_string()]).await;

    let output = run_libra_command(
        &["--json=compact", "grep", "--tree", "HEAD", "needle"],
        repo.path(),
    );
    assert_cli_success(
        &output,
        "grep should skip oversized tree blobs without failing",
    );
    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["total_matches"], Value::from(0));
    assert_eq!(json["data"]["total_files"], Value::from(0));
    assert_eq!(json["data"]["matches"], Value::Array(Vec::new()));
}

#[tokio::test]
#[serial]
async fn test_grep_reports_total_files_as_number_of_matched_files() {
    let repo = tempdir().expect("failed to create repo dir");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = test::ChangeDirGuard::new(repo.path());

    fs::write("a.txt", "needle once\n").expect("failed to write a.txt");
    fs::write("b.txt", "needle twice\nneedle again\n").expect("failed to write b.txt");
    fs::write("c.txt", "no match here\n").expect("failed to write c.txt");
    add_and_commit(
        "add multiple files",
        vec![
            "a.txt".to_string(),
            "b.txt".to_string(),
            "c.txt".to_string(),
        ],
    )
    .await;

    let output = run_libra_command(&["--json=compact", "grep", "needle"], repo.path());
    assert_cli_success(&output, "grep should succeed");

    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["total_files"], Value::from(2));
    assert_eq!(json["data"]["total_matches"], Value::from(3));
}

#[tokio::test]
#[serial]
async fn test_grep_count_reports_matching_lines_per_file() {
    let repo = tempdir().expect("failed to create repo dir");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = test::ChangeDirGuard::new(repo.path());

    fs::write("count.txt", "needle needle\nneedle again\nnone\n")
        .expect("failed to write count file");
    add_and_commit("add count file", vec!["count.txt".to_string()]).await;

    let output = run_libra_command(&["--json=compact", "grep", "-c", "needle"], repo.path());
    assert_cli_success(&output, "grep -c should succeed");

    let json = parse_json_stdout(&output);
    let counts = json["data"]["counts"]
        .as_array()
        .expect("expected grep counts array");

    assert_eq!(json["data"]["total_files"], Value::from(1));
    assert_eq!(json["data"]["total_matches"], Value::from(2));
    assert_eq!(counts.len(), 1);
    assert_eq!(counts[0]["path"], Value::String("count.txt".to_string()));
    assert_eq!(counts[0]["count"], Value::from(2));
}

#[tokio::test]
#[serial]
async fn test_grep_files_without_matches_uses_plural_json_field() {
    let repo = tempdir().expect("failed to create repo dir");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = test::ChangeDirGuard::new(repo.path());

    fs::write("match.txt", "needle here\n").expect("failed to write match file");
    fs::write("miss.txt", "nothing here\n").expect("failed to write miss file");
    add_and_commit(
        "add match and miss files",
        vec!["match.txt".to_string(), "miss.txt".to_string()],
    )
    .await;

    let output = run_libra_command(&["--json=compact", "grep", "-L", "needle"], repo.path());
    assert_cli_success(&output, "grep -L should succeed");

    let json = parse_json_stdout(&output);
    let misses = json["data"]["files_without_matches"]
        .as_array()
        .expect("expected files_without_matches array");

    assert_eq!(json["data"]["total_files"], Value::from(1));
    assert_eq!(misses.len(), 1);
    assert_eq!(misses[0], Value::String("miss.txt".to_string()));
    assert_eq!(json["data"]["files_without_match"], Value::Null);
    assert_eq!(json["data"]["files_with_matches"], Value::Null);
}

#[tokio::test]
#[serial]
async fn test_grep_multiple_regexp_patterns_match_any() {
    let repo = tempdir().expect("failed to create repo dir");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = test::ChangeDirGuard::new(repo.path());

    fs::write("multi.txt", "alpha\nbeta\ngamma\n").expect("failed to write file");
    add_and_commit("add multi file", vec!["multi.txt".to_string()]).await;

    let output = run_libra_command(
        &["--json=compact", "grep", "-e", "alpha", "-e", "gamma"],
        repo.path(),
    );
    assert_cli_success(&output, "grep with multiple -e patterns should succeed");

    let json = parse_json_stdout(&output);
    let matches = json["data"]["matches"]
        .as_array()
        .expect("expected grep matches array");
    assert_eq!(matches.len(), 2);
}

#[tokio::test]
#[serial]
async fn test_grep_reads_patterns_from_file() {
    let repo = tempdir().expect("failed to create repo dir");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = test::ChangeDirGuard::new(repo.path());

    fs::write("target.txt", "alpha\nbeta\ngamma\n").expect("failed to write file");
    fs::write("patterns.txt", "beta\ngamma\n").expect("failed to write pattern file");
    add_and_commit("add target file", vec!["target.txt".to_string()]).await;

    let output = run_libra_command(
        &["--json=compact", "grep", "-f", "patterns.txt"],
        repo.path(),
    );
    assert_cli_success(&output, "grep -f should succeed");

    let json = parse_json_stdout(&output);
    let matches = json["data"]["matches"]
        .as_array()
        .expect("expected grep matches array");
    assert_eq!(matches.len(), 2);
}

#[tokio::test]
#[serial]
async fn test_grep_invalid_pattern_file_returns_structured_error() {
    let repo = tempdir().expect("failed to create repo dir");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = test::ChangeDirGuard::new(repo.path());

    let output = run_libra_command(
        &["--json=compact", "grep", "-f", "missing.txt"],
        repo.path(),
    );
    assert!(
        !output.status.success(),
        "grep with missing pattern file should fail"
    );

    let (_human, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-IO-001");
    assert!(
        report
            .message
            .contains("failed to read pattern file 'missing.txt'")
    );
}

#[tokio::test]
#[serial]
async fn test_grep_tree_large_blob_emits_warning_in_json() {
    let repo = tempdir().expect("failed to create repo dir");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = test::ChangeDirGuard::new(repo.path());

    let large_content = "needle\n".repeat(90_000);
    fs::write("large-warning.txt", large_content).expect("failed to write large file");
    add_and_commit(
        "add large warning file",
        vec!["large-warning.txt".to_string()],
    )
    .await;

    let output = run_libra_command(
        &["--json=compact", "grep", "--tree", "HEAD", "needle"],
        repo.path(),
    );
    assert_cli_success(&output, "grep should succeed and report warnings");

    let json = parse_json_stdout(&output);
    let warnings = json["data"]["warnings"]
        .as_array()
        .expect("expected warnings array");
    assert!(!warnings.is_empty());
    assert!(warnings[0]["path"] == "large-warning.txt");
}

#[tokio::test]
#[serial]
async fn test_grep_all_match_requires_all_patterns_in_same_file() {
    let repo = tempdir().expect("failed to create repo dir");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = test::ChangeDirGuard::new(repo.path());

    fs::write("all-match.txt", "alpha\nonly-one\nbeta\n").expect("failed to write file");
    fs::write("partial.txt", "alpha only\n").expect("failed to write file");
    add_and_commit(
        "add all-match files",
        vec!["all-match.txt".to_string(), "partial.txt".to_string()],
    )
    .await;

    let output = run_libra_command(
        &[
            "--json=compact",
            "grep",
            "--all-match",
            "-e",
            "alpha",
            "-e",
            "beta",
        ],
        repo.path(),
    );
    assert_cli_success(&output, "grep --all-match should succeed");

    let json = parse_json_stdout(&output);
    let matches = json["data"]["matches"]
        .as_array()
        .expect("expected grep matches array");
    let paths: Vec<&str> = matches
        .iter()
        .map(|entry| entry["path"].as_str().expect("expected match path"))
        .collect();
    assert!(paths.iter().all(|path| *path == "all-match.txt"));
}
