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

/// Errors returned by [`apply_scheduler_mutation`] when the input state
/// or mutation can't be advanced.
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum ApplySchedulerMutationError {
    /// The mutation's expected `scheduler` version doesn't match the
    /// state's current version. Caller should reload state and retry.
    #[error("scheduler version mismatch: mutation expected {expected}, state at {actual}")]
    VersionMismatch { expected: i64, actual: i64 },
    /// The mutation variant doesn't yet have a wired implementation in
    /// this helper. Wave 1B follow-up will fold the orchestrator's
    /// existing scheduler updates into this function; until then,
    /// unsupported variants surface this error so callers can route
    /// through the legacy `orchestrator::persistence` path.
    #[error(
        "scheduler mutation variant {variant} is not yet wired by apply_scheduler_mutation; \
         route through orchestrator::persistence for now"
    )]
    VariantNotWired { variant: &'static str },
}

/// Apply a [`SchedulerMutation`](crate::internal::ai::runtime::contracts::SchedulerMutation)
/// to a [`SchedulerState`](crate::internal::ai::projection::scheduler::SchedulerState)
/// snapshot, returning the next state.
///
/// **Pure function** — no DB IO; the caller is responsible for loading
/// `current` via
/// [`SchedulerStateRepository::load`](crate::internal::ai::projection::scheduler::SchedulerStateRepository::load)
/// and persisting the returned state via
/// [`SchedulerStateRepository::compare_and_swap`](crate::internal::ai::projection::scheduler::SchedulerStateRepository::compare_and_swap).
///
/// # Wired variants (v0.17.588)
///
/// - [`SchedulerMutation::MarkTaskActive`](crate::internal::ai::runtime::contracts::SchedulerMutation::MarkTaskActive)
///   — sets `active_task_id` to `Some(task_id)` and `active_run_id` to
///   the mutation's `run_id` (which may itself be `None`); bumps
///   `version` by 1 and refreshes `updated_at`.
/// - [`SchedulerMutation::ClearActiveRun`](crate::internal::ai::runtime::contracts::SchedulerMutation::ClearActiveRun)
///   — clears `active_run_id` to `None` while preserving the
///   `active_task_id` (the task remains the scheduler's current focus
///   even when no run is in flight); bumps `version` by 1.
///
/// # Unwired variants
///
/// `SeedThread` / `SetCurrentPlanHeads` / `SelectPlanSet` / `StartStage`
/// / `MarkProjectionStale` / `ApplyRebuild` return
/// [`ApplySchedulerMutationError::VariantNotWired`]. These variants
/// involve `Phase0Bundle` decomposition, plan-head list management, or
/// full projection rebuilds that today are owned by
/// `orchestrator::persistence::ExecutionAuditSession`; lifting them
/// into this pure function is queued for a future Wave 1B patch.
pub fn apply_scheduler_mutation(
    current: &crate::internal::ai::projection::scheduler::SchedulerState,
    mutation: crate::internal::ai::runtime::contracts::SchedulerMutation,
) -> Result<crate::internal::ai::projection::scheduler::SchedulerState, ApplySchedulerMutationError>
{
    use crate::internal::ai::runtime::contracts::SchedulerMutation;

    let expected = mutation.expected_versions().scheduler;
    if current.version != expected {
        return Err(ApplySchedulerMutationError::VersionMismatch {
            expected,
            actual: current.version,
        });
    }

    let mut next = current.clone();
    next.version = current.version + 1;
    next.updated_at = chrono::Utc::now();

    match mutation {
        SchedulerMutation::MarkTaskActive {
            task_id, run_id, ..
        } => {
            next.active_task_id = Some(task_id);
            next.active_run_id = run_id;
        }
        SchedulerMutation::ClearActiveRun { .. } => {
            next.active_run_id = None;
        }
        SchedulerMutation::SeedThread { .. } => {
            return Err(ApplySchedulerMutationError::VariantNotWired {
                variant: "SeedThread",
            });
        }
        SchedulerMutation::SetCurrentPlanHeads { .. } => {
            return Err(ApplySchedulerMutationError::VariantNotWired {
                variant: "SetCurrentPlanHeads",
            });
        }
        SchedulerMutation::SelectPlanSet { .. } => {
            return Err(ApplySchedulerMutationError::VariantNotWired {
                variant: "SelectPlanSet",
            });
        }
        SchedulerMutation::StartStage { .. } => {
            return Err(ApplySchedulerMutationError::VariantNotWired {
                variant: "StartStage",
            });
        }
        SchedulerMutation::MarkProjectionStale { .. } => {
            return Err(ApplySchedulerMutationError::VariantNotWired {
                variant: "MarkProjectionStale",
            });
        }
        SchedulerMutation::ApplyRebuild { .. } => {
            return Err(ApplySchedulerMutationError::VariantNotWired {
                variant: "ApplyRebuild",
            });
        }
    }

    Ok(next)
}

/// Persist a new plan set as the **formal write** for Phase 1.
///
/// Bridges into
/// [`crate::internal::ai::orchestrator::persistence::write_plan_set_with_outcome`]
/// so the orchestrator's existing `PersistedPlanRevision` /
/// `step_id_map` plumbing stays where it lives today, while the public
/// contract surface (this function + [`PlanWriteOutcome`]) is owned by
/// the Runtime. Once the orchestrator's persistence layer is folded into
/// this module, the bridge disappears.
///
/// # Errors
///
/// Returns the underlying
/// [`crate::internal::ai::orchestrator::types::OrchestratorError`]
/// unchanged so callers can route on the existing error variants without
/// a new typed-error wrapper.
pub async fn write_plan_set(
    mcp_server: &std::sync::Arc<crate::internal::ai::mcp::server::LibraMcpServer>,
    intent_id: &str,
    parent_execution_plan_id: Option<&str>,
    parent_test_plan_id: Option<&str>,
    plan: &crate::internal::ai::orchestrator::types::ExecutionPlanSpec,
) -> Result<PlanWriteOutcome, crate::internal::ai::orchestrator::types::OrchestratorError> {
    crate::internal::ai::orchestrator::persistence::write_plan_set_with_outcome(
        mcp_server,
        intent_id,
        parent_execution_plan_id,
        parent_test_plan_id,
        plan,
    )
    .await
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

    use chrono::Utc;

    use crate::internal::ai::{
        projection::scheduler::SchedulerState,
        runtime::contracts::{ProjectionVersions, SchedulerClearReason, SchedulerMutation},
    };

    fn dummy_scheduler_state(version: i64) -> SchedulerState {
        SchedulerState {
            thread_id: Uuid::new_v4(),
            selected_plan_id: None,
            selected_plan_ids: Vec::new(),
            current_plan_heads: Vec::new(),
            active_task_id: None,
            active_run_id: None,
            live_context_window: Vec::new(),
            metadata: None,
            updated_at: Utc::now(),
            version,
        }
    }

    /// `MarkTaskActive` must set `active_task_id` to the requested task,
    /// pass `run_id` through verbatim (including `None`), bump `version`
    /// by 1, and refresh `updated_at`.
    #[test]
    fn apply_scheduler_mutation_mark_task_active_sets_active_task_and_run() {
        let current = dummy_scheduler_state(7);
        let task_id = Uuid::new_v4();
        let run_id = Uuid::new_v4();
        let mutation = SchedulerMutation::MarkTaskActive {
            expected: ProjectionVersions {
                thread: 0,
                scheduler: 7,
                live_context_window: 0,
            },
            task_id,
            run_id: Some(run_id),
        };

        let next = apply_scheduler_mutation(&current, mutation).expect("mutation should apply");

        assert_eq!(next.active_task_id, Some(task_id));
        assert_eq!(next.active_run_id, Some(run_id));
        assert_eq!(next.version, 8);
        assert!(next.updated_at >= current.updated_at);
    }

    /// `ClearActiveRun` must zero out `active_run_id` while preserving
    /// `active_task_id` (the task remains the scheduler's focus even
    /// without a live run).
    #[test]
    fn apply_scheduler_mutation_clear_active_run_keeps_task_drops_run() {
        let mut current = dummy_scheduler_state(3);
        current.active_task_id = Some(Uuid::new_v4());
        current.active_run_id = Some(Uuid::new_v4());
        let preserved_task = current.active_task_id;
        let mutation = SchedulerMutation::ClearActiveRun {
            expected: ProjectionVersions {
                thread: 0,
                scheduler: 3,
                live_context_window: 0,
            },
            reason: SchedulerClearReason::Completed,
        };

        let next = apply_scheduler_mutation(&current, mutation).expect("mutation should apply");

        assert_eq!(next.active_task_id, preserved_task);
        assert_eq!(next.active_run_id, None);
        assert_eq!(next.version, 4);
    }

    /// Version mismatch must fail-closed with `VersionMismatch` so the
    /// caller can route to a reload-and-retry path instead of silently
    /// writing stale state.
    #[test]
    fn apply_scheduler_mutation_rejects_version_mismatch() {
        let current = dummy_scheduler_state(5);
        let mutation = SchedulerMutation::MarkTaskActive {
            expected: ProjectionVersions {
                thread: 0,
                scheduler: 99, // doesn't match current.version == 5
                live_context_window: 0,
            },
            task_id: Uuid::new_v4(),
            run_id: None,
        };

        let error = apply_scheduler_mutation(&current, mutation)
            .expect_err("version mismatch must fail-closed");
        assert_eq!(
            error,
            ApplySchedulerMutationError::VersionMismatch {
                expected: 99,
                actual: 5,
            }
        );
    }

    /// Unwired variants must surface `VariantNotWired { variant }` with
    /// the variant name so callers can detect when a follow-up has
    /// landed (the error variant string is the discriminator).
    #[test]
    fn apply_scheduler_mutation_unwired_variant_surfaces_variant_name() {
        let current = dummy_scheduler_state(1);
        let mutation = SchedulerMutation::StartStage {
            expected: ProjectionVersions {
                thread: 0,
                scheduler: 1,
                live_context_window: 0,
            },
            stage: crate::internal::ai::runtime::contracts::DagStage::Execution,
        };

        let error =
            apply_scheduler_mutation(&current, mutation).expect_err("StartStage is not yet wired");
        assert_eq!(
            error,
            ApplySchedulerMutationError::VariantNotWired {
                variant: "StartStage"
            }
        );
    }
}
