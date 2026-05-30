//! Wave 1B `TaskExecutor` adapter structs.
//!
//! The Code UI Phase Workflow's Wave 1B "Definition of Done #1" requires
//! that **both** providers — Codex (`CodexTaskExecutor`) and any generic
//! completion-model provider (`CompletionTaskExecutor<M>`) — implement the
//! shared [`TaskExecutor`] trait from
//! [`crate::internal::ai::runtime::contracts`] so the runtime can address
//! all task executors through a single trait object.
//!
//! This module landed the shared impl shapes first. The Codex executor
//! remains schema-only and returns a structured [`TaskExecutionError::Provider`]
//! pointing at the substantive wiring work. The generic completion executor
//! now has a minimal single-shot body for no-tool tasks; it calls the provider,
//! stitches assistant text into the task summary, and fails closed if the
//! response asks for tool execution that this minimal adapter cannot mediate.
//! The remaining body fill-in has to:
//!
//! - **Codex path**: take the existing Codex app-server WebSocket driver
//!   (today living inside `src/internal/ai/codex/mod.rs::
//!   CodexCodeUiAdapter`), extract the per-attempt slice into a free
//!   function, and have `CodexTaskExecutor::execute_task_attempt`
//!   delegate to it.
//! - **Completion path**: take the existing tool-loop runtime, build a
//!   tool-enabled task from `TaskExecutionContext`, and route the completion
//!   model + tool registry through sandbox and approval mediation.
//!
//! The remaining Codex WebSocket adapter and full completion tool-loop are
//! non-trivial cross-cutting refactors. Splitting impl-shape, no-tool
//! completion execution, and full tool-enabled execution across patches keeps
//! downstream readiness rows moving without misrepresenting either executor as
//! production-ready for every task shape.

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

/// Minimal `TaskExecutor` adapter that calls a generic
/// [`CompletionModel`] for a single task attempt.
///
/// **State (v0.17.1106):** Implementation now invokes the provider in a
/// **single-shot, tool-loop-less** mode — it builds a [`CompletionRequest`]
/// from the [`TaskExecutionContext::prompt`], calls
/// [`CompletionModel::completion`], stitches the assistant text into a
/// summary, and returns a `Completed` [`TaskExecutionResult`] only when
/// the response does not request tool execution. Tool-call responses fail
/// closed with [`TaskExecutionError::ToolPolicy`] so this minimal adapter
/// cannot silently mark an unmediated tool request as complete. This is the
/// first real wiring on the `TaskExecutor` trait; it's deliberately
/// minimal so the trait contract can be exercised end-to-end in tests
/// without bringing the full tool-loop / sandbox / approval pipeline
/// along.
///
/// The full wiring (tool-loop dispatch, sandbox guards, approval
/// mediation) is a separate cross-cutting follow-up. The minimal body
/// here is sufficient for:
///
/// - Driving baseline regression tests through the trait surface.
/// - Letting `dagrs`-based task scheduling actually invoke a provider
///   for tasks that don't require tools (e.g. classifier / verifier
///   tasks that just need a text response).
///
/// The generic parameter `M: CompletionModel` is wrapped in `Arc<M>` so
/// the executor is cheap to clone and the model can be shared across
/// concurrent task attempts.
pub struct CompletionTaskExecutor<M: CompletionModel> {
    model: std::sync::Arc<M>,
}

impl<M: CompletionModel> CompletionTaskExecutor<M> {
    /// Construct a `CompletionTaskExecutor<M>` over the given model.
    pub fn new(model: M) -> Self {
        Self {
            model: std::sync::Arc::new(model),
        }
    }

    /// Construct from an already-shared `Arc<M>`. Useful when multiple
    /// executors / pipelines share the same provider instance.
    pub fn from_arc(model: std::sync::Arc<M>) -> Self {
        Self { model }
    }
}

#[async_trait]
impl<M: CompletionModel + Send + Sync + 'static> TaskExecutor for CompletionTaskExecutor<M> {
    async fn execute_task_attempt(
        &self,
        context: TaskExecutionContext,
    ) -> Result<TaskExecutionResult, TaskExecutionError> {
        use crate::internal::ai::{
            completion::{CompletionRequest, Message},
            runtime::contracts::TaskExecutionStatus,
        };

        // Pre-assign a run_id when the context didn't carry one. This
        // matches the contract that every returned TaskExecutionResult
        // identifies its run by a real UUID rather than leaving it for
        // the caller to backfill.
        let run_id = context.run_id.unwrap_or_else(uuid::Uuid::new_v4);

        // Build a minimal CompletionRequest from the prompt package.
        // The package's `preamble` becomes the system/preamble field;
        // its `messages` are mapped onto user-role chat turns. Tool
        // definitions and richer chat-history reconstruction live in
        // the full tool-loop integration follow-up.
        let preamble = if context.prompt.preamble.is_empty() {
            None
        } else {
            Some(context.prompt.preamble.clone())
        };
        let chat_history: Vec<Message> = context
            .prompt
            .messages
            .iter()
            .map(|text| Message::user(text.clone()))
            .collect();
        let request = CompletionRequest {
            preamble,
            chat_history,
            ..CompletionRequest::default()
        };

        let response = self.model.completion(request).await.map_err(|err| {
            TaskExecutionError::Provider(format!("completion model error: {err}"))
        })?;

        let tool_calls = response
            .content
            .iter()
            .filter_map(|segment| match segment {
                crate::internal::ai::completion::AssistantContent::ToolCall(tool_call) => {
                    let name = if tool_call.function.name.is_empty() {
                        tool_call.name.as_str()
                    } else {
                        tool_call.function.name.as_str()
                    };
                    Some(name)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        if !tool_calls.is_empty() {
            return Err(TaskExecutionError::ToolPolicy(format!(
                "completion task executor received tool call(s) [{}], but this minimal executor \
                 cannot run tools; route this task through the full tool-loop executor or disable \
                 tool calls for this executor",
                tool_calls.join(", ")
            )));
        }

        // Stitch the assistant content into a summary. Only Text
        // segments contribute. ToolCall segments were rejected above so
        // this minimal body cannot silently mark a tool-requesting task as
        // completed. The summary stays None when no text segments came
        // back so callers can distinguish "model ran but produced no
        // text" from "model returned a non-empty response".
        let summary_text = response
            .content
            .iter()
            .filter_map(|segment| match segment {
                crate::internal::ai::completion::AssistantContent::Text(text) => {
                    Some(text.text.as_str())
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("");
        let summary = if summary_text.is_empty() {
            None
        } else {
            Some(summary_text)
        };

        Ok(TaskExecutionResult {
            task_id: context.task_id,
            run_id,
            status: TaskExecutionStatus::Completed,
            evidence: Vec::new(),
            summary,
        })
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

    /// Test fixture: returns the configured assistant text as a single
    /// `AssistantContent::Text` segment. Captures the request that was
    /// passed in so tests can assert preamble / message threading.
    #[derive(Clone)]
    struct ScriptedCompletionModel {
        reply: String,
        captured_request: std::sync::Arc<tokio::sync::Mutex<Option<CompletionRequest>>>,
    }

    impl ScriptedCompletionModel {
        fn new(reply: impl Into<String>) -> Self {
            Self {
                reply: reply.into(),
                captured_request: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            }
        }
    }

    #[derive(Clone, Debug)]
    struct ScriptedResponse;

    impl CompletionUsage for ScriptedResponse {
        fn usage_summary(&self) -> Option<CompletionUsageSummary> {
            None
        }
    }

    impl CompletionModel for ScriptedCompletionModel {
        type Response = ScriptedResponse;

        async fn completion(
            &self,
            request: CompletionRequest,
        ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
            *self.captured_request.lock().await = Some(request);
            use crate::internal::ai::completion::{AssistantContent, Text};
            Ok(CompletionResponse {
                content: vec![AssistantContent::Text(Text {
                    text: self.reply.clone(),
                })],
                reasoning_content: None,
                raw_response: ScriptedResponse,
            })
        }
    }

    /// Test fixture: returns a `CompletionError::ProviderError` so
    /// `CompletionTaskExecutor` exposes its `Provider`-error mapping.
    #[derive(Clone)]
    struct ErroringCompletionModel;

    impl CompletionModel for ErroringCompletionModel {
        type Response = ScriptedResponse;

        async fn completion(
            &self,
            _request: CompletionRequest,
        ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
            Err(CompletionError::ProviderError(
                "scripted error: backend exploded".to_string(),
            ))
        }
    }

    /// Test fixture: returns a tool call. The minimal completion executor
    /// must reject this because it does not run the tool-loop / approval
    /// mediation path.
    #[derive(Clone)]
    struct ToolCallingCompletionModel;

    impl CompletionModel for ToolCallingCompletionModel {
        type Response = ScriptedResponse;

        async fn completion(
            &self,
            _request: CompletionRequest,
        ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
            use crate::internal::ai::completion::{AssistantContent, Function, ToolCall};

            Ok(CompletionResponse {
                content: vec![AssistantContent::ToolCall(ToolCall {
                    id: "call_1".to_string(),
                    name: "run_shell".to_string(),
                    function: Function {
                        name: "run_shell".to_string(),
                        arguments: serde_json::json!({ "cmd": "pwd" }),
                    },
                })],
                reasoning_content: None,
                raw_response: ScriptedResponse,
            })
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

    /// `CompletionTaskExecutor<M>::execute_task_attempt` invokes the
    /// underlying model and stitches the assistant text into the
    /// result's `summary`. Happy path: scripted model returns a
    /// single text segment; the executor returns
    /// `TaskExecutionStatus::Completed` with that text as the summary
    /// and threads `task_id` through unchanged.
    #[tokio::test]
    async fn completion_task_executor_returns_completed_with_assistant_text_summary() {
        use crate::internal::ai::runtime::contracts::TaskExecutionStatus;

        let model = ScriptedCompletionModel::new("hello from the test fixture");
        let executor = CompletionTaskExecutor::new(model);
        let context = dummy_context();
        let task_id = context.task_id;

        let result = executor
            .execute_task_attempt(context)
            .await
            .expect("scripted model returns Ok");

        assert_eq!(result.task_id, task_id);
        assert_eq!(result.status, TaskExecutionStatus::Completed);
        assert_eq!(
            result.summary.as_deref(),
            Some("hello from the test fixture")
        );
        assert!(result.evidence.is_empty());
    }

    /// When the context carries `run_id = None`, the executor must
    /// allocate a fresh UUID for `run_id` rather than leaving the
    /// result un-identified. Callers shouldn't have to backfill the
    /// run_id after the fact.
    #[tokio::test]
    async fn completion_task_executor_allocates_run_id_when_context_lacks_one() {
        let model = ScriptedCompletionModel::new("ok");
        let executor = CompletionTaskExecutor::new(model);
        let mut context = dummy_context();
        context.run_id = None;

        let result = executor.execute_task_attempt(context).await.unwrap();
        assert_ne!(result.run_id, Uuid::nil());
    }

    /// When the context carries `run_id = Some(id)`, the executor
    /// must thread it through verbatim — observers correlate the
    /// result back to the originating attempt by run_id.
    #[tokio::test]
    async fn completion_task_executor_preserves_run_id_from_context() {
        let model = ScriptedCompletionModel::new("ok");
        let executor = CompletionTaskExecutor::new(model);
        let run_id = Uuid::new_v4();
        let mut context = dummy_context();
        context.run_id = Some(run_id);

        let result = executor.execute_task_attempt(context).await.unwrap();
        assert_eq!(result.run_id, run_id);
    }

    /// The executor must thread the prompt package's `preamble` and
    /// `messages` into the `CompletionRequest` it builds. Captures the
    /// request via the `ScriptedCompletionModel.captured_request` and
    /// asserts both fields are populated as expected.
    #[tokio::test]
    async fn completion_task_executor_threads_prompt_into_completion_request() {
        let model = ScriptedCompletionModel::new("reply");
        let captured = model.captured_request.clone();
        let executor = CompletionTaskExecutor::new(model);

        let mut context = dummy_context();
        context.prompt.preamble = "you are a test helper".to_string();
        context.prompt.messages = vec!["msg-1".to_string(), "msg-2".to_string()];

        executor.execute_task_attempt(context).await.unwrap();

        let captured = captured.lock().await;
        let request = captured.as_ref().expect("model must have been invoked");
        assert_eq!(request.preamble.as_deref(), Some("you are a test helper"));
        assert_eq!(request.chat_history.len(), 2);
    }

    /// An empty prompt package (no preamble, no messages) must
    /// translate to `CompletionRequest { preamble: None,
    /// chat_history: vec![], .. }`. Pins the "absent fields are
    /// None/empty, not empty-string sentinels" boundary.
    #[tokio::test]
    async fn completion_task_executor_maps_empty_prompt_to_empty_request() {
        let model = ScriptedCompletionModel::new("ok");
        let captured = model.captured_request.clone();
        let executor = CompletionTaskExecutor::new(model);

        executor
            .execute_task_attempt(dummy_context())
            .await
            .unwrap();

        let captured = captured.lock().await;
        let request = captured.as_ref().expect("model must have been invoked");
        assert_eq!(request.preamble, None);
        assert!(request.chat_history.is_empty());
    }

    /// The minimal completion executor is not allowed to silently drop
    /// provider tool calls and report `Completed`. Until the full tool-loop
    /// integration lands, tool-call responses must fail closed at the task
    /// boundary with a message that tells callers which executor to use.
    #[tokio::test]
    async fn completion_task_executor_rejects_tool_calls_without_tool_loop() {
        let executor = CompletionTaskExecutor::new(ToolCallingCompletionModel);
        let error = executor
            .execute_task_attempt(dummy_context())
            .await
            .expect_err("tool calls are unsupported in the minimal executor");
        let TaskExecutionError::ToolPolicy(message) = error else {
            panic!("expected TaskExecutionError::ToolPolicy, got: {error:?}");
        };

        assert!(
            message.contains("run_shell"),
            "tool name should be included for diagnostics (got {message:?})"
        );
        assert!(
            message.contains("full tool-loop executor"),
            "message should direct callers to the supported tool path (got {message:?})"
        );
    }

    /// A model error must be mapped to
    /// `TaskExecutionError::Provider` so callers can route on the
    /// existing executor-error variants without a new typed wrapper.
    #[tokio::test]
    async fn completion_task_executor_maps_model_error_to_provider_error() {
        let executor = CompletionTaskExecutor::new(ErroringCompletionModel);
        let error = executor
            .execute_task_attempt(dummy_context())
            .await
            .expect_err("ErroringCompletionModel must surface as Err");
        let TaskExecutionError::Provider(message) = error else {
            panic!("expected TaskExecutionError::Provider, got: {error:?}");
        };
        assert!(
            message.contains("backend exploded"),
            "underlying error reason should be preserved (got {message:?})"
        );
    }

    /// The Wave 1B Definition of Done #1 requires both impl blocks to be
    /// dyn-compatible with `TaskExecutor`; this test asserts both can be
    /// placed behind a `Box<dyn TaskExecutor>` without compile errors.
    #[test]
    fn both_executors_are_dyn_compatible() {
        let _codex: Box<dyn TaskExecutor> = Box::new(CodexTaskExecutor::new());
        let _completion: Box<dyn TaskExecutor> = Box::new(CompletionTaskExecutor::new(
            ScriptedCompletionModel::new("unused"),
        ));
    }
}
