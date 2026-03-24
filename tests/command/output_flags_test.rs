//! Integration tests for global output flags (--json, --machine, --color,
//! --quiet, --no-pager, --exit-code-on-warning, --progress).
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

use tempfile::tempdir;

use super::{assert_cli_success, configure_identity_via_cli, init_repo_via_cli};

/// Run libra with the given arguments in `cwd`, with an isolated HOME.
fn run(args: &[&str], cwd: &Path) -> Output {
    let home = cwd.join(".libra-test-home");
    let config_home = home.join(".config");
    fs::create_dir_all(&config_home).unwrap();

    Command::new(env!("CARGO_BIN_EXE_libra"))
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .env("LIBRA_TEST", "1")
        .output()
        .expect("failed to execute libra binary")
}

/// Run libra with an extra env var.
fn run_with_env(args: &[&str], cwd: &Path, key: &str, value: &str) -> Output {
    let home = cwd.join(".libra-test-home");
    let config_home = home.join(".config");
    fs::create_dir_all(&config_home).unwrap();

    Command::new(env!("CARGO_BIN_EXE_libra"))
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .env("LIBRA_TEST", "1")
        .env(key, value)
        .output()
        .expect("failed to execute libra binary")
}

fn init_repo_with_commit_via_cli(repo: &Path) {
    init_repo_via_cli(repo);
    configure_identity_via_cli(repo);

    fs::write(repo.join("f.txt"), "x").unwrap();
    let add = run(&["add", "f.txt"], repo);
    assert_cli_success(&add, "add");
    let commit = run(&["commit", "-m", "init", "--no-verify"], repo);
    assert_cli_success(&commit, "commit");
}

// ─── --json ──────────────────────────────────────────────────────────────────

#[test]
fn json_error_on_unknown_command() {
    let temp = tempdir().unwrap();
    let output = run(&["--json", "nonexistent"], temp.path());
    assert_ne!(output.status.code(), Some(0));
    assert!(
        output.stdout.is_empty(),
        "structured JSON errors should not contaminate stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    let parsed: serde_json::Value = serde_json::from_str(stderr.trim())
        .unwrap_or_else(|e| panic!("expected JSON on stderr, got: {stderr}\nerror: {e}"));
    assert_eq!(parsed["ok"], false);
    assert!(parsed["error_code"].as_str().unwrap().starts_with("LBR-"));
}

#[test]
fn json_error_on_repo_not_found() {
    let temp = tempdir().unwrap();
    // Use status --json (after subcommand) so clap doesn't eat "status" as
    // the optional --json value.
    let output = run(&["status", "--json"], temp.path());
    assert_ne!(output.status.code(), Some(0));
    assert!(
        output.stdout.is_empty(),
        "structured JSON errors should not contaminate stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    let parsed: serde_json::Value = serde_json::from_str(stderr.trim())
        .unwrap_or_else(|e| panic!("expected JSON on stderr, got: {stderr}\nerror: {e}"));
    assert_eq!(parsed["ok"], false);
    assert_eq!(parsed["error_code"], "LBR-REPO-001");
}

// ─── --machine ───────────────────────────────────────────────────────────────

#[test]
fn machine_error_is_json() {
    let temp = tempdir().unwrap();
    let output = run(&["--machine", "nonexistent"], temp.path());
    assert_ne!(output.status.code(), Some(0));
    assert!(
        output.stdout.is_empty(),
        "machine-mode errors should keep stdout empty, got: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    let parsed: serde_json::Value = serde_json::from_str(stderr.trim())
        .unwrap_or_else(|e| panic!("expected JSON on stderr, got: {stderr}\nerror: {e}"));
    assert_eq!(parsed["ok"], false);
}

#[test]
fn machine_overrides_json_for_parse_errors() {
    let temp = tempdir().unwrap();
    let output = run(&["--machine", "-J", "nonexistent"], temp.path());
    assert_ne!(output.status.code(), Some(0));
    assert!(
        output.stdout.is_empty(),
        "machine-mode parse errors should keep stdout empty, got: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    let parsed: serde_json::Value = serde_json::from_str(stderr.trim())
        .unwrap_or_else(|e| panic!("expected JSON on stderr, got: {stderr}\nerror: {e}"));
    assert_eq!(parsed["ok"], false);
    assert!(
        !stderr.contains("\n  "),
        "--machine should force single-line JSON even when -J is also present, got: {stderr}"
    );
}

// ─── --json on success path ───────────────────────────────────────────────────

#[test]
fn json_status_success_returns_structured_data() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo_via_cli(&repo);

    let output = run(&["--json", "status"], &repo);
    assert_cli_success(&output, "json status");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("expected JSON on stdout, got: {stdout}\nerror: {e}"));
    assert_eq!(parsed["ok"], true, "envelope should have ok:true");
    assert_eq!(
        parsed["command"], "status",
        "envelope should have command field"
    );
    let data = &parsed["data"];
    assert!(data.is_object(), "envelope should have data object");
    // Structured fields — not a wrapped text blob.
    assert!(data["head"].is_object(), "data must have head object");
    assert!(
        data["is_clean"].is_boolean(),
        "data must have is_clean boolean"
    );
    assert!(data["staged"].is_object(), "data must have staged object");
    assert!(
        data["untracked"].is_array(),
        "data must have untracked array"
    );
    // Clean repo should be empty.
    assert_eq!(data["is_clean"], true);
}

#[test]
fn json_commit_returns_structured_summary() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo_via_cli(&repo);
    configure_identity_via_cli(&repo);

    fs::write(repo.join("f.txt"), "hello").unwrap();
    let add = run(&["add", "f.txt"], &repo);
    assert_cli_success(&add, "add");

    let output = run(&["--json", "commit", "-m", "initial", "--no-verify"], &repo);
    assert_cli_success(&output, "json commit");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("expected JSON on stdout, got: {stdout}\nerror: {e}"));
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["command"], "commit");
    assert_eq!(parsed["data"]["subject"], "initial");
    assert!(parsed["data"]["commit"].is_string());
    assert_eq!(parsed["data"]["files_changed"]["total"], 1);
    assert_eq!(parsed["data"]["files_changed"]["new"], 1);
}

#[test]
fn quiet_commit_suppresses_summary() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo_via_cli(&repo);
    configure_identity_via_cli(&repo);

    fs::write(repo.join("f.txt"), "hello").unwrap();
    let add = run(&["add", "f.txt"], &repo);
    assert_cli_success(&add, "add");

    let output = run(
        &["--quiet", "commit", "-m", "initial", "--no-verify"],
        &repo,
    );
    assert_cli_success(&output, "quiet commit");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.trim().is_empty(),
        "quiet commit should suppress summary output, got: {stdout}"
    );
}

#[test]
fn json_config_get_returns_structured_value() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo_via_cli(&repo);

    let set = run(&["config", "user.name", "Alice"], &repo);
    assert_cli_success(&set, "config set");

    let output = run(&["--json", "config", "--get", "user.name"], &repo);
    assert_cli_success(&output, "json config --get");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("expected JSON on stdout, got: {stdout}\nerror: {e}"));
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["command"], "config");
    assert_eq!(parsed["data"]["action"], "get");
    assert_eq!(parsed["data"]["key"], "user.name");
    assert_eq!(parsed["data"]["value"], "Alice");
}

#[test]
fn quiet_config_get_suppresses_output() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo_via_cli(&repo);

    let set = run(&["config", "user.name", "Alice"], &repo);
    assert_cli_success(&set, "config set");

    let output = run(&["--quiet", "config", "--get", "user.name"], &repo);
    assert_cli_success(&output, "quiet config --get");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.trim().is_empty(),
        "quiet config --get should suppress stdout, got: {stdout}"
    );
}

#[test]
fn quiet_cat_file_type_suppresses_output() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo_with_commit_via_cli(&repo);

    let output = run(&["--quiet", "cat-file", "-t", "HEAD"], &repo);
    assert_cli_success(&output, "quiet cat-file -t HEAD");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.trim().is_empty(),
        "quiet cat-file should suppress stdout, got: {stdout}"
    );
}

#[test]
fn json_cat_file_badref_returns_structured_error() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo_via_cli(&repo);

    let output = run(&["--json", "cat-file", "-t", "badref"], &repo);
    assert_ne!(output.status.code(), Some(0));
    assert!(
        output.stdout.is_empty(),
        "structured JSON errors should not contaminate stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    let parsed: serde_json::Value = serde_json::from_str(stderr.trim())
        .unwrap_or_else(|e| panic!("expected JSON on stderr, got: {stderr}\nerror: {e}"));
    assert_eq!(parsed["ok"], false);
    assert!(
        parsed["message"]
            .as_str()
            .unwrap_or_default()
            .contains("Not a valid object name badref"),
        "expected invalid object error, got: {stderr}"
    );
}

#[test]
fn quiet_status_suppresses_output() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo_via_cli(&repo);

    let output = run(&["--quiet", "status"], &repo);
    assert_cli_success(&output, "quiet status");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.trim().is_empty(),
        "expected no output with --quiet, got: {stdout}"
    );
}

#[test]
fn quiet_status_invalid_index_still_returns_error() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo_via_cli(&repo);

    fs::write(repo.join(".libra").join("index"), b"not a valid index").unwrap();

    let output = run(&["--quiet", "status"], &repo);
    assert_ne!(output.status.code(), Some(0));
    assert!(
        output.stdout.is_empty(),
        "quiet status should not emit stdout on failure: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("failed to determine working tree status"),
        "quiet status should preserve the real status error, got: {stderr}"
    );
}

// ─── --json on dirty worktree ─────────────────────────────────────────────────

#[test]
fn json_status_dirty_repo_has_structured_untracked() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo_via_cli(&repo);

    // Create an untracked file so the worktree is dirty.
    fs::write(repo.join("untracked.txt"), "dirty").unwrap();

    let output = run(&["--json", "status"], &repo);
    assert_cli_success(&output, "json status dirty");

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Must be valid JSON — no stray human text before the envelope.
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("expected valid JSON on stdout, got: {stdout}\nerror: {e}"));
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["data"]["is_clean"], false, "dirty repo is not clean");

    let untracked = parsed["data"]["untracked"]
        .as_array()
        .expect("untracked must be an array");
    let names: Vec<&str> = untracked.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        names.iter().any(|n| n.contains("untracked.txt")),
        "untracked array should contain 'untracked.txt', got: {names:?}"
    );
}

#[test]
fn json_status_ignored_flag_includes_ignored_entries() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo_via_cli(&repo);

    fs::write(repo.join(".libraignore"), "ignored.txt\n").unwrap();
    fs::write(repo.join("ignored.txt"), "ignore me").unwrap();

    let output = run(&["--json", "status", "--ignored"], &repo);
    assert_cli_success(&output, "json status ignored");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("expected valid JSON on stdout, got: {stdout}\nerror: {e}"));
    let ignored = parsed["data"]["ignored"]
        .as_array()
        .expect("ignored must be an array");
    let names: Vec<&str> = ignored.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        names.iter().any(|n| n.contains("ignored.txt")),
        "ignored array should contain 'ignored.txt', got: {names:?}"
    );
}

#[test]
fn json_status_untracked_files_no_suppresses_untracked_entries() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo_via_cli(&repo);

    fs::write(repo.join("untracked.txt"), "dirty").unwrap();

    let output = run(&["--json", "status", "--untracked-files=no"], &repo);
    assert_cli_success(&output, "json status untracked-files=no");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("expected valid JSON on stdout, got: {stdout}\nerror: {e}"));
    let untracked = parsed["data"]["untracked"]
        .as_array()
        .expect("untracked must be an array");
    assert!(
        untracked.is_empty(),
        "--untracked-files=no should suppress untracked entries, got: {untracked:?}"
    );
}

#[test]
fn json_status_invalid_index_returns_structured_error() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo_via_cli(&repo);

    fs::write(repo.join(".libra").join("index"), b"not a valid index").unwrap();

    let output = run(&["--json", "status"], &repo);
    assert_ne!(output.status.code(), Some(0));
    assert!(
        output.stdout.is_empty(),
        "structured JSON errors should not contaminate stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    let parsed: serde_json::Value = serde_json::from_str(stderr.trim())
        .unwrap_or_else(|e| panic!("expected JSON error on stderr, got: {stderr}\nerror: {e}"));
    assert_eq!(parsed["ok"], false);
    assert!(
        parsed["message"]
            .as_str()
            .unwrap_or_default()
            .contains("failed to determine working tree status"),
        "expected structured status error, got: {stderr}"
    );
}

// ─── --json on branch ────────────────────────────────────────────────────────

#[test]
fn json_branch_returns_json_with_branches() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo_with_commit_via_cli(&repo);

    let output = run(&["--json", "branch"], &repo);
    assert_cli_success(&output, "json branch");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("expected JSON on stdout, got: {stdout}\nerror: {e}"));
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["command"], "branch");
    let branches = parsed["data"]["branches"]
        .as_array()
        .expect("expected branches array");
    assert!(!branches.is_empty(), "should have at least one branch");
    assert!(
        branches.iter().any(|b| b["current"] == true),
        "one branch should be marked current"
    );
}

#[test]
fn json_show_ref_returns_structured_entries() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo_with_commit_via_cli(&repo);

    let output = run(&["--json", "show-ref", "--head"], &repo);
    assert_cli_success(&output, "json show-ref --head");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("expected JSON on stdout, got: {stdout}\nerror: {e}"));
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["command"], "show-ref");
    let entries = parsed["data"]["entries"]
        .as_array()
        .expect("show-ref entries should be an array");
    assert!(
        entries.iter().any(|entry| entry["refname"] == "HEAD"),
        "expected HEAD entry, got: {entries:?}"
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry["refname"] == "refs/heads/main"),
        "expected refs/heads/main entry, got: {entries:?}"
    );
}

#[test]
fn quiet_branch_suppresses_output() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo_via_cli(&repo);

    let output = run(&["--quiet", "branch"], &repo);
    assert_cli_success(&output, "quiet branch");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.trim().is_empty(),
        "expected no output with --quiet branch, got: {stdout}"
    );
}

#[test]
fn quiet_show_ref_suppresses_output() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo_with_commit_via_cli(&repo);

    let output = run(&["--quiet", "show-ref", "--head"], &repo);
    assert_cli_success(&output, "quiet show-ref --head");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.trim().is_empty(),
        "quiet show-ref should suppress stdout, got: {stdout}"
    );
}

#[test]
fn quiet_branch_set_upstream_suppresses_output() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo_with_commit_via_cli(&repo);

    let output = run(
        &["--quiet", "branch", "--set-upstream-to", "origin/main"],
        &repo,
    );
    assert_cli_success(&output, "quiet branch --set-upstream-to");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.trim().is_empty(),
        "quiet branch upstream setup should suppress informational output, got: {stdout}"
    );
}

// ─── switch / checkout output suppression ────────────────────────────────────

#[test]
fn machine_switch_dirty_repo_returns_only_json_error() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo_with_commit_via_cli(&repo);

    fs::write(repo.join("f.txt"), "dirty").unwrap();

    let output = run(&["--machine", "switch", "--detach", "main"], &repo);
    assert_ne!(output.status.code(), Some(0));
    assert!(
        output.stdout.is_empty(),
        "machine mode must keep stdout empty on error, got: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    let parsed: serde_json::Value = serde_json::from_str(stderr.trim())
        .unwrap_or_else(|e| panic!("expected JSON error on stderr, got: {stderr}\nerror: {e}"));
    assert_eq!(parsed["ok"], false);
    assert!(
        !stderr.contains("On branch") && !stderr.contains("Changes not staged"),
        "machine mode must not leak human status text, got: {stderr}"
    );
}

#[test]
fn quiet_switch_dirty_repo_suppresses_status_summary() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo_with_commit_via_cli(&repo);

    fs::write(repo.join("f.txt"), "dirty").unwrap();

    let output = run(&["--quiet", "switch", "--detach", "main"], &repo);
    assert_ne!(output.status.code(), Some(0));

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.trim().is_empty(),
        "quiet switch should not print status summary, got: {stdout}"
    );
}

#[test]
fn quiet_checkout_existing_branch_suppresses_output() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo_with_commit_via_cli(&repo);

    let branch = run(&["branch", "foo"], &repo);
    assert_cli_success(&branch, "branch foo");

    let output = run(&["--quiet", "checkout", "foo"], &repo);
    assert_cli_success(&output, "quiet checkout foo");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.trim().is_empty(),
        "quiet checkout should suppress informational output, got: {stdout}"
    );
}

#[test]
fn machine_checkout_existing_branch_suppresses_human_output() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo_with_commit_via_cli(&repo);

    let branch = run(&["branch", "foo"], &repo);
    assert_cli_success(&branch, "branch foo");

    let output = run(&["--machine", "checkout", "foo"], &repo);
    assert_cli_success(&output, "machine checkout foo");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.trim().is_empty(),
        "machine checkout should not emit human text, got: {stdout}"
    );
}

#[test]
fn checkout_invalid_index_preserves_status_error() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo_with_commit_via_cli(&repo);

    let branch = run(&["branch", "foo"], &repo);
    assert_cli_success(&branch, "branch foo");

    fs::write(repo.join(".libra").join("index"), b"not a valid index").unwrap();

    let output = run(&["checkout", "foo"], &repo);
    assert_ne!(output.status.code(), Some(0));

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("failed to determine working tree status"),
        "checkout should preserve status failures, got: {stderr}"
    );
    assert!(
        !stderr.contains("local changes would be overwritten by checkout"),
        "checkout should not collapse index corruption into a dirty-tree message, got: {stderr}"
    );
}

#[test]
fn quiet_merge_fast_forward_suppresses_output() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo_with_commit_via_cli(&repo);

    let checkout_feature = run(&["checkout", "-b", "feature"], &repo);
    assert_cli_success(&checkout_feature, "checkout -b feature");

    fs::write(repo.join("f.txt"), "feature change").unwrap();
    let add = run(&["add", "f.txt"], &repo);
    assert_cli_success(&add, "add");
    let commit = run(&["commit", "-m", "feature", "--no-verify"], &repo);
    assert_cli_success(&commit, "commit feature");

    let checkout_main = run(&["checkout", "main"], &repo);
    assert_cli_success(&checkout_main, "checkout main");

    let output = run(&["--quiet", "merge", "feature"], &repo);
    assert_cli_success(&output, "quiet merge feature");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.trim().is_empty(),
        "quiet fast-forward merge should suppress informational stdout, got: {stdout}"
    );
}

#[test]
fn quiet_log_suppresses_output() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo_with_commit_via_cli(&repo);

    let output = run(&["--quiet", "log"], &repo);
    assert_cli_success(&output, "quiet log");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.trim().is_empty(),
        "quiet log should suppress stdout, got: {stdout}"
    );
}

#[test]
fn json_log_is_rejected_with_structured_error() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo_with_commit_via_cli(&repo);

    let output = run(&["--json", "log"], &repo);
    assert_ne!(output.status.code(), Some(0));
    assert!(
        output.stdout.is_empty(),
        "structured JSON errors should not contaminate stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    let parsed: serde_json::Value = serde_json::from_str(stderr.trim())
        .unwrap_or_else(|e| panic!("expected JSON on stderr, got: {stderr}\nerror: {e}"));
    assert_eq!(parsed["ok"], false);
    assert!(
        parsed["message"]
            .as_str()
            .unwrap_or_default()
            .contains("does not yet support --json or --machine output"),
        "expected unsupported-json error, got: {stderr}"
    );
}

#[test]
fn quiet_blame_suppresses_output() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo_with_commit_via_cli(&repo);

    let output = run(&["--quiet", "blame", "f.txt"], &repo);
    assert_cli_success(&output, "quiet blame");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.trim().is_empty(),
        "quiet blame should suppress stdout, got: {stdout}"
    );
}

// ─── --json=pretty on error ──────────────────────────────────────────────────

#[test]
fn json_pretty_error_is_indented() {
    let temp = tempdir().unwrap();
    // status outside a repo should fail with JSON.
    let output = run(&["--json=pretty", "status"], temp.path());
    assert_ne!(output.status.code(), Some(0));
    assert!(
        output.stdout.is_empty(),
        "structured JSON errors should not contaminate stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    // Pretty-printed JSON should contain newlines and indentation.
    assert!(
        stderr.contains('\n') && stderr.contains("  "),
        "expected pretty-printed JSON, got: {stderr}"
    );
}

// ─── --color=never ───────────────────────────────────────────────────────────

#[test]
fn color_never_has_no_ansi_escapes() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo_via_cli(&repo);

    let output = run(&["--color=never", "status"], &repo);
    assert_cli_success(&output, "status --color=never");

    let stdout = String::from_utf8_lossy(&output.stdout);
    // ANSI escape codes start with \x1b[
    assert!(
        !stdout.contains("\x1b["),
        "expected no ANSI escapes in --color=never output, got: {stdout}"
    );
}

// ─── NO_COLOR env ────────────────────────────────────────────────────────────

#[test]
fn no_color_env_disables_colors() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo_via_cli(&repo);

    let output = run_with_env(&["status"], &repo, "NO_COLOR", "1");
    assert_cli_success(&output, "NO_COLOR=1 status");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("\x1b["),
        "expected no ANSI escapes with NO_COLOR env, got: {stdout}"
    );
}

// ─── --quiet ─────────────────────────────────────────────────────────────────

#[test]
fn quiet_init_suppresses_output() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("quiet-repo");
    fs::create_dir_all(&repo).unwrap();

    let output = run(&["--quiet", "init"], &repo);
    assert_cli_success(&output, "quiet init");

    let stdout = String::from_utf8_lossy(&output.stdout);
    // With --quiet, the "Initialized empty Libra repository" message should
    // not appear.  Note: init.rs currently has its own --quiet flag; the
    // global --quiet should behave equivalently.
    assert!(
        stdout.trim().is_empty() || !stdout.contains("Initialized"),
        "expected quiet init to suppress informational output, got: {stdout}"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.trim().is_empty(),
        "quiet init should suppress progress on stderr, got: {stderr}"
    );
}

#[test]
fn json_init_suppresses_progress_and_returns_one_envelope() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("json-init-repo");
    fs::create_dir_all(&repo).unwrap();

    let output = run(&["--json", "init", "--vault", "false"], &repo);
    assert_cli_success(&output, "json init");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.trim().is_empty(),
        "json init should suppress progress stderr, got: {stderr}"
    );

    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("expected JSON on stdout, got: {stdout}\nerror: {e}"));
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["command"], "init");
}

#[test]
fn human_init_writes_progress_to_stderr_and_summary_to_stdout() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("human-init-repo");
    fs::create_dir_all(&repo).unwrap();

    let output = run(&["init", "--vault", "false"], &repo);
    assert_cli_success(&output, "human init");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("Initialized empty Libra repository"),
        "human init should write final summary to stdout, got: {stdout}"
    );
    assert!(
        stderr.contains("Creating repository layout ..."),
        "human init should write progress to stderr, got: {stderr}"
    );
    assert!(
        stderr.contains("Initializing database ..."),
        "human init should write progress to stderr, got: {stderr}"
    );
}

// ─── --no-pager ──────────────────────────────────────────────────────────────

#[test]
fn no_pager_log_produces_output() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo_via_cli(&repo);
    configure_identity_via_cli(&repo);

    // Create a commit so log has something to show.
    fs::write(repo.join("file.txt"), "hello").unwrap();
    let add = run(&["add", "file.txt"], &repo);
    assert_cli_success(&add, "add");
    let commit = run(&["commit", "-m", "first", "--no-verify"], &repo);
    assert_cli_success(&commit, "commit");

    let output = run(&["--no-pager", "log"], &repo);
    assert_cli_success(&output, "no-pager log");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("first"),
        "expected log output to contain commit message, got: {stdout}"
    );
}

// ─── --help shows global flags ───────────────────────────────────────────────

#[test]
fn help_shows_global_flags() {
    let temp = tempdir().unwrap();
    let output = run(&["--help"], temp.path());
    assert_cli_success(&output, "help");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--json"), "help should mention --json");
    assert!(
        stdout.contains("--machine"),
        "help should mention --machine"
    );
    assert!(stdout.contains("--color"), "help should mention --color");
    assert!(stdout.contains("--quiet"), "help should mention --quiet");
    assert!(
        stdout.contains("--no-pager"),
        "help should mention --no-pager"
    );
    assert!(
        stdout.contains("--progress"),
        "help should mention --progress"
    );
    assert!(
        stdout.contains("--exit-code-on-warning"),
        "help should mention --exit-code-on-warning"
    );
}

// ─── subcommand --help shows inherited flags ─────────────────────────────────

#[test]
fn subcommand_help_shows_global_flags() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo_via_cli(&repo);

    let output = run(&["status", "--help"], &repo);
    assert_cli_success(&output, "status --help");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--json"),
        "subcommand help should inherit --json flag"
    );
}

#[test]
fn branch_help_documents_quiet_listing_deviation() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo_via_cli(&repo);

    let output = run(&["branch", "--help"], &repo);
    assert_cli_success(&output, "branch --help");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("This differs from `git branch --quiet`"),
        "branch help should document quiet-mode deviation, got: {stdout}"
    );
}
