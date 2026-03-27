//! Structured-output tests for `libra init`.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use std::{fs, path::Path};

use tempfile::tempdir;

use super::{assert_cli_success, run_libra_command};

#[test]
fn json_init_returns_structured_schema() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();

    let output = run_libra_command(&["--json", "init", "--vault", "false"], &repo);
    assert_cli_success(&output, "json init");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.trim().is_empty(),
        "json init should keep stderr clean: {stderr}"
    );

    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|error| panic!("expected JSON output, got: {stdout}\nerror: {error}"));
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["command"], "init");

    let data = &parsed["data"];
    let path = data["path"].as_str().expect("path should be a string");
    assert!(
        Path::new(path).is_absolute(),
        "path should be absolute, got: {path}"
    );
    assert!(path.ends_with("/.libra"));
    assert_eq!(data["bare"], false);
    assert_eq!(data["initial_branch"].as_str(), Some("main"));
    assert_eq!(data["object_format"].as_str(), Some("sha1"));
    assert_eq!(data["ref_format"].as_str(), Some("strict"));
    assert_eq!(data["vault_signing"].as_bool(), Some(false));
    let repo_id = data["repo_id"]
        .as_str()
        .expect("repo_id should be a string");
    uuid::Uuid::parse_str(repo_id).expect("repo_id should be a UUID");
    assert!(data["converted_from"].is_null());
    assert!(data["ssh_key_detected"].is_null());
    assert_eq!(
        data["warnings"].as_array().map(Vec::len),
        Some(0),
        "warnings should default to an empty array"
    );
}

#[test]
fn machine_init_is_single_line_json() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();

    let output = run_libra_command(&["--machine", "init", "--vault", "false"], &repo);
    assert_cli_success(&output, "machine init");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.trim().is_empty(),
        "machine init should keep stderr clean: {stderr}"
    );

    let non_empty_lines: Vec<_> = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert_eq!(
        non_empty_lines.len(),
        1,
        "machine init should emit one JSON line: {stdout}"
    );

    let parsed: serde_json::Value = serde_json::from_str(non_empty_lines[0])
        .unwrap_or_else(|error| panic!("expected single-line JSON, got: {stdout}\nerror: {error}"));
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["command"], "init");
    assert_eq!(parsed["data"]["vault_signing"], false);
    assert!(parsed["data"]["ssh_key_detected"].is_null());
}

#[test]
fn bare_json_init_reports_repo_root_path() {
    let temp = tempdir().unwrap();

    let output = run_libra_command(
        &["--json", "init", "--bare", "repo.git", "--vault", "false"],
        temp.path(),
    );
    assert_cli_success(&output, "bare json init");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("expected JSON output for bare init");
    assert_eq!(parsed["data"]["bare"], true);

    let path = parsed["data"]["path"]
        .as_str()
        .expect("path should be a string");
    assert!(
        !path.ends_with("/.libra"),
        "bare init path should point at the repo root, got: {path}"
    );
}
