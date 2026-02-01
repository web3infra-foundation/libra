//! Integration test: Status command after adding a file
//!
//! Verifies that the status command correctly reports added files

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
