//! Integration tests for r2cn submission
//!
//! These tests verify the basic functionality of the Libra version control system,
//! including initialization, adding files, and status reporting.

use assert_cmd::prelude::*;
use std::path::Path;
use std::process::Command;
use tempfile::{tempdir, TempDir};

/// Helper function to initialize a Libra repository in a temporary directory
fn init_libra_repo(dir: &Path) {
    Command::new(assert_cmd::cargo::cargo_bin!("libra"))
        .current_dir(dir)
        .arg("init")
        .assert()
        .success();
}

/// Helper function to create a test repository with initialization
fn setup_test_repo() -> TempDir {
    let dir = tempdir().unwrap();
    init_libra_repo(dir.path());
    dir
}

/// Test: Initialize Libra repository in an empty directory
///
/// Verifies that the init command creates a .libra directory
#[test]
fn test_init_in_empty_dir() {
    let dir = tempdir().unwrap();

    Command::new(assert_cmd::cargo::cargo_bin!("libra"))
        .current_dir(dir.path())
        .arg("init")
        .assert()
        .success();

    assert!(dir.path().join(".libra").exists());
}

/// Test: Add an empty file to the repository
///
/// Ensures that Libra can handle adding empty files without errors
#[test]
fn test_add_empty_file() {
    let dir = setup_test_repo();

    let file_path = dir.path().join("empty.txt");
    std::fs::File::create(&file_path).unwrap();

    Command::new(assert_cmd::cargo::cargo_bin!("libra"))
        .current_dir(dir.path())
        .arg("add")
        .arg("empty.txt")
        .assert()
        .success();
}

/// Test: Attempt to initialize an already initialized repository
///
/// Verifies that running init twice handles the situation appropriately
#[test]
fn test_double_init_warning() {
    let dir = tempdir().unwrap();

    Command::new(assert_cmd::cargo::cargo_bin!("libra"))
        .current_dir(dir.path())
        .arg("init")
        .assert()
        .success();

    // Second initialization should complete (behavior may vary by implementation)
    Command::new(assert_cmd::cargo::cargo_bin!("libra"))
        .current_dir(dir.path())
        .arg("init")
        .assert()
        .success();
}

/// Test: Add a file in a nested subdirectory
///
/// Ensures that Libra correctly handles files in deep directory structures
#[test]
fn test_add_sub_directory_file() {
    let dir = setup_test_repo();

    let sub_dir = dir.path().join("a/b/c");
    std::fs::create_dir_all(&sub_dir).unwrap();
    std::fs::write(sub_dir.join("deep.txt"), "hello deep").unwrap();

    Command::new(assert_cmd::cargo::cargo_bin!("libra"))
        .current_dir(dir.path())
        .arg("add")
        .arg("a/b/c/deep.txt")
        .assert()
        .success();
}

/// Test: Status command after adding a file
///
/// Verifies that the status command correctly reports added files
#[test]
fn test_status_after_add() {
    let dir = setup_test_repo();

    std::fs::write(dir.path().join("test.txt"), "content").unwrap();

    Command::new(assert_cmd::cargo::cargo_bin!("libra"))
        .current_dir(dir.path())
        .arg("add")
        .arg("test.txt")
        .assert()
        .success();

    let output = Command::new(assert_cmd::cargo::cargo_bin!("libra"))
        .current_dir(dir.path())
        .arg("status")
        .output()
        .expect("Failed to execute status command");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("test.txt"),
        "Status output should contain the added file"
    );
}
