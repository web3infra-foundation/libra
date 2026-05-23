//! Integration tests for the `libra agent --help` surface.
//!
//! **Layer:** L1 — deterministic, no external dependencies. Covers
//! the cross-cutting `--help` EXAMPLES rollout from
//! `docs/improvement/README.md` item B for the external Agent capture
//! pipeline operator surface.

use super::*;

/// `libra agent --help` surfaces the EXAMPLES banner so operators see
/// the canonical invocation per visible sub-command (status, enable,
/// disable, session, checkpoint, clean, doctor, push, rpc) plus the
/// `--all` clean form, the `--remote` push form, and the JSON variant
/// without reading the design doc.
#[test]
fn test_agent_help_lists_examples_banner() {
    let repo = tempdir().expect("tempdir for agent --help");
    let output = run_libra_command(&["agent", "--help"], repo.path());
    assert!(
        output.status.success(),
        "agent --help should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("EXAMPLES:"),
        "agent --help should include EXAMPLES banner, stdout: {stdout}"
    );
    for invocation in [
        "libra agent status",
        "libra agent enable --agent claude",
        "libra agent disable --agent claude",
        "libra agent session list",
        "libra agent checkpoint list",
        "libra agent checkpoint show <id>",
        "libra agent checkpoint rewind <id>",
        "libra agent clean",
        "libra agent clean --all",
        "libra agent doctor",
        "libra agent push",
        "libra agent push --remote origin",
        "libra agent rpc list",
        "libra agent rpc invoke",
        "libra agent --json status",
    ] {
        assert!(
            stdout.contains(invocation),
            "agent --help should include `{invocation}`, stdout: {stdout}"
        );
    }
}
