//! Binary-level `libra add` behavior checks.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use std::{fs, path::Path, process::Command};

use tempfile::tempdir;

fn run_libra(args: &[&str], cwd: &Path) -> std::process::Output {
    let home = cwd.join(".libra-test-home");
    let config_home = home.join(".config");
    fs::create_dir_all(&config_home).unwrap();

    Command::new(env!("CARGO_BIN_EXE_libra"))
        .args(args)
        .current_dir(cwd)
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("XDG_CONFIG_HOME", &config_home)
        .env_remove("RUST_LOG")
        .env_remove("LIBRA_LOG")
        .output()
        .unwrap()
}

fn init_repo(repo: &Path) {
    fs::create_dir_all(repo).unwrap();
    let output = run_libra(&["init"], repo);
    assert!(output.status.success(), "{:?}", output);
}

#[test]
fn missing_pathspec_is_fatal_and_atomic() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    fs::write(repo.join("good.txt"), "good").unwrap();

    let output = run_libra(&["add", "good.txt", "missing.txt"], &repo);
    assert_eq!(output.status.code(), Some(129));

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("fatal: pathspec 'missing.txt' did not match any files"));
    assert!(stderr.contains("Error-Code: LBR-CLI-003"));

    let status = run_libra(&["status", "--short"], &repo);
    let stdout = String::from_utf8_lossy(&status.stdout);
    assert!(!stdout.contains("A  good.txt"), "status was: {stdout}");
}

#[test]
fn partial_ignore_stages_good_files_and_warns() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    fs::write(repo.join(".libraignore"), "ignored.txt\n").unwrap();
    fs::write(repo.join("good.txt"), "good").unwrap();
    fs::write(repo.join("ignored.txt"), "ignored").unwrap();

    let output = run_libra(&["add", "good.txt", "ignored.txt"], &repo);
    // Partial ignore: good.txt staged successfully → exit 0 (warning on stderr)
    assert_eq!(output.status.code(), Some(0));

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("ignored.txt"),
        "stderr should warn about ignored file: {stderr}"
    );

    let status = run_libra(&["status", "--short"], &repo);
    let stdout = String::from_utf8_lossy(&status.stdout);
    assert!(stdout.contains("A  good.txt"), "status was: {stdout}");
}

#[test]
fn ignored_only_path_returns_conflict_exit_code() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    fs::write(repo.join(".libraignore"), "ignored.txt\n").unwrap();
    fs::write(repo.join("visible.txt"), "visible").unwrap();
    fs::write(repo.join("ignored.txt"), "ignored").unwrap();

    let output = run_libra(&["add", "ignored.txt"], &repo);
    assert_eq!(output.status.code(), Some(128));

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("ignored.txt"));

    let status = run_libra(&["status", "--short"], &repo);
    let stdout = String::from_utf8_lossy(&status.stdout);
    assert!(
        !stdout.contains("A  visible.txt"),
        "ignored-only add should not stage unrelated files: {stdout}"
    );
    assert!(
        !stdout.contains("A  ignored.txt"),
        "ignored-only add should not stage the ignored file: {stdout}"
    );
}

#[test]
fn corrupt_index_reports_fatal_without_panic() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    fs::write(repo.join("good.txt"), "good").unwrap();
    fs::write(repo.join(".libra").join("index"), b"garb").unwrap();

    let output = run_libra(&["add", "good.txt"], &repo);
    assert_eq!(output.status.code(), Some(128));

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("fatal: unable to read index"));
    assert!(stderr.contains("Error-Code: LBR-REPO-002"));
    assert!(!stderr.contains("thread 'main' panicked"));
    assert!(!stderr.contains("stack backtrace"));
}
