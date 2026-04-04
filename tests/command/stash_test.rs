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

    // stash push with no changes should print "no local changes"
    let output = run_libra_command(&["stash", "push"], temp_path.path());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no local changes"),
        "expected 'no local changes' in stderr, got: {stderr}"
    );
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
