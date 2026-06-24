//! Tests diff command across commits, stage, and working tree with algorithm and pathspec options.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use std::{fs, io::Write};

use clap::Parser;
use libra::{
    command::diff::{self, DiffArgs},
    utils::{output::OutputConfig, pager::LIBRA_PAGER_ENV},
};

use super::*;

#[test]
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

#[test]
fn test_diff_json_output_includes_file_stats() {
    let repo = create_committed_repo_via_cli();
    fs::write(repo.path().join("tracked.txt"), "tracked\nupdated\n").unwrap();

    let output = run_libra_command(&["--json", "diff"], repo.path());
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "diff");
    assert_eq!(json["data"]["files_changed"], 1);
    assert_eq!(json["data"]["files"][0]["path"], "tracked.txt");
    assert!(json["data"]["files"][0]["hunks"].as_array().is_some());
}

#[test]
fn test_diff_two_dot_range_positional() {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();

    fs::write(p.join("a.txt"), "one\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "a.txt"], p), "add c1");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "c1", "--no-verify"], p),
        "commit c1",
    );
    let c1 = String::from_utf8_lossy(&run_libra_command(&["rev-parse", "HEAD"], p).stdout)
        .trim()
        .to_string();

    fs::write(p.join("a.txt"), "two\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "a.txt"], p), "add c2");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "c2", "--no-verify"], p),
        "commit c2",
    );
    let c2 = String::from_utf8_lossy(&run_libra_command(&["rev-parse", "HEAD"], p).stdout)
        .trim()
        .to_string();

    // `diff A..B` (positional two-dot range) should diff the two commits.
    let out = run_libra_command(&["diff", &format!("{c1}..{c2}")], p);
    assert_cli_success(&out, "diff A..B");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("a.txt"),
        "diff A..B should mention a.txt: {stdout}"
    );
    assert!(
        stdout.contains("one") && stdout.contains("two"),
        "diff A..B should show the one->two change: {stdout}"
    );
}

#[test]
fn test_diff_machine_output_is_single_line_json() {
    let repo = create_committed_repo_via_cli();
    fs::write(repo.path().join("tracked.txt"), "tracked\nupdated\n").unwrap();

    let output = run_libra_command(&["--machine", "diff"], repo.path());
    assert_cli_success(&output, "machine diff");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let non_empty_lines: Vec<&str> = stdout.lines().filter(|line| !line.is_empty()).collect();
    assert_eq!(
        non_empty_lines.len(),
        1,
        "machine output should be exactly one non-empty line, got: {stdout}"
    );

    let parsed: serde_json::Value =
        serde_json::from_str(non_empty_lines[0]).expect("machine output should be valid JSON");
    assert_eq!(parsed["command"], "diff");
    assert_eq!(parsed["data"]["files_changed"], 1);
}

#[test]
fn test_diff_reports_tracked_files_inside_ignored_directories() {
    let repo = create_committed_repo_via_cli();
    fs::write(repo.path().join(".libraignore"), "target/\n").unwrap();
    fs::create_dir_all(repo.path().join("target")).unwrap();
    fs::write(repo.path().join("target/tracked.txt"), "tracked\n").unwrap();

    let add = run_libra_command(
        &["add", "-f", ".libraignore", "target/tracked.txt"],
        repo.path(),
    );
    assert_cli_success(&add, "force-add tracked file under ignored directory");
    let commit = run_libra_command(
        &[
            "commit",
            "-m",
            "track ignored directory file",
            "--no-verify",
        ],
        repo.path(),
    );
    assert_cli_success(&commit, "commit ignored directory fixture");

    fs::write(repo.path().join("target/tracked.txt"), "tracked\nupdated\n").unwrap();
    fs::write(repo.path().join("target/untracked.txt"), "ignored\n").unwrap();

    let output = run_libra_command(&["diff", "--name-only"], repo.path());
    assert_cli_success(&output, "diff ignored directory tracked file");

    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "target/tracked.txt"
    );
}

#[test]
fn test_diff_human_worktree_diff_emits_scan_progress() {
    let repo = create_committed_repo_via_cli();
    fs::write(repo.path().join("tracked.txt"), "tracked\nupdated\n").unwrap();

    let output = run_libra_command(&["diff", "--name-only"], repo.path());
    assert_cli_success(&output, "human worktree diff with scan progress");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Scanning working tree"),
        "expected worktree scan progress on stderr, got: {stderr}"
    );
}

#[test]
fn test_diff_progress_none_suppresses_scan_progress() {
    let repo = create_committed_repo_via_cli();
    fs::write(repo.path().join("tracked.txt"), "tracked\nupdated\n").unwrap();

    let output = run_libra_command(&["--progress=none", "diff", "--name-only"], repo.path());
    assert_cli_success(&output, "diff with progress disabled");

    assert!(
        output.stderr.is_empty(),
        "explicit --progress=none should suppress scan progress, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_diff_non_default_algorithm_fails_instead_of_silent_noop() {
    let repo = create_committed_repo_via_cli();
    fs::write(repo.path().join("tracked.txt"), "tracked\nupdated\n").unwrap();

    let output = run_libra_command(&["diff", "--algorithm", "myers"], repo.path());
    assert_eq!(output.status.code(), Some(129));
    assert!(
        output.stdout.is_empty(),
        "unsupported algorithm must not emit a best-effort diff to stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("diff --algorithm=myers is not supported yet"),
        "unsupported algorithm should be explicit, stderr={stderr}"
    );
    assert!(
        stderr.contains("Error-Code: LBR-CLI-002"),
        "unsupported algorithm should carry a stable CLI error code, stderr={stderr}"
    );
    assert!(
        !stderr.contains("Scanning working tree"),
        "algorithm validation should fail before the worktree scan, stderr={stderr}"
    );
}

#[tokio::test]
#[serial]
async fn test_diff_empty_output_does_not_initialize_pager() {
    if cfg!(windows) {
        return;
    }

    let repo = create_committed_repo_via_cli();
    let _guard = ChangeDirGuard::new(repo.path());
    let missing_bin_dir = tempdir().unwrap();
    let _path = test::ScopedEnvVar::set("PATH", missing_bin_dir.path());
    let _pager = test::ScopedEnvVar::set(LIBRA_PAGER_ENV, "always");

    let args = DiffArgs::try_parse_from(["libra"]).unwrap();
    let result = diff::execute_safe(args, &OutputConfig::default()).await;
    assert!(
        result.is_ok(),
        "empty diff should not initialize pager: {result:?}"
    );
}

#[test]
fn test_diff_name_only_and_name_status_flags_render_cli_output() {
    let repo = create_committed_repo_via_cli();
    fs::write(repo.path().join("tracked.txt"), "tracked\nupdated\n").unwrap();

    let name_only = run_libra_command(&["diff", "--name-only"], repo.path());
    assert_cli_success(&name_only, "diff --name-only");
    assert_eq!(
        String::from_utf8_lossy(&name_only.stdout).trim(),
        "tracked.txt"
    );

    let name_status = run_libra_command(&["diff", "--name-status"], repo.path());
    assert_cli_success(&name_status, "diff --name-status");
    assert_eq!(
        String::from_utf8_lossy(&name_status.stdout).trim(),
        "M\ttracked.txt"
    );
}

#[test]
fn test_diff_numstat_and_stat_flags_render_cli_output() {
    let repo = create_committed_repo_via_cli();
    fs::write(repo.path().join("tracked.txt"), "tracked\nupdated\n").unwrap();

    let numstat = run_libra_command(&["diff", "--numstat"], repo.path());
    assert_cli_success(&numstat, "diff --numstat");
    assert_eq!(
        String::from_utf8_lossy(&numstat.stdout).trim(),
        "1\t0\ttracked.txt"
    );

    let stat = run_libra_command(&["diff", "--stat"], repo.path());
    assert_cli_success(&stat, "diff --stat");
    let stat_stdout = String::from_utf8_lossy(&stat.stdout);
    assert!(
        stat_stdout.contains("tracked.txt | 1 +"),
        "expected per-file stat line, got: {stat_stdout}"
    );
    assert!(
        stat_stdout.contains("1 file changed, 1 insertion(+), 0 deletions(-)"),
        "expected stat summary, got: {stat_stdout}"
    );
}

#[test]
fn test_diff_quiet_uses_exit_code_to_signal_changes() {
    let repo = create_committed_repo_via_cli();
    fs::write(repo.path().join("tracked.txt"), "tracked\nupdated\n").unwrap();

    let output = run_libra_command(&["--quiet", "diff"], repo.path());
    assert_eq!(output.status.code(), Some(1));
    assert!(
        output.stdout.is_empty(),
        "unexpected stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        output.stderr.is_empty(),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_diff_quiet_with_output_file_still_returns_exit_code_1() {
    let repo = create_committed_repo_via_cli();
    fs::write(repo.path().join("tracked.txt"), "tracked\nupdated\n").unwrap();
    let output_file = repo.path().join("captured.diff");
    let output_path = output_file.to_str().unwrap();

    let output = run_libra_command(&["--quiet", "diff", "--output", output_path], repo.path());
    assert_eq!(output.status.code(), Some(1));
    assert!(
        output.stderr.is_empty(),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let written = fs::read_to_string(&output_file).unwrap();
    assert!(
        written.contains("diff --git"),
        "expected diff output file to be written, got: {written}"
    );
}

#[test]
fn test_diff_json_ignores_output_file_flag() {
    let repo = create_committed_repo_via_cli();
    fs::write(repo.path().join("tracked.txt"), "tracked\nupdated\n").unwrap();
    let output_file = repo.path().join("ignored.diff");
    let output_path = output_file.to_str().unwrap();

    let output = run_libra_command(&["--json", "diff", "--output", output_path], repo.path());
    assert_cli_success(&output, "json diff with output flag");
    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "diff");
    assert!(
        !output_file.exists(),
        "--output should be ignored in JSON mode, but {:?} was created",
        output_file
    );
}

#[test]
fn test_diff_status_detection_ignores_patch_body_text() {
    let repo = create_committed_repo_via_cli();
    fs::write(
        repo.path().join("tracked.txt"),
        "tracked\nnew file mode 100644\ndeleted file mode 100644\n",
    )
    .unwrap();

    let name_status = run_libra_command(&["diff", "--name-status"], repo.path());
    assert_cli_success(&name_status, "diff --name-status");
    assert_eq!(
        String::from_utf8_lossy(&name_status.stdout).trim(),
        "M\ttracked.txt"
    );

    let json = run_libra_command(&["--json", "diff"], repo.path());
    assert_cli_success(&json, "diff --json");
    let json = parse_json_stdout(&json);
    assert_eq!(json["data"]["files"][0]["path"], "tracked.txt");
    assert_eq!(json["data"]["files"][0]["status"], "modified");
}

#[test]
fn test_diff_stats_count_hunk_lines_that_start_with_header_prefixes() {
    let repo = create_committed_repo_via_cli();
    fs::write(repo.path().join("tracked.txt"), "tracked\n---gone\n").unwrap();
    let add_output = run_libra_command(&["add", "tracked.txt"], repo.path());
    assert_cli_success(&add_output, "add tracked.txt");
    let commit_output =
        run_libra_command(&["commit", "-m", "seed header-like content"], repo.path());
    assert_cli_success(&commit_output, "commit seed header-like content");

    fs::write(repo.path().join("tracked.txt"), "+++added\n").unwrap();

    let numstat = run_libra_command(&["diff", "--numstat"], repo.path());
    assert_cli_success(&numstat, "diff --numstat");
    assert_eq!(
        String::from_utf8_lossy(&numstat.stdout).trim(),
        "1\t2\ttracked.txt"
    );

    let stat = run_libra_command(&["diff", "--stat"], repo.path());
    assert_cli_success(&stat, "diff --stat");
    let stat_stdout = String::from_utf8_lossy(&stat.stdout);
    assert!(
        stat_stdout.contains("tracked.txt | 3 +--"),
        "expected stat output to count header-like hunk lines, got: {stat_stdout}"
    );
    assert!(
        stat_stdout.contains("1 file changed, 1 insertion(+), 2 deletions(-)"),
        "expected stat summary to count header-like hunk lines, got: {stat_stdout}"
    );

    let json = run_libra_command(&["--json", "diff"], repo.path());
    assert_cli_success(&json, "diff --json");
    let json = parse_json_stdout(&json);
    assert_eq!(json["data"]["files"][0]["insertions"], 1);
    assert_eq!(json["data"]["files"][0]["deletions"], 2);
    assert_eq!(json["data"]["total_insertions"], 1);
    assert_eq!(json["data"]["total_deletions"], 2);
}

#[test]
fn test_diff_added_and_deleted_files_use_dev_null_headers() {
    let repo = create_committed_repo_via_cli();

    fs::write(repo.path().join("added.txt"), "added\n").unwrap();
    let added = run_libra_command(&["diff"], repo.path());
    assert_cli_success(&added, "diff added file");
    let added_stdout = String::from_utf8_lossy(&added.stdout);
    assert!(
        added_stdout.contains("--- /dev/null"),
        "expected added file diff to use /dev/null old header, got: {added_stdout}"
    );
    assert!(
        added_stdout.contains("+++ b/added.txt"),
        "expected added file diff to use b/ path in new header, got: {added_stdout}"
    );

    fs::remove_file(repo.path().join("tracked.txt")).unwrap();
    let deleted = run_libra_command(&["diff"], repo.path());
    assert_cli_success(&deleted, "diff deleted file");
    let deleted_stdout = String::from_utf8_lossy(&deleted.stdout);
    assert!(
        deleted_stdout.contains("--- a/tracked.txt"),
        "expected deleted file diff to use a/ path in old header, got: {deleted_stdout}"
    );
    assert!(
        deleted_stdout.contains("+++ /dev/null"),
        "expected deleted file diff to use /dev/null new header, got: {deleted_stdout}"
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

    let content = fs::read_to_string(&output_file).unwrap_or_default();
    assert!(
        content.contains("diff --git a/.libraignore b/.libraignore"),
        "Expected init-created .libraignore to be visible in diff, got: {content}"
    );
    assert!(
        content.contains("# Libra ignore file"),
        "Expected default .libraignore contents in diff, got: {content}"
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
        pathspec_from_file: None,
        pathspec_file_nul: false,
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
        ..Default::default()
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
        pathspec_from_file: None,
        pathspec_file_nul: false,
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
        ..Default::default()
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
        pathspec_from_file: None,
        pathspec_file_nul: false,
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
        pathspec_from_file: None,
        pathspec_file_nul: false,
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
        pathspec_from_file: None,
        pathspec_file_nul: false,
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
        ..Default::default()
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
        pathspec_from_file: None,
        pathspec_file_nul: false,
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
        pathspec_from_file: None,
        pathspec_file_nul: false,
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
        pathspec_from_file: None,
        pathspec_file_nul: false,
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

    // Non-default algorithms are accepted by clap for forward
    // compatibility but fail closed until the backend is actually wired.
    let myers_file = output_dir.path().join("myers_diff.txt");
    let myers_str = myers_file.to_str().unwrap();
    let args = DiffArgs::parse_from(["diff", "--algorithm", "myers", "--output", myers_str]);
    let myers_result = diff::execute_safe(args, &OutputConfig::default()).await;

    let myers_min_file = output_dir.path().join("myersMinimal_diff.txt");
    let myers_min_str = myers_min_file.to_str().unwrap();
    let args = DiffArgs::parse_from([
        "diff",
        "--algorithm",
        "myersMinimal",
        "--output",
        myers_min_str,
    ]);
    let myers_min_result = diff::execute_safe(args, &OutputConfig::default()).await;

    assert!(
        fs::metadata(&histogram_file).is_ok(),
        "Histogram output file should exist"
    );
    assert!(
        myers_result.is_err(),
        "Myers should fail closed until a real backend is wired"
    );
    assert!(
        myers_min_result.is_err(),
        "MyersMinimal should fail closed until a real backend is wired"
    );
    assert!(
        !myers_file.exists(),
        "unsupported Myers should not write a default diff to the output file"
    );
    assert!(
        !myers_min_file.exists(),
        "unsupported MyersMinimal should not write a default diff to the output file"
    );
}

#[test]
fn test_diff_summary_lists_creates_and_deletes() {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();
    // Commit a file we will later delete.
    std::fs::write(p.join("old.txt"), "old\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "old.txt"], p), "add old.txt");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "seed", "--no-verify"], p),
        "commit seed",
    );

    // Stage a created file and a deletion.
    std::fs::write(p.join("new.txt"), "new\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "new.txt"], p), "add new.txt");
    std::fs::remove_file(p.join("old.txt")).unwrap();
    assert_cli_success(&run_libra_command(&["add", "old.txt"], p), "stage deletion");

    let out = run_libra_command(&["diff", "--cached", "--summary"], p);
    assert_cli_success(&out, "diff --cached --summary");
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.lines().any(|l| l == " create mode 100644 new.txt"),
        "summary lists the created file in git's format: {s:?}"
    );
    assert!(
        s.lines().any(|l| l == " delete mode 100644 old.txt"),
        "summary lists the deleted file in git's format: {s:?}"
    );
}

#[test]
fn test_diff_shortstat_exit_code_and_no_patch() {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();
    fs::write(p.join("tracked.txt"), "tracked\nupdated line\n").unwrap();

    // --shortstat: just the trailing summary line (no per-file rows).
    let ss = run_libra_command(&["diff", "--shortstat"], p);
    assert!(ss.status.success(), "shortstat exits 0 without --exit-code");
    let s = String::from_utf8_lossy(&ss.stdout);
    assert!(s.contains("1 file changed"), "shortstat summary: {s:?}");
    assert!(
        !s.contains(" | "),
        "shortstat omits the per-file rows: {s:?}"
    );
    assert_eq!(
        s.lines().filter(|l| !l.trim().is_empty()).count(),
        1,
        "shortstat is a single line: {s:?}"
    );

    // --exit-code: still prints the diff, but exits 1 when there are changes.
    let ec = run_libra_command(&["diff", "--exit-code"], p);
    assert_eq!(ec.status.code(), Some(1), "exit-code is 1 when changed");
    assert!(
        !String::from_utf8_lossy(&ec.stdout).trim().is_empty(),
        "--exit-code still prints the diff body"
    );

    // -s / --no-patch: suppress the body; exit 0 without --exit-code.
    let no_patch = run_libra_command(&["diff", "-s"], p);
    assert!(no_patch.status.success(), "--no-patch exits 0 on its own");
    assert!(
        String::from_utf8_lossy(&no_patch.stdout).trim().is_empty(),
        "--no-patch suppresses the diff body"
    );

    // -s --exit-code: no body, exit 1.
    let both = run_libra_command(&["diff", "-s", "--exit-code"], p);
    assert_eq!(both.status.code(), Some(1), "--no-patch + --exit-code = 1");
    assert!(
        String::from_utf8_lossy(&both.stdout).trim().is_empty(),
        "--no-patch still suppresses the body with --exit-code"
    );

    // --exit-code applies in JSON mode too: still emit JSON, but exit 1.
    let json = run_libra_command(&["--json", "diff", "--exit-code"], p);
    assert_eq!(
        json.status.code(),
        Some(1),
        "--json --exit-code exits 1 on changes"
    );
    assert!(
        String::from_utf8_lossy(&json.stdout).contains("\"files_changed\""),
        "--json --exit-code still emits the JSON payload"
    );
}

#[test]
fn test_diff_z_nul_terminates_name_outputs() {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();
    fs::write(p.join("tracked.txt"), "tracked\nchanged\n").unwrap();

    // --name-only -z: each path NUL-terminated, no trailing newline.
    let no = run_libra_command(&["diff", "--name-only", "-z"], p);
    assert!(no.status.success(), "name-only -z ok");
    assert_eq!(
        no.stdout, b"tracked.txt\0",
        "name-only -z framing: {:?}",
        no.stdout
    );

    // --name-status -z: status and path as separate NUL fields.
    let ns = run_libra_command(&["diff", "--name-status", "-z"], p);
    assert!(ns.status.success(), "name-status -z ok");
    assert_eq!(
        ns.stdout, b"M\0tracked.txt\0",
        "name-status -z framing: {:?}",
        ns.stdout
    );

    // Without -z, the same query is newline-terminated (sanity check).
    let plain = run_libra_command(&["diff", "--name-only"], p);
    assert_eq!(
        plain.stdout, b"tracked.txt\n",
        "name-only plain framing: {:?}",
        plain.stdout
    );
}

#[test]
fn test_diff_check_reports_whitespace_errors() {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();
    std::fs::write(p.join("ws.txt"), "clean\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "ws.txt"], p), "add base");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "base", "--no-verify"], p),
        "commit base",
    );

    // Stage a change with trailing whitespace (line 2) and space-before-tab (line 3).
    std::fs::write(p.join("ws.txt"), "clean\ntrailing   \n \tindent\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "ws.txt"], p), "stage ws");

    let check = run_libra_command(&["diff", "--cached", "--check"], p);
    assert_eq!(
        check.status.code(),
        Some(2),
        "diff --check exits 2 when problems are found"
    );
    let out = String::from_utf8_lossy(&check.stdout);
    assert!(
        out.contains("ws.txt:2: trailing whitespace"),
        "trailing ws: {out:?}"
    );
    assert!(
        out.contains("ws.txt:3: space before tab in indent"),
        "space-before-tab: {out:?}"
    );

    // A clean staged change reports nothing and exits 0.
    std::fs::write(p.join("ws.txt"), "clean\ntidy line\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "ws.txt"], p), "stage tidy");
    let clean = run_libra_command(&["diff", "--cached", "--check"], p);
    assert_cli_success(&clean, "diff --check (clean) exits 0");
    assert!(
        String::from_utf8_lossy(&clean.stdout).trim().is_empty(),
        "no warnings for a clean diff"
    );
}

#[test]
fn test_diff_reverse_swaps_sides() {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();
    std::fs::write(p.join("r.txt"), "line1\nline2\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "r.txt"], p), "add base");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "base", "--no-verify"], p),
        "commit base",
    );
    std::fs::write(p.join("r.txt"), "line1\nCHANGED\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "r.txt"], p), "stage change");

    // Normal staged diff: line2 removed, CHANGED added.
    let normal = run_libra_command(&["diff", "--cached"], p);
    assert_cli_success(&normal, "diff --cached");
    let n = String::from_utf8_lossy(&normal.stdout);
    assert!(n.contains("-line2"), "normal removes line2: {n:?}");
    assert!(n.contains("+CHANGED"), "normal adds CHANGED: {n:?}");

    // Reverse: the sides swap, so CHANGED is removed and line2 is added.
    let reverse = run_libra_command(&["diff", "--cached", "-R"], p);
    assert_cli_success(&reverse, "diff --cached -R");
    let r = String::from_utf8_lossy(&reverse.stdout);
    assert!(r.contains("-CHANGED"), "reverse removes CHANGED: {r:?}");
    assert!(r.contains("+line2"), "reverse adds line2: {r:?}");
}

#[test]
fn diff_text_flag_is_accepted_noop() {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();
    // Stage a change including a NUL byte (Git would call this "binary").
    std::fs::write(p.join("data.bin"), b"line\x00\x01\x02\n").unwrap();
    assert_cli_success(
        &run_libra_command(&["add", "data.bin"], p),
        "stage data.bin",
    );

    let plain = run_libra_command(&["diff", "--cached"], p);
    assert_cli_success(&plain, "diff --cached");
    let plain_out = String::from_utf8_lossy(&plain.stdout);

    // `--text` and its short `-a` are accepted and produce identical output:
    // Libra's diff never detects binary files, so it already shows content.
    for flag in ["--text", "-a"] {
        let out = run_libra_command(&["diff", "--cached", flag], p);
        assert_cli_success(&out, &format!("diff --cached {flag}"));
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            plain_out,
            "diff --cached {flag} matches plain diff (no-op)"
        );
    }
}

#[test]
fn diff_no_ext_diff_flag_is_accepted_noop() {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();
    std::fs::write(p.join("e.txt"), "x\ny\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "e.txt"], p), "stage e.txt");

    let plain = run_libra_command(&["diff", "--cached"], p);
    assert_cli_success(&plain, "diff --cached");
    // `--no-ext-diff` is accepted and produces identical output: Libra has no
    // external diff drivers, so it always uses the built-in diff engine.
    let out = run_libra_command(&["diff", "--cached", "--no-ext-diff"], p);
    assert_cli_success(&out, "diff --cached --no-ext-diff");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&plain.stdout),
        "diff --no-ext-diff matches plain diff (no-op)"
    );
}

#[test]
fn diff_no_color_moved_flag_is_accepted_noop() {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();
    std::fs::write(p.join("m.txt"), "a\nb\nc\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "m.txt"], p), "stage m.txt");

    let plain = run_libra_command(&["diff", "--cached"], p);
    assert_cli_success(&plain, "diff --cached");
    // `--no-color-moved` is accepted and a no-op: Libra's diff never colors
    // moved lines, so the output is identical.
    let out = run_libra_command(&["diff", "--cached", "--no-color-moved"], p);
    assert_cli_success(&out, "diff --cached --no-color-moved");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&plain.stdout),
        "diff --no-color-moved matches plain diff (no-op)"
    );
}
