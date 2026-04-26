//! Binary-level `libra clone` behavior checks.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use std::{fs, path::Path, process::Command};

use tempfile::tempdir;

use super::parse_cli_error_stderr;

fn run_libra(args: &[&str], cwd: &Path) -> std::process::Output {
    let home = cwd.join(".home");
    let config_home = home.join(".config");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&config_home).unwrap();

    Command::new(env!("CARGO_BIN_EXE_libra"))
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("HOME", home)
        .env("USERPROFILE", cwd.join(".home"))
        .env("XDG_CONFIG_HOME", config_home)
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .output()
        .unwrap()
}

fn run_libra_with_home(args: &[&str], cwd: &Path, home: &Path) -> std::process::Output {
    let config_home = home.join(".config");
    fs::create_dir_all(&config_home).unwrap();

    Command::new(env!("CARGO_BIN_EXE_libra"))
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("XDG_CONFIG_HOME", config_home)
        .env("LANG", "C")
        .env("LC_ALL", "C")
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
    assert!(
        run_git(&["symbolic-ref", "HEAD", "refs/heads/main"], &remote)
            .status
            .success()
    );
    remote
}

fn create_remote_with_gitignore(base: &Path) -> std::path::PathBuf {
    let remote = base.join("remote-with-ignore.git");
    assert!(
        run_git(&["init", "--bare", remote.to_str().unwrap()], base)
            .status
            .success()
    );

    let work = base.join("work-with-ignore");
    fs::create_dir_all(work.join("nested")).unwrap();
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
    fs::write(work.join(".gitignore"), "ignored-root.log\n").unwrap();
    fs::write(work.join("nested").join(".gitignore"), "*.tmp\n").unwrap();
    assert!(
        run_git(
            &["add", "README.md", ".gitignore", "nested/.gitignore"],
            &work
        )
        .status
        .success()
    );
    assert!(
        run_git(&["commit", "-m", "initial with ignore files"], &work)
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
    assert!(
        run_git(&["symbolic-ref", "HEAD", "refs/heads/main"], &remote)
            .status
            .success()
    );
    remote
}

fn create_empty_remote(base: &Path) -> std::path::PathBuf {
    let remote = base.join("empty-remote.git");
    assert!(
        run_git(&["init", "--bare", remote.to_str().unwrap()], base)
            .status
            .success()
    );
    remote
}

// =========================================================================
// Existing tests (updated for new output behavior)
// =========================================================================

#[test]
fn invalid_source_does_not_panic() {
    let temp = tempdir().unwrap();
    let dest = temp.path().join("dest");
    let output = run_libra(&["clone", "/", dest.to_str().unwrap()], temp.path());
    assert_eq!(output.status.code(), Some(128));

    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert!(
        stderr.contains("fatal:"),
        "expected fatal message, got: {stderr}"
    );
    assert!(
        stderr.contains("LBR-REPO-001"),
        "expected error code, got: {stderr}"
    );
    assert!(
        stderr.to_ascii_lowercase().contains("hint"),
        "expected hint, got: {stderr}"
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
    assert!(stderr.contains("remote branch"));
    assert!(stderr.contains("nope"));
    assert!(stderr.contains("LBR-REPO-003"));
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
    assert!(
        stdout.contains("Cloned into"),
        "expected clone summary on stdout, got: {stdout}"
    );
    assert!(
        stdout.contains("branch: main"),
        "expected branch info, got: {stdout}"
    );
    assert!(stderr.contains("Connecting to"));
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
    );

    let gpg_output = run_libra_with_home(&["config", "--get", "vault.gpg.pubkey"], &dest, &home);
    assert_eq!(gpg_output.status.code(), Some(0));
    assert!(
        !String::from_utf8_lossy(&gpg_output.stdout)
            .trim()
            .is_empty()
    );
}

#[test]
fn clone_converts_gitignore_files_to_visible_libraignore_files() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_gitignore(temp.path());
    let dest = temp.path().join("clone-ignore");

    let output = run_libra(
        &["clone", remote.to_str().unwrap(), dest.to_str().unwrap()],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "clone failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert_eq!(
        fs::read_to_string(dest.join(".libraignore")).unwrap(),
        "ignored-root.log\n"
    );
    assert_eq!(
        fs::read_to_string(dest.join("nested").join(".libraignore")).unwrap(),
        "*.tmp\n"
    );

    fs::write(dest.join("ignored-root.log"), "ignored\n").unwrap();
    fs::write(dest.join("nested").join("ignored.tmp"), "ignored\n").unwrap();
    fs::write(dest.join("visible.txt"), "visible\n").unwrap();

    let status = run_libra(&["status", "--short"], &dest);
    assert_eq!(
        status.status.code(),
        Some(0),
        "status failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let stdout = String::from_utf8_lossy(&status.stdout);
    assert!(
        stdout.contains("?? .libraignore") && stdout.contains("?? nested/.libraignore"),
        "converted .libraignore files should remain visible, got: {stdout}"
    );
    assert!(
        stdout.contains("?? visible.txt"),
        "non-ignored untracked files should remain visible, got: {stdout}"
    );
    assert!(
        !stdout.contains("ignored-root.log") && !stdout.contains("ignored.tmp"),
        "converted ignore rules should hide matching files, got: {stdout}"
    );
}

#[test]
fn bare_clone_does_not_create_libraignore() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_gitignore(temp.path());
    let dest = temp.path().join("bare-ignore.git");

    let output = run_libra(
        &[
            "clone",
            "--bare",
            remote.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "bare clone failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !dest.join(".libraignore").exists(),
        "bare clone should not create a worktree .libraignore"
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
    assert!(
        !stdout.trim().is_empty(),
        "machine clone should emit JSON on stdout"
    );
    let json: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout should be valid JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "clone");
    assert!(
        stderr.trim().is_empty(),
        "machine clone should suppress decorative stderr, got: {stderr}"
    );
    assert!(dest.join("README.md").exists());
}

#[test]
fn json_clone_does_not_leak_init_output() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let dest = temp.path().join("clone-json");

    let output = run_libra(
        &[
            "--json",
            "clone",
            remote.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "json clone failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout should be valid JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "clone");
    assert!(
        !stderr.contains("\"command\":\"init\"")
            && !stderr.contains("Creating repository layout ..."),
        "clone stderr should not leak init output, got: {stderr}"
    );
    assert!(dest.join("README.md").exists());
}

// =========================================================================
// New tests
// =========================================================================

#[test]
fn json_clone_success_schema() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let dest = temp.path().join("clone-schema");

    let output = run_libra(
        &[
            "--json",
            "clone",
            remote.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(output.status.code(), Some(0));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();

    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "clone");
    let data = &json["data"];
    assert!(data["path"].is_string());
    assert_eq!(data["bare"], false);
    assert!(data["remote_url"].is_string());
    assert_eq!(data["branch"], "main");
    assert!(data["object_format"].is_string());
    assert!(data["repo_id"].is_string());
    assert!(data["vault_signing"].is_boolean());
    assert_eq!(data["shallow"], false);
    assert!(data["warnings"].is_array());
    assert_eq!(data["warnings"].as_array().unwrap().len(), 0);
}

#[test]
fn json_clone_empty_remote() {
    let temp = tempdir().unwrap();
    let remote = create_empty_remote(temp.path());
    let dest = temp.path().join("clone-empty");

    let output = run_libra(
        &[
            "--json",
            "clone",
            remote.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "json clone of empty repo failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();

    assert_eq!(json["ok"], true);
    let data = &json["data"];
    assert!(
        data["branch"].is_null(),
        "empty remote should have branch: null"
    );
    let warnings = data["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str().unwrap().contains("empty repository")),
        "expected empty repo warning, got: {warnings:?}"
    );
}

#[test]
fn machine_clone_single_line_json() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let dest = temp.path().join("clone-machine-line");

    let output = run_libra(
        &[
            "--machine",
            "clone",
            remote.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(output.status.code(), Some(0));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let non_empty_lines: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(
        non_empty_lines.len(),
        1,
        "machine stdout should be exactly 1 non-empty line, got: {non_empty_lines:?}"
    );
    let _json: serde_json::Value =
        serde_json::from_str(non_empty_lines[0]).expect("single line should be valid JSON");
}

#[test]
fn quiet_clone_no_output_on_success() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let dest = temp.path().join("clone-quiet");

    let output = run_libra(
        &[
            "--quiet",
            "clone",
            remote.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(output.status.code(), Some(0));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.trim().is_empty(),
        "quiet clone should produce no stdout, got: {stdout}"
    );
    assert!(
        stderr.trim().is_empty(),
        "quiet clone should produce no stderr, got: {stderr}"
    );
    assert!(dest.join("README.md").exists());
}

#[test]
fn error_code_cannot_infer_destination() {
    let temp = tempdir().unwrap();
    let output = run_libra(&["clone", "///"], temp.path());
    assert_eq!(output.status.code(), Some(129));

    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert!(
        stderr.contains("LBR-CLI-002"),
        "expected LBR-CLI-002, got: {stderr}"
    );
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert_eq!(report.exit_code, 129);
}

#[test]
fn error_code_destination_exists_non_empty() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let dest = temp.path().join("non-empty-dest");
    fs::create_dir_all(&dest).unwrap();
    fs::write(dest.join("blocker.txt"), "exists").unwrap();

    let output = run_libra(
        &["clone", remote.to_str().unwrap(), dest.to_str().unwrap()],
        temp.path(),
    );
    assert_ne!(output.status.code(), Some(0));

    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert!(
        stderr.contains("LBR-CLI-003"),
        "expected LBR-CLI-003, got: {stderr}"
    );
    assert_eq!(report.exit_code, 129);
}

#[test]
fn error_code_missing_local_repo() {
    let temp = tempdir().unwrap();
    let output = run_libra(&["clone", "/nonexistent/path/to/repo"], temp.path());
    assert_eq!(output.status.code(), Some(128));

    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert!(
        stderr.contains("LBR-REPO-001"),
        "expected LBR-REPO-001 for missing local repo, got: {stderr}"
    );
    assert_eq!(report.error_code, "LBR-REPO-001");
}

#[test]
fn error_code_remote_branch_not_found() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let dest = temp.path().join("clone-bad-branch");

    let output = run_libra(
        &[
            "clone",
            "-b",
            "nonexistent-branch",
            remote.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(output.status.code(), Some(128));

    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-REPO-003");
    assert!(stderr.contains("nonexistent-branch"));
}

#[test]
fn hint_present_on_network_like_errors() {
    let temp = tempdir().unwrap();
    let output = run_libra(&["clone", "/nonexistent/path/to/repo"], temp.path());
    assert_ne!(output.status.code(), Some(0));

    let (stderr, _report) = parse_cli_error_stderr(&output.stderr);
    assert!(
        stderr.to_ascii_lowercase().contains("hint"),
        "expected a hint in error output, got: {stderr}"
    );
}

#[test]
fn json_clone_init_output_isolation() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let dest = temp.path().join("clone-isolation");

    let output = run_libra(
        &[
            "--json",
            "clone",
            remote.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(output.status.code(), Some(0));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout should be a single valid JSON object");
    assert_eq!(
        json["command"], "clone",
        "unexpected command in JSON envelope"
    );
    assert_eq!(json["ok"], true);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("\"progress\""),
        "json clone stderr should not contain fetch NDJSON progress, got: {stderr}"
    );
}
