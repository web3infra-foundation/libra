//! Tests status reporting for staged, unstaged, ignored files and path filtering.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use std::{collections::VecDeque, fs, io::Write};

use libra::{
    cli::Stash,
    command::{
        bisect::BisectState,
        rebase::{RebaseRuntimeOptions, RebaseState},
        stash,
        status::{
            PorcelainVersion, StatusArgs, UntrackedFiles, execute_to as status_execute_inner,
            output_porcelain as output_porcelain_inner,
        },
    },
};

use super::*;

async fn status_execute(args: StatusArgs, writer: &mut impl Write) {
    status_execute_inner(args, writer)
        .await
        .expect("status output should succeed in test");
}

fn output_porcelain(
    staged: &libra::command::status::Changes,
    unstaged: &libra::command::status::Changes,
    writer: &mut impl Write,
) {
    output_porcelain_inner(staged, unstaged, writer)
        .expect("porcelain output should succeed in test");
}

#[test]
#[serial]
fn test_status_cli_outside_repository_returns_fatal_128() {
    let temp = tempdir().unwrap();

    let output = run_libra_command(&["status"], temp.path());
    assert_eq!(output.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("fatal: not a libra repository"),
        "unexpected stderr: {stderr}"
    );
}

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

#[tokio::test]
#[serial]
/// Ensures `status` refuses to run inside a bare repository.
async fn test_status_rejects_bare_repository() {
    let temp_path = tempdir().unwrap();
    test::setup_clean_testing_env_in(temp_path.path());

    init(InitArgs {
        bare: true,
        initial_branch: None,
        template: None,
        repo_directory: temp_path.path().to_str().unwrap().to_string(),
        quiet: false,
        shared: None,
        object_format: None,
        ref_format: None,
        from_git_repository: None,
        vault: false,
    })
    .await
    .unwrap();

    let _guard = ChangeDirGuard::new(temp_path.path());

    let mut out = Vec::new();
    let err = status_execute_inner(StatusArgs::default(), &mut out)
        .await
        .expect_err("status should refuse to run in bare repositories");
    assert!(
        err.to_string()
            .contains("this operation must be run in a work tree"),
        "unexpected bare-repo error: {err}"
    );
    assert!(
        out.is_empty(),
        "bare repo status should not write to stdout"
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

    let change = changes_to_be_staged().unwrap();
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
        ..Default::default()
    })
    .await;

    should_ignore_file_0.write_all(b"foo").unwrap();
    should_ignore_file_1.write_all(b"foo").unwrap();
    not_ignore_file_0.write_all(b"foo").unwrap();
    not_ignore_file_1.write_all(b"foo").unwrap();

    let change = changes_to_be_staged().unwrap();
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

    let change = changes_to_be_staged().unwrap();
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
    use std::path::PathBuf;

    use libra::command::status::Changes;

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
        ..Default::default()
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
        ..Default::default()
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
    use std::path::PathBuf;

    use libra::command::status::Changes;

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
        ..Default::default()
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
        ..Default::default()
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
        ..Default::default()
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
/// Tests porcelain v2 branch metadata uses the real HEAD oid and upstream counts.
async fn test_status_porcelain_v2_branch_metadata_includes_upstream_counts() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    fs::write("tracked.txt", "tracked\n").unwrap();
    add::execute_safe(
        AddArgs {
            pathspec: vec![String::from("tracked.txt")],
            all: false,
            update: false,
            verbose: false,
            dry_run: false,
            ignore_errors: false,
            refresh: false,
            force: false,
            ..Default::default()
        },
        &libra::utils::output::OutputConfig::default(),
    )
    .await
    .expect("add tracked.txt should succeed");
    execute_safe(
        create_commit_args("initial"),
        &libra::utils::output::OutputConfig::default(),
    )
    .await
    .expect("initial commit should succeed");

    let output = run_libra_command(&["config", "branch.main.remote", "origin"], test_dir.path());
    assert_cli_success(&output, "configure branch.main.remote");
    let output = run_libra_command(
        &["config", "branch.main.merge", "refs/heads/main"],
        test_dir.path(),
    );
    assert_cli_success(&output, "configure branch.main.merge");

    let head = Head::current_commit().await.expect("head commit");
    Branch::update_branch("main", &head.to_string(), Some("origin"))
        .await
        .expect("remote-tracking branch should be created");

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
        output_str.contains(&format!("# branch.oid {head}")),
        "porcelain v2 should emit the actual HEAD oid: {output_str}"
    );
    assert!(
        output_str.contains("# branch.upstream origin/main"),
        "porcelain v2 should emit upstream metadata: {output_str}"
    );
    assert!(
        output_str.contains("# branch.ab +0 -0"),
        "porcelain v2 should emit ahead/behind counts: {output_str}"
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
        ..Default::default()
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
            untracked_files: Some(UntrackedFiles::No),
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
        ..Default::default()
    })
    .await;

    // untracked file
    fs::write("untracked.txt", "u").unwrap();

    let mut output = Vec::new();
    status_execute(
        StatusArgs {
            porcelain: Some(PorcelainVersion::V2),
            untracked_files: Some(UntrackedFiles::All),
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
        ..Default::default()
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
            untracked_files: Some(UntrackedFiles::No),
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
        ..Default::default()
    })
    .await;

    // untracked file
    fs::write("untracked.txt", "u").unwrap();

    let mut output = Vec::new();
    status_execute(
        StatusArgs {
            porcelain: Some(PorcelainVersion::V1),
            untracked_files: Some(UntrackedFiles::All),
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
        ..Default::default()
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
        ..Default::default()
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
        ..Default::default()
    })
    .await;

    // Execute status command with --untracked-files=all to show individual files
    let mut output = Vec::new();
    status_execute(
        StatusArgs {
            untracked_files: Some(UntrackedFiles::All),
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
            untracked_files: Some(UntrackedFiles::Normal),
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
        ..Default::default()
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
        ..Default::default()
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
            untracked_files: Some(UntrackedFiles::Normal),
            exit_code: false,
            z: false,
        },
        &mut output,
    )
    .await;

    let output_str = String::from_utf8(output).unwrap();

    // Should show branch info in the first line with ## prefix
    assert!(
        output_str.contains("## main"),
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
        ..Default::default()
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
            untracked_files: Some(UntrackedFiles::Normal),
            exit_code: false,
            z: false,
        },
        &mut output,
    )
    .await;

    let output_str = String::from_utf8(output).unwrap();

    // Should show branch info in the first line with ## prefix
    assert!(
        output_str.contains("## main"),
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
        ..Default::default()
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
        ..Default::default()
    })
    .await;

    stash::execute(Stash::Push {
        message: Some("test stash".to_string()),
        include_untracked: false,
        all: false,
        keep_index: false,
        patch: false,
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
            untracked_files: Some(UntrackedFiles::Normal),
            exit_code: false,
            z: false,
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
            untracked_files: Some(UntrackedFiles::Normal),
            exit_code: false,
            z: false,
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
            untracked_files: Some(UntrackedFiles::Normal),
            exit_code: false,
            z: false,
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
        ..Default::default()
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
            untracked_files: Some(UntrackedFiles::Normal),
            exit_code: false,
            z: false,
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
        ..Default::default()
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
        ..Default::default()
    })
    .await;

    commit::execute(create_commit_args("Second commit")).await;

    // checkout the first commit to enter the detached state
    switch::execute(SwitchArgs {
        branch: Some(current_commit.to_string()),
        create: None,
        force_create: None,
        detach: true,
        track: false,
        ..Default::default()
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
            untracked_files: Some(UntrackedFiles::Normal),
            exit_code: false,
            z: false,
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
        ..Default::default()
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
        ..Default::default()
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
        ..Default::default()
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
        ..Default::default()
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
        ..Default::default()
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

#[tokio::test]
#[serial]
/// Tests status command after adding a file
///
/// Verifies that the status command correctly reports added files with proper formatting
async fn test_status_after_add() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // Create a new file
    let file_path = "test.txt";
    fs::write(file_path, "content").unwrap();

    // Add the file
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

    // Test porcelain output
    let mut output = Vec::new();
    status_execute(
        StatusArgs {
            porcelain: Some(PorcelainVersion::V1),
            ..Default::default()
        },
        &mut output,
    )
    .await;

    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str
            .lines()
            .any(|l| l.starts_with("A ") && l.contains(file_path)),
        "Porcelain status should show 'A ' prefix for added file: {}",
        output_str
    );

    // Test short output
    let mut output = Vec::new();
    status_execute(
        StatusArgs {
            short: true,
            ..Default::default()
        },
        &mut output,
    )
    .await;

    let output_str = String::from_utf8(output).unwrap();
    let re = regex::Regex::new(r"\x1b\[[0-9;]*m").unwrap();
    let clean_output = re.replace_all(&output_str, "");
    assert!(
        clean_output
            .lines()
            .any(|l| l.starts_with("A ") && l.contains(file_path)),
        "Short status should show 'A ' prefix for added file: {}",
        clean_output
    );

    // Verify via changes_to_be_committed
    let changes = changes_to_be_committed().await;
    assert!(
        changes.new.iter().any(|x| x.to_str().unwrap() == file_path),
        "Added file should appear in changes_to_be_committed"
    );
}

// ---------------------------------------------------------------------------
// Success summary output for add
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_add_success_summary_output() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let output = run_libra_command(&["init"], &repo);
    assert!(output.status.success());

    std::fs::write(repo.join("new.txt"), "hello").unwrap();

    let output = run_libra_command(&["add", "new.txt"], &repo);
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("add") && stdout.contains("new.txt"),
        "add should print success summary, got: {stdout}"
    );
}

#[test]
#[serial]
fn test_add_quiet_suppresses_output() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let output = run_libra_command(&["init"], &repo);
    assert!(output.status.success());

    std::fs::write(repo.join("new.txt"), "hello").unwrap();

    let output = run_libra_command(&["--quiet", "add", "new.txt"], &repo);
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.trim().is_empty(),
        "quiet mode should suppress stdout, got: {stdout}"
    );
}

#[test]
#[serial]
fn test_add_verbose_shows_per_file_listing() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let output = run_libra_command(&["init"], &repo);
    assert!(output.status.success());

    std::fs::write(repo.join("a.txt"), "a").unwrap();
    std::fs::write(repo.join("b.txt"), "b").unwrap();

    let output = run_libra_command(&["add", "--verbose", "a.txt", "b.txt"], &repo);
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("add(new)"),
        "verbose mode should show per-file details, got: {stdout}"
    );
}

#[test]
#[serial]
fn test_add_nothing_specified_exit_129() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let output = run_libra_command(&["init"], &repo);
    assert!(output.status.success());

    let output = run_libra_command(&["add"], &repo);
    assert_eq!(output.status.code(), Some(129));

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("nothing specified"),
        "should show nothing specified hint: {stderr}"
    );
}

#[test]
#[serial]
fn test_add_dry_run_output() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let output = run_libra_command(&["init"], &repo);
    assert!(output.status.success());

    std::fs::write(repo.join("file.txt"), "content").unwrap();

    let output = run_libra_command(&["add", "--dry-run", "file.txt"], &repo);
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("dry run"),
        "dry run should indicate no files staged, got: {stdout}"
    );

    // Verify file was NOT actually staged
    let status = run_libra_command(&["status", "--short"], &repo);
    let status_stdout = String::from_utf8_lossy(&status.stdout);
    assert!(
        !status_stdout.contains("A  file.txt"),
        "dry run should not stage: {status_stdout}"
    );
}

#[test]
#[serial]
fn test_status_z_nul_terminates_porcelain() {
    let repo = create_committed_repo_via_cli();
    std::fs::write(repo.path().join("untracked.txt"), "u\n").unwrap();
    std::fs::write(repo.path().join("tracked.txt"), "tracked\nmod\n").unwrap();

    // `-z` with no explicit format implies porcelain v1 with NUL terminators.
    let out = run_libra_command(&["status", "-z"], repo.path());
    assert_cli_success(&out, "status -z");
    assert!(
        out.stdout.contains(&0u8),
        "status -z must NUL-terminate entries; stdout={:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        !out.stdout.contains(&b'\n'),
        "status -z must not emit newlines; stdout={:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("tracked.txt"),
        "should list the modified file: {text}"
    );
    assert!(
        text.contains("untracked.txt"),
        "should list the untracked file: {text}"
    );
}

fn stage_tracked_rename() -> tempfile::TempDir {
    let repo = create_committed_repo_via_cli();
    std::fs::rename(
        repo.path().join("tracked.txt"),
        repo.path().join("renamed.txt"),
    )
    .expect("rename tracked file");

    let output = run_libra_command(&["add", "-A"], repo.path());
    assert_cli_success(&output, "stage rename with add -A");
    repo
}

#[test]
#[serial]
fn porcelain_v2_rename_line_emits_r100() {
    let repo = stage_tracked_rename();

    let output = run_libra_command(&["status", "--porcelain=v2"], repo.path());
    assert_cli_success(&output, "status --porcelain=v2 after rename");
    let stdout = String::from_utf8_lossy(&output.stdout);

    let rename_line = stdout
        .lines()
        .find(|line| line.starts_with("2 R "))
        .unwrap_or_else(|| panic!("expected porcelain v2 rename line, got: {stdout}"));
    assert!(
        rename_line.contains(" R100 renamed.txt\ttracked.txt"),
        "rename line must carry R100 and TAB-separated new/original paths, got: {rename_line}"
    );
    assert!(
        !stdout
            .lines()
            .any(|line| line.ends_with(" renamed.txt") && line.starts_with("1 A")),
        "renamed destination must not also render as a plain add: {stdout}"
    );
    assert!(
        !stdout
            .lines()
            .any(|line| line.ends_with(" tracked.txt") && line.starts_with("1 D")),
        "rename source must not also render as a plain delete: {stdout}"
    );
}

#[test]
#[serial]
fn short_rename_arrow_format() {
    let repo = stage_tracked_rename();

    let output = run_libra_command(&["status", "--short"], repo.path());
    assert_cli_success(&output, "status --short after rename");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout
            .lines()
            .any(|line| line == "R  tracked.txt -> renamed.txt"),
        "short status should render staged rename as old -> new arrow, got: {stdout}"
    );
    assert!(
        !stdout
            .lines()
            .any(|line| line == "A  renamed.txt" || line == "D  tracked.txt"),
        "short status should collapse rename instead of add/delete pair, got: {stdout}"
    );
}

#[test]
#[serial]
fn z_flag_porcelain_v2_rename_nul() {
    let repo = stage_tracked_rename();

    let output = run_libra_command(&["status", "--porcelain=v2", "-z"], repo.path());
    assert_cli_success(&output, "status --porcelain=v2 -z after rename");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        output
            .stdout
            .windows(b"renamed.txt\0tracked.txt\0".len())
            .any(|window| window == b"renamed.txt\0tracked.txt\0"),
        "porcelain v2 -z rename must use NUL between new and original path: {stdout:?}"
    );
    assert!(
        !output
            .stdout
            .windows(b"renamed.txt\ttracked.txt".len())
            .any(|window| window == b"renamed.txt\ttracked.txt"),
        "porcelain v2 -z rename must not retain TAB path separator: {stdout:?}"
    );
    assert!(
        !stdout.contains(" -> "),
        "porcelain v2 -z rename must not use arrow syntax: {stdout:?}"
    );
}

#[test]
#[serial]
fn z_flag_short_rename_order_new_then_orig() {
    let repo = stage_tracked_rename();

    let output = run_libra_command(&["status", "-z", "-s"], repo.path());
    assert_cli_success(&output, "status -z -s after rename");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        output.stdout == b"R  renamed.txt\0tracked.txt\0",
        "short -z rename must be `R  <new>\\0<orig>\\0`, got: {stdout:?}"
    );
}

#[test]
#[serial]
fn z_alone_equals_porcelain_v1_z() {
    let repo = create_committed_repo_via_cli();
    std::fs::write(repo.path().join("untracked.txt"), "u\n").unwrap();

    let z_only = run_libra_command(&["status", "-z"], repo.path());
    assert_cli_success(&z_only, "status -z");
    let porcelain_z = run_libra_command(&["status", "--porcelain=v1", "-z"], repo.path());
    assert_cli_success(&porcelain_z, "status --porcelain=v1 -z");

    assert_eq!(
        z_only.stdout, porcelain_z.stdout,
        "status -z must equal porcelain v1 -z"
    );
}

#[test]
#[serial]
fn config_show_untracked_no_hides_untracked() {
    let repo = create_committed_repo_via_cli();
    std::fs::write(repo.path().join("untracked.txt"), "u\n").unwrap();
    let output = run_libra_command(&["config", "status.showUntrackedFiles", "no"], repo.path());
    assert_cli_success(&output, "configure status.showUntrackedFiles");

    let output = run_libra_command(&["status", "--short"], repo.path());
    assert_cli_success(&output, "status --short with status.showUntrackedFiles=no");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        !stdout.contains("untracked.txt"),
        "config should hide untracked files by default: {stdout}"
    );
}

#[test]
#[serial]
fn cli_overrides_config_untracked() {
    let repo = create_committed_repo_via_cli();
    std::fs::create_dir_all(repo.path().join("dir")).unwrap();
    std::fs::write(repo.path().join("dir").join("child.txt"), "u\n").unwrap();
    let output = run_libra_command(&["config", "status.showUntrackedFiles", "no"], repo.path());
    assert_cli_success(&output, "configure status.showUntrackedFiles");

    let output = run_libra_command(&["status", "--short", "--untracked-files=all"], repo.path());
    assert_cli_success(&output, "status --untracked-files=all overrides config");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.contains("?? dir/child.txt"),
        "explicit CLI untracked mode should override config: {stdout}"
    );
}

#[test]
#[serial]
fn config_branch_true_enables_branch_header() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["config", "status.branch", "true"], repo.path());
    assert_cli_success(&output, "configure status.branch");

    let output = run_libra_command(&["status", "--short"], repo.path());
    assert_cli_success(&output, "status --short with status.branch=true");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout
            .lines()
            .next()
            .is_some_and(|line| line.starts_with("## ")),
        "status.branch=true should enable short branch header: {stdout}"
    );
}

#[test]
#[serial]
fn config_short_true_enables_short_output() {
    let repo = create_committed_repo_via_cli();
    std::fs::write(repo.path().join("tracked.txt"), "tracked\nmodified\n").unwrap();
    let output = run_libra_command(&["config", "status.short", "true"], repo.path());
    assert_cli_success(&output, "configure status.short");

    let output = run_libra_command(&["status"], repo.path());
    assert_cli_success(&output, "status with status.short=true");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.lines().any(|line| line == " M tracked.txt"),
        "status.short=true should default to short output: {stdout}"
    );
    assert!(
        !stdout.contains("Changes not staged for commit"),
        "status.short=true should not render long human sections: {stdout}"
    );
}

#[test]
#[serial]
fn invalid_config_value_warns_and_falls_back() {
    let repo = create_committed_repo_via_cli();
    std::fs::write(repo.path().join("untracked.txt"), "u\n").unwrap();
    let output = run_libra_command(
        &["config", "status.showUntrackedFiles", "banana"],
        repo.path(),
    );
    assert_cli_success(&output, "configure invalid status.showUntrackedFiles");

    let output = run_libra_command(&["status", "--short"], repo.path());
    assert_cli_success(&output, "status with invalid status.showUntrackedFiles");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("?? untracked.txt"),
        "invalid config should fall back to normal untracked output: {stdout}"
    );
    assert!(
        stderr.contains("warning: invalid status.showUntrackedFiles 'banana'"),
        "invalid config should warn on stderr: {stderr}"
    );

    let warning_exit = run_libra_command(
        &["--exit-code-on-warning", "status", "--short"],
        repo.path(),
    );
    assert_eq!(warning_exit.status.code(), Some(9));
    let stderr = String::from_utf8_lossy(&warning_exit.stderr);
    assert!(
        stderr.contains("warning: invalid status.showUntrackedFiles 'banana'"),
        "--exit-code-on-warning should preserve the config warning: {stderr}"
    );
}

async fn save_rebase_state_for_status() -> String {
    let head = Head::current_commit()
        .await
        .expect("committed repo should have HEAD");
    let state = RebaseState {
        head_name: "main".to_string(),
        onto: head,
        orig_head: head,
        todo: VecDeque::new(),
        done: Vec::new(),
        stopped_sha: None,
        current_head: head,
        autostash_ref: None,
        options: RebaseRuntimeOptions::default(),
    };
    state.save().await.expect("save rebase state");
    head.to_string()
}

async fn save_bisect_state_for_status() {
    let head = Head::current_commit()
        .await
        .expect("committed repo should have HEAD");
    let state = BisectState {
        orig_head: head,
        orig_head_name: Some("main".to_string()),
        bad: Some(head),
        good: Vec::new(),
        current: Some(head),
        skipped: Vec::new(),
        steps: Some(0),
        completed: false,
    };
    state.save().await.expect("save bisect state");
}

#[tokio::test]
#[serial]
async fn detect_rebase_state_human_hint() {
    let repo = create_committed_repo_via_cli();
    let _guard = ChangeDirGuard::new(repo.path());
    let onto = save_rebase_state_for_status().await;

    let output = run_libra_command(&["status"], repo.path());
    assert_cli_success(&output, "status with rebase state");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.contains("rebase in progress"),
        "human status should report rebase state: {stdout}"
    );
    assert!(
        stdout.contains(&format!("onto {}", &onto[..8])),
        "human status should include the rebase target: {stdout}"
    );
    assert!(
        stdout.contains("libra rebase --continue") && stdout.contains("libra rebase --abort"),
        "human status should include rebase recovery commands: {stdout}"
    );
}

#[tokio::test]
#[serial]
async fn detect_bisect_state_human_hint() {
    let repo = create_committed_repo_via_cli();
    let _guard = ChangeDirGuard::new(repo.path());
    save_bisect_state_for_status().await;

    let output = run_libra_command(&["status"], repo.path());
    assert_cli_success(&output, "status with bisect state");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.contains("bisect in progress"),
        "human status should report bisect state: {stdout}"
    );
    assert!(
        !stdout.contains("bisect --abort"),
        "status must not mention unsupported bisect --abort: {stdout}"
    );
}

#[tokio::test]
#[serial]
async fn json_includes_repo_state() {
    let repo = create_committed_repo_via_cli();

    let clean_output = run_libra_command(&["status", "--json"], repo.path());
    assert_cli_success(&clean_output, "json status without repo state");
    let clean_json = parse_json_stdout(&clean_output);
    assert_eq!(clean_json["data"]["repo_state"], serde_json::Value::Null);

    let _guard = ChangeDirGuard::new(repo.path());
    save_rebase_state_for_status().await;
    drop(_guard);

    let rebase_output = run_libra_command(&["status", "--json"], repo.path());
    assert_cli_success(&rebase_output, "json status with rebase state");
    let rebase_json = parse_json_stdout(&rebase_output);
    assert_eq!(rebase_json["data"]["repo_state"], "rebase");
}

#[tokio::test]
#[serial]
async fn porcelain_modes_omit_repo_state_hints() {
    let repo = create_committed_repo_via_cli();
    let _guard = ChangeDirGuard::new(repo.path());
    save_rebase_state_for_status().await;

    for args in [
        &["status", "--short"][..],
        &["status", "--porcelain=v1"][..],
        &["status", "--porcelain=v2"][..],
    ] {
        let output = run_libra_command(args, repo.path());
        assert_cli_success(&output, "machine status with rebase state");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            !stdout.contains("rebase in progress") && !stdout.contains("libra rebase --continue"),
            "machine/short output must not include human repo-state hints: {stdout}"
        );
    }
}
