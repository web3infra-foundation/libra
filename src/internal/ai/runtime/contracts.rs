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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchedulerClearReason {
    Completed,
    Cancelled,
    Interrupted,
    Failed,
    Rebuild,
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
}
