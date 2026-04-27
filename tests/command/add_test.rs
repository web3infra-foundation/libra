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
    })
    .await;

    // Verify the file was added to index.
    let changes = changes_to_be_committed().await;

    assert!(changes.new.iter().any(|x| x.to_str().unwrap() == file_path));
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
