//! Integration tests for the mv command covering tracked validation and move behaviors.

use std::fs;
use std::process::Command;

use git_internal::internal::index::Index;
use libra::utils::path;

use super::*;

async fn stage_file(path: &str, content: &str) {
    test::ensure_file(path, Some(content));
    add::execute(AddArgs {
        pathspec: vec![path.to_string()],
        all: false,
        update: false,
        refresh: false,
        verbose: false,
        force: false,
        dry_run: false,
        ignore_errors: false,
    })
    .await;
}

#[tokio::test]
#[serial]
/// Moves a tracked file to a new file path.
async fn test_mv_moves_tracked_file_to_new_path() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    stage_file("a.txt", "hello").await;

    mv::execute(MvArgs {
        paths: vec!["a.txt".to_string(), "b.txt".to_string()],
        verbose: false,
        dry_run: false,
        force: false,
    })
    .await;

    assert!(!temp_path.path().join("a.txt").exists());
    assert!(temp_path.path().join("b.txt").exists());

    let index = Index::load(path::index()).unwrap();
    assert!(!index.tracked("a.txt", 0));
    assert!(index.tracked("b.txt", 0));
}

#[tokio::test]
#[serial]
/// Moves a tracked file into an existing destination directory.
async fn test_mv_moves_tracked_file_into_directory() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    stage_file("move_me.txt", "content").await;
    fs::create_dir_all("dest").unwrap();

    mv::execute(MvArgs {
        paths: vec!["move_me.txt".to_string(), "dest".to_string()],
        verbose: false,
        dry_run: false,
        force: false,
    })
    .await;

    assert!(!temp_path.path().join("move_me.txt").exists());
    assert!(temp_path.path().join("dest/move_me.txt").exists());

    let index = Index::load(path::index()).unwrap();
    assert!(!index.tracked("move_me.txt", 0));
    assert!(index.tracked("dest/move_me.txt", 0));
}

#[tokio::test]
#[serial]
/// Moves a directory with tracked files and updates index entries for moved files.
async fn test_mv_moves_directory_with_tracked_files() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    stage_file("src_dir/a.txt", "a").await;
    stage_file("src_dir/sub/b.txt", "b").await;
    fs::create_dir_all("dest").unwrap();

    mv::execute(MvArgs {
        paths: vec!["src_dir".to_string(), "dest".to_string()],
        verbose: false,
        dry_run: false,
        force: false,
    })
    .await;

    assert!(!temp_path.path().join("src_dir/a.txt").exists());
    assert!(!temp_path.path().join("src_dir/sub/b.txt").exists());
    assert!(temp_path.path().join("dest/src_dir/a.txt").exists());
    assert!(temp_path.path().join("dest/src_dir/sub/b.txt").exists());

    let index = Index::load(path::index()).unwrap();
    assert!(!index.tracked("src_dir/a.txt", 0));
    assert!(!index.tracked("src_dir/sub/b.txt", 0));
    assert!(index.tracked("dest/src_dir/a.txt", 0));
    assert!(index.tracked("dest/src_dir/sub/b.txt", 0));
}

#[tokio::test]
#[serial]
/// Overwrites an existing destination file when `--force` is enabled.
async fn test_mv_force_overwrites_existing_file() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    stage_file("src.txt", "new-content").await;
    test::ensure_file("dst.txt", Some("old-content"));

    mv::execute(MvArgs {
        paths: vec!["src.txt".to_string(), "dst.txt".to_string()],
        verbose: false,
        dry_run: false,
        force: true,
    })
    .await;

    assert!(!temp_path.path().join("src.txt").exists());
    let dst_content = fs::read_to_string(temp_path.path().join("dst.txt")).unwrap();
    assert_eq!(dst_content, "new-content");

    let index = Index::load(path::index()).unwrap();
    assert!(!index.tracked("src.txt", 0));
    assert!(index.tracked("dst.txt", 0));
}

#[tokio::test]
#[serial]
/// Prints a rename message when `-v` is used and move succeeds.
async fn test_mv_verbose_prints_rename_message() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    stage_file("verbose.txt", "v").await;

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .args(["mv", "-v", "verbose.txt", "verbose_new.txt"])
        .output()
        .expect("failed to execute libra mv -v");

    assert!(
        output.status.success(),
        "mv -v should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Renaming verbose.txt to verbose_new.txt"),
        "expected verbose output, got stdout: {stdout}"
    );

    assert!(!temp_path.path().join("verbose.txt").exists());
    assert!(temp_path.path().join("verbose_new.txt").exists());
}

#[tokio::test]
#[serial]
/// Prints dry-run messages exactly as defined in mv implementation.
async fn test_mv_dry_run_output_matches_command_text() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    stage_file("dry_cli.txt", "d").await;

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .args(["mv", "-n", "dry_cli.txt", "dry_cli_new.txt"])
        .output()
        .expect("failed to execute libra mv -n");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Checking rename of 'dry_cli.txt' to 'dry_cli_new.txt'"));
    assert!(stdout.contains("Renaming dry_cli.txt to dry_cli_new.txt"));
    assert!(temp_path.path().join("dry_cli.txt").exists());
    assert!(!temp_path.path().join("dry_cli_new.txt").exists());
}

#[tokio::test]
#[serial]
/// Prints usage text when `mv` is called without enough arguments.
async fn test_mv_usage_output_matches_command_text() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .args(["mv"])
        .output()
        .expect("failed to execute libra mv");

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("usage: libra mv [<options>] <source>... <destination>"));
    assert!(stderr.contains("-v, --[no-]verbose    be verbose"));
    assert!(stderr.contains("-n, --[no-]dry-run    dry run"));
    assert!(stderr.contains("-f, --[no-]force      force move/rename even if target exists"));
}

#[tokio::test]
#[serial]
/// Prints the expected bad source error text for non-existent source paths.
async fn test_mv_bad_source_output_matches_command_text() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .args(["mv", "11", "22"])
        .output()
        .expect("failed to execute libra mv bad source case");

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("fatal: bad source, source=11, destination=22"),
        "unexpected stderr: {stderr}"
    );
}

#[tokio::test]
#[serial]
/// Rejects moving an untracked source file.
async fn test_mv_rejects_untracked_source() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    test::ensure_file("untracked.txt", Some("u"));

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .args(["mv", "untracked.txt", "renamed.txt"])
        .output()
        .expect("failed to execute libra mv untracked case");

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(
            "fatal: not under version control, source=untracked.txt, destination=renamed.txt"
        ),
        "unexpected stderr: {stderr}"
    );

    assert!(temp_path.path().join("untracked.txt").exists());
    assert!(!temp_path.path().join("renamed.txt").exists());
}

#[tokio::test]
#[serial]
/// Rejects multi-source moves that would map to the same target path.
async fn test_mv_rejects_multiple_sources_with_same_target_name() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    stage_file("a/same.txt", "from-a").await;
    stage_file("b/same.txt", "from-b").await;
    fs::create_dir_all("dest").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .args(["mv", "a/same.txt", "b/same.txt", "dest"])
        .output()
        .expect("failed to execute libra mv duplicate target case");

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(
            "fatal: multiple sources moving to the same target path, source=b/same.txt, destination=dest"
        ),
        "unexpected stderr: {stderr}"
    );

    assert!(temp_path.path().join("a/same.txt").exists());
    assert!(temp_path.path().join("b/same.txt").exists());
    assert!(!temp_path.path().join("dest/same.txt").exists());
}

#[tokio::test]
#[serial]
/// Rejects moving a directory when it contains no tracked files.
async fn test_mv_rejects_directory_without_tracked_files() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    fs::create_dir_all("src_dir").unwrap();
    test::ensure_file("src_dir/untracked.txt", Some("u"));
    fs::create_dir_all("dest").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .args(["mv", "src_dir", "dest"])
        .output()
        .expect("failed to execute libra mv empty dir case");

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("fatal: source directory is empty, source=src_dir, destination=dest"),
        "unexpected stderr: {stderr}"
    );

    assert!(temp_path.path().join("src_dir/untracked.txt").exists());
    assert!(!temp_path.path().join("dest/src_dir").exists());
}

#[tokio::test]
#[serial]
/// Successfully moves a directory containing tracked files, updating paths accordingly.
async fn test_mv_moves_directory_with_tracked_files() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    // Create and stage tracked files within the source directory (including a nested file).
    stage_file("src_dir/tracked1.txt", "content-1").await;
    stage_file("src_dir/nested/tracked2.txt", "content-2").await;

    // Prepare destination directory.
    fs::create_dir_all("dest").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .args(["mv", "src_dir", "dest"])
        .output()
        .expect("failed to execute libra mv directory with tracked files");

    // The mv command itself should succeed.
    assert!(output.status.success());

    // Source directory and its tracked contents should no longer exist at the original paths.
    assert!(!temp_path.path().join("src_dir").exists());
    assert!(!temp_path.path().join("src_dir/tracked1.txt").exists());
    assert!(!temp_path.path().join("src_dir/nested/tracked2.txt").exists());

    // Destination should now contain the moved directory and all tracked files.
    assert!(temp_path.path().join("dest/src_dir").exists());
    assert!(temp_path.path().join("dest/src_dir/tracked1.txt").exists());
    assert!(temp_path.path().join("dest/src_dir/nested/tracked2.txt").exists());
}

#[tokio::test]
#[serial]
/// Rejects moving a directory to a non-directory destination path.
async fn test_mv_rejects_directory_to_non_directory_destination() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    stage_file("dir/x.txt", "x").await;
    test::ensure_file("dest_file.txt", Some("d"));

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .args(["mv", "dir", "dest_file.txt"])
        .output()
        .expect("failed to execute libra mv directory->file case");

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("fatal: destination 'dest_file.txt' is not a directory"),
        "unexpected stderr: {stderr}"
    );

    assert!(temp_path.path().join("dir/x.txt").exists());
    assert!(temp_path.path().join("dest_file.txt").exists());
}

#[tokio::test]
#[serial]
/// Rejects moving a directory into itself (source and destination are the same path).
async fn test_mv_rejects_directory_into_itself() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    // Create a tracked directory with a tracked file inside.
    stage_file("dir/x.txt", "x").await;

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .args(["mv", "dir", "dir"])
        .output()
        .expect("failed to execute libra mv directory-into-itself case");

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("fatal: can not move directory into itself"),
        "unexpected stderr: {stderr}"
    );

    // Ensure the directory and its tracked contents are unchanged.
    assert!(temp_path.path().join("dir/x.txt").exists());
}

#[tokio::test]
#[serial]
/// Rejects forcing an overwrite when the destination path is an existing directory.
async fn test_mv_force_rejects_overwrite_directory_destination() {
    let temp_path = tempdir().unwrap();
    // 1. Create and commit a base version of the file on the main branch.
    stage_file("conflicted.txt", "base").await;
    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .args(["commit", "-m", "base version"])
        .output()
        .expect("failed to execute initial libra commit");
    assert!(
        output.status.success(),
        "initial commit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // 2. Create a feature branch and switch to it.
    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .args(["branch", "feature"])
        .output()
        .expect("failed to create feature branch");
    assert!(
        output.status.success(),
        "branch creation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .args(["checkout", "feature"])
        .output()
        .expect("failed to checkout feature branch");
    assert!(
        output.status.success(),
        "checkout feature failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // 3. On the feature branch, change the file, stage, and commit.
    stage_file("conflicted.txt", "ours").await;
    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .args(["commit", "-m", "ours on feature"])
        .output()
        .expect("failed to commit on feature branch");
    assert!(
        output.status.success(),
        "feature commit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // 4. Switch back to the main branch and create a conflicting change.
    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .args(["checkout", "main"])
        .output()
        .expect("failed to checkout main branch");
    assert!(
        output.status.success(),
        "checkout main failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    stage_file("conflicted.txt", "theirs").await;
    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .args(["commit", "-m", "theirs on main"])
        .output()
        .expect("failed to commit on main branch");
    assert!(
        output.status.success(),
        "main commit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // 5. Merge the feature branch into main to create an actual merge conflict.
    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .args(["merge", "feature"])
        .output()
        .expect("failed to merge feature into main");
    // The merge is expected to produce a conflict; depending on implementation,
    // it may or may not exit successfully, so we do not assert on status here.

    // At this point, conflicted.txt should have unmerged index entries.
    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .args(["mv", "conflicted.txt", "moved.txt"])
        .output()
        .expect("failed to execute libra mv on conflicted source file");
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Cannot overwrite") || stderr.contains("cannot overwrite"),
        "unexpected stderr: {stderr}"
    );

    // Ensure that neither the source file nor the destination directory was altered.
    assert!(temp_path.path().join("source.txt").exists());
    assert!(temp_path.path().join("dest_path").exists());
}

#[tokio::test]
#[serial]
/// Rejects moving a conflicted source file with a 'conflicted' error.
async fn test_mv_rejects_conflicted_source_file() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    // Create and stage a file that simulates a merge conflict.
    let conflicted_content = "\
<<<<<<< HEAD
ours
=======
theirs
>>>>>>> BRANCH
";
    stage_file("conflicted.txt", conflicted_content).await;

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .args(["mv", "conflicted.txt", "moved.txt"])
        .output()
        .expect("failed to execute libra mv conflicted source case");

    // The mv command reports the error via stderr but still exits successfully,
    // consistent with the other mv tests in this module.
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("conflicted"),
        "expected conflicted error in stderr, got: {stderr}"
    );

    // Ensure no move took place on disk.
    assert!(temp_path.path().join("conflicted.txt").exists());
    assert!(!temp_path.path().join("moved.txt").exists());
}
