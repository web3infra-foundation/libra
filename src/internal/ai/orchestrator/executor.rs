use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;
use dagrs::{Action, DefaultNode, EnvVar, Graph, InChannels, Node, NodeTable, OutChannels, Output};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use super::{
    acl::{AclVerdict, check_tool_acl},
    gate, policy,
    run_state::{RunStateSnapshot, RunStateStore},
    types::{
        ExecutionPlan, GateReport, OrchestratorError, OrchestratorObserver, ReviewOutcome,
        TaskDAG, TaskKind, TaskNode,
        TaskNodeStatus, TaskResult, ToolCallRecord,
    },
};
use crate::internal::ai::{
    agent::{ToolLoopConfig, ToolLoopObserver, run_tool_loop_with_history_and_observer},
    completion::{CompletionError, CompletionModel},
    intentspec::types::IntentSpec,
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
    pub observer: Option<Arc<dyn OrchestratorObserver>>,
}

struct TaskExecutionObserver {
    spec: Arc<IntentSpec>,
    task: TaskNode,
    working_dir: PathBuf,
    in_flight: HashMap<String, ToolCallRecord>,
    tool_calls: Vec<ToolCallRecord>,
    violations: Vec<super::types::PolicyViolation>,
    observer: Option<Arc<dyn OrchestratorObserver>>,
}

impl TaskExecutionObserver {
    fn new(
        spec: Arc<IntentSpec>,
        task: TaskNode,
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
    task: &TaskNode,
    model: &M,
    registry: &ToolRegistry,
    config: &ExecutorConfig,
) -> TaskResult {
    if task.kind == TaskKind::Gate {
        return execute_gate_task(task, &config.working_dir).await;
    }

    let allowed_tools = allowed_tools_for_task(&config.spec, task);
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
                            task_id: task.id,
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
                        task_id: task.id,
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
                    task_id: task.id,
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
                task_id: task.id,
                status: TaskNodeStatus::Failed,
                gate_report: None,
                agent_output,
                retry_count,
                tool_calls: accumulated_tool_calls,
                policy_violations: accumulated_policy_violations,
                review: last_review,
            };
        }

        tracing::warn!(task_id = %task.id, "retrying task after failure: {}", failure_reason);
        if config.backoff_seconds > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(
                config.backoff_seconds as u64,
            ))
            .await;
        }
    }
}

async fn execute_gate_task(task: &TaskNode, working_dir: &Path) -> TaskResult {
    let gate_report = if task.checks.is_empty() {
        GateReport::empty()
    } else {
        gate::run_gates(&task.checks, working_dir).await
    };

    TaskResult {
        task_id: task.id,
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
    task: &TaskNode,
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
        max_steps: Some(6),
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

struct TaskDagrsAction<M: CompletionModel + 'static> {
    task: TaskNode,
    model: M,
    registry: Arc<ToolRegistry>,
    config: ExecutorConfig,
    run_state: RunStateStore,
}

#[derive(Clone)]
struct DagrsDependencySignal {
    success: bool,
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
            let _ = out_channels
                .broadcast(dagrs::Content::new(DagrsDependencySignal { success: false }))
                .await;
            return Output::empty();
        }

        if let Some(observer) = &self.config.observer {
            observer.on_task_started(&self.task);
        }

        let result = execute_task(&self.task, &self.model, &self.registry, &self.config).await;

        if let Some(observer) = &self.config.observer {
            observer.on_task_completed(&self.task, &result);
        }

        self.run_state.record_result(result.clone()).await;

        match result.status {
            TaskNodeStatus::Completed => {
                let _ = out_channels
                    .broadcast(dagrs::Content::new(DagrsDependencySignal { success: true }))
                    .await;
                Output::empty()
            }
            TaskNodeStatus::Failed => {
                let _ = out_channels
                    .broadcast(dagrs::Content::new(DagrsDependencySignal { success: false }))
                    .await;
                Output::error(
                    result
                        .agent_output
                        .clone()
                        .unwrap_or_else(|| format!("task {} failed", self.task.title)),
                )
            }
            TaskNodeStatus::Skipped => {
                let _ = out_channels
                    .broadcast(dagrs::Content::new(DagrsDependencySignal { success: false }))
                    .await;
                Output::empty()
            }
            TaskNodeStatus::Pending | TaskNodeStatus::Running => {
                let _ = out_channels
                    .broadcast(dagrs::Content::new(DagrsDependencySignal { success: false }))
                    .await;
                Output::error(format!("task {} returned invalid terminal state", self.task.title))
            }
        }
    }
}

fn execution_batches(dag: &TaskDAG) -> Result<Vec<Vec<Uuid>>, OrchestratorError> {
    let mut completed = HashSet::new();
    let mut scheduled = HashSet::new();
    let mut batches = Vec::new();
    let max_parallel = dag.max_parallel.max(1) as usize;

    while scheduled.len() < dag.nodes.len() {
        let ready: Vec<Uuid> = dag
            .nodes
            .iter()
            .filter(|node| {
                !scheduled.contains(&node.id)
                    && node.dependencies.iter().all(|dep| completed.contains(dep))
            })
            .map(|node| node.id)
            .collect();

        if ready.is_empty() {
            return Err(OrchestratorError::PlanningFailed(
                "task graph contains unresolved dependencies or a cycle".to_string(),
            ));
        }

        let batch: Vec<Uuid> = ready.into_iter().take(max_parallel).collect();
        completed.extend(batch.iter().copied());
        scheduled.extend(batch.iter().copied());
        batches.push(batch);
    }

    Ok(batches)
}

fn task_index(dag: &TaskDAG) -> HashMap<Uuid, usize> {
    dag.nodes
        .iter()
        .enumerate()
        .map(|(idx, node)| (node.id, idx))
        .collect()
}

fn add_batch_barriers(
    dependencies: &mut HashMap<Uuid, HashSet<Uuid>>,
    batches: &[Vec<Uuid>],
    index: &HashMap<Uuid, usize>,
) {
    for window in batches.windows(2) {
        let current = &window[0];
        let next = &window[1];
        for &to in next {
            let entry = dependencies.entry(to).or_default();
            for &from in current {
                if index.get(&from) != index.get(&to) {
                    entry.insert(from);
                }
            }
        }
    }
}

fn build_dagrs_graph<M: CompletionModel + 'static>(
    dag: &TaskDAG,
    model: &M,
    registry: &Arc<ToolRegistry>,
    config: &ExecutorConfig,
    run_state: RunStateStore,
) -> Result<Graph, OrchestratorError> {
    let mut graph = Graph::new();
    let mut node_table = NodeTable::new();
    let mut dagrs_ids = HashMap::new();
    let index = task_index(dag);

    for node in &dag.nodes {
        let action = TaskDagrsAction {
            task: node.clone(),
            model: model.clone(),
            registry: Arc::clone(registry),
            config: config.clone(),
            run_state: run_state.clone(),
        };
        let dagrs_node =
            DefaultNode::with_action(node.id.to_string(), action, &mut node_table);
        let dagrs_id = dagrs_node.id();
        graph.add_node(dagrs_node);
        dagrs_ids.insert(node.id, dagrs_id);
    }

    let mut dependencies: HashMap<Uuid, HashSet<Uuid>> = dag
        .nodes
        .iter()
        .map(|node| (node.id, node.dependencies.iter().copied().collect()))
        .collect();
    let batches = execution_batches(dag)?;
    add_batch_barriers(&mut dependencies, &batches, &index);

    for (task_id, deps) in dependencies {
        let to_id = dagrs_ids
            .get(&task_id)
            .copied()
            .ok_or_else(|| OrchestratorError::PlanningFailed(format!("missing dagrs node for task {task_id}")))?;
        for dep in deps {
            let from_id = dagrs_ids.get(&dep).copied().ok_or_else(|| {
                OrchestratorError::PlanningFailed(format!("missing dagrs node for dependency {dep}"))
            })?;
            graph.add_edge(from_id, vec![to_id]);
        }
    }

    Ok(graph)
}

/// Execute all tasks in the DAG in topological order.
pub async fn execute_dag<M: CompletionModel + 'static>(
    plan: &mut ExecutionPlan,
    model: &M,
    registry: &Arc<ToolRegistry>,
    config: &ExecutorConfig,
) -> Result<RunStateSnapshot, OrchestratorError> {
    let run_state = RunStateStore::new();
    let plan_snapshot = plan.clone();
    let model_snapshot = model.clone();
    let registry_snapshot = Arc::clone(registry);
    let config_snapshot = config.clone();
    let run_state_snapshot = run_state.clone();
    let mut graph = tokio::task::spawn_blocking(move || {
        build_dagrs_graph(
            &plan_snapshot.dag,
            &model_snapshot,
            &registry_snapshot,
            &config_snapshot,
            run_state_snapshot,
        )
    })
    .await
    .map_err(|err| OrchestratorError::ConfigError(format!("failed to build dagrs graph: {err}")))??;

    if let Err(err) = graph.async_start().await {
        tracing::warn!("dagrs execution terminated with error: {}", err);
        if !run_state.has_results().await {
            return Err(OrchestratorError::AgentError(format!(
                "dagrs execution failed before any task completed: {err}"
            )));
        }
    }

    let snapshot = run_state.snapshot(plan).await;
    for node in &mut plan.dag.nodes {
        node.status = snapshot.status_for(node.id);
    }

    Ok(snapshot)
}

fn build_task_prompt(task: &TaskNode, working_dir: &Path, allowed_tools: &[String]) -> String {
    let mut parts = Vec::new();
    parts.push(format!("## Task\n{}", task.title));
    parts.push(format!("## Objective\n{}", task.objective));

    if let Some(desc) = &task.description {
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

    if !task.acceptance_criteria.is_empty() {
        parts.push(format!(
            "## Acceptance Criteria\n{}",
            task.acceptance_criteria
                .iter()
                .map(|criterion| format!("- {}", criterion))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }

    if !task.scope_in.is_empty() {
        parts.push(format!("## Write Scope\n{}", task.scope_in.join(", ")));
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

    if !task.constraints.is_empty() {
        parts.push(format!(
            "## Constraints\n{}",
            task.constraints
                .iter()
                .map(|constraint| format!("- {}", constraint))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }

    parts.join("\n\n")
}

fn build_reviewer_prompt(
    task: &TaskNode,
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
        format!("## Review Task\n{}", task.title),
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

    if !task.acceptance_criteria.is_empty() {
        parts.push(format!(
            "## Acceptance Criteria\n{}",
            task.acceptance_criteria.join("\n")
        ));
    }

    parts.join("\n\n")
}

fn allowed_tools_for_task(spec: &IntentSpec, task: &TaskNode) -> Vec<String> {
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

    if task.kind == TaskKind::Implementation
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

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        sync::{Arc, Mutex},
    };

    use super::*;
    use crate::internal::ai::{
        completion::{
            CompletionError, CompletionRequest, CompletionResponse,
            message::{AssistantContent, Message, Text, UserContent},
        },
        intentspec::{profiles, types::*},
        orchestrator::types::{ExecutionPlan, TaskContract, TaskKind},
        tools::registry::ToolRegistry,
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

    struct RecordingObserver {
        starts: Arc<Mutex<Vec<String>>>,
    }

    impl OrchestratorObserver for RecordingObserver {
        fn on_task_started(&self, task: &TaskNode) {
            self.starts.lock().unwrap().push(task.title.clone());
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
                objectives: vec!["do thing".into()],
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

    fn implementation_task() -> TaskNode {
        TaskNode {
            id: uuid::Uuid::new_v4(),
            title: "Do thing".into(),
            objective: "Do thing".into(),
            description: Some("Implement change".into()),
            kind: TaskKind::Implementation,
            gate_stage: None,
            owner_role: Some("coder".into()),
            dependencies: vec![],
            constraints: vec!["network:allow".into()],
            acceptance_criteria: vec!["tests pass".into()],
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
            status: TaskNodeStatus::Pending,
        }
    }

    fn plan_for_dag(dag: TaskDAG) -> ExecutionPlan {
        ExecutionPlan {
            intent_spec_id: dag.intent_spec_id.clone(),
            summary: "test plan".into(),
            revision: 1,
            parent_revision: None,
            replan_reason: None,
            dag,
            parallel_groups: vec![],
            checkpoints: vec![],
        }
    }

    #[tokio::test]
    async fn test_execute_gate_task() {
        let task = TaskNode {
            kind: TaskKind::Gate,
            gate_stage: Some(super::super::types::GateStage::Fast),
            checks: vec![Check {
                id: "ok".into(),
                kind: CheckKind::Command,
                command: Some("true".into()),
                timeout_seconds: Some(10),
                expected_exit_code: Some(0),
                required: true,
                artifacts_produced: vec![],
            }],
            ..implementation_task()
        };
        let dir = tempfile::tempdir().unwrap();
        let result = execute_gate_task(&task, dir.path()).await;
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
            observer: None,
        };
        let result = execute_task(&implementation_task(), &model, &registry, &config).await;
        assert_eq!(result.status, TaskNodeStatus::Completed);
        assert_eq!(result.retry_count, 0);
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

    #[tokio::test]
    async fn execute_dag_uses_dagrs_and_preserves_batch_order() {
        let model = ConditionalModel;
        let registry = Arc::new(ToolRegistry::new());
        let dir = tempfile::tempdir().unwrap();
        let starts = Arc::new(Mutex::new(Vec::new()));
        let config = ExecutorConfig {
            tool_loop_config: ToolLoopConfig::default(),
            max_retries: 0,
            backoff_seconds: 0,
            working_dir: dir.path().to_path_buf(),
            spec: spec(),
            reviewer_preamble: None,
            observer: Some(Arc::new(RecordingObserver {
                starts: Arc::clone(&starts),
            })),
        };

        let mut a = implementation_task();
        a.title = "A".into();
        a.objective = "A".into();

        let mut c = implementation_task();
        c.title = "C".into();
        c.objective = "C".into();
        c.dependencies = vec![a.id];

        let mut b = implementation_task();
        b.title = "B".into();
        b.objective = "B".into();

        let mut plan = plan_for_dag(TaskDAG {
            nodes: vec![a.clone(), c.clone(), b.clone()],
            intent_spec_id: "test".into(),
            max_parallel: 1,
        });

        let run_state = execute_dag(&mut plan, &model, &registry, &config)
            .await
            .unwrap();

        let start_order = starts.lock().unwrap().clone();
        assert_eq!(start_order, vec!["A", "C", "B"]);
        assert_eq!(run_state.task_results.len(), 3);
        assert!(run_state
            .ordered_task_results()
            .iter()
            .all(|result| result.status == TaskNodeStatus::Completed));
    }

    #[tokio::test]
    async fn execute_dag_stops_future_batches_after_failure() {
        let model = ConditionalModel;
        let registry = Arc::new(ToolRegistry::new());
        let dir = tempfile::tempdir().unwrap();
        let starts = Arc::new(Mutex::new(Vec::new()));
        let config = ExecutorConfig {
            tool_loop_config: ToolLoopConfig::default(),
            max_retries: 0,
            backoff_seconds: 0,
            working_dir: dir.path().to_path_buf(),
            spec: spec(),
            reviewer_preamble: None,
            observer: Some(Arc::new(RecordingObserver {
                starts: Arc::clone(&starts),
            })),
        };

        let mut failing = implementation_task();
        failing.title = "Fail first".into();
        failing.objective = "Fail first".into();

        let mut later = implementation_task();
        later.title = "Later".into();
        later.objective = "Later".into();

        let mut plan = plan_for_dag(TaskDAG {
            nodes: vec![failing.clone(), later.clone()],
            intent_spec_id: "test".into(),
            max_parallel: 1,
        });

        let run_state = execute_dag(&mut plan, &model, &registry, &config)
            .await
            .unwrap();

        let start_order = starts.lock().unwrap().clone();
        assert_eq!(start_order, vec!["Fail first"]);
        assert_eq!(run_state.task_results.len(), 1);
        assert_eq!(run_state.task_results[0].task_id, failing.id);
        assert_eq!(run_state.task_results[0].status, TaskNodeStatus::Failed);
        assert_eq!(run_state.status_for(later.id), TaskNodeStatus::Pending);
        assert_eq!(plan.dag.get(later.id).unwrap().status, TaskNodeStatus::Pending);
    }
}
