//! Tests add command behavior for staging files, refresh operations, and edge cases.

use std::{fs, io::Write};

use super::*;

#[tokio::test]
#[serial]
/// Tests the basic functionality of add command by adding a single file
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

#[tokio::test]
#[serial]
/// Tests adding multiple files at once
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

#[tokio::test]
#[serial]
/// Tests the --all flag which adds all files in the working tree
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

#[tokio::test]
#[serial]
/// Tests the --update flag which only updates files already in the index
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
    let changes = changes_to_be_staged();
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

#[tokio::test]
#[serial]
/// Tests adding files with respect to ignore patterns in .libraignore
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
    let changes_staged = changes_to_be_staged();
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

#[tokio::test]
#[serial]
/// Ensures `add --force` stages ignored files and subsequent updates no longer require force.
async fn test_add_force_tracks_ignored_file() {
    let repo = tempdir().unwrap();
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = test::ChangeDirGuard::new(repo.path());

    fs::write(".libraignore", "ignored.txt\n").unwrap();
    fs::write("ignored.txt", "first").unwrap();

    let ignored_path = "ignored.txt";

    // Without --force the ignored file should stay hidden from staging
    let unstaged_initial = changes_to_be_staged();
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

    let unstaged_after_edit = changes_to_be_staged();
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

    let unstaged_final = changes_to_be_staged();
    assert!(
        !unstaged_final
            .modified
            .iter()
            .any(|p| p.to_str().unwrap() == ignored_path)
    );
}

#[tokio::test]
#[serial]
/// Ensures `add --force .` surfaces ignored directories and files recursively.
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

#[tokio::test]
#[serial]
/// Tests the dry-run flag which should not actually add files
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
    let changes = changes_to_be_staged();
    assert!(changes.new.iter().any(|x| x.to_str().unwrap() == file_path));
}

#[tokio::test]
#[serial]
/// Tests that running add without specifying files or --all should not modify index
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

#[tokio::test]
#[serial]
/// Tests adding a file that does not exist should produce an error
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

#[tokio::test]
#[serial]
/// Tests adding the same file twice should not create duplicates in the index
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

#[tokio::test]
#[serial]
/// Tests adding an empty file to the repository
///
/// Ensures that Libra can handle adding empty files without errors
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

#[tokio::test]
#[serial]
/// Tests adding a file in a nested subdirectory
///
/// Ensures that Libra correctly handles files in deep directory structures
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
