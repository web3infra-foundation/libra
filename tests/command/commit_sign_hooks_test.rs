//! GPG/Vault signing decisions and message-hook lifecycle for `libra commit`
//! (Batch 1).
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use std::{fs, path::Path, process::Command};

use serde_json::Value;
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
        .env_remove("EDITOR")
        .env_remove("VISUAL")
        .env_remove("GIT_EDITOR")
        .output()
        .unwrap()
}

fn init_repo(repo: &Path) {
    fs::create_dir_all(repo).unwrap();
    assert!(run_libra(&["init"], repo).status.success(), "init failed");
    assert!(
        run_libra(&["config", "user.name", "Test User"], repo)
            .status
            .success()
    );
    assert!(
        run_libra(&["config", "user.email", "test@example.com"], repo)
            .status
            .success()
    );
}

fn stage(repo: &Path, name: &str, content: &str) {
    fs::write(repo.join(name), content).unwrap();
    assert!(run_libra(&["add", name], repo).status.success(), "add");
}

#[cfg(unix)]
fn write_hook(repo: &Path, name: &str, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    let dir = repo.join(".libra").join("hooks");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.sh"));
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
}

fn signed_field(repo: &Path, args: &[&str]) -> bool {
    let mut full = vec!["--json", "commit"];
    full.extend_from_slice(args);
    let out = run_libra(&full, repo);
    assert_eq!(
        out.status.code(),
        Some(0),
        "commit failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap();
    v["data"]["signed"].as_bool().unwrap()
}

fn head_oid(repo: &Path) -> String {
    let out = run_libra(&["rev-parse", "HEAD"], repo);
    assert_eq!(
        out.status.code(),
        Some(0),
        "rev-parse HEAD failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

// ---------------------------------------------------------------------------
// Message hooks
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn commit_msg_hook_failure_aborts_128() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    stage(&repo, "a.txt", "x\n");
    write_hook(&repo, "commit-msg", "exit 1");

    let out = run_libra(&["commit", "-m", "x"], &repo);
    assert_eq!(
        out.status.code(),
        Some(128),
        "commit-msg failure must abort with 128: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[cfg(unix)]
#[test]
fn no_verify_skips_commit_msg() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    stage(&repo, "a.txt", "x\n");
    write_hook(&repo, "commit-msg", "exit 1");

    let out = run_libra(&["commit", "--no-verify", "-m", "x"], &repo);
    assert_eq!(
        out.status.code(),
        Some(0),
        "--no-verify must skip the failing commit-msg hook: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[cfg(unix)]
#[test]
fn disable_pre_does_not_skip_commit_msg() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    stage(&repo, "a.txt", "x\n");
    write_hook(&repo, "commit-msg", "exit 1");

    // --disable-pre skips only pre-commit; commit-msg still runs and fails.
    let out = run_libra(&["commit", "--disable-pre", "-m", "x"], &repo);
    assert_eq!(
        out.status.code(),
        Some(128),
        "--disable-pre must NOT skip commit-msg: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[cfg(unix)]
#[test]
fn prepare_commit_msg_hook_modifies_message() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    stage(&repo, "a.txt", "x\n");
    write_hook(
        &repo,
        "prepare-commit-msg",
        "printf 'PREFIX: ' | cat - \"$1\" > \"$1.tmp\" && mv \"$1.tmp\" \"$1\"",
    );

    assert_eq!(
        run_libra(&["commit", "-m", "body"], &repo).status.code(),
        Some(0)
    );
    let log = String::from_utf8_lossy(&run_libra(&["log", "-1"], &repo).stdout).into_owned();
    assert!(
        log.contains("PREFIX: body"),
        "prepare-commit-msg edit should be used, got: {log}"
    );
}

#[cfg(unix)]
#[test]
fn prepare_commit_msg_hook_receives_message_source() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    stage(&repo, "a.txt", "x\n");
    write_hook(
        &repo,
        "prepare-commit-msg",
        "printf '%s\\n' \"$2\" > hook-source.txt",
    );

    let out = run_libra(&["commit", "-m", "body"], &repo);
    assert_eq!(
        out.status.code(),
        Some(0),
        "commit failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        fs::read_to_string(repo.join("hook-source.txt")).unwrap(),
        "message\n"
    );
}

#[cfg(unix)]
#[test]
fn prepare_commit_msg_hook_receives_squash_source() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    stage(&repo, "a.txt", "x\n");
    assert_eq!(
        run_libra(&["commit", "-m", "base"], &repo).status.code(),
        Some(0)
    );

    stage(&repo, "a.txt", "y\n");
    write_hook(
        &repo,
        "prepare-commit-msg",
        "printf '%s\\n' \"$2\" > hook-source.txt",
    );
    let out = run_libra(&["commit", "--no-edit", "--squash", "HEAD"], &repo);
    assert_eq!(
        out.status.code(),
        Some(0),
        "squash commit failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        fs::read_to_string(repo.join("hook-source.txt")).unwrap(),
        "squash\n"
    );
}

#[cfg(unix)]
#[test]
fn prepare_commit_msg_hook_receives_amend_source_and_sha() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    stage(&repo, "a.txt", "x\n");
    assert_eq!(
        run_libra(&["commit", "-m", "base"], &repo).status.code(),
        Some(0)
    );
    let amended = head_oid(&repo);

    stage(&repo, "a.txt", "y\n");
    write_hook(
        &repo,
        "prepare-commit-msg",
        "printf '%s\\n%s\\n' \"$2\" \"$3\" > hook-source.txt",
    );
    let out = run_libra(&["commit", "--amend", "--no-edit"], &repo);
    assert_eq!(
        out.status.code(),
        Some(0),
        "amend commit failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        fs::read_to_string(repo.join("hook-source.txt")).unwrap(),
        format!("commit\n{amended}\n")
    );
}

#[cfg(unix)]
#[test]
fn commit_msg_hook_receives_path_arg() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    stage(&repo, "a.txt", "x\n");
    // The hook overwrites the file (referenced as $1) with a fixed message.
    write_hook(&repo, "commit-msg", "printf 'rewritten-by-hook' > \"$1\"");

    assert_eq!(
        run_libra(&["commit", "-m", "original"], &repo)
            .status
            .code(),
        Some(0)
    );
    let log = String::from_utf8_lossy(&run_libra(&["log", "-1"], &repo).stdout).into_owned();
    assert!(
        log.contains("rewritten-by-hook"),
        "commit-msg hook should receive and rewrite the message file, got: {log}"
    );
}

// ---------------------------------------------------------------------------
// Signing decisions
// ---------------------------------------------------------------------------

#[test]
fn dash_s_signs() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    stage(&repo, "a.txt", "x\n");
    assert!(signed_field(&repo, &["-S", "-m", "signed"]));
}

#[test]
fn no_gpg_sign_does_not_sign() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    stage(&repo, "a.txt", "x\n");
    assert!(!signed_field(&repo, &["--no-gpg-sign", "-m", "unsigned"]));
}

#[test]
fn gpgsign_config_true_signs_bypassing_vault_gate() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    stage(&repo, "a.txt", "x\n");
    assert!(
        run_libra(&["config", "commit.gpgSign", "true"], &repo)
            .status
            .success()
    );
    assert!(signed_field(&repo, &["-m", "cfg-signed"]));
}

#[test]
fn no_gpg_sign_overrides_config_true() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    stage(&repo, "a.txt", "x\n");
    assert!(
        run_libra(&["config", "commit.gpgSign", "true"], &repo)
            .status
            .success()
    );
    assert!(!signed_field(&repo, &["--no-gpg-sign", "-m", "override"]));
}

#[test]
fn gpgsign_config_reads_both_casings() {
    // The lowercase spelling `commit.gpgsign` must also be honored.
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    stage(&repo, "a.txt", "x\n");
    assert!(
        run_libra(&["config", "commit.gpgsign", "true"], &repo)
            .status
            .success()
    );
    assert!(signed_field(&repo, &["-m", "lowercase-cfg"]));
}

#[test]
fn gpg_sign_and_no_gpg_sign_conflict_exits_129() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    stage(&repo, "a.txt", "x\n");
    let out = run_libra(&["commit", "-S", "--no-gpg-sign", "-m", "x"], &repo);
    assert_eq!(
        out.status.code(),
        Some(129),
        "-S conflicts with --no-gpg-sign (Libra maps clap errors to 129)"
    );
}
