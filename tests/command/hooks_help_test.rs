//! Integration tests for the `libra hooks --help` surface.
//!
//! **Layer:** L1 — deterministic, no external dependencies. Covers
//! the cross-cutting `--help` EXAMPLES rollout from
//! `docs/improvement/README.md` item B for the AI agent hook entry
//! point command.

use super::*;

/// `libra hooks --help` surfaces the EXAMPLES banner so operators see
/// the most commonly wired Claude / Gemini lifecycle events
/// (session-start, prompt, tool-use, stop, session-end) without
/// reading the design doc.
#[test]
fn test_hooks_help_lists_examples_banner() {
    let repo = tempdir().expect("tempdir for hooks --help");
    let output = run_libra_command(&["hooks", "--help"], repo.path());
    assert!(
        output.status.success(),
        "hooks --help should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("EXAMPLES:"),
        "hooks --help should include EXAMPLES banner, stdout: {stdout}"
    );
    for invocation in [
        "libra hooks claude session-start",
        "libra hooks claude prompt",
        "libra hooks claude tool-use",
        "libra hooks claude stop",
        "libra hooks claude session-end",
        "libra hooks gemini session-start",
        "libra hooks gemini prompt",
        "libra hooks gemini tool-use",
    ] {
        assert!(
            stdout.contains(invocation),
            "hooks --help should include `{invocation}`, stdout: {stdout}"
        );
    }
}
