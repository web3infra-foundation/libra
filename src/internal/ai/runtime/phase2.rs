//! Phase 2 Execution — formal write helpers (schema-only landing).
//!
//! The Code UI Phase Workflow models Phase 2 as the **Execution** phase: a
//! [`TaskExecutionContext`] is dispatched to a [`TaskExecutor`], runs a
//! single task attempt, and produces a [`TaskExecutionResult`] alongside the
//! attempt-lifecycle formal writes (start / finish / patchset / evidence).
//!
//! # Schema vs. wiring
//!
//! This module is intentionally **schema-only** at this stage:
//! [`AttemptWriteOutcome`] freezes the shape callers will rely on once the
//! `write_attempt_start` / `write_attempt_finish` entry points are wired up.
//! The current attempt-lifecycle persistence path lives on
//! [`crate::internal::ai::orchestrator::persistence::ExecutionAuditSession`]
//! (the `RuntimeAuditCommand::TaskRuntime` channel plus
//! `RuntimeAuditObserver`); a future Wave 1B patch will either:
//!
//! 1. expose the session-bound recording helpers as `pub(crate)` free
//!    functions and have `phase2::write_attempt_*` delegate to them, **or**
//! 2. lift the channel-based runtime audit machinery into this module so
//!    the Runtime owns the only Execution formal-write entry point.
//!
//! Until that lift happens, callers still go through
//! [`crate::internal::ai::orchestrator::executor::execute_task`] and the
//! session observer. This module freezes the contract shape so the
//! eventual cutover is a mechanical redirect rather than an API redesign.
//!
//! # Why ship the schema now
//!
//! agent.md:161 lists `phase2.rs` as a Wave 1B blocker; flipping that row
//! from "缺失" to "schema 已落地" unblocks downstream documentation rows
//! (e.g. agent.md:153 已落地的 runtime 子模块 list) without bundling the
//! wiring change. The wiring patch can then focus on a single concern.

use uuid::Uuid;

use crate::internal::ai::runtime::contracts::TaskExecutionStatus;

/// Outcome of [`write_attempt_finish`]: the attempt's terminal status plus
/// the formal-write identifiers downstream observers / audit sinks need to
/// stitch the attempt back to its task / run rows.
///
/// **Stability contract:** field names are part of the public Runtime
/// surface once `write_attempt_finish` ships; downstream observers will
/// key off `task_id` / `run_id` / `status`. New fields may be added as
/// `Option<...>`; existing fields cannot be renamed or removed without a
/// parallel deprecation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttemptWriteOutcome {
    /// Task identifier the attempt belongs to (UUID assigned at intent
    /// canonicalisation time, stable across retries).
    pub task_id: Uuid,
    /// Run identifier persisted at attempt-start time; the row that the
    /// terminal status will be written against.
    pub run_id: Uuid,
    /// Terminal status of the attempt (matches the contracts-level enum so
    /// the Phase 2 path doesn't introduce parallel status semantics).
    pub status: TaskExecutionStatus,
    /// Optional human-readable summary of the attempt. Audit sinks may
    /// redact this through `SecretRedactor` before persisting; the schema
    /// stores the unredacted form so the boundary between Phase 2 and the
    /// audit layer stays explicit.
    pub summary: Option<String>,
}

impl AttemptWriteOutcome {
    /// `true` when the attempt ended in a terminal-failure status (i.e.
    /// not `Completed`). Used by Phase 3 validation routing to decide
    /// whether to escalate to a retry or open a `MergeCandidate`.
    pub fn is_failure(&self) -> bool {
        !matches!(self.status, TaskExecutionStatus::Completed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `is_failure()` must return `false` for `Completed` and `true` for
    /// every other variant so Phase 3 routing logic can fail-closed on
    /// unknown future statuses.
    #[test]
    fn is_failure_matches_completed_only() {
        let task_id = Uuid::new_v4();
        let run_id = Uuid::new_v4();

        let completed = AttemptWriteOutcome {
            task_id,
            run_id,
            status: TaskExecutionStatus::Completed,
            summary: None,
        };
        assert!(!completed.is_failure());

        for status in [
            TaskExecutionStatus::Failed,
            TaskExecutionStatus::Cancelled,
            TaskExecutionStatus::TimedOut,
            TaskExecutionStatus::Interrupted,
        ] {
            let label = format!("{status:?}");
            let outcome = AttemptWriteOutcome {
                task_id,
                run_id,
                status,
                summary: None,
            };
            assert!(
                outcome.is_failure(),
                "expected {label} to be a failure status",
            );
        }
    }

    /// `AttemptWriteOutcome` must derive `Clone` so observer / audit
    /// handlers can keep a snapshot while the caller continues mutating
    /// the executor state.
    #[test]
    fn outcome_is_clone() {
        let outcome = AttemptWriteOutcome {
            task_id: Uuid::new_v4(),
            run_id: Uuid::new_v4(),
            status: TaskExecutionStatus::Completed,
            summary: Some("ok".to_string()),
        };
        let cloned = outcome.clone();
        assert_eq!(cloned, outcome);
        assert_eq!(cloned.summary.as_deref(), Some("ok"));
    }
}
