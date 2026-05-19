//! Wave 1B `TaskExecutor` adapter structs (schema-only landing).
//!
//! The Code UI Phase Workflow's Wave 1B "Definition of Done #1" requires
//! that **both** providers — Codex (`CodexTaskExecutor`) and any generic
//! completion-model provider (`CompletionTaskExecutor<M>`) — implement the
//! shared [`TaskExecutor`] trait from
//! [`crate::internal::ai::runtime::contracts`] so the runtime can address
//! all task executors through a single trait object.
//!
//! This module is the **schema-only landing** for those impl blocks: the
//! struct shapes and the `impl TaskExecutor` blocks are present (so the
//! Wave 1B blocker rows can flip from "缺失" to "schema 已落地"), but the
//! `execute_task_attempt` bodies return a structured
//! [`TaskExecutionError::Provider`] pointing at the substantive wiring
//! work. The body fill-in is a follow-up patch that has to:
//!
//! - **Codex path**: take the existing Codex app-server WebSocket driver
//!   (today living inside `src/internal/ai/codex/mod.rs::
//!   CodexCodeUiAdapter`), extract the per-attempt slice into a free
//!   function, and have `CodexTaskExecutor::execute_task_attempt`
//!   delegate to it.
//! - **Completion path**: take the existing
//!   `orchestrator::executor::execute_task<M>` function, build a
//!   `TaskSpec` from `TaskExecutionContext`, and route the completion
//!   model + tool registry through it.
//!
//! Both bodies are non-trivial cross-cutting refactors; landing them in a
//! single patch was the original Wave 1B plan, but stalling the rest of
//! Wave 1B on that single patch is what the readiness matrix at
//! [`docs/improvement/agent.md`](../../../../../docs/improvement/agent.md)
//! line 173 calls out. Splitting impl-shape vs. impl-body into two patches
//! unblocks downstream readiness rows (`agent.md:164` / `:165` flip to
//! "schema 已落地") without misrepresenting the executor as production
//! ready.

use async_trait::async_trait;

use crate::internal::ai::{
    completion::CompletionModel,
    runtime::contracts::{
        TaskExecutionContext, TaskExecutionError, TaskExecutionResult, TaskExecutor,
    },
};

/// Schema-only `TaskExecutor` adapter for the Codex backend.
///
/// **State:** Wave 1B impl-shape only. Calls to
/// [`Self::execute_task_attempt`] return a structured
/// [`TaskExecutionError::Provider`] pointing at the wiring follow-up;
/// callers should NOT route real task attempts through this struct yet.
///
/// The struct has no fields today because the real wiring will pull
/// runtime configuration from a future `RuntimeConfig` extension; keeping
/// the type unit-like avoids freezing field-name decisions before that
/// extension is designed.
#[derive(Default)]
pub struct CodexTaskExecutor;

impl CodexTaskExecutor {
    /// Construct a schema-only `CodexTaskExecutor`.
    ///
    /// See the struct docs for the "what's missing" contract.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl TaskExecutor for CodexTaskExecutor {
    async fn execute_task_attempt(
        &self,
        _context: TaskExecutionContext,
    ) -> Result<TaskExecutionResult, TaskExecutionError> {
        Err(TaskExecutionError::Provider(
            "CodexTaskExecutor::execute_task_attempt is a Wave 1B schema-only stub; \
             the Codex WebSocket-driven per-attempt loop will land in a follow-up \
             patch (see src/internal/ai/runtime/task_executors.rs module docs)."
                .to_string(),
        ))
    }
}

/// Schema-only `TaskExecutor` adapter for any generic completion-model
/// provider.
///
/// **State:** Wave 1B impl-shape only. Same caveat as
/// [`CodexTaskExecutor`]: calls return a structured
/// [`TaskExecutionError::Provider`] pointing at the wiring follow-up.
///
/// The generic parameter `M: CompletionModel` is held via `PhantomData`
/// because the eventual wiring will store `Arc<M>` plus the active
/// `ToolRegistry` / `ExecutorConfig` as fields; carrying the type
/// parameter today freezes the public type signature so downstream
/// readiness rows can reference it.
pub struct CompletionTaskExecutor<M: CompletionModel> {
    _phantom: std::marker::PhantomData<fn() -> M>,
}

impl<M: CompletionModel> CompletionTaskExecutor<M> {
    /// Construct a schema-only `CompletionTaskExecutor<M>`.
    pub fn new() -> Self {
        Self {
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<M: CompletionModel> Default for CompletionTaskExecutor<M> {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl<M: CompletionModel + Send + Sync + 'static> TaskExecutor for CompletionTaskExecutor<M> {
    async fn execute_task_attempt(
        &self,
        _context: TaskExecutionContext,
    ) -> Result<TaskExecutionResult, TaskExecutionError> {
        Err(TaskExecutionError::Provider(
            "CompletionTaskExecutor::execute_task_attempt is a Wave 1B schema-only stub; \
             the per-attempt completion-model + tool-loop wiring will land in a follow-up \
             patch (see src/internal/ai/runtime/task_executors.rs module docs)."
                .to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use uuid::Uuid;

    use super::*;
    use crate::internal::ai::{
        completion::{
            CompletionError, CompletionRequest, CompletionResponse, CompletionUsage,
            CompletionUsageSummary,
        },
        runtime::contracts::{ApprovalMediationState, PromptPackage, WorkflowPhase},
    };

    /// Minimal `CompletionModel` shim for the `dyn TaskExecutor` /
    /// `execute_task_attempt` stub assertions. The model is never actually
    /// invoked — its `.completion()` body is unreachable from the stub
    /// path — so the trait impl returns a sentinel error to make accidental
    /// real use easy to spot.
    #[derive(Clone)]
    struct FakeCompletionModel;

    #[derive(Clone, Debug)]
    struct FakeResponse;

    impl CompletionUsage for FakeResponse {
        fn usage_summary(&self) -> Option<CompletionUsageSummary> {
            None
        }
    }

    impl CompletionModel for FakeCompletionModel {
        type Response = FakeResponse;

        async fn completion(
            &self,
            _request: CompletionRequest,
        ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
            Err(CompletionError::ProviderError(
                "FakeCompletionModel: test-only model, .completion() should not be invoked"
                    .to_string(),
            ))
        }
    }

    fn dummy_prompt() -> PromptPackage {
        PromptPackage {
            phase: WorkflowPhase::Execution,
            provider: "fake".to_string(),
            model: "task-executors-test".to_string(),
            preamble: String::new(),
            messages: Vec::new(),
            readonly_tools: Vec::new(),
        }
    }

    fn dummy_context() -> TaskExecutionContext {
        TaskExecutionContext {
            thread_id: Uuid::new_v4(),
            task_id: Uuid::new_v4(),
            run_id: None,
            working_dir: PathBuf::from("/tmp"),
            prompt: dummy_prompt(),
            approval: ApprovalMediationState::LegacyInteractive,
        }
    }

    /// `CodexTaskExecutor::execute_task_attempt` must return a structured
    /// `TaskExecutionError::Provider` rather than `Ok(...)`. The error
    /// message must mention "Wave 1B schema-only stub" so a future
    /// reviewer encountering the error in logs can trace it back to this
    /// module.
    #[tokio::test]
    async fn codex_task_executor_attempt_returns_schema_only_stub_error() {
        let executor = CodexTaskExecutor::new();
        let result = executor.execute_task_attempt(dummy_context()).await;

        let error = result.expect_err("schema-only stub must return Err");
        let TaskExecutionError::Provider(message) = error else {
            panic!("expected TaskExecutionError::Provider, got: {error:?}");
        };
        assert!(
            message.contains("CodexTaskExecutor"),
            "error message must self-identify (got {message:?})"
        );
        assert!(
            message.contains("Wave 1B schema-only stub"),
            "error message must mark itself as Wave 1B stub (got {message:?})"
        );
    }

    /// `CompletionTaskExecutor<M>::execute_task_attempt` must return a
    /// structured `TaskExecutionError::Provider` with the same
    /// self-identification + Wave 1B markers as the Codex executor.
    #[tokio::test]
    async fn completion_task_executor_attempt_returns_schema_only_stub_error() {
        let executor: CompletionTaskExecutor<FakeCompletionModel> = CompletionTaskExecutor::new();
        let result = executor.execute_task_attempt(dummy_context()).await;

        let error = result.expect_err("schema-only stub must return Err");
        let TaskExecutionError::Provider(message) = error else {
            panic!("expected TaskExecutionError::Provider, got: {error:?}");
        };
        assert!(
            message.contains("CompletionTaskExecutor"),
            "error message must self-identify (got {message:?})"
        );
        assert!(
            message.contains("Wave 1B schema-only stub"),
            "error message must mark itself as Wave 1B stub (got {message:?})"
        );
    }

    /// The Wave 1B Definition of Done #1 requires both impl blocks to be
    /// dyn-compatible with `TaskExecutor`; this test asserts both can be
    /// placed behind a `Box<dyn TaskExecutor>` without compile errors.
    #[test]
    fn both_executors_are_dyn_compatible() {
        let _codex: Box<dyn TaskExecutor> = Box::new(CodexTaskExecutor::new());
        let _completion: Box<dyn TaskExecutor> =
            Box::new(CompletionTaskExecutor::<FakeCompletionModel>::new());
    }
}
