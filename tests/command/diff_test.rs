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
        ..Default::default()
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
        ..Default::default()
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
        ..Default::default()
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
        ..Default::default()
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
        ..Default::default()
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
        ..Default::default()
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
        ..Default::default()
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
        ..Default::default()
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

// ----- Wave 0: whitespace ignore, -U context, --exit-code -----

/// Commit `content` to `tracked.txt` so a later worktree edit diffs against it.
fn commit_tracked(repo: &tempfile::TempDir, content: &str) {
    fs::write(repo.path().join("tracked.txt"), content).unwrap();
    assert_cli_success(
        &run_libra_command(&["add", "tracked.txt"], repo.path()),
        "stage tracked base",
    );
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "base", "--no-verify"], repo.path()),
        "commit tracked base",
    );
}

#[test]
fn diff_ignore_all_space_suppresses_whitespace_only_changes() {
    let repo = create_committed_repo_via_cli();
    // Base tracked.txt is "tracked\n"; the worktree only adds surrounding spaces.
    fs::write(repo.path().join("tracked.txt"), "   tracked   \n").unwrap();
    let output = run_libra_command(&["--json", "diff", "-w"], repo.path());
    assert_cli_success(&output, "diff -w");
    let json = parse_json_stdout(&output);
    assert_eq!(
        json["data"]["files_changed"], 0,
        "whitespace-only change must be ignored by -w"
    );
}

#[test]
fn diff_ignore_space_change_collapses_runs() {
    let repo = create_committed_repo_via_cli();
    commit_tracked(&repo, "a b c\n");
    fs::write(repo.path().join("tracked.txt"), "a    b    c\n").unwrap();
    let output = run_libra_command(&["--json", "diff", "-b"], repo.path());
    assert_cli_success(&output, "diff -b");
    let json = parse_json_stdout(&output);
    assert_eq!(
        json["data"]["files_changed"], 0,
        "whitespace-run-only change must be ignored by -b"
    );
}

#[test]
fn diff_ignore_blank_lines_skips_blank_only_hunks() {
    let repo = create_committed_repo_via_cli();
    commit_tracked(&repo, "alpha\nbeta\n");
    fs::write(repo.path().join("tracked.txt"), "alpha\n\nbeta\n").unwrap();
    let output = run_libra_command(&["--json", "diff", "--ignore-blank-lines"], repo.path());
    assert_cli_success(&output, "diff --ignore-blank-lines");
    let json = parse_json_stdout(&output);
    assert_eq!(
        json["data"]["files_changed"], 0,
        "blank-only insertion must be ignored"
    );
}

#[test]
fn diff_unified_context_default_is_three() {
    let repo = create_committed_repo_via_cli();
    commit_tracked(&repo, "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\n");
    fs::write(
        repo.path().join("tracked.txt"),
        "l1\nl2\nl3\nl4\nL5\nl6\nl7\nl8\nl9\n",
    )
    .unwrap();
    let output = run_libra_command(&["diff"], repo.path());
    assert_cli_success(&output, "default diff");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("@@ -2,7 +2,7 @@"),
        "default context should be 3: {stdout}"
    );
}

#[test]
fn diff_context_config_sets_default() {
    let repo = create_committed_repo_via_cli();
    commit_tracked(&repo, "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\n");
    fs::write(
        repo.path().join("tracked.txt"),
        "l1\nl2\nl3\nl4\nL5\nl6\nl7\nl8\nl9\n",
    )
    .unwrap();
    assert_cli_success(
        &run_libra_command(&["config", "set", "diff.context", "5"], repo.path()),
        "set diff.context",
    );
    let output = run_libra_command(&["diff"], repo.path());
    assert_cli_success(&output, "diff with diff.context=5");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("@@ -1,9 +1,9 @@"),
        "diff.context=5 should widen context: {stdout}"
    );

    // Explicit -U overrides the config value.
    let output = run_libra_command(&["diff", "-U2"], repo.path());
    assert_cli_success(&output, "diff -U2");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("@@ -3,5 +3,5 @@"),
        "-U2 should override diff.context: {stdout}"
    );
}

#[test]
fn diff_context_config_invalid_value_errors() {
    let repo = create_committed_repo_via_cli();
    fs::write(repo.path().join("tracked.txt"), "tracked\nmore\n").unwrap();
    assert_cli_success(
        &run_libra_command(
            &["config", "set", "diff.context", "notanumber"],
            repo.path(),
        ),
        "set invalid diff.context",
    );
    let output = run_libra_command(&["diff"], repo.path());
    assert_eq!(
        output.status.code(),
        Some(129),
        "invalid diff.context should be a usage error: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let (_stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-002");
}

#[test]
fn diff_unified_zero_context_emits_only_changed_lines() {
    let repo = create_committed_repo_via_cli();
    commit_tracked(&repo, "l1\nl2\nl3\nl4\nl5\n");
    fs::write(repo.path().join("tracked.txt"), "l1\nl2\nL3\nl4\nl5\n").unwrap();
    let output = run_libra_command(&["diff", "-U0"], repo.path());
    assert_cli_success(&output, "diff -U0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("@@ -3,1 +3,1 @@"),
        "zero-context header: {stdout}"
    );
    let context_lines = stdout
        .lines()
        .filter(|l| l.starts_with(' ') && !l.starts_with("@@"))
        .count();
    assert_eq!(context_lines, 0, "no context lines at -U0: {stdout}");
}

#[test]
fn diff_exit_code_flag_returns_1_when_changes() {
    let repo = create_committed_repo_via_cli();
    fs::write(repo.path().join("tracked.txt"), "tracked\nmore\n").unwrap();
    let output = run_libra_command(&["diff", "--exit-code"], repo.path());
    assert_eq!(
        output.status.code(),
        Some(1),
        "exit-code with changes should be 1: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.stdout.is_empty(),
        "--exit-code still renders the diff body"
    );
}

#[test]
fn diff_exit_code_flag_returns_0_when_no_changes() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["diff", "--exit-code"], repo.path());
    assert_eq!(
        output.status.code(),
        Some(0),
        "exit-code without changes should be 0"
    );
}

#[test]
fn diff_json_exit_code_returns_1_when_changes() {
    let repo = create_committed_repo_via_cli();
    fs::write(repo.path().join("tracked.txt"), "tracked\nmore\n").unwrap();
    let output = run_libra_command(&["--json", "diff", "--exit-code"], repo.path());
    assert_eq!(
        output.status.code(),
        Some(1),
        "json --exit-code with changes should be 1"
    );
    let json = parse_json_stdout(&output);
    assert_eq!(
        json["data"]["files_changed"], 1,
        "JSON envelope is still emitted under --exit-code"
    );
}

#[test]
fn diff_output_file_exit_code_returns_1_when_changes() {
    let repo = create_committed_repo_via_cli();
    fs::write(repo.path().join("tracked.txt"), "tracked\nmore\n").unwrap();
    let out_file = repo.path().join("diff.out");
    let output = run_libra_command(
        &[
            "diff",
            "--exit-code",
            "--output",
            out_file.to_str().unwrap(),
        ],
        repo.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(1),
        "--exit-code --output with changes should be 1"
    );
    assert!(
        out_file.exists(),
        "diff body was written to the output file"
    );
}

#[test]
fn diff_quiet_returns_1_when_changes_no_stdout() {
    let repo = create_committed_repo_via_cli();
    fs::write(repo.path().join("tracked.txt"), "tracked\nmore\n").unwrap();
    let output = run_libra_command(&["diff", "-q"], repo.path());
    assert_eq!(
        output.status.code(),
        Some(1),
        "quiet with changes should be 1"
    );
    assert!(output.stdout.is_empty(), "quiet suppresses stdout");
}

#[test]
fn diff_default_returns_0_even_with_changes() {
    let repo = create_committed_repo_via_cli();
    fs::write(repo.path().join("tracked.txt"), "tracked\nmore\n").unwrap();
    let output = run_libra_command(&["diff"], repo.path());
    assert_eq!(
        output.status.code(),
        Some(0),
        "default diff exits 0 even with changes"
    );
}

#[test]
fn diff_binary_change_emits_binary_files_differ() {
    let repo = create_committed_repo_via_cli();
    fs::write(repo.path().join("tracked.txt"), [0u8, 159, 146, 150]).unwrap();
    let output = run_libra_command(&["diff", "--exit-code"], repo.path());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Binary files differ"),
        "binary change should report Binary files differ: {stdout}"
    );
    assert_eq!(
        output.status.code(),
        Some(1),
        "--exit-code still triggers on a binary change"
    );
}

#[test]
fn diff_preprocess_does_not_mutate_blob() {
    let repo = create_committed_repo_via_cli();
    let content = "   tracked   \n";
    fs::write(repo.path().join("tracked.txt"), content).unwrap();
    let output = run_libra_command(&["diff", "-w"], repo.path());
    assert_cli_success(&output, "diff -w");
    // The worktree file must be byte-for-byte unchanged after a -w comparison.
    let after = fs::read_to_string(repo.path().join("tracked.txt")).unwrap();
    assert_eq!(after, content, "-w preprocessing must not rewrite the file");
}

#[test]
fn diff_myers_still_fail_closed_lbr_cli_002() {
    let repo = create_committed_repo_via_cli();
    fs::write(repo.path().join("tracked.txt"), "tracked\nmore\n").unwrap();
    let output = run_libra_command(&["diff", "--algorithm", "myers"], repo.path());
    assert_eq!(
        output.status.code(),
        Some(129),
        "--algorithm=myers must remain fail-closed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let (_stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-002");
}

// ----- Wave 1: rename / copy detection -----

/// Commit a fresh file so a later rename diffs against it.
fn commit_file(repo: &tempfile::TempDir, name: &str, content: &str) {
    fs::write(repo.path().join(name), content).unwrap();
    assert_cli_success(
        &run_libra_command(&["add", name], repo.path()),
        "stage new file",
    );
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "add file", "--no-verify"], repo.path()),
        "commit new file",
    );
}

fn renamed_entry(json: &serde_json::Value) -> Option<&serde_json::Value> {
    json["data"]["files"]
        .as_array()?
        .iter()
        .find(|f| f["status"] == "renamed")
}

#[test]
fn diff_rename_detected_with_small_edit() {
    let repo = create_committed_repo_via_cli();
    commit_file(&repo, "orig.txt", "l1\nl2\nl3\nl4\n");
    fs::remove_file(repo.path().join("orig.txt")).unwrap();
    fs::write(repo.path().join("new.txt"), "l1\nl2\nl3\nCHANGED\n").unwrap();

    let output = run_libra_command(&["--json", "diff", "-M"], repo.path());
    assert_cli_success(&output, "diff -M");
    let json = parse_json_stdout(&output);
    let renamed = renamed_entry(&json).expect("a renamed entry");
    assert_eq!(renamed["path"], "new.txt");
    assert_eq!(renamed["old_path"], "orig.txt");
}

#[test]
fn diff_exact_rename_uses_hash_fast_path() {
    let repo = create_committed_repo_via_cli();
    commit_file(&repo, "orig.txt", "alpha\nbeta\ngamma\n");
    fs::remove_file(repo.path().join("orig.txt")).unwrap();
    fs::write(repo.path().join("new.txt"), "alpha\nbeta\ngamma\n").unwrap();

    let output = run_libra_command(&["--json", "diff", "-M"], repo.path());
    assert_cli_success(&output, "diff -M exact");
    let json = parse_json_stdout(&output);
    let renamed = renamed_entry(&json).expect("a renamed entry");
    assert_eq!(renamed["similarity"], 100);
    assert_eq!(
        renamed["hunks"].as_array().map(|h| h.len()),
        Some(0),
        "an exact rename has no hunk body"
    );

    // Human output carries the similarity header but no @@ hunk.
    let human = run_libra_command(&["diff", "-M"], repo.path());
    let stdout = String::from_utf8_lossy(&human.stdout);
    assert!(stdout.contains("similarity index 100%"), "stdout={stdout}");
    assert!(stdout.contains("rename from orig.txt"), "stdout={stdout}");
    assert!(stdout.contains("rename to new.txt"), "stdout={stdout}");
    assert!(!stdout.contains("@@"), "pure rename has no hunk: {stdout}");
}

#[test]
fn diff_no_renames_forces_add_delete() {
    let repo = create_committed_repo_via_cli();
    commit_file(&repo, "orig.txt", "alpha\nbeta\ngamma\n");
    fs::remove_file(repo.path().join("orig.txt")).unwrap();
    fs::write(repo.path().join("new.txt"), "alpha\nbeta\ngamma\n").unwrap();

    let output = run_libra_command(&["--json", "diff", "--no-renames"], repo.path());
    assert_cli_success(&output, "diff --no-renames");
    let json = parse_json_stdout(&output);
    let statuses: Vec<String> = json["data"]["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["status"].as_str().unwrap().to_string())
        .collect();
    assert!(statuses.contains(&"added".to_string()), "{statuses:?}");
    assert!(statuses.contains(&"deleted".to_string()), "{statuses:?}");
    assert!(!statuses.contains(&"renamed".to_string()), "{statuses:?}");
}

#[test]
fn diff_default_without_flag_does_not_detect_rename() {
    let repo = create_committed_repo_via_cli();
    commit_file(&repo, "orig.txt", "alpha\nbeta\ngamma\n");
    fs::remove_file(repo.path().join("orig.txt")).unwrap();
    fs::write(repo.path().join("new.txt"), "alpha\nbeta\ngamma\n").unwrap();

    // No -M and no config => libra keeps the add/delete pair (off by default).
    let output = run_libra_command(&["--json", "diff"], repo.path());
    assert_cli_success(&output, "diff default");
    let json = parse_json_stdout(&output);
    assert!(
        renamed_entry(&json).is_none(),
        "default diff must not rename"
    );
}

#[test]
fn diff_rename_threshold_percent_respected() {
    let repo = create_committed_repo_via_cli();
    // 3 of 6 lines change => ~50% similar; -M80% must NOT pair them.
    commit_file(&repo, "orig.txt", "l1\nl2\nl3\nl4\nl5\nl6\n");
    fs::remove_file(repo.path().join("orig.txt")).unwrap();
    fs::write(repo.path().join("new.txt"), "l1\nl2\nl3\nX4\nX5\nX6\n").unwrap();

    let output = run_libra_command(&["--json", "diff", "--find-renames=80%"], repo.path());
    assert_cli_success(&output, "diff -M80%");
    let json = parse_json_stdout(&output);
    assert!(
        renamed_entry(&json).is_none(),
        "50%-similar files must not pair at -M80%"
    );
}

#[test]
fn diff_rename_threshold_bare_number_means_percent() {
    let repo = create_committed_repo_via_cli();
    commit_file(&repo, "orig.txt", "l1\nl2\nl3\nl4\n");
    fs::remove_file(repo.path().join("orig.txt")).unwrap();
    fs::write(repo.path().join("new.txt"), "l1\nl2\nl3\nCHANGED\n").unwrap();

    // 75% similar: -M80 (== -M80%) rejects, -M70 accepts.
    let strict = run_libra_command(&["--json", "diff", "--find-renames=80"], repo.path());
    assert!(renamed_entry(&parse_json_stdout(&strict)).is_none());
    let loose = run_libra_command(&["--json", "diff", "--find-renames=70"], repo.path());
    assert!(renamed_entry(&parse_json_stdout(&loose)).is_some());
}

#[test]
fn diff_config_renames_false_disables() {
    let repo = create_committed_repo_via_cli();
    commit_file(&repo, "orig.txt", "alpha\nbeta\ngamma\n");
    fs::remove_file(repo.path().join("orig.txt")).unwrap();
    fs::write(repo.path().join("new.txt"), "alpha\nbeta\ngamma\n").unwrap();
    assert_cli_success(
        &run_libra_command(&["config", "set", "diff.renames", "false"], repo.path()),
        "set diff.renames=false",
    );

    let output = run_libra_command(&["--json", "diff", "-M"], repo.path());
    assert_cli_success(&output, "diff -M with renames disabled");
    // Explicit -M overrides config off.
    assert!(
        renamed_entry(&parse_json_stdout(&output)).is_some(),
        "explicit -M should still detect renames"
    );

    // Config true enables without a flag.
    assert_cli_success(
        &run_libra_command(&["config", "set", "diff.renames", "true"], repo.path()),
        "set diff.renames=true",
    );
    let output = run_libra_command(&["--json", "diff"], repo.path());
    assert!(
        renamed_entry(&parse_json_stdout(&output)).is_some(),
        "diff.renames=true should enable detection without a flag"
    );
}

#[test]
fn diff_copy_detection_basic() {
    let repo = create_committed_repo_via_cli();
    commit_file(&repo, "src.txt", "one\ntwo\nthree\nfour\n");
    // Modify the source and add a near-identical copy.
    fs::write(repo.path().join("src.txt"), "one\ntwo\nthree\nFOUR\n").unwrap();
    fs::write(repo.path().join("copy.txt"), "one\ntwo\nthree\nfour\n").unwrap();

    let output = run_libra_command(&["--json", "diff", "-C"], repo.path());
    assert_cli_success(&output, "diff -C");
    let json = parse_json_stdout(&output);
    let copied = json["data"]["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["status"] == "copied")
        .expect("a copied entry");
    assert_eq!(copied["path"], "copy.txt");
    assert_eq!(copied["old_path"], "src.txt");
    // The copy source is NOT consumed: src.txt still shows as modified.
    assert!(
        json["data"]["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["path"] == "src.txt" && f["status"] == "modified"),
        "copy source remains independently in the diff"
    );
}

#[test]
fn diff_rename_limit_cutoff_warns() {
    let repo = create_committed_repo_via_cli();
    commit_file(&repo, "a.txt", "alpha\nbeta\ngamma\ndelta\n");
    commit_file(&repo, "b.txt", "uno\ndos\ntres\ncuatro\n");
    // Delete both and add two near-but-not-exact copies (forces inexact stage).
    fs::remove_file(repo.path().join("a.txt")).unwrap();
    fs::remove_file(repo.path().join("b.txt")).unwrap();
    fs::write(repo.path().join("a2.txt"), "alpha\nbeta\ngamma\nDELTA\n").unwrap();
    fs::write(repo.path().join("b2.txt"), "uno\ndos\ntres\nCUATRO\n").unwrap();
    assert_cli_success(
        &run_libra_command(&["config", "set", "diff.renameLimit", "1"], repo.path()),
        "set diff.renameLimit=1",
    );

    let output = run_libra_command(&["diff", "-M"], repo.path());
    assert_cli_success(&output, "diff -M over rename limit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("rename detection was skipped"),
        "expected a rename-limit warning: {stderr}"
    );
}

#[test]
fn diff_binary_rename_by_hash() {
    let repo = create_committed_repo_via_cli();
    fs::write(repo.path().join("orig.bin"), [0u8, 1, 2, 3, 0, 9]).unwrap();
    assert_cli_success(
        &run_libra_command(&["add", "orig.bin"], repo.path()),
        "stage binary",
    );
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "bin", "--no-verify"], repo.path()),
        "commit binary",
    );
    fs::remove_file(repo.path().join("orig.bin")).unwrap();
    fs::write(repo.path().join("moved.bin"), [0u8, 1, 2, 3, 0, 9]).unwrap();

    let output = run_libra_command(&["--json", "diff", "-M"], repo.path());
    assert_cli_success(&output, "diff -M binary");
    let json = parse_json_stdout(&output);
    let renamed = renamed_entry(&json).expect("binary rename by hash");
    assert_eq!(renamed["path"], "moved.bin");
    assert_eq!(renamed["similarity"], 100);
}

#[test]
fn diff_native_path_treats_nul_byte_content_as_binary() {
    let repo = create_committed_repo_via_cli();
    // A NUL byte is valid UTF-8 but Git treats such content as binary.
    commit_file(&repo, "data.bin", "a\u{0}b\nx\ny\n");
    fs::write(repo.path().join("data.bin"), "a\u{0}c\nx\ny\n").unwrap();
    // `-w` forces the native generator, which must detect the NUL byte.
    let output = run_libra_command(&["diff", "-w"], repo.path());
    assert_cli_success(&output, "diff -w nul");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Binary files differ"),
        "NUL-byte content must be treated as binary: {stdout:?}"
    );
}

#[test]
fn diff_copy_source_can_be_a_renamed_delete() {
    let repo = create_committed_repo_via_cli();
    commit_file(&repo, "a.txt", "one\ntwo\nthree\nfour\n");
    // a.txt is deleted and reappears as b.txt (rename) and c.txt (copy).
    fs::remove_file(repo.path().join("a.txt")).unwrap();
    fs::write(repo.path().join("b.txt"), "one\ntwo\nthree\nfour\n").unwrap();
    fs::write(repo.path().join("c.txt"), "one\ntwo\nthree\nfour\n").unwrap();

    let output = run_libra_command(&["--json", "diff", "-M", "-C"], repo.path());
    assert_cli_success(&output, "diff -M -C");
    let json = parse_json_stdout(&output);
    let files = json["data"]["files"].as_array().unwrap();
    let renamed: Vec<_> = files.iter().filter(|f| f["status"] == "renamed").collect();
    let copied: Vec<_> = files.iter().filter(|f| f["status"] == "copied").collect();
    assert_eq!(renamed.len(), 1, "exactly one rename: {files:?}");
    assert_eq!(
        copied.len(),
        1,
        "the renamed-away delete is still a valid copy source: {files:?}"
    );
    assert_eq!(renamed[0]["old_path"], "a.txt");
    assert_eq!(copied[0]["old_path"], "a.txt");
    assert!(
        !files.iter().any(|f| f["status"] == "deleted"),
        "the consumed source must not also appear as a deletion: {files:?}"
    );
}

#[cfg(unix)]
#[test]
fn diff_rename_header_includes_mode_change() {
    use std::os::unix::fs::PermissionsExt;
    let repo = create_committed_repo_via_cli();
    commit_file(&repo, "orig.sh", "echo hi\nline2\nline3\n");
    fs::remove_file(repo.path().join("orig.sh")).unwrap();
    let new = repo.path().join("new.sh");
    fs::write(&new, "echo hi\nline2\nline3\n").unwrap();
    fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755)).unwrap();

    let output = run_libra_command(&["diff", "-M"], repo.path());
    assert_cli_success(&output, "diff -M mode change");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("old mode 100644"), "stdout={stdout}");
    assert!(stdout.contains("new mode 100755"), "stdout={stdout}");
    assert!(stdout.contains("rename from orig.sh"), "stdout={stdout}");
}

// ----- Wave 2a: --raw, --relative, diff.noPrefix -----

/// Create `src/a.txt` and `b.txt`, commit them, then modify both in the worktree.
fn setup_subdir_repo() -> tempfile::TempDir {
    let repo = create_committed_repo_via_cli();
    fs::create_dir_all(repo.path().join("src")).unwrap();
    fs::write(repo.path().join("src/a.txt"), "a\n").unwrap();
    fs::write(repo.path().join("b.txt"), "b\n").unwrap();
    assert_cli_success(
        &run_libra_command(&["add", "src/a.txt", "b.txt"], repo.path()),
        "stage subdir fixture",
    );
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "subdir base", "--no-verify"], repo.path()),
        "commit subdir fixture",
    );
    repo
}

#[test]
fn diff_raw_format_matches_git() {
    let repo = create_committed_repo_via_cli();
    fs::write(repo.path().join("tracked.txt"), "tracked\nmore\n").unwrap();
    let output = run_libra_command(&["diff", "--raw"], repo.path());
    assert_cli_success(&output, "diff --raw");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.lines().find(|l| l.ends_with("tracked.txt")).unwrap();
    let parts: Vec<&str> = line.splitn(5, ' ').collect();
    assert_eq!(parts[0], ":100644", "raw old mode: {line}");
    assert_eq!(parts[1], "100644", "raw new mode: {line}");
    assert_eq!(parts[2].len(), 7, "abbreviated old sha: {line}");
    assert_eq!(parts[3].len(), 7, "abbreviated new sha: {line}");
    assert_eq!(parts[4], "M\ttracked.txt", "status + path: {line}");
}

#[test]
fn diff_raw_rename_emits_two_paths() {
    let repo = create_committed_repo_via_cli();
    commit_file(&repo, "orig.txt", "alpha\nbeta\ngamma\n");
    fs::remove_file(repo.path().join("orig.txt")).unwrap();
    fs::write(repo.path().join("new.txt"), "alpha\nbeta\ngamma\n").unwrap();

    let output = run_libra_command(&["diff", "-M", "--raw"], repo.path());
    assert_cli_success(&output, "diff -M --raw");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .find(|l| l.contains("R100"))
        .expect("a rename raw line");
    assert!(
        line.ends_with("R100\torig.txt\tnew.txt"),
        "rename raw line must carry both paths: {line}"
    );
}

#[test]
fn diff_relative_filters_to_subdir() {
    let repo = setup_subdir_repo();
    fs::write(repo.path().join("src/a.txt"), "a2\n").unwrap();
    fs::write(repo.path().join("b.txt"), "b2\n").unwrap();

    let output = run_libra_command(&["--json", "diff", "--relative=src"], repo.path());
    assert_cli_success(&output, "diff --relative=src");
    let json = parse_json_stdout(&output);
    let files = json["data"]["files"].as_array().unwrap();
    assert_eq!(files.len(), 1, "only src/ files survive: {files:?}");
    assert_eq!(files[0]["path"], "a.txt", "prefix is stripped");
}

#[test]
fn diff_relative_strips_prefix() {
    let repo = setup_subdir_repo();
    fs::write(repo.path().join("src/a.txt"), "a2\n").unwrap();

    let output = run_libra_command(&["diff", "--relative=src"], repo.path());
    assert_cli_success(&output, "diff --relative=src unified");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("diff --git a/a.txt b/a.txt"),
        "stdout={stdout}"
    );
    assert!(stdout.contains("--- a/a.txt"), "stdout={stdout}");
    assert!(
        !stdout.contains("src/a.txt"),
        "prefix must be stripped: {stdout}"
    );
}

#[test]
fn diff_relative_combines_with_name_only() {
    let repo = setup_subdir_repo();
    fs::write(repo.path().join("src/a.txt"), "a2\n").unwrap();
    fs::write(repo.path().join("b.txt"), "b2\n").unwrap();

    let output = run_libra_command(&["diff", "--relative=src", "--name-only"], repo.path());
    assert_cli_success(&output, "diff --relative --name-only");
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "a.txt");
}

#[test]
fn diff_relative_excludes_outside_files_from_count() {
    let repo = setup_subdir_repo();
    // Only the file OUTSIDE src/ changes.
    fs::write(repo.path().join("b.txt"), "b2\n").unwrap();

    let output = run_libra_command(&["diff", "--relative=src", "--exit-code"], repo.path());
    assert_eq!(
        output.status.code(),
        Some(0),
        "outside-subtree change must not count under --relative: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn diff_relative_rejects_traversal() {
    let repo = create_committed_repo_via_cli();
    fs::write(repo.path().join("tracked.txt"), "tracked\nmore\n").unwrap();
    let output = run_libra_command(&["diff", "--relative=../escape"], repo.path());
    assert_eq!(
        output.status.code(),
        Some(129),
        "traversal must be rejected: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let (_stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-002");
}

#[test]
fn diff_noprefix_config_omits_ab_prefix() {
    let repo = create_committed_repo_via_cli();
    fs::write(repo.path().join("tracked.txt"), "tracked\nmore\n").unwrap();
    assert_cli_success(
        &run_libra_command(&["config", "set", "diff.noPrefix", "true"], repo.path()),
        "set diff.noPrefix",
    );

    let output = run_libra_command(&["diff"], repo.path());
    assert_cli_success(&output, "diff with noPrefix");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("diff --git tracked.txt tracked.txt"),
        "stdout={stdout}"
    );
    assert!(stdout.contains("--- tracked.txt"), "stdout={stdout}");
    assert!(stdout.contains("+++ tracked.txt"), "stdout={stdout}");
    assert!(!stdout.contains("a/tracked.txt"), "no a/ prefix: {stdout}");
}

#[test]
fn diff_output_format_precedence() {
    let repo = create_committed_repo_via_cli();
    fs::write(repo.path().join("tracked.txt"), "tracked\nmore\n").unwrap();

    // --raw beats --stat.
    let raw = run_libra_command(&["diff", "--raw", "--stat"], repo.path());
    assert!(
        String::from_utf8_lossy(&raw.stdout).contains(":100644"),
        "--raw should win over --stat"
    );

    // --json envelope is orthogonal and wins over human formats.
    let json = run_libra_command(&["--json", "diff", "--stat"], repo.path());
    let parsed = parse_json_stdout(&json);
    assert_eq!(parsed["command"], "diff");
    assert_eq!(parsed["data"]["files_changed"], 1);
}

// ----- Wave 2b: --word-diff -----

#[test]
fn diff_word_diff_plain_markers() {
    let repo = create_committed_repo_via_cli();
    commit_file(&repo, "w.txt", "alpha beta gamma\n");
    fs::write(repo.path().join("w.txt"), "alpha BETA gamma\n").unwrap();

    let output = run_libra_command(&["diff", "--word-diff=plain"], repo.path());
    assert_cli_success(&output, "diff --word-diff=plain");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("[-beta-]"), "deletion marker: {stdout}");
    assert!(stdout.contains("{+BETA+}"), "insertion marker: {stdout}");
    assert!(
        !stdout.contains('\u{1b}'),
        "plain mode has no ANSI: {stdout:?}"
    );
}

#[test]
fn diff_word_diff_color_has_ansi() {
    let repo = create_committed_repo_via_cli();
    commit_file(&repo, "w.txt", "alpha beta gamma\n");
    fs::write(repo.path().join("w.txt"), "alpha BETA gamma\n").unwrap();

    let output = run_libra_command(&["diff", "--word-diff=color"], repo.path());
    assert_cli_success(&output, "diff --word-diff=color");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains('\u{1b}'),
        "color mode emits ANSI: {stdout:?}"
    );
}

#[test]
fn diff_word_diff_regex_over_4kib_rejected() {
    let repo = create_committed_repo_via_cli();
    fs::write(repo.path().join("tracked.txt"), "tracked\nmore\n").unwrap();
    let long = "a".repeat(4097);
    let output = run_libra_command(&["diff", "--word-diff-regex", &long], repo.path());
    assert_eq!(
        output.status.code(),
        Some(129),
        "over-long --word-diff-regex must be a usage error: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let (_stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-002");
}

#[test]
fn diff_word_regex_config_override() {
    let repo = create_committed_repo_via_cli();
    commit_file(&repo, "w.txt", "x foo-bar y\n");
    fs::write(repo.path().join("w.txt"), "x foo-baz y\n").unwrap();
    // Treat hyphenated identifiers as a single word so the whole token is marked.
    assert_cli_success(
        &run_libra_command(
            &["config", "set", "diff.wordRegex", r"[\w-]+|\s+|[^\w\s]"],
            repo.path(),
        ),
        "set diff.wordRegex",
    );

    let output = run_libra_command(&["diff", "--word-diff=plain"], repo.path());
    assert_cli_success(&output, "diff --word-diff with custom regex");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("[-foo-bar-]"),
        "custom word regex should mark the whole hyphenated token: {stdout}"
    );
}

#[test]
fn diff_function_context_expands_hunk() {
    let repo = create_committed_repo_via_cli();
    let base = "fn alpha() {\n    let a = 1;\n    let b = 2;\n    let c = 3;\n    let d = 4;\n    let e = 5;\n    return a;\n}\n";
    let modified = "fn alpha() {\n    let a = 1;\n    let b = 2;\n    let c = 3;\n    let d = 40;\n    let e = 5;\n    return a;\n}\n";
    commit_file(&repo, "code.rs", base);
    fs::write(repo.path().join("code.rs"), modified).unwrap();

    // Default context (3) is too narrow to reach the `fn alpha` header.
    let default = run_libra_command(&["diff"], repo.path());
    assert_cli_success(&default, "default diff");
    assert!(
        !String::from_utf8_lossy(&default.stdout).contains("fn alpha"),
        "default context must not reach the function header"
    );

    // -W expands the hunk to the whole function.
    let wide = run_libra_command(&["diff", "-W"], repo.path());
    assert_cli_success(&wide, "diff -W");
    let stdout = String::from_utf8_lossy(&wide.stdout);
    assert!(
        stdout.contains("fn alpha() {"),
        "-W shows the function header: {stdout}"
    );
    assert!(stdout.contains("-    let d = 4;"), "{stdout}");
    assert!(stdout.contains("+    let d = 40;"), "{stdout}");
}

#[test]
fn diff_word_diff_large_file_falls_back() {
    let repo = create_committed_repo_via_cli();
    // ~12 MB across 1000 lines (well under git-internal's 10k-line marker, but
    // over the 10 MB word-diff cap).
    let base: String = (0..1000)
        .map(|_| format!("{}\n", "a".repeat(6000)))
        .collect();
    let modified: String = (0..1000)
        .map(|_| format!("{}\n", "b".repeat(6000)))
        .collect();
    commit_file(&repo, "big.txt", &base);
    fs::write(repo.path().join("big.txt"), &modified).unwrap();

    let output = run_libra_command(&["diff", "--word-diff=plain"], repo.path());
    assert_cli_success(&output, "diff --word-diff large file");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("word-diff skipped"),
        "large file should warn and fall back: {stderr}"
    );
}
