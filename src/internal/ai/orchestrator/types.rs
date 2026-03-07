use std::{collections::HashMap, path::PathBuf, sync::Arc};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::internal::ai::{intentspec::types::Check, mcp::server::LibraMcpServer};

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

/// A single task within the execution DAG.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskNode {
    pub id: Uuid,
    pub title: String,
    pub objective: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub kind: TaskKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_stage: Option<GateStage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_role: Option<String>,
    #[serde(default)]
    pub dependencies: Vec<Uuid>,
    #[serde(default)]
    pub constraints: Vec<String>,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    #[serde(default)]
    pub scope_in: Vec<String>,
    #[serde(default)]
    pub scope_out: Vec<String>,
    #[serde(default)]
    pub checks: Vec<Check>,
    #[serde(default)]
    pub contract: TaskContract,
    pub status: TaskNodeStatus,
}

impl TaskNode {
    pub fn is_gate(&self) -> bool {
        self.kind == TaskKind::Gate
    }
}

/// Directed acyclic graph of tasks to execute.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskDAG {
    pub nodes: Vec<TaskNode>,
    pub intent_spec_id: String,
    pub max_parallel: u8,
}

impl TaskDAG {
    /// Return task IDs in topological order (dependencies before dependents).
    pub fn topological_order(&self) -> Vec<Uuid> {
        let index: HashMap<Uuid, usize> = self
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.id, i))
            .collect();

        let mut in_degree: HashMap<Uuid, usize> = self.nodes.iter().map(|n| (n.id, 0)).collect();
        let mut adj: HashMap<Uuid, Vec<Uuid>> =
            self.nodes.iter().map(|n| (n.id, Vec::new())).collect();

        for node in &self.nodes {
            for dep in &node.dependencies {
                if index.contains_key(dep) {
                    adj.get_mut(dep).unwrap().push(node.id);
                    *in_degree.get_mut(&node.id).unwrap() += 1;
                }
            }
        }

        let mut queue: Vec<Uuid> = in_degree
            .iter()
            .filter(|(_, deg)| **deg == 0)
            .map(|(&id, _)| id)
            .collect();
        queue.sort();

        let mut order = Vec::with_capacity(self.nodes.len());
        while let Some(id) = queue.pop() {
            order.push(id);
            if let Some(dependents) = adj.get(&id) {
                for &dep_id in dependents {
                    let deg = in_degree.get_mut(&dep_id).unwrap();
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push(dep_id);
                        queue.sort();
                    }
                }
            }
        }

        order
    }

    /// Return IDs of tasks that are ready to execute (pending, all deps completed).
    pub fn ready_tasks(&self) -> Vec<Uuid> {
        let status_map: HashMap<Uuid, &TaskNodeStatus> =
            self.nodes.iter().map(|n| (n.id, &n.status)).collect();

        self.nodes
            .iter()
            .filter(|n| {
                n.status == TaskNodeStatus::Pending
                    && n.dependencies
                        .iter()
                        .all(|dep| matches!(status_map.get(dep), Some(TaskNodeStatus::Completed)))
            })
            .map(|n| n.id)
            .collect()
    }

    /// Get an immutable reference to a task by ID.
    pub fn get(&self, id: Uuid) -> Option<&TaskNode> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// Get a mutable reference to a task by ID.
    pub fn get_mut(&mut self, id: Uuid) -> Option<&mut TaskNode> {
        self.nodes.iter_mut().find(|n| n.id == id)
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

/// The compiled execution plan derived from an IntentSpec.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecutionPlan {
    pub intent_spec_id: String,
    pub summary: String,
    pub dag: TaskDAG,
    #[serde(default)]
    pub parallel_groups: Vec<Vec<Uuid>>,
    #[serde(default)]
    pub checkpoints: Vec<ExecutionCheckpoint>,
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
    pub overall_passed: bool,
}

/// Final result of an orchestration run.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrchestratorResult {
    pub decision: DecisionOutcome,
    pub execution_plan: ExecutionPlan,
    pub task_results: Vec<TaskResult>,
    pub system_report: SystemReport,
    pub intent_spec_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persistence: Option<PersistedExecution>,
}

/// MCP object IDs emitted during orchestrator persistence.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PersistedTaskArtifacts {
    pub task_id: Uuid,
    #[serde(default)]
    pub tool_invocation_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patchset_id: Option<String>,
    #[serde(default)]
    pub evidence_ids: Vec<String>,
}

/// Persisted execution object chain for an orchestrator run.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PersistedExecution {
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_id: Option<String>,
    #[serde(default)]
    pub tasks: Vec<PersistedTaskArtifacts>,
}

/// Configuration for the orchestrator.
#[derive(Clone)]
pub struct OrchestratorConfig {
    pub working_dir: PathBuf,
    pub base_commit: Option<String>,
    /// System prompt injected into each task's tool loop (e.g. coder agent prompt).
    pub coder_preamble: Option<String>,
    /// Optional MCP server used to persist workflow objects.
    pub mcp_server: Option<Arc<LibraMcpServer>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn implementation_task(id: Uuid) -> TaskNode {
        TaskNode {
            id,
            title: "Do thing".into(),
            objective: "do thing".into(),
            description: None,
            kind: TaskKind::Implementation,
            gate_stage: None,
            owner_role: Some("coder".into()),
            dependencies: vec![],
            constraints: vec![],
            acceptance_criteria: vec![],
            scope_in: vec![],
            scope_out: vec![],
            checks: vec![],
            contract: TaskContract::default(),
            status: TaskNodeStatus::Pending,
        }
    }

    #[test]
    fn test_task_dag_topological_order_single() {
        let id = Uuid::new_v4();
        let dag = TaskDAG {
            nodes: vec![implementation_task(id)],
            intent_spec_id: "test".into(),
            max_parallel: 1,
        };
        assert_eq!(dag.topological_order(), vec![id]);
    }

    #[test]
    fn test_task_dag_topological_order_chain() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        let dag = TaskDAG {
            nodes: vec![
                TaskNode {
                    dependencies: vec![b],
                    objective: "c".into(),
                    title: "c".into(),
                    ..implementation_task(c)
                },
                implementation_task(a),
                TaskNode {
                    dependencies: vec![a],
                    objective: "b".into(),
                    title: "b".into(),
                    ..implementation_task(b)
                },
            ],
            intent_spec_id: "test".into(),
            max_parallel: 1,
        };
        let order = dag.topological_order();
        let pos_a = order.iter().position(|&x| x == a).unwrap();
        let pos_b = order.iter().position(|&x| x == b).unwrap();
        let pos_c = order.iter().position(|&x| x == c).unwrap();
        assert!(pos_a < pos_b);
        assert!(pos_b < pos_c);
    }

    #[test]
    fn test_ready_tasks_after_dependency_completion() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let mut dag = TaskDAG {
            nodes: vec![
                TaskNode {
                    status: TaskNodeStatus::Completed,
                    ..implementation_task(a)
                },
                TaskNode {
                    dependencies: vec![a],
                    objective: "b".into(),
                    title: "b".into(),
                    ..implementation_task(b)
                },
            ],
            intent_spec_id: "test".into(),
            max_parallel: 2,
        };
        let ready = dag.ready_tasks();
        assert_eq!(ready, vec![b]);

        dag.get_mut(b).unwrap().status = TaskNodeStatus::Running;
        assert!(dag.ready_tasks().is_empty());
    }

    #[test]
    fn test_serde_roundtrip_execution_plan() {
        let id = Uuid::new_v4();
        let plan = ExecutionPlan {
            intent_spec_id: "spec-123".into(),
            summary: "summary".into(),
            dag: TaskDAG {
                nodes: vec![implementation_task(id)],
                intent_spec_id: "spec-123".into(),
                max_parallel: 2,
            },
            parallel_groups: vec![vec![id]],
            checkpoints: vec![ExecutionCheckpoint {
                label: "after-fast".into(),
                after_tasks: vec![id],
                reason: "gate boundary".into(),
            }],
        };
        let json = serde_json::to_string(&plan).unwrap();
        let back: ExecutionPlan = serde_json::from_str(&json).unwrap();
        assert_eq!(back.parallel_groups.len(), 1);
        assert_eq!(back.dag.nodes[0].id, id);
    }

    #[test]
    fn test_serde_roundtrip_orchestrator_result() {
        let id = Uuid::new_v4();
        let result = OrchestratorResult {
            decision: DecisionOutcome::Commit,
            execution_plan: ExecutionPlan {
                intent_spec_id: "test".into(),
                summary: "summary".into(),
                dag: TaskDAG {
                    nodes: vec![implementation_task(id)],
                    intent_spec_id: "test".into(),
                    max_parallel: 1,
                },
                parallel_groups: vec![vec![id]],
                checkpoints: vec![],
            },
            task_results: vec![],
            system_report: SystemReport {
                integration: GateReport::empty(),
                security: GateReport::empty(),
                release: GateReport::empty(),
                overall_passed: true,
            },
            intent_spec_id: "test".into(),
            persistence: Some(PersistedExecution {
                run_id: "run-1".into(),
                provenance_id: Some("prov-1".into()),
                decision_id: Some("decision-1".into()),
                tasks: vec![PersistedTaskArtifacts {
                    task_id: id,
                    tool_invocation_ids: vec!["inv-1".into()],
                    patchset_id: Some("patch-1".into()),
                    evidence_ids: vec!["ev-1".into()],
                }],
            }),
        };
        let json = serde_json::to_string(&result).unwrap();
        let back: OrchestratorResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.decision, DecisionOutcome::Commit);
        assert_eq!(back.execution_plan.dag.nodes.len(), 1);
        assert_eq!(back.persistence.unwrap().tasks.len(), 1);
    }
}
