//! Phase 1 Plan — formal write helpers (schema-only landing).
//!
//! The Code UI Phase Workflow models Phase 1 as the **Plan** phase: the
//! Phase 0 [`IntentSpec`] gets compiled into an `ExecutionPlanSpec` which is
//! persisted as a paired execution / test plan revision and then folded into
//! the scheduler state machine.
//!
//! # Schema vs. wiring
//!
//! This module is intentionally **schema-only** at this stage:
//! [`PlanWriteOutcome`] declares the stable contract callers can rely on
//! once the formal-write entry point (`write_plan_set`) is wired up. The
//! current Plan persistence path lives on
//! [`crate::internal::ai::orchestrator::persistence::ExecutionAuditSession::record_plan_compiled`]
//! (a session method) plus private free functions
//! (`create_plan_set_revision`, `build_plan_set`); a future Wave 1B patch
//! will either:
//!
//! 1. expose the free-function path with `pub(crate)` visibility and have
//!    `phase1::write_plan_set` delegate to it, **or**
//! 2. lift the session-bound `record_plan_compiled` into a free function on
//!    this module so the Runtime owns the only Plan formal-write entry
//!    point.
//!
//! Until that lift happens, callers still go through
//! `ExecutionAuditSession::record_plan_compiled` directly. This module
//! freezes the contract shape so the eventual cutover is a mechanical
//! redirect rather than an API redesign.
//!
//! # Why ship the schema now
//!
//! agent.md:160 lists `phase1.rs` as a Wave 1B blocker; flipping that row
//! from "缺失" to "schema 已落地" unblocks downstream documentation rows
//! (e.g. agent.md:153 已落地的 runtime 子模块 list) without bundling the
//! wiring change. The wiring patch can then focus on a single concern.

/// Outcome of the planned [`write_plan_set`] entry point: identifiers for
/// the paired execution / test plan revisions and the
/// `task_id → plan_id` map the scheduler will use to advance.
///
/// **Stability contract:** field names are part of the public Runtime
/// surface once `write_plan_set` ships; downstream observers / audit code
/// will key off `execution_plan_id` and `test_plan_id`. New fields may be
/// added as `Option<...>` or `#[serde(default)]`; existing fields cannot be
/// renamed or removed without a parallel deprecation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanWriteOutcome {
    /// Identifier of the persisted execution-plan revision.
    pub execution_plan_id: String,
    /// Identifier of the paired test-plan revision (Libra always creates
    /// execution + test plans together so Phase 3 validation has a stable
    /// reference).
    pub test_plan_id: String,
    /// Map from logical `task_id` (UUID assigned at intent canonicalisation
    /// time) to the persisted `plan_id` that owns the corresponding step.
    /// The Scheduler reads this to thread `task_id` ↔ `plan_id` for `dagrs`
    /// node addressing and for the `agent_usage_stats.plan_id` column.
    pub plan_id_by_task_id: std::collections::HashMap<uuid::Uuid, String>,
}

impl PlanWriteOutcome {
    /// Returns the (execution, test) plan id pair as the canonical
    /// scheduler-facing ordering.
    ///
    /// `SchedulerMutation::SetCurrentPlanHeads` expects the execution head
    /// before the test head, matching
    /// [`crate::internal::ai::runtime::contracts::SelectedPlanSet::ordered_ids`].
    pub fn ordered_plan_ids(&self) -> (&str, &str) {
        (self.execution_plan_id.as_str(), self.test_plan_id.as_str())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use uuid::Uuid;

    use super::*;

    /// `ordered_plan_ids()` must return `(execution, test)` so it lines up
    /// with [`SelectedPlanSet::ordered_ids`] downstream.
    #[test]
    fn ordered_plan_ids_returns_execution_then_test() {
        let outcome = PlanWriteOutcome {
            execution_plan_id: "plan-exec-1".to_string(),
            test_plan_id: "plan-test-1".to_string(),
            plan_id_by_task_id: HashMap::new(),
        };
        let (exec, test) = outcome.ordered_plan_ids();
        assert_eq!(exec, "plan-exec-1");
        assert_eq!(test, "plan-test-1");
    }

    /// `PlanWriteOutcome` must derive `Clone` so observer / audit handlers
    /// can keep a snapshot while the caller continues mutating the
    /// scheduler state.
    #[test]
    fn outcome_is_clone() {
        let task_id = Uuid::new_v4();
        let mut map = HashMap::new();
        map.insert(task_id, "plan-exec-1".to_string());

        let outcome = PlanWriteOutcome {
            execution_plan_id: "plan-exec-1".to_string(),
            test_plan_id: "plan-test-1".to_string(),
            plan_id_by_task_id: map,
        };
        let cloned = outcome.clone();
        assert_eq!(cloned, outcome);
        assert_eq!(
            cloned.plan_id_by_task_id.get(&task_id).map(String::as_str),
            Some("plan-exec-1")
        );
    }
}
