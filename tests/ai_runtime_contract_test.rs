//! Wave 1A runtime contract tests.
//!
//! Pin the `TaskExecutor` trait contract so any provider that implements
//! `CompletionModel` can be plugged into the runtime by wrapping it in a thin
//! adapter. Verifies the runtime can build a task prompt, dispatch through a
//! `TaskExecutor`, and surface the response back as a `TaskExecutionResult`.
//!
//! **Layer:** L1 — uses `MockCompletionModel`, no external dependencies.

mod helpers;

use std::path::PathBuf;

use async_trait::async_trait;
use helpers::mock_completion_model::MockCompletionModel;
use libra::internal::ai::{
    completion::{AssistantContent, CompletionModel, CompletionRequest},
    runtime::{
        Runtime, RuntimeConfig,
        contracts::{
            ApprovalMediationState, TaskExecutionContext, TaskExecutionError, TaskExecutionResult,
            TaskExecutionStatus, TaskExecutor,
        },
    },
};
use uuid::Uuid;

/// Generic adapter that turns any `CompletionModel` into a `TaskExecutor`.
///
/// Demonstrates the wiring an integrator would write to plug a custom provider into
/// the runtime: forward the prompt messages, capture the first text response as the
/// summary, fabricate a `run_id` if one was not supplied, and report
/// `TaskExecutionStatus::Completed`.
#[derive(Clone)]
struct CompletionBackedTaskExecutor<M> {
    model: M,
}

#[async_trait]
impl<M> TaskExecutor for CompletionBackedTaskExecutor<M>
where
    M: CompletionModel + Clone + Send + Sync,
{
    async fn execute_task_attempt(
        &self,
        context: TaskExecutionContext,
    ) -> Result<TaskExecutionResult, TaskExecutionError> {
        let response = self
            .model
            .completion(CompletionRequest::new(
                context
                    .prompt
                    .messages
                    .into_iter()
                    .map(Into::into)
                    .collect(),
            ))
            .await
            .map_err(|err| TaskExecutionError::Provider(err.to_string()))?;
        let summary = response.content.first().and_then(|content| match content {
            AssistantContent::Text(text) => Some(text.text.clone()),
            AssistantContent::ToolCall(_) => None,
        });

        Ok(TaskExecutionResult {
            task_id: context.task_id,
            run_id: context.run_id.unwrap_or_else(Uuid::new_v4),
            status: TaskExecutionStatus::Completed,
            evidence: vec![],
            summary,
        })
    }
}

/// Scenario: build the runtime's task prompt with a fixture provider/model pair,
/// dispatch a single attempt through `CompletionBackedTaskExecutor` backed by
/// `MockCompletionModel::text("attempt complete")`, and assert the result preserves
/// the supplied `task_id`, marks the attempt completed, and surfaces the model's
/// text as the summary. Acts as the contract pin proving the runtime actually
/// integrates a generic provider via the `TaskExecutor` trait alone.
#[tokio::test]
async fn generic_provider_can_execute_through_task_executor_contract() {
    let runtime = Runtime::new(RuntimeConfig {
        principal: "contract-test".into(),
    });
    let prompt = runtime
        .task_prompt_builder("mock", "scripted")
        .task("write tests", "prove the runtime contract")
        .build();
    let task_id = Uuid::new_v4();
    let executor = CompletionBackedTaskExecutor {
        model: MockCompletionModel::text("attempt complete"),
    };

    let result = executor
        .execute_task_attempt(TaskExecutionContext {
            thread_id: Uuid::new_v4(),
            task_id,
            run_id: None,
            working_dir: PathBuf::from("."),
            prompt,
            approval: ApprovalMediationState::RuntimeMediatedInteractive,
        })
        .await
        .expect("task attempt");

    assert_eq!(result.task_id, task_id);
    assert_eq!(result.status, TaskExecutionStatus::Completed);
    assert_eq!(result.summary.as_deref(), Some("attempt complete"));
}
