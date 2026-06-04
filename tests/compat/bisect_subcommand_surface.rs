//! `tests/compat/bisect_subcommand_surface.rs` — surface contract for
//! `libra bisect run` / `libra bisect view` (C4 plan).
//!
//! Per-handler tests live in `tests/command/bisect_test.rs`. This file pins
//! only the contract guaranteed by [`COMPATIBILITY.md`](../../COMPATIBILITY.md):
//!
//! - `libra bisect --help` lists `start` / `bad` / `good` / `reset` / `skip`
//!   / `log` / `run` / `view`.
//! - The EXAMPLES banner is emitted (proves `BISECT_EXAMPLES` is wired).

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
fn bisect_help_lists_full_subcommand_surface() {
    let output = run(&["bisect", "--help"]);
    assert!(
        output.status.success(),
        "bisect --help should succeed; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    for sub in [
        "start", "bad", "good", "reset", "skip", "log", "run", "view",
    ] {
        assert!(
            stdout.contains(sub),
            "bisect --help must list `{sub}`; stdout: {stdout}"
        );
    }
    assert!(
        stdout.contains("EXAMPLES:"),
        "bisect --help must include EXAMPLES banner; stdout: {stdout}"
    );
}

/// `bisect terms` / `bisect replay` are declined (no `cli::Bisect` variant):
/// clap rejects them as unrecognized subcommands, mapped to `LBR-CLI-002`
/// (default coarse exit 129). `--term-old` / `--term-new` are likewise rejected
/// as unexpected arguments. Pins the declined surface documented in
/// `docs/improvement/compatibility/declined.md` (D6/D7) so a future enum
/// addition cannot silently turn them into "recognized but refused".
#[test]
fn bisect_declined_subcommands_rejected() {
    for sub in ["terms", "replay"] {
        let output = run(&["bisect", sub]);
        assert_eq!(
            output.status.code(),
            Some(129),
            "`bisect {sub}` should exit 129 (declined); got {:?}",
            output.status.code()
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("unrecognized subcommand"),
            "`bisect {sub}` stderr must say 'unrecognized subcommand'; got: {stderr}"
        );
        assert!(
            stderr.contains("LBR-CLI-002"),
            "`bisect {sub}` stderr must carry LBR-CLI-002; got: {stderr}"
        );
    }

    for flag in ["--term-old", "--term-new"] {
        let output = run(&["bisect", "start", flag, "foo"]);
        assert_eq!(
            output.status.code(),
            Some(129),
            "`bisect start {flag}` should exit 129 (declined); got {:?}",
            output.status.code()
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("unexpected argument"),
            "`bisect start {flag}` stderr must say 'unexpected argument'; got: {stderr}"
        );
        assert!(
            stderr.contains("LBR-CLI-002"),
            "`bisect start {flag}` stderr must carry LBR-CLI-002; got: {stderr}"
        );
    }
}
