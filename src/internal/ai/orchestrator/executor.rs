use std::{
    collections::{HashMap, HashSet},
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use dagrs::{
    Action, CheckpointConfig, DefaultNode, EnvVar, FileCheckpointStore, Graph, InChannels, Node,
    NodeTable, OutChannels, Output, event::GraphEvent,
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
        TaskKind, TaskNodeStatus, TaskResult, TaskSpec, ToolCallRecord,
    },
    workspace::{cleanup_task_worktree, prepare_task_worktree, sync_task_worktree_back},
};
use crate::{
    internal::ai::{
        agent::{ToolLoopConfig, ToolLoopObserver, run_tool_loop_with_history_and_observer},
        completion::{CompletionError, CompletionModel},
        hooks::HookRunner,
        intentspec::types::{IntentSpec, NetworkPolicy},
        sandbox::{SandboxPermissions, SandboxPolicy, ToolRuntimeContext, ToolSandboxContext},
        tools::{ToolOutput, registry::ToolRegistry},
    },
    utils::util,
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
            observer,
        }
    }

    fn finish(self) -> (Vec<ToolCallRecord>, Vec<super::types::PolicyViolation>) {
        (self.tool_calls, self.violations)
    }
}

impl ToolLoopObserver for TaskExecutionObserver {
    fn on_assistant_step_text(&mut self, text: &str) {
        if let Some(observer) = &self.observer {
            observer.on_task_assistant_message(&self.task, text);
        }
    }

    fn on_tool_call_begin(&mut self, call_id: &str, tool_name: &str, arguments: &Value) {
        if let Some(observer) = &self.observer {
            observer.on_tool_call_begin(&self.task, call_id, tool_name, arguments);
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
            observer.on_tool_call_end(&self.task, call_id, tool_name, result);
        }
        if let Some(mut record) = self.in_flight.remove(call_id) {
            match result {
                Ok(output) => {
                    if let Err(violation) =
                        policy::evaluate_tool_result(&self.spec, tool_name, output, &mut record)
                    {
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

#[derive(Deserialize)]
struct ReviewerDecision {
    approved: bool,
    summary: String,
    #[serde(default)]
    issues: Vec<String>,
}

/// Execute a single task with retry logic.
pub async fn execute_task<M: CompletionModel>(
    task: &TaskSpec,
    model: &M,
    registry: &ToolRegistry,
    config: &ExecutorConfig,
) -> TaskResult {
    if task.kind == TaskKind::Gate {
        return execute_gate_task(
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
    let mut last_review = None;

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
        let agent_result = run_tool_loop_with_history_and_observer(
            model,
            Vec::new(),
            &prompt,
            registry,
            tool_loop_config,
            &mut observer,
        )
        .await;
        let (tool_calls, policy_violations) = observer.finish();
        accumulated_tool_calls.extend(tool_calls.iter().cloned());
        accumulated_policy_violations.extend(policy_violations.iter().cloned());

        let retryable_failure = match agent_result {
            Ok(turn) if policy_violations.is_empty() => {
                let review = match run_reviewer_pass(
                    task,
                    &turn.final_text,
                    &accumulated_tool_calls,
                    model,
                    registry,
                    config,
                )
                .await
                {
                    Ok(review) => review,
                    Err(message) => {
                        if let Some(observer) = &config.observer {
                            observer.on_reviewer_completed(task, None);
                        }
                        return TaskResult {
                            task_id: task.id(),
                            status: TaskNodeStatus::Failed,
                            gate_report: None,
                            agent_output: Some(message),
                            retry_count,
                            tool_calls: accumulated_tool_calls,
                            policy_violations: accumulated_policy_violations,
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
                        review,
                    };
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
                review: last_review,
            };
        }

        tracing::warn!(task_id = %task.id(), "retrying task after failure: {}", failure_reason);
        if config.backoff_seconds > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(
                config.backoff_seconds as u64,
            ))
            .await;
        }
    }
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
                observer.on_gate_check_started(task, check);
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
                observer.on_gate_check_completed(task, check, &result);
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
        review: None,
    }
}

async fn run_reviewer_pass<M: CompletionModel>(
    task: &TaskSpec,
    agent_output: &str,
    tool_calls: &[ToolCallRecord],
    model: &M,
    registry: &ToolRegistry,
    config: &ExecutorConfig,
) -> Result<Option<ReviewOutcome>, String> {
    let Some(reviewer_preamble) = config.reviewer_preamble.clone() else {
        return Ok(None);
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
        observer.on_reviewer_started(task);
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
    .await
    .map_err(|err| format!("reviewer pass failed: {err}"))?;

    let review = parse_reviewer_decision(&turn.final_text)?;
    let outcome = ReviewOutcome {
        approved: review.approved,
        summary: review.summary,
        issues: review.issues,
    };
    if let Some(observer) = &config.observer {
        observer.on_reviewer_completed(task, Some(&outcome));
    }
    Ok(Some(outcome))
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

fn should_use_task_worktree(task: &TaskSpec, worktree_parallelism: bool) -> bool {
    worktree_parallelism && task.kind == TaskKind::Implementation
}

async fn execute_task_in_task_worktree<M: CompletionModel>(
    task: &TaskSpec,
    model: &M,
    registry: &Arc<ToolRegistry>,
    config: &ExecutorConfig,
    workspace_sync: &Arc<tokio::sync::Mutex<()>>,
) -> TaskResult {
    let prepared = match tokio::task::spawn_blocking({
        let working_dir = config.working_dir.clone();
        let task_id = task.id();
        move || prepare_task_worktree(&working_dir, task_id)
    })
    .await
    {
        Ok(Ok(worktree)) => worktree,
        Ok(Err(err)) => return task_workspace_failure(task, err),
        Err(err) => {
            return task_workspace_failure(
                task,
                io::Error::other(format!("failed to prepare task worktree: {err}")),
            );
        }
    };

    let task_registry = Arc::new(registry.clone_with_working_dir(prepared.root.clone()));
    let mut task_config = config.clone();
    task_config.working_dir = prepared.root.clone();
    task_config.tool_loop_config =
        clone_tool_loop_config_for_workdir(&config.tool_loop_config, &prepared.root);
    if let Some(observer) = &config.observer {
        observer.on_task_workspace_ready(task, &prepared.root, true);
    }

    let mut result = execute_task(task, model, &task_registry, &task_config).await;

    if result.status == TaskNodeStatus::Completed {
        let sync_result = {
            let _guard = workspace_sync.lock().await;
            tokio::task::spawn_blocking({
                let main_working_dir = config.working_dir.clone();
                let task_worktree_dir = prepared.root.clone();
                let baseline = prepared.baseline.clone();
                let touch_files = task.contract.touch_files.clone();
                let scope_in = task.scope_in.clone();
                let scope_out = task.scope_out.clone();
                move || {
                    sync_task_worktree_back(
                        &main_working_dir,
                        &task_worktree_dir,
                        &baseline,
                        &touch_files,
                        &scope_in,
                        &scope_out,
                    )
                }
            })
            .await
        };

        match sync_result {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                result.status = TaskNodeStatus::Failed;
                result.agent_output = Some(format!(
                    "task completed in isolated worktree but failed to sync changes back: {err}"
                ));
            }
            Err(err) => {
                result.status = TaskNodeStatus::Failed;
                result.agent_output = Some(format!(
                    "task completed in isolated worktree but sync worker failed: {err}"
                ));
            }
        }
    }

    let cleanup_result = tokio::task::spawn_blocking({
        let task_worktree_dir = prepared.root.clone();
        move || cleanup_task_worktree(&task_worktree_dir)
    })
    .await;
    match cleanup_result {
        Err(err) => tracing::warn!("task worktree cleanup worker failed: {}", err),
        Ok(Err(err)) => tracing::warn!(
            path = %prepared.root.display(),
            "failed to clean up task worktree: {}",
            err
        ),
        Ok(Ok(())) => {}
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
        observer.on_task_completed(task, &result);
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
    worktree_parallelism: bool,
}

#[derive(Clone)]
struct DagrsBuildContext {
    run_state: RunStateStore,
    metered_task_ids: Arc<HashSet<Uuid>>,
    parallelism: Arc<Semaphore>,
    cost_budget_serial: Arc<tokio::sync::Mutex<()>>,
    workspace_sync_serial: Arc<tokio::sync::Mutex<()>>,
    worktree_parallelism: bool,
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
impl<M: CompletionModel + 'static> Action for TaskDagrsAction<M> {
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
                return Output::error(message);
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
                return Output::error(message);
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
                    return Output::error(message);
                }
                cost_budget_guard = Some(guard);
            }
        }

        if let Some(observer) = &self.config.observer {
            observer.on_task_started(&self.task);
        }

        let use_task_worktree = should_use_task_worktree(&self.task, self.worktree_parallelism);
        if !use_task_worktree && let Some(observer) = &self.config.observer {
            observer.on_task_workspace_ready(&self.task, &self.config.working_dir, false);
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
            observer.on_task_completed(&self.task, &result);
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
                Output::error(
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
                Output::error(format!(
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
) -> Result<Graph, OrchestratorError> {
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
            worktree_parallelism: context.worktree_parallelism,
        };
        let dagrs_node =
            DefaultNode::with_action(task_spec.id().to_string(), action, &mut node_table);
        let dagrs_id = dagrs_node.id();
        graph.add_node(dagrs_node);
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
            graph.add_edge(from_id, vec![to_id]);
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
            Ok(GraphEvent::GraphFinished) => break,
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
) -> Result<RunStateSnapshot, OrchestratorError> {
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
    let worktree_parallelism = plan_spec.max_parallel > 1
        && util::try_get_storage_path(Some(config.working_dir.clone())).is_ok();
    let graph_context = DagrsBuildContext {
        run_state: run_state_snapshot,
        metered_task_ids,
        parallelism,
        cost_budget_serial,
        workspace_sync_serial,
        worktree_parallelism,
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

    let execution_error = execution_result.err().map(|err| {
        tracing::warn!("dagrs execution terminated with error: {}", err);
        err.to_string()
    });

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

    if !task.contract.touch_files.is_empty() {
        parts.push(format!(
            "## Touch Hints\nFiles: {}",
            task.contract.touch_files.join(", ")
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
        ]);
    }

    if task.kind == TaskKind::Implementation
        && acl_allows(&spec.security.tool_acl, "workspace.fs", "write")
    {
        tools.push("apply_patch".to_string());
    }

    if matches!(task.kind, TaskKind::Implementation | TaskKind::Analysis)
        && acl_allows(&spec.security.tool_acl, "shell", "execute")
    {
        tools.push("shell".to_string());
    }

    tools
}

fn allowed_tools_for_reviewer(spec: &IntentSpec) -> Vec<String> {
    if acl_allows(&spec.security.tool_acl, "workspace.fs", "read") {
        vec![
            "read_file".to_string(),
            "list_dir".to_string(),
            "grep_files".to_string(),
        ]
    } else {
        Vec::new()
    }
}

fn acl_allows(
    acl: &crate::internal::ai::intentspec::types::ToolAcl,
    tool: &str,
    action: &str,
) -> bool {
    matches!(check_tool_acl(acl, tool, action), AclVerdict::Allow)
}

fn parse_reviewer_decision(raw: &str) -> Result<ReviewerDecision, String> {
    if let Ok(parsed) = serde_json::from_str::<ReviewerDecision>(raw.trim()) {
        return Ok(parsed);
    }

    let start = raw
        .find('{')
        .ok_or_else(|| "reviewer response missing JSON object".to_string())?;
    let end = raw
        .rfind('}')
        .ok_or_else(|| "reviewer response missing JSON terminator".to_string())?;
    serde_json::from_str::<ReviewerDecision>(&raw[start..=end])
        .map_err(|err| format!("invalid reviewer JSON: {err}"))
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
            network_access: crate::internal::ai::sandbox::NetworkAccess::Enabled,
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

fn max_output_limit(
    acl: &crate::internal::ai::intentspec::types::ToolAcl,
    tool_name: &str,
    action: &str,
) -> Option<usize> {
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

    use super::*;
    use crate::{
        internal::ai::{
            completion::{
                CompletionError, CompletionRequest, CompletionResponse,
                message::{AssistantContent, Function, Message, Text, ToolCall, UserContent},
            },
            intentspec::{profiles, types::*},
            orchestrator::types::{
                ExecutionPlanSpec, GateResult, TaskContract, TaskKind, TaskSpec,
            },
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

            if prompt.contains("## Task\nB") {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }

            if prompt.contains("Fail first") {
                Err(CompletionError::ResponseError("intentional failure".into()))
            } else {
                Ok(CompletionResponse {
                    content: vec![AssistantContent::Text(Text {
                        text: "done".into(),
                    })],
                    raw_response: (),
                })
            }
        }
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
                raw_response: (),
            })
        }
    }

    struct RecordingObserver {
        events: Arc<Mutex<Vec<String>>>,
    }

    impl OrchestratorObserver for RecordingObserver {
        fn on_task_started(&self, task: &TaskSpec) {
            self.events
                .lock()
                .unwrap()
                .push(format!("start:{}", task.title()));
        }

        fn on_task_completed(&self, task: &TaskSpec, _result: &TaskResult) {
            self.events
                .lock()
                .unwrap()
                .push(format!("done:{}", task.title()));
        }

        fn on_gate_check_started(&self, task: &TaskSpec, check: &Check) {
            self.events
                .lock()
                .unwrap()
                .push(format!("gate-start:{}:{}", task.title(), check.id));
        }

        fn on_gate_check_completed(&self, task: &TaskSpec, check: &Check, result: &GateResult) {
            self.events.lock().unwrap().push(format!(
                "gate-done:{}:{}:{}",
                task.title(),
                check.id,
                if result.passed { "pass" } else { "fail" }
            ));
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
        assert!(recorded.contains(&"gate-start:Do thing:fmt".to_string()));
        assert!(recorded.contains(&"gate-done:Do thing:fmt:pass".to_string()));
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
    async fn test_execute_implementation_task() {
        let model = MockModel {
            final_text: "done".into(),
        };
        let registry = Arc::new(ToolRegistry::new());
        let dir = tempfile::tempdir().unwrap();
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
        let result = execute_task(&task, &model, &registry, &config).await;
        assert_eq!(result.status, TaskNodeStatus::Completed);
        assert_eq!(result.retry_count, 0);
    }

    #[tokio::test]
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
    fn default_acl_does_not_expose_shell_to_coder() {
        let task = implementation_task();
        let mut spec = (*spec()).clone();
        spec.security = profiles::default_security();
        let tools = allowed_tools_for_task(&spec, &task);
        assert!(tools.contains(&"read_file".to_string()));
        assert!(tools.contains(&"apply_patch".to_string()));
        assert!(!tools.contains(&"shell".to_string()));
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
        assert!(!tools.contains(&"apply_patch".to_string()));
    }

    #[tokio::test]
    async fn execute_dag_uses_real_dependencies_without_batch_barriers() {
        let model = ConditionalModel;
        let registry = Arc::new(ToolRegistry::new());
        let dir = tempfile::tempdir().unwrap();
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

        let mut a = implementation_task();
        a.task = {
            let actor = ActorRef::agent("test-executor").unwrap();
            let mut task = GitTask::new(actor, "A", None).unwrap();
            task.set_description(Some("Implement change".into()));
            task.add_constraint("network:allow");
            task.add_acceptance_criterion("tests pass");
            task
        };
        a.objective = "A".into();

        let mut c = implementation_task();
        c.task = {
            let actor = ActorRef::agent("test-executor").unwrap();
            let mut task = GitTask::new(actor, "C", None).unwrap();
            task.set_description(Some("Implement change".into()));
            task.add_constraint("network:allow");
            task.add_acceptance_criterion("tests pass");
            task
        };
        c.objective = "C".into();
        c.task.add_dependency(a.id());

        let mut b = implementation_task();
        b.task = {
            let actor = ActorRef::agent("test-executor").unwrap();
            let mut task = GitTask::new(actor, "B", None).unwrap();
            task.set_description(Some("Implement change".into()));
            task.add_constraint("network:allow");
            task.add_acceptance_criterion("tests pass");
            task
        };
        b.objective = "B".into();

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
        let registry = Arc::new(ToolRegistry::new());
        let dir = tempfile::tempdir().unwrap();
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
        let model = MockModel {
            final_text: "done".into(),
        };
        let registry = Arc::new(ToolRegistry::new());
        let dir = tempfile::tempdir().unwrap();
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
        let run_state = execute_dag(&plan, &model, &registry, &config)
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
