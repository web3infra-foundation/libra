//! Integration tests for the grep command.

use std::fs;

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
            ..Default::default()
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
            ..Default::default()
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
            unset_upstream: None,
            show_current: false,
            rename: Vec::new(),
            remotes: false,
            all: false,
            contains: Vec::new(),
            no_contains: Vec::new(),
            merged: None,
            no_merged: None,
            points_at: None,
            ignore_case: false,
            sort: None,
            format: None,
            copy: vec![],
            force_copy: vec![],
            edit_description: None,
            force: false,
            create_reflog: false,
            track: None,
            no_track: false,
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
    let repo = tempdir().expect("failed to create repo dir");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = test::ChangeDirGuard::new(repo.path());

    fs::write("words.txt", "foo\nbar\nfoobar\nbarista\n").expect("failed to write file");
    add_and_commit("add words file", vec!["words.txt".to_string()]).await;

    let output = run_libra_command(&["--json=compact", "grep", "-w", "foo|bar"], repo.path());
    assert_cli_success(&output, "-w should preserve regex alternation semantics");

    let json = parse_json_stdout(&output);
    let matches = json["data"]["matches"]
        .as_array()
        .expect("expected grep matches array");
    let lines: Vec<&str> = matches
        .iter()
        .map(|entry| entry["line"].as_str().expect("expected matched line"))
        .collect();
    assert_eq!(lines, vec!["foo", "bar"]);
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
    assert_eq!(output.status.code(), Some(129));
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
    assert_eq!(output.status.code(), Some(129));

    let json = parse_json_stdout(&output);
    let warnings = json["data"]["warnings"]
        .as_array()
        .expect("expected warnings array");
    assert!(!warnings.is_empty());
    assert!(warnings[0]["path"] == "large-warning.txt");
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn test_grep_working_tree_symlink_emits_warning_and_skips_target() {
    use std::os::unix::fs::symlink;

    let repo = tempdir().expect("failed to create repo dir");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = test::ChangeDirGuard::new(repo.path());

    fs::write("real.txt", "needle\n").expect("failed to write real file");
    fs::write("link.txt", "needle\n").expect("failed to write tracked file");
    add_and_commit(
        "add target and tracked path",
        vec!["real.txt".to_string(), "link.txt".to_string()],
    )
    .await;
    fs::remove_file("link.txt").expect("failed to remove tracked file");
    symlink("real.txt", "link.txt").expect("failed to create symlink");

    let output = run_libra_command(&["--json=compact", "grep", "needle"], repo.path());
    assert_cli_success(&output, "grep should succeed while skipping symlink");

    let json = parse_json_stdout(&output);
    let warnings = json["data"]["warnings"]
        .as_array()
        .expect("expected warnings array");
    assert!(warnings.iter().any(|warning| warning["path"] == "link.txt"));
}

#[tokio::test]
#[serial]
async fn test_grep_returns_nonzero_when_no_matches_found() {
    let repo = tempdir().expect("failed to create repo dir");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = test::ChangeDirGuard::new(repo.path());

    fs::write("nomatch.txt", "alpha\nbeta\n").expect("failed to write file");
    add_and_commit("add no-match file", vec!["nomatch.txt".to_string()]).await;

    let output = run_libra_command(&["grep", "needle"], repo.path());
    assert_eq!(output.status.code(), Some(129));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error: no matches found"));
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

#[tokio::test]
#[serial]
async fn test_grep_all_match_is_based_on_positive_pattern_presence_even_with_invert() {
    let repo = tempdir().expect("failed to create repo dir");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = test::ChangeDirGuard::new(repo.path());

    fs::write("both.txt", "alpha\nkeep\nbeta\n").expect("failed to write file");
    fs::write("only-alpha.txt", "alpha\nkeep\n").expect("failed to write file");
    add_and_commit(
        "add invert all-match files",
        vec!["both.txt".to_string(), "only-alpha.txt".to_string()],
    )
    .await;

    let output = run_libra_command(
        &[
            "--json=compact",
            "grep",
            "-v",
            "--all-match",
            "-e",
            "alpha",
            "-e",
            "beta",
        ],
        repo.path(),
    );
    assert_cli_success(&output, "grep -v --all-match should succeed");

    let json = parse_json_stdout(&output);
    let matches = json["data"]["matches"]
        .as_array()
        .expect("expected grep matches array");
    let paths: Vec<&str> = matches
        .iter()
        .map(|entry| entry["path"].as_str().expect("expected match path"))
        .collect();
    assert!(paths.iter().all(|path| *path == "both.txt"));
}

/// `libra grep --help` surfaces the EXAMPLES banner so users see the
/// canonical invocations (regex vs literal, multi-pattern, --cached,
/// --tree REV, count, filename listing, --json) without reading the
/// design doc. Cross-cutting `--help` EXAMPLES rollout per
/// `docs/improvement/README.md` item B.
#[test]
fn test_grep_help_lists_examples_banner() {
    let repo = tempdir().expect("tempdir for grep --help");
    let output = run_libra_command(&["grep", "--help"], repo.path());
    assert!(
        output.status.success(),
        "grep --help should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("EXAMPLES:"),
        "grep --help should include EXAMPLES banner, stdout: {stdout}"
    );
    for invocation in [
        "libra grep 'TODO'",
        "libra grep -F 'fn foo()'",
        "libra grep -i 'panic'",
        "libra grep -n 'TODO' src/",
        "libra grep -c 'unsafe' src/",
        "libra grep -l 'unwrap()' src/",
        "libra grep -e 'TODO' -e 'FIXME'",
        "libra grep -A 2 'panic' src/",
        "libra grep -B 2 'panic' src/",
        "libra grep -C 2 'panic' src/",
        "libra grep --heading -n 'TODO' src/",
        "libra grep -z -l 'TODO' src/",
        "libra grep --cached 'TODO'",
        "libra grep --tree HEAD~5 'TODO'",
        "libra grep --json 'TODO'",
    ] {
        assert!(
            stdout.contains(invocation),
            "grep --help should include `{invocation}`, stdout: {stdout}"
        );
    }
}

/// `-E`/`--extended-regexp` and `-G`/`--basic-regexp` are accepted as aliases:
/// they match the same lines as the default engine.
#[tokio::test]
#[serial]
async fn test_grep_extended_basic_regexp_are_aliases() {
    let repo = tempdir().expect("failed to create repo dir");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = test::ChangeDirGuard::new(repo.path());

    fs::write("f.txt", "alpha\nbeta\ngamma\n").expect("write file");
    add_and_commit("add f", vec!["f.txt".to_string()]).await;

    for flag in ["-E", "-G"] {
        let output = run_libra_command(&["grep", flag, "be.a", "f.txt"], repo.path());
        assert_eq!(
            output.status.code(),
            Some(0),
            "grep {flag} should match like the default engine, stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("beta"),
            "grep {flag} should find the matching line"
        );
    }
}

/// `-P`/`--perl-regexp` is declined with a usage error (exit 129).
#[tokio::test]
#[serial]
async fn test_grep_perl_regexp_is_declined() {
    let repo = tempdir().expect("failed to create repo dir");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = test::ChangeDirGuard::new(repo.path());

    fs::write("f.txt", "hello\n").expect("write file");
    add_and_commit("add f", vec!["f.txt".to_string()]).await;

    let output = run_libra_command(&["grep", "-P", "hel+o", "f.txt"], repo.path());
    assert_eq!(
        output.status.code(),
        Some(129),
        "grep -P should be declined with exit 129, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("not supported"),
        "grep -P should explain it is unsupported"
    );
}

/// `-C <n>` emits both leading and trailing context, using `:` for match lines,
/// `-` for context lines, and `--` between non-contiguous groups.
#[tokio::test]
#[serial]
async fn test_grep_combined_context_emits_both_sides() {
    let repo = tempdir().expect("repo dir");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = test::ChangeDirGuard::new(repo.path());

    fs::write("lines", "1\n2\nmatch\n4\n5\n6\nmatch\n8\n").expect("write file");
    add_and_commit("add lines", vec!["lines".to_string()]).await;

    let output = run_libra_command(&["grep", "-n", "-C", "1", "match", "lines"], repo.path());
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "lines-2-2\nlines:3:match\nlines-4-4\n--\nlines-6-6\nlines:7:match\nlines-8-8\n",
        "context output should use `:`/`-`/`--` separators"
    );
}

/// `-A <n>` emits only trailing context lines.
#[tokio::test]
#[serial]
async fn test_grep_after_context_emits_trailing_lines() {
    let repo = tempdir().expect("repo dir");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = test::ChangeDirGuard::new(repo.path());

    fs::write("lines", "1\nmatch\n3\n4\n5\n").expect("write file");
    add_and_commit("add lines", vec!["lines".to_string()]).await;

    let output = run_libra_command(&["grep", "-n", "-A", "2", "match", "lines"], repo.path());
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "lines:2:match\nlines-3-3\nlines-4-4\n",
        "-A should emit trailing context only"
    );
}

/// `-B <n>` emits only leading context lines.
#[tokio::test]
#[serial]
async fn test_grep_before_context_emits_leading_lines() {
    let repo = tempdir().expect("repo dir");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = test::ChangeDirGuard::new(repo.path());

    fs::write("lines", "1\n2\n3\nmatch\n5\n").expect("write file");
    add_and_commit("add lines", vec!["lines".to_string()]).await;

    let output = run_libra_command(&["grep", "-n", "-B", "1", "match", "lines"], repo.path());
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "lines-3-3\nlines:4:match\n",
        "-B should emit leading context only"
    );
}

/// Overlapping context windows merge into one group (no `--` separator).
#[tokio::test]
#[serial]
async fn test_grep_context_overlap_omits_separator() {
    let repo = tempdir().expect("repo dir");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = test::ChangeDirGuard::new(repo.path());

    fs::write("lines", "a\nb\nmatch\nd\nmatch\nf\n").expect("write file");
    add_and_commit("add lines", vec!["lines".to_string()]).await;

    let output = run_libra_command(&["grep", "-n", "-C", "1", "match", "lines"], repo.path());
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("--"),
        "overlapping windows must not emit a group separator: {stdout}"
    );
    assert_eq!(
        stdout, "lines-2-b\nlines:3:match\nlines-4-d\nlines:5:match\nlines-6-f\n",
        "overlapping windows should render as one group"
    );
}

/// Combining `-A/-B/-C` with `--json` leaves the JSON schema unchanged: matches
/// are present and no context lines pollute the matches array.
#[tokio::test]
#[serial]
async fn test_grep_context_json_renders_without_error() {
    let repo = tempdir().expect("repo dir");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = test::ChangeDirGuard::new(repo.path());

    fs::write("lines", "1\nmatch\n3\n").expect("write file");
    add_and_commit("add lines", vec!["lines".to_string()]).await;

    let output = run_libra_command(
        &["--json=compact", "grep", "-A", "1", "match", "lines"],
        repo.path(),
    );
    assert_cli_success(&output, "grep --json with context should succeed");
    let json = parse_json_stdout(&output);
    let matches = json["data"]["matches"]
        .as_array()
        .expect("expected matches array");
    assert_eq!(
        matches.len(),
        1,
        "only the match line should appear in JSON"
    );
    assert_eq!(matches[0]["line"], "match");
}

/// By default a binary file is skipped with a warning (and so produces no match).
#[tokio::test]
#[serial]
async fn test_grep_binary_skipped_by_default_with_warning() {
    let repo = tempdir().expect("repo dir");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = test::ChangeDirGuard::new(repo.path());

    fs::write("bin.dat", b"prefix\x00needle\n").expect("write binary file");
    add_and_commit("add binary", vec!["bin.dat".to_string()]).await;

    let output = run_libra_command(&["grep", "needle", "bin.dat"], repo.path());
    assert_eq!(
        output.status.code(),
        Some(129),
        "binary skip means no match"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("skipped binary file"),
        "default should warn about the skipped binary file"
    );
}

/// `-a`/`--text` forces a binary file to be searched as text.
#[tokio::test]
#[serial]
async fn test_grep_text_flag_forces_binary_search() {
    let repo = tempdir().expect("repo dir");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = test::ChangeDirGuard::new(repo.path());

    fs::write("bin.dat", b"prefix\x00needle\n").expect("write binary file");
    add_and_commit("add binary", vec!["bin.dat".to_string()]).await;

    let output = run_libra_command(&["grep", "-a", "needle", "bin.dat"], repo.path());
    assert_eq!(output.status.code(), Some(0), "-a should find the match");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("needle"),
        "-a should print the matching line"
    );
}

/// `-I` silently skips binary files (no warning).
#[tokio::test]
#[serial]
async fn test_grep_capital_i_silently_skips_binary() {
    let repo = tempdir().expect("repo dir");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = test::ChangeDirGuard::new(repo.path());

    fs::write("bin.dat", b"prefix\x00needle\n").expect("write binary file");
    add_and_commit("add binary", vec!["bin.dat".to_string()]).await;

    let output = run_libra_command(&["grep", "-I", "needle", "bin.dat"], repo.path());
    assert_eq!(
        output.status.code(),
        Some(129),
        "binary skip means no match"
    );
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("skipped binary file"),
        "-I must not print a binary-skip warning"
    );
}

/// `--heading` prints the file name once as a header and drops the inline prefix.
#[tokio::test]
#[serial]
async fn test_grep_heading_prints_filename_header() {
    let repo = tempdir().expect("repo dir");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = test::ChangeDirGuard::new(repo.path());

    fs::write("a.txt", "match\nsecond match\n").expect("write file");
    add_and_commit("add a", vec!["a.txt".to_string()]).await;

    let output = run_libra_command(&["grep", "--heading", "-n", "match", "a.txt"], repo.path());
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "a.txt\n1:match\n2:second match\n",
        "--heading should print a header line and drop the inline prefix"
    );
}

/// `--no-heading` overrides `--heading`, restoring the inline file-name prefix.
#[tokio::test]
#[serial]
async fn test_grep_no_heading_overrides_heading() {
    let repo = tempdir().expect("repo dir");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = test::ChangeDirGuard::new(repo.path());

    fs::write("a.txt", "match\n").expect("write file");
    add_and_commit("add a", vec!["a.txt".to_string()]).await;

    let output = run_libra_command(
        &["grep", "--heading", "--no-heading", "-n", "match", "a.txt"],
        repo.path(),
    );
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "a.txt:1:match\n",
        "--no-heading should override --heading"
    );
}

/// `--break` inserts a blank line between the matches of different files.
#[tokio::test]
#[serial]
async fn test_grep_break_inserts_blank_line_between_files() {
    let repo = tempdir().expect("repo dir");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = test::ChangeDirGuard::new(repo.path());

    fs::write("a.txt", "match\n").expect("write a");
    fs::write("b.txt", "match\n").expect("write b");
    add_and_commit("add files", vec!["a.txt".to_string(), "b.txt".to_string()]).await;

    let output = run_libra_command(&["grep", "--break", "match"], repo.path());
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("match\n\n"),
        "--break should put a blank line between files: {stdout:?}"
    );
    assert!(stdout.contains("a.txt:match") && stdout.contains("b.txt:match"));
}

/// `-z`/`--null` uses NUL field separators while keeping records newline-terminated.
#[tokio::test]
#[serial]
async fn test_grep_null_uses_nul_separators() {
    let repo = tempdir().expect("repo dir");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = test::ChangeDirGuard::new(repo.path());

    fs::write("a.txt", "match\n").expect("write file");
    add_and_commit("add a", vec!["a.txt".to_string()]).await;

    let output = run_libra_command(&["grep", "-z", "-n", "match", "a.txt"], repo.path());
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "a.txt\u{0}1\u{0}match\n",
        "-z should use NUL field separators and a newline record terminator"
    );
}

#[tokio::test]
#[serial]
async fn test_grep_null_context_uses_nul_separators() {
    let repo = tempdir().expect("repo dir");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = test::ChangeDirGuard::new(repo.path());

    fs::write("lines", "before\nmatch\nafter\n").expect("write file");
    add_and_commit("add lines", vec!["lines".to_string()]).await;

    let output = run_libra_command(
        &["grep", "-z", "-n", "-C", "1", "match", "lines"],
        repo.path(),
    );
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "lines\u{0}1\u{0}before\nlines\u{0}2\u{0}match\nlines\u{0}3\u{0}after\n",
        "-z context output should use NUL field separators"
    );
}

/// `-z` with `-l` NUL-terminates each path and emits no trailing newline.
#[tokio::test]
#[serial]
async fn test_grep_null_file_list_nul_terminated() {
    let repo = tempdir().expect("repo dir");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = test::ChangeDirGuard::new(repo.path());

    fs::write("a.txt", "match\n").expect("write file");
    add_and_commit("add a", vec!["a.txt".to_string()]).await;

    let output = run_libra_command(&["grep", "-z", "-l", "match", "a.txt"], repo.path());
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "a.txt\u{0}",
        "-z -l should NUL-terminate paths with no trailing newline"
    );
}

/// `--no-index` searches the filesystem without requiring a repository.
#[test]
#[serial]
fn test_grep_no_index_runs_outside_repo() {
    let dir = tempdir().expect("dir");
    // No `libra init`: there is no repository here.
    fs::write(dir.path().join("plain.txt"), "needle here\n").expect("write file");

    let output = run_libra_command(&["grep", "--no-index", "needle"], dir.path());
    assert_eq!(
        output.status.code(),
        Some(0),
        "--no-index should work without a repo, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("needle"));
}

/// `--no-index` never descends into the `.libra` metadata directory.
#[tokio::test]
#[serial]
async fn test_grep_no_index_prunes_libra_dir() {
    let dir = tempdir().expect("dir");
    test::setup_with_new_libra_in(dir.path()).await;
    fs::write(dir.path().join(".libra/marker.txt"), "prunemarker\n").expect("marker in .libra");
    fs::write(dir.path().join("outside.txt"), "prunemarker\n").expect("marker outside");

    let output = run_libra_command(&["grep", "--no-index", "prunemarker"], dir.path());
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("outside.txt"),
        "should find the marker outside .libra: {stdout}"
    );
    assert!(
        !stdout.contains(".libra"),
        ".libra must be pruned from --no-index: {stdout}"
    );
}

/// `--no-index` restricts the walk to the given pathspec.
#[test]
#[serial]
fn test_grep_no_index_respects_pathspec() {
    let dir = tempdir().expect("dir");
    fs::create_dir(dir.path().join("sub")).expect("mkdir sub");
    fs::write(dir.path().join("sub/in.txt"), "needle\n").expect("write in");
    fs::write(dir.path().join("out.txt"), "needle\n").expect("write out");

    let output = run_libra_command(&["grep", "--no-index", "needle", "sub"], dir.path());
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("sub"),
        "should match inside the pathspec: {stdout}"
    );
    assert!(
        !stdout.contains("out.txt"),
        "pathspec should exclude out.txt: {stdout}"
    );
}

/// Conflicting search-scope flags are rejected at parse time. Libra surfaces
/// clap usage errors as `LBR-CLI-002` with exit code 129.
#[test]
#[serial]
fn test_grep_conflicting_scope_flags_rejected() {
    let dir = tempdir().expect("dir");
    fs::write(dir.path().join("f.txt"), "x\n").expect("write file");

    for pair in [
        ["--no-index", "--cached"],
        ["--no-index", "--tree"],
        ["--cached", "--tree"],
    ] {
        let mut argv = vec!["grep"];
        argv.extend_from_slice(&pair);
        if pair.contains(&"--tree") {
            argv.push("HEAD");
        }
        argv.push("x");
        let output = run_libra_command(&argv, dir.path());
        assert_eq!(
            output.status.code(),
            Some(129),
            "conflicting scope flags {pair:?} should be rejected as a usage error"
        );
    }
}

/// `--untracked` searches untracked files in addition to tracked ones.
#[tokio::test]
#[serial]
async fn test_grep_untracked_searches_untracked_files() {
    let repo = tempdir().expect("repo dir");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = test::ChangeDirGuard::new(repo.path());

    fs::write("tracked.txt", "needle\n").expect("write tracked");
    add_and_commit("add tracked", vec!["tracked.txt".to_string()]).await;
    fs::write("untracked.txt", "needle\n").expect("write untracked");

    // Default search only sees the tracked file.
    let default = run_libra_command(&["--json=compact", "grep", "needle"], repo.path());
    let default_paths = json_match_paths(&default);
    assert!(default_paths.contains(&"tracked.txt".to_string()));
    assert!(!default_paths.contains(&"untracked.txt".to_string()));

    // `--untracked` also sees the untracked file.
    let output = run_libra_command(
        &["--json=compact", "grep", "--untracked", "needle"],
        repo.path(),
    );
    assert_cli_success(&output, "grep --untracked should succeed");
    let paths = json_match_paths(&output);
    assert!(
        paths.contains(&"tracked.txt".to_string()),
        "tracked file should match: {paths:?}"
    );
    assert!(
        paths.contains(&"untracked.txt".to_string()),
        "untracked file should match: {paths:?}"
    );
}

/// `--untracked` excludes files matched by `.libraignore`.
#[tokio::test]
#[serial]
async fn test_grep_untracked_respects_libraignore() {
    let repo = tempdir().expect("repo dir");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = test::ChangeDirGuard::new(repo.path());

    fs::write(".libraignore", "ignored.txt\n").expect("write libraignore");
    fs::write("ignored.txt", "needle\n").expect("write ignored");
    fs::write("visible.txt", "needle\n").expect("write visible");

    let output = run_libra_command(
        &["--json=compact", "grep", "--untracked", "needle"],
        repo.path(),
    );
    assert_cli_success(&output, "grep --untracked should succeed");
    let paths = json_match_paths(&output);
    assert!(
        paths.contains(&"visible.txt".to_string()),
        "non-ignored untracked file should match: {paths:?}"
    );
    assert!(
        !paths.contains(&"ignored.txt".to_string()),
        "ignored untracked file must be excluded: {paths:?}"
    );
}

#[test]
fn test_grep_extended_regexp_alias_works() {
    let repo = tempdir().unwrap();
    super::init_repo_via_cli(repo.path());
    fs::write(repo.path().join("test.txt"), "foobar\nbaz\nquux").unwrap();
    super::run_libra_command(&["add", "test.txt"], repo.path());
    super::run_libra_command(
        &["commit", "--allow-empty", "-m", "initial"],
        repo.path(),
    );

    // Test --extended-regexp (should be no-op since Rust regex is already ERE-style)
    let extended = super::run_libra_command(
        &["grep", "--extended-regexp", "foo|baz", "test.txt"],
        repo.path(),
    );
    assert!(
        extended.status.success(),
        "grep --extended-regexp must succeed"
    );
    let output = String::from_utf8_lossy(&extended.stdout);
    assert!(output.contains("foobar") || output.contains("baz"), "must find matches with ERE pattern");
}

#[test]
fn test_grep_basic_regexp_alias_works() {
    let repo = tempdir().unwrap();
    super::init_repo_via_cli(repo.path());
    fs::write(repo.path().join("test.txt"), "foobar\nbaz\nquux").unwrap();
    super::run_libra_command(&["add", "test.txt"], repo.path());
    super::run_libra_command(
        &["commit", "--allow-empty", "-m", "initial"],
        repo.path(),
    );

    // Test --basic-regexp (should accept the flag as alias, pattern still interpreted as Rust regex)
    let basic = super::run_libra_command(
        &["grep", "--basic-regexp", "foo", "test.txt"],
        repo.path(),
    );
    assert!(
        basic.status.success(),
        "grep --basic-regexp must accept the flag"
    );
    let output = String::from_utf8_lossy(&basic.stdout);
    assert!(output.contains("foobar"), "must find matches with basic pattern");
}

#[test]
fn test_grep_perl_regexp_rejected() {
    let repo = tempdir().unwrap();
    super::init_repo_via_cli(repo.path());
    fs::write(repo.path().join("test.txt"), "foobar").unwrap();

    // Test -P/--perl-regexp (should be rejected)
    let perl = super::run_libra_command(
        &["grep", "--perl-regexp", "foo", "test.txt"],
        repo.path(),
    );
    assert!(
        !perl.status.success(),
        "grep --perl-regexp must be rejected"
    );
    let stderr = String::from_utf8_lossy(&perl.stderr);
    assert!(
        stderr.contains("not supported") || stderr.contains("Perl"),
        "error must mention Perl regex not supported: {stderr}"
    );
}

/// Collect the `path` of each match in a `--json` grep output.
fn json_match_paths(output: &std::process::Output) -> Vec<String> {
    let json = parse_json_stdout(output);
    json["data"]["matches"]
        .as_array()
        .map(|matches| {
            matches
                .iter()
                .filter_map(|m| m["path"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}
