//! Tests LFS subcommands covering upload/download negotiation, locks, and tracking detection.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use std::{fs, path::Path, process::Command};

use tempfile::TempDir;

/// Build a `Command` for the Libra binary with an isolated HOME.
fn libra_command(cwd: &Path) -> Command {
    let home = cwd.join(".libra-test-home");
    let config_home = home.join(".config");
    fs::create_dir_all(&config_home).expect("failed to create isolated HOME");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_libra"));
    cmd.current_dir(cwd)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("USERPROFILE", &home);
    cmd
}

/// Helper function: Initialize a temporary Libra repository
fn init_temp_repo() -> TempDir {
    let temp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
    let temp_path = temp_dir.path();

    let output = libra_command(temp_path)
        .args(["init"])
        .output()
        .expect("Failed to execute libra binary");

    if !output.status.success() {
        panic!(
            "Failed to initialize libra repository: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    temp_dir
}

#[tokio::test]
/// Test track/untrack path rule management
async fn test_lfs_track_untrack() {
    let temp_repo = init_temp_repo();
    let temp_path = temp_repo.path();

    let track_output = libra_command(temp_path)
        .args(["lfs", "track", "*.txt"])
        .output()
        .expect("Failed to track path");
    assert!(
        track_output.status.success(),
        "Failed to track path: {}",
        String::from_utf8_lossy(&track_output.stderr)
    );

    let untrack_output = libra_command(temp_path)
        .args(["lfs", "untrack", "*.txt"])
        .output()
        .expect("Failed to untrack path");
    assert!(
        untrack_output.status.success(),
        "Failed to untrack path: {}",
        String::from_utf8_lossy(&untrack_output.stderr)
    );
}

#[tokio::test]
/// Test JSON output for local LFS tracking operations.
async fn test_lfs_track_and_untrack_json_output() {
    let temp_repo = init_temp_repo();
    let temp_path = temp_repo.path();

    let track_output = libra_command(temp_path)
        .args(["--json", "lfs", "track", "*.txt"])
        .output()
        .expect("Failed to track path");
    assert!(
        track_output.status.success(),
        "Failed to track path: {}",
        String::from_utf8_lossy(&track_output.stderr)
    );
    assert!(track_output.stderr.is_empty());
    let json: serde_json::Value =
        serde_json::from_slice(&track_output.stdout).expect("track stdout should be JSON");
    assert_eq!(json["command"], "lfs");
    assert_eq!(json["data"]["action"], "track");
    assert_eq!(json["data"]["patterns"][0], "*.txt");

    let list_output = libra_command(temp_path)
        .args(["--json", "lfs", "track"])
        .output()
        .expect("Failed to list tracked patterns");
    assert!(
        list_output.status.success(),
        "Failed to list tracked patterns: {}",
        String::from_utf8_lossy(&list_output.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_slice(&list_output.stdout).expect("track list stdout should be JSON");
    assert_eq!(json["data"]["action"], "track-list");
    assert_eq!(json["data"]["patterns"][0], "*.txt");

    let untrack_output = libra_command(temp_path)
        .args(["--json", "lfs", "untrack", "*.txt"])
        .output()
        .expect("Failed to untrack path");
    assert!(
        untrack_output.status.success(),
        "Failed to untrack path: {}",
        String::from_utf8_lossy(&untrack_output.stderr)
    );
    assert!(untrack_output.stderr.is_empty());
    let json: serde_json::Value =
        serde_json::from_slice(&untrack_output.stdout).expect("untrack stdout should be JSON");
    assert_eq!(json["data"]["action"], "untrack");
    assert_eq!(json["data"]["patterns"][0], "*.txt");
}

#[tokio::test]
/// Test file status viewing
async fn test_lfs_ls_files() {
    let temp_repo = init_temp_repo();
    let temp_path = temp_repo.path();

    // Create a test file and add it to LFS
    let file_path = temp_path.join("tracked_file.txt");
    std::fs::write(&file_path, "Tracked content").expect("Failed to create tracked file");

    libra_command(temp_path)
        .args(["lfs", "track", "*.txt"])
        .output()
        .expect("Failed to track file");

    libra_command(temp_path)
        .args(["add", "tracked_file.txt"])
        .output()
        .expect("Failed to add file to LFS");

    let ls_files_output = libra_command(temp_path)
        .args(["lfs", "ls-files"])
        .output()
        .expect("Failed to list LFS files");
    assert!(
        ls_files_output.status.success(),
        "Failed to list LFS files: {}",
        String::from_utf8_lossy(&ls_files_output.stderr)
    );

    let stdout = String::from_utf8_lossy(&ls_files_output.stdout);
    assert!(
        stdout.contains("tracked_file.txt"),
        "LFS file list does not contain expected file: {stdout}",
    );
}

#[tokio::test]
/// Test JSON output for LFS file listing.
async fn test_lfs_ls_files_json_output() {
    let temp_repo = init_temp_repo();
    let temp_path = temp_repo.path();

    let file_path = temp_path.join("tracked_file.txt");
    std::fs::write(&file_path, "Tracked content").expect("Failed to create tracked file");

    libra_command(temp_path)
        .args(["lfs", "track", "*.txt"])
        .output()
        .expect("Failed to track file");

    libra_command(temp_path)
        .args(["add", "tracked_file.txt"])
        .output()
        .expect("Failed to add file to LFS");

    let output = libra_command(temp_path)
        .args(["--json", "lfs", "ls-files", "--size"])
        .output()
        .expect("Failed to list LFS files");
    assert!(
        output.status.success(),
        "Failed to list LFS files: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("ls-files stdout should be JSON");
    assert_eq!(json["command"], "lfs");
    assert_eq!(json["data"]["action"], "ls-files");
    assert_eq!(json["data"]["show_size"], true);
    assert_eq!(json["data"]["files"][0]["path"], "tracked_file.txt");
    assert!(json["data"]["files"][0]["size"].as_u64().is_some());
}
