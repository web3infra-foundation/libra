//! Binary-level CLI error rendering and exit code checks.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use std::{path::Path, process::Command};

use tempfile::tempdir;

use super::parse_cli_error_stderr;

fn run_libra(args: &[&str], cwd: &Path) -> std::process::Output {
    let home = cwd.join(".libra-test-home");
    let config_home = home.join(".config");
    std::fs::create_dir_all(&config_home).unwrap();

    Command::new(env!("CARGO_BIN_EXE_libra"))
        .args(args)
        .current_dir(cwd)
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("XDG_CONFIG_HOME", &config_home)
        .env_remove("RUST_LOG")
        .env_remove("LIBRA_LOG")
        .env_remove("LIBRA_ERROR_JSON")
        .output()
        .unwrap()
}

#[test]
fn unknown_command_uses_cli_exit_code_and_json_report() {
    let temp = tempdir().unwrap();
    let output = run_libra(&["wat"], temp.path());
    assert_eq!(output.status.code(), Some(129));

    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(
        stderr,
        "libra: 'wat' is not a libra command. See 'libra --help'.\nError-Code: LBR-CLI-001"
    );
    assert_eq!(report.error_code, "LBR-CLI-001");
    assert_eq!(report.category, "cli");
    assert_eq!(report.exit_code, 129);
}

#[test]
fn help_output_is_not_treated_as_an_error() {
    let temp = tempdir().unwrap();
    let output = run_libra(&["--help"], temp.path());
    assert_eq!(output.status.code(), Some(0));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.starts_with("Libra: An AI native version control system"));
    assert!(
        stdout.contains("libra help error-codes"),
        "root help should advertise the error code topic: {stdout}"
    );
    assert!(stderr.is_empty(), "unexpected stderr: {stderr}");
}

#[test]
fn help_error_codes_topic_prints_error_code_reference() {
    let temp = tempdir().unwrap();
    let output = run_libra(&["help", "error-codes"], temp.path());
    assert_eq!(output.status.code(), Some(0));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.contains("# Libra CLI Error Codes"));
    assert!(stdout.contains("LBR-CLI-001"));
    assert!(stdout.contains("LBR-REPO-001"));
    assert!(
        !stdout.contains("/Volumes/Data/GitMono/libra"),
        "help output should not leak local filesystem paths: {stdout}"
    );
    assert!(stdout.contains("## How To Change Codes"));
    assert!(stderr.is_empty(), "unexpected stderr: {stderr}");
}

#[test]
fn version_output_is_not_treated_as_an_error() {
    let temp = tempdir().unwrap();
    let output = run_libra(&["--version"], temp.path());
    assert_eq!(output.status.code(), Some(0));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let expected = format!("libra {}\n", env!("CARGO_PKG_VERSION"));
    assert_eq!(stdout, expected);
    assert!(stderr.is_empty(), "unexpected stderr: {stderr}");
}

#[test]
fn global_parse_error_uses_exit_code_2() {
    let temp = tempdir().unwrap();
    let output = run_libra(&["--bad"], temp.path());
    assert_eq!(output.status.code(), Some(129));

    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert!(stderr.starts_with("error: unexpected argument '--bad' found"));
    assert!(stderr.contains("Error-Code: LBR-CLI-002"));
    assert!(stderr.contains("Usage: libra"));
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert_eq!(report.category, "cli");
    assert_eq!(report.exit_code, 129);
}

#[test]
fn command_usage_error_uses_cli_exit_code_and_json_report() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let init = run_libra(&["init"], &repo);
    assert!(init.status.success());

    let output = run_libra(&["add", "--bad"], &repo);
    assert_eq!(output.status.code(), Some(129));

    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert!(stderr.starts_with("error: unexpected argument '--bad' found"));
    assert!(stderr.contains("Error-Code: LBR-CLI-002"));
    assert!(stderr.contains("Usage: libra add [OPTIONS] [PATHSPEC]..."));
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert_eq!(report.category, "cli");
    assert_eq!(report.exit_code, 129);
}

#[test]
fn runtime_repo_error_uses_repo_exit_code_and_json_report() {
    let temp = tempdir().unwrap();
    let output = run_libra(&["add", "good.txt"], temp.path());
    assert_eq!(output.status.code(), Some(128));

    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert!(
        stderr.contains("fatal: not a libra repository"),
        "unexpected stderr: {stderr}"
    );
    assert!(stderr.contains("Error-Code: LBR-REPO-001"));
    assert!(
        stderr.contains("Hint: run 'libra init'"),
        "missing init hint in stderr: {stderr}"
    );
    assert_eq!(report.error_code, "LBR-REPO-001");
    assert_eq!(report.category, "repo");
    assert_eq!(report.exit_code, 128);
}

#[test]
fn runtime_repo_error_in_git_repo_suggests_conversion() {
    let temp = tempdir().unwrap();
    let git = temp.path().join(".git");
    std::fs::create_dir_all(git.join("objects")).unwrap();
    std::fs::write(git.join("HEAD"), b"ref: refs/heads/main\n").unwrap();
    std::fs::write(
        git.join("config"),
        b"[core]\n\trepositoryformatversion = 0\n",
    )
    .unwrap();

    let output = run_libra(&["status"], temp.path());
    assert_eq!(output.status.code(), Some(128));

    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-REPO-001");
    assert_eq!(
        report.hints.first().map(String::as_str),
        Some("run 'libra init --from-git-repository .' to convert this Git repository to Libra.")
    );
    assert!(
        stderr.contains(
            "\n\nHint: run 'libra init --from-git-repository .' to convert this Git repository to Libra."
        ),
        "expected blank line before conversion hint, got: {stderr}"
    );
}
