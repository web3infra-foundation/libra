//! Integration test: Add a file in a nested subdirectory
//!
//! Ensures that Libra correctly handles files in deep directory structures

use std::{path::Path, process::Command};

use assert_cmd::prelude::*;
use tempfile::{TempDir, tempdir};

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
