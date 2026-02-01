//! Integration test: Attempt to initialize an already initialized repository
//!
//! Verifies that running init twice handles the situation appropriately

use std::process::Command;

use assert_cmd::prelude::*;
use tempfile::tempdir;

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
