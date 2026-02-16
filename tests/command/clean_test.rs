//! Tests clean command removing untracked files with minimal flags.

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::{fs, io::Write, process::Command};

use libra::utils::path;

use super::*;

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

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(test_dir.path())
        .arg("clean")
        .output()
        .expect("failed to execute `libra clean`");

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("fatal: clean requires -f or -n"));

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
/// Tests clean fails on a corrupted index and does not delete files.
async fn test_clean_force_with_corrupted_index() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    let index_path = path::index();
    fs::write(&index_path, b"corrupted-index-data").unwrap();

    fs::write("untracked.txt", "content").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(test_dir.path())
        .arg("clean")
        .arg("-f")
        .output()
        .expect("failed to execute `libra clean`");

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Failed to load index"));
    assert!(std::path::Path::new("untracked.txt").exists());
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

#[tokio::test]
#[serial]
/// Tests that -f and -n together behave like dry-run (no deletion).
async fn test_clean_force_and_dry_run_prefers_dry_run() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;

    fs::write(test_dir.path().join("untracked.txt"), "content").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(test_dir.path())
        .arg("clean")
        .arg("-f")
        .arg("-n")
        .output()
        .expect("failed to execute `libra clean`");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Would remove untracked.txt"));
    assert!(test_dir.path().join("untracked.txt").exists());
}

#[tokio::test]
#[serial]
/// Tests clean can handle relatively long file paths.
async fn test_clean_force_with_long_path() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    let long_name = "a".repeat(200);
    let long_path = format!("dir/{long_name}.txt");
    fs::create_dir_all("dir").unwrap();
    fs::write(&long_path, "content").unwrap();

    clean::execute(CleanArgs {
        dry_run: false,
        force: true,
    })
    .await;

    assert!(!std::path::Path::new(&long_path).exists());
}

#[cfg(unix)]
#[tokio::test]
#[serial]
/// Tests clean does not delete files outside the workdir via symlinked directories.
async fn test_clean_force_does_not_follow_symlink_dirs() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    let outside_dir = tempdir().unwrap();
    let outside_file = outside_dir.path().join("outside.txt");
    fs::write(&outside_file, "content").unwrap();

    symlink(outside_dir.path(), "linked").unwrap();

    clean::execute(CleanArgs {
        dry_run: false,
        force: true,
    })
    .await;

    assert!(outside_file.exists());
}

#[cfg(unix)]
#[tokio::test]
#[serial]
/// Tests clean reports permission errors during deletion.
async fn test_clean_force_permission_error() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    fs::create_dir_all("protected").unwrap();
    fs::write("protected/untracked.txt", "content").unwrap();

    let mut perms = fs::metadata("protected").unwrap().permissions();
    perms.set_mode(0o555);
    fs::set_permissions("protected", perms).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(test_dir.path())
        .arg("clean")
        .arg("-f")
        .output()
        .expect("failed to execute `libra clean`");

    assert!(output.status.success());
    assert!(std::path::Path::new("protected/untracked.txt").exists());

    let mut perms = fs::metadata("protected").unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions("protected", perms).unwrap();
}
