//! Tests open command integration to ensure it finds remote correctly.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use libra::{
    command::{
        open,
        remote::{self, RemoteCmds},
    },
    utils::test,
};
use serial_test::serial;
use tempfile::tempdir;

use super::*;

#[tokio::test]
#[serial]
async fn test_open_remote_origin() {
    let repo_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(repo_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(repo_dir.path());

    // Add origin remote
    remote::execute(RemoteCmds::Add {
        name: "origin".into(),
        url: "git@github.com:web3infra-foundation/libra.git".into(),
    })
    .await;

    // Test explicit remote
    open::execute(open::OpenArgs {
        remote: Some("origin".to_string()),
    })
    .await;

    // Test default remote should find origin
    open::execute(open::OpenArgs { remote: None }).await;

    // Test non-existent remote
    open::execute(open::OpenArgs {
        remote: Some("nonexistent".to_string()),
    })
    .await;
}

#[tokio::test]
#[serial]
async fn test_open_no_remote() {
    let repo_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(repo_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(repo_dir.path());

    // Should handle no remote configured
    open::execute(open::OpenArgs { remote: None }).await;
}

#[test]
fn test_open_json_output_uses_origin_remote() {
    let repo = create_committed_repo_via_cli();

    let add_remote = run_libra_command(
        &[
            "remote",
            "add",
            "origin",
            "git@github.com:web3infra-foundation/libra.git",
        ],
        repo.path(),
    );
    assert_cli_success(&add_remote, "failed to add origin for open test");

    let output = run_libra_command(&["open", "--json"], repo.path());

    assert_cli_success(&output, "open --json should succeed");
    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "open");
    assert_eq!(json["data"]["remote"], "origin");
    assert_eq!(
        json["data"]["web_url"],
        "https://github.com/web3infra-foundation/libra"
    );
}

#[test]
fn test_open_without_remote_reports_stable_error() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());

    let output = run_libra_command(&["open"], repo.path());

    let (_stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-REPO-003");
    assert!(
        report
            .hints
            .iter()
            .any(|hint| hint.contains("libra remote add origin")),
        "expected hint to mention adding a remote, got {:?}",
        report.hints
    );
}

#[test]
fn test_open_json_output_transforms_explicit_ssh_url() {
    let temp = tempdir().unwrap();

    let output = run_libra_command(
        &[
            "open",
            "--json",
            "ssh://git@github.com/web3infra-foundation/libra.git",
        ],
        temp.path(),
    );

    assert_cli_success(&output, "open --json with explicit ssh URL should succeed");
    let json = parse_json_stdout(&output);
    assert!(json["data"]["remote"].is_null());
    assert_eq!(
        json["data"]["remote_url"],
        "ssh://git@github.com/web3infra-foundation/libra.git"
    );
    assert_eq!(
        json["data"]["web_url"],
        "https://github.com/web3infra-foundation/libra"
    );
}

#[test]
fn test_open_json_output_keeps_explicit_https_url() {
    let temp = tempdir().unwrap();

    let output = run_libra_command(
        &[
            "open",
            "--json",
            "https://github.com/web3infra-foundation/libra.git",
        ],
        temp.path(),
    );

    assert_cli_success(
        &output,
        "open --json with explicit https URL should succeed",
    );
    let json = parse_json_stdout(&output);
    assert!(json["data"]["remote"].is_null());
    assert_eq!(
        json["data"]["remote_url"],
        "https://github.com/web3infra-foundation/libra.git"
    );
    assert_eq!(
        json["data"]["web_url"],
        "https://github.com/web3infra-foundation/libra"
    );
}
