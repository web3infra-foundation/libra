use std::{collections::HashMap, sync::Arc};

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use uuid::Uuid;

use super::types::{ExecutionPlanSpec, TaskNodeStatus, TaskResult};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskStatusSnapshot {
    pub task_id: Uuid,
    pub status: TaskNodeStatus,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RunStateSnapshot {
    #[serde(default)]
    pub intent_spec_id: String,
    #[serde(default)]
    pub revision: u32,
    #[serde(default)]
    pub task_statuses: Vec<TaskStatusSnapshot>,
    #[serde(default)]
    pub task_results: Vec<TaskResult>,
}

impl RunStateSnapshot {
    pub fn result_for(&self, task_id: Uuid) -> Option<&TaskResult> {
        self.task_results.iter().find(|result| result.task_id == task_id)
    }

    pub fn status_for(&self, task_id: Uuid) -> TaskNodeStatus {
        self.task_statuses
            .iter()
            .find(|status| status.task_id == task_id)
            .map(|status| status.status.clone())
            .unwrap_or(TaskNodeStatus::Pending)
    }

    pub fn ordered_task_results(&self) -> &[TaskResult] {
        &self.task_results
    }

    pub fn is_empty(&self) -> bool {
        self.task_statuses.is_empty() && self.task_results.is_empty()
    }
}

#[derive(Clone, Default)]
pub struct RunStateStore {
    results: Arc<Mutex<HashMap<Uuid, TaskResult>>>,
}

impl RunStateStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn record_result(&self, result: TaskResult) {
        self.results.lock().await.insert(result.task_id, result);
    }

    pub async fn has_results(&self) -> bool {
        !self.results.lock().await.is_empty()
    }

    pub async fn snapshot(&self, plan: &ExecutionPlanSpec) -> RunStateSnapshot {
        let results = self.results.lock().await;
        let task_statuses = plan
            .tasks
            .iter()
            .map(|task| TaskStatusSnapshot {
                task_id: task.id,
                status: results
                    .get(&task.id)
                    .map(|result| result.status.clone())
                    .unwrap_or(TaskNodeStatus::Pending),
            })
            .collect();
        let task_results = plan
            .tasks
            .iter()
            .filter_map(|task| results.get(&task.id).cloned())
            .collect();

        RunStateSnapshot {
            intent_spec_id: plan.intent_spec_id.clone(),
            revision: plan.revision,
            task_statuses,
            task_results,
        }
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;
    use crate::internal::ai::orchestrator::types::{TaskContract, TaskKind, TaskSpec};

    fn test_plan(task_id: Uuid) -> ExecutionPlanSpec {
        ExecutionPlanSpec {
            intent_spec_id: "spec-1".into(),
            summary: "summary".into(),
            revision: 3,
            parent_revision: Some(2),
            replan_reason: Some("test".into()),
            tasks: vec![TaskSpec {
                id: task_id,
                title: "task".into(),
                objective: "task".into(),
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
            }],
            max_parallel: 1,
            parallel_groups: vec![vec![task_id]],
            checkpoints: vec![],
        }
    }

    #[tokio::test]
    async fn snapshot_preserves_plan_identity_and_order() {
        let task_id = Uuid::new_v4();
        let plan = test_plan(task_id);
        let store = RunStateStore::new();
        store
            .record_result(TaskResult {
                task_id,
                status: TaskNodeStatus::Completed,
                gate_report: None,
                agent_output: Some("done".into()),
                retry_count: 0,
                tool_calls: vec![],
                policy_violations: vec![],
                review: None,
            })
            .await;

        let snapshot = store.snapshot(&plan).await;
        assert_eq!(snapshot.intent_spec_id, "spec-1");
        assert_eq!(snapshot.revision, 3);
        assert_eq!(snapshot.task_results.len(), 1);
        assert_eq!(snapshot.status_for(task_id), TaskNodeStatus::Completed);
    }
}
