//! Integration coverage for `libra code` CLI compatibility errors.

use tempfile::tempdir;

use super::{parse_cli_error_stderr, run_libra_command};

#[test]
fn code_rejects_removed_claudecode_provider_with_migration_hint() {
    let repo = tempdir().expect("failed to create temporary directory");

    let output = run_libra_command(&["code", "--provider", "claudecode"], repo.path());

    assert!(!output.status.success());
    let (_stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert!(
        report
            .hints
            .iter()
            .any(|hint| hint.contains("`libra code --provider claudecode` was removed")),
        "expected removed-provider hint, got {:?}",
        report.hints
    );
}

#[test]
fn code_rejects_removed_claudecode_session_flags_with_migration_hint() {
    let repo = tempdir().expect("failed to create temporary directory");

    let output = run_libra_command(
        &[
            "code",
            "--provider",
            "codex",
            "--resume-session",
            "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        ],
        repo.path(),
    );

    assert!(!output.status.success());
    let (_stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert!(
        report
            .hints
            .iter()
            .any(|hint| hint.contains("Claude Code provider-session flags were removed")),
        "expected removed-session-flag hint, got {:?}",
        report.hints
    );
}
