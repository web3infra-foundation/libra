//! Integration tests for `libra check-attr`.
//!
//! Layer: L1 (deterministic; tempdir + isolated HOME, no network).

use std::fs;

use tempfile::tempdir;

use super::{parse_json_stdout, run_libra_command, run_libra_command_with_stdin};

/// Init a repo whose `.libra_attributes` runs `*.bin` through the LFS filter.
fn setup_repo() -> tempfile::TempDir {
    let repo = tempdir().expect("tempdir");
    let init = run_libra_command(&["init"], repo.path());
    assert!(
        init.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );
    fs::write(
        repo.path().join(".libra_attributes"),
        "*.bin filter=lfs diff=lfs merge=lfs -text\n",
    )
    .expect("write .libra_attributes");
    repo
}

fn stdout_of(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn filter_on_tracked_path_reports_lfs() {
    let repo = setup_repo();
    let out = run_libra_command(&["check-attr", "filter", "a.bin"], repo.path());
    assert_eq!(
        out.status.code(),
        Some(0),
        "check-attr always exits 0 on success"
    );
    assert_eq!(stdout_of(&out), "a.bin: filter: lfs\n");
}

#[test]
fn filter_on_untracked_path_is_unspecified() {
    let repo = setup_repo();
    let out = run_libra_command(&["check-attr", "filter", "a.txt"], repo.path());
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout_of(&out), "a.txt: filter: unspecified\n");
}

#[test]
fn all_reports_only_set_attributes() {
    let repo = setup_repo();
    // Tracked path: filter=lfs is set.
    let tracked = run_libra_command(&["check-attr", "--all", "a.bin"], repo.path());
    assert_eq!(tracked.status.code(), Some(0));
    assert_eq!(stdout_of(&tracked), "a.bin: filter: lfs\n");
    // Untracked path: nothing is set.
    let untracked = run_libra_command(&["check-attr", "--all", "a.txt"], repo.path());
    assert_eq!(untracked.status.code(), Some(0));
    assert!(
        stdout_of(&untracked).is_empty(),
        "no attributes are set on an untracked path: {:?}",
        stdout_of(&untracked)
    );
}

#[test]
fn double_dash_separates_multiple_attributes() {
    let repo = setup_repo();
    let out = run_libra_command(
        &["check-attr", "filter", "text", "--", "a.bin"],
        repo.path(),
    );
    assert_eq!(out.status.code(), Some(0));
    let stdout = stdout_of(&out);
    assert!(
        stdout.contains("a.bin: filter: lfs"),
        "filter set: {stdout}"
    );
    assert!(
        stdout.contains("a.bin: text: unspecified"),
        "unknown attr is unspecified: {stdout}"
    );
}

#[test]
fn stdin_reads_pathnames() {
    let repo = setup_repo();
    let out = run_libra_command_with_stdin(
        &["check-attr", "filter", "--stdin"],
        repo.path(),
        "a.bin\na.txt\n",
    );
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        stdout_of(&out),
        "a.bin: filter: lfs\na.txt: filter: unspecified\n"
    );
}

#[test]
fn z_uses_nul_delimiters() {
    let repo = setup_repo();
    let out = run_libra_command(&["check-attr", "-z", "filter", "a.bin"], repo.path());
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout_of(&out), "a.bin\0filter\0lfs\0");
}

#[test]
fn json_output_reports_each_pair() {
    let repo = setup_repo();
    let out = run_libra_command(
        &["--json", "check-attr", "filter", "--", "a.bin", "a.txt"],
        repo.path(),
    );
    assert_eq!(out.status.code(), Some(0));
    let json = parse_json_stdout(&out);
    let results = json["data"]["results"].as_array().expect("results array");
    assert_eq!(results.len(), 2);

    let bin = results
        .iter()
        .find(|r| r["path"] == "a.bin")
        .expect("a.bin");
    assert_eq!(bin["attr"], "filter");
    assert_eq!(bin["value"], "lfs");

    let txt = results
        .iter()
        .find(|r| r["path"] == "a.txt")
        .expect("a.txt");
    assert_eq!(txt["value"], "unspecified");
}

#[test]
fn missing_attribute_is_a_usage_error() {
    let repo = setup_repo();
    // A single positional with no `--`, `--all`, or `--stdin` cannot be split
    // into an attribute and a path -> fatal usage error (exit 128).
    let out = run_libra_command(&["check-attr", "filter"], repo.path());
    assert_eq!(
        out.status.code(),
        Some(128),
        "need an attribute and a pathname: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn outside_repository_is_an_error() {
    let dir = tempdir().expect("tempdir");
    let out = run_libra_command(&["check-attr", "filter", "a.bin"], dir.path());
    assert_eq!(
        out.status.code(),
        Some(128),
        "outside a repository is fatal: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn path_outside_worktree_is_unspecified_not_a_panic() {
    let repo = setup_repo();
    // `../escape.bin` matches the `*.bin` rule lexically, but it escapes the
    // worktree — it must report `unspecified` (not `lfs`) and must NOT panic
    // (the LFS path relativization would otherwise trip).
    let out = run_libra_command(&["check-attr", "filter", "../escape.bin"], repo.path());
    assert_eq!(
        out.status.code(),
        Some(0),
        "out-of-worktree path is not fatal here"
    );
    assert_eq!(stdout_of(&out), "../escape.bin: filter: unspecified\n");
}
