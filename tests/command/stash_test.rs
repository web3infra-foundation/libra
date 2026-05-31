//! Tests stash push/pop/apply/drop/list operations.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use std::{fs, path::PathBuf};

use libra::{
    command::{
        add::{self, AddArgs},
        commit::{self, CommitArgs},
    },
    internal::branch::Branch,
    utils::test::ChangeDirGuard,
};
use serial_test::serial;
use tempfile::tempdir;

use super::*;

#[test]
#[serial]
fn test_stash_cli_outside_repository_returns_fatal_128() {
    let temp = tempdir().unwrap();
    let output = run_libra_command(&["stash", "push"], temp.path());
    assert_eq!(output.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("fatal: not a libra repository"),
        "unexpected stderr: {stderr}"
    );
}

#[tokio::test]
#[serial]
async fn test_stash_push_no_changes() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    // Create initial commit so HEAD exists
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

    // stash push with no changes should remain a successful no-op
    let output = run_libra_command(&["stash", "push"], temp_path.path());
    assert_cli_success(&output, "stash push should be a no-op success");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("No local changes to save"),
        "expected no-op message in stdout, got: {stdout}"
    );
}

#[tokio::test]
#[serial]
async fn test_stash_push_no_changes_json_output() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

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

    let output = run_libra_command(&["stash", "push", "--json"], temp_path.path());
    assert_cli_success(&output, "stash push --json should be a no-op success");

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "stash");
    assert_eq!(json["data"]["action"], "noop");
    assert_eq!(json["data"]["message"], "No local changes to save");
    assert!(json["data"].get("stash_id").is_none());
}

#[tokio::test]
#[serial]
async fn test_stash_push_and_pop() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    // Create initial commit
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

    // Modify file
    fs::write("base.txt", "modified content").unwrap();

    // Stash push
    let output = run_libra_command(&["stash", "push"], temp_path.path());
    assert!(
        output.status.success(),
        "stash push failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Saved working directory"),
        "expected confirmation message, got: {stdout}"
    );

    // File should be restored to original
    let content = fs::read_to_string(temp_path.path().join("base.txt")).unwrap();
    assert_eq!(
        content, "base content",
        "file should be restored after stash push"
    );

    // Stash pop
    let output = run_libra_command(&["stash", "pop"], temp_path.path());
    assert!(
        output.status.success(),
        "stash pop failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // File should have modified content again
    let content = fs::read_to_string(temp_path.path().join("base.txt")).unwrap();
    assert_eq!(
        content, "modified content",
        "file should be modified after stash pop"
    );
}

#[tokio::test]
#[serial]
async fn test_stash_push_and_pop_preserves_dotfiles() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    fs::create_dir_all(".config").unwrap();
    fs::write(".gitignore", "target/\n").unwrap();
    fs::write(".config/tool.toml", "mode = \"base\"\n").unwrap();

    add::execute(AddArgs {
        pathspec: vec![".gitignore".to_string(), ".config/tool.toml".to_string()],
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
        message: Some("Track dotfiles".to_string()),
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

    fs::write(".gitignore", "target/\n.env\n").unwrap();
    fs::write(".config/tool.toml", "mode = \"stashed\"\n").unwrap();

    let output = run_libra_command(&["stash", "push"], temp_path.path());
    assert!(
        output.status.success(),
        "stash push failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(".gitignore").unwrap(),
        "target/\n",
        "dotfile should be restored after stash push"
    );
    assert_eq!(
        fs::read_to_string(".config/tool.toml").unwrap(),
        "mode = \"base\"\n",
        "dot-directory content should be restored after stash push"
    );

    let output = run_libra_command(&["stash", "pop"], temp_path.path());
    assert!(
        output.status.success(),
        "stash pop failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(".gitignore").unwrap(),
        "target/\n.env\n",
        "dotfile change should round-trip through stash"
    );
    assert_eq!(
        fs::read_to_string(".config/tool.toml").unwrap(),
        "mode = \"stashed\"\n",
        "dot-directory change should round-trip through stash"
    );
}

#[tokio::test]
#[serial]
async fn test_stash_list() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    // Create initial commit
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

    // Empty stash list
    let output = run_libra_command(&["stash", "list"], temp_path.path());
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.trim().is_empty(),
        "stash list should be empty initially"
    );

    // Create a stash
    fs::write("base.txt", "modified").unwrap();
    let output = run_libra_command(&["stash", "push"], temp_path.path());
    assert!(
        output.status.success(),
        "stash push failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // List should now show one entry
    let output = run_libra_command(&["stash", "list"], temp_path.path());
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("stash@{0}"),
        "expected stash@{{0}} in list, got: {stdout}"
    );
}

#[test]
fn test_stash_list_json_skips_blank_reflog_lines() {
    let repo = create_committed_repo_via_cli();

    fs::write(repo.path().join("tracked.txt"), "modified\n")
        .expect("failed to modify tracked file");
    assert_cli_success(
        &run_libra_command(&["stash", "push"], repo.path()),
        "stash push before reflog blank-line mutation",
    );

    let stash_log_path = repo.path().join(".libra/logs/refs/stash");
    let original = fs::read_to_string(&stash_log_path).expect("failed to read stash reflog");
    fs::write(&stash_log_path, format!("\n{original}\n\n"))
        .expect("failed to inject blank lines into stash reflog");

    let output = run_libra_command(&["stash", "list", "--json"], repo.path());
    assert_cli_success(
        &output,
        "stash list --json should ignore blank reflog lines",
    );

    let json = parse_json_stdout(&output);
    let entries = json["data"]["entries"]
        .as_array()
        .expect("expected stash list entries array");
    assert_eq!(entries.len(), 1, "blank reflog lines should be ignored");
    assert_eq!(entries[0]["index"], 0);
}

#[test]
fn test_stash_list_malformed_reflog_entry_returns_io_error() {
    let repo = create_committed_repo_via_cli();

    fs::write(repo.path().join("tracked.txt"), "modified\n")
        .expect("failed to modify tracked file");
    assert_cli_success(
        &run_libra_command(&["stash", "push"], repo.path()),
        "stash push before reflog corruption",
    );

    let stash_log_path = repo.path().join(".libra/logs/refs/stash");
    fs::write(&stash_log_path, "corrupted entry without hash\n")
        .expect("failed to corrupt stash reflog");

    let output = run_libra_command(&["stash", "list"], repo.path());
    assert_eq!(output.status.code(), Some(128));

    let (human, report) = parse_cli_error_stderr(&output.stderr);
    assert!(
        human.contains("corrupted stash log entry"),
        "unexpected stderr: {human}"
    );
    assert_eq!(report.error_code, "LBR-IO-001");
    assert_eq!(report.exit_code, 128);
}

#[tokio::test]
#[serial]
async fn test_stash_drop() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    // Create initial commit
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

    // Create a stash
    fs::write("base.txt", "modified").unwrap();
    run_libra_command(&["stash", "push"], temp_path.path());

    // Drop it
    let output = run_libra_command(&["stash", "drop"], temp_path.path());
    assert!(
        output.status.success(),
        "stash drop failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Dropped stash@{0}"),
        "expected drop confirmation, got: {stdout}"
    );

    // List should be empty now
    let output = run_libra_command(&["stash", "list"], temp_path.path());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.trim().is_empty(),
        "stash list should be empty after drop"
    );
}

#[tokio::test]
#[serial]
async fn test_stash_drop_missing_reflog_returns_no_stash_found() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

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

    fs::write("base.txt", "modified").unwrap();
    assert_cli_success(
        &run_libra_command(&["stash", "push"], temp_path.path()),
        "stash push before reflog removal",
    );

    fs::remove_file(temp_path.path().join(".libra/logs/refs/stash"))
        .expect("failed to remove stash reflog");

    let output = run_libra_command(&["stash", "drop"], temp_path.path());
    assert_eq!(output.status.code(), Some(129));

    let (human, report) = parse_cli_error_stderr(&output.stderr);
    assert!(
        human.contains("fatal: no stash found"),
        "unexpected stderr: {human}"
    );
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert_eq!(report.exit_code, 129);
}

#[tokio::test]
#[serial]
async fn test_stash_json_output() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    // Create initial commit
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

    // JSON list on empty stash
    let output = run_libra_command(&["stash", "list", "--json"], temp_path.path());
    assert!(output.status.success());
    let json: Value =
        serde_json::from_slice(&output.stdout).expect("expected valid JSON from stash list --json");
    assert_eq!(json["command"], "stash");
    assert_eq!(json["data"]["action"], "list");
    assert!(json["data"]["entries"].as_array().unwrap().is_empty());

    // Stash something and test push JSON
    fs::write("base.txt", "modified").unwrap();
    let output = run_libra_command(&["stash", "push", "--json"], temp_path.path());
    assert!(
        output.status.success(),
        "stash push --json failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value =
        serde_json::from_slice(&output.stdout).expect("expected valid JSON from stash push --json");
    assert_eq!(json["command"], "stash");
    assert_eq!(json["data"]["action"], "push");
    assert!(json["data"]["message"].as_str().is_some());
    assert!(json["data"]["stash_id"].as_str().is_some());
}

#[test]
fn stash_round_trip_preserves_nested_dotfile_paths() {
    let repo = create_committed_repo_via_cli();

    let config_dir = repo.path().join(".config");
    let nested_file = config_dir.join("tool.toml");
    fs::create_dir_all(&config_dir).expect("failed to create nested config dir");
    fs::write(&nested_file, "name = \"base\"\n").expect("failed to write base nested file");

    let output = run_libra_command(&["add", ".config/tool.toml"], repo.path());
    assert_cli_success(&output, "add nested dotfile");

    let output = run_libra_command(
        &["commit", "-m", "track nested dotfile", "--no-verify"],
        repo.path(),
    );
    assert_cli_success(&output, "commit nested dotfile");

    fs::write(&nested_file, "name = \"modified\"\n").expect("failed to write modified nested file");

    let output = run_libra_command(&["stash", "push"], repo.path());
    assert_cli_success(&output, "stash push nested dotfile");
    assert_eq!(
        fs::read_to_string(&nested_file).expect("failed to read nested file after stash push"),
        "name = \"base\"\n"
    );

    let output = run_libra_command(&["stash", "pop"], repo.path());
    assert_cli_success(&output, "stash pop nested dotfile");

    assert_eq!(
        fs::read_to_string(&nested_file).expect("failed to read nested file after stash pop"),
        "name = \"modified\"\n"
    );
    assert!(
        !repo.path().join("tool.toml").exists(),
        "stash pop should not flatten nested dotfiles into the repo root"
    );
}

// ── C4 surface tests: `stash show` / `stash branch` / `stash clear` ───────────────────────

/// `libra stash --help` lists the new subcommands plus the EXAMPLES banner.
#[test]
fn test_stash_help_lists_show_branch_clear() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["stash", "--help"], repo.path());
    assert!(
        output.status.success(),
        "stash --help should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    for sub in ["show", "branch", "clear"] {
        assert!(
            stdout.contains(sub),
            "stash --help should list '{sub}', stdout: {stdout}"
        );
    }
    assert!(
        stdout.contains("EXAMPLES:"),
        "stash --help should include EXAMPLES, stdout: {stdout}"
    );
}

/// `stash show` against a stash with a modified file emits a per-file
/// status entry and the matching JSON envelope.
#[test]
fn test_stash_show_reports_modified_file() {
    let repo = create_committed_repo_via_cli();

    fs::write(repo.path().join("tracked.txt"), "modified content\n")
        .expect("failed to modify tracked file");
    assert_cli_success(
        &run_libra_command(&["stash", "push"], repo.path()),
        "stash push before show",
    );

    let output = run_libra_command(&["stash", "show", "--json"], repo.path());
    assert_cli_success(&output, "stash show --json");

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "stash");
    assert_eq!(json["data"]["action"], "show");
    let files = json["data"]["files"]
        .as_array()
        .expect("files should be an array");
    let tracked_modified = files
        .iter()
        .find(|f| f["path"] == "tracked.txt")
        .expect("tracked.txt must appear in stash show output");
    assert_eq!(
        tracked_modified["status"], "modified",
        "tracked.txt should be reported as modified"
    );
    assert!(
        json["data"]["files_changed"]["modified"]
            .as_u64()
            .expect("files_changed.modified should be a number")
            >= 1
    );
}

/// `stash show --name-only` in human mode prints only the file path,
/// without the "files changed" footer.
#[test]
fn test_stash_show_name_only_strips_summary() {
    let repo = create_committed_repo_via_cli();

    fs::write(repo.path().join("tracked.txt"), "modified content\n")
        .expect("failed to modify tracked file");
    assert_cli_success(
        &run_libra_command(&["stash", "push"], repo.path()),
        "stash push before show --name-only",
    );

    let output = run_libra_command(&["stash", "show", "--name-only"], repo.path());
    assert_cli_success(&output, "stash show --name-only");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.lines().any(|l| l == "tracked.txt"),
        "stash show --name-only should print 'tracked.txt', stdout: {stdout}"
    );
    assert!(
        !stdout.contains("files changed"),
        "stash show --name-only should suppress the footer, stdout: {stdout}"
    );
}

/// `stash show stash@{NN}` with an out-of-range index returns a fatal
/// error mapped to `LBR-CLI-003`.
#[test]
fn test_stash_show_invalid_index_errors() {
    let repo = create_committed_repo_via_cli();

    fs::write(repo.path().join("tracked.txt"), "modified\n").unwrap();
    assert_cli_success(
        &run_libra_command(&["stash", "push"], repo.path()),
        "stash push before invalid show",
    );

    let output = run_libra_command(&["stash", "show", "stash@{42}"], repo.path());
    assert!(
        !output.status.success(),
        "stash show with bad index must fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("LBR-CLI-003"),
        "stash show invalid index should map to CLI-003, stderr: {stderr}"
    );
}

/// `stash branch <name>` creates a new branch, applies the stash, and
/// drops it. `applied` and `dropped` are both `true` in the JSON output
/// when the operation succeeds end-to-end.
#[test]
fn test_stash_branch_creates_branch_and_applies() {
    let repo = create_committed_repo_via_cli();

    fs::write(repo.path().join("tracked.txt"), "modified\n").unwrap();
    assert_cli_success(
        &run_libra_command(&["stash", "push"], repo.path()),
        "stash push before branch",
    );

    let output = run_libra_command(&["stash", "branch", "stash-feature", "--json"], repo.path());
    assert_cli_success(&output, "stash branch --json");

    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["action"], "branch");
    assert_eq!(json["data"]["branch"], "stash-feature");
    assert_eq!(json["data"]["applied"], true);
    assert_eq!(json["data"]["dropped"], true);
}

/// `stash branch <existing-name>` refuses with the dedicated
/// `LBR-CONFLICT-002` so callers can distinguish from generic failures.
#[test]
fn test_stash_branch_refuses_existing_branch() {
    let repo = create_committed_repo_via_cli();

    // Create a competing branch first via the CLI.
    assert_cli_success(
        &run_libra_command(&["branch", "occupied"], repo.path()),
        "create occupied branch",
    );

    fs::write(repo.path().join("tracked.txt"), "modified\n").unwrap();
    assert_cli_success(
        &run_libra_command(&["stash", "push"], repo.path()),
        "stash push before branch conflict",
    );

    let output = run_libra_command(&["stash", "branch", "occupied"], repo.path());
    assert!(
        !output.status.success(),
        "stash branch onto existing name must fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("LBR-CONFLICT-002"),
        "branch conflict should surface ConflictOperationBlocked, stderr: {stderr}"
    );
}

/// `stash branch` must not create the new branch or overwrite the current
/// dirty worktree when unrelated local edits exist after the stash was made.
#[tokio::test]
#[serial]
async fn test_stash_branch_refuses_dirty_worktree_without_creating_branch() {
    let repo = create_committed_repo_via_cli();

    fs::write(repo.path().join("tracked.txt"), "stashed change\n").unwrap();
    assert_cli_success(
        &run_libra_command(&["stash", "push"], repo.path()),
        "stash push before dirty branch refusal",
    );
    fs::write(repo.path().join("tracked.txt"), "local dirty change\n").unwrap();

    let output = run_libra_command(&["stash", "branch", "stash-dirty"], repo.path());
    assert!(
        !output.status.success(),
        "stash branch must refuse dirty worktree"
    );
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-REPO-003");
    assert!(
        stderr.contains("unstaged changes, can't create branch from stash"),
        "unexpected stderr: {stderr}"
    );
    assert_eq!(
        fs::read_to_string(repo.path().join("tracked.txt")).unwrap(),
        "local dirty change\n",
        "stash branch refusal must preserve the user's dirty worktree"
    );

    {
        let _guard = ChangeDirGuard::new(repo.path());
        let branch = Branch::find_branch_result("stash-dirty", None)
            .await
            .expect("branch lookup should succeed");
        assert!(
            branch.is_none(),
            "stash branch must not create a branch after dirty-worktree refusal"
        );
    }

    let list = run_libra_command(&["stash", "list", "--json"], repo.path());
    assert_cli_success(&list, "stash list after dirty branch refusal");
    let json = parse_json_stdout(&list);
    assert_eq!(
        json["data"]["entries"]
            .as_array()
            .expect("entries array")
            .len(),
        1,
        "stash entry should remain available after dirty-worktree refusal"
    );
}

/// Untracked files are also dirty for `stash branch`, because applying the
/// stashed tree can overwrite an untracked path with the same name.
#[tokio::test]
#[serial]
async fn test_stash_branch_refuses_untracked_worktree_without_side_effects() {
    let repo = create_committed_repo_via_cli();

    fs::write(repo.path().join("tracked.txt"), "stashed tracked change\n").unwrap();
    fs::write(repo.path().join("scratch.txt"), "stashed scratch\n").unwrap();
    assert_cli_success(
        &run_libra_command(&["stash", "push"], repo.path()),
        "stash push before untracked branch refusal",
    );
    fs::write(repo.path().join("scratch.txt"), "local untracked scratch\n").unwrap();

    let output = run_libra_command(&["stash", "branch", "stash-untracked"], repo.path());
    assert!(
        !output.status.success(),
        "stash branch must refuse untracked files"
    );
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-REPO-003");
    assert!(
        stderr.contains("untracked files, can't create branch from stash"),
        "unexpected stderr: {stderr}"
    );
    assert_eq!(
        fs::read_to_string(repo.path().join("scratch.txt")).unwrap(),
        "local untracked scratch\n",
        "stash branch refusal must not overwrite a conflicting untracked file"
    );

    {
        let _guard = ChangeDirGuard::new(repo.path());
        let branch = Branch::find_branch_result("stash-untracked", None)
            .await
            .expect("branch lookup should succeed");
        assert!(
            branch.is_none(),
            "stash branch must not create a branch after untracked-worktree refusal"
        );
    }

    let list = run_libra_command(&["stash", "list", "--json"], repo.path());
    assert_cli_success(&list, "stash list after untracked branch refusal");
    let json = parse_json_stdout(&list);
    assert_eq!(
        json["data"]["entries"]
            .as_array()
            .expect("entries array")
            .len(),
        1,
        "stash entry should remain available after untracked-worktree refusal"
    );
}

/// `stash branch` also refuses staged changes before creating a branch, applying
/// the stash, or dropping the stash entry.
#[tokio::test]
#[serial]
async fn test_stash_branch_refuses_staged_worktree_without_side_effects() {
    let repo = create_committed_repo_via_cli();

    fs::write(repo.path().join("tracked.txt"), "stashed change\n").unwrap();
    assert_cli_success(
        &run_libra_command(&["stash", "push"], repo.path()),
        "stash push before staged branch refusal",
    );
    fs::write(repo.path().join("tracked.txt"), "local staged change\n").unwrap();
    assert_cli_success(
        &run_libra_command(&["add", "tracked.txt"], repo.path()),
        "stage local change before stash branch",
    );

    let output = run_libra_command(&["stash", "branch", "stash-staged"], repo.path());
    assert!(
        !output.status.success(),
        "stash branch must refuse staged changes"
    );
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-REPO-003");
    assert!(
        stderr.contains("uncommitted changes, can't create branch from stash"),
        "unexpected stderr: {stderr}"
    );
    assert_eq!(
        fs::read_to_string(repo.path().join("tracked.txt")).unwrap(),
        "local staged change\n",
        "stash branch refusal must preserve the user's staged worktree"
    );

    {
        let _guard = ChangeDirGuard::new(repo.path());
        let branch = Branch::find_branch_result("stash-staged", None)
            .await
            .expect("branch lookup should succeed");
        assert!(
            branch.is_none(),
            "stash branch must not create a branch after staged-worktree refusal"
        );

        let staged = libra::command::status::changes_to_be_committed().await;
        assert!(
            staged.modified.contains(&PathBuf::from("tracked.txt")),
            "staged tracked.txt modification must remain in the index"
        );
        let unstaged = libra::command::status::changes_to_be_staged()
            .expect("unstaged status should be readable");
        assert!(
            unstaged.is_empty(),
            "stash branch refusal must not introduce unstaged changes: {unstaged:?}"
        );
    }

    let list = run_libra_command(&["stash", "list", "--json"], repo.path());
    assert_cli_success(&list, "stash list after staged branch refusal");
    let json = parse_json_stdout(&list);
    assert_eq!(
        json["data"]["entries"]
            .as_array()
            .expect("entries array")
            .len(),
        1,
        "stash entry should remain available after staged-worktree refusal"
    );
}

/// A dirty worktree must not mask the existing "no stash found" failure mode.
#[tokio::test]
#[serial]
async fn test_stash_branch_reports_no_stash_before_dirty_worktree() {
    let repo = create_committed_repo_via_cli();

    fs::write(repo.path().join("tracked.txt"), "local dirty change\n").unwrap();

    let output = run_libra_command(&["stash", "branch", "missing-stash"], repo.path());
    assert!(
        !output.status.success(),
        "stash branch with no stash must fail"
    );
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert!(
        stderr.contains("no stash found"),
        "no-stash priority should be preserved, stderr: {stderr}"
    );
    assert_eq!(
        fs::read_to_string(repo.path().join("tracked.txt")).unwrap(),
        "local dirty change\n",
        "no-stash failure must preserve the user's dirty worktree"
    );

    {
        let _guard = ChangeDirGuard::new(repo.path());
        let branch = Branch::find_branch_result("missing-stash", None)
            .await
            .expect("branch lookup should succeed");
        assert!(
            branch.is_none(),
            "stash branch must not create a branch when no stash exists"
        );
    }
}

/// `stash branch <name>` must treat a corrupt existing branch row as
/// name-occupied instead of letting the lossy branch lookup downgrade it to
/// "missing" and overwrite the row.
#[tokio::test]
#[serial]
async fn test_stash_branch_refuses_corrupt_existing_branch() {
    let repo = create_committed_repo_via_cli();
    {
        let _guard = ChangeDirGuard::new(repo.path());
        Branch::update_branch("occupied", "not-a-valid-hash", None)
            .await
            .unwrap();
    }

    fs::write(repo.path().join("tracked.txt"), "modified\n").unwrap();
    assert_cli_success(
        &run_libra_command(&["stash", "push"], repo.path()),
        "stash push before corrupt branch conflict",
    );

    let output = run_libra_command(&["stash", "branch", "occupied"], repo.path());
    assert!(
        !output.status.success(),
        "stash branch must not overwrite a corrupt existing branch row"
    );
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CONFLICT-002");
    assert!(
        stderr.contains("a branch named 'occupied' already exists"),
        "unexpected stderr: {stderr}"
    );
}

/// `stash clear` without `--force` and not in JSON mode is rejected with
/// `LBR-CLI-002` to avoid accidental destructive runs in interactive use.
#[test]
fn test_stash_clear_requires_force_in_human_mode() {
    let repo = create_committed_repo_via_cli();

    fs::write(repo.path().join("tracked.txt"), "modified\n").unwrap();
    assert_cli_success(
        &run_libra_command(&["stash", "push"], repo.path()),
        "stash push before clear without force",
    );

    let output = run_libra_command(&["stash", "clear"], repo.path());
    assert!(
        !output.status.success(),
        "stash clear without --force should fail in human mode"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("LBR-CLI-002"),
        "stash clear refusal should use CLI-002, stderr: {stderr}"
    );
}

/// `stash clear --force` removes every entry and reports the count.
#[test]
fn test_stash_clear_force_removes_all_entries() {
    let repo = create_committed_repo_via_cli();

    // Create two stash entries so the cleared_count is non-trivial.
    fs::write(repo.path().join("tracked.txt"), "first\n").unwrap();
    assert_cli_success(
        &run_libra_command(&["stash", "push"], repo.path()),
        "stash push first",
    );
    fs::write(repo.path().join("tracked.txt"), "second\n").unwrap();
    assert_cli_success(
        &run_libra_command(&["stash", "push"], repo.path()),
        "stash push second",
    );

    let output = run_libra_command(&["stash", "clear", "--force", "--json"], repo.path());
    assert_cli_success(&output, "stash clear --force --json");

    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["action"], "clear");
    assert_eq!(json["data"]["cleared_count"], 2);

    // After clear the list should be empty again.
    let list = run_libra_command(&["stash", "list", "--json"], repo.path());
    assert_cli_success(&list, "stash list after clear");
    let list_json = parse_json_stdout(&list);
    assert_eq!(
        list_json["data"]["entries"]
            .as_array()
            .expect("entries array")
            .len(),
        0
    );
}
