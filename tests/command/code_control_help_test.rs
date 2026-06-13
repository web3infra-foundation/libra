//! Integration tests for the `libra code-control --help` surface.
//!
//! **Layer:** L1 — deterministic, no external dependencies. Covers
//! the cross-cutting `--help` EXAMPLES rollout from
//! `docs/development/commands/_general.md` item B for the local Libra Code TUI
//! automation control JSON-RPC shim.

use super::*;

/// `libra code-control --help` surfaces the EXAMPLES banner so users
/// see the canonical `--stdio` form, how to wire it to the discovery
/// file emitted by `libra code --control write`, and a piped
/// JSON-RPC example without reading the design doc.
#[test]
fn test_code_control_help_lists_examples_banner() {
    let repo = tempdir().expect("tempdir for code-control --help");
    let output = run_libra_command(&["code-control", "--help"], repo.path());
    assert!(
        output.status.success(),
        "code-control --help should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("EXAMPLES:"),
        "code-control --help should include EXAMPLES banner, stdout: {stdout}"
    );
    for invocation in [
        "libra code-control --stdio --url",
        ".libra/code/control.json",
        "echo '{\"jsonrpc\":\"2.0\"",
    ] {
        assert!(
            stdout.contains(invocation),
            "code-control --help should include `{invocation}`, stdout: {stdout}"
        );
    }
}
