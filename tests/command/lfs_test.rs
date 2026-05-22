//! Tests LFS subcommands covering upload/download negotiation, locks, and tracking detection.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use std::{fs, path::Path, process::Command};

use axum::{
    Json, Router,
    http::StatusCode,
    routing::{get, post},
};
use serde_json::json;
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

/// Spawn an axum-based mock LFS server on a free port and return the bound address.
/// The returned `JoinHandle` is dropped by the caller when the test finishes, which
/// aborts the server task.
async fn spawn_mock_lfs_server(app: Router) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind mock LFS listener");
    let addr = listener
        .local_addr()
        .expect("failed to read mock LFS bound address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    addr
}

/// Initialize an isolated libra repo wired to the given LFS server URL via the `origin`
/// remote, so `LFSClient::new()` resolves through `branch.main.remote=origin` at runtime.
fn init_repo_with_mock_remote(remote_url: &str) -> TempDir {
    let repo = init_temp_repo();
    let repo_path = repo.path();

    let add_remote = libra_command(repo_path)
        .args(["remote", "add", "origin", remote_url])
        .output()
        .expect("failed to add mock remote");
    assert!(
        add_remote.status.success(),
        "remote add failed: {}",
        String::from_utf8_lossy(&add_remote.stderr)
    );

    let set_upstream = libra_command(repo_path)
        .args(["config", "branch.main.remote", "origin"])
        .output()
        .expect("failed to set branch upstream remote");
    assert!(
        set_upstream.status.success(),
        "config set failed: {}",
        String::from_utf8_lossy(&set_upstream.stderr)
    );

    repo
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
/// Track with duplicate patterns in a single invocation must record each pattern only
/// once, both in the structured output and in the on-disk attributes file. This pins
/// the dedupe behaviour that prevents `libra lfs track foo foo` from appending `foo`
/// twice (regression guard for the legacy "TODO: deduplicate" path in `run_lfs`).
async fn test_lfs_track_deduplicates_repeated_patterns() {
    let temp_repo = init_temp_repo();
    let temp_path = temp_repo.path();

    let track_output = libra_command(temp_path)
        .args(["--json", "lfs", "track", "*.txt", "*.txt"])
        .output()
        .expect("Failed to track path");
    assert!(
        track_output.status.success(),
        "Failed to track path: {}",
        String::from_utf8_lossy(&track_output.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&track_output.stdout).expect("track stdout should be JSON");
    let patterns = json["data"]["patterns"]
        .as_array()
        .expect("patterns should be an array");
    assert_eq!(
        patterns.len(),
        1,
        "duplicate input pattern must be reported once: {patterns:?}"
    );
    assert_eq!(patterns[0], "*.txt");

    // Re-listing should also report a single entry; the on-disk attributes file must
    // not contain duplicate filter=lfs lines.
    let attributes = fs::read_to_string(temp_path.join(".libra_attributes"))
        .expect("attributes file should exist after tracking");
    let lfs_lines = attributes
        .lines()
        .filter(|line| line.contains("filter=lfs"))
        .count();
    assert_eq!(
        lfs_lines, 1,
        "attributes file should contain exactly one filter=lfs line, got: {attributes}",
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
    let file = &json["data"]["files"][0];
    assert_eq!(file["path"], "tracked_file.txt");
    assert!(file["size"].as_u64().is_some());
    // `oid` is the display oid (10-char prefix by default), `full_oid` always carries
    // the canonical 64-char hash so `--json` consumers don't have to pass `--long`.
    let display_oid = file["oid"].as_str().expect("oid should be a string");
    let full_oid = file["full_oid"]
        .as_str()
        .expect("full_oid should be a string");
    assert_eq!(display_oid.len(), 10);
    assert_eq!(full_oid.len(), 64);
    assert!(full_oid.starts_with(display_oid));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
/// `lfs locks --json` against a mock server that returns one lock; verifies the JSON
/// envelope surfaces the locks list and matches the `LfsOutput` schema.
async fn test_lfs_locks_cli_returns_locks_from_mock_server() {
    let app = Router::new().route(
        "/locks",
        get(|| async {
            Json(json!({
                "locks": [{
                    "id": "lock-1",
                    "path": "tracked.txt",
                    "locked_at": "2026-01-01T00:00:00Z",
                    "owner": { "name": "tester" }
                }],
                "next_cursor": ""
            }))
        }),
    );
    let addr = spawn_mock_lfs_server(app).await;
    let repo = init_repo_with_mock_remote(&format!("http://{addr}"));
    let repo_path = repo.path().to_path_buf();

    let output = tokio::task::spawn_blocking(move || {
        libra_command(&repo_path)
            .args(["--json", "lfs", "locks"])
            .output()
            .expect("failed to run lfs locks")
    })
    .await
    .expect("spawn_blocking join failed");

    assert!(
        output.status.success(),
        "lfs locks should succeed; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("locks stdout should be JSON");
    assert_eq!(stdout["command"], "lfs");
    assert_eq!(stdout["data"]["action"], "locks");
    assert_eq!(stdout["data"]["locks"][0]["path"], "tracked.txt");
    assert_eq!(stdout["data"]["locks"][0]["id"], "lock-1");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
/// `lfs locks --json` against a mock server that returns 403; verifies the CLI surfaces
/// stable error code `LBR-AUTH-002` and exits non-zero.
async fn test_lfs_locks_cli_forbidden_returns_auth_permission_denied() {
    let app = Router::new().route("/locks", get(|| async { StatusCode::FORBIDDEN }));
    let addr = spawn_mock_lfs_server(app).await;
    let repo = init_repo_with_mock_remote(&format!("http://{addr}"));
    let repo_path = repo.path().to_path_buf();

    let output = tokio::task::spawn_blocking(move || {
        libra_command(&repo_path)
            .args(["--json", "lfs", "locks"])
            .output()
            .expect("failed to run lfs locks")
    })
    .await
    .expect("spawn_blocking join failed");

    assert!(
        !output.status.success(),
        "lfs locks should fail when server returns 403"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    let envelope: serde_json::Value = serde_json::from_str(stderr.trim())
        .unwrap_or_else(|err| panic!("error envelope should be JSON: {err}; stderr={stderr}"));
    assert_eq!(envelope["error_code"], "LBR-AUTH-002");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
/// `lfs lock --json` against a mock server that accepts the request; verifies the JSON
/// envelope reports the locked path and action.
async fn test_lfs_lock_cli_success_with_mock_server() {
    let app = Router::new().route(
        "/locks",
        post(|| async {
            (
                StatusCode::CREATED,
                Json(json!({
                    "lock": {
                        "id": "lock-1",
                        "path": "tracked.txt",
                        "locked_at": "2026-01-01T00:00:00Z",
                        "owner": { "name": "tester" }
                    }
                })),
            )
        }),
    );
    let addr = spawn_mock_lfs_server(app).await;
    let repo = init_repo_with_mock_remote(&format!("http://{addr}"));
    let repo_path = repo.path().to_path_buf();
    fs::write(repo_path.join("tracked.txt"), "content").expect("failed to create tracked file");

    let output = tokio::task::spawn_blocking(move || {
        libra_command(&repo_path)
            .args(["--json", "lfs", "lock", "tracked.txt"])
            .output()
            .expect("failed to run lfs lock")
    })
    .await
    .expect("spawn_blocking join failed");

    assert!(
        output.status.success(),
        "lfs lock should succeed; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("lock stdout should be JSON");
    assert_eq!(stdout["command"], "lfs");
    assert_eq!(stdout["data"]["action"], "lock");
    assert_eq!(stdout["data"]["path"], "tracked.txt");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
/// `lfs lock --json` against a mock server that returns 409; verifies the CLI surfaces
/// stable error code `LBR-CONFLICT-002` and exits non-zero.
async fn test_lfs_lock_cli_conflict_returns_conflict_blocked() {
    let app = Router::new().route("/locks", post(|| async { StatusCode::CONFLICT }));
    let addr = spawn_mock_lfs_server(app).await;
    let repo = init_repo_with_mock_remote(&format!("http://{addr}"));
    let repo_path = repo.path().to_path_buf();
    fs::write(repo_path.join("tracked.txt"), "content").expect("failed to create tracked file");

    let output = tokio::task::spawn_blocking(move || {
        libra_command(&repo_path)
            .args(["--json", "lfs", "lock", "tracked.txt"])
            .output()
            .expect("failed to run lfs lock")
    })
    .await
    .expect("spawn_blocking join failed");

    assert!(
        !output.status.success(),
        "lfs lock should fail when server returns 409"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    let envelope: serde_json::Value = serde_json::from_str(stderr.trim())
        .unwrap_or_else(|err| panic!("error envelope should be JSON: {err}; stderr={stderr}"));
    assert_eq!(envelope["error_code"], "LBR-CONFLICT-002");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
/// `lfs unlock --json --force --id <id>` against a mock server that accepts the request;
/// verifies the JSON envelope reports the unlocked path.
async fn test_lfs_unlock_cli_success_with_force_and_id() {
    let app = Router::new().route(
        "/locks/{id}/unlock",
        post(|| async {
            Json(json!({
                "lock": {
                    "id": "lock-1",
                    "path": "tracked.txt",
                    "locked_at": "2026-01-01T00:00:00Z",
                    "owner": { "name": "tester" }
                }
            }))
        }),
    );
    let addr = spawn_mock_lfs_server(app).await;
    let repo = init_repo_with_mock_remote(&format!("http://{addr}"));
    let repo_path = repo.path().to_path_buf();

    let output = tokio::task::spawn_blocking(move || {
        libra_command(&repo_path)
            .args([
                "--json",
                "lfs",
                "unlock",
                "tracked.txt",
                "--force",
                "--id",
                "lock-1",
            ])
            .output()
            .expect("failed to run lfs unlock")
    })
    .await
    .expect("spawn_blocking join failed");

    assert!(
        output.status.success(),
        "lfs unlock should succeed; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("unlock stdout should be JSON");
    assert_eq!(stdout["command"], "lfs");
    assert_eq!(stdout["data"]["action"], "unlock");
    assert_eq!(stdout["data"]["path"], "tracked.txt");
}
