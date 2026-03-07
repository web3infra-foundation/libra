use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::Deserialize;
use serde_json::Value;

use super::{
    gate, policy,
    types::{
        GateReport, ReviewOutcome, TaskDAG, TaskKind, TaskNode, TaskNodeStatus, TaskResult,
        ToolCallRecord,
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
}

struct TaskExecutionObserver {
    spec: Arc<IntentSpec>,
    task: TaskNode,
    working_dir: PathBuf,
    in_flight: HashMap<String, ToolCallRecord>,
    tool_calls: Vec<ToolCallRecord>,
    violations: Vec<super::types::PolicyViolation>,
}

impl TaskExecutionObserver {
    fn new(spec: Arc<IntentSpec>, task: TaskNode, working_dir: PathBuf) -> Self {
        Self {
            spec,
            task,
            working_dir,
            in_flight: HashMap::new(),
            tool_calls: Vec::new(),
            violations: Vec::new(),
        }
    }

    fn finish(self) -> (Vec<ToolCallRecord>, Vec<super::types::PolicyViolation>) {
        (self.tool_calls, self.violations)
    }
}

impl ToolLoopObserver for TaskExecutionObserver {
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

    let prompt = build_task_prompt(task);
    let mut retry_count: u8 = 0;
    let mut accumulated_tool_calls = Vec::new();
    let mut accumulated_policy_violations = Vec::new();
    let mut last_review = None;

    loop {
        let mut observer = TaskExecutionObserver::new(
            Arc::clone(&config.spec),
            task.clone(),
            config.working_dir.clone(),
        );
        let agent_result = run_tool_loop_with_history_and_observer(
            model,
            Vec::new(),
            &prompt,
            registry,
            config.tool_loop_config.clone(),
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

    let review_prompt = build_reviewer_prompt(task, agent_output, tool_calls);
    let review_config = ToolLoopConfig {
        preamble: Some(reviewer_preamble),
        allowed_tools: Some(vec![
            "read_file".to_string(),
            "grep_files".to_string(),
            "list_dir".to_string(),
        ]),
        max_steps: Some(6),
        ..config.tool_loop_config.clone()
    };

    let mut observer = TaskExecutionObserver::new(
        Arc::clone(&config.spec),
        task.clone(),
        config.working_dir.clone(),
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
    Ok(Some(ReviewOutcome {
        approved: review.approved,
        summary: review.summary,
        issues: review.issues,
    }))
}

/// Execute all tasks in the DAG in topological order.
pub async fn execute_dag<M: CompletionModel + 'static>(
    dag: &mut TaskDAG,
    model: &M,
    registry: &Arc<ToolRegistry>,
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

        let batch: Vec<_> = ready.into_iter().take(max_parallel).collect();
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
            let result = execute_task(&tasks[0], model, registry, config).await;
            if let Some(node) = dag.get_mut(result.task_id) {
                node.status = result.status.clone();
            }
            if result.status == TaskNodeStatus::Failed {
                failed = true;
            }
            results.push(result);
        } else {
            let mut handles = Vec::with_capacity(tasks.len());
            for task in tasks {
                let model = model.clone();
                let task_registry = Arc::clone(registry);
                let config = config.clone();
                handles.push(tokio::spawn(async move {
                    execute_task(&task, &model, &task_registry, &config).await
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
    parts.push(format!("## Task\n{}", task.title));
    parts.push(format!("## Objective\n{}", task.objective));

    if let Some(desc) = &task.description {
        parts.push(format!("## Background\n{}", desc));
    }

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
    use std::{collections::BTreeMap, sync::Arc};

    use super::*;
    use crate::internal::ai::{
        completion::{
            CompletionError, CompletionRequest, CompletionResponse,
            message::{AssistantContent, Text},
        },
        intentspec::types::*,
        orchestrator::types::{TaskContract, TaskKind},
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
        };
        let result = execute_task(&implementation_task(), &model, &registry, &config).await;
        assert_eq!(result.status, TaskNodeStatus::Completed);
        assert_eq!(result.retry_count, 0);
    }
}
