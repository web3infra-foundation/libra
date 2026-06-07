//! Tests `libra add` behavior for staging files, refresh operations, and
//! edge cases via the in-process API (`add::execute`).
//!
//! **Layer:** L1 — deterministic, no external dependencies.
//!
//! Fixture convention: every test creates a `tempdir()`, calls
//! `test::setup_with_new_libra_in()` to bootstrap a fresh repo, holds a
//! `ChangeDirGuard` (hence `#[serial]`), then operates on plain text files
//! at the repo root or in nested subdirectories. Assertions inspect the
//! index via `changes_to_be_committed()` (staged) or
//! `changes_to_be_staged()` (working-tree-vs-index).

use std::{fs, io::Write};

use git_internal::internal::object::tree::TreeItemMode;
use libra::internal::{ai::automation::AutomationHistory, db::get_db_conn_instance};

use super::*;

/// Scenario: smoke test for the simplest staging path — create one file,
/// run `add`, and confirm the path appears in the staged "new" set.
#[tokio::test]
#[serial]
async fn test_add_single_file() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // Create a new file
    let file_content = "Hello, World!";
    let file_path = "test_file.txt";
    let mut file = fs::File::create(file_path).unwrap();
    file.write_all(file_content.as_bytes()).unwrap();

    // Execute add command
    add::execute(AddArgs {
        pathspec: vec![String::from(file_path)],
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

    // Verify the file was added to index.
    let changes = changes_to_be_committed().await;

    assert!(changes.new.iter().any(|x| x.to_str().unwrap() == file_path));
}

#[tokio::test]
#[serial]
async fn test_add_dispatches_vcs_automation_history() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());
    fs::write(
        test_dir.path().join(".libra").join("automations.toml"),
        r#"
        [[rules]]
        id = "index_summary"
        trigger = { kind = "vcs", event = "post_add" }
        action = { kind = "prompt", prompt = "summarize staged changes" }
    "#,
    )
    .unwrap();
    fs::write("automated.txt", "content").unwrap();

    add::execute_safe(
        AddArgs {
            pathspec: vec!["automated.txt".to_string()],
            all: false,
            update: false,
            refresh: false,
            force: false,
            verbose: false,
            dry_run: false,
            ignore_errors: false,
            ..Default::default()
        },
        &libra::utils::output::OutputConfig::default(),
    )
    .await
    .unwrap();

    let db = get_db_conn_instance().await;
    let rows = AutomationHistory::list_recent(&db, 10).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].rule_id, "index_summary");
    assert_eq!(rows[0].trigger_kind, "vcs");
    assert_eq!(rows[0].details["prompt"], "summarize staged changes");
}

#[tokio::test]
#[serial]
async fn test_add_dry_run_does_not_dispatch_vcs_automation_history() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());
    fs::write(
        test_dir.path().join(".libra").join("automations.toml"),
        r#"
        [[rules]]
        id = "index_summary"
        trigger = { kind = "vcs", event = "post_add" }
        action = { kind = "prompt", prompt = "summarize staged changes" }
    "#,
    )
    .unwrap();
    fs::write("dry-run.txt", "content").unwrap();

    add::execute_safe(
        AddArgs {
            pathspec: vec!["dry-run.txt".to_string()],
            all: false,
            update: false,
            refresh: false,
            force: false,
            verbose: false,
            dry_run: true,
            ignore_errors: false,
            ..Default::default()
        },
        &libra::utils::output::OutputConfig::default(),
    )
    .await
    .unwrap();

    let db = get_db_conn_instance().await;
    let rows = AutomationHistory::list_recent(&db, 10).await.unwrap();
    assert!(rows.is_empty());
}

/// Scenario: passing several pathspecs in one `add` call must stage every
/// listed file. Guards against accidental short-circuiting after the first
/// path.
#[tokio::test]
#[serial]
async fn test_add_multiple_files() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // Create multiple files
    for i in 1..=3 {
        let file_content = format!("File content {i}");
        let file_path = format!("test_file_{i}.txt");
        let mut file = fs::File::create(&file_path).unwrap();
        file.write_all(file_content.as_bytes()).unwrap();
    }

    // Execute add command
    add::execute(AddArgs {
        pathspec: vec![
            String::from("test_file_1.txt"),
            String::from("test_file_2.txt"),
            String::from("test_file_3.txt"),
        ],
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

    // Verify all files were added to index
    let changes = changes_to_be_committed().await;
    assert!(
        changes
            .new
            .iter()
            .any(|x| x.to_str().unwrap() == "test_file_1.txt")
    );
    assert!(
        changes
            .new
            .iter()
            .any(|x| x.to_str().unwrap() == "test_file_2.txt")
    );
    assert!(
        changes
            .new
            .iter()
            .any(|x| x.to_str().unwrap() == "test_file_3.txt")
    );
}

/// Scenario: `--all` walks the working tree and stages every untracked
/// file even though no pathspec is supplied. Locks in the recursive
/// scan behavior of `-A`.
#[tokio::test]
#[serial]
async fn test_add_all_flag() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // Create multiple files
    for i in 1..=3 {
        let file_content = format!("File content {i}");
        let file_path = format!("test_file_{i}.txt");
        let mut file = fs::File::create(&file_path).unwrap();
        file.write_all(file_content.as_bytes()).unwrap();
    }

    // Execute add command with --all flag
    add::execute(AddArgs {
        pathspec: vec![],
        all: true,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        ..Default::default()
    })
    .await;

    // Verify all files were added to index
    let changes = changes_to_be_committed().await;
    assert!(
        changes
            .new
            .iter()
            .any(|x| x.to_str().unwrap() == "test_file_1.txt")
    );
    assert!(
        changes
            .new
            .iter()
            .any(|x| x.to_str().unwrap() == "test_file_2.txt")
    );
    assert!(
        changes
            .new
            .iter()
            .any(|x| x.to_str().unwrap() == "test_file_3.txt")
    );
}

/// Scenario: `--update` (`-u`) must update tracked files only and never
/// promote untracked files to staged. Verifies that the previously-tracked
/// file ceases to show as modified (it was restaged) while the untracked
/// file remains in the "new" set.
#[tokio::test]
#[serial]
async fn test_add_update_flag() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // Create files and add one to the index
    let tracked_file = "tracked_file.txt";
    let untracked_file = "untracked_file.txt";

    // Create and write initial content
    let mut file1 = fs::File::create(tracked_file).unwrap();
    file1.write_all(b"Initial content").unwrap();

    let mut file2 = fs::File::create(untracked_file).unwrap();
    file2.write_all(b"Initial content").unwrap();

    // Add only one file to the index
    add::execute(AddArgs {
        pathspec: vec![String::from(tracked_file)],
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

    // Modify both files
    let mut file1 = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(tracked_file)
        .unwrap();
    file1.write_all(b" - Modified").unwrap();

    let mut file2 = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(untracked_file)
        .unwrap();
    file2.write_all(b" - Modified").unwrap();

    // Execute add command with --update flag
    add::execute(AddArgs {
        pathspec: vec![String::from(".")],
        all: false,
        update: true,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        ..Default::default()
    })
    .await;

    // Verify only tracked file was updated
    let changes = changes_to_be_staged().unwrap();
    // Tracked file should not appear in changes (because it was updated in index)
    assert!(
        !changes
            .modified
            .iter()
            .any(|x| x.to_str().unwrap() == tracked_file)
    );
    // Untracked file should still be untracked and show as new
    assert!(
        changes
            .new
            .iter()
            .any(|x| x.to_str().unwrap() == untracked_file)
    );
}

/// Scenario: `.libraignore` patterns must filter both globbed file names
/// and entire directories. The non-ignored file must end up staged while
/// `ignored_*.txt` and `ignore_dir/**` remain hidden in both staged and
/// committed change lists. Pins ignore-glob semantics.
#[tokio::test]
#[serial]
async fn test_add_with_ignore_patterns() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // Create .libraignore file
    let mut ignore_file = fs::File::create(".libraignore").unwrap();
    ignore_file
        .write_all(b"ignored_*.txt\nignore_dir/**")
        .unwrap();

    // Create files that should be ignored and not ignored
    let ignored_file = "ignored_file.txt";
    let tracked_file = "tracked_file.txt";

    // Create directory that should be ignored
    fs::create_dir("ignore_dir").unwrap();
    let ignored_dir_file = "ignore_dir/file.txt";

    // Create and write content
    let mut file1 = fs::File::create(ignored_file).unwrap();
    file1.write_all(b"Should be ignored").unwrap();

    let mut file2 = fs::File::create(tracked_file).unwrap();
    file2.write_all(b"Should be tracked").unwrap();

    let mut file3 = fs::File::create(ignored_dir_file).unwrap();
    file3.write_all(b"Should be ignored").unwrap();

    // Execute add command with all files
    add::execute(AddArgs {
        pathspec: vec![String::from(".")],
        all: true,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        ..Default::default()
    })
    .await;

    // Verify only non-ignored files were added
    let changes_staged = changes_to_be_staged().unwrap();
    let changes_committed = changes_to_be_committed().await;

    // Ignored files should not appear in any status (they are ignored)
    assert!(
        !changes_staged
            .new
            .iter()
            .any(|x| x.to_str().unwrap() == ignored_file)
    );
    assert!(
        !changes_staged
            .new
            .iter()
            .any(|x| x.to_str().unwrap() == ignored_dir_file)
    );
    assert!(
        !changes_committed
            .new
            .iter()
            .any(|x| x.to_str().unwrap() == ignored_file)
    );
    assert!(
        !changes_committed
            .new
            .iter()
            .any(|x| x.to_str().unwrap() == ignored_dir_file)
    );

    // Non-ignored file should not show as new in staged (was added) but should show in committed
    assert!(
        !changes_staged
            .new
            .iter()
            .any(|x| x.to_str().unwrap() == tracked_file)
    );
    assert!(
        changes_committed
            .new
            .iter()
            .any(|x| x.to_str().unwrap() == tracked_file)
    );
}

/// Scenario: `--force` lifts the ignore filter for a single path and once
/// that path is tracked, subsequent edits flow through without `--force`.
/// Validates the "force once, stay tracked" promise.
#[tokio::test]
#[serial]
async fn test_add_force_tracks_ignored_file() {
    let repo = tempdir().unwrap();
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = test::ChangeDirGuard::new(repo.path());

    fs::write(".libraignore", "ignored.txt\n").unwrap();
    fs::write("ignored.txt", "first").unwrap();

    let ignored_path = "ignored.txt";

    // Without --force the ignored file should stay hidden from staging
    let unstaged_initial = changes_to_be_staged().unwrap();
    assert!(
        !unstaged_initial
            .new
            .iter()
            .any(|p| p.to_str().unwrap() == ignored_path)
    );

    add::execute(AddArgs {
        pathspec: vec![ignored_path.into()],
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

    let staged_without_force = changes_to_be_committed().await;
    assert!(
        !staged_without_force
            .new
            .iter()
            .any(|p| p.to_str().unwrap() == ignored_path)
    );

    // Force add should stage the ignored file
    add::execute(AddArgs {
        pathspec: vec![ignored_path.into()],
        all: false,
        update: false,
        refresh: false,
        force: true,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        ..Default::default()
    })
    .await;

    let staged_with_force = changes_to_be_committed().await;
    assert!(
        staged_with_force
            .new
            .iter()
            .any(|p| p.to_str().unwrap() == ignored_path)
    );

    // After being tracked, further updates should appear without --force
    fs::write("ignored.txt", "second").unwrap();

    let unstaged_after_edit = changes_to_be_staged().unwrap();
    assert!(
        unstaged_after_edit
            .modified
            .iter()
            .any(|p| p.to_str().unwrap() == ignored_path)
    );

    add::execute(AddArgs {
        pathspec: vec![ignored_path.into()],
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

    let staged_after_update = changes_to_be_committed().await;
    assert!(
        staged_after_update
            .new
            .iter()
            .any(|p| p.to_str().unwrap() == ignored_path)
    );

    let unstaged_final = changes_to_be_staged().unwrap();
    assert!(
        !unstaged_final
            .modified
            .iter()
            .any(|p| p.to_str().unwrap() == ignored_path)
    );
}

/// Scenario: `add --force .` recursively includes the contents of an
/// ignored directory. Path separators are normalized to forward slashes
/// for cross-platform comparison. Pins the directory-level force semantic.
#[tokio::test]
#[serial]
async fn test_add_force_dot_includes_ignored_directory() {
    let repo = tempdir().unwrap();
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = test::ChangeDirGuard::new(repo.path());

    fs::write(".libraignore", "ignored_dir/\n").unwrap();
    fs::create_dir_all("ignored_dir").unwrap();
    fs::write("ignored_dir/nested.txt", "ignored").unwrap();
    fs::write("visible.txt", "seen").unwrap();

    // Baseline: without --force the ignored directory stays hidden
    add::execute(AddArgs {
        pathspec: vec![".".into()],
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

    let staged_without_force = changes_to_be_committed().await;
    assert!(
        !staged_without_force
            .new
            .iter()
            .any(|p| p.to_str().unwrap().replace("\\", "/") == "ignored_dir/nested.txt"),
        "ignored entries should not be staged when force is false"
    );
    assert!(
        staged_without_force
            .new
            .iter()
            .any(|p| p.to_str().unwrap() == "visible.txt"),
        "non-ignored files should still be staged"
    );

    // Re-run with --force to include ignored entries
    add::execute(AddArgs {
        pathspec: vec![".".into()],
        all: false,
        update: false,
        refresh: false,
        force: true,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        ..Default::default()
    })
    .await;

    let staged_with_force = changes_to_be_committed().await;
    assert!(
        staged_with_force
            .new
            .iter()
            .any(|p| p.to_str().unwrap().replace("\\", "/") == "ignored_dir/nested.txt"),
        "`add --force .` should surface ignored children"
    );
}

/// Scenario: `--dry-run` should leave the index unchanged. Note: this
/// test asserts that the path appears in `changes_to_be_staged().new` —
/// i.e. the file is detected as untracked in the working tree, confirming
/// it was not staged.
#[tokio::test]
#[serial]
async fn test_add_dry_run() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // Create a file.
    let file_path = "test_file.txt";
    let mut file = fs::File::create(file_path).unwrap();
    file.write_all(b"Test content").unwrap();

    // Execute add command with dry-run
    add::execute(AddArgs {
        pathspec: vec![String::from(file_path)],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: true,
        ignore_errors: false,
        ..Default::default()
    })
    .await;

    // Verify the file was not actually added to index
    let changes = changes_to_be_staged().unwrap();
    assert!(changes.new.iter().any(|x| x.to_str().unwrap() == file_path));
}

/// Scenario: in-process `add::execute` with no pathspec and no `--all`
/// must not silently stage anything. The index should be empty after the
/// call. Boundary condition: the in-process API does not surface CLI exit
/// codes, so the assertion is on side effects only.
#[tokio::test]
#[serial]
async fn test_add_without_path_should_error() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // Create a file to ensure there's something that could be added
    let file_path = "existing_file.txt";
    let mut file = fs::File::create(file_path).unwrap();
    file.write_all(b"Some content").unwrap();

    // Try running `add` without any pathspec and without --all
    add::execute(AddArgs {
        pathspec: vec![], // Empty pathspec
        all: false,       // Not using --all
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        ..Default::default()
    })
    .await;

    // Verify no files were added to the index
    let changes = changes_to_be_committed().await;
    assert!(
        changes.new.is_empty(),
        "Expected no files in index when no pathspec provided and --all not used"
    );
}

/// Scenario: passing a path that doesn't exist must not stage anything.
/// Pins the post-condition: the bogus path never appears in
/// `changes_to_be_committed().new`.
#[tokio::test]
#[serial]
async fn test_add_nonexistent_file_should_error() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    let fake_path = "no_such_file.txt";

    // Try to add non-existent file
    add::execute(AddArgs {
        pathspec: vec![String::from(fake_path)],
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

    // The file should not be in the index
    let changes = changes_to_be_committed().await;
    let file_in_index = changes.new.iter().any(|x| x.to_str().unwrap() == fake_path);
    assert!(
        !file_in_index,
        "Non-existent file should not be added to index"
    );
}

/// Scenario: invoking `add` twice on the same path must not produce
/// duplicate index entries. Pins the idempotency invariant of the staging
/// pipeline.
#[tokio::test]
#[serial]
async fn test_add_duplicate_file_should_not_duplicate_index() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    let file_path = "dup_test.txt";
    let mut file = fs::File::create(file_path).unwrap();
    file.write_all(b"content").unwrap();

    // Add same file twice
    for i in 0..2 {
        add::execute(AddArgs {
            pathspec: vec![String::from(file_path)],
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

        // Check after each add operation
        let changes = changes_to_be_committed().await;
        let occurrences = changes
            .new
            .iter()
            .filter(|x| x.to_str().unwrap() == file_path)
            .count();
        assert_eq!(
            occurrences,
            1,
            "File should appear exactly once in index after {} add operation(s)",
            i + 1
        );
    }
}

/// Scenario: zero-byte files must be stageable. Regression guard against
/// "non-empty content required" assumptions in the blob hashing path.
#[tokio::test]
#[serial]
async fn test_add_empty_file() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // Create an empty file
    let file_path = "empty.txt";
    fs::File::create(file_path).unwrap();

    // Execute add command
    add::execute(AddArgs {
        pathspec: vec![String::from(file_path)],
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

    // Verify the empty file was added to index
    let changes = changes_to_be_committed().await;
    assert!(
        changes.new.iter().any(|x| x.to_str().unwrap() == file_path),
        "Empty file should be added to index"
    );
}

/// Scenario: deeply nested paths (`a/b/c/deep.txt`) must be staged with
/// their full repository-relative path. Path separators are normalized to
/// `/` so the test passes on Windows.
#[tokio::test]
#[serial]
async fn test_add_sub_directory_file() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // Create nested subdirectory structure
    let sub_dir = "a/b/c";
    fs::create_dir_all(sub_dir).unwrap();
    let file_path = "a/b/c/deep.txt";
    fs::write(file_path, "hello deep").unwrap();

    // Execute add command
    add::execute(AddArgs {
        pathspec: vec![String::from(file_path)],
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

    // Verify the file in nested directory was added to index
    let changes = changes_to_be_committed().await;
    assert!(
        changes
            .new
            .iter()
            .any(|x| x.to_str().unwrap().replace("\\", "/") == file_path),
        "File in nested subdirectory should be added to index"
    );
}

/// Scenario: `--sparse` is declined inside a repo with a friendly usage error
/// (CliInvalidTarget -> exit 129) rather than being silently ignored.
#[tokio::test]
#[serial]
async fn test_add_sparse_declined() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    let err = add::run_add(&AddArgs {
        sparse: true,
        ..Default::default()
    })
    .await
    .expect_err("--sparse must be declined inside a repo");
    assert_eq!(
        err.stable_code(),
        libra::utils::error::StableErrorCode::CliInvalidTarget
    );
    assert!(err.message().contains("sparse"), "msg: {}", err.message());
}

/// Scenario: outside a repository, `--sparse` first hits repo discovery and
/// returns RepoNotFound (128) — matching `git add --sparse` outside a repo.
#[tokio::test]
#[serial]
async fn test_add_sparse_outside_repo_is_repo_not_found() {
    let test_dir = tempdir().unwrap();
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    let err = add::run_add(&AddArgs {
        sparse: true,
        ..Default::default()
    })
    .await
    .expect_err("outside a repo, add must fail with RepoNotFound");
    assert_eq!(
        err.stable_code(),
        libra::utils::error::StableErrorCode::RepoNotFound
    );
}

/// Scenario: `-N`/`--intent-to-add` is declined (the on-disk index cannot model
/// an intent-to-add entry).
#[tokio::test]
#[serial]
async fn test_add_intent_to_add_declined() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    let err = add::run_add(&AddArgs {
        intent_to_add: true,
        ..Default::default()
    })
    .await
    .expect_err("-N must be declined");
    assert_eq!(
        err.stable_code(),
        libra::utils::error::StableErrorCode::CliInvalidTarget
    );
    assert!(
        err.message().contains("intent-to-add"),
        "msg: {}",
        err.message()
    );
}

/// Scenario: an invalid `--chmod` value is rejected at the run_add entry with
/// CliInvalidTarget before any staging happens.
#[tokio::test]
#[serial]
async fn test_add_chmod_rejects_invalid_value_via_run_add() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    let err = add::run_add(&AddArgs {
        chmod: Some("xyz".to_string()),
        pathspec: vec![".".to_string()],
        ..Default::default()
    })
    .await
    .expect_err("invalid --chmod must be rejected");
    assert_eq!(
        err.stable_code(),
        libra::utils::error::StableErrorCode::CliInvalidTarget
    );
}

/// Scenario: `add.ignoreErrors = true` in config makes `--ignore-errors` the
/// default when no CLI flag is given (tri-state precedence: config layer).
#[tokio::test]
#[serial]
async fn test_add_config_ignore_errors_default() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    libra::internal::config::ConfigKv::set("add.ignoreErrors", "true", false)
        .await
        .unwrap();
    assert!(
        add::resolve_ignore_errors(&AddArgs::default()).await,
        "config add.ignoreErrors=true should make ignore-errors the default"
    );
}

#[tokio::test]
#[serial]
async fn test_add_config_ignore_errors_lowercase_key_default() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    libra::internal::config::ConfigKv::set("add.ignoreerrors", "true", false)
        .await
        .unwrap();

    assert!(
        add::resolve_ignore_errors(&AddArgs::default()).await,
        "lowercase add.ignoreerrors should be treated like add.ignoreErrors"
    );
}

/// Scenario: an explicit CLI flag overrides the config default in both
/// directions (proves the `Option<bool>`-equivalent tri-state).
#[tokio::test]
#[serial]
async fn test_add_cli_overrides_config_ignore_errors() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    libra::internal::config::ConfigKv::set("add.ignoreErrors", "true", false)
        .await
        .unwrap();
    // Explicit --no-ignore-errors overrides config=true -> false.
    assert!(
        !add::resolve_ignore_errors(&AddArgs {
            no_ignore_errors: true,
            ..Default::default()
        })
        .await
    );
    // Explicit --ignore-errors is honored regardless of config -> true.
    assert!(
        add::resolve_ignore_errors(&AddArgs {
            ignore_errors: true,
            ..Default::default()
        })
        .await
    );
}

/// Read the recorded index mode for a stage-0 entry, reloading `.libra/index`
/// from disk so the assertion reflects the persisted (atomically saved) state.
fn index_mode(repo: &std::path::Path, name: &str) -> Option<u32> {
    let index = git_internal::internal::index::Index::load(repo.join(".libra/index")).ok()?;
    index.get(name, 0).map(|entry| entry.mode)
}

/// Scenario: `--chmod=+x` records index mode 0o100755 for a staged file.
#[tokio::test]
#[serial]
async fn test_add_chmod_plus_x() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());
    fs::write("script.sh", "#!/bin/sh\necho hi\n").unwrap();

    add::run_add(&AddArgs {
        pathspec: vec!["script.sh".to_string()],
        chmod: Some("+x".to_string()),
        ..Default::default()
    })
    .await
    .expect("add --chmod=+x");

    assert_eq!(index_mode(test_dir.path(), "script.sh"), Some(0o100755));
}

#[tokio::test]
#[serial]
async fn test_add_chmod_then_commit_tree_mode() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());
    configure_identity_via_cli(test_dir.path());
    fs::write("build.sh", "#!/bin/sh\necho hi\n").unwrap();

    add::run_add(&AddArgs {
        pathspec: vec!["build.sh".to_string()],
        chmod: Some("+x".to_string()),
        ..Default::default()
    })
    .await
    .expect("add --chmod=+x");

    let commit = run_libra_command(
        &["commit", "-m", "add executable", "--no-verify"],
        test_dir.path(),
    );
    assert_cli_success(&commit, "commit executable");

    let head = Head::current_commit().await.expect("HEAD should exist");
    let commit: Commit = load_object(&head).expect("HEAD commit should load");
    let tree: Tree = load_object(&commit.tree_id).expect("HEAD tree should load");
    let item = tree
        .tree_items
        .iter()
        .find(|item| item.name == "build.sh")
        .expect("tree should contain build.sh");
    assert_eq!(item.mode, TreeItemMode::BlobExecutable);
}

/// Scenario: `--chmod=-x` records index mode 0o100644 even when the worktree
/// file is executable (only the index mode is changed).
#[tokio::test]
#[serial]
async fn test_add_chmod_minus_x() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());
    fs::write("script.sh", "#!/bin/sh\necho hi\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions("script.sh", fs::Permissions::from_mode(0o755)).unwrap();
    }

    add::run_add(&AddArgs {
        pathspec: vec!["script.sh".to_string()],
        chmod: Some("-x".to_string()),
        ..Default::default()
    })
    .await
    .expect("add --chmod=-x");

    assert_eq!(index_mode(test_dir.path(), "script.sh"), Some(0o100644));
}

/// Scenario: `--chmod` updates an already-tracked entry whose content/stat is
/// unchanged — proving the candidate set includes entries outside the
/// status-change set (`git add --chmod` semantics).
#[tokio::test]
#[serial]
async fn test_add_chmod_unchanged_tracked() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());
    fs::write("tracked.sh", "#!/bin/sh\n").unwrap();

    // First stage: entry exists with the default (non-executable) mode.
    add::run_add(&AddArgs {
        pathspec: vec!["tracked.sh".to_string()],
        ..Default::default()
    })
    .await
    .expect("initial add");
    assert_eq!(index_mode(test_dir.path(), "tracked.sh"), Some(0o100644));

    // No worktree change; --chmod must still flip the recorded index mode.
    add::run_add(&AddArgs {
        pathspec: vec!["tracked.sh".to_string()],
        chmod: Some("+x".to_string()),
        ..Default::default()
    })
    .await
    .expect("chmod on unchanged tracked file");
    assert_eq!(index_mode(test_dir.path(), "tracked.sh"), Some(0o100755));
}

/// Scenario: `--dry-run --chmod` previews a mode-only change for an unchanged
/// tracked file without writing the index.
#[tokio::test]
#[serial]
async fn test_add_chmod_dry_run_previews_unchanged_tracked() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());
    fs::write("tracked.sh", "#!/bin/sh\n").unwrap();

    add::run_add(&AddArgs {
        pathspec: vec!["tracked.sh".to_string()],
        ..Default::default()
    })
    .await
    .expect("initial add");
    assert_eq!(index_mode(test_dir.path(), "tracked.sh"), Some(0o100644));

    let out = add::run_add(&AddArgs {
        pathspec: vec!["tracked.sh".to_string()],
        chmod: Some("+x".to_string()),
        dry_run: true,
        ..Default::default()
    })
    .await
    .expect("dry-run chmod on unchanged tracked file");

    assert!(
        out.modified.iter().any(|p| p == "tracked.sh"),
        "dry-run should preview the mode-only change: {:?}",
        out.modified
    );
    assert_eq!(
        index_mode(test_dir.path(), "tracked.sh"),
        Some(0o100644),
        "dry-run must not write the chmod mode to the index"
    );
}

/// Scenario: `--chmod` changes only the index mode, never the working-tree
/// file's filesystem permissions.
#[cfg(unix)]
#[tokio::test]
#[serial]
async fn test_add_chmod_does_not_touch_worktree_perms() {
    use std::os::unix::fs::PermissionsExt;

    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());
    fs::write("script.sh", "#!/bin/sh\n").unwrap();
    fs::set_permissions("script.sh", fs::Permissions::from_mode(0o644)).unwrap();
    let before = fs::metadata("script.sh").unwrap().permissions().mode();

    add::run_add(&AddArgs {
        pathspec: vec!["script.sh".to_string()],
        chmod: Some("+x".to_string()),
        ..Default::default()
    })
    .await
    .expect("add --chmod=+x");

    let after = fs::metadata("script.sh").unwrap().permissions().mode();
    assert_eq!(before, after, "worktree file permissions must be unchanged");
    assert_eq!(index_mode(test_dir.path(), "script.sh"), Some(0o100755));
}

/// Scenario: under `core.fileMode = false`, `--chmod` still records the index
/// mode but emits a warning (driving `--exit-code-on-warning`).
#[tokio::test]
#[serial]
async fn test_add_chmod_core_filemode_false_warns() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());
    fs::write("script.sh", "#!/bin/sh\n").unwrap();
    libra::internal::config::ConfigKv::set("core.filemode", "false", false)
        .await
        .unwrap();

    libra::utils::output::reset_warning_tracker();
    add::run_add(&AddArgs {
        pathspec: vec!["script.sh".to_string()],
        chmod: Some("+x".to_string()),
        ..Default::default()
    })
    .await
    .expect("add --chmod=+x under core.fileMode=false");

    assert!(
        libra::utils::output::warning_was_emitted(),
        "a warning must be emitted when core.fileMode is false"
    );
    assert_eq!(index_mode(test_dir.path(), "script.sh"), Some(0o100755));
}

#[tokio::test]
#[serial]
async fn test_add_chmod_core_filemode_camel_case_warns() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());
    fs::write("script.sh", "#!/bin/sh\n").unwrap();
    libra::internal::config::ConfigKv::set("core.fileMode", "false", false)
        .await
        .unwrap();

    libra::utils::output::reset_warning_tracker();
    add::run_add(&AddArgs {
        pathspec: vec!["script.sh".to_string()],
        chmod: Some("+x".to_string()),
        ..Default::default()
    })
    .await
    .expect("add --chmod=+x under core.fileMode=false");

    assert!(
        libra::utils::output::warning_was_emitted(),
        "camel-case core.fileMode should be treated like core.filemode"
    );
}

/// Scenario (Wave 1 prerequisite regression): with the fallible blob path,
/// `--ignore-errors` skips an unreadable file instead of panicking, and still
/// stages the readable ones.
#[cfg(unix)]
#[tokio::test]
#[serial]
async fn test_add_ignore_errors_skips_unreadable() {
    use std::os::unix::fs::PermissionsExt;

    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());
    fs::write("readable.txt", "ok").unwrap();
    fs::write("secret.txt", "nope").unwrap();
    fs::set_permissions("secret.txt", fs::Permissions::from_mode(0o000)).unwrap();

    let out = add::run_add(&AddArgs {
        pathspec: vec!["readable.txt".to_string(), "secret.txt".to_string()],
        ignore_errors: true,
        ..Default::default()
    })
    .await
    .expect("--ignore-errors must not abort on an unreadable file");

    // restore perms so the tempdir can be cleaned up
    fs::set_permissions("secret.txt", fs::Permissions::from_mode(0o644)).unwrap();

    assert!(
        out.added.iter().any(|p| p == "readable.txt"),
        "readable file should be staged: {:?}",
        out.added
    );
    assert!(
        out.failed.iter().any(|f| f.path == "secret.txt"),
        "unreadable file should be reported as failed: {:?}",
        out.failed
    );
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn test_add_config_ignore_errors_skips_unreadable() {
    use std::os::unix::fs::PermissionsExt;

    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());
    fs::write("readable.txt", "ok").unwrap();
    fs::write("secret.txt", "nope").unwrap();
    fs::set_permissions("secret.txt", fs::Permissions::from_mode(0o000)).unwrap();
    libra::internal::config::ConfigKv::set("add.ignoreerrors", "true", false)
        .await
        .unwrap();

    let out = add::run_add(&AddArgs {
        pathspec: vec!["readable.txt".to_string(), "secret.txt".to_string()],
        ..Default::default()
    })
    .await
    .expect("configured ignore-errors must not abort on an unreadable file");

    fs::set_permissions("secret.txt", fs::Permissions::from_mode(0o644)).unwrap();

    assert!(
        out.added.iter().any(|p| p == "readable.txt"),
        "readable file should be staged: {:?}",
        out.added
    );
    assert!(
        out.failed.iter().any(|f| f.path == "secret.txt"),
        "unreadable file should be reported as failed: {:?}",
        out.failed
    );
}

/// Scenario: an atomic index save that fails (here: the target directory is
/// read-only, so the temp-file write fails) leaves the existing index file
/// byte-identical — no partial write.
#[cfg(unix)]
#[test]
fn test_add_index_save_failure_no_partial() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempdir().unwrap();
    let index_path = dir.path().join("index");
    fs::write(&index_path, b"ORIGINAL-INDEX-SENTINEL-BYTES").unwrap();
    let original = fs::read(&index_path).unwrap();

    // An empty in-memory index we would attempt to persist over the sentinel.
    let index = git_internal::internal::index::Index::load(dir.path().join("nonexistent"))
        .expect("missing index loads as empty");

    // Make the directory read-only so creating the sibling temp file fails.
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o500)).unwrap();
    let result = add::save_index_atomic(&index, &index_path);
    // Restore before asserting so the tempdir can be cleaned up.
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();

    assert!(
        result.is_err(),
        "atomic save into a read-only dir must fail"
    );
    assert_eq!(
        fs::read(&index_path).unwrap(),
        original,
        "the original index must be untouched after a failed atomic save"
    );
}

/// Scenario (regression): `--chmod=+x` on a brand-new file must report the path
/// once (as a new file), not double-count it as both added and modified.
#[tokio::test]
#[serial]
async fn test_add_chmod_new_file_reported_once() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());
    fs::write("script.sh", "#!/bin/sh\n").unwrap();

    let out = add::run_add(&AddArgs {
        pathspec: vec!["script.sh".to_string()],
        chmod: Some("+x".to_string()),
        ..Default::default()
    })
    .await
    .expect("add --chmod=+x on a new file");

    assert!(
        out.added.iter().any(|p| p == "script.sh"),
        "new file must be in `added`: {:?}",
        out.added
    );
    assert!(
        !out.modified.iter().any(|p| p == "script.sh"),
        "new file must NOT also be in `modified` (no double count): {:?}",
        out.modified
    );
    // And the recorded index mode is still the executable bit.
    assert_eq!(index_mode(test_dir.path(), "script.sh"), Some(0o100755));
}

/// Scenario: `--pathspec-from-file` stages every path listed in the file.
#[tokio::test]
#[serial]
async fn test_add_pathspec_from_file() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());
    fs::write("a.txt", "a").unwrap();
    fs::write("b.txt", "b").unwrap();
    fs::write("list.txt", "a.txt\nb.txt\n").unwrap();

    let out = add::run_add(&AddArgs {
        pathspec_from_file: Some("list.txt".to_string()),
        ..Default::default()
    })
    .await
    .expect("add --pathspec-from-file");

    assert!(out.added.iter().any(|p| p == "a.txt"), "{:?}", out.added);
    assert!(out.added.iter().any(|p| p == "b.txt"), "{:?}", out.added);
    // The list file itself was not listed, so it is not staged.
    assert!(
        !out.added.iter().any(|p| p == "list.txt"),
        "{:?}",
        out.added
    );
}

/// Scenario: `--pathspec-file-nul` parses NUL-separated input.
#[tokio::test]
#[serial]
async fn test_add_pathspec_from_file_nul() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());
    fs::write("a.txt", "a").unwrap();
    fs::write("b.txt", "b").unwrap();
    fs::write("list.txt", "a.txt\0b.txt\0").unwrap();

    let out = add::run_add(&AddArgs {
        pathspec_from_file: Some("list.txt".to_string()),
        pathspec_file_nul: true,
        ..Default::default()
    })
    .await
    .expect("add --pathspec-from-file --pathspec-file-nul");

    assert!(out.added.iter().any(|p| p == "a.txt"), "{:?}", out.added);
    assert!(out.added.iter().any(|p| p == "b.txt"), "{:?}", out.added);
}

/// Scenario: `--pathspec-from-file=-` reads newline-separated pathspecs from
/// stdin through the real binary and stops at EOF.
#[tokio::test]
#[serial]
async fn test_add_pathspec_from_file_stdin() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    fs::write(test_dir.path().join("a.txt"), "a").unwrap();
    fs::write(test_dir.path().join("b.txt"), "b").unwrap();

    let output = run_libra_command_with_stdin(
        &["add", "--pathspec-from-file", "-"],
        test_dir.path(),
        "a.txt\nb.txt\n",
    );
    assert_cli_success(&output, "add --pathspec-from-file=-");

    let status = run_libra_command(&["status", "--short"], test_dir.path());
    let stdout = String::from_utf8_lossy(&status.stdout);
    assert!(stdout.contains("A  a.txt"), "status was: {stdout}");
    assert!(stdout.contains("A  b.txt"), "status was: {stdout}");
}

/// Scenario: a missing `--pathspec-from-file` returns a read error (IoReadFailed)
/// whose message names the file.
#[tokio::test]
#[serial]
async fn test_add_pathspec_from_file_missing() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    let err = add::run_add(&AddArgs {
        pathspec_from_file: Some("nope.txt".to_string()),
        ..Default::default()
    })
    .await
    .expect_err("missing pathspec file must error");
    assert_eq!(
        err.stable_code(),
        libra::utils::error::StableErrorCode::IoReadFailed
    );
    assert!(err.message().contains("nope.txt"), "msg: {}", err.message());
}

/// Scenario: a `--pathspec-from-file` larger than 128 MiB is rejected without
/// reading it into memory (uses a sparse file so the test stays cheap).
#[tokio::test]
#[serial]
async fn test_add_pathspec_from_file_oversize() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());
    let f = fs::File::create("big.txt").unwrap();
    f.set_len(128 * 1024 * 1024 + 1).unwrap();
    drop(f);

    let err = add::run_add(&AddArgs {
        pathspec_from_file: Some("big.txt".to_string()),
        ..Default::default()
    })
    .await
    .expect_err("oversize pathspec file must error");
    assert_eq!(
        err.stable_code(),
        libra::utils::error::StableErrorCode::IoReadFailed
    );
}

/// Scenario: a `../escape` entry in a `--pathspec-from-file` is rejected as
/// outside the repository (CliInvalidTarget), not silently resolved.
#[tokio::test]
#[serial]
async fn test_add_pathspec_from_file_traversal_blocked() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());
    fs::write("list.txt", "../escape.txt\n").unwrap();

    let err = add::run_add(&AddArgs {
        pathspec_from_file: Some("list.txt".to_string()),
        ..Default::default()
    })
    .await
    .expect_err("traversal pathspec must be rejected");
    assert_eq!(
        err.stable_code(),
        libra::utils::error::StableErrorCode::CliInvalidTarget
    );
}

/// Scenario: `--dry-run --ignore-missing` skips a path that does not exist in
/// the working tree (with a warning) instead of erroring, and still previews
/// the present paths.
#[tokio::test]
#[serial]
async fn test_add_ignore_missing_dry_run_skips_missing() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());
    fs::write("real.txt", "x").unwrap();

    let out = add::run_add(&AddArgs {
        pathspec: vec!["real.txt".to_string(), "ghost.txt".to_string()],
        dry_run: true,
        ignore_missing: true,
        ..Default::default()
    })
    .await
    .expect("--dry-run --ignore-missing must skip the missing path");
    assert!(out.added.iter().any(|p| p == "real.txt"), "{:?}", out.added);
}

#[tokio::test]
#[serial]
async fn test_add_ignore_missing_exit_code_on_warning() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;

    let out = run_libra_command(
        &[
            "add",
            "--dry-run",
            "--ignore-missing",
            "--exit-code-on-warning",
            "ghost.txt",
        ],
        test_dir.path(),
    );

    assert_eq!(
        out.status.code(),
        Some(9),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Scenario: the in-process API must enforce the same Git/clap contract as the
/// CLI: `--ignore-missing` is only legal with `--dry-run`.
#[tokio::test]
#[serial]
async fn test_add_ignore_missing_requires_dry_run_via_run_add() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    let err = add::run_add(&AddArgs {
        pathspec: vec!["ghost.txt".to_string()],
        ignore_missing: true,
        ..Default::default()
    })
    .await
    .expect_err("run_add must reject --ignore-missing without --dry-run");
    assert_eq!(
        err.stable_code(),
        libra::utils::error::StableErrorCode::CliInvalidArguments
    );
}

/// Scenario: under `--ignore-missing`, a path that EXISTS but matches nothing
/// (an empty directory) still errors with PathspecNotMatched.
#[tokio::test]
#[serial]
async fn test_add_ignore_missing_still_errors_present_unmatched() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());
    fs::create_dir("emptydir").unwrap();

    let err = add::run_add(&AddArgs {
        pathspec: vec!["emptydir".to_string()],
        dry_run: true,
        ignore_missing: true,
        ..Default::default()
    })
    .await
    .expect_err("present-but-unmatched path must still error");
    assert_eq!(
        err.stable_code(),
        libra::utils::error::StableErrorCode::CliInvalidTarget
    );
}

/// Scenario: `--dry-run` WITHOUT `--ignore-missing` still errors on a missing
/// path (the relaxation is opt-in).
#[tokio::test]
#[serial]
async fn test_add_missing_dry_run_without_ignore_missing() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    let err = add::run_add(&AddArgs {
        pathspec: vec!["ghost.txt".to_string()],
        dry_run: true,
        ..Default::default()
    })
    .await
    .expect_err("missing path without --ignore-missing must error");
    assert_eq!(
        err.stable_code(),
        libra::utils::error::StableErrorCode::CliInvalidTarget
    );
}

/// Scenario: `--renormalize` (implies `-u`) never stages an untracked file.
#[tokio::test]
#[serial]
async fn test_add_renormalize_only_tracked() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());
    fs::write("tracked.txt", "v1").unwrap();
    add::run_add(&AddArgs {
        pathspec: vec!["tracked.txt".to_string()],
        ..Default::default()
    })
    .await
    .expect("stage tracked.txt");
    fs::write("untracked.txt", "new").unwrap();

    let out = add::run_add(&AddArgs {
        renormalize: true,
        ..Default::default()
    })
    .await
    .expect("renormalize");

    assert!(
        !out.added.iter().any(|p| p == "untracked.txt")
            && !out.modified.iter().any(|p| p == "untracked.txt"),
        "untracked file must not be staged by --renormalize: added={:?} modified={:?}",
        out.added,
        out.modified
    );
}

/// Scenario: `--renormalize` stages the deletion of a tracked file removed from
/// the working tree (the `-u` part).
#[tokio::test]
#[serial]
async fn test_add_renormalize_stages_tracked_deletion() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());
    fs::write("tracked.txt", "v1").unwrap();
    add::run_add(&AddArgs {
        pathspec: vec!["tracked.txt".to_string()],
        ..Default::default()
    })
    .await
    .expect("stage tracked.txt");
    fs::remove_file("tracked.txt").unwrap();

    let out = add::run_add(&AddArgs {
        pathspec: vec!["tracked.txt".to_string()],
        renormalize: true,
        ..Default::default()
    })
    .await
    .expect("renormalize a deleted tracked file");

    assert!(
        out.removed.iter().any(|p| p == "tracked.txt"),
        "deletion should be staged: {:?}",
        out.removed
    );
}

/// Scenario (direct): `renormalize_entry` rewrites an unchanged tracked entry,
/// bypassing the unchanged/verify-hash short-circuit (returns `Modified`).
#[tokio::test]
#[serial]
async fn test_add_renormalize_rewrites_unchanged_tracked() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());
    fs::write("tracked.txt", "v1").unwrap();
    add::run_add(&AddArgs {
        pathspec: vec!["tracked.txt".to_string()],
        ..Default::default()
    })
    .await
    .expect("stage tracked.txt");

    // Content/stat unchanged; renormalize_entry must still report a rewrite.
    let mut index =
        git_internal::internal::index::Index::load(test_dir.path().join(".libra/index"))
            .expect("load index");
    let action = add::renormalize_entry(
        std::path::Path::new("tracked.txt"),
        &mut index,
        test_dir.path(),
    )
    .await
    .expect("renormalize_entry");
    assert_eq!(action, add::StagedAction::Modified);
}

/// Scenario: `--dry-run --renormalize` previews the tracked entries that would
/// be rewritten, instead of the (often empty) status-change set. Regression for
/// the Codex finding that dry-run ignored the renormalize candidate set.
#[tokio::test]
#[serial]
async fn test_add_renormalize_dry_run_previews_tracked() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());
    fs::write("tracked.txt", "v1").unwrap();
    add::run_add(&AddArgs {
        pathspec: vec!["tracked.txt".to_string()],
        ..Default::default()
    })
    .await
    .expect("stage tracked.txt");

    // Content/stat unchanged: dry-run --renormalize must still preview it.
    let out = add::run_add(&AddArgs {
        renormalize: true,
        dry_run: true,
        ..Default::default()
    })
    .await
    .expect("dry-run renormalize");
    assert!(
        out.modified.iter().any(|p| p == "tracked.txt"),
        "dry-run should preview the renormalize rewrite: {:?}",
        out.modified
    );
}
