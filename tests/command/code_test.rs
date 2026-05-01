//! Integration coverage for `libra code` CLI compatibility errors.

use tempfile::tempdir;

use super::{init_repo_via_cli, parse_cli_error_stderr, run_libra_command};

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

#[test]
fn code_accepts_deepseek_provider_flags_until_auth_bootstrap() {
    let repo = tempdir().expect("failed to create temporary directory");
    init_repo_via_cli(repo.path());

    let output = run_libra_command(
        &[
            "code",
            "--provider",
            "deepseek",
            "--deepseek-thinking",
            "enabled",
            "--deepseek-reasoning-effort",
            "xhigh",
            "--deepseek-stream",
            "false",
        ],
        repo.path(),
    );

    assert!(!output.status.success());
    let (human, report) = parse_cli_error_stderr(&output.stderr);
    let rendered = format!("{human}\n{}", report.message);
    assert_eq!(
        report.error_code, "LBR-AUTH-001",
        "expected auth-missing error code when DEEPSEEK_API_KEY is unset, got {} ({rendered})",
        report.error_code
    );
    assert!(
        rendered.contains("DEEPSEEK_API_KEY"),
        "expected DeepSeek auth bootstrap error, got {rendered}"
    );
    assert!(
        !rendered.contains("only supported with --provider=deepseek"),
        "DeepSeek-specific flags should be accepted for the DeepSeek provider: {rendered}"
    );
}

#[test]
fn code_rejects_deepseek_flags_for_other_providers() {
    let repo = tempdir().expect("failed to create temporary directory");
    init_repo_via_cli(repo.path());

    let output = run_libra_command(
        &[
            "code",
            "--provider",
            "openai",
            "--deepseek-thinking",
            "enabled",
        ],
        repo.path(),
    );

    assert!(!output.status.success());
    let (human, report) = parse_cli_error_stderr(&output.stderr);
    let rendered = format!("{human}\n{}", report.message);
    assert!(
        rendered.contains("--deepseek-thinking is only supported with --provider=deepseek"),
        "expected provider-scoped flag error, got {rendered}"
    );
}
