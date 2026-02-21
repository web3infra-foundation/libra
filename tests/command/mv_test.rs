//! Integration tests for the mv command covering tracked validation and move behaviors.

use std::{fs, process::Command};

use git_internal::internal::index::{Index, IndexEntry};
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

    let result = mv::execute(MvArgs {
        paths: vec!["a.txt".to_string(), "b.txt".to_string()],
        verbose: false,
        dry_run: false,
        force: false,
    })
    .await;
    assert!(result.is_ok());

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

    let result = mv::execute(MvArgs {
        paths: vec!["move_me.txt".to_string(), "dest".to_string()],
        verbose: false,
        dry_run: false,
        force: false,
    })
    .await;
    assert!(result.is_ok());

    assert!(!temp_path.path().join("move_me.txt").exists());
    assert!(temp_path.path().join("dest/move_me.txt").exists());

    let index = Index::load(path::index()).unwrap();
    assert!(!index.tracked("move_me.txt", 0));
    assert!(index.tracked("dest/move_me.txt", 0));
}

#[tokio::test]
#[serial]
/// Resolves mv path arguments relative to current directory when invoked from a subdirectory.
async fn test_mv_resolves_paths_from_current_subdirectory() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    stage_file("sub/a.txt", "content").await;

    let _sub_guard = ChangeDirGuard::new(temp_path.path().join("sub"));
    let result = mv::execute(MvArgs {
        paths: vec!["a.txt".to_string(), "b.txt".to_string()],
        verbose: false,
        dry_run: false,
        force: false,
    })
    .await;

    assert!(result.is_ok());
    assert!(!temp_path.path().join("sub/a.txt").exists());
    assert!(temp_path.path().join("sub/b.txt").exists());

    let index = Index::load(path::index()).unwrap();
    assert!(!index.tracked("sub/a.txt", 0));
    assert!(index.tracked("sub/b.txt", 0));
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

    let result = mv::execute(MvArgs {
        paths: vec!["src_dir".to_string(), "dest".to_string()],
        verbose: false,
        dry_run: false,
        force: false,
    })
    .await;
    assert!(result.is_ok());

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
/// When force-overwriting an already tracked destination, index should keep only the renamed destination entry.
async fn test_mv_force_overwrites_tracked_destination_and_replaces_index_entry() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    stage_file("src.txt", "new-content").await;
    stage_file("dst.txt", "old-content").await;

    let result = mv::execute(MvArgs {
        paths: vec!["src.txt".to_string(), "dst.txt".to_string()],
        verbose: false,
        dry_run: false,
        force: true,
    })
    .await;
    assert!(result.is_ok());

    assert!(!temp_path.path().join("src.txt").exists());
    let dst_content = fs::read_to_string(temp_path.path().join("dst.txt")).unwrap();
    assert_eq!(dst_content, "new-content");

    let index = Index::load(path::index()).unwrap();
    assert!(!index.tracked("src.txt", 0));
    assert!(index.tracked("dst.txt", 0));
}

#[tokio::test]
#[serial]
/// Refreshes destination index metadata/hash by rebuilding the entry from the moved file.
async fn test_mv_rebuilds_index_entry_from_destination_file() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    stage_file("src.txt", "actual-src-content").await;
    stage_file("other.txt", "different-content").await;

    let mut index = Index::load(path::index()).unwrap();
    let stale_hash = index.get("other.txt", 0).unwrap().hash;
    let mut src_entry = index.remove("src.txt", 0).unwrap();
    src_entry.hash = stale_hash;
    index.add(src_entry);
    index.save(path::index()).unwrap();

    let result = mv::execute(MvArgs {
        paths: vec!["src.txt".to_string(), "dst.txt".to_string()],
        verbose: false,
        dry_run: false,
        force: false,
    })
    .await;
    assert!(result.is_ok());

    let index = Index::load(path::index()).unwrap();
    let dst_entry_hash = index.get("dst.txt", 0).unwrap().hash;
    let expected_hash = calc_file_blob_hash(temp_path.path().join("dst.txt")).unwrap();

    assert_eq!(dst_entry_hash, expected_hash);
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
    assert!(stderr.contains("-v, --verbose    be verbose"));
    assert!(stderr.contains("-n, --dry-run    dry run"));
    assert!(stderr.contains("-f, --force      force move/rename even if target exists"));
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
/// Rejects source paths that escape repository boundary.
async fn test_mv_rejects_source_path_outside_workdir() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    let outside_src = temp_path
        .path()
        .parent()
        .unwrap()
        .join("mv_outside_src.txt");
    fs::write(&outside_src, "x").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .args(["mv", "../mv_outside_src.txt", "renamed.txt"])
        .output()
        .expect("failed to execute libra mv outside-source case");

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("is outside of the repository at"),
        "unexpected stderr: {stderr}"
    );
    assert!(outside_src.exists());
    assert!(!temp_path.path().join("renamed.txt").exists());
}

#[tokio::test]
#[serial]
/// Rejects destination paths that escape repository boundary.
async fn test_mv_rejects_destination_path_outside_workdir() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    stage_file("inside.txt", "x").await;
    let outside_dst = temp_path
        .path()
        .parent()
        .unwrap()
        .join("mv_outside_dst.txt");
    if outside_dst.exists() {
        fs::remove_file(&outside_dst).unwrap();
    }

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .args(["mv", "inside.txt", "../mv_outside_dst.txt"])
        .output()
        .expect("failed to execute libra mv outside-destination case");

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("is outside of the repository at"),
        "unexpected stderr: {stderr}"
    );
    assert!(temp_path.path().join("inside.txt").exists());
    assert!(!outside_dst.exists());
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
/// Rejects moving a path that is in conflicted (unmerged) index state.
async fn test_mv_rejects_conflicted_source_file() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    stage_file("conflict.txt", "base").await;

    let mut index = Index::load(path::index()).unwrap();
    let (stage0_hash, stage0_size) = {
        let stage0 = index
            .get("conflict.txt", 0)
            .expect("conflict.txt should be present at stage 0 before conflict setup");
        (stage0.hash, stage0.size)
    };

    for stage in 1..=3 {
        let mut entry =
            IndexEntry::new_from_blob("conflict.txt".to_string(), stage0_hash, stage0_size);
        entry.flags.stage = stage;
        index.add(entry);
    }
    index
        .save(path::index())
        .expect("failed to save conflict index entries");

    let index = Index::load(path::index()).unwrap();
    assert!(index.tracked("conflict.txt", 0));
    assert!((1..=3).all(|stage| index.tracked("conflict.txt", stage)));

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .args(["mv", "conflict.txt", "renamed.txt"])
        .output()
        .expect("failed to execute libra mv conflict case");

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("fatal: conflicted, source=conflict.txt, destination=renamed.txt"),
        "unexpected stderr: {stderr}"
    );
    assert!(temp_path.path().join("conflict.txt").exists());
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
/// Moves a directory even when it contains only untracked files.
async fn test_mv_moves_directory_without_tracked_files() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    fs::create_dir_all("src_dir").unwrap();
    test::ensure_file("src_dir/untracked.txt", Some("u"));
    fs::create_dir_all("dest").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .args(["mv", "src_dir", "dest"])
        .output()
        .expect("failed to execute libra mv untracked-only directory case");

    assert!(output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).is_empty(),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(!temp_path.path().join("src_dir/untracked.txt").exists());
    assert!(temp_path.path().join("dest/src_dir/untracked.txt").exists());
}

#[tokio::test]
#[serial]
/// Moves tracked and untracked files together for directory sources, but updates index only for tracked paths.
async fn test_mv_moves_mixed_directory_and_updates_only_tracked_index_entries() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    stage_file("src_dir/tracked.txt", "t").await;
    test::ensure_file("src_dir/untracked.txt", Some("u"));
    fs::create_dir_all("dest").unwrap();

    let result = mv::execute(MvArgs {
        paths: vec!["src_dir".to_string(), "dest".to_string()],
        verbose: false,
        dry_run: false,
        force: false,
    })
    .await;
    assert!(result.is_ok());

    assert!(temp_path.path().join("dest/src_dir/tracked.txt").exists());
    assert!(temp_path.path().join("dest/src_dir/untracked.txt").exists());

    let index = Index::load(path::index()).unwrap();
    assert!(!index.tracked("src_dir/tracked.txt", 0));
    assert!(index.tracked("dest/src_dir/tracked.txt", 0));
    assert!(!index.tracked("dest/src_dir/untracked.txt", 0));
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
/// Rejects moves where source and destination are the same path.
async fn test_mv_rejects_same_source_and_destination_path() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    test::ensure_file("same.txt", Some("x"));

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .args(["mv", "same.txt", "same.txt"])
        .output()
        .expect("failed to execute libra mv same-path case");

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(
            "fatal: can not move directory into itself, source=same.txt, destination=same.txt"
        ),
        "unexpected stderr: {stderr}"
    );
}

#[tokio::test]
#[serial]
/// Rejects moving a directory into its own subdirectory.
async fn test_mv_rejects_moving_directory_into_subdirectory() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    stage_file("dir/file.txt", "x").await;
    fs::create_dir_all("dir/sub").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .args(["mv", "dir", "dir/sub"])
        .output()
        .expect("failed to execute libra mv directory-into-subdirectory case");

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr
            .contains("fatal: can not move directory into itself, source=dir, destination=dir/sub"),
        "unexpected stderr: {stderr}"
    );

    assert!(temp_path.path().join("dir/file.txt").exists());
}
