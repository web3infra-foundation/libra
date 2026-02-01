//! Integration test: Add an empty file to the repository
//!
//! Ensures that Libra can handle adding empty files without errors

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
