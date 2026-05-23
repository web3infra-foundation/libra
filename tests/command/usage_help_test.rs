//! Integration tests for the `libra usage` command surface.
//!
//! **Layer:** L1 — deterministic, no external dependencies. Covers the
//! cross-cutting `--help` EXAMPLES rollout from
//! `docs/improvement/README.md` item B for the AI usage reporting
//! command.

use super::*;

/// `libra usage --help` surfaces the EXAMPLES banner so users see the
/// canonical invocation per sub-command (`report` / `prune`) plus
/// common filter combinations (`--since`, `--session`, `--thread`,
/// `--include-failed`, csv format, `--retention-days`) without having
/// to read the design doc.
#[test]
fn test_usage_help_lists_examples_banner() {
    let repo = tempdir().expect("tempdir for usage --help");
    let output = run_libra_command(&["usage", "--help"], repo.path());
    assert!(
        output.status.success(),
        "usage --help should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("EXAMPLES:"),
        "usage --help should include EXAMPLES banner, stdout: {stdout}"
    );
    for invocation in [
        "libra usage report",
        "libra usage report --since 24h",
        "libra usage report --since 7d --include-failed",
        "libra usage report --session",
        "libra usage report --thread",
        "libra usage report --format csv",
        "libra usage --json report",
        "libra usage prune",
        "libra usage prune --retention-days 30",
    ] {
        assert!(
            stdout.contains(invocation),
            "usage --help should include `{invocation}`, stdout: {stdout}"
        );
    }
}
