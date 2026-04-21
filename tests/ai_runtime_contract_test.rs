//! Wave 1A runtime contract tests.

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
