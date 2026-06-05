//! Tests LFS subcommands covering upload/download negotiation, locks, and tracking detection.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use std::{fs, path::Path, process::Command};

use axum::{
    Json, Router,
    http::StatusCode,
    routing::{get, post, put},
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
/// Pre-v0.17.1065 `libra lfs track` (list mode) printed nothing at all
/// on a fresh repo with no tracked patterns — the user could not tell
/// whether the command had run or hung. Pin the new behavior: the
/// "Listing tracked patterns" header is always emitted so empty is a
/// confirmed-empty, not a silent no-op.
async fn test_lfs_track_list_prints_header_on_empty_repo() {
    let temp_repo = init_temp_repo();
    let temp_path = temp_repo.path();

    let output = libra_command(temp_path)
        .args(["lfs", "track"])
        .output()
        .expect("failed to run lfs track");
    assert!(
        output.status.success(),
        "lfs track (list) should succeed on empty repo: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Listing tracked patterns"),
        "empty-repo lfs track should still print the header, stdout={stdout:?}"
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

#[tokio::test]
/// Pre-v0.17.1067 `libra lfs track "*.txt"` ran twice in a row would
/// produce zero stdout on the second invocation — the dedup fix in
/// v0.17.1057 returned an empty `added` Vec and the human renderer
/// silently no-op'd. Pin the confirmed-already-tracked notice so the
/// command never looks like a hang.
async fn test_lfs_track_prints_notice_when_all_patterns_already_tracked() {
    let temp_repo = init_temp_repo();
    let temp_path = temp_repo.path();

    // First track adds the pattern; second track has nothing new to add.
    libra_command(temp_path)
        .args(["lfs", "track", "*.txt"])
        .output()
        .expect("first track should succeed");

    let output = libra_command(temp_path)
        .args(["lfs", "track", "*.txt"])
        .output()
        .expect("second track should succeed");
    assert!(
        output.status.success(),
        "second track should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("No new patterns added (already tracked)"),
        "duplicate-track should print the already-tracked notice, stdout={stdout:?}"
    );
}

#[tokio::test]
/// Pre-v0.17.1067 `libra lfs untrack "*.txt"` on a pattern that was
/// never tracked produced zero stdout — the human renderer silently
/// no-op'd. Pin the confirmed-no-match notice.
async fn test_lfs_untrack_prints_notice_when_no_match() {
    let temp_repo = init_temp_repo();
    let temp_path = temp_repo.path();

    let output = libra_command(temp_path)
        .args(["lfs", "untrack", "*.never-tracked"])
        .output()
        .expect("untrack should succeed");
    assert!(
        output.status.success(),
        "untrack of an untracked pattern should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("No matching LFS patterns to untrack"),
        "no-match untrack should print the no-op notice, stdout={stdout:?}"
    );
}

#[tokio::test]
/// Pre-v0.17.1067 `libra lfs ls-files` on a repo with no LFS-tracked
/// files printed zero stdout. Pin the confirmed-empty notice for the
/// default human path while preserving silence under `--name-only`
/// (which shell pipelines rely on).
async fn test_lfs_ls_files_prints_notice_when_empty_but_silent_with_name_only() {
    let temp_repo = init_temp_repo();
    let temp_path = temp_repo.path();

    // Default human mode → notice present.
    let output = libra_command(temp_path)
        .args(["lfs", "ls-files"])
        .output()
        .expect("ls-files should succeed");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("No LFS files"),
        "empty ls-files should print the no-op notice, stdout={stdout:?}"
    );

    // --name-only → silent (pipeline consumers).
    let output = libra_command(temp_path)
        .args(["lfs", "ls-files", "--name-only"])
        .output()
        .expect("ls-files --name-only should succeed");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.trim().is_empty(),
        "empty ls-files --name-only should stay silent, stdout={stdout:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
/// Pre-v0.17.1066 `libra lfs locks` (human mode) printed nothing when
/// the server returned an empty list — same silent-no-op UX class as
/// the `track-list` fix in v0.17.1065. Pin the new "No locks on the
/// current branch" notice so users always see a confirmed-empty signal.
async fn test_lfs_locks_human_prints_notice_when_empty() {
    let app = Router::new().route(
        "/locks",
        get(|| async { Json(json!({ "locks": [], "next_cursor": "" })) }),
    );
    let addr = spawn_mock_lfs_server(app).await;
    let repo = init_repo_with_mock_remote(&format!("http://{addr}"));
    let repo_path = repo.path().to_path_buf();

    let output = tokio::task::spawn_blocking(move || {
        libra_command(&repo_path)
            .args(["lfs", "locks"])
            .output()
            .expect("failed to run lfs locks")
    })
    .await
    .expect("spawn_blocking join failed");

    assert!(
        output.status.success(),
        "lfs locks should succeed on empty server response; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("No locks"),
        "empty `lfs locks` should still print a notice, stdout={stdout:?}"
    );
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
/// `lfs unlock <path>` (no `--id`) exercises the path → id lookup
/// branch in `LfsCmds::Unlock`: the CLI first calls `GET /locks?path=...`
/// to resolve the lock id, then issues `POST /locks/<id>/unlock`. No
/// prior CLI test covered this branch — only the `--id`-supplied paths.
async fn test_lfs_unlock_by_path_resolves_lock_id_via_get_locks() {
    let app = Router::new()
        .route(
            "/locks",
            get(|| async {
                Json(json!({
                    "locks": [{
                        "id": "lock-by-path",
                        "path": "tracked.bin",
                        "locked_at": "2026-01-01T00:00:00Z",
                        "owner": { "name": "tester" }
                    }],
                    "next_cursor": ""
                }))
            }),
        )
        .route(
            "/locks/{id}/unlock",
            post(|| async {
                Json(json!({
                    "lock": {
                        "id": "lock-by-path",
                        "path": "tracked.bin",
                        "locked_at": "2026-01-01T00:00:00Z",
                        "owner": { "name": "tester" }
                    }
                }))
            }),
        );
    let addr = spawn_mock_lfs_server(app).await;
    let repo = init_repo_with_mock_remote(&format!("http://{addr}"));
    let repo_path = repo.path().to_path_buf();

    // Use `--force` to bypass the path-existence + clean-tree pre-checks.
    // `force` short-circuits the pre-check guard in `LfsCmds::Unlock` but
    // does *not* skip the `id.is_none()` lookup branch in the unlock body,
    // which is exactly what this test exercises: the path → id resolution
    // via `get_locks`. We assert the resolved id came from the server
    // response, proving we went through the path branch and not a `--id`
    // arg.
    let output = tokio::task::spawn_blocking(move || {
        libra_command(&repo_path)
            .args(["--json", "lfs", "unlock", "tracked.bin", "--force"])
            .output()
            .expect("failed to run lfs unlock")
    })
    .await
    .expect("spawn_blocking join failed");

    assert!(
        output.status.success(),
        "lfs unlock by path should succeed; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("unlock stdout should be JSON");
    assert_eq!(stdout["data"]["action"], "unlock");
    assert_eq!(stdout["data"]["path"], "tracked.bin");
    // The id must come from the get_locks response, not from a --id arg.
    assert_eq!(stdout["data"]["id"], "lock-by-path");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
/// Pre-v0.17.1071 `current_refspec` printed
/// `"fatal: HEAD is detached"` via `emit_legacy_stderr` then returned
/// `None`. Every caller wrapped the `None` in a typed error and
/// reported it again through the normal `OutputConfig` error renderer.
/// Net effect: detached-HEAD users (especially `--json` consumers) saw
/// two stderr lines for a single failure — the legacy text plus the
/// typed envelope.
///
/// Pin the deduplicated behavior by running `lfs locks --json` on a
/// detached HEAD and asserting stderr parses as exactly one JSON
/// envelope (no leading legacy line, no trailing duplicate).
async fn test_lfs_locks_on_detached_head_emits_single_error_envelope() {
    let temp_repo = init_temp_repo();
    let temp_path = temp_repo.path();

    // Need at least one commit so HEAD can be detached to it.
    for (k, v) in [
        ("user.name", "tester"),
        ("user.email", "tester@example.com"),
    ] {
        let cfg = libra_command(temp_path)
            .args(["config", k, v])
            .output()
            .unwrap();
        assert!(
            cfg.status.success(),
            "config {k}: {}",
            String::from_utf8_lossy(&cfg.stderr)
        );
    }
    fs::write(temp_path.join("seed.txt"), b"hi").unwrap();
    let add = libra_command(temp_path)
        .args(["add", "seed.txt"])
        .output()
        .unwrap();
    assert!(
        add.status.success(),
        "add: {}",
        String::from_utf8_lossy(&add.stderr)
    );
    let commit = libra_command(temp_path)
        .args(["commit", "-m", "seed"])
        .output()
        .unwrap();
    assert!(
        commit.status.success(),
        "commit: {}",
        String::from_utf8_lossy(&commit.stderr)
    );

    // Detach HEAD by checking out the commit hash directly.
    let head = libra_command(temp_path)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let head_hash = String::from_utf8_lossy(&head.stdout).trim().to_string();
    assert!(
        !head_hash.is_empty(),
        "rev-parse HEAD returned empty; stderr={}",
        String::from_utf8_lossy(&head.stderr)
    );
    let detach = libra_command(temp_path)
        .args(["switch", "--detach", &head_hash])
        .output()
        .unwrap();
    assert!(
        detach.status.success(),
        "switch --detach {head_hash}: {}",
        String::from_utf8_lossy(&detach.stderr)
    );

    let output = libra_command(temp_path)
        .args(["--json", "lfs", "locks"])
        .output()
        .expect("lfs locks should run");
    assert!(
        !output.status.success(),
        "lfs locks on detached HEAD should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    let trimmed = stderr.trim();

    // The whole stderr must parse as exactly one JSON envelope; no
    // unwrapped "fatal: HEAD is detached" line leaking before it.
    let envelope: serde_json::Value = serde_json::from_str(trimmed).unwrap_or_else(|err| {
        panic!("stderr should parse as a single JSON envelope: {err}; stderr={trimmed:?}")
    });
    assert_eq!(envelope["error_code"], "LBR-REPO-003");

    // Defensive: the legacy text "fatal: HEAD is detached" must NOT
    // appear as a standalone line before the JSON envelope.
    assert!(
        !trimmed.starts_with("fatal: HEAD is detached"),
        "stderr should not begin with the legacy plain-text error; stderr={trimmed:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
/// `lfs unlock <path>` (no `--id`) must surface a typed error when
/// `get_locks?path=...` returns an empty list — there is no id to
/// unlock by. Asserts the fatal error envelope carries
/// `LBR-REPO-001` (`RepoStateInvalid`) and a hint-bearing message
/// rather than a generic 500 from the unlock leg or, worse, a panic.
async fn test_lfs_unlock_by_path_returns_typed_error_when_no_lock_found() {
    let app = Router::new().route(
        "/locks",
        get(|| async { Json(json!({ "locks": [], "next_cursor": "" })) }),
    );
    let addr = spawn_mock_lfs_server(app).await;
    let repo = init_repo_with_mock_remote(&format!("http://{addr}"));
    let repo_path = repo.path().to_path_buf();

    let output = tokio::task::spawn_blocking(move || {
        libra_command(&repo_path)
            .args(["--json", "lfs", "unlock", "absent.bin", "--force"])
            .output()
            .expect("failed to run lfs unlock")
    })
    .await
    .expect("spawn_blocking join failed");

    assert!(
        !output.status.success(),
        "lfs unlock without a lock should fail; stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    let envelope: serde_json::Value = serde_json::from_str(stderr.trim())
        .unwrap_or_else(|err| panic!("error envelope should be JSON: {err}; stderr={stderr}"));
    assert_eq!(envelope["error_code"], "LBR-REPO-003");
    assert!(
        envelope["message"]
            .as_str()
            .is_some_and(|m| m.contains("no lock found for path 'absent.bin'")),
        "message should mention the offending path; envelope={envelope}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
/// `lfs unlock --id <id> <path>` should succeed when the path does not
/// exist locally — `--id` makes the path purely a label (the id is the
/// lookup key on the server). Prior to the fix, this case required
/// `--force`, which has stronger semantics (force-release a lock you do
/// not own).
async fn test_lfs_unlock_with_id_skips_path_existence_check() {
    let app = Router::new().route(
        "/locks/{id}/unlock",
        post(|| async {
            Json(json!({
                "lock": {
                    "id": "lock-99",
                    "path": "deleted.bin",
                    "locked_at": "2026-01-01T00:00:00Z",
                    "owner": { "name": "tester" }
                }
            }))
        }),
    );
    let addr = spawn_mock_lfs_server(app).await;
    let repo = init_repo_with_mock_remote(&format!("http://{addr}"));
    let repo_path = repo.path().to_path_buf();

    // Note: no `--force`, and `deleted.bin` does not exist in the repo.
    let output = tokio::task::spawn_blocking(move || {
        libra_command(&repo_path)
            .args(["--json", "lfs", "unlock", "deleted.bin", "--id", "lock-99"])
            .output()
            .expect("failed to run lfs unlock")
    })
    .await
    .expect("spawn_blocking join failed");

    assert!(
        output.status.success(),
        "lfs unlock --id should bypass path check; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("unlock stdout should be JSON");
    assert_eq!(stdout["data"]["action"], "unlock");
    assert_eq!(stdout["data"]["id"], "lock-99");
    assert_eq!(stdout["data"]["path"], "deleted.bin");
}

/// `libra lfs --help` surfaces the EXAMPLES banner so users see the
/// canonical invocation per sub-command (`track`, `untrack`, `ls-files`,
/// `locks`, `lock`, `unlock`) plus a JSON variant without reading the
/// design doc. Cross-cutting `--help` EXAMPLES rollout per
/// `docs/improvement/README.md` item B.
#[test]
fn test_lfs_help_lists_examples_banner() {
    let repo = tempfile::tempdir().expect("tempdir for lfs --help");
    let output = libra_command(repo.path())
        .args(["lfs", "--help"])
        .output()
        .expect("failed to run libra lfs --help");
    assert!(
        output.status.success(),
        "lfs --help should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("EXAMPLES:"),
        "lfs --help should include EXAMPLES banner, stdout: {stdout}"
    );
    assert!(
        stdout.contains(".libra_attributes"),
        "lfs --help should name the real Libra attributes file, stdout: {stdout}"
    );
    assert!(
        !stdout.contains(".libraattributes"),
        "lfs --help should not mention the old misspelled attributes file, stdout: {stdout}"
    );
    for invocation in [
        "libra lfs track",
        "libra lfs untrack",
        "libra lfs ls-files",
        "libra lfs locks",
        "libra lfs lock build/output.bin",
        "libra lfs unlock build/output.bin",
        "libra lfs unlock --force",
        "libra lfs install",
        "libra lfs push origin main",
        "libra lfs fetch origin main",
        "libra lfs prune --dry-run",
        "libra lfs checkout",
        "libra lfs --json ls-files",
    ] {
        assert!(
            stdout.contains(invocation),
            "lfs --help should include `{invocation}`, stdout: {stdout}"
        );
    }
}

// ── LFS push/fetch/prune/checkout/install CLI structure (lfs-improvement-plan Batch 0) ──

#[test]
fn test_lfs_install_noop_exits_zero() {
    let repo = init_temp_repo();
    let output = libra_command(repo.path())
        .args(["lfs", "install"])
        .output()
        .expect("failed to run libra lfs install");
    assert!(
        output.status.success(),
        "lfs install should exit 0, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("native built-in LFS"),
        "expected built-in LFS notice, stdout: {stdout}"
    );
    assert!(
        output.stderr.is_empty(),
        "install should not write to stderr, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_lfs_uninstall_noop_exits_zero() {
    let repo = init_temp_repo();
    let output = libra_command(repo.path())
        .args(["lfs", "uninstall"])
        .output()
        .expect("failed to run libra lfs uninstall");
    assert!(
        output.status.success(),
        "lfs uninstall should exit 0, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("nothing to uninstall"),
        "expected uninstall no-op notice, stdout: {stdout}"
    );
    assert!(
        output.stderr.is_empty(),
        "uninstall should not write stderr"
    );
}

#[test]
fn test_lfs_install_json_action() {
    let repo = init_temp_repo();
    for (sub, action) in [("install", "install"), ("uninstall", "uninstall")] {
        let output = libra_command(repo.path())
            .args(["--json", "lfs", sub])
            .output()
            .expect("failed to run libra --json lfs <sub>");
        assert!(
            output.status.success(),
            "lfs {sub} --json should exit 0, stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let json: serde_json::Value = serde_json::from_slice(&output.stdout)
            .expect("stdout should be a single JSON envelope");
        assert_eq!(json["command"], "lfs");
        assert_eq!(json["data"]["action"], action);
    }
}

#[test]
fn test_lfs_help_lists_new_subcommands() {
    let repo = tempfile::tempdir().expect("tempdir");
    let output = libra_command(repo.path())
        .args(["lfs", "--help"])
        .output()
        .expect("failed to run libra lfs --help");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    for sub in ["install", "uninstall", "push", "fetch", "prune", "checkout"] {
        assert!(
            stdout.contains(sub),
            "lfs --help should list `{sub}` subcommand, stdout: {stdout}"
        );
    }
}

#[test]
fn test_lfs_invalid_flag_clap_error() {
    let repo = init_temp_repo();
    let output = libra_command(repo.path())
        .args(["lfs", "push", "--invalid-flag"])
        .output()
        .expect("failed to run libra lfs push --invalid-flag");
    // Libra intercepts clap parse errors and remaps them to the stable
    // `LBR-CLI-002` (CliInvalidArguments) surface, which exits 129 (not clap's
    // raw 2). The error text still carries the clap "unexpected argument" line.
    assert_eq!(
        output.status.code(),
        Some(129),
        "unknown flag should map to LBR-CLI-002 (exit 129), stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("LBR-CLI-002") && stderr.contains("unexpected argument"),
        "expected clap-arg error remapped to LBR-CLI-002, stderr: {stderr}"
    );
}

#[test]
fn test_lfs_deferred_subcommands_parse_not_clap_error() {
    let repo = init_temp_repo();
    // prune/checkout parse successfully but are not yet implemented: they
    // dispatch and return a typed "not yet implemented" error (exit 128) — never
    // a clap error (exit 2). (`push`/`fetch` are implemented and tested separately.)
    for args in [vec!["lfs", "prune"], vec!["lfs", "checkout"]] {
        let output = libra_command(repo.path())
            .args(&args)
            .output()
            .unwrap_or_else(|e| panic!("failed to run libra {args:?}: {e}"));
        assert_ne!(
            output.status.code(),
            Some(2),
            "`{args:?}` should parse (not a clap error), stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("not yet implemented"),
            "`{args:?}` should dispatch to the not-implemented stub, stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

// ── LFS commit-graph scanner + fetch (lfs-improvement-plan Batch 1) ──

fn run_libra_ok(repo: &Path, args: &[&str]) {
    let out = libra_command(repo)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("failed to run libra {args:?}: {e}"));
    assert!(
        out.status.success(),
        "`libra {args:?}` failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Track `*.bin`, write `content` to `name`, then add+commit it (with the
/// `.libra_attributes` file) so the commit tree carries the LFS pointer.
fn commit_lfs_file(repo: &Path, name: &str, content: &[u8]) {
    run_libra_ok(repo, &["lfs", "track", "*.bin"]);
    fs::write(repo.join(name), content).expect("write lfs file");
    run_libra_ok(repo, &["config", "user.name", "Tester"]);
    run_libra_ok(repo, &["config", "user.email", "tester@example.com"]);
    run_libra_ok(repo, &["add", ".libra_attributes", name]);
    run_libra_ok(repo, &["commit", "-m", "add lfs file"]);
}

fn lfs_entity_path(repo: &Path, oid: &str) -> std::path::PathBuf {
    repo.join(".libra/lfs/objects")
        .join(&oid[..2])
        .join(&oid[2..4])
        .join(oid)
}

/// Find the single LFS entity (64-char hex filename) under `.libra/lfs/objects`.
fn find_single_lfs_entity(repo: &Path) -> Option<std::path::PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        if let Ok(rd) = fs::read_dir(dir) {
            for entry in rd.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.len() == 64 && n.bytes().all(|b| b.is_ascii_hexdigit()))
                {
                    out.push(path);
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(&repo.join(".libra/lfs/objects"), &mut out);
    out.into_iter().next()
}

/// Spawn a mock LFS server that answers the download batch protocol and serves
/// `served` bytes for the object (use the real content for success, or tampered
/// bytes to exercise checksum failure).
async fn spawn_lfs_download_mock(served: Vec<u8>) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind mock LFS listener");
    let addr = listener
        .local_addr()
        .expect("failed to read mock LFS bound address");
    let href = format!("http://{addr}/dl");
    let app = Router::new()
        .route(
            "/objects/batch",
            post(move |body: Json<serde_json::Value>| {
                let href = href.clone();
                async move {
                    let oid = body.0["objects"][0]["oid"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    let size = body.0["objects"][0]["size"].as_i64().unwrap_or(0);
                    Json(json!({
                        "objects": [{
                            "oid": oid,
                            "size": size,
                            "actions": {
                                "download": {
                                    "href": href,
                                    "header": {},
                                    "expires_at": "2099-01-01T00:00:00Z"
                                }
                            }
                        }]
                    }))
                }
            }),
        )
        // The chunk API probe must 404 so the client falls back to a single GET.
        .route("/dl/chunks", get(|| async { StatusCode::NOT_FOUND }))
        .route(
            "/dl",
            get(move || {
                let body = served.clone();
                async move { body }
            }),
        );
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    addr
}

#[test]
fn test_lfs_fetch_noop_when_no_objects() {
    let repo = init_temp_repo();
    let output = libra_command(repo.path())
        .args(["lfs", "fetch"])
        .output()
        .expect("failed to run lfs fetch");
    assert!(
        output.status.success(),
        "fetch no-op should exit 0: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("No missing LFS objects"),
        "expected no-op notice, stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn test_lfs_fetch_json_clean_stdout() {
    let repo = init_temp_repo();
    let output = libra_command(repo.path())
        .args(["--json", "lfs", "fetch"])
        .output()
        .expect("failed to run --json lfs fetch");
    assert!(
        output.status.success(),
        "fetch --json should exit 0: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .expect("fetch stdout should be a single JSON envelope");
    assert_eq!(json["command"], "lfs");
    assert_eq!(json["data"]["action"], "fetch");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_lfs_fetch_downloads_missing_object() {
    let content = b"LFS-ENTITY-CONTENT-for-fetch-test-1234567890".to_vec();
    let addr = spawn_lfs_download_mock(content.clone()).await;
    let repo = init_repo_with_mock_remote(&format!("http://{addr}"));
    let repo_path = repo.path().to_path_buf();

    // Commit an LFS file, then delete its local entity to simulate a missing
    // object that `fetch` must restore from the remote.
    let setup_path = repo_path.clone();
    let setup_content = content.clone();
    let oid = tokio::task::spawn_blocking(move || {
        commit_lfs_file(&setup_path, "big.bin", &setup_content);
        let entity = find_single_lfs_entity(&setup_path).expect("entity should exist after add");
        let oid = entity.file_name().unwrap().to_string_lossy().into_owned();
        fs::remove_file(&entity).expect("delete entity to simulate missing object");
        oid
    })
    .await
    .expect("setup join failed");

    let fetch_path = repo_path.clone();
    let output = tokio::task::spawn_blocking(move || {
        libra_command(&fetch_path)
            .args(["lfs", "fetch", "origin"])
            .output()
            .expect("failed to run lfs fetch origin")
    })
    .await
    .expect("fetch join failed");

    assert!(
        output.status.success(),
        "fetch should restore the object: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let entity = lfs_entity_path(repo.path(), &oid);
    assert!(
        entity.exists(),
        "fetch must restore the LFS entity at {}",
        entity.display()
    );
    assert_eq!(
        fs::read(&entity).expect("read restored entity"),
        content,
        "restored entity content must match the original"
    );
    // No leftover temp file.
    assert!(
        !entity.with_file_name(format!("{oid}.tmp")).exists(),
        "fetch must not leave a .tmp file behind"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_lfs_fetch_checksum_mismatch_rolls_back() {
    let content = b"GENUINE-LFS-CONTENT-abcdefghijklmnop".to_vec();
    // The mock serves tampered bytes whose SHA256 will not match the pointer oid.
    let addr = spawn_lfs_download_mock(b"TAMPERED-BYTES-do-not-match-oid".to_vec()).await;
    let repo = init_repo_with_mock_remote(&format!("http://{addr}"));
    let repo_path = repo.path().to_path_buf();

    let setup_path = repo_path.clone();
    let setup_content = content.clone();
    let oid = tokio::task::spawn_blocking(move || {
        commit_lfs_file(&setup_path, "big.bin", &setup_content);
        let entity = find_single_lfs_entity(&setup_path).expect("entity should exist after add");
        let oid = entity.file_name().unwrap().to_string_lossy().into_owned();
        fs::remove_file(&entity).expect("delete entity");
        oid
    })
    .await
    .expect("setup join failed");

    let fetch_path = repo_path.clone();
    let output = tokio::task::spawn_blocking(move || {
        libra_command(&fetch_path)
            .args(["lfs", "fetch", "origin"])
            .output()
            .expect("failed to run lfs fetch origin")
    })
    .await
    .expect("fetch join failed");

    // A checksum mismatch must fail the fetch and never corrupt the object store.
    assert!(
        !output.status.success(),
        "fetch should fail on checksum mismatch, stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let entity = lfs_entity_path(repo.path(), &oid);
    assert!(
        !entity.exists(),
        "no corrupt entity may be stored on checksum mismatch"
    );
    assert!(
        !entity.with_file_name(format!("{oid}.tmp")).exists(),
        "the temp file must be removed on checksum mismatch"
    );
}

/// Spawn a mock LFS server that accepts the upload batch protocol: it answers
/// `objects/batch` with an upload action, returns an empty lock list from
/// `locks/verify`, and accepts the object PUT.
async fn spawn_lfs_upload_mock() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind mock LFS listener");
    let addr = listener
        .local_addr()
        .expect("failed to read mock LFS bound address");
    let upload_href = format!("http://{addr}/upload");
    let app = Router::new()
        .route(
            "/objects/batch",
            post(move |body: Json<serde_json::Value>| {
                let href = upload_href.clone();
                async move {
                    let oid = body.0["objects"][0]["oid"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    let size = body.0["objects"][0]["size"].as_i64().unwrap_or(0);
                    Json(json!({
                        "objects": [{
                            "oid": oid,
                            "size": size,
                            "actions": {
                                "upload": {
                                    "href": href,
                                    "header": {},
                                    "expires_at": "2099-01-01T00:00:00Z"
                                }
                            }
                        }]
                    }))
                }
            }),
        )
        // `verify_locks` always parses the body, so return a valid empty list.
        .route(
            "/locks/verify",
            post(|| async { Json(json!({"ours": [], "theirs": [], "next_cursor": ""})) }),
        )
        // Drain the uploaded body and accept it.
        .route(
            "/upload",
            put(|_body: axum::body::Bytes| async { StatusCode::OK }),
        );
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    addr
}

#[test]
fn test_lfs_push_noop_when_no_pointers() {
    let repo = init_temp_repo();
    run_libra_ok(repo.path(), &["config", "user.name", "Tester"]);
    run_libra_ok(repo.path(), &["config", "user.email", "tester@example.com"]);
    fs::write(repo.path().join("readme.txt"), "plain content").expect("write file");
    run_libra_ok(repo.path(), &["add", "readme.txt"]);
    run_libra_ok(repo.path(), &["commit", "-m", "non-lfs commit"]);

    // No remote is configured and none is needed: with no LFS pointers, push is
    // a pure no-op and never contacts a server.
    let output = libra_command(repo.path())
        .args(["lfs", "push"])
        .output()
        .expect("failed to run lfs push");
    assert!(
        output.status.success(),
        "push no-op should exit 0: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("No LFS objects to push"),
        "expected no-op notice, stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn test_lfs_push_rejects_non_current_branch() {
    let repo = init_temp_repo();
    run_libra_ok(repo.path(), &["config", "user.name", "Tester"]);
    run_libra_ok(repo.path(), &["config", "user.email", "tester@example.com"]);
    fs::write(repo.path().join("a.txt"), "x").expect("write file");
    run_libra_ok(repo.path(), &["add", "a.txt"]);
    run_libra_ok(repo.path(), &["commit", "-m", "commit"]);

    // The current branch is `main`; pushing a different branch is rejected.
    let output = libra_command(repo.path())
        .args(["lfs", "push", "origin", "feature"])
        .output()
        .expect("failed to run lfs push");
    assert_eq!(
        output.status.code(),
        Some(129),
        "non-current-branch push should map to LBR-CLI-003 (exit 129), stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("only the current branch"),
        "expected current-branch-only error, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_lfs_push_missing_local_object_errors() {
    let addr = spawn_lfs_upload_mock().await;
    let repo = init_repo_with_mock_remote(&format!("http://{addr}"));
    let repo_path = repo.path().to_path_buf();

    let setup_path = repo_path.clone();
    tokio::task::spawn_blocking(move || {
        commit_lfs_file(&setup_path, "big.bin", b"PUSH-CONTENT-missing-case");
        let entity = find_single_lfs_entity(&setup_path).expect("entity should exist after add");
        fs::remove_file(&entity).expect("delete entity to simulate missing object");
    })
    .await
    .expect("setup join failed");

    let push_path = repo_path.clone();
    let output = tokio::task::spawn_blocking(move || {
        libra_command(&push_path)
            .args(["lfs", "push", "origin"])
            .output()
            .expect("failed to run lfs push origin")
    })
    .await
    .expect("push join failed");

    assert!(
        !output.status.success(),
        "push must fail when a referenced object is missing locally, stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("missing from the local cache"),
        "expected missing-object error, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_lfs_push_uploads_to_mock_server() {
    let addr = spawn_lfs_upload_mock().await;
    let repo = init_repo_with_mock_remote(&format!("http://{addr}"));
    let repo_path = repo.path().to_path_buf();

    let setup_path = repo_path.clone();
    tokio::task::spawn_blocking(move || {
        commit_lfs_file(
            &setup_path,
            "big.bin",
            b"PUSH-CONTENT-upload-abcdef-1234567890",
        );
    })
    .await
    .expect("setup join failed");

    let push_path = repo_path.clone();
    let output = tokio::task::spawn_blocking(move || {
        libra_command(&push_path)
            .args(["lfs", "push", "origin"])
            .output()
            .expect("failed to run lfs push origin")
    })
    .await
    .expect("push join failed");

    assert!(
        output.status.success(),
        "push should upload to the mock server: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("Pushed 1 LFS object"),
        "expected push summary, stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_lfs_push_json_output() {
    let addr = spawn_lfs_upload_mock().await;
    let repo = init_repo_with_mock_remote(&format!("http://{addr}"));
    let repo_path = repo.path().to_path_buf();

    let setup_path = repo_path.clone();
    tokio::task::spawn_blocking(move || {
        commit_lfs_file(&setup_path, "big.bin", b"PUSH-JSON-CONTENT-0987654321");
    })
    .await
    .expect("setup join failed");

    let push_path = repo_path.clone();
    let output = tokio::task::spawn_blocking(move || {
        libra_command(&push_path)
            .args(["--json", "lfs", "push", "origin"])
            .output()
            .expect("failed to run --json lfs push origin")
    })
    .await
    .expect("push join failed");

    assert!(
        output.status.success(),
        "push --json should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .expect("push stdout should be a single JSON envelope");
    assert_eq!(json["command"], "lfs");
    assert_eq!(json["data"]["action"], "push");
    assert_eq!(
        json["data"]["pushed_oids"].as_array().map(|a| a.len()),
        Some(1),
        "pushed_oids should list the uploaded object, stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}
