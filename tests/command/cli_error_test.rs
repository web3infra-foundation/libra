//! Binary-level CLI error rendering and exit code checks.

use std::{path::Path, process::Command};

use tempfile::tempdir;

fn run_libra(args: &[&str], cwd: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_libra"))
        .args(args)
        .current_dir(cwd)
        .env_remove("RUST_LOG")
        .env_remove("LIBRA_LOG")
        .output()
        .unwrap()
}

#[test]
fn unknown_command_uses_git_style_exit_code() {
    let temp = tempdir().unwrap();
    let output = run_libra(&["wat"], temp.path());
    assert_eq!(output.status.code(), Some(1));

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        stderr,
        "libra: 'wat' is not a libra command. See 'libra --help'.\n"
    );
}

#[test]
fn help_output_is_not_treated_as_an_error() {
    let temp = tempdir().unwrap();
    let output = run_libra(&["--help"], temp.path());
    assert_eq!(output.status.code(), Some(0));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.starts_with("Libra: An AI native version control system"));
    assert!(stderr.is_empty(), "unexpected stderr: {stderr}");
}

#[test]
fn version_output_is_not_treated_as_an_error() {
    let temp = tempdir().unwrap();
    let output = run_libra(&["--version"], temp.path());
    assert_eq!(output.status.code(), Some(0));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(stdout, "libra 0.1.0\n");
    assert!(stderr.is_empty(), "unexpected stderr: {stderr}");
}

#[test]
fn global_parse_error_uses_exit_code_2() {
    let temp = tempdir().unwrap();
    let output = run_libra(&["--bad"], temp.path());
    assert_eq!(output.status.code(), Some(2));

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.starts_with("error: unexpected argument '--bad' found"));
    assert!(stderr.contains("Usage: libra <COMMAND>"));
}

#[test]
fn command_usage_error_uses_exit_code_129() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let init = run_libra(&["init"], &repo);
    assert!(init.status.success());

    let output = run_libra(&["add", "--bad"], &repo);
    assert_eq!(output.status.code(), Some(129));

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.starts_with("error: unexpected argument '--bad' found"));
    assert!(stderr.contains("Usage: libra add [OPTIONS] [PATHSPEC]..."));
}

#[test]
fn runtime_fatal_uses_exit_code_128() {
    let temp = tempdir().unwrap();
    let output = run_libra(&["add", "good.txt"], temp.path());
    assert_eq!(output.status.code(), Some(128));

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("fatal: not a libra repository"),
        "unexpected stderr: {stderr}"
    );
    assert!(
        stderr.contains("Hint: run 'libra init'"),
        "missing init hint in stderr: {stderr}"
    );
}
