//! JSON schema stability tests for `libra commit --json`.
//!
//! **Layer:** L1 — deterministic, no external dependencies.
//!
//! These tests verify the backward-compatible JSON contract: existing fields
//! (`head`, `commit`, `short_id`, `subject`, `root_commit`,
//! `files_changed.total/new/modified/deleted`) must not change shape, and new
//! fields (`branch`, `amend`, `signoff`, `conventional`, `signed`) must be
//! present.

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

fn parse_json_stdout(output: &std::process::Output) -> Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("expected JSON stdout, got: {stdout}\nerror: {e}"))
}

// ---------------------------------------------------------------------------
// Schema completeness
// ---------------------------------------------------------------------------

#[test]
fn json_commit_has_all_required_fields() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    configure_identity(&repo);

    fs::write(repo.join("f.txt"), "hello").unwrap();
    let add = run_libra(&["add", "f.txt"], &repo);
    assert!(add.status.success());

    let output = run_libra(&["--json", "commit", "-m", "initial", "--no-verify"], &repo);
    assert!(output.status.success(), "json commit should succeed");

    let v = parse_json_stdout(&output);
    assert_eq!(v["ok"], true);
    assert_eq!(v["command"], "commit");

    let data = &v["data"];
    // Backward-compatible fields (must match old schema exactly)
    assert!(data["head"].is_string(), "head must be string");
    assert!(data["commit"].is_string(), "commit must be string");
    assert!(data["short_id"].is_string(), "short_id must be string");
    assert!(data["subject"].is_string(), "subject must be string");
    assert!(data["root_commit"].is_boolean(), "root_commit must be bool");
    assert!(
        data["files_changed"]["total"].is_number(),
        "files_changed.total must be number"
    );
    assert!(
        data["files_changed"]["new"].is_number(),
        "files_changed.new must be number"
    );
    assert!(
        data["files_changed"]["modified"].is_number(),
        "files_changed.modified must be number"
    );
    assert!(
        data["files_changed"]["deleted"].is_number(),
        "files_changed.deleted must be number"
    );

    // New fields (incremental extension)
    assert!(
        data["branch"].is_string() || data["branch"].is_null(),
        "branch must be string or null"
    );
    assert!(data["amend"].is_boolean(), "amend must be bool");
    assert!(data["signoff"].is_boolean(), "signoff must be bool");
    assert!(
        data["conventional"].is_boolean() || data["conventional"].is_null(),
        "conventional must be bool or null"
    );
    assert!(data["signed"].is_boolean(), "signed must be bool");
}

#[test]
fn json_root_commit_fields() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    configure_identity(&repo);

    fs::write(repo.join("f.txt"), "hello").unwrap();
    let add = run_libra(&["add", "f.txt"], &repo);
    assert!(add.status.success());

    let output = run_libra(&["--json", "commit", "-m", "initial", "--no-verify"], &repo);
    let v = parse_json_stdout(&output);
    let data = &v["data"];

    assert_eq!(data["root_commit"], true, "first commit is root");
    assert_eq!(data["subject"], "initial");
    assert!(
        data["branch"].is_string(),
        "branch should be present for root commit"
    );
    assert_eq!(data["amend"], false);
    assert_eq!(data["files_changed"]["total"], 1);
    assert_eq!(data["files_changed"]["new"], 1);
}

#[test]
fn json_signoff_field() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    configure_identity(&repo);

    fs::write(repo.join("f.txt"), "hello").unwrap();
    let add = run_libra(&["add", "f.txt"], &repo);
    assert!(add.status.success());

    let output = run_libra(
        &["--json", "commit", "-m", "feat: add", "-s", "--no-verify"],
        &repo,
    );
    assert!(output.status.success());
    let v = parse_json_stdout(&output);
    assert_eq!(
        v["data"]["signoff"], true,
        "signoff should be true when -s is used"
    );
}

#[test]
fn json_conventional_field() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    configure_identity(&repo);

    fs::write(repo.join("f.txt"), "hello").unwrap();
    let add = run_libra(&["add", "f.txt"], &repo);
    assert!(add.status.success());

    let output = run_libra(
        &["--json", "commit", "-m", "test: initial", "--conventional"],
        &repo,
    );
    assert!(output.status.success());
    let v = parse_json_stdout(&output);
    assert_eq!(
        v["data"]["conventional"], true,
        "conventional should be true when --conventional is used and passes"
    );
}

#[test]
fn json_conventional_null_when_not_requested() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    configure_identity(&repo);

    fs::write(repo.join("f.txt"), "hello").unwrap();
    let add = run_libra(&["add", "f.txt"], &repo);
    assert!(add.status.success());

    let output = run_libra(&["--json", "commit", "-m", "initial", "--no-verify"], &repo);
    assert!(output.status.success());
    let v = parse_json_stdout(&output);
    assert!(
        v["data"]["conventional"].is_null(),
        "conventional should be null when --conventional is not requested"
    );
}

#[test]
fn json_amend_field() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    configure_identity(&repo);

    // Create initial commit
    fs::write(repo.join("f.txt"), "hello").unwrap();
    let add = run_libra(&["add", "f.txt"], &repo);
    assert!(add.status.success());
    let c = run_libra(&["commit", "-m", "initial", "--no-verify"], &repo);
    assert!(c.status.success());

    // Amend
    fs::write(repo.join("f.txt"), "updated").unwrap();
    let add2 = run_libra(&["add", "f.txt"], &repo);
    assert!(add2.status.success());

    let output = run_libra(
        &[
            "--json",
            "commit",
            "--amend",
            "-m",
            "amended msg",
            "--no-verify",
        ],
        &repo,
    );
    assert!(output.status.success());
    let v = parse_json_stdout(&output);
    assert_eq!(v["data"]["amend"], true, "amend should be true");
    assert_eq!(v["data"]["subject"], "amended msg");
}

// ---------------------------------------------------------------------------
// Machine format
// ---------------------------------------------------------------------------

#[test]
fn machine_commit_is_single_line_json() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    configure_identity(&repo);

    fs::write(repo.join("f.txt"), "hello").unwrap();
    let add = run_libra(&["add", "f.txt"], &repo);
    assert!(add.status.success());

    let output = run_libra(
        &["--machine", "commit", "-m", "initial", "--no-verify"],
        &repo,
    );
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    let non_empty_lines: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(
        non_empty_lines.len(),
        1,
        "machine mode should produce exactly 1 non-empty line: {stdout}"
    );
    let v: Value = serde_json::from_str(non_empty_lines[0]).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["command"], "commit");
}

// ---------------------------------------------------------------------------
// Error JSON format
// ---------------------------------------------------------------------------

#[test]
fn json_nothing_to_commit_returns_structured_error() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    configure_identity(&repo);

    // Create initial commit first
    fs::write(repo.join("f.txt"), "hello").unwrap();
    let add = run_libra(&["add", "f.txt"], &repo);
    assert!(add.status.success());
    let c = run_libra(&["commit", "-m", "initial", "--no-verify"], &repo);
    assert!(c.status.success());

    // Now try to commit with nothing staged (in json mode)
    let output = run_libra(&["--json", "commit", "-m", "empty", "--no-verify"], &repo);
    assert!(!output.status.success());

    // In --json mode, error JSON goes to stderr as a full JSON object
    let stderr = String::from_utf8_lossy(&output.stderr);
    // The error may be pretty-printed (multi-line), so parse the entire stderr
    let err: Value = serde_json::from_str(stderr.trim()).unwrap_or_else(|_| {
        // If the full stderr isn't valid JSON, try to extract the JSON portion
        // (there may be a human-readable prefix line before the JSON)
        let json_start = stderr.find('{').expect("stderr should contain JSON");
        serde_json::from_str(stderr[json_start..].trim()).unwrap_or_else(|e| {
            panic!("failed to parse error JSON from stderr: {e}\nstderr: {stderr}")
        })
    });
    assert_eq!(err["ok"], false);
    assert_eq!(err["error_code"], "LBR-REPO-003");
}

// ---------------------------------------------------------------------------
// Structured output isolation
// ---------------------------------------------------------------------------

#[test]
fn json_commit_stdout_is_clean_json_only() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    configure_identity(&repo);

    fs::write(repo.join("f.txt"), "hello").unwrap();
    let add = run_libra(&["add", "f.txt"], &repo);
    assert!(add.status.success());

    let output = run_libra(&["--json", "commit", "-m", "initial", "--no-verify"], &repo);
    assert!(output.status.success());

    // stdout should be ONLY valid JSON, no human text mixed in
    let stdout = String::from_utf8_lossy(&output.stdout);
    let _: Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!("stdout should be valid JSON without any human text mixed in.\nstdout: {stdout}\nerror: {e}")
    });
}
