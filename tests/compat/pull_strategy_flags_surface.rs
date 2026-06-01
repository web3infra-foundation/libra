//! `tests/compat/pull_strategy_flags_surface.rs` — surface contract for the
//! `libra pull` merge-strategy flags.
//!
//! Per-handler behaviour lives in `tests/command/pull_test.rs`. This file
//! pins the user-visible flag matrix so the `COMPATIBILITY.md` `pull` row
//! cannot silently drift away from what `pull --help` actually exposes:
//!
//! - `--ff-only` and `--rebase` (`-r`) are implemented and MUST appear in
//!   `pull --help` (they were misreported as "not exposed" before v0.17.1215).
//! - `--squash` is genuinely deferred, so it MUST NOT appear — if a future
//!   change adds it, this guard flips red and forces a `COMPATIBILITY.md`
//!   update in the same PR.

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
fn pull_help_exposes_ff_only_and_rebase() {
    let output = run(&["pull", "--help"]);
    assert!(
        output.status.success(),
        "pull --help should succeed; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--ff-only"),
        "pull --help must expose `--ff-only`; stdout: {stdout}"
    );
    assert!(
        stdout.contains("--rebase"),
        "pull --help must expose `--rebase`; stdout: {stdout}"
    );
    assert!(
        stdout.contains("-r"),
        "pull --help must expose the `-r` short alias for `--rebase`; stdout: {stdout}"
    );
}

#[test]
fn pull_help_omits_unimplemented_squash() {
    let output = run(&["pull", "--help"]);
    assert!(
        output.status.success(),
        "pull --help should succeed; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("--squash"),
        "pull --help must NOT advertise the deferred `--squash` flag; if it was \
         implemented, update COMPATIBILITY.md and this guard. stdout: {stdout}"
    );
}

#[test]
fn compatibility_matrix_pull_row_matches_implemented_flags() {
    let repo = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let matrix =
        std::fs::read_to_string(repo.join("COMPATIBILITY.md")).expect("read COMPATIBILITY.md");
    let pull_row = matrix
        .lines()
        .find(|line| line.starts_with("| pull "))
        .expect("COMPATIBILITY.md must carry a `pull` row");
    assert!(
        pull_row.contains("`--ff-only`") && pull_row.contains("`--rebase`"),
        "COMPATIBILITY.md pull row must record that `--ff-only` / `--rebase` are exposed \
         (they are real clap flags), not that they are unexposed; row: {pull_row}"
    );
    assert!(
        !pull_row.contains("no `--ff-only`"),
        "COMPATIBILITY.md pull row must not claim `--ff-only` is unexposed; row: {pull_row}"
    );
}
