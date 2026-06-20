//! Integration tests for the `libra automation --help` surface.
//!
//! **Layer:** L1 — deterministic, no external dependencies. Covers
//! the cross-cutting `--help` EXAMPLES rollout from
//! `docs/development/commands/_general.md` item B for the rule-based automation
//! runtime command.

use super::*;

/// `libra automation --help` surfaces the EXAMPLES banner so users see
/// the canonical invocation per sub-command (`list` / `run` /
/// `history`), a named-rule run, a simulated-time run, the `--live`
/// opt-in for actually spawning shell actions, and the JSON variants
/// without reading the design doc.
#[test]
fn test_automation_help_lists_examples_banner() {
    let repo = tempdir().expect("tempdir for automation --help");
    let output = run_libra_command(&["automation", "--help"], repo.path());
    assert!(
        output.status.success(),
        "automation --help should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("EXAMPLES:"),
        "automation --help should include EXAMPLES banner, stdout: {stdout}"
    );
    for invocation in [
        "libra automation list",
        "libra automation run",
        "libra automation run --rule my-rule",
        "libra automation run --now",
        "libra automation run --live",
        "libra automation history --limit 50",
        "libra automation --json list",
        "libra automation --json run",
    ] {
        assert!(
            stdout.contains(invocation),
            "automation --help should include `{invocation}`, stdout: {stdout}"
        );
    }
}
