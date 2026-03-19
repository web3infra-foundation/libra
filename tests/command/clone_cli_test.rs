//! Binary-level `libra clone` behavior checks.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use std::{fs, path::Path, process::Command};

use tempfile::tempdir;

use super::parse_cli_error_stderr;

fn run_libra(args: &[&str], cwd: &Path) -> std::process::Output {
    let home = cwd.join(".home");
    fs::create_dir_all(&home).unwrap();

    Command::new(env!("CARGO_BIN_EXE_libra"))
        .args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .env_remove("RUST_LOG")
        .env_remove("LIBRA_LOG")
        .output()
        .unwrap()
}

fn run_libra_with_home(args: &[&str], cwd: &Path, home: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_libra"))
        .args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .env_remove("RUST_LOG")
        .env_remove("LIBRA_LOG")
        .output()
        .unwrap()
}

fn run_git(args: &[&str], cwd: &Path) -> std::process::Output {
    Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap()
}

fn create_remote_with_main(base: &Path) -> std::path::PathBuf {
    let remote = base.join("remote.git");
    assert!(
        run_git(&["init", "--bare", remote.to_str().unwrap()], base)
            .status
            .success()
    );

    let work = base.join("work");
    fs::create_dir_all(&work).unwrap();
    assert!(run_git(&["init"], &work).status.success());
    assert!(
        run_git(&["config", "user.name", "T"], &work)
            .status
            .success()
    );
    assert!(
        run_git(&["config", "user.email", "t@example.com"], &work)
            .status
            .success()
    );
    fs::write(work.join("README.md"), "hello\n").unwrap();
    assert!(run_git(&["add", "README.md"], &work).status.success());
    assert!(
        run_git(&["commit", "-m", "initial"], &work)
            .status
            .success()
    );
    assert!(run_git(&["branch", "-M", "main"], &work).status.success());
    assert!(
        run_git(
            &["remote", "add", "origin", remote.to_str().unwrap()],
            &work
        )
        .status
        .success()
    );
    assert!(run_git(&["push", "origin", "main"], &work).status.success());
    // Ensure bare repo HEAD points to main regardless of init.defaultBranch config.
    assert!(
        run_git(&["symbolic-ref", "HEAD", "refs/heads/main"], &remote)
            .status
            .success()
    );
    remote
}

#[test]
fn invalid_source_does_not_panic() {
    let temp = tempdir().unwrap();
    let output = run_libra(&["clone", "/"], temp.path());
    assert_eq!(output.status.code(), Some(128));

    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(
        stderr,
        "fatal: '/' does not appear to be a libra repository\nError-Code: LBR-REPO-001"
    );
    assert_eq!(report.error_code, "LBR-REPO-001");
    assert_eq!(report.exit_code, 128);
    assert!(!stderr.contains("thread 'main' panicked"));
}

#[test]
fn missing_branch_keeps_preexisting_empty_destination() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let existing = temp.path().join("existing");
    fs::create_dir_all(&existing).unwrap();

    let output = run_libra(
        &[
            "clone",
            "-b",
            "nope",
            remote.to_str().unwrap(),
            existing.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(output.status.code(), Some(128));

    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert!(stderr.contains("fatal: remote branch nope not found in upstream origin"));
    assert!(stderr.contains("Error-Code: LBR-REPO-003"));
    assert_eq!(report.error_code, "LBR-REPO-003");
    assert!(existing.is_dir());
    assert_eq!(fs::read_dir(&existing).unwrap().count(), 0);
}

#[test]
fn successful_clone_output_has_no_debug_noise() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let dest = temp.path().join("clone");

    let output = run_libra(
        &["clone", remote.to_str().unwrap(), dest.to_str().unwrap()],
        temp.path(),
    );
    assert_eq!(output.status.code(), Some(0));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.is_empty(), "unexpected stdout: {stdout}");
    assert!(stderr.contains("Cloning into 'clone'..."));
    assert!(!stderr.contains(" INFO "));
    assert!(!stderr.contains(" WARN "));
    assert!(!stderr.contains("fatal: fatal:"));
    assert!(!stderr.contains('\u{2}'));
    assert!(dest.join("README.md").exists());
}

#[test]
fn successful_clone_initializes_vault() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let dest = temp.path().join("clone");
    let home = temp.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let output = run_libra_with_home(
        &["clone", remote.to_str().unwrap(), dest.to_str().unwrap()],
        temp.path(),
        &home,
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "clone failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        dest.join(".libra").join("vault.db").exists(),
        "clone should initialize .libra/vault.db for vault-backed workflows"
    );

    let signing_output = run_libra_with_home(&["config", "--get", "vault.signing"], &dest, &home);
    assert_eq!(
        signing_output.status.code(),
        Some(0),
        "failed to read vault.signing: {}",
        String::from_utf8_lossy(&signing_output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&signing_output.stdout).trim(),
        "true",
        "clone should enable vault.signing"
    );
}

#[test]
fn machine_clone_suppresses_decorative_stderr() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let dest = temp.path().join("clone-machine");

    let output = run_libra(
        &[
            "--machine",
            "clone",
            remote.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "machine clone failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.trim().is_empty(), "unexpected stdout: {stdout}");
    assert!(
        stderr.trim().is_empty(),
        "machine clone should suppress decorative stderr, got: {stderr}"
    );
    assert!(dest.join("README.md").exists());
}
