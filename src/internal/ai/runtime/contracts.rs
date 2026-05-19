//! Formal runtime contracts shared by prompt builders, execution phases, validators,
//! and persistence.
//!
//! Boundary: these structs are stable internal APIs. Additive fields need defaults and
//! tests because persisted runs and projection rebuilds deserialize older records.
//! Runtime contract tests cover phase transitions and required evidence fields.

use std::{collections::HashSet, path::PathBuf};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::internal::ai::orchestrator::types::{ExecutionPlanSpec, TaskSpec};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanRole {
    Execution,
    Test,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlanSetWriteInput {
    pub execution: PlanRevisionSource,
    pub test: PlanRevisionSource,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum PlanRevisionSource {
    Existing {
        plan_id: Uuid,
    },
    New {
        spec: ExecutionPlanSpec,
        tasks: Vec<TaskSpec>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectedPlanSet {
    pub execution_plan_id: Uuid,
    pub test_plan_id: Uuid,
}

impl SelectedPlanSet {
    pub fn ordered_ids(&self) -> [Uuid; 2] {
        [self.execution_plan_id, self.test_plan_id]
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectionVersions {
    pub thread: i64,
    pub scheduler: i64,
    pub live_context_window: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Phase0Bundle {
    pub thread_id: Uuid,
    pub intent_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_snapshot_id: Option<Uuid>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DagStage {
    Execution,
    Test,
}

impl DagStage {
    /// Stable lower-snake-case identifier matching the
    /// `#[serde(rename_all = "snake_case")]` tag values. Used by
    /// `apply_scheduler_mutation(StartStage)` when writing the `stage`
    /// metadata field — keeping the string in one place protects
    /// against drift between metadata strings and serialised payloads.
    pub fn variant_name(self) -> &'static str {
        match self {
            DagStage::Execution => "execution",
            DagStage::Test => "test",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchedulerClearReason {
    Completed,
    Cancelled,
    Interrupted,
    Failed,
    Rebuild,
}

impl SchedulerClearReason {
    /// Stable lower-snake-case identifier matching the
    /// `#[serde(rename_all = "snake_case")]` tag values. Used by audit
    /// log emission so the `reason` field of a `ClearActiveRun` mutation
    /// can be stringified without reaching for `serde_json::to_value`.
    pub fn variant_name(&self) -> &'static str {
        match self {
            SchedulerClearReason::Completed => "completed",
            SchedulerClearReason::Cancelled => "cancelled",
            SchedulerClearReason::Interrupted => "interrupted",
            SchedulerClearReason::Failed => "failed",
            SchedulerClearReason::Rebuild => "rebuild",
        }
    }

    /// `true` when the clear reason represents a clean task completion
    /// (the only variant that does NOT signal a failure or interruption).
    /// Phase 3 routing uses this to decide whether to move on or
    /// escalate.
    pub fn is_clean_completion(&self) -> bool {
        matches!(self, SchedulerClearReason::Completed)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionStaleReason {
    RebuildRequired,
    DerivedRecordStale,
    CasConflict,
    Backpressure,
    ManualRepair,
}

impl ProjectionStaleReason {
    /// Stable lower-snake-case identifier matching the
    /// `#[serde(rename_all = "snake_case")]` tag values. Used by audit
    /// log emission so the `reason` field of a `MarkProjectionStale`
    /// mutation can be stringified without reaching for
    /// `serde_json::to_value`.
    pub fn variant_name(&self) -> &'static str {
        match self {
            ProjectionStaleReason::RebuildRequired => "rebuild_required",
            ProjectionStaleReason::DerivedRecordStale => "derived_record_stale",
            ProjectionStaleReason::CasConflict => "cas_conflict",
            ProjectionStaleReason::Backpressure => "backpressure",
            ProjectionStaleReason::ManualRepair => "manual_repair",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaterializedProjection {
    pub thread_id: Uuid,
    pub versions: ProjectionVersions,
    pub freshness: ProjectionFreshness,
    #[serde(default)]
    pub summary: serde_json::Value,
}

impl super::snapshot::Snapshot for MaterializedProjection {
    fn snapshot_kind(&self) -> &'static str {
        "materialized_projection"
    }

    fn snapshot_id(&self) -> Uuid {
        // Projection identity is the owning thread; multiple projection
        // versions for the same thread share the same snapshot id and are
        // distinguished by `versions`.
        self.thread_id
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mutation", rename_all = "snake_case")]
pub enum SchedulerMutation {
    SeedThread {
        expected: ProjectionVersions,
        bundle: Phase0Bundle,
    },
    SetCurrentPlanHeads {
        expected: ProjectionVersions,
        execution_plan_id: Uuid,
        test_plan_id: Uuid,
    },
    SelectPlanSet {
        expected: ProjectionVersions,
        selected: SelectedPlanSet,
    },
    StartStage {
        expected: ProjectionVersions,
        stage: DagStage,
    },
    MarkTaskActive {
        expected: ProjectionVersions,
        task_id: Uuid,
        run_id: Option<Uuid>,
    },
    ClearActiveRun {
        expected: ProjectionVersions,
        reason: SchedulerClearReason,
    },
    MarkProjectionStale {
        expected: ProjectionVersions,
        reason: ProjectionStaleReason,
    },
    ApplyRebuild {
        expected: ProjectionVersions,
        materialized: MaterializedProjection,
    },
}

impl SchedulerMutation {
    pub fn expected_versions(&self) -> ProjectionVersions {
        match self {
            SchedulerMutation::SeedThread { expected, .. }
            | SchedulerMutation::SetCurrentPlanHeads { expected, .. }
            | SchedulerMutation::SelectPlanSet { expected, .. }
            | SchedulerMutation::StartStage { expected, .. }
            | SchedulerMutation::MarkTaskActive { expected, .. }
            | SchedulerMutation::ClearActiveRun { expected, .. }
            | SchedulerMutation::MarkProjectionStale { expected, .. }
            | SchedulerMutation::ApplyRebuild { expected, .. } => *expected,
        }
    }

    /// Stable lower-snake-case identifier for the mutation variant.
    ///
    /// Used by audit log emission, the `ApplySchedulerMutationError::
    /// VariantNotWired { variant }` field, and any other site that needs
    /// to refer to "which mutation" without pattern-matching the full
    /// enum. The strings match the `#[serde(rename_all = "snake_case")]`
    /// tag values so a stringified variant name lines up with what
    /// observers see in serialised mutation payloads.
    pub fn variant_name(&self) -> &'static str {
        match self {
            SchedulerMutation::SeedThread { .. } => "seed_thread",
            SchedulerMutation::SetCurrentPlanHeads { .. } => "set_current_plan_heads",
            SchedulerMutation::SelectPlanSet { .. } => "select_plan_set",
            SchedulerMutation::StartStage { .. } => "start_stage",
            SchedulerMutation::MarkTaskActive { .. } => "mark_task_active",
            SchedulerMutation::ClearActiveRun { .. } => "clear_active_run",
            SchedulerMutation::MarkProjectionStale { .. } => "mark_projection_stale",
            SchedulerMutation::ApplyRebuild { .. } => "apply_rebuild",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinalDecisionVerdict {
    Accepted,
    Rejected,
    Cancelled,
    Abandon,
}

impl FinalDecisionVerdict {
    /// Stable lower-snake-case identifier matching the
    /// `#[serde(rename_all = "snake_case")]` tag values.
    pub fn variant_name(&self) -> &'static str {
        match self {
            FinalDecisionVerdict::Accepted => "accepted",
            FinalDecisionVerdict::Rejected => "rejected",
            FinalDecisionVerdict::Cancelled => "cancelled",
            FinalDecisionVerdict::Abandon => "abandon",
        }
    }

    /// `true` only for `Accepted` — the loop committed the change.
    pub fn is_accepted(&self) -> bool {
        matches!(self, FinalDecisionVerdict::Accepted)
    }

    /// `true` when the verdict ended the workflow without committing
    /// the change: `Rejected`, `Cancelled`, or `Abandon`. Distinguished
    /// from `is_accepted()` so callers can route on "did this loop
    /// produce an artifact?" without enumerating three variants.
    pub fn is_uncommitted(&self) -> bool {
        !self.is_accepted()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    Test,
    Lint,
    Build,
    Security,
    Performance,
    ContextSnapshotFreezeFailed,
    ProjectionRebuildFailed,
    ValidationBlockingFailed,
    ValidatorInfrastructureFailed,
    ToolPolicyViolation,
    SandboxProvisionFailed,
    SyncBackFailed,
    CleanupFailed,
    AuditPersistFailed,
    ProviderDisconnected,
    Timeout,
    Other(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionFreshness {
    Fresh,
    StaleReadOnly,
    Unavailable,
}

impl ProjectionFreshness {
    pub fn allows_scheduler_write(self) -> bool {
        matches!(self, ProjectionFreshness::Fresh)
    }

    pub fn allows_final_decision_write(self) -> bool {
        matches!(self, ProjectionFreshness::Fresh)
    }

    pub fn allows_resume_read(self) -> bool {
        matches!(
            self,
            ProjectionFreshness::Fresh | ProjectionFreshness::StaleReadOnly
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMediationState {
    LegacyInteractive,
    RuntimeMediatedInteractive,
    RuntimeMediatedNever,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptPackage {
    pub phase: WorkflowPhase,
    pub provider: String,
    pub model: String,
    pub preamble: String,
    pub messages: Vec<String>,
    #[serde(default)]
    pub readonly_tools: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowPhase {
    Intent,
    Planning,
    Execution,
    Validation,
    Decision,
}

impl WorkflowPhase {
    /// Every variant of [`WorkflowPhase`] in declaration order, which is
    /// also the Code UI Phase Workflow execution order
    /// (Intent → Planning → Execution → Validation → Decision; see
    /// `docs/improvement/agent.md` Part B).
    ///
    /// Useful for tests and validation loops that need to assert a
    /// property for every phase, and for ordered traversal of the
    /// workflow pipeline. The fixed-length array makes the enumeration
    /// size part of the public API: adding a new phase requires
    /// extending this list in the same patch, which forces every
    /// caller that pattern-matches on `WorkflowPhase` to be reviewed
    /// for the new variant.
    pub fn all() -> [Self; 5] {
        [
            Self::Intent,
            Self::Planning,
            Self::Execution,
            Self::Validation,
            Self::Decision,
        ]
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskExecutionContext {
    pub thread_id: Uuid,
    pub task_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<Uuid>,
    pub working_dir: PathBuf,
    pub prompt: PromptPackage,
    pub approval: ApprovalMediationState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskExecutionResult {
    pub task_id: Uuid,
    pub run_id: Uuid,
    pub status: TaskExecutionStatus,
    #[serde(default)]
    pub evidence: Vec<EvidenceKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskExecutionStatus {
    Completed,
    Failed,
    Cancelled,
    TimedOut,
    Interrupted,
}

impl TaskExecutionStatus {
    /// Every variant of [`TaskExecutionStatus`] in declaration order.
    ///
    /// Useful for exhaustive iteration in tests and validation loops —
    /// see e.g. [`crate::internal::ai::runtime::phase2::AttemptWriteOutcome`]'s
    /// `is_completed_fires_only_for_completed_and_matches_terminal_xor_failure`
    /// test which sweeps every status through the `is_completed` /
    /// `is_terminal` / `is_failure` partition invariant. The fixed-length
    /// array type makes the enumeration size part of the public API:
    /// adding a new variant requires extending this list in the same
    /// patch, which forces every caller's match arms and partition
    /// helpers (currently `AttemptWriteOutcome::is_terminal`,
    /// `is_failure`, `is_completed`) to be reconsidered.
    pub fn all() -> [Self; 5] {
        [
            Self::Completed,
            Self::Failed,
            Self::Cancelled,
            Self::TimedOut,
            Self::Interrupted,
        ]
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancellationReason {
    UserCancelled,
    Timeout,
    ProviderDisconnected,
    PolicyDenied,
    SchedulerStopped,
}

#[derive(Clone, Debug, Error, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskExecutionError {
    #[error("task execution was cancelled: {0:?}")]
    Cancelled(CancellationReason),
    #[error("provider failed during task execution: {0}")]
    Provider(String),
    #[error("tool boundary rejected task execution: {0}")]
    ToolPolicy(String),
    #[error("execution environment failed: {0}")]
    Environment(String),
}

#[async_trait]
pub trait TaskExecutor: Send + Sync {
    async fn execute_task_attempt(
        &self,
        context: TaskExecutionContext,
    ) -> Result<TaskExecutionResult, TaskExecutionError>;
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum TaskDependencyError {
    #[error("duplicate task id in stage plan: {task_id}")]
    DuplicateTask { task_id: Uuid },
    #[error("task {task_id} depends on {dependency_id}, which is outside the current stage plan")]
    CrossPlanDependency { task_id: Uuid, dependency_id: Uuid },
}

pub fn validate_same_plan_dependencies(
    tasks: &[(Uuid, Vec<Uuid>)],
) -> Result<(), TaskDependencyError> {
    let mut task_ids = HashSet::with_capacity(tasks.len());
    for (task_id, _) in tasks {
        if !task_ids.insert(*task_id) {
            return Err(TaskDependencyError::DuplicateTask { task_id: *task_id });
        }
    }

    for (task_id, dependencies) in tasks {
        for dependency_id in dependencies {
            if !task_ids.contains(dependency_id) {
                return Err(TaskDependencyError::CrossPlanDependency {
                    task_id: *task_id,
                    dependency_id: *dependency_id,
                });
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// `WorkflowPhase::all()` must enumerate every variant in the
    /// declaration order, which is also the Code UI Phase Workflow's
    /// execution order. Exhaustive cross-check pins the snake_case
    /// serialisation tag for each variant.
    #[test]
    fn workflow_phase_all_enumerates_every_variant_in_pipeline_order() {
        let phases = WorkflowPhase::all();
        assert_eq!(phases.len(), 5);
        assert_eq!(
            phases,
            [
                WorkflowPhase::Intent,
                WorkflowPhase::Planning,
                WorkflowPhase::Execution,
                WorkflowPhase::Validation,
                WorkflowPhase::Decision,
            ]
        );

        for phase in WorkflowPhase::all() {
            let expected = match phase {
                WorkflowPhase::Intent => "intent",
                WorkflowPhase::Planning => "planning",
                WorkflowPhase::Execution => "execution",
                WorkflowPhase::Validation => "validation",
                WorkflowPhase::Decision => "decision",
            };
            let serialised = serde_json::to_value(phase).unwrap();
            assert_eq!(serialised, json!(expected));
        }
    }

    /// `TaskExecutionStatus::all()` must enumerate every variant in
    /// declaration order and return a fixed-length array. The body
    /// uses an exhaustive `match` to force a compile error if a new
    /// variant lands without extending `all()` — the test stays in
    /// lock-step with the enum without relying on a runtime check.
    #[test]
    fn task_execution_status_all_enumerates_every_variant_in_declaration_order() {
        let statuses = TaskExecutionStatus::all();
        assert_eq!(statuses.len(), 5);
        assert_eq!(
            statuses,
            [
                TaskExecutionStatus::Completed,
                TaskExecutionStatus::Failed,
                TaskExecutionStatus::Cancelled,
                TaskExecutionStatus::TimedOut,
                TaskExecutionStatus::Interrupted,
            ]
        );

        // Exhaustive cross-check: every variant returned by `all()` must
        // serialise to a stable snake_case tag. The match is exhaustive
        // so a future sixth variant fails to compile here unless
        // `all()` is also updated and this arm gets a new branch.
        for status in TaskExecutionStatus::all() {
            let expected = match status {
                TaskExecutionStatus::Completed => "completed",
                TaskExecutionStatus::Failed => "failed",
                TaskExecutionStatus::Cancelled => "cancelled",
                TaskExecutionStatus::TimedOut => "timed_out",
                TaskExecutionStatus::Interrupted => "interrupted",
            };
            let serialised = serde_json::to_value(&status).unwrap();
            assert_eq!(serialised, json!(expected));
        }
    }

    #[test]
    fn selected_plan_set_preserves_execution_then_test_order() {
        let execution_plan_id = Uuid::new_v4();
        let test_plan_id = Uuid::new_v4();
        let selected = SelectedPlanSet {
            execution_plan_id,
            test_plan_id,
        };

        assert_eq!(selected.ordered_ids(), [execution_plan_id, test_plan_id]);
    }

    #[test]
    fn scheduler_mutation_requires_expected_versions() {
        let expected = ProjectionVersions {
            thread: 1,
            scheduler: 2,
            live_context_window: 3,
        };
        let mutation = SchedulerMutation::StartStage {
            expected,
            stage: DagStage::Execution,
        };

        assert_eq!(mutation.expected_versions(), expected);
        assert_eq!(
            serde_json::to_value(&mutation).unwrap(),
            json!({
                "mutation": "start_stage",
                "expected": {
                    "thread": 1,
                    "scheduler": 2,
                    "live_context_window": 3
                },
                "stage": "execution"
            })
        );
    }

    /// `variant_name()` must produce stable lower-snake-case strings
    /// matching the `#[serde(rename_all = "snake_case")]` tag values, so
    /// audit consumers can correlate the string against the serialised
    /// `mutation` field of the same payload.
    #[test]
    fn scheduler_mutation_variant_names_match_serde_tags() {
        let expected = ProjectionVersions::default();
        let logical = Uuid::new_v4();
        let selected = SelectedPlanSet {
            execution_plan_id: Uuid::new_v4(),
            test_plan_id: Uuid::new_v4(),
        };
        let bundle = Phase0Bundle {
            thread_id: Uuid::new_v4(),
            intent_id: Uuid::new_v4(),
            context_snapshot_id: None,
        };

        let cases: Vec<(SchedulerMutation, &str)> = vec![
            (
                SchedulerMutation::SeedThread {
                    expected,
                    bundle: bundle.clone(),
                },
                "seed_thread",
            ),
            (
                SchedulerMutation::SetCurrentPlanHeads {
                    expected,
                    execution_plan_id: logical,
                    test_plan_id: logical,
                },
                "set_current_plan_heads",
            ),
            (
                SchedulerMutation::SelectPlanSet {
                    expected,
                    selected: selected.clone(),
                },
                "select_plan_set",
            ),
            (
                SchedulerMutation::StartStage {
                    expected,
                    stage: DagStage::Execution,
                },
                "start_stage",
            ),
            (
                SchedulerMutation::MarkTaskActive {
                    expected,
                    task_id: logical,
                    run_id: Some(logical),
                },
                "mark_task_active",
            ),
            (
                SchedulerMutation::ClearActiveRun {
                    expected,
                    reason: SchedulerClearReason::Completed,
                },
                "clear_active_run",
            ),
            (
                SchedulerMutation::MarkProjectionStale {
                    expected,
                    reason: ProjectionStaleReason::RebuildRequired,
                },
                "mark_projection_stale",
            ),
            (
                SchedulerMutation::ApplyRebuild {
                    expected,
                    materialized: MaterializedProjection {
                        thread_id: bundle.thread_id,
                        versions: expected,
                        freshness: ProjectionFreshness::Fresh,
                        summary: serde_json::Value::Null,
                    },
                },
                "apply_rebuild",
            ),
        ];

        for (mutation, expected_name) in cases {
            // The variant_name() must match the const string.
            assert_eq!(
                mutation.variant_name(),
                expected_name,
                "variant_name mismatch for {expected_name}",
            );
            // It must also match the serialised `mutation` tag.
            let serialised = serde_json::to_value(&mutation).unwrap();
            assert_eq!(
                serialised.get("mutation").and_then(|v| v.as_str()),
                Some(expected_name),
                "serde tag mismatch for {expected_name}",
            );
        }
    }

    #[test]
    fn final_decision_verdict_serializes_cancelled_and_abandon() {
        assert_eq!(
            serde_json::to_string(&FinalDecisionVerdict::Cancelled).unwrap(),
            "\"cancelled\""
        );
        assert_eq!(
            serde_json::to_string(&FinalDecisionVerdict::Abandon).unwrap(),
            "\"abandon\""
        );
    }

    #[test]
    fn projection_freshness_controls_phase4_writes() {
        assert!(ProjectionFreshness::Fresh.allows_final_decision_write());
        assert!(!ProjectionFreshness::StaleReadOnly.allows_final_decision_write());
        assert!(!ProjectionFreshness::Unavailable.allows_resume_read());
    }

    #[test]
    fn dependency_validation_rejects_cross_plan_edges() {
        let task_id = Uuid::new_v4();
        let external_dependency = Uuid::new_v4();
        let err =
            validate_same_plan_dependencies(&[(task_id, vec![external_dependency])]).unwrap_err();

        assert_eq!(
            err,
            TaskDependencyError::CrossPlanDependency {
                task_id,
                dependency_id: external_dependency
            }
        );
    }

    #[test]
    fn task_execution_error_display_pins_each_variant() {
        assert_eq!(
            TaskExecutionError::Cancelled(CancellationReason::UserCancelled).to_string(),
            "task execution was cancelled: UserCancelled",
        );
        assert_eq!(
            TaskExecutionError::Provider("rate limited".to_string()).to_string(),
            "provider failed during task execution: rate limited",
        );
        assert_eq!(
            TaskExecutionError::ToolPolicy("apply_patch denied".to_string()).to_string(),
            "tool boundary rejected task execution: apply_patch denied",
        );
        assert_eq!(
            TaskExecutionError::Environment("workspace locked".to_string()).to_string(),
            "execution environment failed: workspace locked",
        );
    }

    #[test]
    fn task_dependency_error_display_pins_each_variant() {
        let task_id = Uuid::nil();
        let dependency_id = Uuid::nil();
        assert_eq!(
            TaskDependencyError::DuplicateTask { task_id }.to_string(),
            format!("duplicate task id in stage plan: {task_id}"),
        );
        assert_eq!(
            TaskDependencyError::CrossPlanDependency {
                task_id,
                dependency_id,
            }
            .to_string(),
            format!(
                "task {task_id} depends on {dependency_id}, \
                 which is outside the current stage plan",
            ),
        );
    }

    /// `DagStage::variant_name` and serde tag must agree — they're both
    /// public surfaces and drift would silently break the `stage`
    /// metadata field written by `apply_scheduler_mutation(StartStage)`.
    #[test]
    fn dag_stage_variant_name_matches_serde_tag() {
        for (stage, expected) in [(DagStage::Execution, "execution"), (DagStage::Test, "test")] {
            assert_eq!(stage.variant_name(), expected);
            assert_eq!(
                serde_json::to_string(&stage).unwrap(),
                format!("\"{expected}\""),
            );
        }
    }

    /// `SchedulerClearReason::variant_name` and serde tag must agree
    /// for all 5 variants. `is_clean_completion()` must be the
    /// `Completed`-only predicate.
    #[test]
    fn scheduler_clear_reason_variant_name_and_clean_completion() {
        for (reason, expected) in [
            (SchedulerClearReason::Completed, "completed"),
            (SchedulerClearReason::Cancelled, "cancelled"),
            (SchedulerClearReason::Interrupted, "interrupted"),
            (SchedulerClearReason::Failed, "failed"),
            (SchedulerClearReason::Rebuild, "rebuild"),
        ] {
            assert_eq!(reason.variant_name(), expected);
            assert_eq!(
                serde_json::to_string(&reason).unwrap(),
                format!("\"{expected}\""),
            );
        }

        assert!(SchedulerClearReason::Completed.is_clean_completion());
        for reason in [
            SchedulerClearReason::Cancelled,
            SchedulerClearReason::Interrupted,
            SchedulerClearReason::Failed,
            SchedulerClearReason::Rebuild,
        ] {
            assert!(
                !reason.is_clean_completion(),
                "{reason:?} must NOT be a clean completion",
            );
        }
    }

    /// `ProjectionStaleReason::variant_name` must match serde tags for
    /// all 5 variants — same drift-detection logic as the other enums.
    #[test]
    fn projection_stale_reason_variant_name_matches_serde_tag() {
        for (reason, expected) in [
            (ProjectionStaleReason::RebuildRequired, "rebuild_required"),
            (
                ProjectionStaleReason::DerivedRecordStale,
                "derived_record_stale",
            ),
            (ProjectionStaleReason::CasConflict, "cas_conflict"),
            (ProjectionStaleReason::Backpressure, "backpressure"),
            (ProjectionStaleReason::ManualRepair, "manual_repair"),
        ] {
            assert_eq!(reason.variant_name(), expected);
            assert_eq!(
                serde_json::to_string(&reason).unwrap(),
                format!("\"{expected}\""),
            );
        }
    }

    /// `FinalDecisionVerdict::variant_name` for all 4 variants +
    /// `is_accepted` / `is_uncommitted` partition.
    #[test]
    fn final_decision_verdict_variant_name_and_partition() {
        for (verdict, expected) in [
            (FinalDecisionVerdict::Accepted, "accepted"),
            (FinalDecisionVerdict::Rejected, "rejected"),
            (FinalDecisionVerdict::Cancelled, "cancelled"),
            (FinalDecisionVerdict::Abandon, "abandon"),
        ] {
            assert_eq!(verdict.variant_name(), expected);
            assert_eq!(
                serde_json::to_string(&verdict).unwrap(),
                format!("\"{expected}\""),
            );
        }

        // Partition: is_accepted XOR is_uncommitted across all 4 variants.
        assert!(FinalDecisionVerdict::Accepted.is_accepted());
        assert!(!FinalDecisionVerdict::Accepted.is_uncommitted());
        for verdict in [
            FinalDecisionVerdict::Rejected,
            FinalDecisionVerdict::Cancelled,
            FinalDecisionVerdict::Abandon,
        ] {
            assert!(!verdict.is_accepted(), "{verdict:?} must NOT be accepted",);
            assert!(
                verdict.is_uncommitted(),
                "{verdict:?} must flag as uncommitted",
            );
        }
    }
}
