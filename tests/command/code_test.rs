//! Integration coverage for `libra code` CLI compatibility errors.
//!
//! Spawns the real `libra` binary built by `cargo test` (see `run_libra_command`
//! in `tests/command/mod.rs`, which clears `env_clear()` and points `HOME` at
//! a tempdir) and asserts the structured CLI error report (`error_code` /
//! `hints` / `message`) matches the documented contract for each scenario.
//!
//! Two groups of scenarios:
//!
//! 1. **Removed `claudecode` migration hints** (pre-existing) — exercised at
//!    clap-parse time before any repository preflight, because `claudecode`
//!    is no longer a valid `CodeProvider` variant and the flag-style session
//!    args were removed in the [Claude-Code → Codex migration][1]. Both tests
//!    run in a bare tempdir without `libra init` because clap parsing fails
//!    first.
//!
//! 2. **DeepSeek provider-flag scoping** (added by CEX-00) — exercises the
//!    `validate_mode_args` rules in `src/command/code.rs`, which fire **after**
//!    repository preflight (`command_preflight_storage` runs first). Both
//!    DeepSeek tests therefore call `init_repo_via_cli` to create a `.libra`
//!    so the request reaches the flag-scoping check; otherwise they would
//!    short-circuit on `LBR-REPO-001`.
//!
//! [1]: see commit `6e9d752 feat(claudecode): migrate managed runtime` for the
//! rationale; the migration removed the `claudecode` provider entirely.
//!
//! **Layer:** L2 — invokes the real binary as a subprocess. The DeepSeek
//! scenarios deliberately stop *before* any provider HTTP call by relying on
//! the fact that `DEEPSEEK_API_KEY` is unset (`env_clear()`), so they remain
//! hermetic and CI-safe.

use tempfile::tempdir;

use super::{init_repo_via_cli, parse_cli_error_stderr, run_libra_command};

/// Scenario: invoke `libra code --provider claudecode` in a bare tempdir
/// (no `.libra/`) and assert clap rejects the unknown `CodeProvider` variant
/// at parse time, surfacing the documented `LBR-CLI-002` error code with a
/// hint that explains the removal and points at the migration path
/// (`--provider codex` for managed runtime, `--provider anthropic` for direct
/// Anthropic chat completions).
///
/// Acts as the migration safety net: any reintroduction of the `claudecode`
/// provider (or accidental loss of the migration hint when `classify_parse_
/// error` is refactored) flips this test red.
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

/// Scenario: invoke `libra code --provider codex --resume-session <uuid>`
/// (a flag spelling that only existed under the old `claudecode` provider)
/// and assert clap surfaces `LBR-CLI-002` with the documented "Claude Code
/// provider-session flags were removed" migration hint.
///
/// Acts as the regression net for the Claude-Code → Codex flag migration:
/// reintroducing `--resume-session` or losing its migration hint flips this
/// test red. Like the provider-rejection scenario above, this fires at
/// clap-parse time so it doesn't need a `.libra/` repository.
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

/// Scenario: in a fresh tempdir initialized via `libra init`, invoke
/// `libra code --provider deepseek --deepseek-thinking enabled
/// --deepseek-reasoning-effort xhigh --deepseek-stream false` and assert the
/// command reaches the DeepSeek auth bootstrap step (`LBR-AUTH-001`) — i.e.
/// flag validation passed, env file resolution ran, and the only thing
/// missing is `DEEPSEEK_API_KEY`.
///
/// The negative assertion (`!rendered.contains("only supported with
/// --provider=deepseek")`) confirms the runtime did *not* mistakenly reject
/// the DeepSeek-only flags as cross-provider misuse. Together the two
/// assertions pin "DeepSeek flags pass `validate_mode_args` when paired
/// with `--provider=deepseek`".
///
/// Note on environment: `run_libra_command` calls `env_clear()` and sets
/// `HOME` / `XDG_CONFIG_HOME` to a tempdir, so any host-side
/// `DEEPSEEK_API_KEY` cannot leak in and short-circuit the auth path. The
/// `init_repo_via_cli` call is required because `command_preflight_storage`
/// runs before `validate_mode_args`; without `.libra/` the test would
/// short-circuit on `LBR-REPO-001`.
///
/// Acts as the CEX-00 baseline pin for the DeepSeek smoke contract: any
/// future Step 1.x change that breaks "DeepSeek flag → auth bootstrap" or
/// renames the auth error code flips this test red.
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

/// Scenario: in a fresh tempdir initialized via `libra init`, invoke
/// `libra code --provider openai --deepseek-thinking enabled` (DeepSeek-only
/// flag with a non-DeepSeek provider) and assert the rendered error contains
/// the documented rejection text `--deepseek-thinking is only supported with
/// --provider=deepseek`.
///
/// Pairs with `code_accepts_deepseek_provider_flags_until_auth_bootstrap`:
/// together they prove the cross-provider flag scoping works in both
/// directions — DeepSeek flags pass when paired with the right provider, and
/// fail with a precise message when paired with the wrong one. Same
/// `init_repo_via_cli` requirement applies because flag scoping is enforced
/// after repository preflight.
///
/// Acts as the regression pin for `validate_mode_args` flag-scope logic in
/// `src/command/code.rs`; renaming the error string requires updating this
/// assertion in lockstep.
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
