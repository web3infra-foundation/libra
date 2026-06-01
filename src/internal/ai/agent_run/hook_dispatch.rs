//! Pure PreToolUse hook exit-code classifier (CEX-S2-16 / S2-INV-13, Step 2.2).
//!
//! `docs/improvement/agent.md` Step 2.2 defines the authoritative hook exit-code
//! mapping table. The security-critical property is **fail-closed**: only the
//! three documented outcomes (`0` → allow, `2` → deny, `3` → needs-human) are
//! recognised; *every other* terminal condition — an unknown exit code, a
//! panic, a timeout, an OS-signal kill, or a spawn failure — maps to **deny**,
//! never to "warn but pass". A capability package, third-party MCP source, or
//! sub-agent definition must never be able to turn a hook failure into an allow.
//!
//! This module is the **pure** classifier: it turns a [`HookOutcome`]
//! (whatever the runner observed) into a [`PreToolUseDecision`] without any I/O.
//! Spawning the hook, enforcing the timeout, and writing the resulting
//! `AgentRunEvent` stay in the runner; isolating the mapping here makes the
//! fail-closed table exhaustively unit-testable and gives the runner one
//! authority to call.
//!
//! Scope: this classifies the **PreToolUse** phase, where the decision can still
//! block dispatch. PostToolUse exceptions (dispatch already happened) map to
//! `post_tool_review_required` via [`super::event::PostToolReason`] and are the
//! runner's concern.

use super::event::HookFailureReason;

/// What the runner observed when it ran a PreToolUse hook. The classifier maps
/// each of these to a [`PreToolUseDecision`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HookOutcome {
    /// The hook process exited with this code.
    Exited { code: i32 },
    /// The hook process panicked / aborted with no exit code.
    Panicked,
    /// The hook exceeded its timeout.
    TimedOut,
    /// The hook was killed by an OS signal (no exit code).
    KilledBySignal { signo: i32 },
    /// `execve(2)` returned ENOENT — the hook binary was not found.
    SpawnEnoent,
    /// `execve(2)` returned EACCES — the hook binary was not executable.
    SpawnEacces,
}

/// The PreToolUse decision the classifier reached. `Deny` carries the
/// fail-closed [`HookFailureReason`] so the runner can write the matching
/// `AgentRunEvent::BlockedByHookFailure` (or `BlockedByHook` for the documented
/// exit-2 deny).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreToolUseDecision {
    /// Exit 0: dispatch continues (sandbox / approval still apply downstream).
    Allow,
    /// Exit 2: the hook explicitly denied; dispatch is blocked. Maps to
    /// `AgentRunEvent::BlockedByHook` (with `hook_reason` supplied separately
    /// from the hook's stdout).
    Deny,
    /// Exit 3: the hook requested human approval; dispatch pauses for Layer 1.
    /// Maps to `AgentRunEvent::HookRequestedHuman`.
    NeedsHuman,
    /// Any other terminal condition — fail-closed deny. Carries the reason for
    /// `AgentRunEvent::BlockedByHookFailure`.
    DenyFailClosed { reason: HookFailureReason },
}

impl PreToolUseDecision {
    /// Whether this decision allows the tool call to dispatch. Only [`Allow`]
    /// does; every deny / needs-human / fail-closed outcome does not.
    ///
    /// [`Allow`]: PreToolUseDecision::Allow
    pub fn permits_dispatch(&self) -> bool {
        matches!(self, Self::Allow)
    }
}

/// Classify a PreToolUse [`HookOutcome`] into a [`PreToolUseDecision`],
/// fail-closed per the Step 2.2 authoritative table.
///
/// The only allow path is exit `0`. Exit `2` / `3` are the documented
/// deny / needs-human signals. **Everything else** — including exit `1`,
/// negative codes, codes `> 3`, panics, timeouts, signals and spawn failures —
/// is a fail-closed [`PreToolUseDecision::DenyFailClosed`], never an allow.
pub fn classify_pre_tool_use(outcome: &HookOutcome) -> PreToolUseDecision {
    match *outcome {
        HookOutcome::Exited { code: 0 } => PreToolUseDecision::Allow,
        HookOutcome::Exited { code: 2 } => PreToolUseDecision::Deny,
        HookOutcome::Exited { code: 3 } => PreToolUseDecision::NeedsHuman,
        HookOutcome::Exited { code } => PreToolUseDecision::DenyFailClosed {
            reason: HookFailureReason::UnknownExitCode { exit_code: code },
        },
        HookOutcome::Panicked => PreToolUseDecision::DenyFailClosed {
            reason: HookFailureReason::Panic,
        },
        HookOutcome::TimedOut => PreToolUseDecision::DenyFailClosed {
            reason: HookFailureReason::Timeout,
        },
        HookOutcome::KilledBySignal { signo } => PreToolUseDecision::DenyFailClosed {
            reason: HookFailureReason::KilledBySignal { signo },
        },
        HookOutcome::SpawnEnoent => PreToolUseDecision::DenyFailClosed {
            reason: HookFailureReason::SpawnEnoent,
        },
        HookOutcome::SpawnEacces => PreToolUseDecision::DenyFailClosed {
            reason: HookFailureReason::SpawnEacces,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_zero_allows_dispatch() {
        let decision = classify_pre_tool_use(&HookOutcome::Exited { code: 0 });
        assert_eq!(decision, PreToolUseDecision::Allow);
        assert!(decision.permits_dispatch());
    }

    #[test]
    fn exit_two_denies() {
        let decision = classify_pre_tool_use(&HookOutcome::Exited { code: 2 });
        assert_eq!(decision, PreToolUseDecision::Deny);
        assert!(!decision.permits_dispatch());
    }

    #[test]
    fn exit_three_requests_human() {
        let decision = classify_pre_tool_use(&HookOutcome::Exited { code: 3 });
        assert_eq!(decision, PreToolUseDecision::NeedsHuman);
        assert!(!decision.permits_dispatch());
    }

    /// The fail-closed core: exit 1, negative codes, and codes > 3 are all
    /// denied — never "warn but pass" — and carry the exact unknown-exit-code
    /// reason for the audit event.
    #[test]
    fn unknown_exit_codes_fail_closed() {
        for code in [1, -1, 4, 127, 255, i32::MAX, i32::MIN] {
            let decision = classify_pre_tool_use(&HookOutcome::Exited { code });
            assert_eq!(
                decision,
                PreToolUseDecision::DenyFailClosed {
                    reason: HookFailureReason::UnknownExitCode { exit_code: code },
                },
                "exit code {code} must fail closed",
            );
            assert!(
                !decision.permits_dispatch(),
                "exit code {code} must not permit dispatch",
            );
        }
    }

    /// Panic / timeout / signal / spawn failures each map to their own
    /// fail-closed reason — and none permit dispatch.
    #[test]
    fn abnormal_terminations_fail_closed_with_distinct_reasons() {
        let cases = [
            (HookOutcome::Panicked, HookFailureReason::Panic),
            (HookOutcome::TimedOut, HookFailureReason::Timeout),
            (
                HookOutcome::KilledBySignal { signo: 9 },
                HookFailureReason::KilledBySignal { signo: 9 },
            ),
            (HookOutcome::SpawnEnoent, HookFailureReason::SpawnEnoent),
            (HookOutcome::SpawnEacces, HookFailureReason::SpawnEacces),
        ];
        for (outcome, reason) in cases {
            let decision = classify_pre_tool_use(&outcome);
            assert_eq!(
                decision,
                PreToolUseDecision::DenyFailClosed { reason },
                "{outcome:?} must fail closed with its specific reason",
            );
            assert!(!decision.permits_dispatch());
        }
    }

    /// Exhaustive: across the documented allow/deny/needs-human codes plus a
    /// broad sweep of "other" outcomes, ONLY exit 0 ever permits dispatch.
    #[test]
    fn only_exit_zero_permits_dispatch() {
        let mut permitting = Vec::new();
        for code in -5..=10 {
            if classify_pre_tool_use(&HookOutcome::Exited { code }).permits_dispatch() {
                permitting.push(code);
            }
        }
        for outcome in [
            HookOutcome::Panicked,
            HookOutcome::TimedOut,
            HookOutcome::KilledBySignal { signo: 15 },
            HookOutcome::SpawnEnoent,
            HookOutcome::SpawnEacces,
        ] {
            assert!(
                !classify_pre_tool_use(&outcome).permits_dispatch(),
                "{outcome:?} must never permit dispatch",
            );
        }
        assert_eq!(
            permitting,
            vec![0],
            "exactly and only exit code 0 may permit dispatch",
        );
    }
}
