//! CEX-S2-16 / S2-INV-13 PreToolUse hook fail-closed dispatch fixtures.
//!
//! Spec: `docs/development/commands/agent.md` Step 2.2 hook exit-code 权威映射表. The
//! security-critical property is **fail-closed**: only an exit-0 hook allows the
//! tool call; every other terminal condition (explicit block, unknown exit code,
//! timeout, OS-signal kill, spawn failure) must NOT allow it.
//!
//! These fixtures run *real* hook scripts through the production
//! [`HookRunner::run_pre_tool_use`] path (the same method `tool_loop.rs` calls
//! before every tool dispatch), so a regression that re-opens the fail-open hole
//! surfaces here rather than only in the pure classifier unit tests.
//!
//! The pure exit-code → decision mapping is additionally pinned by
//! `internal::ai::agent_run::hook_dispatch` unit tests; this file is the
//! end-to-end counterpart over the actual subprocess runner.

use libra::internal::ai::hooks::{HookAction, HookConfig, HookDefinition, HookEvent, HookRunner};
use tempfile::TempDir;

/// Build a `HookRunner` with a single PreToolUse hook running `command`,
/// matching every tool (`*`).
fn runner_with_hook(command: &str, timeout_ms: u64) -> (HookRunner, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let config = HookConfig {
        hooks: vec![HookDefinition {
            event: HookEvent::PreToolUse,
            matcher: "*".to_string(),
            command: command.to_string(),
            description: "fixture hook".to_string(),
            timeout_ms,
            enabled: true,
        }],
    };
    let runner = HookRunner::new(config, tmp.path().to_path_buf());
    (runner, tmp)
}

async fn run(command: &str, timeout_ms: u64) -> HookAction {
    let (runner, _tmp) = runner_with_hook(command, timeout_ms);
    runner
        .run_pre_tool_use("read_file", serde_json::json!({}))
        .await
}

// ---- Fixture 1: exit 0 → allow -------------------------------------------

#[tokio::test]
async fn fixture_exit_zero_allows() {
    assert_eq!(run("exit 0", 5000).await, HookAction::Allow);
}

// ---- Fixture 2: exit 0 with empty stdout → allow -------------------------

#[tokio::test]
async fn fixture_exit_zero_empty_stdout_allows() {
    // `true` exits 0 and prints nothing.
    assert_eq!(run("true", 5000).await, HookAction::Allow);
}

// ---- Fixture 3: explicit block (exit 129) → block ------------------------

#[tokio::test]
async fn fixture_explicit_block_blocks() {
    let action = run("echo 'denied by policy'; exit 129", 5000).await;
    assert!(action.is_blocked(), "exit 129 must block, got: {action:?}");
    if let HookAction::Block(reason) = action {
        assert!(
            reason.contains("denied by policy"),
            "block reason should carry the hook's stdout: {reason}",
        );
    }
}

// ---- Fixture 4: exit 1 (unknown) → fail-closed block ---------------------

#[tokio::test]
async fn fixture_unknown_exit_one_fails_closed() {
    let action = run("exit 1", 5000).await;
    assert!(
        action.is_blocked(),
        "unknown exit code 1 must fail closed, got: {action:?}",
    );
}

// ---- Fixture 5: exit 2 (documented deny code) → block with reason -------

#[tokio::test]
async fn fixture_exit_two_blocks_with_reason() {
    // Exit 2 is the documented S2-INV-13 deny code (agent.md Step 2.2): it
    // blocks the tool call and surfaces the hook's stdout as the reason.
    let action = run("echo 'policy denied'; exit 2", 5000).await;
    assert!(action.is_blocked(), "exit 2 must block, got: {action:?}",);
    if let HookAction::Block(reason) = &action {
        assert!(
            reason.contains("policy denied"),
            "exit 2 must carry the hook stdout reason: {reason}",
        );
    }
}

// ---- Fixture 6: large exit code (> 3) → fail-closed block ----------------

#[tokio::test]
async fn fixture_large_exit_code_fails_closed() {
    let action = run("exit 42", 5000).await;
    assert!(
        action.is_blocked(),
        "exit 42 must fail closed, got: {action:?}",
    );
}

// ---- Fixture 7: timeout → fail-closed block ------------------------------

#[tokio::test]
async fn fixture_timeout_fails_closed() {
    // Sleeps far longer than the 100ms timeout; the runner kills it and the
    // failure must block.
    let action = run("sleep 10", 100).await;
    assert!(
        action.is_blocked(),
        "a timed-out hook must fail closed, got: {action:?}",
    );
}

// ---- Fixture 8: killed by signal → fail-closed block --------------------

#[tokio::test]
async fn fixture_killed_by_signal_fails_closed() {
    // The hook kills itself with SIGKILL (no exit code). A signal death is a
    // failure and must block.
    let action = run("kill -9 $$", 5000).await;
    assert!(
        action.is_blocked(),
        "a signal-killed hook must fail closed, got: {action:?}",
    );
}

// ---- Fixture 9: spawn failure (non-existent binary) → fail-closed block --

#[tokio::test]
async fn fixture_spawn_enoent_fails_closed() {
    // `sh -c` runs the command; an unknown binary makes the *command* exit
    // non-zero (127), which is an unknown exit code and must fail closed.
    let action = run("this-binary-does-not-exist-12345", 5000).await;
    assert!(
        action.is_blocked(),
        "a spawn/exec failure must fail closed, got: {action:?}",
    );
}

// ---- Fixture 10: non-executable target → fail-closed block ---------------

#[tokio::test]
async fn fixture_non_executable_fails_closed() {
    // Attempt to execute a path that exists but is not executable: `sh -c`
    // reports a non-zero exit, which must fail closed.
    let action = run("/etc/hosts", 5000).await;
    assert!(
        action.is_blocked(),
        "executing a non-executable target must fail closed, got: {action:?}",
    );
}

// ---- Fixture 11: no matching hook → allow (not a failure) ----------------

#[tokio::test]
async fn fixture_no_matching_hook_allows() {
    let tmp = TempDir::new().expect("tempdir");
    let config = HookConfig {
        hooks: vec![HookDefinition {
            event: HookEvent::PreToolUse,
            // Only matches the `shell` tool; we call `read_file`.
            matcher: "shell".to_string(),
            command: "exit 1".to_string(),
            description: "non-matching hook".to_string(),
            timeout_ms: 5000,
            enabled: true,
        }],
    };
    let runner = HookRunner::new(config, tmp.path().to_path_buf());
    let action = runner
        .run_pre_tool_use("read_file", serde_json::json!({}))
        .await;
    assert_eq!(
        action,
        HookAction::Allow,
        "a hook that does not match the tool must not block it",
    );
}

// ---- Fixture 12: disabled hook → allow (not a failure) -------------------

#[tokio::test]
async fn fixture_disabled_hook_allows() {
    let tmp = TempDir::new().expect("tempdir");
    let config = HookConfig {
        hooks: vec![HookDefinition {
            event: HookEvent::PreToolUse,
            matcher: "*".to_string(),
            command: "exit 1".to_string(),
            description: "disabled hook".to_string(),
            timeout_ms: 5000,
            enabled: false,
        }],
    };
    let runner = HookRunner::new(config, tmp.path().to_path_buf());
    let action = runner
        .run_pre_tool_use("read_file", serde_json::json!({}))
        .await;
    assert_eq!(
        action,
        HookAction::Allow,
        "a disabled hook must be skipped, not run",
    );
}

/// Cross-cutting: the ONLY way a configured, matching, enabled PreToolUse hook
/// permits the tool call is exit 0. Every failure mode blocks. This is the
/// fail-closed invariant in one assertion sweep.
#[tokio::test]
async fn only_exit_zero_permits_through_real_runner() {
    assert_eq!(run("exit 0", 5000).await, HookAction::Allow);
    for command in [
        "exit 1",
        "exit 2",
        "exit 3",
        "exit 42",
        "exit 255",
        "kill -9 $$",
        "this-binary-does-not-exist-12345",
    ] {
        assert!(
            run(command, 5000).await.is_blocked(),
            "`{command}` must not permit the tool call (fail-closed)",
        );
    }
    // Timeout is also a failure → block.
    assert!(run("sleep 10", 100).await.is_blocked());
}
