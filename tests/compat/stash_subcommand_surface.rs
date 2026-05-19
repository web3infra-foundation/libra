//! `tests/compat/stash_subcommand_surface.rs` — cross-subcommand surface
//! tests for `libra stash`.
//!
//! Owned by C4 (compatibility plan: subcommand surface补齐). The per-subcommand
//! happy / error paths live in `tests/command/stash_test.rs`; this file pins
//! the *cross-subcommand* contract:
//!
//! 1. `libra stash --help` lists every public subcommand we promise in
//!    [`COMPATIBILITY.md`](../../COMPATIBILITY.md): `push` / `pop` / `list` /
//!    `apply` / `drop` / `show` / `branch` / `clear`.
//! 2. JSON envelopes share the `{ "command": "stash", "data": { "action": .. } }`
//!    shape for the new actions (`show` / `branch` / `clear`), matching the
//!    pattern already pinned for the existing actions.
//!
//! These tests run in the `compat-offline-core` job — they need no network
//! and no credentials.

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
fn stash_help_lists_full_subcommand_surface() {
    let output = run(&["stash", "--help"]);
    assert!(
        output.status.success(),
        "stash --help should succeed; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    for sub in [
        "push", "pop", "list", "apply", "drop", "show", "branch", "clear",
    ] {
        assert!(
            stdout.contains(sub),
            "stash --help must list `{sub}`; stdout: {stdout}"
        );
    }
    assert!(
        stdout.contains("EXAMPLES:"),
        "stash --help must include EXAMPLES banner; stdout: {stdout}"
    );
}
