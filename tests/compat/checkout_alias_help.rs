//! `tests/compat/checkout_alias_help.rs` — surface contract for un-hiding
//! `libra checkout` (C5 plan).
//!
//! Per-handler tests live in `tests/command/checkout_test.rs`. This file
//! pins the user-visible contract:
//!
//! - The top-level `libra --help` lists `checkout` (no longer hidden).
//! - `libra checkout --help` includes the migration banner directing users
//!   to `switch` for branch navigation and `restore` for file restoration.

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
fn top_level_help_lists_checkout() {
    let output = run(&["--help"]);
    assert!(
        output.status.success(),
        "libra --help should succeed; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("checkout"),
        "libra --help must list `checkout` (un-hidden in C5); stdout: {stdout}"
    );
}

#[test]
fn checkout_help_recommends_switch_and_restore() {
    let output = run(&["checkout", "--help"]);
    assert!(
        output.status.success(),
        "checkout --help should succeed; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("switch"),
        "checkout --help must recommend `libra switch`; stdout: {stdout}"
    );
    assert!(
        stdout.contains("restore"),
        "checkout --help must recommend `libra restore`; stdout: {stdout}"
    );
}
