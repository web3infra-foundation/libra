use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Errors that can occur during orchestration.
#[derive(Debug, thiserror::Error)]
pub enum OrchestratorError {
    #[error("intent spec validation failed: {0}")]
    ValidationFailed(String),
    #[error("task failed: {task_id} — {reason}")]
    TaskFailed { task_id: Uuid, reason: String },
    #[error("agent error: {0}")]
    AgentError(String),
    #[error("gate execution error: {0}")]
    GateExecutionError(String),
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

/// A single task within the execution DAG.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskNode {
    pub id: Uuid,
    pub objective: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
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
    pub status: TaskNodeStatus,
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
        let mut adj: HashMap<Uuid, Vec<Uuid>> = self.nodes.iter().map(|n| (n.id, Vec::new())).collect();

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
                    && n.dependencies.iter().all(|dep| {
                        matches!(status_map.get(dep), Some(TaskNodeStatus::Completed))
                    })
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
    pub task_results: Vec<TaskResult>,
    pub system_report: SystemReport,
    pub intent_spec_id: String,
}

/// Configuration for the orchestrator.
#[derive(Clone, Debug)]
pub struct OrchestratorConfig {
    pub working_dir: PathBuf,
    pub base_commit: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_dag_topological_order_single() {
        let id = Uuid::new_v4();
        let dag = TaskDAG {
            nodes: vec![TaskNode {
                id,
                objective: "do thing".into(),
                description: None,
                dependencies: vec![],
                constraints: vec![],
                acceptance_criteria: vec![],
                scope_in: vec![],
                scope_out: vec![],
                status: TaskNodeStatus::Pending,
            }],
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
                    id: c,
                    objective: "c".into(),
                    description: None,
                    dependencies: vec![b],
                    constraints: vec![],
                    acceptance_criteria: vec![],
                    scope_in: vec![],
                    scope_out: vec![],
                    status: TaskNodeStatus::Pending,
                },
                TaskNode {
                    id: a,
                    objective: "a".into(),
                    description: None,
                    dependencies: vec![],
                    constraints: vec![],
                    acceptance_criteria: vec![],
                    scope_in: vec![],
                    scope_out: vec![],
                    status: TaskNodeStatus::Pending,
                },
                TaskNode {
                    id: b,
                    objective: "b".into(),
                    description: None,
                    dependencies: vec![a],
                    constraints: vec![],
                    acceptance_criteria: vec![],
                    scope_in: vec![],
                    scope_out: vec![],
                    status: TaskNodeStatus::Pending,
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
    fn test_ready_tasks_with_deps() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let dag = TaskDAG {
            nodes: vec![
                TaskNode {
                    id: a,
                    objective: "a".into(),
                    description: None,
                    dependencies: vec![],
                    constraints: vec![],
                    acceptance_criteria: vec![],
                    scope_in: vec![],
                    scope_out: vec![],
                    status: TaskNodeStatus::Pending,
                },
                TaskNode {
                    id: b,
                    objective: "b".into(),
                    description: None,
                    dependencies: vec![a],
                    constraints: vec![],
                    acceptance_criteria: vec![],
                    scope_in: vec![],
                    scope_out: vec![],
                    status: TaskNodeStatus::Pending,
                },
            ],
            intent_spec_id: "test".into(),
            max_parallel: 2,
        };
        // Only a is ready (b depends on a)
        let ready = dag.ready_tasks();
        assert!(ready.contains(&a));
        assert!(!ready.contains(&b));
    }

    #[test]
    fn test_ready_tasks_after_completion() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let mut dag = TaskDAG {
            nodes: vec![
                TaskNode {
                    id: a,
                    objective: "a".into(),
                    description: None,
                    dependencies: vec![],
                    constraints: vec![],
                    acceptance_criteria: vec![],
                    scope_in: vec![],
                    scope_out: vec![],
                    status: TaskNodeStatus::Completed,
                },
                TaskNode {
                    id: b,
                    objective: "b".into(),
                    description: None,
                    dependencies: vec![a],
                    constraints: vec![],
                    acceptance_criteria: vec![],
                    scope_in: vec![],
                    scope_out: vec![],
                    status: TaskNodeStatus::Pending,
                },
            ],
            intent_spec_id: "test".into(),
            max_parallel: 2,
        };
        let ready = dag.ready_tasks();
        assert!(ready.contains(&b));
        // Mark b running, no more ready
        dag.get_mut(b).unwrap().status = TaskNodeStatus::Running;
        assert!(dag.ready_tasks().is_empty());
    }

    #[test]
    fn test_serde_roundtrip_task_dag() {
        let id = Uuid::new_v4();
        let dag = TaskDAG {
            nodes: vec![TaskNode {
                id,
                objective: "test".into(),
                description: Some("desc".into()),
                dependencies: vec![],
                constraints: vec!["no-network".into()],
                acceptance_criteria: vec!["tests pass".into()],
                scope_in: vec!["src/".into()],
                scope_out: vec!["vendor/".into()],
                status: TaskNodeStatus::Pending,
            }],
            intent_spec_id: "spec-123".into(),
            max_parallel: 2,
        };
        let json = serde_json::to_string(&dag).unwrap();
        let back: TaskDAG = serde_json::from_str(&json).unwrap();
        assert_eq!(back.nodes.len(), 1);
        assert_eq!(back.nodes[0].id, id);
        assert_eq!(back.intent_spec_id, "spec-123");
    }

    #[test]
    fn test_serde_roundtrip_orchestrator_result() {
        let result = OrchestratorResult {
            decision: DecisionOutcome::Commit,
            task_results: vec![],
            system_report: SystemReport {
                integration: GateReport::empty(),
                security: GateReport::empty(),
                release: GateReport::empty(),
                overall_passed: true,
            },
            intent_spec_id: "test".into(),
        };
        let json = serde_json::to_string(&result).unwrap();
        let back: OrchestratorResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.decision, DecisionOutcome::Commit);
    }
}
