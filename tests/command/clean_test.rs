//! Tests clean command removing untracked files with minimal flags.

use std::{fs, io::Write, process::Command};

use super::*;
use libra::utils::path;

#[tokio::test]
#[serial]
/// Tests dry-run mode does not delete files.
async fn test_clean_dry_run_keeps_files() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    let mut file = fs::File::create("untracked.txt").unwrap();
    file.write_all(b"content").unwrap();

    clean::execute(CleanArgs {
        dry_run: true,
        force: false,
    })
    .await;

    assert!(std::path::Path::new("untracked.txt").exists());
}

#[tokio::test]
#[serial]
/// Tests force mode deletes untracked files.
async fn test_clean_force_removes_files() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    let mut file = fs::File::create("untracked.txt").unwrap();
    file.write_all(b"content").unwrap();

    clean::execute(CleanArgs {
        dry_run: false,
        force: true,
    })
    .await;

    assert!(!std::path::Path::new("untracked.txt").exists());
}

#[tokio::test]
#[serial]
/// Tests requiring -f or -n to proceed.
async fn test_clean_requires_flag() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    let mut file = fs::File::create("untracked.txt").unwrap();
    file.write_all(b"content").unwrap();

    clean::execute(CleanArgs {
        dry_run: false,
        force: false,
    })
    .await;

    assert!(std::path::Path::new("untracked.txt").exists());
}

#[tokio::test]
#[serial]
/// Tests clean does not remove tracked files.
async fn test_clean_force_keeps_tracked_files() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    let mut file = fs::File::create("tracked.txt").unwrap();
    file.write_all(b"content").unwrap();

    add::execute(AddArgs {
        pathspec: vec![String::from("tracked.txt")],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
    })
    .await;

    clean::execute(CleanArgs {
        dry_run: false,
        force: true,
    })
    .await;

    assert!(std::path::Path::new("tracked.txt").exists());
}

#[tokio::test]
#[serial]
/// Tests clean removes untracked files in subdirectories.
async fn test_clean_force_removes_nested_untracked() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    fs::create_dir_all("dir/sub").unwrap();
    let mut file = fs::File::create("dir/sub/untracked.txt").unwrap();
    file.write_all(b"content").unwrap();

    clean::execute(CleanArgs {
        dry_run: false,
        force: true,
    })
    .await;

    assert!(!std::path::Path::new("dir/sub/untracked.txt").exists());
    assert!(std::path::Path::new("dir/sub").exists());
}

#[tokio::test]
#[serial]
/// Tests clean respects ignore rules for untracked files.
async fn test_clean_force_respects_ignore_rules() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    fs::write(".libraignore", "ignored.txt\n").unwrap();
    add::execute(AddArgs {
        pathspec: vec![String::from(".libraignore")],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
    })
    .await;

    fs::write("ignored.txt", "ignored").unwrap();
    fs::write("normal.txt", "normal").unwrap();

    clean::execute(CleanArgs {
        dry_run: false,
        force: true,
    })
    .await;

    assert!(std::path::Path::new("ignored.txt").exists());
    assert!(!std::path::Path::new("normal.txt").exists());
}

#[tokio::test]
#[serial]
/// Tests clean removes multiple untracked files but keeps tracked files.
async fn test_clean_force_multiple_untracked_with_tracked() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    let mut tracked = fs::File::create("tracked.txt").unwrap();
    tracked.write_all(b"content").unwrap();
    add::execute(AddArgs {
        pathspec: vec![String::from("tracked.txt")],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
    })
    .await;

    fs::write("untracked1.txt", "one").unwrap();
    fs::write("untracked2.txt", "two").unwrap();

    clean::execute(CleanArgs {
        dry_run: false,
        force: true,
    })
    .await;

    assert!(std::path::Path::new("tracked.txt").exists());
    assert!(!std::path::Path::new("untracked1.txt").exists());
    assert!(!std::path::Path::new("untracked2.txt").exists());
}

#[tokio::test]
#[serial]
/// Tests clean handles missing index by treating all files as untracked.
async fn test_clean_force_with_missing_index() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    let index_path = path::index();
    if index_path.exists() {
        fs::remove_file(index_path).unwrap();
    }

    fs::write("untracked.txt", "content").unwrap();

    clean::execute(CleanArgs {
        dry_run: false,
        force: true,
    })
    .await;

    assert!(!std::path::Path::new("untracked.txt").exists());
}

#[tokio::test]
#[serial]
/// Tests dry-run output format.
async fn test_clean_dry_run_output_format() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;

    let file_path = test_dir.path().join("untracked.txt");
    fs::write(&file_path, "content").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(test_dir.path())
        .arg("clean")
        .arg("-n")
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Would remove untracked.txt"));
}
