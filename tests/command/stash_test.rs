//! Tests stash push/pop/apply/drop/list operations.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use std::fs;

use libra::{
    command::{
        add::{self, AddArgs},
        commit::{self, CommitArgs},
    },
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
