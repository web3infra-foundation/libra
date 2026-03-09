use std::{collections::BTreeSet, path::PathBuf, sync::Arc};

use git_internal::internal::object::{plan::PlanStep, task::Task as GitTask};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::run_state::RunStateSnapshot;
use crate::internal::ai::{
    intentspec::types::{ChangeLogEntry, Check},
    mcp::server::LibraMcpServer,
};

/// Errors that can occur during orchestration.
#[derive(Debug, thiserror::Error)]
pub enum OrchestratorError {
    #[error("intent spec validation failed: {0}")]
    ValidationFailed(String),
    #[error("planning failed: {0}")]
    PlanningFailed(String),
    #[error("task failed: {task_id} — {reason}")]
    TaskFailed { task_id: Uuid, reason: String },
    #[error("agent error: {0}")]
    AgentError(String),
    #[error("gate execution error: {0}")]
    GateExecutionError(String),
    #[error("policy violation: {0}")]
    PolicyViolation(String),
    #[error("config error: {0}")]
    ConfigError(String),
}

/// Status of a single task node in the DAG.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskNodeStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Skipped,
}

/// High-level type of an execution task.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskKind {
    Implementation,
    Gate,
}

/// The verification stage represented by a gate task.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GateStage {
    Fast,
    Integration,
    Security,
    Release,
}

/// Contract for a compiled task.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TaskContract {
    #[serde(default)]
    pub write_scope: Vec<String>,
    #[serde(default)]
    pub forbidden_scope: Vec<String>,
    #[serde(default)]
    pub touch_files: Vec<String>,
    #[serde(default)]
    pub touch_symbols: Vec<String>,
    #[serde(default)]
    pub touch_apis: Vec<String>,
    #[serde(default)]
    pub expected_outputs: Vec<String>,
}

/// A static task specification produced by planning.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskSpec {
    pub step: PlanStep,
    pub task: GitTask,
    pub objective: String,
    pub kind: TaskKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_stage: Option<GateStage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_role: Option<String>,
    #[serde(default)]
    pub scope_in: Vec<String>,
    #[serde(default)]
    pub scope_out: Vec<String>,
    #[serde(default)]
    pub checks: Vec<Check>,
    #[serde(default)]
    pub contract: TaskContract,
}

impl TaskSpec {
    pub fn step_id(&self) -> Uuid {
        self.step.step_id()
    }

    pub fn id(&self) -> Uuid {
        self.task.header().object_id()
    }

    pub fn title(&self) -> &str {
        self.task.title()
    }

    pub fn description(&self) -> Option<&str> {
        self.task.description()
    }

    pub fn dependencies(&self) -> &[Uuid] {
        self.task.dependencies()
    }

    pub fn constraints(&self) -> &[String] {
        self.task.constraints()
    }

    pub fn acceptance_criteria(&self) -> &[String] {
        self.task.acceptance_criteria()
    }
}

/// A checkpoint in the compiled execution plan.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecutionCheckpoint {
    pub label: String,
    #[serde(default)]
    pub after_tasks: Vec<Uuid>,
    pub reason: String,
}

/// A static execution plan specification derived from an IntentSpec.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecutionPlanSpec {
    pub intent_spec_id: String,
    #[serde(default = "default_execution_revision")]
    pub revision: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_revision: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replan_reason: Option<String>,
    #[serde(default)]
    pub tasks: Vec<TaskSpec>,
    pub max_parallel: u8,
    #[serde(default)]
    pub checkpoints: Vec<ExecutionCheckpoint>,
}

impl ExecutionPlanSpec {
    pub fn summary_line(&self) -> String {
        let mut summary = format!(
            "plan revision {} for intent {} ({} tasks, parallelism {})",
            self.revision,
            self.intent_spec_id,
            self.tasks.len(),
            self.max_parallel
        );
        if let Some(reason) = &self.replan_reason
            && !reason.trim().is_empty()
        {
            summary.push_str(&format!("; replan: {}", reason.trim()));
        }
        summary
    }

    pub fn parallel_groups(&self) -> Vec<Vec<Uuid>> {
        let mut remaining = self.tasks.clone();
        let mut completed = BTreeSet::new();
        let mut groups = Vec::new();

        while !remaining.is_empty() {
            let ready: Vec<Uuid> = remaining
                .iter()
                .filter(|task| {
                    task.dependencies()
                        .iter()
                        .all(|dep| completed.contains(dep))
                })
                .map(TaskSpec::id)
                .collect();
            if ready.is_empty() {
                break;
            }
            for id in &ready {
                completed.insert(*id);
            }
            remaining.retain(|task| !ready.contains(&task.id()));
            groups.push(ready);
        }

        groups
    }
}

/// A policy violation detected before or after a tool call.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PolicyViolation {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// A summary of a tool call executed within a task.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ToolDiffRecord {
    pub path: String,
    pub change_type: String,
    pub diff: String,
}

/// A summary of a tool call executed within a task.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub tool_name: String,
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments_json: Option<Value>,
    #[serde(default)]
    pub paths_read: Vec<String>,
    #[serde(default)]
    pub paths_written: Vec<String>,
    pub success: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default)]
    pub diffs: Vec<ToolDiffRecord>,
}

/// Result of executing a single verification check.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GateResult {
    pub check_id: String,
    pub kind: String,
    pub passed: bool,
    pub exit_code: i32,
    #[serde(default)]
    pub stdout: String,
    #[serde(default)]
    pub stderr: String,
    pub duration_ms: u64,
    pub timed_out: bool,
}

/// Aggregate report for a set of verification checks.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GateReport {
    pub results: Vec<GateResult>,
    pub all_required_passed: bool,
}

impl GateReport {
    pub fn empty() -> Self {
        Self {
            results: Vec::new(),
            all_required_passed: true,
        }
    }
}

/// Result of executing a single task.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReviewOutcome {
    pub approved: bool,
    pub summary: String,
    #[serde(default)]
    pub issues: Vec<String>,
}

/// Result of executing a single task.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskResult {
    pub task_id: Uuid,
    pub status: TaskNodeStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_report: Option<GateReport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_output: Option<String>,
    pub retry_count: u8,
    #[serde(default)]
    pub tool_calls: Vec<ToolCallRecord>,
    #[serde(default)]
    pub policy_violations: Vec<PolicyViolation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<ReviewOutcome>,
}

/// Final decision outcome for the orchestration run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DecisionOutcome {
    Commit,
    HumanReviewRequired,
    Abandon,
}

/// System-level verification report (integration, security, release).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SystemReport {
    pub integration: GateReport,
    pub security: GateReport,
    pub release: GateReport,
    pub review_passed: bool,
    #[serde(default)]
    pub review_findings: Vec<String>,
    pub artifacts_complete: bool,
    #[serde(default)]
    pub missing_artifacts: Vec<String>,
    pub overall_passed: bool,
}

/// Final result of an orchestration run.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrchestratorResult {
    pub decision: DecisionOutcome,
    pub execution_plan_spec: ExecutionPlanSpec,
    #[serde(default)]
    pub plan_revision_specs: Vec<ExecutionPlanSpec>,
    #[serde(default)]
    pub run_state: RunStateSnapshot,
    pub task_results: Vec<TaskResult>,
    pub system_report: SystemReport,
    pub intent_spec_id: String,
    #[serde(default)]
    pub lifecycle_change_log: Vec<ChangeLogEntry>,
    #[serde(default)]
    pub replan_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persistence: Option<PersistedExecution>,
}

/// MCP object IDs emitted during orchestrator persistence.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PersistedTaskArtifacts {
    pub task_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persisted_task_id: Option<String>,
    #[serde(default)]
    pub tool_invocation_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patchset_id: Option<String>,
    #[serde(default)]
    pub evidence_ids: Vec<String>,
}

/// Persisted checkpoint objects created during execution/replan.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PersistedCheckpoint {
    pub revision: u32,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dagrs_checkpoint_id: Option<String>,
}

/// Persisted execution object chain for an orchestrator run.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PersistedExecution {
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_snapshot_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_id: Option<String>,
    #[serde(default)]
    pub plan_ids: Vec<String>,
    #[serde(default)]
    pub checkpoints: Vec<PersistedCheckpoint>,
    #[serde(default)]
    pub tasks: Vec<PersistedTaskArtifacts>,
}

/// Best-effort observer for surfacing orchestrator runtime progress.
pub trait OrchestratorObserver: Send + Sync {
    fn on_plan_compiled(&self, _plan: &ExecutionPlanSpec) {}

    fn on_task_started(&self, _task: &TaskSpec) {}

    fn on_task_completed(&self, _task: &TaskSpec, _result: &TaskResult) {}

    fn on_task_assistant_message(&self, _task: &TaskSpec, _text: &str) {}

    fn on_tool_call_begin(
        &self,
        _task: &TaskSpec,
        _call_id: &str,
        _tool_name: &str,
        _arguments: &Value,
    ) {
    }

    fn on_tool_call_end(
        &self,
        _task: &TaskSpec,
        _call_id: &str,
        _tool_name: &str,
        _result: &Result<crate::internal::ai::tools::ToolOutput, String>,
    ) {
    }

    fn on_reviewer_started(&self, _task: &TaskSpec) {}

    fn on_reviewer_completed(&self, _task: &TaskSpec, _review: Option<&ReviewOutcome>) {}

    fn on_graph_progress(&self, _completed: usize, _total: usize) {}

    fn on_graph_checkpoint_saved(&self, _checkpoint_id: &str, _pc: usize, _completed_nodes: usize) {
    }

    fn on_graph_checkpoint_restored(&self, _checkpoint_id: &str, _pc: usize) {}

    fn on_replan(
        &self,
        _current_revision: u32,
        _next_revision: u32,
        _reason: &str,
        _diff_summary: &str,
    ) {
    }

    fn on_persistence_complete(&self, _execution: &PersistedExecution) {}
}

fn default_execution_revision() -> u32 {
    1
}

/// Configuration for the orchestrator.
#[derive(Clone)]
pub struct OrchestratorConfig {
    pub working_dir: PathBuf,
    pub base_commit: Option<String>,
    /// TODO: keep as a placeholder until checkpoint/resume is redesigned around
    /// userspace-fs change tracking. dagrs-native resume remains disabled.
    pub dagrs_resume_checkpoint_id: Option<String>,
    /// System prompt injected into each task's tool loop (e.g. coder agent prompt).
    pub coder_preamble: Option<String>,
    /// Optional system prompt for the reviewer pass.
    pub reviewer_preamble: Option<String>,
    /// Optional MCP server used to persist workflow objects.
    pub mcp_server: Option<Arc<LibraMcpServer>>,
    /// Optional observer used to surface runtime progress.
    pub observer: Option<Arc<dyn OrchestratorObserver>>,
}

#[cfg(test)]
mod tests {
    use git_internal::internal::object::{task::Task as GitTask, types::ActorRef};

    use super::*;
    use crate::internal::ai::orchestrator::run_state::RunStateSnapshot;

    fn implementation_task() -> TaskSpec {
        let actor = ActorRef::agent("test-planner").unwrap();
        let mut task = GitTask::new(actor, "Do thing", None).unwrap();
        task.set_description(None);
        TaskSpec {
            step: PlanStep::new("Do thing"),
            task,
            objective: "do thing".into(),
            kind: TaskKind::Implementation,
            gate_stage: None,
            owner_role: Some("coder".into()),
            scope_in: vec![],
            scope_out: vec![],
            checks: vec![],
            contract: TaskContract::default(),
        }
    }

    #[test]
    fn test_task_spec_serde_roundtrip() {
        let task = implementation_task();
        let json = serde_json::to_string(&task).unwrap();
        let back: TaskSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id(), task.id());
        assert_eq!(back.kind, TaskKind::Implementation);
    }

    #[test]
    fn test_execution_plan_spec_preserves_dependencies() {
        let first = implementation_task();
        let a = first.id();
        let second = {
            let actor = ActorRef::agent("test-planner").unwrap();
            let mut task = GitTask::new(actor, "b", None).unwrap();
            task.add_dependency(a);
            TaskSpec {
                step: PlanStep::new("b"),
                task,
                objective: "b".into(),
                kind: TaskKind::Implementation,
                gate_stage: None,
                owner_role: Some("coder".into()),
                scope_in: vec![],
                scope_out: vec![],
                checks: vec![],
                contract: TaskContract::default(),
            }
        };
        let b = second.id();
        let spec = ExecutionPlanSpec {
            intent_spec_id: "spec-123".into(),
            revision: 1,
            parent_revision: None,
            replan_reason: None,
            tasks: vec![first, second],
            max_parallel: 2,
            checkpoints: vec![],
        };

        assert_eq!(spec.tasks.len(), 2);
        assert_eq!(spec.tasks[1].dependencies(), &[a]);
        assert_eq!(spec.parallel_groups(), vec![vec![a], vec![b]]);
    }

    #[test]
    fn test_serde_roundtrip_execution_plan_spec() {
        let task = implementation_task();
        let id = task.id();
        let spec = ExecutionPlanSpec {
            intent_spec_id: "spec-123".into(),
            revision: 2,
            parent_revision: Some(1),
            replan_reason: Some("replan".into()),
            tasks: vec![task],
            max_parallel: 2,
            checkpoints: vec![ExecutionCheckpoint {
                label: "after-fast".into(),
                after_tasks: vec![id],
                reason: "gate boundary".into(),
            }],
        };
        let json = serde_json::to_string(&spec).unwrap();
        let back: ExecutionPlanSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(back.tasks.len(), 1);
        assert_eq!(back.tasks[0].id(), spec.tasks[0].id());
        assert_eq!(back.max_parallel, 2);
    }

    #[test]
    fn test_serde_roundtrip_orchestrator_result() {
        let task = implementation_task();
        let id = task.id();
        let result = OrchestratorResult {
            decision: DecisionOutcome::Commit,
            execution_plan_spec: ExecutionPlanSpec {
                intent_spec_id: "test".into(),
                revision: 2,
                parent_revision: Some(1),
                replan_reason: Some("security gate failed".into()),
                tasks: vec![task],
                max_parallel: 1,
                checkpoints: vec![],
            },
            plan_revision_specs: vec![],
            run_state: RunStateSnapshot {
                intent_spec_id: "test".into(),
                revision: 2,
                task_statuses: vec![],
                task_results: vec![],
                dagrs_runtime: Default::default(),
            },
            task_results: vec![],
            system_report: SystemReport {
                integration: GateReport::empty(),
                security: GateReport::empty(),
                release: GateReport::empty(),
                review_passed: true,
                review_findings: vec![],
                artifacts_complete: true,
                missing_artifacts: vec![],
                overall_passed: true,
            },
            intent_spec_id: "test".into(),
            lifecycle_change_log: vec![],
            replan_count: 1,
            persistence: Some(PersistedExecution {
                run_id: "run-1".into(),
                initial_snapshot_id: Some("snapshot-1".into()),
                provenance_id: Some("prov-1".into()),
                decision_id: Some("decision-1".into()),
                plan_ids: vec!["plan-1".into()],
                checkpoints: vec![PersistedCheckpoint {
                    revision: 2,
                    reason: "security gate failed".into(),
                    snapshot_id: Some("snapshot-2".into()),
                    decision_id: Some("checkpoint-1".into()),
                    dagrs_checkpoint_id: Some("ckpt-1".into()),
                }],
                tasks: vec![PersistedTaskArtifacts {
                    task_id: id,
                    persisted_task_id: Some("task-1".into()),
                    tool_invocation_ids: vec!["inv-1".into()],
                    patchset_id: Some("patch-1".into()),
                    evidence_ids: vec!["ev-1".into()],
                }],
            }),
        };
        let json = serde_json::to_string(&result).unwrap();
        let back: OrchestratorResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.decision, DecisionOutcome::Commit);
        assert_eq!(back.execution_plan_spec.tasks.len(), 1);
        assert_eq!(back.persistence.unwrap().tasks.len(), 1);
    }
}
