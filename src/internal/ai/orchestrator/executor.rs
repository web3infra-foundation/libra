use std::{
    collections::{HashMap, HashSet},
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use dagrs::{
    Action, CheckpointConfig, CompletionStatus, DefaultNode, EnvVar, FileCheckpointStore, Graph,
    InChannels, Node, NodeTable, OutChannels, Output, event::GraphEvent,
};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::Semaphore;
use uuid::Uuid;

use super::{
    acl::{AclVerdict, check_tool_acl},
    checkpoint_policy::dagrs_checkpointing_enabled,
    gate, policy,
    run_state::{RunStateSnapshot, RunStateStore},
    types::{
        ExecutionPlanSpec, GateReport, OrchestratorError, OrchestratorObserver, ReviewOutcome,
        TaskKind, TaskNodeStatus, TaskResult, TaskRuntimeEvent, TaskRuntimeNoteLevel,
        TaskRuntimePhase, TaskSpec, ToolCallRecord,
    },
};
use crate::internal::ai::{
    agent::{ToolLoopConfig, ToolLoopObserver, run_tool_loop_with_history_and_observer},
    completion::{
        CompletionError, CompletionModel, CompletionStreamEvent, CompletionUsage,
        CompletionUsageSummary,
    },
    hooks::HookRunner,
    intentspec::types::{IntentSpec, NetworkPolicy, ToolAcl},
    libra_vcs::run_libra_vcs_tool_guidance,
    runtime::environment::{ExecutionEnvironmentProvider, SyncBackRequest},
    sandbox::{
        NetworkAccess, SandboxPermissions, SandboxPolicy, ToolRuntimeContext, ToolSandboxContext,
    },
    tools::{ToolOutput, registry::ToolRegistry},
};

/// Configuration for task execution.
#[derive(Clone)]
pub struct ExecutorConfig {
    pub tool_loop_config: ToolLoopConfig,
    pub max_retries: u8,
    pub backoff_seconds: u32,
    pub working_dir: PathBuf,
    pub spec: Arc<IntentSpec>,
    pub reviewer_preamble: Option<String>,
    pub dagrs_resume_checkpoint_id: Option<String>,
    pub observer: Option<Arc<dyn OrchestratorObserver>>,
}

struct TaskExecutionObserver {
    spec: Arc<IntentSpec>,
    task: TaskSpec,
    working_dir: PathBuf,
    in_flight: HashMap<String, ToolCallRecord>,
    tool_calls: Vec<ToolCallRecord>,
    violations: Vec<super::types::PolicyViolation>,
    model_usage: CompletionUsageSummary,
    observer: Option<Arc<dyn OrchestratorObserver>>,
}

impl TaskExecutionObserver {
    fn new(
        spec: Arc<IntentSpec>,
        task: TaskSpec,
        working_dir: PathBuf,
        observer: Option<Arc<dyn OrchestratorObserver>>,
    ) -> Self {
        Self {
            spec,
            task,
            working_dir,
            in_flight: HashMap::new(),
            tool_calls: Vec::new(),
            violations: Vec::new(),
            model_usage: CompletionUsageSummary::default(),
            observer,
        }
    }

    fn finish(
        self,
    ) -> (
        Vec<ToolCallRecord>,
        Vec<super::types::PolicyViolation>,
        Option<CompletionUsageSummary>,
    ) {
        let usage = (!self.model_usage.is_zero()).then_some(self.model_usage);
        (self.tool_calls, self.violations, usage)
    }
}

impl ToolLoopObserver for TaskExecutionObserver {
    fn on_model_turn_start(&mut self, turn: usize) {
        if let Some(observer) = &self.observer {
            observer.on_task_runtime_event(
                &self.task,
                TaskRuntimeEvent::Phase(TaskRuntimePhase::AwaitingModel { turn }),
            );
        }
    }

    fn on_assistant_step_text(&mut self, text: &str) {
        if let Some(observer) = &self.observer {
            observer.on_task_runtime_event(
                &self.task,
                TaskRuntimeEvent::AssistantMessage(text.to_string()),
            );
        }
    }

    fn on_model_usage(&mut self, usage: &CompletionUsageSummary) {
        self.model_usage.merge(usage);
    }

    fn on_model_stream_event(&mut self, event: &CompletionStreamEvent) {
        if let CompletionStreamEvent::ThinkingDelta { delta, .. } = event
            && !delta.is_empty()
            && let Some(observer) = &self.observer
        {
            observer
                .on_task_runtime_event(&self.task, TaskRuntimeEvent::ThinkingDelta(delta.clone()));
        }
    }

    fn on_tool_call_begin(&mut self, call_id: &str, tool_name: &str, arguments: &Value) {
        if let Some(observer) = &self.observer {
            observer.on_task_runtime_event(
                &self.task,
                TaskRuntimeEvent::Phase(TaskRuntimePhase::ExecutingTool {
                    tool_name: tool_name.to_string(),
                }),
            );
            observer.on_task_runtime_event(
                &self.task,
                TaskRuntimeEvent::ToolCallBegin {
                    call_id: call_id.to_string(),
                    tool_name: tool_name.to_string(),
                    arguments: arguments.clone(),
                },
            );
        }
    }

    fn on_tool_call_preflight(
        &mut self,
        call_id: &str,
        tool_name: &str,
        arguments: &Value,
    ) -> Result<(), String> {
        match policy::evaluate_tool_call(
            &self.spec,
            &self.task,
            tool_name,
            arguments,
            &self.working_dir,
        ) {
            Ok(preflight) => {
                self.in_flight.insert(call_id.to_string(), preflight.record);
                Ok(())
            }
            Err(violation) => {
                self.violations.push(violation.clone());
                Err(violation.message)
            }
        }
    }

    fn on_tool_call_end(
        &mut self,
        call_id: &str,
        tool_name: &str,
        result: &Result<ToolOutput, String>,
    ) {
        if let Some(observer) = &self.observer {
            observer.on_task_runtime_event(
                &self.task,
                TaskRuntimeEvent::ToolCallEnd {
                    call_id: call_id.to_string(),
                    tool_name: tool_name.to_string(),
                    result: result.clone(),
                },
            );
        }
        if let Some(mut record) = self.in_flight.remove(call_id) {
            match result {
                Ok(output) => {
                    if let Err(violation) = policy::evaluate_tool_result(
                        &self.spec,
                        &self.task,
                        tool_name,
                        output,
                        &mut record,
                    ) {
                        self.violations.push(violation);
                    }
                }
                Err(message) => {
                    record.success = false;
                    record.summary = Some(message.clone());
                }
            }
            self.tool_calls.push(record);
        }
    }
}

#[derive(Debug, Deserialize)]
struct ReviewerDecision {
    approved: bool,
    summary: String,
    #[serde(default)]
    issues: Vec<String>,
}

struct ReviewerPassArtifacts {
    outcome: Result<Option<ReviewOutcome>, String>,
    tool_calls: Vec<ToolCallRecord>,
    policy_violations: Vec<super::types::PolicyViolation>,
    model_usage: Option<CompletionUsageSummary>,
}

/// Execute a single task with retry logic.
pub async fn execute_task<M: CompletionModel>(
    task: &TaskSpec,
    model: &M,
    registry: &ToolRegistry,
    config: &ExecutorConfig,
) -> TaskResult
where
    M::Response: CompletionUsage,
{
    if task.kind == TaskKind::Gate {
        return execute_gate_task_in_task_worktree(
            task,
            &config.working_dir,
            &config.spec,
            config.tool_loop_config.runtime_context.as_ref(),
            config.observer.as_ref(),
        )
        .await;
    }

    let allowed_tools = allowed_tools_for_task(&config.spec, task);
    let runtime_context = runtime_context_for_task(
        &config.spec,
        task,
        &config.working_dir,
        config.tool_loop_config.runtime_context.as_ref(),
    );
    let prompt = build_task_prompt(task, &config.working_dir, &allowed_tools);
    let mut retry_count: u8 = 0;
    let mut accumulated_tool_calls = Vec::new();
    let mut accumulated_policy_violations = Vec::new();
    let mut accumulated_model_usage = CompletionUsageSummary::default();
    let mut last_review = None;
    let mut retry_feedback: Option<String> = None;

    loop {
        let mut observer = TaskExecutionObserver::new(
            Arc::clone(&config.spec),
            task.clone(),
            config.working_dir.clone(),
            config.observer.clone(),
        );
        let tool_loop_config = ToolLoopConfig {
            allowed_tools: Some(allowed_tools.clone()),
            runtime_context: Some(runtime_context.clone()),
            ..config.tool_loop_config.clone()
        };
        let attempt_prompt = match retry_feedback.as_deref() {
            Some(feedback) => format!(
                "{prompt}\n\n## Previous Attempt Failure\n{feedback}\nCorrect this failure before reporting completion."
            ),
            None => prompt.clone(),
        };
        let agent_result = run_tool_loop_with_history_and_observer(
            model,
            Vec::new(),
            &attempt_prompt,
            registry,
            tool_loop_config,
            &mut observer,
        )
        .await;
        let (tool_calls, policy_violations, model_usage) = observer.finish();
        accumulated_tool_calls.extend(tool_calls.iter().cloned());
        accumulated_policy_violations.extend(policy_violations.iter().cloned());
        if let Some(usage) = model_usage.as_ref() {
            accumulated_model_usage.merge(usage);
        }

        let retryable_failure = match agent_result {
            Ok(turn) if policy_violations.is_empty() => {
                if let Some(observer) = &config.observer
                    && !turn.final_text.trim().is_empty()
                {
                    observer.on_task_runtime_event(
                        task,
                        TaskRuntimeEvent::AssistantMessage(turn.final_text.clone()),
                    );
                }
                if let Some(reason) =
                    implementation_missing_write_output(task, &accumulated_tool_calls)
                {
                    (
                        Some(reason.clone()),
                        tool_calls,
                        policy_violations,
                        reason,
                        None,
                    )
                } else {
                    let review_artifacts = run_reviewer_pass(
                        task,
                        &turn.final_text,
                        &accumulated_tool_calls,
                        model,
                        registry,
                        config,
                    )
                    .await;
                    accumulated_tool_calls.extend(review_artifacts.tool_calls.iter().cloned());
                    accumulated_policy_violations
                        .extend(review_artifacts.policy_violations.iter().cloned());
                    if let Some(usage) = review_artifacts.model_usage.as_ref() {
                        accumulated_model_usage.merge(usage);
                    }
                    let review = match review_artifacts.outcome {
                        Ok(review) => review,
                        Err(message) => {
                            if let Some(observer) = &config.observer {
                                observer.on_task_runtime_event(
                                    task,
                                    TaskRuntimeEvent::Phase(TaskRuntimePhase::Failed),
                                );
                            }
                            return TaskResult {
                                task_id: task.id(),
                                status: TaskNodeStatus::Failed,
                                gate_report: None,
                                agent_output: Some(message),
                                retry_count,
                                tool_calls: accumulated_tool_calls,
                                policy_violations: accumulated_policy_violations,
                                model_usage: (!accumulated_model_usage.is_zero())
                                    .then_some(accumulated_model_usage),
                                review: None,
                            };
                        }
                    };
                    if let Some(review) = review.as_ref()
                        && !review.approved
                    {
                        last_review = Some(review.clone());
                        (
                            Some(turn.final_text),
                            tool_calls,
                            policy_violations,
                            format!("review rejected: {}", review.summary),
                            Some(review.clone()),
                        )
                    } else {
                        return TaskResult {
                            task_id: task.id(),
                            status: TaskNodeStatus::Completed,
                            gate_report: None,
                            agent_output: Some(turn.final_text),
                            retry_count,
                            tool_calls: accumulated_tool_calls,
                            policy_violations: accumulated_policy_violations,
                            model_usage: (!accumulated_model_usage.is_zero())
                                .then_some(accumulated_model_usage),
                            review,
                        };
                    }
                }
            }
            Ok(turn) => {
                let reason = policy_violations
                    .iter()
                    .map(|violation| violation.message.clone())
                    .collect::<Vec<_>>()
                    .join("; ");
                (
                    Some(turn.final_text),
                    tool_calls,
                    policy_violations,
                    reason,
                    None,
                )
            }
            Err(CompletionError::ResponseError(msg)) => {
                (Some(msg.clone()), tool_calls, policy_violations, msg, None)
            }
            Err(e) => {
                return TaskResult {
                    task_id: task.id(),
                    status: TaskNodeStatus::Failed,
                    gate_report: None,
                    agent_output: Some(e.to_string()),
                    retry_count,
                    tool_calls: accumulated_tool_calls,
                    policy_violations: accumulated_policy_violations,
                    model_usage: (!accumulated_model_usage.is_zero())
                        .then_some(accumulated_model_usage),
                    review: last_review,
                };
            }
        };

        retry_count += 1;
        let (agent_output, _, _, failure_reason, review) = retryable_failure;
        if let Some(review) = review {
            last_review = Some(review);
        }
        if retry_count > config.max_retries {
            return TaskResult {
                task_id: task.id(),
                status: TaskNodeStatus::Failed,
                gate_report: None,
                agent_output,
                retry_count,
                tool_calls: accumulated_tool_calls,
                policy_violations: accumulated_policy_violations,
                model_usage: (!accumulated_model_usage.is_zero())
                    .then_some(accumulated_model_usage),
                review: last_review,
            };
        }

        tracing::warn!(task_id = %task.id(), "retrying task after failure: {}", failure_reason);
        retry_feedback = Some(failure_reason);
        if config.backoff_seconds > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(
                config.backoff_seconds as u64,
            ))
            .await;
        }
    }
}

fn implementation_missing_write_output(
    task: &TaskSpec,
    tool_calls: &[ToolCallRecord],
) -> Option<String> {
    if task.kind != TaskKind::Implementation {
        return None;
    }

    let has_successful_write = tool_calls
        .iter()
        .any(|call| call.success && (!call.paths_written.is_empty() || !call.diffs.is_empty()));

    (!has_successful_write).then(|| {
        "implementation task completed without writing any files; use apply_patch or an allowed shell write to create or modify the expected project files before reporting completion"
            .to_string()
    })
}

async fn execute_gate_task(
    task: &TaskSpec,
    working_dir: &Path,
    spec: &IntentSpec,
    inherited_runtime: Option<&ToolRuntimeContext>,
    observer: Option<&Arc<dyn OrchestratorObserver>>,
) -> TaskResult {
    let runtime_context = runtime_context_for_gate_task(spec, working_dir, inherited_runtime);
    let gate_report = if task.checks.is_empty() {
        GateReport::empty()
    } else {
        let mut results = Vec::with_capacity(task.checks.len());
        let mut all_required_passed = true;

        for check in &task.checks {
            if let Some(observer) = observer {
                observer.on_task_runtime_event(
                    task,
                    TaskRuntimeEvent::Note {
                        level: TaskRuntimeNoteLevel::Info,
                        text: format!("gate running · {}", check.id),
                    },
                );
            }

            let result = gate::run_check_with_context(
                check,
                working_dir,
                Some(spec),
                Some(task),
                Some(&runtime_context),
            )
            .await;

            if let Some(observer) = observer {
                observer.on_task_runtime_event(
                    task,
                    TaskRuntimeEvent::Note {
                        level: if result.passed {
                            TaskRuntimeNoteLevel::Info
                        } else {
                            TaskRuntimeNoteLevel::Error
                        },
                        text: format!(
                            "gate {} · {} · exit {}",
                            if result.passed { "passed" } else { "failed" },
                            check.id,
                            result.exit_code
                        ),
                    },
                );
            }

            if !result.passed && check.required {
                all_required_passed = false;
            }
            results.push(result);
        }

        GateReport {
            results,
            all_required_passed,
        }
    };

    TaskResult {
        task_id: task.id(),
        status: if gate_report.all_required_passed {
            TaskNodeStatus::Completed
        } else {
            TaskNodeStatus::Failed
        },
        gate_report: Some(gate_report),
        agent_output: None,
        retry_count: 0,
        tool_calls: Vec::new(),
        policy_violations: Vec::new(),
        model_usage: None,
        review: None,
    }
}

async fn execute_gate_task_in_task_worktree(
    task: &TaskSpec,
    working_dir: &Path,
    spec: &IntentSpec,
    inherited_runtime: Option<&ToolRuntimeContext>,
    observer: Option<&Arc<dyn OrchestratorObserver>>,
) -> TaskResult {
    let environment_provider = ExecutionEnvironmentProvider;
    let environment = match environment_provider
        .provision_task_worktree(working_dir.to_path_buf(), task.id())
        .await
    {
        Ok(environment) => environment,
        Err(err) => return task_workspace_failure(task, err),
    };
    let task_worktree_root = environment.root().to_path_buf();

    if let Some(observer) = observer {
        observer.on_task_runtime_event(
            task,
            TaskRuntimeEvent::WorkspaceReady {
                working_dir: task_worktree_root.clone(),
                isolated: true,
            },
        );
    }

    let result =
        execute_gate_task(task, &task_worktree_root, spec, inherited_runtime, observer).await;

    if let Err(err) = environment_provider.cleanup(environment).await {
        tracing::warn!(
            path = %task_worktree_root.display(),
            "failed to clean up gate worktree: {}",
            err
        );
    }

    result
}

async fn run_reviewer_pass<M: CompletionModel>(
    task: &TaskSpec,
    agent_output: &str,
    tool_calls: &[ToolCallRecord],
    model: &M,
    registry: &ToolRegistry,
    config: &ExecutorConfig,
) -> ReviewerPassArtifacts
where
    M::Response: CompletionUsage,
{
    let Some(reviewer_preamble) = config.reviewer_preamble.clone() else {
        return ReviewerPassArtifacts {
            outcome: Ok(None),
            tool_calls: Vec::new(),
            policy_violations: Vec::new(),
            model_usage: None,
        };
    };

    let allowed_tools = allowed_tools_for_reviewer(&config.spec);
    let review_prompt = build_reviewer_prompt(
        task,
        agent_output,
        tool_calls,
        &config.working_dir,
        &allowed_tools,
    );
    let review_config = ToolLoopConfig {
        preamble: Some(reviewer_preamble),
        allowed_tools: Some(allowed_tools),
        runtime_context: Some(runtime_context_for_reviewer(
            &config.spec,
            config.tool_loop_config.runtime_context.as_ref(),
        )),
        ..config.tool_loop_config.clone()
    };

    if let Some(observer) = &config.observer {
        observer.on_task_runtime_event(task, TaskRuntimeEvent::Phase(TaskRuntimePhase::Reviewing));
    }
    let mut observer = TaskExecutionObserver::new(
        Arc::clone(&config.spec),
        task.clone(),
        config.working_dir.clone(),
        config.observer.clone(),
    );
    let turn = run_tool_loop_with_history_and_observer(
        model,
        Vec::new(),
        &review_prompt,
        registry,
        review_config,
        &mut observer,
    )
    .await;
    let (tool_calls, policy_violations, model_usage) = observer.finish();
    let turn = match turn {
        Ok(turn) => turn,
        Err(err) => {
            return ReviewerPassArtifacts {
                outcome: Err(format!("reviewer pass failed: {err}")),
                tool_calls,
                policy_violations,
                model_usage,
            };
        }
    };

    let review = match parse_reviewer_decision(&turn.final_text) {
        Ok(review) => review,
        Err(error) => {
            return ReviewerPassArtifacts {
                outcome: Err(error),
                tool_calls,
                policy_violations,
                model_usage,
            };
        }
    };
    let outcome = ReviewOutcome {
        approved: review.approved,
        summary: review.summary,
        issues: review.issues,
    };
    if let Some(observer) = &config.observer {
        observer.on_task_runtime_event(
            task,
            TaskRuntimeEvent::Note {
                level: if outcome.approved {
                    TaskRuntimeNoteLevel::Info
                } else {
                    TaskRuntimeNoteLevel::Error
                },
                text: format!(
                    "review {} · {}",
                    if outcome.approved {
                        "approved"
                    } else {
                        "rejected"
                    },
                    outcome.summary
                ),
            },
        );
    }
    ReviewerPassArtifacts {
        outcome: Ok(Some(outcome)),
        tool_calls,
        policy_violations,
        model_usage,
    }
}

fn clone_tool_loop_config_for_workdir(
    config: &ToolLoopConfig,
    working_dir: &Path,
) -> ToolLoopConfig {
    let mut cloned = config.clone();
    if cloned.hook_runner.is_some() {
        cloned.hook_runner = Some(Arc::new(HookRunner::load(working_dir)));
    }
    cloned
}

fn should_use_task_worktree(task: &TaskSpec) -> bool {
    task.kind == TaskKind::Implementation
}

async fn execute_task_in_task_worktree<M: CompletionModel>(
    task: &TaskSpec,
    model: &M,
    registry: &Arc<ToolRegistry>,
    config: &ExecutorConfig,
    workspace_sync: &Arc<tokio::sync::Mutex<()>>,
) -> TaskResult
where
    M::Response: CompletionUsage,
{
    let environment_provider = ExecutionEnvironmentProvider;
    let environment = match environment_provider
        .provision_task_worktree(config.working_dir.clone(), task.id())
        .await
    {
        Ok(environment) => environment,
        Err(err) => return task_workspace_failure(task, err),
    };
    let task_worktree_root = environment.root().to_path_buf();

    let task_registry = Arc::new(registry.clone_with_working_dir(task_worktree_root.clone()));
    let mut task_config = config.clone();
    task_config.working_dir = task_worktree_root.clone();
    task_config.tool_loop_config =
        clone_tool_loop_config_for_workdir(&config.tool_loop_config, &task_worktree_root);
    if let Some(observer) = &config.observer {
        observer.on_task_runtime_event(
            task,
            TaskRuntimeEvent::WorkspaceReady {
                working_dir: task_worktree_root.clone(),
                isolated: true,
            },
        );
    }

    let mut result = execute_task(task, model, &task_registry, &task_config).await;

    if result.status == TaskNodeStatus::Completed {
        let sync_result = {
            let _guard = workspace_sync.lock().await;
            environment_provider
                .sync_back(
                    &environment,
                    SyncBackRequest {
                        main_working_dir: config.working_dir.clone(),
                        touch_files: task.contract.touch_files.clone(),
                        scope_in: task.scope_in.clone(),
                        scope_out: task.scope_out.clone(),
                    },
                )
                .await
        };

        match sync_result {
            Ok(()) => {}
            Err(err) => {
                result.status = TaskNodeStatus::Failed;
                result.agent_output = Some(format!(
                    "task completed in isolated worktree but failed to sync changes back: {err}"
                ));
            }
        }
    }

    if let Err(err) = environment_provider.cleanup(environment).await {
        tracing::warn!(
            path = %task_worktree_root.display(),
            "failed to clean up task worktree: {}",
            err
        );
    }

    result
}

fn task_workspace_failure(task: &TaskSpec, err: io::Error) -> TaskResult {
    TaskResult {
        task_id: task.id(),
        status: TaskNodeStatus::Failed,
        gate_report: None,
        agent_output: Some(format!("failed to prepare isolated worktree: {err}")),
        retry_count: 0,
        tool_calls: Vec::new(),
        policy_violations: Vec::new(),
        model_usage: None,
        review: None,
    }
}

fn terminal_task_result(
    task: &TaskSpec,
    status: TaskNodeStatus,
    agent_output: impl Into<Option<String>>,
) -> TaskResult {
    TaskResult {
        task_id: task.id(),
        status,
        gate_report: None,
        agent_output: agent_output.into(),
        retry_count: 0,
        tool_calls: Vec::new(),
        policy_violations: Vec::new(),
        model_usage: None,
        review: None,
    }
}

async fn record_terminal_result(
    run_state: &RunStateStore,
    observer: Option<&Arc<dyn OrchestratorObserver>>,
    task: &TaskSpec,
    result: TaskResult,
) {
    if let Some(observer) = observer {
        observer.on_task_runtime_event(
            task,
            TaskRuntimeEvent::Phase(match result.status {
                TaskNodeStatus::Completed => TaskRuntimePhase::Completed,
                TaskNodeStatus::Failed => TaskRuntimePhase::Failed,
                _ => TaskRuntimePhase::Pending,
            }),
        );
    }
    run_state.record_result(result).await;
}

struct TaskDagrsAction<M: CompletionModel + 'static> {
    task: TaskSpec,
    model: M,
    registry: Arc<ToolRegistry>,
    config: ExecutorConfig,
    run_state: RunStateStore,
    metered_task_ids: Arc<HashSet<Uuid>>,
    parallelism: Arc<Semaphore>,
    cost_budget_serial: Arc<tokio::sync::Mutex<()>>,
    workspace_sync_serial: Arc<tokio::sync::Mutex<()>>,
}

#[derive(Clone)]
struct DagrsBuildContext {
    run_state: RunStateStore,
    metered_task_ids: Arc<HashSet<Uuid>>,
    parallelism: Arc<Semaphore>,
    cost_budget_serial: Arc<tokio::sync::Mutex<()>>,
    workspace_sync_serial: Arc<tokio::sync::Mutex<()>>,
}

#[derive(Clone)]
struct DagrsDependencySignal {
    success: bool,
}

async fn broadcast_dependency_signal(out_channels: &mut OutChannels, success: bool) {
    let _ = out_channels
        .broadcast(dagrs::Content::new(DagrsDependencySignal { success }))
        .await;
}

#[async_trait]
impl<M: CompletionModel + 'static> Action for TaskDagrsAction<M>
where
    M::Response: CompletionUsage,
{
    async fn run(
        &self,
        in_channels: &mut InChannels,
        out_channels: &mut OutChannels,
        _env: Arc<EnvVar>,
    ) -> Output {
        let sender_ids = in_channels.get_sender_ids();
        let mut dependencies_ok = true;
        for sender_id in sender_ids {
            match in_channels.recv_from(&sender_id).await {
                Ok(content) => {
                    let signal = content.into_inner::<DagrsDependencySignal>();
                    if signal.as_deref().is_none_or(|signal| !signal.success) {
                        dependencies_ok = false;
                    }
                }
                Err(_) => {
                    dependencies_ok = false;
                }
            }
        }

        if !dependencies_ok {
            record_terminal_result(
                &self.run_state,
                self.config.observer.as_ref(),
                &self.task,
                terminal_task_result(
                    &self.task,
                    TaskNodeStatus::Skipped,
                    Some(
                        "skipped because an upstream dependency did not complete successfully"
                            .into(),
                    ),
                ),
            )
            .await;
            broadcast_dependency_signal(out_channels, false).await;
            return Output::empty();
        }

        let _parallel_permit = match Arc::clone(&self.parallelism).acquire_owned().await {
            Ok(permit) => permit,
            Err(err) => {
                let message = format!(
                    "failed to acquire execution permit for task {}: {}",
                    self.task.title(),
                    err
                );
                record_terminal_result(
                    &self.run_state,
                    self.config.observer.as_ref(),
                    &self.task,
                    terminal_task_result(&self.task, TaskNodeStatus::Failed, Some(message.clone())),
                )
                .await;
                broadcast_dependency_signal(out_channels, false).await;
                return Output::execution_failed(message);
            }
        };

        // Cost units are currently metered as completed/failed implementation tasks.
        // TODO: switch to provider token/cost usage accumulation when usage plumbing lands.
        let max_cost_units = self.config.spec.constraints.resources.max_cost_units as usize;
        let mut cost_budget_guard = None;
        if max_cost_units > 0 {
            let consumed = self
                .run_state
                .metered_result_count(&self.metered_task_ids)
                .await;
            if consumed >= max_cost_units {
                let message = format!(
                    "cost budget exceeded: maxCostUnits={} consumed={}",
                    max_cost_units, consumed
                );
                record_terminal_result(
                    &self.run_state,
                    self.config.observer.as_ref(),
                    &self.task,
                    terminal_task_result(&self.task, TaskNodeStatus::Failed, Some(message.clone())),
                )
                .await;
                broadcast_dependency_signal(out_channels, false).await;
                return Output::execution_failed(message);
            }

            let remaining = max_cost_units.saturating_sub(consumed);
            if remaining <= 1 {
                let guard = self.cost_budget_serial.lock().await;
                let consumed_after_lock = self
                    .run_state
                    .metered_result_count(&self.metered_task_ids)
                    .await;
                if consumed_after_lock >= max_cost_units {
                    let message = format!(
                        "cost budget exceeded: maxCostUnits={} consumed={}",
                        max_cost_units, consumed_after_lock
                    );
                    record_terminal_result(
                        &self.run_state,
                        self.config.observer.as_ref(),
                        &self.task,
                        terminal_task_result(
                            &self.task,
                            TaskNodeStatus::Failed,
                            Some(message.clone()),
                        ),
                    )
                    .await;
                    broadcast_dependency_signal(out_channels, false).await;
                    return Output::execution_failed(message);
                }
                cost_budget_guard = Some(guard);
            }
        }

        if let Some(observer) = &self.config.observer {
            observer.on_task_runtime_event(
                &self.task,
                TaskRuntimeEvent::Phase(TaskRuntimePhase::Starting),
            );
        }

        let use_task_worktree = should_use_task_worktree(&self.task);
        if !use_task_worktree
            && self.task.kind != TaskKind::Gate
            && let Some(observer) = &self.config.observer
        {
            observer.on_task_runtime_event(
                &self.task,
                TaskRuntimeEvent::WorkspaceReady {
                    working_dir: self.config.working_dir.clone(),
                    isolated: false,
                },
            );
        }

        let result = if use_task_worktree {
            execute_task_in_task_worktree(
                &self.task,
                &self.model,
                &self.registry,
                &self.config,
                &self.workspace_sync_serial,
            )
            .await
        } else {
            execute_task(&self.task, &self.model, &self.registry, &self.config).await
        };

        if let Some(observer) = &self.config.observer {
            observer.on_task_runtime_event(
                &self.task,
                TaskRuntimeEvent::Phase(match result.status {
                    TaskNodeStatus::Completed => TaskRuntimePhase::Completed,
                    TaskNodeStatus::Failed => TaskRuntimePhase::Failed,
                    _ => TaskRuntimePhase::Pending,
                }),
            );
            observer.on_task_runtime_event(
                &self.task,
                TaskRuntimeEvent::Note {
                    level: if result.status == TaskNodeStatus::Failed {
                        TaskRuntimeNoteLevel::Error
                    } else {
                        TaskRuntimeNoteLevel::Info
                    },
                    text: match result.status {
                        TaskNodeStatus::Completed => "task completed".to_string(),
                        TaskNodeStatus::Failed => result
                            .agent_output
                            .clone()
                            .map(|message| format!("task failed · {message}"))
                            .unwrap_or_else(|| "task failed".to_string()),
                        TaskNodeStatus::Pending => "task pending".to_string(),
                        TaskNodeStatus::Running => "task running".to_string(),
                        TaskNodeStatus::Skipped => "task skipped".to_string(),
                    },
                },
            );
        }

        self.run_state.record_result(result.clone()).await;
        drop(cost_budget_guard);

        match result.status {
            TaskNodeStatus::Completed => {
                broadcast_dependency_signal(out_channels, true).await;
                Output::empty()
            }
            TaskNodeStatus::Failed => {
                broadcast_dependency_signal(out_channels, false).await;
                Output::execution_failed(
                    result
                        .agent_output
                        .clone()
                        .unwrap_or_else(|| format!("task {} failed", self.task.title())),
                )
            }
            TaskNodeStatus::Skipped => {
                broadcast_dependency_signal(out_channels, false).await;
                Output::empty()
            }
            TaskNodeStatus::Pending | TaskNodeStatus::Running => {
                broadcast_dependency_signal(out_channels, false).await;
                Output::execution_failed(format!(
                    "task {} returned invalid terminal state",
                    self.task.title()
                ))
            }
        }
    }
}

fn build_dagrs_graph<M: CompletionModel + 'static>(
    plan: &ExecutionPlanSpec,
    model: &M,
    registry: &Arc<ToolRegistry>,
    config: &ExecutorConfig,
    context: DagrsBuildContext,
) -> Result<Graph, OrchestratorError>
where
    M::Response: CompletionUsage,
{
    let mut graph = Graph::new();
    configure_graph_runtime(&mut graph, plan, config);
    let mut node_table = NodeTable::new();
    let mut dagrs_ids = HashMap::new();

    for task_spec in &plan.tasks {
        let action = TaskDagrsAction {
            task: task_spec.clone(),
            model: model.clone(),
            registry: Arc::clone(registry),
            config: config.clone(),
            run_state: context.run_state.clone(),
            metered_task_ids: Arc::clone(&context.metered_task_ids),
            parallelism: Arc::clone(&context.parallelism),
            cost_budget_serial: Arc::clone(&context.cost_budget_serial),
            workspace_sync_serial: Arc::clone(&context.workspace_sync_serial),
        };
        let dagrs_node =
            DefaultNode::with_action(task_spec.id().to_string(), action, &mut node_table);
        let dagrs_id = dagrs_node.id();
        graph.add_node(dagrs_node).map_err(|err| {
            OrchestratorError::PlanningFailed(format!(
                "failed to add dagrs node for task {}: {err}",
                task_spec.id()
            ))
        })?;
        dagrs_ids.insert(task_spec.id(), dagrs_id);
    }

    for task_spec in &plan.tasks {
        let to_id = dagrs_ids.get(&task_spec.id()).copied().ok_or_else(|| {
            OrchestratorError::PlanningFailed(format!(
                "missing dagrs node for task {}",
                task_spec.id()
            ))
        })?;
        for dep in task_spec.dependencies() {
            let from_id = dagrs_ids.get(dep).copied().ok_or_else(|| {
                OrchestratorError::PlanningFailed(format!(
                    "missing dagrs node for dependency {dep}"
                ))
            })?;
            graph.add_edge(from_id, vec![to_id]).map_err(|err| {
                OrchestratorError::PlanningFailed(format!(
                    "failed to add dagrs edge {dep} -> {}: {err}",
                    task_spec.id()
                ))
            })?;
        }
    }

    Ok(graph)
}

fn configure_graph_runtime(graph: &mut Graph, plan: &ExecutionPlanSpec, config: &ExecutorConfig) {
    if !dagrs_checkpointing_enabled(&config.spec) {
        return;
    }

    let checkpoint_dir = dagrs_checkpoint_dir(&config.working_dir, plan);
    graph.set_checkpoint_store(Box::new(FileCheckpointStore::new(checkpoint_dir)));

    let checkpoint_interval = usize::from(plan.max_parallel.max(1));
    graph.set_checkpoint_config(
        CheckpointConfig::enabled()
            .with_node_interval(checkpoint_interval)
            .with_max_checkpoints(8),
    );
}

fn dagrs_checkpoint_dir(working_dir: &Path, plan: &ExecutionPlanSpec) -> PathBuf {
    working_dir
        .join(".libra")
        .join("dagrs-checkpoints")
        .join(sanitize_checkpoint_component(&plan.intent_spec_id))
        .join(format!("rev-{}", plan.revision))
}

fn sanitize_checkpoint_component(raw: &str) -> String {
    let sanitized: String = raw
        .chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => ch,
            _ => '_',
        })
        .collect();
    if sanitized.is_empty() {
        "intent-spec".to_string()
    } else {
        sanitized
    }
}

async fn monitor_graph_events(
    mut events: tokio::sync::broadcast::Receiver<GraphEvent>,
    run_state: RunStateStore,
    observer: Option<Arc<dyn OrchestratorObserver>>,
    total_nodes: usize,
) {
    run_state.record_graph_progress(0, total_nodes).await;
    loop {
        match events.recv().await {
            Ok(GraphEvent::NodeSuccess { .. })
            | Ok(GraphEvent::NodeFailed { .. })
            | Ok(GraphEvent::NodeSkipped { .. }) => {
                let next_completed = run_state.increment_graph_completed(total_nodes).await;
                if let Some(observer) = &observer {
                    observer.on_graph_progress(next_completed, total_nodes);
                }
            }
            Ok(GraphEvent::CheckpointSaved {
                checkpoint_id,
                pc,
                completed_nodes,
            }) => {
                run_state
                    .record_graph_checkpoint(checkpoint_id.clone(), pc, completed_nodes)
                    .await;
                if let Some(observer) = &observer {
                    observer.on_graph_checkpoint_saved(&checkpoint_id, pc, completed_nodes);
                }
            }
            Ok(GraphEvent::CheckpointRestored { checkpoint_id, pc }) => {
                run_state
                    .record_graph_restore(checkpoint_id.clone(), pc)
                    .await;
                if let Some(observer) = &observer {
                    observer.on_graph_checkpoint_restored(&checkpoint_id, pc);
                }
            }
            Ok(GraphEvent::ExecutionTerminated { .. }) => break,
            Ok(_) => {}
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }
}

/// Execute all tasks in the DAG in topological order.
pub async fn execute_dag<M: CompletionModel + 'static>(
    plan_spec: &ExecutionPlanSpec,
    model: &M,
    registry: &Arc<ToolRegistry>,
    config: &ExecutorConfig,
) -> Result<RunStateSnapshot, OrchestratorError>
where
    M::Response: CompletionUsage,
{
    if config.dagrs_resume_checkpoint_id.is_some() {
        return Err(OrchestratorError::ConfigError(
            "dagrs checkpoint resume is not supported yet; TODO: redesign resume semantics after userspace-fs checkpoint integration".to_string(),
        ));
    }

    let run_state = RunStateStore::new();
    let plan_snapshot = plan_spec.clone();
    let model_snapshot = model.clone();
    let registry_snapshot = Arc::clone(registry);
    let config_snapshot = config.clone();
    let run_state_snapshot = run_state.clone();
    let metered_task_ids = Arc::new(
        plan_spec
            .tasks
            .iter()
            .filter(|task| matches!(task.kind, TaskKind::Implementation))
            .map(TaskSpec::id)
            .collect::<HashSet<_>>(),
    );
    let max_cost_units = config.spec.constraints.resources.max_cost_units as usize;
    let initial_parallel = if max_cost_units > 0 && max_cost_units <= 2 {
        1
    } else {
        plan_spec.max_parallel.max(1) as usize
    };
    let parallelism = Arc::new(Semaphore::new(initial_parallel));
    let cost_budget_serial = Arc::new(tokio::sync::Mutex::new(()));
    let workspace_sync_serial = Arc::new(tokio::sync::Mutex::new(()));
    let graph_context = DagrsBuildContext {
        run_state: run_state_snapshot,
        metered_task_ids,
        parallelism,
        cost_budget_serial,
        workspace_sync_serial,
    };
    let mut graph = tokio::task::spawn_blocking(move || {
        build_dagrs_graph(
            &plan_snapshot,
            &model_snapshot,
            &registry_snapshot,
            &config_snapshot,
            graph_context,
        )
    })
    .await
    .map_err(|err| {
        OrchestratorError::ConfigError(format!("failed to build dagrs graph: {err}"))
    })??;

    let event_monitor = tokio::spawn(monitor_graph_events(
        graph.subscribe(),
        run_state.clone(),
        config.observer.clone(),
        plan_spec.tasks.len(),
    ));

    let wall_clock_limit_secs = config.spec.constraints.resources.max_wall_clock_seconds as u64;
    let execution_result = if wall_clock_limit_secs == 0 {
        graph.async_start().await
    } else {
        match tokio::time::timeout(
            Duration::from_secs(wall_clock_limit_secs),
            graph.async_start(),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => {
                drop(graph);
                if let Err(err) = event_monitor.await {
                    tracing::warn!("dagrs event monitor terminated unexpectedly: {}", err);
                }
                return Err(OrchestratorError::AgentError(format!(
                    "execution exceeded constraints.resources.maxWallClockSeconds={}s",
                    wall_clock_limit_secs
                )));
            }
        }
    };
    drop(graph);
    if let Err(err) = event_monitor.await {
        tracing::warn!("dagrs event monitor terminated unexpectedly: {}", err);
    }

    let (execution_report, execution_error) = match execution_result {
        Ok(report) => {
            tracing::debug!(
                run_id = %report.run_id,
                node_total = report.node_total,
                node_succeeded = report.node_succeeded,
                node_failed = report.node_failed,
                node_skipped = report.node_skipped,
                status = ?report.status,
                "dagrs execution completed"
            );
            (Some(report), None)
        }
        Err(err) => {
            tracing::warn!("dagrs execution terminated with error: {}", err);
            (None, Some(err.to_string()))
        }
    };

    let snapshot = run_state.snapshot(plan_spec).await;
    let incomplete_tasks = plan_spec
        .tasks
        .iter()
        .filter(|task| {
            matches!(
                snapshot.status_for(task.id()),
                TaskNodeStatus::Pending | TaskNodeStatus::Running
            )
        })
        .map(|task| task.title().to_string())
        .collect::<Vec<_>>();
    if !incomplete_tasks.is_empty() {
        let detail = execution_error
            .as_deref()
            .map(|err| format!("; dagrs_error={err}"))
            .unwrap_or_default();
        return Err(OrchestratorError::AgentError(format!(
            "dagrs execution ended with incomplete tasks: {}{}",
            incomplete_tasks.join(", "),
            detail
        )));
    }
    if let Some(report) = execution_report
        && matches!(report.status, CompletionStatus::Aborted)
    {
        return Err(OrchestratorError::AgentError(format!(
            "dagrs execution aborted: run_id={}, succeeded={}, failed={}, skipped={}",
            report.run_id, report.node_succeeded, report.node_failed, report.node_skipped
        )));
    }
    Ok(snapshot)
}

fn build_task_prompt(task: &TaskSpec, working_dir: &Path, allowed_tools: &[String]) -> String {
    let mut parts = Vec::new();
    parts.push(format!("## Task\n{}", task.title()));
    parts.push(format!("## Objective\n{}", task.objective));

    if let Some(desc) = task.description() {
        parts.push(format!("## Background\n{}", desc));
    }

    parts.push(format!(
        "## Runtime Workspace\nWorking directory: {}\nAll file access must stay inside this directory.",
        working_dir.display()
    ));

    if !allowed_tools.is_empty() {
        parts.push(format!("## Allowed Tools\n{}", allowed_tools.join(", ")));
    }

    parts.push(
        "## Path Rules\nUse repository-relative paths for read_file, list_dir, and grep_files. The runtime will resolve them from the working directory. Never invent or use paths outside the current workspace.".to_string(),
    );
    parts.push(format!(
        "## Version Control\nDo not use git for status, diff, add, commit, branch, log, \
             show, or switch operations. Use Libra version-control only; when the \
             run_libra_vcs tool is available, call it for those operations.\n{}\nrun_libra_vcs \
             is not a shell and must not be used for cargo, fmt, clippy, test, build, or \
             arbitrary command execution; deterministic verification commands are owned by \
             gate tasks.",
        run_libra_vcs_tool_guidance()
    ));

    if !task.contract.touch_files.is_empty() {
        parts.push(format!(
            "## Write Contract\nOnly modify these files/patterns:\n{}\nTreat this list as a hard boundary. If the task requires another file, explain the mismatch instead of editing it.",
            task.contract
                .touch_files
                .iter()
                .map(|path| format!("- {path}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }

    if !task.contract.touch_symbols.is_empty() {
        parts.push(format!(
            "Symbols: {}",
            task.contract.touch_symbols.join(", ")
        ));
    }

    if !task.contract.touch_apis.is_empty() {
        parts.push(format!("APIs: {}", task.contract.touch_apis.join(", ")));
    }

    if !task.acceptance_criteria().is_empty() {
        parts.push(format!(
            "## Acceptance Criteria\n{}",
            task.acceptance_criteria()
                .iter()
                .map(|criterion| format!("- {}", criterion))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }

    if !task.scope_in.is_empty() {
        let heading = match task.kind {
            TaskKind::Analysis => "## Focus Scope",
            _ => "## Write Scope",
        };
        parts.push(format!("{heading}\n{}", task.scope_in.join(", ")));
    }

    if !task.scope_out.is_empty() {
        parts.push(format!("## Forbidden Scope\n{}", task.scope_out.join(", ")));
    }

    if !task.contract.expected_outputs.is_empty() {
        parts.push(format!(
            "## Expected Outputs\n{}",
            task.contract
                .expected_outputs
                .iter()
                .map(|output| format!("- {}", output))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }

    if task.kind == TaskKind::Implementation {
        parts.push(
            "## Completion Requirement\nBefore reporting completion, use apply_patch or an allowed shell command that creates or modifies at least one in-scope project file. If no file change is needed or the write scope is wrong, report the mismatch instead of claiming the task is complete."
                .to_string(),
        );
    }

    if !task.constraints().is_empty() {
        parts.push(format!(
            "## Constraints\n{}",
            task.constraints()
                .iter()
                .map(|constraint| format!("- {}", constraint))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }

    if task.kind == TaskKind::Analysis {
        parts.push(
            "## Analysis Mode\nDo not modify repository files. Use read-only exploration and, if shell is allowed, only non-mutating inspection or verification commands."
                .to_string(),
        );
    }

    parts.join("\n\n")
}

fn build_reviewer_prompt(
    task: &TaskSpec,
    agent_output: &str,
    tool_calls: &[ToolCallRecord],
    working_dir: &Path,
    allowed_tools: &[String],
) -> String {
    let touched_files = tool_calls
        .iter()
        .flat_map(|call| call.paths_written.iter().cloned())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();

    let mut parts = vec![
        format!("## Review Task\n{}", task.title()),
        format!("## Objective\n{}", task.objective),
        format!(
            "## Runtime Workspace\nWorking directory: {}\nAll file access must stay inside this directory.",
            working_dir.display()
        ),
        format!("## Allowed Tools\n{}", allowed_tools.join(", ")),
        format!("## Candidate Output\n{}", agent_output.trim()),
        "Return JSON only in this exact shape: {\"approved\":true|false,\"summary\":\"...\",\"issues\":[\"...\"]}".to_string(),
    ];

    if !touched_files.is_empty() {
        parts.push(format!("## Touched Files\n{}", touched_files.join(", ")));
    }

    if !task.contract.touch_files.is_empty() {
        parts.push(format!(
            "## Write Contract\nOnly these paths may be modified:\n{}\nReject the candidate if Touched Files contains any path outside this list.",
            task.contract
                .touch_files
                .iter()
                .map(|path| format!("- {path}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }

    if !task.acceptance_criteria().is_empty() {
        parts.push(format!(
            "## Acceptance Criteria\n{}",
            task.acceptance_criteria().join("\n")
        ));
    }

    parts.join("\n\n")
}

fn allowed_tools_for_task(spec: &IntentSpec, task: &TaskSpec) -> Vec<String> {
    let mut tools = Vec::new();

    if acl_allows(&spec.security.tool_acl, "workspace.fs", "read") {
        tools.extend([
            "read_file".to_string(),
            "list_dir".to_string(),
            "grep_files".to_string(),
            "search_files".to_string(),
        ]);
    }

    if task.kind == TaskKind::Implementation
        && acl_allows(&spec.security.tool_acl, "workspace.fs", "write")
    {
        tools.push("apply_patch".to_string());
    }

    if matches!(task.kind, TaskKind::Implementation | TaskKind::Analysis)
        && (acl_allows(&spec.security.tool_acl, "libra.vcs", "read")
            || acl_allows(&spec.security.tool_acl, "libra.vcs", "write"))
    {
        tools.push("run_libra_vcs".to_string());
    }

    if matches!(task.kind, TaskKind::Implementation | TaskKind::Analysis)
        && acl_allows(&spec.security.tool_acl, "shell", "execute")
    {
        tools.push("shell".to_string());
    }

    if matches!(task.kind, TaskKind::Implementation | TaskKind::Analysis)
        && spec.constraints.security.network_policy == NetworkPolicy::Allow
        && acl_allows(&spec.security.tool_acl, "web.search", "query")
    {
        tools.push("web_search".to_string());
    }

    tools
}

fn allowed_tools_for_reviewer(spec: &IntentSpec) -> Vec<String> {
    let mut tools = if acl_allows(&spec.security.tool_acl, "workspace.fs", "read") {
        vec![
            "read_file".to_string(),
            "list_dir".to_string(),
            "grep_files".to_string(),
            "search_files".to_string(),
        ]
    } else {
        Vec::new()
    };
    if spec.constraints.security.network_policy == NetworkPolicy::Allow
        && acl_allows(&spec.security.tool_acl, "web.search", "query")
    {
        tools.push("web_search".to_string());
    }
    tools
}

fn acl_allows(acl: &ToolAcl, tool: &str, action: &str) -> bool {
    matches!(check_tool_acl(acl, tool, action), AclVerdict::Allow)
}

fn parse_reviewer_decision(raw: &str) -> Result<ReviewerDecision, String> {
    if let Ok(parsed) = serde_json::from_str::<ReviewerDecision>(raw.trim()) {
        return Ok(parsed);
    }

    let mut last_error = None;
    for candidate in fenced_json_blocks(raw).chain(json_object_candidates(raw)) {
        match parse_reviewer_decision_prefix(candidate) {
            Ok(parsed) => return Ok(parsed),
            Err(error) => last_error = Some(error),
        }
    }

    match last_error {
        Some(error) => Err(format!("invalid reviewer JSON: {error}")),
        None => Err("reviewer response missing JSON object".to_string()),
    }
}

fn fenced_json_blocks(raw: &str) -> impl Iterator<Item = &str> {
    struct Blocks<'a> {
        raw: &'a str,
        offset: usize,
    }

    impl<'a> Iterator for Blocks<'a> {
        type Item = &'a str;

        fn next(&mut self) -> Option<Self::Item> {
            while let Some(relative_start) = self.raw[self.offset..].find("```") {
                let fence_start = self.offset + relative_start;
                let header_start = fence_start + 3;
                let Some(relative_header_end) = self.raw[header_start..].find('\n') else {
                    self.offset = header_start;
                    continue;
                };
                let header_end = header_start + relative_header_end;
                let header = self.raw[header_start..header_end].trim();
                let body_start = header_end + 1;
                let Some(relative_end) = self.raw[body_start..].find("```") else {
                    self.offset = body_start;
                    continue;
                };
                let body_end = body_start + relative_end;
                self.offset = body_end + 3;
                if header.is_empty() || header.eq_ignore_ascii_case("json") {
                    return Some(self.raw[body_start..body_end].trim());
                }
            }
            None
        }
    }

    Blocks { raw, offset: 0 }
}

fn json_object_candidates(raw: &str) -> impl Iterator<Item = &str> {
    const MAX_JSON_OBJECT_CANDIDATES: usize = 16;

    raw.char_indices()
        .filter(|(_, ch)| *ch == '{')
        .map(|(index, _)| &raw[index..])
        .take(MAX_JSON_OBJECT_CANDIDATES)
}

fn parse_reviewer_decision_prefix(raw: &str) -> Result<ReviewerDecision, serde_json::Error> {
    let mut deserializer = serde_json::Deserializer::from_str(raw.trim_start());
    ReviewerDecision::deserialize(&mut deserializer)
}

fn runtime_context_for_task(
    spec: &IntentSpec,
    task: &TaskSpec,
    working_dir: &Path,
    inherited_runtime: Option<&ToolRuntimeContext>,
) -> ToolRuntimeContext {
    let network_access = matches!(
        spec.constraints.security.network_policy,
        NetworkPolicy::Allow
    );
    let writable_roots = collect_writable_roots(spec, task, working_dir);
    let policy = SandboxPolicy::WorkspaceWrite {
        writable_roots,
        network_access,
        exclude_tmpdir_env_var: false,
        exclude_slash_tmp: false,
    };
    ToolRuntimeContext {
        sandbox: Some(ToolSandboxContext {
            policy,
            permissions: SandboxPermissions::UseDefault,
        }),
        sandbox_runtime: inherited_runtime.and_then(|ctx| ctx.sandbox_runtime.clone()),
        approval: inherited_runtime.and_then(|ctx| ctx.approval.clone()),
        max_output_bytes: max_output_limit(&spec.security.tool_acl, "shell", "execute"),
    }
}

fn runtime_context_for_gate_task(
    spec: &IntentSpec,
    working_dir: &Path,
    inherited_runtime: Option<&ToolRuntimeContext>,
) -> ToolRuntimeContext {
    ToolRuntimeContext {
        sandbox: Some(ToolSandboxContext {
            policy: SandboxPolicy::WorkspaceWrite {
                writable_roots: vec![working_dir.to_path_buf()],
                network_access: matches!(
                    spec.constraints.security.network_policy,
                    NetworkPolicy::Allow
                ),
                exclude_tmpdir_env_var: false,
                exclude_slash_tmp: false,
            },
            permissions: SandboxPermissions::UseDefault,
        }),
        sandbox_runtime: inherited_runtime.and_then(|ctx| ctx.sandbox_runtime.clone()),
        approval: inherited_runtime.and_then(|ctx| ctx.approval.clone()),
        max_output_bytes: max_output_limit(&spec.security.tool_acl, "shell", "execute"),
    }
}

fn runtime_context_for_reviewer(
    spec: &IntentSpec,
    inherited_runtime: Option<&ToolRuntimeContext>,
) -> ToolRuntimeContext {
    let policy = if matches!(
        spec.constraints.security.network_policy,
        NetworkPolicy::Allow
    ) {
        SandboxPolicy::ExternalSandbox {
            network_access: NetworkAccess::Enabled,
        }
    } else {
        SandboxPolicy::ReadOnly
    };
    ToolRuntimeContext {
        sandbox: Some(ToolSandboxContext {
            policy,
            permissions: SandboxPermissions::UseDefault,
        }),
        sandbox_runtime: inherited_runtime.and_then(|ctx| ctx.sandbox_runtime.clone()),
        approval: inherited_runtime.and_then(|ctx| ctx.approval.clone()),
        max_output_bytes: max_output_limit(&spec.security.tool_acl, "workspace.fs", "read"),
    }
}

fn collect_writable_roots(spec: &IntentSpec, task: &TaskSpec, working_dir: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::<PathBuf>::new();

    for touch_file in &task.contract.touch_files {
        if let Some(resolved) = resolve_writable_root_candidate(touch_file, working_dir, true) {
            push_unique_root(&mut roots, resolved);
        }
    }

    if roots.is_empty() {
        for scope in task.scope_in.iter().chain(task.contract.write_scope.iter()) {
            if let Some(resolved) = resolve_writable_root_candidate(scope, working_dir, false) {
                push_unique_root(&mut roots, resolved);
            }
        }
    }

    if roots.is_empty() {
        for rule in &spec.security.tool_acl.allow {
            let writes_allowed = rule
                .actions
                .iter()
                .any(|action| action == "*" || action == "write");
            if !writes_allowed {
                continue;
            }
            if let Some(serde_json::Value::Array(write_roots)) = rule.constraints.get("writeRoots")
            {
                for root in write_roots.iter().filter_map(serde_json::Value::as_str) {
                    if let Some(resolved) =
                        resolve_writable_root_candidate(root, working_dir, false)
                    {
                        push_unique_root(&mut roots, resolved);
                    }
                }
            }
        }
    }

    if roots.is_empty() {
        push_unique_root(&mut roots, working_dir.to_path_buf());
    }

    roots
}

fn resolve_writable_root_candidate(
    raw: &str,
    working_dir: &Path,
    prefer_parent_for_missing_leaf: bool,
) -> Option<PathBuf> {
    let normalized = raw.trim().replace('\\', "/");
    let trimmed = normalized.trim_start_matches("./");
    if trimmed.is_empty() || matches!(trimmed, "*" | "**") {
        return None;
    }

    let literal_or_root = trimmed
        .find(['*', '?', '[', '{'])
        .map(|index| trimmed[..index].trim_end_matches('/').to_string())
        .unwrap_or_else(|| trimmed.trim_end_matches('/').to_string());

    if literal_or_root.is_empty() {
        return None;
    }

    let resolved = if Path::new(&literal_or_root).is_absolute() {
        PathBuf::from(&literal_or_root)
    } else {
        working_dir.join(&literal_or_root)
    };

    if prefer_parent_for_missing_leaf && !resolved.exists() {
        return resolved.parent().map(Path::to_path_buf).or(Some(resolved));
    }

    Some(resolved)
}

fn push_unique_root(roots: &mut Vec<PathBuf>, root: PathBuf) {
    let normalized = root.canonicalize().unwrap_or(root);
    if roots.iter().any(|existing| existing == &normalized) {
        return;
    }
    roots.push(normalized);
}

fn max_output_limit(acl: &ToolAcl, tool_name: &str, action: &str) -> Option<usize> {
    acl.allow
        .iter()
        .filter(|rule| {
            rule.tool == "*" || rule.tool == tool_name || matches_tool_glob(&rule.tool, tool_name)
        })
        .filter(|rule| {
            rule.actions
                .iter()
                .any(|rule_action| rule_action == "*" || rule_action == action)
        })
        .filter_map(|rule| rule.constraints.get("maxOutputBytes"))
        .filter_map(serde_json::Value::as_u64)
        .map(|value| value as usize)
        .min()
}

fn matches_tool_glob(pattern: &str, tool_name: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix(".*") {
        return tool_name.starts_with(prefix)
            && tool_name
                .get(prefix.len()..)
                .is_some_and(|rest| rest.starts_with('.'));
    }
    false
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        path::PathBuf,
        sync::{Arc, Mutex},
        time::Duration,
    };

    use git_internal::internal::object::{task::Task as GitTask, types::ActorRef};
    use serial_test::serial;

    use super::*;
    use crate::{
        internal::ai::{
            completion::{
                CompletionError, CompletionRequest, CompletionResponse,
                message::{AssistantContent, Function, Message, Text, ToolCall, UserContent},
            },
            intentspec::{profiles, types::*},
            orchestrator::types::{ExecutionPlanSpec, TaskContract, TaskKind, TaskSpec},
            tools::{handlers::ApplyPatchHandler, registry::ToolRegistry},
        },
        utils::test,
    };

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
                    reasoning_content: None,
                    raw_response: (),
                })
            }
        }
    }

    #[derive(Clone)]
    struct ConditionalModel;

    impl CompletionModel for ConditionalModel {
        type Response = ();

        async fn completion(
            &self,
            request: CompletionRequest,
        ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
            let prompt = request
                .chat_history
                .iter()
                .rev()
                .find_map(|message| match message {
                    Message::User { content } => content.iter().find_map(|item| match item {
                        UserContent::Text(text) => Some(text.text.clone()),
                        _ => None,
                    }),
                    _ => None,
                })
                .unwrap_or_default();
            let has_tool_result = request.chat_history.iter().any(|message| match message {
                Message::User { content } => content
                    .iter()
                    .any(|item| matches!(item, UserContent::ToolResult(_))),
                _ => false,
            });

            if prompt.contains("## Task\nB") {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }

            if prompt.contains("Fail first") {
                Err(CompletionError::ResponseError("intentional failure".into()))
            } else if !has_tool_result && prompt.contains("apply_patch") {
                Ok(CompletionResponse {
                    content: vec![AssistantContent::ToolCall(add_file_patch_call(
                        &prompt,
                        "conditional",
                    ))],
                    reasoning_content: None,
                    raw_response: (),
                })
            } else {
                Ok(CompletionResponse {
                    content: vec![AssistantContent::Text(Text {
                        text: "done".into(),
                    })],
                    reasoning_content: None,
                    raw_response: (),
                })
            }
        }
    }

    #[derive(Clone)]
    struct AddFilePatchModel;

    impl CompletionModel for AddFilePatchModel {
        type Response = ();

        async fn completion(
            &self,
            request: CompletionRequest,
        ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
            let has_tool_result = request.chat_history.iter().any(|message| match message {
                Message::User { content } => content
                    .iter()
                    .any(|item| matches!(item, UserContent::ToolResult(_))),
                _ => false,
            });
            if has_tool_result {
                return Ok(CompletionResponse {
                    content: vec![AssistantContent::Text(Text {
                        text: "done".to_string(),
                    })],
                    reasoning_content: None,
                    raw_response: (),
                });
            }

            let prompt = request
                .chat_history
                .iter()
                .rev()
                .find_map(|message| match message {
                    Message::User { content } => content.iter().find_map(|item| match item {
                        UserContent::Text(text) => Some(text.text.clone()),
                        _ => None,
                    }),
                    _ => None,
                })
                .unwrap_or_default();

            Ok(CompletionResponse {
                content: vec![AssistantContent::ToolCall(add_file_patch_call(
                    &prompt, "budget",
                ))],
                reasoning_content: None,
                raw_response: (),
            })
        }
    }

    fn add_file_patch_call(prompt: &str, prefix: &str) -> ToolCall {
        let slug = prompt
            .split("## Task\n")
            .nth(1)
            .and_then(|tail| tail.lines().next())
            .map(slugify_task_title)
            .filter(|slug| !slug.is_empty())
            .unwrap_or_else(|| "task".to_string());
        let path = match slug.as_str() {
            "do_thing" | "second" => "src/main.rs".to_string(),
            _ => format!("src/{slug}.txt"),
        };
        let patch = format!("*** Begin Patch\n*** Add File: {path}\n+done\n*** End Patch");
        ToolCall {
            id: format!("call_{prefix}_{slug}"),
            name: "apply_patch".to_string(),
            function: Function {
                name: "apply_patch".to_string(),
                arguments: serde_json::json!({ "input": patch }),
            },
        }
    }

    fn slugify_task_title(title: &str) -> String {
        title
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() {
                    ch.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .collect::<String>()
            .trim_matches('_')
            .to_string()
    }

    #[derive(Clone)]
    struct PatchApplyingModel;

    impl CompletionModel for PatchApplyingModel {
        type Response = ();

        async fn completion(
            &self,
            request: CompletionRequest,
        ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
            let has_tool_result = request.chat_history.iter().any(|message| match message {
                Message::User { content } => content
                    .iter()
                    .any(|item| matches!(item, UserContent::ToolResult(_))),
                _ => false,
            });
            if has_tool_result {
                return Ok(CompletionResponse {
                    content: vec![AssistantContent::Text(Text {
                        text: "done".to_string(),
                    })],
                    reasoning_content: None,
                    raw_response: (),
                });
            }

            let prompt = request
                .chat_history
                .iter()
                .rev()
                .find_map(|message| match message {
                    Message::User { content } => content.iter().find_map(|item| match item {
                        UserContent::Text(text) => Some(text.text.clone()),
                        _ => None,
                    }),
                    _ => None,
                })
                .unwrap_or_default();

            let (call_id, patch) = if prompt.contains("## Task\nTask A") {
                (
                    "call_a",
                    "*** Begin Patch\n*** Update File: task_a.txt\n@@\n-base\n+task-a\n*** End Patch",
                )
            } else {
                (
                    "call_b",
                    "*** Begin Patch\n*** Update File: task_b.txt\n@@\n-base\n+task-b\n*** End Patch",
                )
            };

            Ok(CompletionResponse {
                content: vec![AssistantContent::ToolCall(ToolCall {
                    id: call_id.to_string(),
                    name: "apply_patch".to_string(),
                    function: Function {
                        name: "apply_patch".to_string(),
                        arguments: serde_json::json!({ "input": patch }),
                    },
                })],
                reasoning_content: None,
                raw_response: (),
            })
        }
    }

    #[derive(Clone)]
    struct SrcMainPatchModel;

    impl CompletionModel for SrcMainPatchModel {
        type Response = ();

        async fn completion(
            &self,
            request: CompletionRequest,
        ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
            let has_tool_result = request.chat_history.iter().any(|message| match message {
                Message::User { content } => content
                    .iter()
                    .any(|item| matches!(item, UserContent::ToolResult(_))),
                _ => false,
            });
            if has_tool_result {
                return Ok(CompletionResponse {
                    content: vec![AssistantContent::Text(Text {
                        text: "done".to_string(),
                    })],
                    reasoning_content: None,
                    raw_response: (),
                });
            }

            Ok(CompletionResponse {
                content: vec![AssistantContent::ToolCall(ToolCall {
                    id: "call_src_main".to_string(),
                    name: "apply_patch".to_string(),
                    function: Function {
                        name: "apply_patch".to_string(),
                        arguments: serde_json::json!({
                            "input": "*** Begin Patch\n*** Update File: src/main.rs\n@@\n-fn main() {}\n+fn main() { println!(\"hello\"); }\n*** End Patch"
                        }),
                    },
                })],
                reasoning_content: None,
                raw_response: (),
            })
        }
    }

    #[derive(Clone)]
    struct RetryAfterNoWriteModel;

    impl CompletionModel for RetryAfterNoWriteModel {
        type Response = ();

        async fn completion(
            &self,
            request: CompletionRequest,
        ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
            let has_tool_result = request.chat_history.iter().any(|message| match message {
                Message::User { content } => content
                    .iter()
                    .any(|item| matches!(item, UserContent::ToolResult(_))),
                _ => false,
            });
            if has_tool_result {
                return Ok(CompletionResponse {
                    content: vec![AssistantContent::Text(Text {
                        text: "done".to_string(),
                    })],
                    reasoning_content: None,
                    raw_response: (),
                });
            }

            let prompt = request
                .chat_history
                .iter()
                .rev()
                .find_map(|message| match message {
                    Message::User { content } => content.iter().find_map(|item| match item {
                        UserContent::Text(text) => Some(text.text.clone()),
                        _ => None,
                    }),
                    _ => None,
                })
                .unwrap_or_default();

            if prompt.contains("## Previous Attempt Failure") {
                return Ok(CompletionResponse {
                    content: vec![AssistantContent::ToolCall(ToolCall {
                        id: "call_retry_patch".to_string(),
                        name: "apply_patch".to_string(),
                        function: Function {
                            name: "apply_patch".to_string(),
                            arguments: serde_json::json!({
                                "input": "*** Begin Patch\n*** Update File: src/main.rs\n@@\n-fn main() {}\n+fn main() { println!(\"fixed\"); }\n*** End Patch"
                            }),
                        },
                    })],
                    reasoning_content: None,
                    raw_response: (),
                });
            }

            Ok(CompletionResponse {
                content: vec![AssistantContent::Text(Text {
                    text: "done without edits".to_string(),
                })],
                reasoning_content: None,
                raw_response: (),
            })
        }
    }

    #[derive(Clone)]
    struct PatchThenFailModel;

    impl CompletionModel for PatchThenFailModel {
        type Response = ();

        async fn completion(
            &self,
            request: CompletionRequest,
        ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
            let has_tool_result = request.chat_history.iter().any(|message| match message {
                Message::User { content } => content
                    .iter()
                    .any(|item| matches!(item, UserContent::ToolResult(_))),
                _ => false,
            });
            if has_tool_result {
                return Err(CompletionError::ResponseError(
                    "intentional failure after patch".to_string(),
                ));
            }

            Ok(CompletionResponse {
                content: vec![AssistantContent::ToolCall(ToolCall {
                    id: "call_fail".to_string(),
                    name: "apply_patch".to_string(),
                    function: Function {
                        name: "apply_patch".to_string(),
                        arguments: serde_json::json!({
                            "input": "*** Begin Patch\n*** Update File: task_a.txt\n@@\n-base\n+task-a\n*** End Patch"
                        }),
                    },
                })],
                reasoning_content: None,
                raw_response: (),
            })
        }
    }

    struct RecordingObserver {
        events: Arc<Mutex<Vec<String>>>,
    }

    impl OrchestratorObserver for RecordingObserver {
        fn on_task_runtime_event(&self, task: &TaskSpec, event: TaskRuntimeEvent) {
            let mut events = self.events.lock().unwrap();
            match event {
                TaskRuntimeEvent::Phase(TaskRuntimePhase::Starting) => {
                    events.push(format!("start:{}", task.title()));
                }
                TaskRuntimeEvent::Phase(TaskRuntimePhase::Completed) => {
                    events.push(format!("done:{}", task.title()));
                }
                TaskRuntimeEvent::Note { text, .. } if text.starts_with("gate running") => {
                    events.push(format!("gate-start:{}:{}", task.title(), text));
                }
                TaskRuntimeEvent::Note { text, .. } if text.starts_with("gate passed") => {
                    events.push(format!("gate-done:{}:pass", task.title()));
                }
                TaskRuntimeEvent::Note { text, .. } if text.starts_with("gate failed") => {
                    events.push(format!("gate-done:{}:fail", task.title()));
                }
                _ => {}
            }
        }
    }

    fn spec() -> Arc<IntentSpec> {
        Arc::new(IntentSpec {
            api_version: "intentspec.io/v1alpha1".into(),
            kind: "IntentSpec".into(),
            metadata: Metadata {
                id: "test".into(),
                created_at: "2025-01-01T00:00:00Z".into(),
                created_by: CreatedBy {
                    creator_type: CreatorType::User,
                    id: "tester".into(),
                    display_name: None,
                },
                target: Target {
                    repo: RepoTarget {
                        repo_type: RepoType::Local,
                        locator: "/tmp".into(),
                    },
                    base_ref: "HEAD".into(),
                    workspace_id: None,
                    labels: BTreeMap::new(),
                },
            },
            intent: Intent {
                summary: "summary".into(),
                problem_statement: "problem".into(),
                change_type: ChangeType::Feature,
                objectives: vec![Objective {
                    title: "do thing".into(),
                    kind: ObjectiveKind::Implementation,
                }],
                in_scope: vec!["src/".into()],
                out_of_scope: vec![],
                touch_hints: None,
            },
            acceptance: Acceptance {
                success_criteria: vec!["tests pass".into()],
                verification_plan: VerificationPlan {
                    fast_checks: vec![],
                    integration_checks: vec![],
                    security_checks: vec![],
                    release_checks: vec![],
                },
                quality_gates: None,
            },
            constraints: Constraints {
                security: ConstraintSecurity {
                    network_policy: NetworkPolicy::Allow,
                    dependency_policy: DependencyPolicy::NoNew,
                    crypto_policy: String::new(),
                },
                privacy: ConstraintPrivacy {
                    data_classes_allowed: vec![DataClass::Public],
                    redaction_required: false,
                    retention_days: 30,
                },
                licensing: ConstraintLicensing {
                    allowed_spdx: vec![],
                    forbid_new_licenses: false,
                },
                platform: ConstraintPlatform {
                    language_runtime: "rust".into(),
                    supported_os: vec![],
                },
                resources: ConstraintResources {
                    max_wall_clock_seconds: 60,
                    max_cost_units: 10,
                },
            },
            risk: Risk {
                level: RiskLevel::Low,
                rationale: "low".into(),
                factors: vec![],
                human_in_loop: HumanInLoop {
                    required: false,
                    min_approvers: 0,
                },
            },
            evidence: EvidencePolicy {
                strategy: EvidenceStrategy::RepoFirst,
                trust_tiers: vec![TrustTier::Repo],
                domain_allowlist_mode: DomainAllowlistMode::Disabled,
                allowed_domains: vec![],
                blocked_domains: vec![],
                min_citations_per_decision: 1,
            },
            security: SecurityPolicy {
                tool_acl: ToolAcl {
                    allow: vec![ToolRule {
                        tool: "*".into(),
                        actions: vec!["*".into()],
                        constraints: BTreeMap::new(),
                    }],
                    deny: vec![],
                },
                secrets: SecretPolicy {
                    policy: SecretAccessPolicy::DenyAll,
                    allowed_scopes: vec![],
                },
                prompt_injection: PromptInjectionPolicy {
                    treat_retrieved_content_as_untrusted: true,
                    enforce_output_schema: true,
                    disallow_instruction_from_evidence: true,
                },
                output_handling: OutputHandlingPolicy {
                    encoding_policy: EncodingPolicy::ContextualEscape,
                    no_direct_eval: true,
                },
            },
            execution: ExecutionPolicy {
                retry: RetryPolicy {
                    max_retries: 1,
                    backoff_seconds: 0,
                },
                replan: ReplanPolicy { triggers: vec![] },
                concurrency: ConcurrencyPolicy {
                    max_parallel_tasks: 1,
                },
            },
            artifacts: Artifacts {
                required: vec![],
                retention: ArtifactRetention { days: 30 },
            },
            provenance: ProvenancePolicy {
                require_slsa_provenance: false,
                require_sbom: false,
                transparency_log: TransparencyLogPolicy {
                    mode: TransparencyMode::None,
                },
                bindings: ProvenanceBindings {
                    embed_intent_spec_digest: false,
                    embed_evidence_digests: false,
                },
            },
            lifecycle: Lifecycle {
                schema_version: "1.0.0".into(),
                status: LifecycleStatus::Active,
                change_log: vec![],
            },
            libra: None,
            extensions: BTreeMap::new(),
        })
    }

    fn implementation_task() -> TaskSpec {
        let actor = ActorRef::agent("test-executor").unwrap();
        let mut task = GitTask::new(actor, "Do thing", None).unwrap();
        task.set_description(Some("Implement change".into()));
        task.add_constraint("network:allow");
        task.add_acceptance_criterion("tests pass");
        TaskSpec {
            step: git_internal::internal::object::plan::PlanStep::new("Do thing"),
            task,
            objective: "Do thing".into(),
            kind: TaskKind::Implementation,
            gate_stage: None,
            owner_role: Some("coder".into()),
            scope_in: vec!["src/".into()],
            scope_out: vec![],
            checks: vec![],
            contract: TaskContract {
                write_scope: vec!["src/".into()],
                forbidden_scope: vec![],
                touch_files: vec!["src/main.rs".into()],
                touch_symbols: vec![],
                touch_apis: vec![],
                expected_outputs: vec!["tests pass".into()],
            },
        }
    }

    fn analysis_task() -> TaskSpec {
        TaskSpec {
            kind: TaskKind::Analysis,
            owner_role: Some("analyst".into()),
            ..implementation_task()
        }
    }

    fn scoped_implementation_task(title: &str, file: &str) -> TaskSpec {
        let actor = ActorRef::agent("test-executor").unwrap();
        let mut task = GitTask::new(actor, title, None).unwrap();
        task.set_description(Some(format!("Modify {file}")));
        task.add_constraint("network:allow");
        task.add_acceptance_criterion("tests pass");
        TaskSpec {
            step: git_internal::internal::object::plan::PlanStep::new(title),
            task,
            objective: title.into(),
            kind: TaskKind::Implementation,
            gate_stage: None,
            owner_role: Some("coder".into()),
            scope_in: vec![file.into()],
            scope_out: vec![],
            checks: vec![],
            contract: TaskContract {
                write_scope: vec![file.into()],
                forbidden_scope: vec![],
                touch_files: vec![file.into()],
                touch_symbols: vec![],
                touch_apis: vec![],
                expected_outputs: vec![format!("update {file}")],
            },
        }
    }

    fn plan_for_tasks(tasks: Vec<TaskSpec>, max_parallel: u8) -> ExecutionPlanSpec {
        ExecutionPlanSpec {
            intent_spec_id: "test".into(),
            revision: 1,
            parent_revision: None,
            replan_reason: None,
            tasks,
            max_parallel,
            checkpoints: vec![],
        }
    }

    #[tokio::test]
    async fn test_execute_gate_task() {
        let task = TaskSpec {
            kind: TaskKind::Gate,
            gate_stage: Some(super::super::types::GateStage::Fast),
            checks: vec![Check {
                id: "ok".into(),
                kind: CheckKind::Command,
                command: Some(":".into()),
                timeout_seconds: Some(10),
                expected_exit_code: Some(0),
                required: true,
                artifacts_produced: vec![],
            }],
            ..implementation_task()
        };
        let dir = tempfile::tempdir().unwrap();
        let result = execute_gate_task(&task, dir.path(), &spec(), None, None).await;
        assert_eq!(result.status, TaskNodeStatus::Completed);
        assert!(result.gate_report.unwrap().all_required_passed);
    }

    #[tokio::test]
    async fn test_execute_gate_task_emits_check_progress_events() {
        let task = TaskSpec {
            kind: TaskKind::Gate,
            gate_stage: Some(super::super::types::GateStage::Fast),
            checks: vec![Check {
                id: "fmt".into(),
                kind: CheckKind::Command,
                command: Some(":".into()),
                timeout_seconds: Some(10),
                expected_exit_code: Some(0),
                required: true,
                artifacts_produced: vec![],
            }],
            ..implementation_task()
        };
        let dir = tempfile::tempdir().unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let observer: Arc<dyn OrchestratorObserver> = Arc::new(RecordingObserver {
            events: Arc::clone(&events),
        });

        let result = execute_gate_task(&task, dir.path(), &spec(), None, Some(&observer)).await;

        assert_eq!(result.status, TaskNodeStatus::Completed);
        let recorded = events.lock().unwrap();
        assert!(
            recorded
                .iter()
                .any(|event| event.starts_with("gate-start:Do thing:gate running"))
        );
        assert!(recorded.contains(&"gate-done:Do thing:pass".to_string()));
    }

    #[tokio::test]
    async fn test_execute_gate_task_with_default_security() {
        let task = TaskSpec {
            kind: TaskKind::Gate,
            gate_stage: Some(super::super::types::GateStage::Fast),
            checks: vec![Check {
                id: "verify".into(),
                kind: CheckKind::Command,
                command: Some(":".into()),
                timeout_seconds: Some(10),
                expected_exit_code: Some(0),
                required: true,
                artifacts_produced: vec![],
            }],
            ..implementation_task()
        };
        let dir = tempfile::tempdir().unwrap();
        let mut spec = (*spec()).clone();
        spec.security = profiles::default_security();

        let result = execute_gate_task(&task, dir.path(), &spec, None, None).await;

        assert_eq!(result.status, TaskNodeStatus::Completed);
        assert!(result.gate_report.unwrap().all_required_passed);
    }

    #[tokio::test]
    #[serial]
    async fn execute_task_runs_gate_checks_in_isolated_worktree() {
        let repo = tempfile::tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        std::fs::write(repo.path().join("tracked.txt"), "base\n").unwrap();

        let task = TaskSpec {
            kind: TaskKind::Gate,
            gate_stage: Some(super::super::types::GateStage::Fast),
            checks: vec![Check {
                id: "scratch".into(),
                kind: CheckKind::Command,
                command: Some("printf 'scratch\\n' > gate-output.txt".into()),
                timeout_seconds: Some(10),
                expected_exit_code: Some(0),
                required: true,
                artifacts_produced: vec![],
            }],
            ..implementation_task()
        };
        let config = ExecutorConfig {
            tool_loop_config: ToolLoopConfig::default(),
            max_retries: 0,
            backoff_seconds: 0,
            working_dir: repo.path().to_path_buf(),
            spec: spec(),
            reviewer_preamble: None,
            dagrs_resume_checkpoint_id: None,
            observer: None,
        };
        let registry = ToolRegistry::new();
        let model = MockModel {
            final_text: "unused".into(),
        };

        let result = execute_task(&task, &model, &registry, &config).await;

        assert_eq!(result.status, TaskNodeStatus::Completed);
        assert!(!repo.path().join("gate-output.txt").exists());
        assert_eq!(
            std::fs::read_to_string(repo.path().join("tracked.txt")).unwrap(),
            "base\n"
        );
    }

    #[tokio::test]
    async fn test_execute_implementation_task() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        let mut registry = ToolRegistry::with_working_dir(dir.path().to_path_buf());
        registry.register("apply_patch", Arc::new(ApplyPatchHandler));
        let registry = Arc::new(registry);
        let config = ExecutorConfig {
            tool_loop_config: ToolLoopConfig::default(),
            max_retries: 1,
            backoff_seconds: 0,
            working_dir: dir.path().to_path_buf(),
            spec: spec(),
            reviewer_preamble: None,
            dagrs_resume_checkpoint_id: None,
            observer: None,
        };
        let task = implementation_task();
        let result = execute_task(&task, &SrcMainPatchModel, &registry, &config).await;
        assert_eq!(result.status, TaskNodeStatus::Completed);
        assert_eq!(result.retry_count, 0);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("src/main.rs")).unwrap(),
            "fn main() { println!(\"hello\"); }\n"
        );
    }

    #[tokio::test]
    async fn execute_implementation_task_requires_file_write() {
        let model = MockModel {
            final_text: "done".into(),
        };
        let dir = tempfile::tempdir().unwrap();
        let registry = Arc::new(ToolRegistry::with_working_dir(dir.path().to_path_buf()));
        let config = ExecutorConfig {
            tool_loop_config: ToolLoopConfig::default(),
            max_retries: 0,
            backoff_seconds: 0,
            working_dir: dir.path().to_path_buf(),
            spec: spec(),
            reviewer_preamble: None,
            dagrs_resume_checkpoint_id: None,
            observer: None,
        };
        let task = implementation_task();

        let result = execute_task(&task, &model, &registry, &config).await;

        assert_eq!(result.status, TaskNodeStatus::Failed);
        assert!(
            result
                .agent_output
                .as_deref()
                .is_some_and(|output| output.contains("without writing any files"))
        );
        assert!(result.tool_calls.is_empty());
    }

    #[tokio::test]
    async fn execute_implementation_task_retries_with_missing_write_feedback() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        let mut registry = ToolRegistry::with_working_dir(dir.path().to_path_buf());
        registry.register("apply_patch", Arc::new(ApplyPatchHandler));
        let registry = Arc::new(registry);
        let config = ExecutorConfig {
            tool_loop_config: ToolLoopConfig::default(),
            max_retries: 1,
            backoff_seconds: 0,
            working_dir: dir.path().to_path_buf(),
            spec: spec(),
            reviewer_preamble: None,
            dagrs_resume_checkpoint_id: None,
            observer: None,
        };

        let result = execute_task(
            &implementation_task(),
            &RetryAfterNoWriteModel,
            &registry,
            &config,
        )
        .await;

        assert_eq!(result.status, TaskNodeStatus::Completed);
        assert_eq!(result.retry_count, 1);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("src/main.rs")).unwrap(),
            "fn main() { println!(\"fixed\"); }\n"
        );
    }

    #[tokio::test]
    #[serial]
    async fn execute_dag_fails_noop_implementation_task() {
        let repo = tempfile::tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;

        let registry = Arc::new(ToolRegistry::with_working_dir(repo.path().to_path_buf()));
        let config = ExecutorConfig {
            tool_loop_config: ToolLoopConfig::default(),
            max_retries: 0,
            backoff_seconds: 0,
            working_dir: repo.path().to_path_buf(),
            spec: spec(),
            reviewer_preamble: None,
            dagrs_resume_checkpoint_id: None,
            observer: None,
        };
        let plan = plan_for_tasks(vec![implementation_task()], 1);
        let model = MockModel {
            final_text: "done".into(),
        };

        let run_state = execute_dag(&plan, &model, &registry, &config)
            .await
            .unwrap();

        let result = &run_state.ordered_task_results()[0];
        assert_eq!(result.status, TaskNodeStatus::Failed);
        assert!(
            result
                .agent_output
                .as_deref()
                .is_some_and(|output| output.contains("without writing any files"))
        );
        assert!(!repo.path().join("src/main.rs").exists());
    }

    #[tokio::test]
    #[serial]
    async fn execute_dag_replays_parallel_task_worktrees_back_to_main_workspace() {
        let repo = tempfile::tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        std::fs::write(repo.path().join("task_a.txt"), "base\n").unwrap();
        std::fs::write(repo.path().join("task_b.txt"), "base\n").unwrap();

        let mut registry = ToolRegistry::with_working_dir(repo.path().to_path_buf());
        registry.register("apply_patch", Arc::new(ApplyPatchHandler));
        let registry = Arc::new(registry);

        let config = ExecutorConfig {
            tool_loop_config: ToolLoopConfig::default(),
            max_retries: 0,
            backoff_seconds: 0,
            working_dir: repo.path().to_path_buf(),
            spec: spec(),
            reviewer_preamble: None,
            dagrs_resume_checkpoint_id: None,
            observer: None,
        };

        let plan = plan_for_tasks(
            vec![
                scoped_implementation_task("Task A", "task_a.txt"),
                scoped_implementation_task("Task B", "task_b.txt"),
            ],
            2,
        );

        let run_state = execute_dag(&plan, &PatchApplyingModel, &registry, &config)
            .await
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(repo.path().join("task_a.txt")).unwrap(),
            "task-a\n"
        );
        assert_eq!(
            std::fs::read_to_string(repo.path().join("task_b.txt")).unwrap(),
            "task-b\n"
        );
        assert!(
            run_state
                .ordered_task_results()
                .iter()
                .all(|result| result.status == TaskNodeStatus::Completed)
        );
    }

    #[tokio::test]
    #[serial]
    async fn execute_dag_keeps_main_workspace_clean_when_serial_task_fails() {
        let repo = tempfile::tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        std::fs::write(repo.path().join("task_a.txt"), "base\n").unwrap();

        let mut registry = ToolRegistry::with_working_dir(repo.path().to_path_buf());
        registry.register("apply_patch", Arc::new(ApplyPatchHandler));
        let registry = Arc::new(registry);

        let config = ExecutorConfig {
            tool_loop_config: ToolLoopConfig::default(),
            max_retries: 0,
            backoff_seconds: 0,
            working_dir: repo.path().to_path_buf(),
            spec: spec(),
            reviewer_preamble: None,
            dagrs_resume_checkpoint_id: None,
            observer: None,
        };

        let plan = plan_for_tasks(vec![scoped_implementation_task("Task A", "task_a.txt")], 1);

        let run_state = execute_dag(&plan, &PatchThenFailModel, &registry, &config)
            .await
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(repo.path().join("task_a.txt")).unwrap(),
            "base\n"
        );
        assert_eq!(
            run_state.ordered_task_results()[0].status,
            TaskNodeStatus::Failed
        );
    }

    #[test]
    fn task_prompt_includes_runtime_workspace() {
        let prompt = build_task_prompt(
            &implementation_task(),
            Path::new("/tmp/workspace"),
            &["read_file".into(), "apply_patch".into()],
        );
        assert!(prompt.contains("Working directory: /tmp/workspace"));
        assert!(prompt.contains("Allowed Tools"));
    }

    #[test]
    fn task_prompt_marks_touch_files_as_hard_write_contract() {
        let prompt = build_task_prompt(
            &implementation_task(),
            Path::new("/tmp/workspace"),
            &["read_file".into(), "apply_patch".into()],
        );

        assert!(prompt.contains("## Write Contract"));
        assert!(prompt.contains("Only modify these files"));
        assert!(prompt.contains("## Version Control"));
        assert!(prompt.contains("Do not use git"));
        assert!(prompt.contains("run_libra_vcs"));
        assert!(prompt.contains("Allowed run_libra_vcs commands"));
        assert!(prompt.contains("status --json"));
        assert!(prompt.contains("ls-files"));
        assert!(prompt.contains("status -uall"));
        assert!(prompt.contains("must not be used for cargo"));
        assert!(prompt.contains("verification commands are owned by gate tasks"));
        assert!(prompt.contains("## Completion Requirement"));
        assert!(prompt.contains("Before reporting completion"));
        assert!(prompt.contains("src/main.rs"));
    }

    #[test]
    fn parses_reviewer_json_from_fenced_block_after_markdown_notes() {
        let raw = "\
Review notes:
- Do not parse this pseudo object: {approved: true}

```json
{\"approved\":true,\"summary\":\"clap dependency added\",\"issues\":[]}
```

Done.";

        let decision = parse_reviewer_decision(raw).unwrap();

        assert!(decision.approved);
        assert_eq!(decision.summary, "clap dependency added");
        assert!(decision.issues.is_empty());
    }

    #[test]
    fn parses_reviewer_json_embedded_in_plain_text() {
        let raw = "The change is acceptable. {\"approved\":false,\"summary\":\"needs test\",\"issues\":[\"missing CLI test\"]}";

        let decision = parse_reviewer_decision(raw).unwrap();

        assert!(!decision.approved);
        assert_eq!(decision.summary, "needs test");
        assert_eq!(decision.issues, vec!["missing CLI test"]);
    }

    #[test]
    fn rejects_reviewer_text_without_json_object() {
        let error = parse_reviewer_decision("approved: true").unwrap_err();

        assert!(error.contains("missing JSON object"));
    }

    #[test]
    fn reviewer_prompt_includes_write_contract() {
        let record = ToolCallRecord {
            paths_written: vec!["src/main.rs".into()],
            ..ToolCallRecord::default()
        };
        let prompt = build_reviewer_prompt(
            &implementation_task(),
            "done",
            &[record],
            Path::new("/tmp/workspace"),
            &["read_file".into()],
        );

        assert!(prompt.contains("## Write Contract"));
        assert!(prompt.contains("Only these paths may be modified"));
        assert!(prompt.contains("src/main.rs"));
    }

    #[test]
    fn default_acl_does_not_expose_shell_to_coder() {
        let task = implementation_task();
        let mut spec = (*spec()).clone();
        spec.security = profiles::default_security();
        spec.constraints.security.network_policy = NetworkPolicy::Deny;
        let tools = allowed_tools_for_task(&spec, &task);
        assert!(tools.contains(&"read_file".to_string()));
        assert!(tools.contains(&"apply_patch".to_string()));
        assert!(tools.contains(&"run_libra_vcs".to_string()));
        assert!(!tools.contains(&"shell".to_string()));
        assert!(!tools.contains(&"web_search".to_string()));
    }

    #[test]
    fn network_allow_exposes_web_search_to_coder() {
        let task = analysis_task();
        let mut spec = (*spec()).clone();
        spec.security = profiles::default_security();
        spec.constraints.security.network_policy = NetworkPolicy::Allow;

        let tools = allowed_tools_for_task(&spec, &task);

        assert!(tools.contains(&"web_search".to_string()));
    }

    #[test]
    fn gate_runtime_uses_workspace_write_sandbox() {
        let runtime = runtime_context_for_gate_task(&spec(), Path::new("/tmp/workspace"), None);
        let sandbox = runtime
            .sandbox
            .expect("gate tasks should always execute with sandbox context");
        assert!(matches!(
            sandbox.policy,
            SandboxPolicy::WorkspaceWrite {
                writable_roots,
                ..
            } if writable_roots == vec![PathBuf::from("/tmp/workspace")]
        ));
    }

    #[test]
    fn task_runtime_prefers_touch_files_as_writable_roots() {
        let workspace = tempfile::tempdir().unwrap();
        let src_dir = workspace.path().join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::write(src_dir.join("main.rs"), "fn main() {}\n").unwrap();
        let expected_root = workspace.path().join("src/main.rs").canonicalize().unwrap();

        let runtime =
            runtime_context_for_task(&spec(), &implementation_task(), workspace.path(), None);
        let sandbox = runtime
            .sandbox
            .expect("implementation tasks should always execute with sandbox context");
        assert!(matches!(
            sandbox.policy,
            SandboxPolicy::WorkspaceWrite {
                writable_roots,
                ..
            } if writable_roots == vec![expected_root]
        ));
    }

    #[test]
    fn task_runtime_falls_back_to_scope_roots_when_touch_files_are_absent() {
        let mut task = implementation_task();
        task.contract.touch_files.clear();
        task.scope_in = vec!["src/**/*.rs".into()];
        task.contract.write_scope = vec!["src/**/*.rs".into()];

        let runtime = runtime_context_for_task(&spec(), &task, Path::new("/tmp/workspace"), None);
        let sandbox = runtime
            .sandbox
            .expect("implementation tasks should always execute with sandbox context");
        assert!(matches!(
            sandbox.policy,
            SandboxPolicy::WorkspaceWrite {
                writable_roots,
                ..
            } if writable_roots == vec![PathBuf::from("/tmp/workspace/src")]
        ));
    }

    #[test]
    fn analysis_tasks_do_not_get_apply_patch() {
        let task = analysis_task();
        let mut spec = (*spec()).clone();
        spec.security = profiles::default_security();
        let tools = allowed_tools_for_task(&spec, &task);
        assert!(tools.contains(&"read_file".to_string()));
        assert!(tools.contains(&"run_libra_vcs".to_string()));
        assert!(!tools.contains(&"apply_patch".to_string()));
    }

    #[tokio::test]
    async fn execute_dag_uses_real_dependencies_without_batch_barriers() {
        let model = ConditionalModel;
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        let mut registry = ToolRegistry::with_working_dir(dir.path().to_path_buf());
        registry.register("apply_patch", Arc::new(ApplyPatchHandler));
        let registry = Arc::new(registry);
        let events = Arc::new(Mutex::new(Vec::new()));
        let config = ExecutorConfig {
            tool_loop_config: ToolLoopConfig::default(),
            max_retries: 0,
            backoff_seconds: 0,
            working_dir: dir.path().to_path_buf(),
            spec: spec(),
            reviewer_preamble: None,
            dagrs_resume_checkpoint_id: None,
            observer: Some(Arc::new(RecordingObserver {
                events: Arc::clone(&events),
            })),
        };

        let a = scoped_implementation_task("A", "src/a.txt");
        let mut c = scoped_implementation_task("C", "src/c.txt");
        c.task.add_dependency(a.id());

        let b = scoped_implementation_task("B", "src/b.txt");

        let plan = plan_for_tasks(vec![a, c, b], 2);

        let run_state = execute_dag(&plan, &model, &registry, &config)
            .await
            .unwrap();

        let timeline = events.lock().unwrap().clone();
        let c_started_at = timeline
            .iter()
            .position(|event| event == "start:C")
            .expect("C should start");
        let b_completed_at = timeline
            .iter()
            .position(|event| event == "done:B")
            .expect("B should complete");
        assert!(
            c_started_at < b_completed_at,
            "C should start before B completes when only real dependencies are wired"
        );
        assert_eq!(run_state.task_results.len(), 3);
        assert_eq!(run_state.dagrs_runtime.total_nodes, 3);
        assert_eq!(run_state.dagrs_runtime.completed_nodes, 3);
        assert!(
            run_state
                .ordered_task_results()
                .iter()
                .all(|result| result.status == TaskNodeStatus::Completed)
        );
    }

    #[tokio::test]
    async fn execute_dag_skips_dependent_tasks_after_failure() {
        let model = ConditionalModel;
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        let mut registry = ToolRegistry::with_working_dir(dir.path().to_path_buf());
        registry.register("apply_patch", Arc::new(ApplyPatchHandler));
        let registry = Arc::new(registry);
        let events = Arc::new(Mutex::new(Vec::new()));
        let config = ExecutorConfig {
            tool_loop_config: ToolLoopConfig::default(),
            max_retries: 0,
            backoff_seconds: 0,
            working_dir: dir.path().to_path_buf(),
            spec: spec(),
            reviewer_preamble: None,
            dagrs_resume_checkpoint_id: None,
            observer: Some(Arc::new(RecordingObserver {
                events: Arc::clone(&events),
            })),
        };

        let mut failing = implementation_task();
        failing.task = {
            let actor = ActorRef::agent("test-executor").unwrap();
            let mut task = GitTask::new(actor, "Fail first", None).unwrap();
            task.set_description(Some("Implement change".into()));
            task.add_constraint("network:allow");
            task.add_acceptance_criterion("tests pass");
            task
        };
        failing.objective = "Fail first".into();

        let mut later = implementation_task();
        later.task = {
            let actor = ActorRef::agent("test-executor").unwrap();
            let mut task = GitTask::new(actor, "Later", None).unwrap();
            task.set_description(Some("Implement change".into()));
            task.add_constraint("network:allow");
            task.add_acceptance_criterion("tests pass");
            task
        };
        later.objective = "Later".into();
        later.task.add_dependency(failing.id());

        let plan = plan_for_tasks(vec![failing.clone(), later.clone()], 1);

        let run_state = execute_dag(&plan, &model, &registry, &config)
            .await
            .unwrap();

        let timeline = events.lock().unwrap().clone();
        let start_order: Vec<String> = timeline
            .iter()
            .filter_map(|event| event.strip_prefix("start:").map(ToString::to_string))
            .collect();
        assert_eq!(start_order, vec!["Fail first"]);
        assert_eq!(run_state.task_results.len(), 2);
        assert_eq!(run_state.task_results[0].task_id, failing.id());
        assert_eq!(run_state.task_results[0].status, TaskNodeStatus::Failed);
        assert_eq!(run_state.task_results[1].task_id, later.id());
        assert_eq!(run_state.task_results[1].status, TaskNodeStatus::Skipped);
        assert_eq!(run_state.dagrs_runtime.total_nodes, 2);
        assert_eq!(run_state.dagrs_runtime.completed_nodes, 2);
        assert_eq!(run_state.status_for(later.id()), TaskNodeStatus::Skipped);
    }

    #[tokio::test]
    async fn execute_dag_records_cost_budget_abort_as_failure() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        let mut registry = ToolRegistry::with_working_dir(dir.path().to_path_buf());
        registry.register("apply_patch", Arc::new(ApplyPatchHandler));
        let registry = Arc::new(registry);
        let mut spec = (*spec()).clone();
        spec.constraints.resources.max_cost_units = 1;
        spec.execution.concurrency.max_parallel_tasks = 1;
        let config = ExecutorConfig {
            tool_loop_config: ToolLoopConfig::default(),
            max_retries: 0,
            backoff_seconds: 0,
            working_dir: dir.path().to_path_buf(),
            spec: Arc::new(spec),
            reviewer_preamble: None,
            dagrs_resume_checkpoint_id: None,
            observer: None,
        };

        let first = implementation_task();
        let mut second = implementation_task();
        second.task = {
            let actor = ActorRef::agent("test-executor").unwrap();
            let mut task = GitTask::new(actor, "Second", None).unwrap();
            task.set_description(Some("Implement change".into()));
            task.add_constraint("network:allow");
            task.add_acceptance_criterion("tests pass");
            task
        };
        second.objective = "Second".into();

        let plan = plan_for_tasks(vec![first.clone(), second.clone()], 1);
        let run_state = execute_dag(&plan, &AddFilePatchModel, &registry, &config)
            .await
            .unwrap();

        let completed = run_state
            .ordered_task_results()
            .iter()
            .filter(|result| result.status == TaskNodeStatus::Completed)
            .count();
        let failed = run_state
            .ordered_task_results()
            .iter()
            .filter(|result| result.status == TaskNodeStatus::Failed)
            .count();
        assert_eq!(completed, 1, "{:?}", run_state.ordered_task_results());
        assert_eq!(failed, 1, "{:?}", run_state.ordered_task_results());

        let failure = run_state
            .ordered_task_results()
            .iter()
            .find(|result| result.status == TaskNodeStatus::Failed)
            .expect("budget-exhausted task result should be recorded");
        assert!(
            failure
                .agent_output
                .as_deref()
                .is_some_and(|message| message.contains("cost budget exceeded")),
            "{failure:?}"
        );
        assert!(
            [first.id(), second.id()].into_iter().all(|task_id| {
                !matches!(
                    run_state.status_for(task_id),
                    TaskNodeStatus::Pending | TaskNodeStatus::Running
                )
            }),
            "{:?}",
            run_state.task_statuses
        );
    }

    #[tokio::test]
    async fn execute_dag_rejects_unimplemented_checkpoint_resume() {
        let registry = Arc::new(ToolRegistry::new());
        let dir = tempfile::tempdir().unwrap();

        let task = implementation_task();
        let plan = plan_for_tasks(vec![task], 1);

        let resume_config = ExecutorConfig {
            tool_loop_config: ToolLoopConfig::default(),
            max_retries: 0,
            backoff_seconds: 0,
            working_dir: dir.path().to_path_buf(),
            spec: spec(),
            reviewer_preamble: None,
            dagrs_resume_checkpoint_id: Some("todo".into()),
            observer: None,
        };
        let err = execute_dag(
            &plan,
            &MockModel {
                final_text: "done".into(),
            },
            &registry,
            &resume_config,
        )
        .await
        .expect_err("resume should remain disabled until checkpoint semantics are redesigned");

        let message = err.to_string();
        assert!(message.contains("not supported yet"));
        assert!(message.contains("userspace-fs"));
    }
}
