use std::path::PathBuf;

use super::gate;
use super::types::{GateReport, TaskNode, TaskNodeStatus, TaskResult, TaskDAG};
use crate::internal::ai::agent::tool_loop::{run_tool_loop, ToolLoopConfig};
use crate::internal::ai::completion::{CompletionModel, CompletionError};
use crate::internal::ai::intentspec::types::Check;
use crate::internal::ai::tools::registry::ToolRegistry;

/// Configuration for task execution.
#[derive(Clone)]
pub struct ExecutorConfig {
    pub tool_loop_config: ToolLoopConfig,
    pub max_retries: u8,
    pub backoff_seconds: u32,
    pub fast_checks: Vec<Check>,
    pub working_dir: PathBuf,
}

/// Execute a single task with retry logic.
///
/// The retry loop: run agent → run fast gates → if gates fail, retry up to max_retries.
pub async fn execute_task<M: CompletionModel>(
    task: &TaskNode,
    model: &M,
    registry: &ToolRegistry,
    config: &ExecutorConfig,
) -> TaskResult {
    let prompt = build_task_prompt(task);
    let mut retry_count: u8 = 0;

    loop {
        let agent_result = run_tool_loop(
            model,
            &prompt,
            registry,
            config.tool_loop_config.clone(),
        )
        .await;

        let agent_output = match agent_result {
            Ok(output) => output,
            Err(CompletionError::ResponseError(msg)) => {
                // Agent hit max_steps or similar — treat as failure but allow retry
                tracing::warn!(task_id = %task.id, "agent response error: {}", msg);
                msg
            }
            Err(e) => {
                return TaskResult {
                    task_id: task.id,
                    status: TaskNodeStatus::Failed,
                    gate_report: None,
                    agent_output: Some(e.to_string()),
                    retry_count,
                };
            }
        };

        // Run fast gates
        let gate_report = if config.fast_checks.is_empty() {
            GateReport::empty()
        } else {
            gate::run_gates(&config.fast_checks, &config.working_dir).await
        };

        if gate_report.all_required_passed {
            return TaskResult {
                task_id: task.id,
                status: TaskNodeStatus::Completed,
                gate_report: Some(gate_report),
                agent_output: Some(agent_output),
                retry_count,
            };
        }

        retry_count += 1;
        if retry_count > config.max_retries {
            return TaskResult {
                task_id: task.id,
                status: TaskNodeStatus::Failed,
                gate_report: Some(gate_report),
                agent_output: Some(agent_output),
                retry_count,
            };
        }

        // Backoff before retry
        if config.backoff_seconds > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(
                config.backoff_seconds as u64,
            ))
            .await;
        }
    }
}

/// Execute all tasks in the DAG in topological order.
///
/// Respects `max_parallel` for concurrent execution. When `max_parallel > 1`,
/// ready tasks (whose dependencies are all completed) run concurrently up to
/// the parallelism limit.
pub async fn execute_dag<M: CompletionModel + 'static>(
    dag: &mut TaskDAG,
    model: &M,
    registry: &ToolRegistry,
    config: &ExecutorConfig,
) -> Vec<TaskResult> {
    let max_parallel = dag.max_parallel.max(1) as usize;
    let mut results = Vec::with_capacity(dag.nodes.len());
    let mut failed = false;

    loop {
        if failed {
            break;
        }

        let ready = dag.ready_tasks();
        if ready.is_empty() {
            break;
        }

        // Take up to max_parallel ready tasks
        let batch: Vec<_> = ready.into_iter().take(max_parallel).collect();

        // Collect task snapshots for this batch
        let tasks: Vec<_> = batch
            .iter()
            .filter_map(|&id| dag.get(id).cloned())
            .collect();

        for &id in &batch {
            if let Some(node) = dag.get_mut(id) {
                node.status = TaskNodeStatus::Running;
            }
        }

        if tasks.len() == 1 {
            // Single task — no need for join
            let result = execute_task(&tasks[0], model, registry, config).await;
            if let Some(node) = dag.get_mut(result.task_id) {
                node.status = result.status.clone();
            }
            if result.status == TaskNodeStatus::Failed {
                failed = true;
            }
            results.push(result);
        } else {
            // Multiple tasks — run concurrently
            let mut handles = Vec::with_capacity(tasks.len());
            for task in tasks {
                let model = model.clone();
                let registry_dir = registry.working_dir().to_path_buf();
                let config = config.clone();
                handles.push(tokio::spawn(async move {
                    let local_registry = ToolRegistry::with_working_dir(registry_dir);
                    execute_task(&task, &model, &local_registry, &config).await
                }));
            }

            for handle in handles {
                match handle.await {
                    Ok(result) => {
                        if let Some(node) = dag.get_mut(result.task_id) {
                            node.status = result.status.clone();
                        }
                        if result.status == TaskNodeStatus::Failed {
                            failed = true;
                        }
                        results.push(result);
                    }
                    Err(e) => {
                        tracing::warn!("task join error: {}", e);
                        failed = true;
                    }
                }
            }
        }
    }

    results
}

fn build_task_prompt(task: &TaskNode) -> String {
    let mut parts = Vec::new();
    parts.push(format!("## Objective\n{}", task.objective));

    if let Some(desc) = &task.description {
        parts.push(format!("## Description\n{}", desc));
    }

    if !task.acceptance_criteria.is_empty() {
        parts.push(format!(
            "## Acceptance Criteria\n{}",
            task.acceptance_criteria
                .iter()
                .map(|c| format!("- {}", c))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }

    if !task.scope_in.is_empty() {
        parts.push(format!(
            "## In Scope\n{}",
            task.scope_in.join(", ")
        ));
    }

    if !task.scope_out.is_empty() {
        parts.push(format!(
            "## Out of Scope\n{}",
            task.scope_out.join(", ")
        ));
    }

    if !task.constraints.is_empty() {
        parts.push(format!(
            "## Constraints\n{}",
            task.constraints
                .iter()
                .map(|c| format!("- {}", c))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }

    parts.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::completion::{
        CompletionRequest, CompletionResponse, CompletionError,
        message::{AssistantContent, Text},
    };
    use crate::internal::ai::tools::registry::ToolRegistry;
    use std::path::Path;
    use uuid::Uuid;

    #[derive(Clone)]
    struct MockModel {
        final_text: String,
    }

    impl CompletionModel for MockModel {
        type Response = ();

        fn completion(
            &self,
            _request: CompletionRequest,
        ) -> impl std::future::Future<
            Output = Result<CompletionResponse<Self::Response>, CompletionError>,
        > + Send {
            let text = self.final_text.clone();
            async move {
                Ok(CompletionResponse {
                    content: vec![AssistantContent::Text(Text { text })],
                    raw_response: (),
                })
            }
        }
    }

    fn make_task(objective: &str) -> TaskNode {
        TaskNode {
            id: Uuid::new_v4(),
            objective: objective.into(),
            description: None,
            dependencies: vec![],
            constraints: vec![],
            acceptance_criteria: vec!["tests pass".into()],
            scope_in: vec!["src/".into()],
            scope_out: vec![],
            status: TaskNodeStatus::Pending,
        }
    }

    fn make_config(dir: &Path) -> ExecutorConfig {
        ExecutorConfig {
            tool_loop_config: ToolLoopConfig::default(),
            max_retries: 1,
            backoff_seconds: 0,
            fast_checks: vec![],
            working_dir: dir.to_path_buf(),
        }
    }

    #[tokio::test]
    async fn test_execute_task_success() {
        let model = MockModel {
            final_text: "done".into(),
        };
        let registry = ToolRegistry::new();
        let dir = tempfile::tempdir().unwrap();
        let config = make_config(dir.path());
        let task = make_task("do something");

        let result = execute_task(&task, &model, &registry, &config).await;
        assert_eq!(result.status, TaskNodeStatus::Completed);
        assert_eq!(result.retry_count, 0);
    }

    #[tokio::test]
    async fn test_execute_task_with_fast_checks_pass() {
        let model = MockModel {
            final_text: "done".into(),
        };
        let registry = ToolRegistry::new();
        let dir = tempfile::tempdir().unwrap();
        let mut config = make_config(dir.path());
        config.fast_checks = vec![Check {
            id: "fc1".into(),
            kind: crate::internal::ai::intentspec::types::CheckKind::Command,
            command: Some("true".into()),
            timeout_seconds: Some(10),
            expected_exit_code: None,
            required: true,
            artifacts_produced: vec![],
        }];

        let task = make_task("do something");
        let result = execute_task(&task, &model, &registry, &config).await;
        assert_eq!(result.status, TaskNodeStatus::Completed);
    }

    #[tokio::test]
    async fn test_execute_task_with_fast_checks_fail_then_retry() {
        let model = MockModel {
            final_text: "done".into(),
        };
        let registry = ToolRegistry::new();
        let dir = tempfile::tempdir().unwrap();
        let mut config = make_config(dir.path());
        config.max_retries = 1;
        config.fast_checks = vec![Check {
            id: "fc1".into(),
            kind: crate::internal::ai::intentspec::types::CheckKind::Command,
            command: Some("false".into()),
            timeout_seconds: Some(10),
            expected_exit_code: None,
            required: true,
            artifacts_produced: vec![],
        }];

        let task = make_task("do something");
        let result = execute_task(&task, &model, &registry, &config).await;
        assert_eq!(result.status, TaskNodeStatus::Failed);
        assert_eq!(result.retry_count, 2); // tried once + 1 retry + exceeded
    }

    #[tokio::test]
    async fn test_execute_dag_ordering() {
        let model = MockModel {
            final_text: "done".into(),
        };
        let registry = ToolRegistry::new();
        let dir = tempfile::tempdir().unwrap();
        let config = make_config(dir.path());

        let a = make_task("a");
        let mut b = make_task("b");
        b.dependencies = vec![a.id];

        let mut dag = TaskDAG {
            nodes: vec![a.clone(), b.clone()],
            intent_spec_id: "test".into(),
            max_parallel: 1,
        };

        let results = execute_dag(&mut dag, &model, &registry, &config).await;
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].task_id, a.id);
        assert_eq!(results[1].task_id, b.id);
    }

    #[test]
    fn test_build_task_prompt() {
        let task = TaskNode {
            id: Uuid::new_v4(),
            objective: "Implement feature X".into(),
            description: Some("Add new module".into()),
            dependencies: vec![],
            constraints: vec!["network:deny".into()],
            acceptance_criteria: vec!["tests pass".into()],
            scope_in: vec!["src/".into()],
            scope_out: vec!["vendor/".into()],
            status: TaskNodeStatus::Pending,
        };
        let prompt = build_task_prompt(&task);
        assert!(prompt.contains("Implement feature X"));
        assert!(prompt.contains("Add new module"));
        assert!(prompt.contains("tests pass"));
        assert!(prompt.contains("src/"));
        assert!(prompt.contains("vendor/"));
        assert!(prompt.contains("network:deny"));
    }

    #[tokio::test]
    async fn test_execute_dag_parallel_independent() {
        let model = MockModel {
            final_text: "done".into(),
        };
        let registry = ToolRegistry::new();
        let dir = tempfile::tempdir().unwrap();
        let config = make_config(dir.path());

        let a = make_task("a");
        let b = make_task("b");
        let c = make_task("c");

        let mut dag = TaskDAG {
            nodes: vec![a.clone(), b.clone(), c.clone()],
            intent_spec_id: "test".into(),
            max_parallel: 4,
        };

        let results = execute_dag(&mut dag, &model, &registry, &config).await;
        assert_eq!(results.len(), 3);
        // All should complete
        for r in &results {
            assert_eq!(r.status, TaskNodeStatus::Completed);
        }
    }

    #[tokio::test]
    async fn test_execute_dag_stops_on_failure() {
        // Model that always produces a response, but the fast check will fail
        let model = MockModel {
            final_text: "done".into(),
        };
        let registry = ToolRegistry::new();
        let dir = tempfile::tempdir().unwrap();
        let mut config = make_config(dir.path());
        config.max_retries = 0;
        config.fast_checks = vec![Check {
            id: "fail".into(),
            kind: crate::internal::ai::intentspec::types::CheckKind::Command,
            command: Some("false".into()),
            timeout_seconds: Some(10),
            expected_exit_code: None,
            required: true,
            artifacts_produced: vec![],
        }];

        let a = make_task("a");
        let b = make_task("b");

        let mut dag = TaskDAG {
            nodes: vec![a.clone(), b.clone()],
            intent_spec_id: "test".into(),
            max_parallel: 1,
        };

        let results = execute_dag(&mut dag, &model, &registry, &config).await;
        // First task fails, second should be skipped
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, TaskNodeStatus::Failed);
    }
}
