//! Wave 2 / PR 2 — `libra code` CLI dispatch L1 tests.
//!
//! Per `docs/development/commands/_general.md` §5.1, Wave 2's CLI surface must
//! cover mode selection, mutual exclusion, and parser smoke without
//! ever spawning the binary. We assert directly against
//! `clap::Parser::try_parse_from` so a bad flag combination fails
//! at parse time, before the runtime starts.
//!
//! What this file covers (P0 set):
//!
//! * `--web --stdio` mutual exclusion error.
//! * `--web` (alias for `--web-only`) parses to the same flag.
//! * `--mcp-port 0` is accepted (kernel-assigned port, used by the
//!   PTY harness).
//! * `--port 0` likewise.
//! * `--env-file` is parsed into the right field.
//! * `--repo`, `--cwd`, `--resume` pass through as `Some(...)`.
//! * `--browser-control loopback` together with `--stdio` is
//!   rejected (clap conflicts_with).
//!
//! What this file does NOT cover (deferred per the plan):
//!
//! * Provider boot smoke (Wave 10 / PR 10).
//! * `--plan-mode` default per provider — already covered by
//!   `effective_plan_mode_*` tests inside `src/command/code.rs`.
//! * TUI / MCP / Codex runtime — Waves 9/13.

use std::path::PathBuf;

use clap::Parser;
use libra::command::code::{CodeArgs, CodeNetworkAccess, ControlMode};

/// Helper: parse `argv0 + args` with a fixed binary name. Strip the
/// `--web`/`--stdio` from the spelling caller passed since clap
/// expects the binary name as `argv[0]`.
fn parse(args: &[&str]) -> Result<CodeArgs, clap::Error> {
    let mut full: Vec<String> = vec!["code".to_string()];
    for arg in args {
        full.push((*arg).to_string());
    }
    CodeArgs::try_parse_from(full)
}

#[test]
fn web_only_and_stdio_are_mutually_exclusive() {
    let error = parse(&["--web-only", "--stdio"]).expect_err("clap must reject the combination");
    let rendered = error.to_string();
    // clap formats this as "argument '--stdio' cannot be used with '--web-only'".
    // We assert on both flag names instead of the exact phrasing so a
    // future clap upgrade doesn't break the test.
    assert!(
        rendered.contains("--stdio") && rendered.contains("--web-only"),
        "expected mutual-exclusion error to mention both flags; got: {rendered}",
    );
}

#[test]
fn web_alias_resolves_to_web_only() {
    let parsed = parse(&["--web"]).expect("--web is a documented alias");
    assert!(
        parsed.web_only,
        "--web must set web_only=true (alias for --web-only)",
    );
    assert!(!parsed.stdio, "--web must NOT enable stdio mode");
}

#[test]
fn web_and_stdio_are_mutually_exclusive_via_alias() {
    // Same conflict but exercised through the `--web` alias.
    let error = parse(&["--web", "--stdio"]).expect_err("alias must inherit conflicts_with");
    let rendered = error.to_string();
    assert!(
        rendered.contains("--stdio"),
        "expected --stdio in error: {rendered}"
    );
}

#[test]
fn mcp_port_zero_is_accepted() {
    let parsed = parse(&["--mcp-port", "0"]).expect("--mcp-port 0 is the kernel-pick sentinel");
    assert_eq!(parsed.mcp_port, 0);
}

#[test]
fn web_port_zero_is_accepted() {
    let parsed = parse(&["--port", "0"]).expect("--port 0 is the kernel-pick sentinel");
    assert_eq!(parsed.port, 0);
}

#[test]
fn env_file_parses_into_pathbuf() {
    let parsed = parse(&["--env-file", "/tmp/.env.test"]).expect(".env paths are valid input");
    assert_eq!(parsed.env_file, Some(PathBuf::from("/tmp/.env.test")));
}

#[test]
fn repo_and_cwd_and_resume_are_optional() {
    let bare = parse(&[]).expect("CodeArgs has no required positional args");
    assert!(bare.repo.is_none());
    assert!(bare.cwd.is_none());
    assert!(bare.resume.is_none());

    let with_paths = parse(&[
        "--repo",
        "/tmp/some-repo",
        "--cwd",
        "/tmp/some-cwd",
        "--resume",
        "thread-2026-05-10-001",
    ])
    .expect("--repo / --cwd / --resume are optional but well-typed");
    assert_eq!(with_paths.repo, Some(PathBuf::from("/tmp/some-repo")));
    assert_eq!(with_paths.cwd, Some(PathBuf::from("/tmp/some-cwd")));
    assert_eq!(with_paths.resume.as_deref(), Some("thread-2026-05-10-001"));
}

#[test]
fn browser_control_loopback_conflicts_with_stdio() {
    // `--browser-control loopback` is incompatible with `--stdio`
    // because the stdio MCP server has no HTTP surface for a
    // browser to attach to. clap's conflicts_with should reject.
    let error = parse(&["--browser-control", "loopback", "--stdio"])
        .expect_err("--browser-control + --stdio must be rejected");
    let rendered = error.to_string();
    assert!(
        rendered.contains("--browser-control") && rendered.contains("--stdio"),
        "expected conflict error to mention both flags; got: {rendered}",
    );
}

#[test]
fn defaults_are_observe_control_and_deny_network() {
    let bare = parse(&[]).expect("CodeArgs has no required args");
    // Spot-check that the documented defaults from publish.md /
    // docs/commands/code.md actually flow through.
    // ControlMode::Observe is the safe default (no automation
    // writes); CodeNetworkAccess::Deny is the safe default for
    // shell tools.
    //
    // Codex pass-1 P3: assert via PartialEq on the enum directly
    // instead of `format!("{:?}")` substring matching, which
    // would pass on accidental Debug-impl substring overlap.
    assert_eq!(
        bare.control,
        ControlMode::Observe,
        "control default must be ControlMode::Observe",
    );
    assert_eq!(
        bare.network_access,
        CodeNetworkAccess::Deny,
        "network_access default must be CodeNetworkAccess::Deny",
    );
}
