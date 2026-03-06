//! Binary-level `libra clone` behavior checks.

use std::{fs, path::Path, process::Command};

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
    remote
}

#[test]
fn invalid_source_does_not_panic() {
    let temp = tempdir().unwrap();
    let output = run_libra(&["clone", "/"], temp.path());
    assert_eq!(output.status.code(), Some(128));

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(stderr, "fatal: repository '/' does not exist\n");
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

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("fatal: remote branch nope not found in upstream origin"));
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
