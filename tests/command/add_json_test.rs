//! Structured JSON output tests for `libra add`.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use std::fs;

use tempfile::tempdir;

use super::{assert_cli_success, configure_identity_via_cli, init_repo_via_cli, run_libra_command};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_json_stdout(output: &std::process::Output) -> serde_json::Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("expected JSON output, got: {stdout}\nerror: {e}"))
}

/// Create a repo with identity configured and an initial commit.
fn create_committed_repo() -> tempfile::TempDir {
    let repo = tempdir().expect("tempdir");
    init_repo_via_cli(repo.path());
    configure_identity_via_cli(repo.path());

    fs::write(repo.path().join("base.txt"), "base\n").unwrap();
    let out = run_libra_command(&["add", "base.txt"], repo.path());
    assert_cli_success(&out, "add base.txt");
    let out = run_libra_command(&["commit", "-m", "initial", "--no-verify"], repo.path());
    assert_cli_success(&out, "initial commit");

    repo
}

// ---------------------------------------------------------------------------
// Schema completeness
// ---------------------------------------------------------------------------

#[test]
fn json_add_new_file_returns_structured_schema() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());

    fs::write(repo.path().join("new_file.rs"), "fn main() {}").unwrap();

    let output = run_libra_command(&["--json", "add", "new_file.rs"], repo.path());
    assert_cli_success(&output, "json add new file");

    let parsed = parse_json_stdout(&output);
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["command"], "add");

    let data = &parsed["data"];
    // added is a string array containing the new file
    let added = data["added"].as_array().expect("added should be array");
    assert!(
        added.iter().any(|v| v.as_str() == Some("new_file.rs")),
        "added should contain new_file.rs, got: {added:?}"
    );
    // Other arrays should be empty
    assert_eq!(data["modified"].as_array().map(Vec::len), Some(0));
    assert_eq!(data["removed"].as_array().map(Vec::len), Some(0));
    assert_eq!(data["refreshed"].as_array().map(Vec::len), Some(0));
    assert_eq!(data["ignored"].as_array().map(Vec::len), Some(0));
    assert_eq!(data["failed"].as_array().map(Vec::len), Some(0));
    assert_eq!(data["dry_run"], false);
}

#[test]
fn json_add_modified_file() {
    let repo = create_committed_repo();

    fs::write(repo.path().join("base.txt"), "modified\n").unwrap();

    let output = run_libra_command(&["--json", "add", "base.txt"], repo.path());
    assert_cli_success(&output, "json add modified");

    let parsed = parse_json_stdout(&output);
    assert_eq!(parsed["ok"], true);

    let data = &parsed["data"];
    let modified = data["modified"]
        .as_array()
        .expect("modified should be array");
    assert!(
        modified.iter().any(|v| v.as_str() == Some("base.txt")),
        "modified should contain base.txt, got: {modified:?}"
    );
    assert_eq!(data["added"].as_array().map(Vec::len), Some(0));
}

// ---------------------------------------------------------------------------
// --dry-run --json
// ---------------------------------------------------------------------------

#[test]
fn json_dry_run_does_not_modify_index() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());

    fs::write(repo.path().join("a.rs"), "content").unwrap();

    let output = run_libra_command(&["--json", "add", "--dry-run", "a.rs"], repo.path());
    assert_cli_success(&output, "json dry-run add");

    let parsed = parse_json_stdout(&output);
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["data"]["dry_run"], true);
    let added = parsed["data"]["added"]
        .as_array()
        .expect("added should be array");
    assert!(
        added.iter().any(|v| v.as_str() == Some("a.rs")),
        "dry-run should preview a.rs"
    );

    // Verify index was NOT modified
    let status = run_libra_command(&["status", "--short"], repo.path());
    let stdout = String::from_utf8_lossy(&status.stdout);
    assert!(
        !stdout.contains("A  a.rs"),
        "dry-run should not stage the file: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// --refresh --json
// ---------------------------------------------------------------------------

#[test]
fn json_refresh_returns_refreshed_array() {
    let repo = create_committed_repo();

    // Touch the file to change mtime (but not content hash)
    let path = repo.path().join("base.txt");
    // Write same content to trigger mtime change
    let content = fs::read(&path).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    fs::write(&path, &content).unwrap();

    let output = run_libra_command(&["--json", "add", "--refresh"], repo.path());
    assert_cli_success(&output, "json refresh");

    let parsed = parse_json_stdout(&output);
    assert_eq!(parsed["ok"], true);

    let data = &parsed["data"];
    // refreshed may or may not be non-empty depending on stat changes
    assert!(data["refreshed"].is_array());
    assert_eq!(data["added"].as_array().map(Vec::len), Some(0));
    assert_eq!(data["modified"].as_array().map(Vec::len), Some(0));
    assert_eq!(data["removed"].as_array().map(Vec::len), Some(0));
}

// ---------------------------------------------------------------------------
// -A --json
// ---------------------------------------------------------------------------

#[test]
fn json_add_all_includes_all_changes() {
    let repo = create_committed_repo();

    // Create new file + modify existing + delete a tracked file
    fs::write(repo.path().join("new.rs"), "new").unwrap();
    fs::write(repo.path().join("base.txt"), "changed\n").unwrap();

    let output = run_libra_command(&["--json", "add", "-A"], repo.path());
    assert_cli_success(&output, "json add -A");

    let parsed = parse_json_stdout(&output);
    assert_eq!(parsed["ok"], true);
    let data = &parsed["data"];

    let added: Vec<&str> = data["added"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    let modified: Vec<&str> = data["modified"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();

    assert!(added.contains(&"new.rs"), "added: {added:?}");
    assert!(modified.contains(&"base.txt"), "modified: {modified:?}");
}

// ---------------------------------------------------------------------------
// -u --json (update tracked only, no new files)
// ---------------------------------------------------------------------------

#[test]
fn json_add_update_excludes_new_files() {
    let repo = create_committed_repo();

    fs::write(repo.path().join("new.rs"), "new").unwrap();
    fs::write(repo.path().join("base.txt"), "changed\n").unwrap();

    let output = run_libra_command(&["--json", "add", "-u"], repo.path());
    assert_cli_success(&output, "json add -u");

    let parsed = parse_json_stdout(&output);
    let data = &parsed["data"];
    let added = data["added"].as_array().unwrap();
    assert!(
        added.is_empty(),
        "-u should not add new files, got: {added:?}"
    );
    let modified: Vec<&str> = data["modified"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(modified.contains(&"base.txt"));
}

// ---------------------------------------------------------------------------
// --force --json
// ---------------------------------------------------------------------------

#[test]
fn json_force_add_ignored_file() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());

    fs::write(repo.path().join(".libraignore"), "ignored.log\n").unwrap();
    fs::write(repo.path().join("ignored.log"), "log data").unwrap();

    let output = run_libra_command(&["--json", "add", "-f", "ignored.log"], repo.path());
    assert_cli_success(&output, "json force add");

    let parsed = parse_json_stdout(&output);
    assert_eq!(parsed["ok"], true);

    let data = &parsed["data"];
    let added: Vec<&str> = data["added"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        added.contains(&"ignored.log"),
        "force should stage ignored file: {added:?}"
    );
    assert_eq!(
        data["ignored"].as_array().map(Vec::len),
        Some(0),
        "ignored list should be empty when --force is used"
    );
}

// ---------------------------------------------------------------------------
// --force --dry-run --json
// ---------------------------------------------------------------------------

#[test]
fn json_force_dry_run_previews_ignored_file() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());

    fs::write(repo.path().join(".libraignore"), "ignored.log\n").unwrap();
    fs::write(repo.path().join("ignored.log"), "log data").unwrap();

    let output = run_libra_command(
        &["--json", "add", "-f", "--dry-run", "ignored.log"],
        repo.path(),
    );
    assert_cli_success(&output, "json force dry-run");

    let parsed = parse_json_stdout(&output);
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["data"]["dry_run"], true);

    let added: Vec<&str> = parsed["data"]["added"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(added.contains(&"ignored.log"));

    // Verify index was NOT modified
    let status = run_libra_command(&["status", "--short"], repo.path());
    let stdout = String::from_utf8_lossy(&status.stdout);
    assert!(!stdout.contains("ignored.log"));
}

// ---------------------------------------------------------------------------
// ignored-only JSON: ok == false
// ---------------------------------------------------------------------------

#[test]
fn json_ignored_only_returns_error() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());

    fs::write(repo.path().join(".libraignore"), "ignored.txt\n").unwrap();
    fs::write(repo.path().join("ignored.txt"), "data").unwrap();

    let output = run_libra_command(&["--json", "add", "ignored.txt"], repo.path());
    assert!(!output.status.success());

    // Error JSON is on stderr (pretty-printed)
    let stderr = String::from_utf8_lossy(&output.stderr);
    let parsed: serde_json::Value =
        serde_json::from_str(stderr.trim()).expect("parse error JSON from stderr");
    assert_eq!(parsed["ok"], false);
    assert_eq!(parsed["error_code"], "LBR-ADD-001");
}

// ---------------------------------------------------------------------------
// Nothing specified JSON
// ---------------------------------------------------------------------------

#[test]
fn json_nothing_specified_returns_error() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());

    let output = run_libra_command(&["--json", "add"], repo.path());
    assert_eq!(output.status.code(), Some(129));

    let stderr = String::from_utf8_lossy(&output.stderr);
    let parsed: serde_json::Value =
        serde_json::from_str(stderr.trim()).expect("parse error JSON from stderr");
    assert_eq!(parsed["ok"], false);
    assert_eq!(parsed["error_code"], "LBR-CLI-002");
}

// ---------------------------------------------------------------------------
// No changes scenario
// ---------------------------------------------------------------------------

#[test]
fn json_add_no_changes() {
    let repo = create_committed_repo();

    // base.txt is already committed and unchanged
    let output = run_libra_command(&["--json", "add", "base.txt"], repo.path());
    assert_cli_success(&output, "json add no changes");

    let parsed = parse_json_stdout(&output);
    assert_eq!(parsed["ok"], true);
    let data = &parsed["data"];
    assert_eq!(data["added"].as_array().map(Vec::len), Some(0));
    assert_eq!(data["modified"].as_array().map(Vec::len), Some(0));
    assert_eq!(data["removed"].as_array().map(Vec::len), Some(0));
}

// ---------------------------------------------------------------------------
// Paths are relative, no leading /
// ---------------------------------------------------------------------------

#[test]
fn json_paths_are_relative_no_leading_slash() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());

    fs::create_dir_all(repo.path().join("src")).unwrap();
    fs::write(repo.path().join("src/main.rs"), "fn main() {}").unwrap();

    let output = run_libra_command(&["--json", "add", "src/main.rs"], repo.path());
    assert_cli_success(&output, "json add with subdirectory");

    let parsed = parse_json_stdout(&output);
    let added = parsed["data"]["added"].as_array().unwrap();
    for path in added {
        let s = path.as_str().unwrap();
        assert!(!s.starts_with('/'), "path should be relative: {s}");
    }
}

// ---------------------------------------------------------------------------
// --machine produces single-line JSON
// ---------------------------------------------------------------------------

#[test]
fn machine_add_is_single_line_json() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());

    fs::write(repo.path().join("file.txt"), "content").unwrap();

    let output = run_libra_command(&["--machine", "add", "file.txt"], repo.path());
    assert_cli_success(&output, "machine add");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let non_empty_lines: Vec<_> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(
        non_empty_lines.len(),
        1,
        "machine output should be exactly 1 line, got: {non_empty_lines:?}"
    );
    let _: serde_json::Value =
        serde_json::from_str(non_empty_lines[0]).expect("machine output should be valid JSON");
}

// ---------------------------------------------------------------------------
// Partial ignore JSON: ok == true + ignored list
// ---------------------------------------------------------------------------

#[test]
fn json_partial_ignore_returns_ok_with_ignored_list() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());

    fs::write(repo.path().join(".libraignore"), "ignored.txt\n").unwrap();
    fs::write(repo.path().join("good.txt"), "good").unwrap();
    fs::write(repo.path().join("ignored.txt"), "ignored").unwrap();

    let output = run_libra_command(&["--json", "add", "good.txt", "ignored.txt"], repo.path());
    // Partial ignore: good.txt is staged, ignored.txt triggers warning
    // The current behavior returns an error because ignored-only pathspec triggers AddNothingStaged
    // But when mixed with good files, good.txt gets staged first and then ignored.txt
    // gets added to the ignored list
    let parsed = parse_json_stdout(&output);
    assert_eq!(parsed["ok"], true);

    let data = &parsed["data"];
    let added: Vec<&str> = data["added"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(added.contains(&"good.txt"), "good.txt should be staged");

    let ignored: Vec<&str> = data["ignored"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        ignored.contains(&"ignored.txt"),
        "ignored.txt should be in ignored list"
    );
}

// ---------------------------------------------------------------------------
// Error JSON: pathspec not matched
// ---------------------------------------------------------------------------

#[test]
fn json_pathspec_not_matched_returns_error() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());

    let output = run_libra_command(&["--json", "add", "nonexistent.rs"], repo.path());
    assert_eq!(output.status.code(), Some(129));

    let stderr = String::from_utf8_lossy(&output.stderr);
    let parsed: serde_json::Value =
        serde_json::from_str(stderr.trim()).expect("parse error JSON from stderr");
    assert_eq!(parsed["ok"], false);
    assert_eq!(parsed["error_code"], "LBR-CLI-003");
    assert!(
        parsed["message"]
            .as_str()
            .unwrap()
            .contains("nonexistent.rs")
    );
}
