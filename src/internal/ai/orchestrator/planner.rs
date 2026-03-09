use std::collections::{BTreeSet, HashMap};

use git_internal::internal::object::{
    plan::PlanStep,
    task::{GoalType, Task as GitTask},
};
use uuid::Uuid;

use super::types::{
    ExecutionCheckpoint, ExecutionPlanSpec, GateStage, OrchestratorError, TaskContract, TaskKind,
    TaskSpec,
};
use crate::internal::ai::{
    intentspec::types::{
        ChangeType, ConflictResolution, DecompositionMode, DependencyPolicy, IntentSpec,
        LibraBinding, NetworkPolicy, PlanGenerationConfig, RiskLevel, TouchHints,
    },
    workflow_objects::planner_actor,
};

struct TaskSpecMeta {
    objective: String,
    kind: TaskKind,
    gate_stage: Option<GateStage>,
    owner_role: Option<String>,
    scope_in: Vec<String>,
    scope_out: Vec<String>,
    checks: Vec<crate::internal::ai::intentspec::types::Check>,
    contract: TaskContract,
}

/// Compile an IntentSpec into a static execution plan specification.
pub fn compile_execution_plan_spec(
    spec: &IntentSpec,
) -> Result<ExecutionPlanSpec, OrchestratorError> {
    let plan_config = effective_plan_generation(spec.libra.as_ref());
    let max_parallel = effective_max_parallel(spec);
    let common_constraints = build_common_constraints(spec);
    let common_contract = build_common_contract(spec);

    let implementation_tasks = build_implementation_tasks(
        spec,
        &plan_config,
        max_parallel,
        &common_constraints,
        &common_contract,
    )?;
    let mut tasks = apply_conflict_resolution(implementation_tasks, &plan_config)?;

    if should_force_serial(&plan_config, max_parallel) {
        make_sequential(&mut tasks);
    }

    let implementation_ids: Vec<Uuid> = tasks.iter().map(TaskSpec::id).collect();
    let mut checkpoints = Vec::new();

    if plan_config.gate_task_per_stage {
        let gate_chain = vec![
            (
                GateStage::Fast,
                "Fast gate",
                spec.acceptance.verification_plan.fast_checks.clone(),
            ),
            (
                GateStage::Integration,
                "Integration gate",
                spec.acceptance.verification_plan.integration_checks.clone(),
            ),
            (
                GateStage::Security,
                "Security gate",
                spec.acceptance.verification_plan.security_checks.clone(),
            ),
            (
                GateStage::Release,
                "Release gate",
                spec.acceptance.verification_plan.release_checks.clone(),
            ),
        ];

        let mut previous_gate: Option<Uuid> = None;
        for (stage, title, checks) in gate_chain {
            let dependencies = previous_gate
                .map(|id| vec![id])
                .unwrap_or_else(|| implementation_ids.clone());
            let label = format!("after-{}", stage_label(&stage));
            let task = task_spec(
                git_task(
                    title.to_string(),
                    Some(format!(
                        "Advance to the {} stage only if all required checks pass.",
                        stage_label(&stage)
                    )),
                    Some(GoalType::Test),
                    common_constraints.clone(),
                    spec.acceptance.success_criteria.clone(),
                    dependencies,
                )?,
                TaskSpecMeta {
                    objective: format!("Run {} verification checks", stage_label(&stage)),
                    kind: TaskKind::Gate,
                    gate_stage: Some(stage.clone()),
                    owner_role: Some("verifier".to_string()),
                    scope_in: spec.intent.in_scope.clone(),
                    scope_out: spec.intent.out_of_scope.clone(),
                    checks,
                    contract: common_contract.clone(),
                },
            );
            let gate_id = task.id();
            tasks.push(task);
            checkpoints.push(ExecutionCheckpoint {
                label,
                after_tasks: vec![gate_id],
                reason: format!("{} gate boundary", stage_label(&stage)),
            });
            previous_gate = Some(gate_id);
        }
    }

    Ok(ExecutionPlanSpec {
        intent_spec_id: spec.metadata.id.clone(),
        revision: 1,
        parent_revision: None,
        replan_reason: None,
        tasks: tasks.clone(),
        max_parallel,
        checkpoints,
    })
}

fn effective_plan_generation(libra: Option<&LibraBinding>) -> PlanGenerationConfig {
    libra
        .and_then(|binding| binding.plan_generation.clone())
        .unwrap_or_default()
}

fn effective_max_parallel(spec: &IntentSpec) -> u8 {
    if spec.risk.level == RiskLevel::High {
        1
    } else {
        spec.execution.concurrency.max_parallel_tasks.max(1)
    }
}

fn build_common_constraints(spec: &IntentSpec) -> Vec<String> {
    let mut constraints = Vec::new();

    constraints.push(match spec.constraints.security.network_policy {
        NetworkPolicy::Deny => "network:deny".to_string(),
        NetworkPolicy::Allow => "network:allow".to_string(),
    });
    constraints.push(format!(
        "dependency-policy:{}",
        dependency_policy_label(&spec.constraints.security.dependency_policy)
    ));

    if !spec.constraints.security.crypto_policy.trim().is_empty() {
        constraints.push(format!(
            "crypto-policy:{}",
            spec.constraints.security.crypto_policy.trim()
        ));
    }

    constraints.push(format!("risk:{:?}", spec.risk.level).to_lowercase());
    constraints.push(format!("evidence-strategy:{:?}", spec.evidence.strategy).to_lowercase());
    constraints.push(format!(
        "citations-min:{}",
        spec.evidence.min_citations_per_decision
    ));
    constraints
}

fn build_common_contract(spec: &IntentSpec) -> TaskContract {
    let hints = spec.intent.touch_hints.clone().unwrap_or(TouchHints {
        files: Vec::new(),
        symbols: Vec::new(),
        apis: Vec::new(),
    });

    TaskContract {
        write_scope: spec.intent.in_scope.clone(),
        forbidden_scope: spec.intent.out_of_scope.clone(),
        touch_files: hints.files,
        touch_symbols: hints.symbols,
        touch_apis: hints.apis,
        expected_outputs: spec.acceptance.success_criteria.clone(),
    }
}

fn build_implementation_tasks(
    spec: &IntentSpec,
    plan_config: &PlanGenerationConfig,
    max_parallel: u8,
    common_constraints: &[String],
    common_contract: &TaskContract,
) -> Result<Vec<TaskSpec>, OrchestratorError> {
    match plan_config.decomposition_mode {
        DecompositionMode::SingleTask => Ok(vec![implementation_task(
            "Implement requested change".to_string(),
            spec.intent.objectives.join("\n"),
            Some(spec.intent.problem_statement.clone()),
            common_constraints.to_vec(),
            common_contract.clone(),
        )?]),
        DecompositionMode::PerObjective => {
            let sequential = should_force_serial(plan_config, max_parallel);
            let mut tasks = Vec::with_capacity(spec.intent.objectives.len());
            let mut previous: Option<Uuid> = None;

            for objective in &spec.intent.objectives {
                let dependencies = if sequential {
                    previous.into_iter().collect()
                } else {
                    Vec::new()
                };
                let task = task_spec(
                    git_task(
                        objective.clone(),
                        Some(spec.intent.problem_statement.clone()),
                        Some(goal_type(&spec.intent.change_type)),
                        common_constraints.to_vec(),
                        spec.acceptance.success_criteria.clone(),
                        dependencies,
                    )?,
                    TaskSpecMeta {
                        objective: objective.clone(),
                        kind: TaskKind::Implementation,
                        gate_stage: None,
                        owner_role: Some("coder".to_string()),
                        scope_in: spec.intent.in_scope.clone(),
                        scope_out: spec.intent.out_of_scope.clone(),
                        checks: Vec::new(),
                        contract: common_contract.clone(),
                    },
                );
                previous = Some(task.id());
                tasks.push(task);
            }
            Ok(tasks)
        }
        DecompositionMode::PerFileCluster => {
            let hints = spec.intent.touch_hints.clone().unwrap_or(TouchHints {
                files: Vec::new(),
                symbols: Vec::new(),
                apis: Vec::new(),
            });

            if hints.files.is_empty() {
                return build_implementation_tasks(
                    spec,
                    &PlanGenerationConfig {
                        decomposition_mode: DecompositionMode::PerObjective,
                        ..plan_config.clone()
                    },
                    max_parallel,
                    common_constraints,
                    common_contract,
                );
            }

            let sequential = should_force_serial(plan_config, max_parallel);
            let mut tasks = Vec::with_capacity(hints.files.len());
            let mut previous: Option<Uuid> = None;

            for file_hint in hints.files {
                let dependencies = if sequential {
                    previous.into_iter().collect()
                } else {
                    Vec::new()
                };
                let mut contract = common_contract.clone();
                contract.touch_files = vec![file_hint.clone()];

                let task = task_spec(
                    git_task(
                        format!("Modify {file_hint}"),
                        Some(format!(
                            "{}\nFocus change analysis on file cluster rooted at {}.",
                            spec.intent.problem_statement, file_hint
                        )),
                        Some(goal_type(&spec.intent.change_type)),
                        common_constraints.to_vec(),
                        spec.acceptance.success_criteria.clone(),
                        dependencies,
                    )?,
                    TaskSpecMeta {
                        objective: format!("Implement changes touching {file_hint}"),
                        kind: TaskKind::Implementation,
                        gate_stage: None,
                        owner_role: Some("coder".to_string()),
                        scope_in: spec.intent.in_scope.clone(),
                        scope_out: spec.intent.out_of_scope.clone(),
                        checks: Vec::new(),
                        contract,
                    },
                );
                previous = Some(task.id());
                tasks.push(task);
            }

            Ok(tasks)
        }
    }
}

fn implementation_task(
    title: String,
    objective: String,
    description: Option<String>,
    constraints: Vec<String>,
    contract: TaskContract,
) -> Result<TaskSpec, OrchestratorError> {
    Ok(task_spec(
        git_task(
            title,
            description,
            Some(GoalType::Other("implementation".to_string())),
            constraints,
            contract.expected_outputs.clone(),
            Vec::new(),
        )?,
        TaskSpecMeta {
            objective,
            kind: TaskKind::Implementation,
            gate_stage: None,
            owner_role: Some("coder".to_string()),
            scope_in: contract.write_scope.clone(),
            scope_out: contract.forbidden_scope.clone(),
            checks: Vec::new(),
            contract,
        },
    ))
}

fn apply_conflict_resolution(
    mut tasks: Vec<TaskSpec>,
    plan_config: &PlanGenerationConfig,
) -> Result<Vec<TaskSpec>, OrchestratorError> {
    let overlaps = find_overlaps(&tasks);
    if overlaps.is_empty() {
        return Ok(tasks);
    }

    match plan_config.conflict_resolution {
        ConflictResolution::ForceSerial => {
            make_sequential(&mut tasks);
            Ok(tasks)
        }
        ConflictResolution::MergeTasks => {
            let merged_objective = tasks
                .iter()
                .map(|n| n.objective.clone())
                .collect::<Vec<_>>()
                .join("\n");
            let merged_title = tasks
                .iter()
                .map(|n| n.title().to_string())
                .collect::<Vec<_>>()
                .join(" + ");
            let merged_description = tasks
                .iter()
                .filter_map(|n| n.description().map(ToString::to_string))
                .collect::<Vec<_>>()
                .join("\n");

            let mut contract = TaskContract::default();
            let mut constraints = BTreeSet::new();
            let mut acceptance = BTreeSet::new();
            let mut scope_in = BTreeSet::new();
            let mut scope_out = BTreeSet::new();

            for node in tasks {
                constraints.extend(node.constraints().iter().cloned());
                acceptance.extend(node.acceptance_criteria().iter().cloned());
                scope_in.extend(node.scope_in);
                scope_out.extend(node.scope_out);
                contract.write_scope.extend(node.contract.write_scope);
                contract
                    .forbidden_scope
                    .extend(node.contract.forbidden_scope);
                contract.touch_files.extend(node.contract.touch_files);
                contract.touch_symbols.extend(node.contract.touch_symbols);
                contract.touch_apis.extend(node.contract.touch_apis);
                contract
                    .expected_outputs
                    .extend(node.contract.expected_outputs);
            }

            contract.write_scope.sort();
            contract.write_scope.dedup();
            contract.forbidden_scope.sort();
            contract.forbidden_scope.dedup();
            contract.touch_files.sort();
            contract.touch_files.dedup();
            contract.touch_symbols.sort();
            contract.touch_symbols.dedup();
            contract.touch_apis.sort();
            contract.touch_apis.dedup();
            contract.expected_outputs.sort();
            contract.expected_outputs.dedup();

            Ok(vec![task_spec(
                git_task(
                    merged_title,
                    Some(merged_description),
                    Some(GoalType::Other("implementation".to_string())),
                    constraints.into_iter().collect(),
                    acceptance.into_iter().collect(),
                    Vec::new(),
                )?,
                TaskSpecMeta {
                    objective: merged_objective,
                    kind: TaskKind::Implementation,
                    gate_stage: None,
                    owner_role: Some("coder".into()),
                    scope_in: scope_in.into_iter().collect(),
                    scope_out: scope_out.into_iter().collect(),
                    checks: Vec::new(),
                    contract,
                },
            )])
        }
        ConflictResolution::FailFast => Err(OrchestratorError::PlanningFailed(format!(
            "task decomposition produced overlapping write clusters: {}",
            overlaps.join(", ")
        ))),
    }
}

fn find_overlaps(nodes: &[TaskSpec]) -> Vec<String> {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    let mut overlaps = Vec::new();

    for node in nodes {
        for path in &node.contract.touch_files {
            let count = seen.entry(path.as_str()).or_default();
            *count += 1;
            if *count == 2 {
                overlaps.push(path.clone());
            }
        }
    }

    overlaps
}

fn make_sequential(nodes: &mut [TaskSpec]) {
    let mut previous: Option<Uuid> = None;
    for node in nodes {
        if let Some(prev) = previous
            && !node.dependencies().contains(&prev)
        {
            node.task.add_dependency(prev);
        }
        previous = Some(node.id());
    }
}

fn should_force_serial(plan_config: &PlanGenerationConfig, max_parallel: u8) -> bool {
    max_parallel <= 1 || plan_config.conflict_resolution == ConflictResolution::ForceSerial
}

fn git_task(
    title: String,
    description: Option<String>,
    goal: Option<GoalType>,
    constraints: Vec<String>,
    acceptance_criteria: Vec<String>,
    dependencies: Vec<Uuid>,
) -> Result<GitTask, OrchestratorError> {
    let actor = planner_actor()
        .map_err(|e| OrchestratorError::PlanningFailed(format!("invalid planner actor: {e}")))?;
    let mut task = GitTask::new(actor, title, goal)
        .map_err(|e| OrchestratorError::PlanningFailed(format!("failed to create task: {e}")))?;
    task.set_description(description);
    for constraint in constraints {
        task.add_constraint(constraint);
    }
    for criterion in acceptance_criteria {
        task.add_acceptance_criterion(criterion);
    }
    for dependency in dependencies {
        task.add_dependency(dependency);
    }
    Ok(task)
}

fn task_spec(task: GitTask, meta: TaskSpecMeta) -> TaskSpec {
    let mut step = PlanStep::new(task.title().to_string());
    step.set_inputs(Some(serde_json::json!({
        "objective": meta.objective,
        "kind": format!("{:?}", meta.kind),
        "gateStage": meta.gate_stage.as_ref().map(|stage| format!("{:?}", stage)),
        "scopeIn": meta.scope_in,
        "scopeOut": meta.scope_out,
        "touchFiles": meta.contract.touch_files,
        "touchSymbols": meta.contract.touch_symbols,
        "touchApis": meta.contract.touch_apis,
        "constraints": task.constraints(),
        "expectedOutputs": meta.contract.expected_outputs,
        "acceptanceCriteria": task.acceptance_criteria(),
        "ownerRole": meta.owner_role,
    })));
    if !meta.checks.is_empty() {
        step.set_checks(Some(
            serde_json::to_value(&meta.checks).unwrap_or_else(|_| serde_json::json!([])),
        ));
    }
    let mut task = task;
    task.set_origin_step_id(Some(step.step_id()));
    TaskSpec {
        step,
        task,
        objective: meta.objective,
        kind: meta.kind,
        gate_stage: meta.gate_stage,
        owner_role: meta.owner_role,
        scope_in: meta.scope_in,
        scope_out: meta.scope_out,
        checks: meta.checks,
        contract: meta.contract,
    }
}

fn goal_type(change_type: &ChangeType) -> GoalType {
    match change_type {
        ChangeType::Bugfix => GoalType::Bugfix,
        ChangeType::Feature => GoalType::Feature,
        ChangeType::Refactor => GoalType::Refactor,
        ChangeType::Performance => GoalType::Perf,
        ChangeType::Security => GoalType::Other("security".to_string()),
        ChangeType::Docs => GoalType::Docs,
        ChangeType::Chore => GoalType::Chore,
        ChangeType::Unknown => GoalType::Other("unknown".to_string()),
    }
}

fn stage_label(stage: &GateStage) -> &'static str {
    match stage {
        GateStage::Fast => "fast",
        GateStage::Integration => "integration",
        GateStage::Security => "security",
        GateStage::Release => "release",
    }
}

fn dependency_policy_label(policy: &DependencyPolicy) -> &'static str {
    match policy {
        DependencyPolicy::NoNew => "no-new",
        DependencyPolicy::AllowWithReview => "allow-with-review",
        DependencyPolicy::Allow => "allow",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::internal::ai::intentspec::types::*;

    fn minimal_spec() -> IntentSpec {
        IntentSpec {
            api_version: "intentspec.io/v1alpha1".into(),
            kind: "IntentSpec".into(),
            metadata: Metadata {
                id: "test-id".into(),
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
                summary: "Implement auth flow".into(),
                problem_statement: "Need login and logout changes".into(),
                change_type: ChangeType::Feature,
                objectives: vec!["Add login flow".into(), "Add logout flow".into()],
                in_scope: vec!["src/".into()],
                out_of_scope: vec!["vendor/".into()],
                touch_hints: Some(TouchHints {
                    files: vec!["src/auth/login.rs".into(), "src/auth/logout.rs".into()],
                    symbols: vec!["login".into()],
                    apis: vec!["/v1/login".into()],
                }),
            },
            acceptance: Acceptance {
                success_criteria: vec!["tests pass".into()],
                verification_plan: VerificationPlan {
                    fast_checks: vec![Check {
                        id: "fmt".into(),
                        kind: CheckKind::Command,
                        command: Some("cargo fmt --check".into()),
                        timeout_seconds: Some(30),
                        expected_exit_code: Some(0),
                        required: true,
                        artifacts_produced: vec![],
                    }],
                    integration_checks: vec![],
                    security_checks: vec![],
                    release_checks: vec![],
                },
                quality_gates: None,
            },
            constraints: Constraints {
                security: ConstraintSecurity {
                    network_policy: NetworkPolicy::Deny,
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
                    max_wall_clock_seconds: 3600,
                    max_cost_units: 100,
                },
            },
            risk: Risk {
                level: RiskLevel::Medium,
                rationale: "normal feature".into(),
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
                    allow: vec![],
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
                    max_retries: 2,
                    backoff_seconds: 0,
                },
                replan: ReplanPolicy { triggers: vec![] },
                concurrency: ConcurrencyPolicy {
                    max_parallel_tasks: 2,
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
            libra: Some(LibraBinding {
                object_store: None,
                context_pipeline: None,
                plan_generation: Some(PlanGenerationConfig {
                    decomposition_mode: DecompositionMode::PerObjective,
                    conflict_resolution: ConflictResolution::ForceSerial,
                    gate_task_per_stage: true,
                }),
                run_policy: None,
                actor_mapping: None,
                decision_policy: None,
            }),
            extensions: BTreeMap::new(),
        }
    }

    #[test]
    fn test_compile_execution_plan_builds_gate_tasks() {
        let plan = compile_execution_plan_spec(&minimal_spec()).unwrap();
        assert_eq!(plan.tasks.len(), 6);
        assert_eq!(plan.parallel_groups().len(), 6);
        assert!(
            plan.tasks
                .iter()
                .any(|task| task.gate_stage == Some(GateStage::Fast))
        );
    }

    #[test]
    fn test_compile_execution_plan_spec_tracks_dependencies_without_runtime_plan() {
        let plan_spec = compile_execution_plan_spec(&minimal_spec()).unwrap();
        assert_eq!(plan_spec.tasks.len(), 6);
        assert_eq!(plan_spec.max_parallel, 2);
        assert!(
            plan_spec
                .tasks
                .iter()
                .any(|task| task.gate_stage == Some(GateStage::Fast))
        );
        assert_eq!(plan_spec.parallel_groups().len(), 6);
    }

    #[test]
    fn test_compile_execution_plan_per_file_cluster() {
        let mut spec = minimal_spec();
        spec.libra
            .as_mut()
            .unwrap()
            .plan_generation
            .as_mut()
            .unwrap()
            .decomposition_mode = DecompositionMode::PerFileCluster;
        let plan = compile_execution_plan_spec(&spec).unwrap();
        assert!(
            plan.tasks
                .iter()
                .any(|task| task.contract.touch_files == vec!["src/auth/login.rs".to_string()])
        );
    }

    #[test]
    fn test_compile_execution_plan_fail_fast_overlap() {
        let mut spec = minimal_spec();
        spec.intent.touch_hints = Some(TouchHints {
            files: vec!["src/shared.rs".into(), "src/shared.rs".into()],
            symbols: vec![],
            apis: vec![],
        });
        spec.libra
            .as_mut()
            .unwrap()
            .plan_generation
            .as_mut()
            .unwrap()
            .decomposition_mode = DecompositionMode::PerFileCluster;
        spec.libra
            .as_mut()
            .unwrap()
            .plan_generation
            .as_mut()
            .unwrap()
            .conflict_resolution = ConflictResolution::FailFast;

        let err = compile_execution_plan_spec(&spec).unwrap_err();
        assert!(matches!(err, OrchestratorError::PlanningFailed(_)));
    }
}
