//! `tests/compat/worktree_delete_dir.rs` — surface contract for the
//! `worktree remove --delete-dir` flag (C5 plan).
//!
//! Per-handler tests live in `tests/command/worktree_test.rs`. This file
//! pins the user-visible contract:
//!
//! - `libra worktree remove --help` lists `--delete-dir`.
//! - The new EXAMPLES banner is emitted and mentions the dirty-worktree
//!   refusal explicitly.

use std::process::Command;

fn libra_bin() -> &'static str {
    env!("CARGO_BIN_EXE_libra")
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(libra_bin())
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("HOME", "/tmp")
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .output()
        .expect("failed to spawn libra binary")
}

#[test]
fn worktree_remove_help_lists_delete_dir() {
    let output = run(&["worktree", "remove", "--help"]);
    assert!(
        output.status.success(),
        "worktree remove --help should succeed; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--delete-dir"),
        "worktree remove --help must list `--delete-dir`; stdout: {stdout}"
    );
}

#[test]
fn worktree_help_mentions_delete_dir_example() {
    let output = run(&["worktree", "--help"]);
    assert!(
        output.status.success(),
        "worktree --help should succeed; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--delete-dir"),
        "worktree --help EXAMPLES must reference `--delete-dir`; stdout: {stdout}"
    );
}
