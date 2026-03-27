//! CLI-level error code, exit code and human output verification for `libra commit`.
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
    assert!(output.status.success(), "init failed: {:?}", output);
}

fn configure_identity(repo: &Path) {
    let o1 = run_libra(&["config", "user.name", "Test User"], repo);
    assert!(o1.status.success());
    let o2 = run_libra(&["config", "user.email", "test@example.com"], repo);
    assert!(o2.status.success());
}

fn make_initial_commit(repo: &Path) {
    fs::write(repo.join("init.txt"), "init\n").unwrap();
    let add = run_libra(&["add", "init.txt"], repo);
    assert!(add.status.success(), "add failed");
    let commit = run_libra(&["commit", "-m", "initial", "--no-verify"], repo);
    assert!(commit.status.success(), "commit failed");
}

// ---------------------------------------------------------------------------
// Exit code classification
// ---------------------------------------------------------------------------

#[test]
fn nothing_to_commit_returns_exit_128() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    configure_identity(&repo);
    make_initial_commit(&repo);

    // No changes staged → nothing to commit
    let output = run_libra(&["commit", "-m", "empty", "--no-verify"], &repo);
    assert_eq!(output.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("nothing to commit"),
        "stderr should mention nothing to commit: {stderr}"
    );
    assert!(stderr.contains("LBR-REPO-003"));
}

#[test]
fn nothing_to_commit_no_tracked_returns_exit_128() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    configure_identity(&repo);

    // Empty index, no files ever staged
    let output = run_libra(&["commit", "-m", "empty", "--no-verify"], &repo);
    assert_eq!(output.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("nothing to commit"),
        "stderr should mention nothing to commit: {stderr}"
    );
    assert!(stderr.contains("LBR-REPO-003"));
}

#[test]
fn missing_identity_returns_auth_exit_code() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    // No identity configured
    fs::write(repo.join("f.txt"), "hello").unwrap();
    let add = run_libra(&["add", "f.txt"], &repo);
    assert!(add.status.success());

    let output = run_libra(&["commit", "-m", "test", "--no-verify"], &repo);
    assert_eq!(output.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("LBR-AUTH-001"));
    assert!(stderr.contains("author identity unknown"));
}

#[test]
fn invalid_author_format_returns_exit_129() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    configure_identity(&repo);
    fs::write(repo.join("f.txt"), "hello").unwrap();
    let add = run_libra(&["add", "f.txt"], &repo);
    assert!(add.status.success());

    let output = run_libra(
        &[
            "commit",
            "-m",
            "test",
            "--author",
            "bad-format",
            "--no-verify",
        ],
        &repo,
    );
    assert_eq!(output.status.code(), Some(129));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("LBR-CLI-002"));
}

#[test]
fn conventional_validation_failure_returns_exit_129() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    configure_identity(&repo);
    fs::write(repo.join("f.txt"), "hello").unwrap();
    let add = run_libra(&["add", "f.txt"], &repo);
    assert!(add.status.success());

    let output = run_libra(
        &["commit", "-m", "not conventional at all", "--conventional"],
        &repo,
    );
    assert_eq!(output.status.code(), Some(129));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("LBR-CLI-002"));
    assert!(stderr.contains("conventional"));
}

#[test]
fn message_from_file_works() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    configure_identity(&repo);
    fs::write(repo.join("f.txt"), "hello").unwrap();
    let add = run_libra(&["add", "f.txt"], &repo);
    assert!(add.status.success());

    fs::write(repo.join("msg.txt"), "feat: from file").unwrap();
    let output = run_libra(&["commit", "-F", "msg.txt", "--no-verify"], &repo);
    assert!(
        output.status.success(),
        "commit -F should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("from file"), "stdout: {stdout}");
}

#[test]
fn message_from_missing_file_returns_exit_128() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    configure_identity(&repo);
    fs::write(repo.join("f.txt"), "hello").unwrap();
    let add = run_libra(&["add", "f.txt"], &repo);
    assert!(add.status.success());

    let output = run_libra(&["commit", "-F", "nonexistent.txt", "--no-verify"], &repo);
    assert_eq!(output.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("LBR-IO-001"));
}

// ---------------------------------------------------------------------------
// Human output format
// ---------------------------------------------------------------------------

#[test]
fn human_output_shows_branch_and_subject() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    configure_identity(&repo);
    fs::write(repo.join("f.txt"), "hello").unwrap();
    let add = run_libra(&["add", "f.txt"], &repo);
    assert!(add.status.success());

    let output = run_libra(&["commit", "-m", "feat: add feature", "--no-verify"], &repo);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Should match pattern: [branch short_id] subject
    assert!(
        stdout.contains("feat: add feature"),
        "stdout should contain subject: {stdout}"
    );
    assert!(
        stdout.contains("main") || stdout.contains("master"),
        "stdout should contain branch name: {stdout}"
    );
}

#[test]
fn root_commit_shows_root_marker() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    configure_identity(&repo);
    fs::write(repo.join("f.txt"), "hello").unwrap();
    let add = run_libra(&["add", "f.txt"], &repo);
    assert!(add.status.success());

    let output = run_libra(&["commit", "-m", "initial", "--no-verify"], &repo);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("(root-commit)"),
        "first commit should show (root-commit): {stdout}"
    );
}

#[test]
fn quiet_commit_suppresses_stdout() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    configure_identity(&repo);
    fs::write(repo.join("f.txt"), "hello").unwrap();
    let add = run_libra(&["add", "f.txt"], &repo);
    assert!(add.status.success());

    let output = run_libra(
        &["--quiet", "commit", "-m", "initial", "--no-verify"],
        &repo,
    );
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.trim().is_empty(),
        "quiet should suppress stdout: {stdout}"
    );
}

#[test]
fn amend_without_prior_commit_returns_repo_state_error() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    configure_identity(&repo);
    fs::write(repo.join("f.txt"), "hello").unwrap();
    let add = run_libra(&["add", "f.txt"], &repo);
    assert!(add.status.success());

    let output = run_libra(&["commit", "--amend", "--no-edit", "--no-verify"], &repo);
    assert_eq!(output.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("LBR-REPO-003"), "stderr: {stderr}");
}
