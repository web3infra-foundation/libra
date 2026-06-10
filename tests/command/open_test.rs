//! Tests open command integration to ensure it finds remote correctly.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use libra::{
    command::{
        open,
        remote::{self, RemoteCmds},
    },
    utils::{error::StableErrorCode, output::OutputConfig, test},
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
    let output = OutputConfig {
        quiet: true,
        ..OutputConfig::default()
    };

    // Add origin remote
    remote::execute_safe(
        RemoteCmds::Add {
            name: "origin".into(),
            url: "git@github.com:web3infra-foundation/libra.git".into(),
        },
        &output,
    )
    .await
    .expect("adding origin remote should succeed");

    // Test explicit remote
    open::execute_safe(
        open::OpenArgs {
            remote: Some("origin".to_string()),
            print_only: false,
            ..Default::default()
        },
        &output,
    )
    .await
    .expect("opening explicit origin remote should succeed");

    // Test default remote should find origin
    open::execute_safe(
        open::OpenArgs {
            remote: None,
            print_only: false,
            ..Default::default()
        },
        &output,
    )
    .await
    .expect("opening default remote should succeed");

    let error = open::execute_safe(
        open::OpenArgs {
            remote: Some("nonexistent".to_string()),
            print_only: false,
            ..Default::default()
        },
        &output,
    )
    .await
    .expect_err("invalid direct remote target should return a CLI error");
    assert_eq!(error.stable_code(), StableErrorCode::CliInvalidTarget);
    assert_eq!(error.exit_code(), 129);
    assert!(
        error.message().contains("unsafe or invalid"),
        "unexpected error message: {}",
        error.message()
    );
}

#[tokio::test]
#[serial]
async fn test_open_no_remote() {
    let repo_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(repo_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(repo_dir.path());
    let output = OutputConfig {
        quiet: true,
        ..OutputConfig::default()
    };

    let error = open::execute_safe(
        open::OpenArgs {
            remote: None,
            print_only: false,
            ..Default::default()
        },
        &output,
    )
    .await
    .expect_err("opening without a configured remote should fail");
    assert_eq!(error.stable_code(), StableErrorCode::RepoStateInvalid);
    assert_eq!(error.exit_code(), 128);
    assert!(
        error.message().contains("no remote configured"),
        "unexpected error message: {}",
        error.message()
    );
    assert!(
        error
            .hints()
            .iter()
            .any(|hint| hint.as_str().contains("libra remote add origin")),
        "expected add-remote hint, got {:?}",
        error.hints()
    );
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
    assert_eq!(json["data"]["launched"], false);
}

#[cfg(not(windows))]
#[test]
fn test_open_json_output_does_not_require_browser_launcher() {
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
    assert_cli_success(
        &add_remote,
        "failed to add origin for browser-launch bypass test",
    );

    let output = base_libra_command(&["open", "--json"], repo.path())
        .env_remove(LIBRA_TEST_ENV)
        .env("PATH", repo.path())
        .output()
        .expect("failed to execute open --json without browser launcher");

    assert_cli_success(
        &output,
        "open --json should not require a browser launcher in automation",
    );
    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["remote"], "origin");
    assert_eq!(json["data"]["launched"], false);
}

#[test]
fn test_open_json_output_falls_back_to_origin_when_head_is_detached() {
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
    assert_cli_success(
        &add_remote,
        "failed to add origin for detached-head open test",
    );

    let log_out = run_libra_command(&["log"], repo.path());
    let stdout = String::from_utf8_lossy(&log_out.stdout);
    let hash = stdout
        .lines()
        .find(|line| line.starts_with("commit "))
        .and_then(|line| line.strip_prefix("commit "))
        .map(str::trim)
        .expect("expected commit hash in log output");

    let switch_out = run_libra_command(&["switch", "--detach", hash], repo.path());
    assert_cli_success(
        &switch_out,
        "failed to detach HEAD before running open --json",
    );

    let output = run_libra_command(&["open", "--json"], repo.path());
    assert_cli_success(
        &output,
        "open --json should fall back to origin on detached HEAD",
    );

    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["remote"], "origin");
    assert_eq!(
        json["data"]["web_url"],
        "https://github.com/web3infra-foundation/libra"
    );
    assert_eq!(json["data"]["launched"], false);
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
    assert_eq!(json["data"]["launched"], false);
}

// ── Deep-link target flags (Batch 0) ─────────────────────────────────────

/// Adds an `origin` remote pointing at the libra GitHub repo and returns the
/// committed repo dir.
fn repo_with_github_origin() -> tempfile::TempDir {
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
    assert_cli_success(&add_remote, "failed to add origin for open deep-link test");
    repo
}

#[test]
fn test_open_with_branch_flag() {
    let repo = repo_with_github_origin();

    let output = run_libra_command(&["open", "--json", "-b", "dev", "origin"], repo.path());
    assert_cli_success(&output, "open --json -b dev origin should succeed");

    let json = parse_json_stdout(&output);
    assert_eq!(
        json["data"]["web_url"],
        "https://github.com/web3infra-foundation/libra/tree/dev"
    );
    assert_eq!(json["data"]["target_type"], "branch");
    assert_eq!(json["data"]["platform"], "github");
    assert_eq!(json["data"]["launched"], false);
}

#[test]
fn test_open_default_no_flag_opens_repo_root() {
    let repo = repo_with_github_origin();

    let output = run_libra_command(&["open", "--json", "origin"], repo.path());
    assert_cli_success(&output, "open --json origin should succeed");

    let json = parse_json_stdout(&output);
    assert_eq!(
        json["data"]["web_url"],
        "https://github.com/web3infra-foundation/libra"
    );
    assert_eq!(json["data"]["target_type"], "repo");
    assert_eq!(json["data"]["platform"], "github");
}

#[test]
fn test_open_issue_id_with_equals() {
    let repo = repo_with_github_origin();

    // `--issue=12` keeps `origin` as the positional remote.
    let output = run_libra_command(&["open", "--json", "--issue=12", "origin"], repo.path());
    assert_cli_success(&output, "open --issue=12 origin should succeed");

    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["remote"], "origin");
    assert_eq!(json["data"]["target_type"], "issue");
    assert_eq!(
        json["data"]["web_url"],
        "https://github.com/web3infra-foundation/libra/issues/12"
    );
}

#[test]
fn test_open_issue_list_no_id() {
    let repo = repo_with_github_origin();

    // `--issue` with no `=value` opens the list; `origin` stays the remote.
    let output = run_libra_command(&["open", "--json", "--issue", "origin"], repo.path());
    assert_cli_success(&output, "open --issue origin should succeed");

    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["remote"], "origin");
    assert_eq!(json["data"]["target_type"], "issue");
    assert_eq!(
        json["data"]["web_url"],
        "https://github.com/web3infra-foundation/libra/issues"
    );
}

#[test]
fn test_open_mutually_exclusive_flags_error() {
    let repo = repo_with_github_origin();

    let output = run_libra_command(
        &["open", "-b", "main", "-c", "abcdef", "origin"],
        repo.path(),
    );
    assert_eq!(output.status.code(), Some(129));
    let (_stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-002");
}

#[test]
fn test_open_rejects_malicious_branch() {
    let repo = repo_with_github_origin();

    let output = run_libra_command(&["open", "-b", "main; rm -rf /", "origin"], repo.path());
    assert_eq!(output.status.code(), Some(129));
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert!(
        stderr.contains("unsafe or invalid"),
        "unexpected stderr: {stderr}"
    );
}

// ── Platform adaptation & config templates (Batch 1) ─────────────────────

#[test]
fn test_open_platform_override() {
    let repo = repo_with_github_origin();

    // Force GitLab style even though the host is github.com.
    let set = run_libra_command(&["config", "open.platform", "gitlab"], repo.path());
    assert_cli_success(&set, "config open.platform gitlab should succeed");

    let output = run_libra_command(&["open", "--json", "-c", "a1b2c3d", "origin"], repo.path());
    assert_cli_success(&output, "open commit with gitlab override should succeed");

    let json = parse_json_stdout(&output);
    assert_eq!(
        json["data"]["web_url"],
        "https://github.com/web3infra-foundation/libra/-/commit/a1b2c3d"
    );
    assert_eq!(json["data"]["platform"], "gitlab");
}

#[test]
fn test_open_local_platform_config_takes_effect() {
    let repo = repo_with_github_origin();

    let set = run_libra_command(&["config", "open.platform", "gitlab"], repo.path());
    assert_cli_success(&set, "config open.platform gitlab should succeed");

    let output = run_libra_command(&["open", "--json", "-b", "main", "origin"], repo.path());
    assert_cli_success(
        &output,
        "open branch with local gitlab config should succeed",
    );

    let json = parse_json_stdout(&output);
    assert_eq!(
        json["data"]["web_url"],
        "https://github.com/web3infra-foundation/libra/-/tree/main"
    );
    assert_eq!(json["data"]["platform"], "gitlab");
}

#[test]
fn test_open_json_reports_platform() {
    let repo = repo_with_github_origin();

    let output = run_libra_command(&["open", "--json", "origin"], repo.path());
    assert_cli_success(&output, "open --json should succeed");

    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["platform"], "github");
}

#[test]
fn test_open_invalid_platform_warns_and_falls_back() {
    let repo = repo_with_github_origin();

    let set = run_libra_command(&["config", "open.platform", "nonsense"], repo.path());
    assert_cli_success(&set, "config open.platform nonsense should succeed");

    let output = run_libra_command(&["open", "--json", "-c", "a1b2c3d", "origin"], repo.path());
    assert_cli_success(&output, "invalid platform should fall back, not crash");

    let json = parse_json_stdout(&output);
    // Falls back to host detection (github) — commit path is GitHub style.
    assert_eq!(
        json["data"]["web_url"],
        "https://github.com/web3infra-foundation/libra/commit/a1b2c3d"
    );
    assert_eq!(json["data"]["platform"], "github");
}

#[test]
fn test_open_outside_repo_direct_url_with_branch() {
    // Non-repository directory: in_repo == false, so no local config is read.
    let temp = tempdir().unwrap();

    let output = run_libra_command(
        &["open", "--json", "-b", "dev", "https://github.com/foo/bar"],
        temp.path(),
    );
    assert_cli_success(
        &output,
        "open with direct URL outside a repo should succeed",
    );

    let json = parse_json_stdout(&output);
    assert!(json["data"]["remote"].is_null());
    assert_eq!(
        json["data"]["web_url"],
        "https://github.com/foo/bar/tree/dev"
    );
    assert_eq!(json["data"]["target_type"], "branch");
    assert_eq!(json["data"]["platform"], "github");
    assert_eq!(json["data"]["launched"], false);
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
    assert_eq!(json["data"]["launched"], false);
}

#[test]
fn test_open_print_only_prints_url_without_opening_browser() {
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
    assert_cli_success(&add_remote, "failed to add origin for print-only test");

    let output = run_libra_command(&["open", "--print-only"], repo.path());
    assert_cli_success(&output, "open --print-only should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();

    assert_eq!(stdout, "https://github.com/web3infra-foundation/libra");
}

#[test]
fn test_open_print_only_with_json_includes_resolved_from_remote() {
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
    assert_cli_success(&add_remote, "failed to add origin for print-only json test");

    let output = run_libra_command(&["open", "--print-only", "--json"], repo.path());
    assert_cli_success(&output, "open --print-only --json should succeed");
    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["resolved_from_remote"], true);
    assert_eq!(json["data"]["launched"], false);
    assert_eq!(
        json["data"]["web_url"],
        "https://github.com/web3infra-foundation/libra"
    );
}

#[test]
fn test_open_print_only_with_explicit_url() {
    let temp = tempdir().unwrap();

    let output = run_libra_command(
        &[
            "open",
            "--print-only",
            "ssh://git@github.com/web3infra-foundation/libra.git",
        ],
        temp.path(),
    );
    assert_cli_success(
        &output,
        "open --print-only with explicit URL should succeed",
    );
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();

    assert_eq!(stdout, "https://github.com/web3infra-foundation/libra");
}
