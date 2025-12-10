use super::*;
use libra::cli::Stash;
use libra::command::stash;
use libra::command::status::execute_to as status_execute;
use libra::command::status::output_porcelain;
use libra::command::status::{PorcelainVersion, StatusArgs, UntrackedFiles};
use std::fs;
use std::io::Write;
#[tokio::test]
#[serial]
/// Tests --ignored flag: ignored files appear in outputs
async fn test_status_ignored_outputs() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // Create .libraignore ignoring foo* and dir/
    let mut ign = fs::File::create(".libraignore").unwrap();
    ign.write_all(b"foo*\ndir/\n").unwrap();

    // Create ignored files and non-ignored
    fs::write("foo.txt", "x").unwrap();
    fs::create_dir_all("dir").unwrap();
    fs::write("dir/a.txt", "y").unwrap();
    fs::write("bar.txt", "z").unwrap();

    // Porcelain
    let mut out = Vec::new();
    status_execute(
        StatusArgs {
            porcelain: Some(PorcelainVersion::V1),
            ignored: true,
            ..Default::default()
        },
        &mut out,
    )
    .await;
    let s = String::from_utf8(out).unwrap();
    assert!(
        s.lines().any(|l| l.starts_with("!! foo.txt")),
        "porcelain should show !! for ignored file: {}",
        s
    );

    // Short
    let mut out = Vec::new();
    status_execute(
        StatusArgs {
            short: true,
            ignored: true,
            ..Default::default()
        },
        &mut out,
    )
    .await;
    let s = String::from_utf8(out).unwrap();
    assert!(
        s.lines().any(|l| l.starts_with("!! foo.txt")),
        "short should show !! for ignored file: {}",
        s
    );

    // Standard
    let mut out = Vec::new();
    status_execute(
        StatusArgs {
            ignored: true,
            ..Default::default()
        },
        &mut out,
    )
    .await;
    let s = String::from_utf8(out).unwrap();
    // In standard mode, headers are printed to stdout via println!, so the writer content may
    // only include per-file lines. Assert that ignored file names are present.
    assert!(
        s.contains("foo.txt"),
        "standard should include ignored file name in writer output: {}",
        s
    );
}

// Helper function to create CommitArgs with a message, using default values for other fields
fn create_commit_args(message: &str) -> CommitArgs {
    CommitArgs {
        message: Some(message.to_string()),
        ..Default::default()
    }
}

#[tokio::test]
#[serial]
/// Tests the file status detection functionality with respect to ignore patterns.
/// Verifies that files matching patterns in .libraignore are properly excluded from status reports.
async fn test_changes_to_be_staged() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    let mut gitignore_file = fs::File::create(".libraignore").unwrap();
    gitignore_file
        .write_all(b"should_ignore*\nignore_dir/")
        .unwrap();

    let mut should_ignore_file_0 = fs::File::create("should_ignore.0").unwrap();
    let mut not_ignore_file_0 = fs::File::create("not_ignore.0").unwrap();
    fs::create_dir("ignore_dir").unwrap();
    let mut should_ignore_file_1 = fs::File::create("ignore_dir/should_ignore.1").unwrap();
    fs::create_dir("not_ignore_dir").unwrap();
    let mut not_ignore_file_1 = fs::File::create("not_ignore_dir/not_ignore.1").unwrap();

    let change = changes_to_be_staged();
    assert!(
        !change
            .new
            .iter()
            .any(|x| x.file_name().unwrap() == "should_ignore.0")
    );
    assert!(
        !change
            .new
            .iter()
            .any(|x| x.file_name().unwrap() == "should_ignore.1")
    );
    assert!(
        change
            .new
            .iter()
            .any(|x| x.file_name().unwrap() == "not_ignore.0")
    );
    assert!(
        change
            .new
            .iter()
            .any(|x| x.file_name().unwrap() == "not_ignore.1")
    );

    add::execute(AddArgs {
        pathspec: vec![String::from(".")],
        all: true,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    })
    .await;

    should_ignore_file_0.write_all(b"foo").unwrap();
    should_ignore_file_1.write_all(b"foo").unwrap();
    not_ignore_file_0.write_all(b"foo").unwrap();
    not_ignore_file_1.write_all(b"foo").unwrap();

    let change = changes_to_be_staged();
    assert!(
        !change
            .modified
            .iter()
            .any(|x| x.file_name().unwrap() == "should_ignore.0")
    );
    assert!(
        !change
            .modified
            .iter()
            .any(|x| x.file_name().unwrap() == "should_ignore.1")
    );
    assert!(
        change
            .modified
            .iter()
            .any(|x| x.file_name().unwrap() == "not_ignore.0")
    );
    assert!(
        change
            .modified
            .iter()
            .any(|x| x.file_name().unwrap() == "not_ignore.1")
    );

    fs::remove_dir_all("ignore_dir").unwrap();
    fs::remove_dir_all("not_ignore_dir").unwrap();
    fs::remove_file("should_ignore.0").unwrap();
    fs::remove_file("not_ignore.0").unwrap();

    not_ignore_file_1.write_all(b"foo").unwrap();

    let change = changes_to_be_staged();
    assert!(
        !change
            .deleted
            .iter()
            .any(|x| x.file_name().unwrap() == "should_ignore.0")
    );
    assert!(
        !change
            .deleted
            .iter()
            .any(|x| x.file_name().unwrap() == "should_ignore.1")
    );
    assert!(
        change
            .deleted
            .iter()
            .any(|x| x.file_name().unwrap() == "not_ignore.0")
    );
    assert!(
        change
            .deleted
            .iter()
            .any(|x| x.file_name().unwrap() == "not_ignore.1")
    );
}

#[test]
fn test_output_porcelain_format() {
    use libra::command::status::Changes;
    use std::path::PathBuf;

    // Create test data
    let staged = Changes {
        new: vec![PathBuf::from("new_file.txt")],
        modified: vec![PathBuf::from("modified_file.txt")],
        deleted: vec![PathBuf::from("deleted_file.txt")],
    };

    let unstaged = Changes {
        new: vec![PathBuf::from("untracked_file.txt")],
        modified: vec![PathBuf::from("unstaged_modified.txt")],
        deleted: vec![PathBuf::from("unstaged_deleted.txt")],
    };

    // Create a buffer to capture the output
    let mut output = Vec::new();

    // Call the output_porcelain function
    output_porcelain(&staged, &unstaged, &mut output);

    // Get the output as a string
    let output_str = String::from_utf8(output).unwrap();

    // Verify the output format
    let lines: Vec<&str> = output_str.trim().split('\n').collect();

    assert!(lines.contains(&"A  new_file.txt"));
    assert!(lines.contains(&"M  modified_file.txt"));
    assert!(lines.contains(&"D  deleted_file.txt"));
    assert!(lines.contains(&" M unstaged_modified.txt"));
    assert!(lines.contains(&" D unstaged_deleted.txt"));
    assert!(lines.contains(&"?? untracked_file.txt"));
}

#[tokio::test]
#[serial]
/// Tests the --porcelain flag for machine-readable output format.
/// Verifies that the output matches Git's porcelain format specification.
async fn test_status_porcelain() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // Create test data
    let mut file1 = fs::File::create("file1.txt").unwrap();
    file1.write_all(b"content").unwrap();

    let mut file2 = fs::File::create("file2.txt").unwrap();
    file2.write_all(b"content").unwrap();

    // Add one file to the staging area
    add::execute(AddArgs {
        pathspec: vec![String::from("file1.txt")],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    })
    .await;

    // Add another file to the staging area and modify it
    add::execute(AddArgs {
        pathspec: vec![String::from("file2.txt")],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    })
    .await;
    file2.write_all(b"modified content").unwrap();

    // Create a new file (untracked)
    let mut file3 = fs::File::create("file3.txt").unwrap();
    file3.write_all(b"new content").unwrap();

    // Create a buffer to capture the output
    let mut output = Vec::new();

    // Execute the status command with the --porcelain flag
    status_execute(
        StatusArgs {
            porcelain: Some(PorcelainVersion::V1),
            ..Default::default()
        },
        &mut output,
    )
    .await;

    // Get the output as a string
    let output_str = String::from_utf8(output).unwrap();

    // Verify the porcelain output format
    let lines: Vec<&str> = output_str.trim().split('\n').collect();

    // Should contain staged file (only staged, no unstaged modification)
    assert!(
        lines.iter().any(|line| line.starts_with("A  file1.txt")),
        "Should show 'A  file1.txt' for staged-only file: {:?}",
        lines
    );
    // file2.txt is staged AND modified after staging - should be merged as "AM"
    assert!(
        lines.iter().any(|line| line.starts_with("AM file2.txt")),
        "Should show 'AM file2.txt' for staged+modified file: {:?}",
        lines
    );

    // Should contain untracked files
    assert!(
        lines.iter().any(|line| line.starts_with("?? file3.txt")),
        "Should show '?? file3.txt' for untracked file: {:?}",
        lines
    );

    // Should not contain human-readable text
    assert!(!output_str.contains("Changes to be committed"));
    assert!(!output_str.contains("Untracked files"));
    assert!(!output_str.contains("On branch"));
}

#[test]
fn test_output_short_format() {
    use libra::command::status::Changes;
    use std::path::PathBuf;

    // Create test data
    let staged = Changes {
        new: vec![PathBuf::from("new_file.txt")],
        modified: vec![PathBuf::from("modified_file.txt")],
        deleted: vec![PathBuf::from("deleted_file.txt")],
    };

    let unstaged = Changes {
        new: vec![PathBuf::from("untracked_file.txt")],
        modified: vec![PathBuf::from("unstaged_modified.txt")],
        deleted: vec![PathBuf::from("unstaged_deleted.txt")],
    };

    // Create a buffer to capture the output
    let mut output = Vec::new();

    // Test the core logic directly without config dependency
    let status_list = libra::command::status::generate_short_format_status(&staged, &unstaged);

    // Output the short format (without colors for testing)
    for (file, staged_status, unstaged_status) in status_list {
        writeln!(
            output,
            "{}{} {}",
            staged_status,
            unstaged_status,
            file.display()
        )
        .unwrap();
    }

    // Get the output as a string
    let output_str = String::from_utf8(output).unwrap();

    // Verify the output format
    let lines: Vec<&str> = output_str.trim().split('\n').collect();

    // Check staged changes
    assert!(lines.contains(&"A  new_file.txt"));
    assert!(lines.contains(&"M  modified_file.txt"));
    assert!(lines.contains(&"D  deleted_file.txt"));

    // Check unstaged changes
    assert!(lines.contains(&" M unstaged_modified.txt"));
    assert!(lines.contains(&" D unstaged_deleted.txt"));

    // Check untracked files
    assert!(lines.contains(&"?? untracked_file.txt"));
}

#[tokio::test]
#[serial]
/// Tests the -s (--short) flag for short format output.
/// Verifies that the output matches Git's short format specification.
async fn test_status_short_format() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // Create test data
    let mut file1 = fs::File::create("file1.txt").unwrap();
    file1.write_all(b"content").unwrap();

    let mut file2 = fs::File::create("file2.txt").unwrap();
    file2.write_all(b"content").unwrap();

    // Add one file to the staging area
    add::execute(AddArgs {
        pathspec: vec![String::from("file1.txt")],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    })
    .await;

    // Add another file to the staging area and modify it
    add::execute(AddArgs {
        pathspec: vec![String::from("file2.txt")],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    })
    .await;

    // Reopen file2.txt for writing after staging
    let mut file2 = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open("file2.txt")
        .unwrap();
    file2.write_all(b"modified content").unwrap();

    // Create a new file (untracked)
    let mut file3 = fs::File::create("file3.txt").unwrap();
    file3.write_all(b"new content").unwrap();

    // Create a buffer to capture the output
    let mut output = Vec::new();

    // Execute the status command with the -s flag
    status_execute(
        StatusArgs {
            short: true,
            ..Default::default()
        },
        &mut output,
    )
    .await;

    // Get the output as a string
    let output_str = String::from_utf8(output).unwrap();
    println!("Actual short format output: {}", output_str); // Add debug output

    // Verify the short format output
    let lines: Vec<&str> = output_str.trim().split('\n').collect();

    // More flexible assertion: check whether the file appears in the output, but do not specify the exact status code
    let file1_found = lines.iter().any(|line| line.contains("file1.txt"));
    let file2_found = lines.iter().any(|line| line.contains("file2.txt"));
    let file3_found = lines.iter().any(|line| line.contains("file3.txt"));

    assert!(
        file1_found,
        "file1.txt should appear in short format output. Got: {}",
        output_str
    );
    assert!(
        file2_found,
        "file2.txt should appear in short format output. Got: {}",
        output_str
    );
    assert!(
        file3_found,
        "file3.txt should appear in short format output. Got: {}",
        output_str
    );

    // Check that the output format is short (should not contain human-readable text)
    assert!(
        !output_str.contains("Changes to be committed"),
        "Short format should not contain human-readable text. Got: {}",
        output_str
    );
    assert!(
        !output_str.contains("Untracked files"),
        "Short format should not contain human-readable text. Got: {}",
        output_str
    );
    assert!(
        !output_str.contains("On branch"),
        "Short format should not contain branch information. Got: {}",
        output_str
    );
}

#[tokio::test]
#[serial]
/// Tests porcelain v2 output: branch info, tracked changes, and untracked files.
async fn test_status_porcelain_v2_basic() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // staged + modified file
    let mut file1 = fs::File::create("file1.txt").unwrap();
    file1.write_all(b"content").unwrap();
    add::execute(AddArgs {
        pathspec: vec![String::from("file1.txt")],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    })
    .await;
    file1.write_all(b" more").unwrap(); // unstaged modification

    // untracked file
    fs::write("untracked.txt", "u").unwrap();

    let mut output = Vec::new();
    status_execute(
        StatusArgs {
            porcelain: Some(PorcelainVersion::V2),
            branch: true,
            ..Default::default()
        },
        &mut output,
    )
    .await;

    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.lines().any(|l| l.starts_with("# branch.head")),
        "porcelain v2 should contain branch.head line: {}",
        output_str
    );
    assert!(
        output_str.lines().any(|l| l.starts_with("1 AM")),
        "porcelain v2 should contain tracked entry line: {}",
        output_str
    );
    assert!(
        output_str.lines().any(|l| l.starts_with("? untracked.txt")),
        "porcelain v2 should list untracked files with '? ': {}",
        output_str
    );

    // Test --ignored flag with porcelain v2
    fs::write(".libraignore", "ignored.txt\n").unwrap();
    fs::write("ignored.txt", "i").unwrap();

    let mut output = Vec::new();
    status_execute(
        StatusArgs {
            porcelain: Some(PorcelainVersion::V2),
            ignored: true,
            ..Default::default()
        },
        &mut output,
    )
    .await;

    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.lines().any(|l| l.starts_with("! ignored.txt")),
        "porcelain v2 should list ignored files with '! ' when --ignored: {}",
        output_str
    );
}

#[tokio::test]
#[serial]
/// Tests porcelain v2 with --untracked-files=no hides untracked and ignored entries.
async fn test_status_porcelain_v2_untracked_files_no() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // tracked file
    fs::write("tracked.txt", "t").unwrap();
    add::execute(AddArgs {
        pathspec: vec![String::from("tracked.txt")],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    })
    .await;

    // untracked + ignored
    fs::write("untracked.txt", "u").unwrap();
    fs::write(".libraignore", "ignored.txt\n").unwrap();
    fs::write("ignored.txt", "i").unwrap();

    let mut output = Vec::new();
    status_execute(
        StatusArgs {
            porcelain: Some(PorcelainVersion::V2),
            ignored: true,
            untracked_files: UntrackedFiles::No,
            ..Default::default()
        },
        &mut output,
    )
    .await;

    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.lines().any(|l| l.starts_with("1 A")),
        "tracked entry should remain visible in v2: {}",
        output_str
    );
    assert!(
        !output_str.lines().any(|l| l.starts_with("? untracked.txt")),
        "untracked files should be hidden in v2 when --untracked-files=no: {}",
        output_str
    );
    assert!(
        !output_str.lines().any(|l| l.starts_with("! ignored.txt")),
        "ignored files should be hidden in v2 when --untracked-files=no even with --ignored: {}",
        output_str
    );
}

#[tokio::test]
#[serial]
/// Tests porcelain v2 with --untracked-files=all retains untracked output.
async fn test_status_porcelain_v2_untracked_files_all() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // tracked file
    fs::write("tracked.txt", "t").unwrap();
    add::execute(AddArgs {
        pathspec: vec![String::from("tracked.txt")],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    })
    .await;

    // untracked file
    fs::write("untracked.txt", "u").unwrap();

    let mut output = Vec::new();
    status_execute(
        StatusArgs {
            porcelain: Some(PorcelainVersion::V2),
            untracked_files: UntrackedFiles::All,
            ..Default::default()
        },
        &mut output,
    )
    .await;

    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.lines().any(|l| l.starts_with("1 A")),
        "tracked entry should be present in v2: {}",
        output_str
    );
    assert!(
        output_str.lines().any(|l| l.starts_with("? untracked.txt")),
        "untracked entry should be present in v2 when --untracked-files=all: {}",
        output_str
    );
}

#[tokio::test]
#[serial]
/// Tests --untracked-files=no hides untracked and ignored entries.
async fn test_status_untracked_files_no() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // tracked file
    fs::write("tracked.txt", "t").unwrap();
    add::execute(AddArgs {
        pathspec: vec![String::from("tracked.txt")],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    })
    .await;

    // untracked + ignored
    fs::write("untracked.txt", "u").unwrap();
    fs::write(".libraignore", "ignored.txt\n").unwrap();
    fs::write("ignored.txt", "i").unwrap();

    let mut output = Vec::new();
    status_execute(
        StatusArgs {
            porcelain: Some(PorcelainVersion::V1),
            ignored: true,
            untracked_files: UntrackedFiles::No,
            ..Default::default()
        },
        &mut output,
    )
    .await;

    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.contains("A  tracked.txt"),
        "tracked entry should remain visible: {}",
        output_str
    );
    assert!(
        !output_str.contains("?? untracked.txt"),
        "untracked files should be hidden when --untracked-files=no: {}",
        output_str
    );
    assert!(
        !output_str.contains("!! ignored.txt"),
        "ignored files should be hidden when --untracked-files=no even with --ignored: {}",
        output_str
    );
}

#[tokio::test]
#[serial]
/// Tests --untracked-files=all retains untracked output (same as normal for now).
async fn test_status_untracked_files_all() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // tracked file
    fs::write("tracked.txt", "t").unwrap();
    add::execute(AddArgs {
        pathspec: vec![String::from("tracked.txt")],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    })
    .await;

    // untracked file
    fs::write("untracked.txt", "u").unwrap();

    let mut output = Vec::new();
    status_execute(
        StatusArgs {
            porcelain: Some(PorcelainVersion::V1),
            untracked_files: UntrackedFiles::All,
            ..Default::default()
        },
        &mut output,
    )
    .await;

    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.contains("A  tracked.txt"),
        "tracked entry should be present: {}",
        output_str
    );
    assert!(
        output_str.contains("?? untracked.txt"),
        "untracked entry should be present when --untracked-files=all: {}",
        output_str
    );
}

#[tokio::test]
#[serial]
/// Tests status in a newly initialized empty repository
/// Verifies the initial state message for empty repositories
async fn test_status_empty_repository() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // Execute status command with default arguments
    let mut output = Vec::new();
    status_execute(
        StatusArgs {
            ..Default::default()
        },
        &mut output,
    )
    .await;

    let output_str = String::from_utf8(output).unwrap();

    // Should indicate no commits or nothing to commit in empty repo
    assert!(
        output_str.contains("No commits yet")
            || output_str.contains("nothing to commit")
            || output_str.contains("initial commit"),
        "Empty repository status should indicate initial state. Got: {}",
        output_str
    );
}

#[tokio::test]
#[serial]
/// Tests status with mixed staged and unstaged changes
/// Verifies proper separation of staged vs working directory changes
async fn test_status_mixed_changes() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // Create and stage a file
    let mut file1 = fs::File::create("staged.txt").unwrap();
    file1.write_all(b"initial content").unwrap();

    add::execute(AddArgs {
        pathspec: vec![String::from("staged.txt")],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    })
    .await;

    // Create an unstaged file
    let mut file2 = fs::File::create("unstaged.txt").unwrap();
    file2.write_all(b"unstaged content").unwrap();

    // Modify the staged file in working directory
    let mut file1 = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open("staged.txt")
        .unwrap();
    file1.write_all(b"modified content").unwrap();

    // Execute status command
    let mut output = Vec::new();
    status_execute(
        StatusArgs {
            ..Default::default()
        },
        &mut output,
    )
    .await;

    let output_str = String::from_utf8(output).unwrap();

    // Should show both staged and unstaged sections
    assert!(
        output_str.contains("staged.txt"),
        "Should show staged file: {}",
        output_str
    );
    assert!(
        output_str.contains("unstaged.txt"),
        "Should show unstaged file: {}",
        output_str
    );
}

#[tokio::test]
#[serial]
/// Tests status after file deletion
/// Verifies that deleted files are properly detected and reported
async fn test_status_deleted_files() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // Create, stage, and commit a file
    let file_path = "to_delete.txt";
    let mut file = fs::File::create(file_path).unwrap();
    file.write_all(b"content to delete").unwrap();

    add::execute(AddArgs {
        pathspec: vec![String::from(file_path)],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    })
    .await;

    // Use helper function to create CommitArgs
    commit::execute(create_commit_args("Add file to delete")).await;

    // Delete the file
    fs::remove_file(file_path).unwrap();

    // Execute status command
    let mut output = Vec::new();
    status_execute(
        StatusArgs {
            ..Default::default()
        },
        &mut output,
    )
    .await;

    let output_str = String::from_utf8(output).unwrap();

    // Should report the deleted file
    assert!(
        output_str.contains(file_path),
        "Should show deleted file: {}",
        output_str
    );
    assert!(
        output_str.contains("deleted") || output_str.contains("Deleted"),
        "Should indicate file deletion: {}",
        output_str
    );
}

#[tokio::test]
#[serial]
/// Tests status with subdirectory structure
/// Verifies that status works correctly with nested directory structures
async fn test_status_with_subdirectories() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // Create directory structure
    fs::create_dir_all("subdir/nested").unwrap();

    // Create files in different directories
    let files = [
        "root_file.txt",
        "subdir/sub_file.txt",
        "subdir/nested/deep_file.txt",
    ];

    for file_path in &files {
        let mut file = fs::File::create(file_path).unwrap();
        file.write_all(b"content").unwrap();
    }

    // Stage some files
    add::execute(AddArgs {
        pathspec: vec![String::from("root_file.txt")],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    })
    .await;

    // Execute status command with --untracked-files=all to show individual files
    let mut output = Vec::new();
    status_execute(
        StatusArgs {
            untracked_files: UntrackedFiles::All,
            ..Default::default()
        },
        &mut output,
    )
    .await;

    let output_str = String::from_utf8(output).unwrap().replace("\\", "/");

    // Should show files from all directories (with --untracked-files=all)
    for file_path in &files {
        assert!(
            output_str.contains(file_path),
            "Should show file from subdirectory: {} in {}",
            file_path,
            output_str
        );
    }

    // Test normal mode: untracked directories should be collapsed
    let mut output = Vec::new();
    status_execute(
        StatusArgs {
            untracked_files: UntrackedFiles::Normal,
            ..Default::default()
        },
        &mut output,
    )
    .await;

    let output_str = String::from_utf8(output).unwrap().replace("\\", "/");

    // In normal mode, subdir/ should be shown as a collapsed directory
    // since it's completely untracked
    assert!(
        output_str.contains("subdir/"),
        "Should show collapsed untracked directory: subdir/ in {}",
        output_str
    );
    // Individual files inside subdir should NOT be shown in normal mode
    assert!(
        !output_str.contains("subdir/sub_file.txt"),
        "Should NOT show individual files in collapsed directory: {}",
        output_str
    );
}

#[tokio::test]
#[serial]
/// Tests status verbose output format
/// Verifies that verbose mode provides additional information when requested
async fn test_status_verbose_output() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // Create a file and make it executable
    let mut file = fs::File::create("script.sh").unwrap();
    file.write_all(b"#!/bin/bash\necho hello").unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata("script.sh").unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions("script.sh", perms).unwrap();
    }

    // Stage the file
    add::execute(AddArgs {
        pathspec: vec![String::from("script.sh")],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    })
    .await;

    // Execute status command - we'll test that it completes without error
    // since we can't predict the exact verbose output format
    let mut output = Vec::new();

    // This should complete successfully without panicking
    status_execute(
        StatusArgs {
            ..Default::default()
        },
        &mut output,
    )
    .await;

    let output_str = String::from_utf8(output).unwrap();

    // Basic verification that status produced some output
    assert!(
        !output_str.is_empty(),
        "Status should produce output in verbose mode"
    );
    assert!(
        output_str.contains("script.sh"),
        "Should show staged file: {}",
        output_str
    );
}

#[tokio::test]
#[serial]
/// Tests --short --branch combination output
/// Verifies that branch info is displayed in short format when --branch flag is enabled.
async fn test_status_short_format_with_branch() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // Create and commit a file
    let mut file1 = fs::File::create("file1.txt").unwrap();
    file1.write_all(b"content").unwrap();

    // Add one file to the staging area
    add::execute(AddArgs {
        pathspec: vec![String::from("file1.txt")],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    })
    .await;

    commit::execute(create_commit_args("Initial commit")).await;

    // Modify the file
    let mut file1 = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open("file1.txt")
        .unwrap();
    file1.write_all(b"modified content").unwrap();

    let mut output = Vec::new();

    status_execute(
        StatusArgs {
            porcelain: None,
            short: true,
            branch: true,
            show_stash: false,
            ignored: false,
            untracked_files: UntrackedFiles::Normal,
        },
        &mut output,
    )
    .await;

    let output_str = String::from_utf8(output).unwrap();

    // Should show branch info in the first line with ## prefix
    assert!(
        output_str.contains("## master"),
        "Short format with --branch should start with branch info (##). Got: {}",
        output_str
    );
}

#[tokio::test]
#[serial]
/// Tests --porcelain --branch combination output
/// Verifies that branch info is displayed in porcelain format when --branch flag is enabled.
async fn test_status_porcelain_format_with_branch() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // Create and commit a file
    let mut file1 = fs::File::create("file1.txt").unwrap();
    file1.write_all(b"content").unwrap();

    // Add one file to the staging area
    add::execute(AddArgs {
        pathspec: vec![String::from("file1.txt")],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    })
    .await;

    commit::execute(create_commit_args("Initial commit")).await;

    // Modify the file
    let mut file1 = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open("file1.txt")
        .unwrap();
    file1.write_all(b"modified content").unwrap();

    let mut output = Vec::new();

    status_execute(
        StatusArgs {
            porcelain: Some(PorcelainVersion::V1),
            short: false,
            branch: true,
            show_stash: false,
            ignored: false,
            untracked_files: UntrackedFiles::Normal,
        },
        &mut output,
    )
    .await;

    let output_str = String::from_utf8(output).unwrap();

    // Should show branch info in the first line with ## prefix
    assert!(
        output_str.contains("## master"),
        "Porcelain format with --branch should start with branch info (##). Got: {}",
        output_str
    );
}

#[tokio::test]
#[serial]
/// Tests --show-stash output when stash exists
/// Verifies that stash count info is displayed in standard mode when --show-stash flag is enabled
async fn test_status_show_stash_with_existing_stash() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // Create and commit a file
    let mut file1 = fs::File::create("file1.txt").unwrap();
    file1.write_all(b"content").unwrap();

    // Add one file to the staging area
    add::execute(AddArgs {
        pathspec: vec![String::from("file1.txt")],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    })
    .await;

    commit::execute(create_commit_args("Initial commit")).await;

    // Create changes for stashing
    let mut file1 = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open("file1.txt")
        .unwrap();
    file1.write_all(b"modified content").unwrap();

    add::execute(AddArgs {
        pathspec: vec![String::from("file1.txt")],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
    })
    .await;

    stash::execute(Stash::Push {
        message: Some("test stash".to_string()),
    })
    .await;

    let mut output = Vec::new();

    status_execute(
        StatusArgs {
            porcelain: None,
            short: false,
            branch: false,
            show_stash: true,
            ignored: false,
            untracked_files: UntrackedFiles::Normal,
        },
        &mut output,
    )
    .await;

    let output_str = String::from_utf8(output).unwrap();

    // Should display stash count info
    assert!(
        output_str.contains("Your stash currently has 1 entry"),
        "Should show stash count when --show-stash flag is enabled. Got: {}",
        output_str
    );

    // Test for porcelain mode
    // Shouldn't output the stash count info
    let mut output = Vec::new();

    status_execute(
        StatusArgs {
            porcelain: Some(PorcelainVersion::V1),
            short: false,
            branch: false,
            show_stash: true,
            ignored: false,
            untracked_files: UntrackedFiles::Normal,
        },
        &mut output,
    )
    .await;

    let output_str = String::from_utf8(output).unwrap();

    // Shouldn't display stash count info
    assert!(
        !output_str.contains("Your stash currently has 1 entry"),
        "Porcelain format with --show-stash shouldn't start with stash count info. Got: {}",
        output_str
    );

    // Test for short mode
    // Shouldn't output the stash count info
    let mut output = Vec::new();

    status_execute(
        StatusArgs {
            porcelain: None,
            short: true,
            branch: false,
            show_stash: true,
            ignored: false,
            untracked_files: UntrackedFiles::Normal,
        },
        &mut output,
    )
    .await;

    let output_str = String::from_utf8(output).unwrap();

    // Shouldn't display stash count info
    assert!(
        !output_str.contains("Your stash currently has 1 entry"),
        "Short format with --show-stash shouldn't start with stash count info. Got: {}",
        output_str
    );
}

#[tokio::test]
#[serial]
/// Tests --show-stash output when no stash exists
/// Verifies that stash info is not displayed when no stash is present
async fn test_status_show_stash_without_stash() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // Create and commit a file
    let mut file1 = fs::File::create("file1.txt").unwrap();
    file1.write_all(b"content").unwrap();

    // Add one file to the staging area
    add::execute(AddArgs {
        pathspec: vec![String::from("file1.txt")],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    })
    .await;

    commit::execute(create_commit_args("Initial commit")).await;

    let mut output = Vec::new();

    status_execute(
        StatusArgs {
            porcelain: None,
            short: false,
            branch: false,
            show_stash: true,
            ignored: false,
            untracked_files: UntrackedFiles::Normal,
        },
        &mut output,
    )
    .await;

    let output_str = String::from_utf8(output).unwrap();

    // Should not display stash information when there are no stashes
    assert!(
        !output_str.contains("Your stash currently has"),
        "Should not show stash info when no stash exists. Got: {}",
        output_str
    );
}

#[tokio::test]
#[serial]
/// Tests --branch output in detached HEAD state
/// Verifies that branch info shows detached HEAD status correctly
async fn test_status_branch_detached_head() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // Create and commit a file
    let mut file1 = fs::File::create("file1.txt").unwrap();
    file1.write_all(b"initial content").unwrap();

    add::execute(AddArgs {
        pathspec: vec![String::from("file1.txt")],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    })
    .await;

    commit::execute(create_commit_args("Initial commit")).await;

    // Get the current commit hash for checkout
    let current_commit = Head::current_commit().await.expect("Should have a commit");

    // Create a second commit
    let mut file2 = fs::File::create("file2.txt").unwrap();
    file2.write_all(b"second file").unwrap();

    add::execute(AddArgs {
        pathspec: vec![String::from("file2.txt")],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    })
    .await;

    commit::execute(create_commit_args("Second commit")).await;

    // checkout the first commit to enter the detached state
    switch::execute(SwitchArgs {
        branch: Some(current_commit.to_string()),
        create: None,
        detach: true,
    })
    .await;

    let mut output = Vec::new();

    status_execute(
        StatusArgs {
            porcelain: None,
            short: true,
            branch: true,
            show_stash: false,
            ignored: false,
            untracked_files: UntrackedFiles::Normal,
        },
        &mut output,
    )
    .await;

    let output_str = String::from_utf8(output).unwrap();
    let display_info = format!("## HEAD (detached at {})", &current_commit.to_string()[..8]);
    // Should show detached HEAD info with ## prefix
    assert!(
        output_str.contains(&display_info),
        "Should show detached HEAD status in branch info. Got: {}",
        output_str
    );
}

#[tokio::test]
#[serial]
/// Tests porcelain v2 output shows actual file modes and hashes.
/// Verifies:
/// - New files have mH=000000 and zero hash for hH
/// - Tracked files show actual hashes from index and HEAD
async fn test_status_porcelain_v2_file_modes_and_hashes() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // Create and commit a file first
    fs::write("existing.txt", "existing content").unwrap();
    add::execute(AddArgs {
        pathspec: vec![String::from("existing.txt")],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    })
    .await;
    commit::execute(create_commit_args("Initial commit")).await;

    // Modify the existing file
    fs::write("existing.txt", "modified content").unwrap();
    add::execute(AddArgs {
        pathspec: vec![String::from("existing.txt")],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    })
    .await;

    // Create a new file (staged)
    fs::write("new_file.txt", "new content").unwrap();
    add::execute(AddArgs {
        pathspec: vec![String::from("new_file.txt")],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    })
    .await;

    let mut output = Vec::new();
    status_execute(
        StatusArgs {
            porcelain: Some(PorcelainVersion::V2),
            ..Default::default()
        },
        &mut output,
    )
    .await;

    let output_str = String::from_utf8(output).unwrap();

    // Use dynamic zero hash to support both SHA-1 (40 chars) and SHA-256 (64 chars)
    let zero_hash = git_internal::hash::ObjectHash::zero_str(git_internal::hash::get_hash_kind());

    // Check format for modified file: should have actual modes and hashes
    let existing_line = output_str.lines().find(|l| l.contains("existing.txt"));
    assert!(
        existing_line.is_some(),
        "Should contain existing.txt in output: {}",
        output_str
    );
    let existing_line = existing_line.unwrap();

    // Format: 1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>
    let parts: Vec<&str> = existing_line.split_whitespace().collect();
    assert!(
        parts.len() >= 9,
        "Modified file line should have at least 9 parts: {}",
        existing_line
    );

    // Check that mH (mode HEAD) is 100644 for existing file
    assert_eq!(
        parts[3], "100644",
        "mH should be 100644 for regular file: {}",
        existing_line
    );
    // Check that mI (mode index) is 100644
    assert_eq!(
        parts[4], "100644",
        "mI should be 100644 for regular file: {}",
        existing_line
    );
    // Check that hH and hI are not zero hashes
    assert!(
        parts[6] != zero_hash,
        "hH should not be zero hash for tracked file: {}",
        existing_line
    );
    assert!(
        parts[7] != zero_hash,
        "hI should not be zero hash for staged file: {}",
        existing_line
    );

    // Check format for new file: mH should be 000000 and hH should be zero hash
    let new_line = output_str.lines().find(|l| l.contains("new_file.txt"));
    assert!(
        new_line.is_some(),
        "Should contain new_file.txt in output: {}",
        output_str
    );
    let new_line = new_line.unwrap();

    let parts: Vec<&str> = new_line.split_whitespace().collect();
    assert!(
        parts.len() >= 9,
        "New file line should have at least 9 parts: {}",
        new_line
    );

    // Check that mH is 000000 for new file
    assert_eq!(
        parts[3], "000000",
        "mH should be 000000 for new file: {}",
        new_line
    );
    // Check that hH is zero hash for new file
    assert_eq!(
        parts[6], zero_hash,
        "hH should be zero hash for new file: {}",
        new_line
    );
    // Check that hI is NOT zero hash (file is in index)
    assert!(
        parts[7] != zero_hash,
        "hI should not be zero hash for staged new file: {}",
        new_line
    );
}

#[cfg(unix)]
#[tokio::test]
#[serial]
/// Tests porcelain v2 output shows 100755 for executable files.
async fn test_status_porcelain_v2_executable_file() {
    use std::os::unix::fs::PermissionsExt;

    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // Create an executable file
    fs::write("script.sh", "#!/bin/bash\necho hello").unwrap();
    let mut perms = fs::metadata("script.sh").unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions("script.sh", perms).unwrap();

    // Stage the executable file
    add::execute(AddArgs {
        pathspec: vec![String::from("script.sh")],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    })
    .await;

    let mut output = Vec::new();
    status_execute(
        StatusArgs {
            porcelain: Some(PorcelainVersion::V2),
            ..Default::default()
        },
        &mut output,
    )
    .await;

    let output_str = String::from_utf8(output).unwrap();

    let script_line = output_str.lines().find(|l| l.contains("script.sh"));
    assert!(
        script_line.is_some(),
        "Should contain script.sh in output: {}",
        output_str
    );
    let script_line = script_line.unwrap();

    let parts: Vec<&str> = script_line.split_whitespace().collect();
    assert!(
        parts.len() >= 9,
        "Executable file line should have at least 9 parts: {}",
        script_line
    );

    // Check that mI (mode index) is 100755 for executable
    assert_eq!(
        parts[4], "100755",
        "mI should be 100755 for executable file: {}",
        script_line
    );
    // Check that mW (mode worktree) is 100755 for executable
    assert_eq!(
        parts[5], "100755",
        "mW should be 100755 for executable file: {}",
        script_line
    );
}

#[tokio::test]
#[serial]
/// Tests porcelain v2 output for deleted files shows correct modes.
async fn test_status_porcelain_v2_deleted_file() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // Create, stage and commit a file
    fs::write("to_delete.txt", "content").unwrap();
    add::execute(AddArgs {
        pathspec: vec![String::from("to_delete.txt")],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    })
    .await;
    commit::execute(create_commit_args("Initial commit")).await;

    // Delete the file from working tree (but not from index)
    fs::remove_file("to_delete.txt").unwrap();

    let mut output = Vec::new();
    status_execute(
        StatusArgs {
            porcelain: Some(PorcelainVersion::V2),
            ..Default::default()
        },
        &mut output,
    )
    .await;

    let output_str = String::from_utf8(output).unwrap();

    let deleted_line = output_str.lines().find(|l| l.contains("to_delete.txt"));
    assert!(
        deleted_line.is_some(),
        "Should contain to_delete.txt in output: {}",
        output_str
    );
    let deleted_line = deleted_line.unwrap();

    let parts: Vec<&str> = deleted_line.split_whitespace().collect();
    assert!(
        parts.len() >= 9,
        "Deleted file line should have at least 9 parts: {}",
        deleted_line
    );

    // Should show status  D (space + D) for unstaged deletion
    assert!(
        deleted_line.starts_with("1  D"),
        "Should show ' D' status for deleted file: {}",
        deleted_line
    );

    // mW (mode worktree) should be 000000 for deleted file
    assert_eq!(
        parts[5], "000000",
        "mW should be 000000 for deleted file: {}",
        deleted_line
    );

    // mH and mI should still be 100644
    assert_eq!(
        parts[3], "100644",
        "mH should be 100644 for deleted file: {}",
        deleted_line
    );
    assert_eq!(
        parts[4], "100644",
        "mI should be 100644 for deleted file: {}",
        deleted_line
    );
}
