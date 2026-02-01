//! Integration test: Initialize Libra repository in an empty directory
//!
//! Verifies that the init command creates a .libra directory

use assert_cmd::prelude::*;
use std::process::Command;
use tempfile::tempdir;

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
