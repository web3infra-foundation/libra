use std::collections::{BTreeSet, HashMap};

use uuid::Uuid;

use super::types::{
    ExecutionCheckpoint, ExecutionPlanSpec, GateStage, OrchestratorError, TaskContract, TaskKind,
    TaskSpec,
};
use crate::internal::ai::intentspec::types::{
    ChangeType, ConflictResolution, DecompositionMode, DependencyPolicy, IntentSpec, LibraBinding,
    NetworkPolicy, PlanGenerationConfig, RiskLevel, TouchHints,
};

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

    let implementation_ids: Vec<Uuid> = tasks.iter().map(|task| task.id).collect();
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
            let gate_id = Uuid::new_v4();
            let label = format!("after-{}", stage_label(&stage));
            tasks.push(TaskSpec {
                id: gate_id,
                title: title.to_string(),
                objective: format!("Run {} verification checks", stage_label(&stage)),
                description: Some(format!(
                    "Advance to the {} stage only if all required checks pass.",
                    stage_label(&stage)
                )),
                kind: TaskKind::Gate,
                gate_stage: Some(stage.clone()),
                owner_role: Some("verifier".to_string()),
                dependencies,
                constraints: common_constraints.clone(),
                acceptance_criteria: spec.acceptance.success_criteria.clone(),
                scope_in: spec.intent.in_scope.clone(),
                scope_out: spec.intent.out_of_scope.clone(),
                checks,
                contract: common_contract.clone(),
            });
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
        summary: format!(
            "{} change: {} ({} tasks, parallelism {})",
            change_type_label(&spec.intent.change_type),
            spec.intent.summary,
            tasks.len(),
            max_parallel
        ),
        revision: 1,
        parent_revision: None,
        replan_reason: None,
        tasks: tasks.clone(),
        max_parallel,
        parallel_groups: compute_parallel_groups(&tasks),
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
        )]),
        DecompositionMode::PerObjective => {
            let sequential = should_force_serial(plan_config, max_parallel);
            let mut tasks = Vec::with_capacity(spec.intent.objectives.len());
            let mut previous: Option<Uuid> = None;

            for objective in &spec.intent.objectives {
                let id = Uuid::new_v4();
                let dependencies = if sequential {
                    previous.into_iter().collect()
                } else {
                    Vec::new()
                };
                tasks.push(TaskSpec {
                    id,
                    title: objective.clone(),
                    objective: objective.clone(),
                    description: Some(spec.intent.problem_statement.clone()),
                    kind: TaskKind::Implementation,
                    gate_stage: None,
                    owner_role: Some("coder".to_string()),
                    dependencies,
                    constraints: common_constraints.to_vec(),
                    acceptance_criteria: spec.acceptance.success_criteria.clone(),
                    scope_in: spec.intent.in_scope.clone(),
                    scope_out: spec.intent.out_of_scope.clone(),
                    checks: Vec::new(),
                    contract: common_contract.clone(),
                });
                previous = Some(id);
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
                let id = Uuid::new_v4();
                let dependencies = if sequential {
                    previous.into_iter().collect()
                } else {
                    Vec::new()
                };
                let mut contract = common_contract.clone();
                contract.touch_files = vec![file_hint.clone()];

                tasks.push(TaskSpec {
                    id,
                    title: format!("Modify {file_hint}"),
                    objective: format!("Implement changes touching {file_hint}"),
                    description: Some(format!(
                        "{}\nFocus change analysis on file cluster rooted at {}.",
                        spec.intent.problem_statement, file_hint
                    )),
                    kind: TaskKind::Implementation,
                    gate_stage: None,
                    owner_role: Some("coder".to_string()),
                    dependencies,
                    constraints: common_constraints.to_vec(),
                    acceptance_criteria: spec.acceptance.success_criteria.clone(),
                    scope_in: spec.intent.in_scope.clone(),
                    scope_out: spec.intent.out_of_scope.clone(),
                    checks: Vec::new(),
                    contract,
                });
                previous = Some(id);
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
) -> TaskSpec {
    TaskSpec {
        id: Uuid::new_v4(),
        title,
        objective,
        description,
        kind: TaskKind::Implementation,
        gate_stage: None,
        owner_role: Some("coder".to_string()),
        dependencies: Vec::new(),
        constraints,
        acceptance_criteria: contract.expected_outputs.clone(),
        scope_in: contract.write_scope.clone(),
        scope_out: contract.forbidden_scope.clone(),
        checks: Vec::new(),
        contract,
    }
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
                .map(|n| n.title.clone())
                .collect::<Vec<_>>()
                .join(" + ");
            let merged_description = tasks
                .iter()
                .filter_map(|n| n.description.clone())
                .collect::<Vec<_>>()
                .join("\n");

            let mut contract = TaskContract::default();
            let mut constraints = BTreeSet::new();
            let mut acceptance = BTreeSet::new();
            let mut scope_in = BTreeSet::new();
            let mut scope_out = BTreeSet::new();

            for node in tasks {
                constraints.extend(node.constraints);
                acceptance.extend(node.acceptance_criteria);
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

            Ok(vec![TaskSpec {
                id: Uuid::new_v4(),
                title: merged_title,
                objective: merged_objective,
                description: Some(merged_description),
                kind: TaskKind::Implementation,
                gate_stage: None,
                owner_role: Some("coder".into()),
                dependencies: Vec::new(),
                constraints: constraints.into_iter().collect(),
                acceptance_criteria: acceptance.into_iter().collect(),
                scope_in: scope_in.into_iter().collect(),
                scope_out: scope_out.into_iter().collect(),
                checks: Vec::new(),
                contract,
            }])
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
            && !node.dependencies.contains(&prev)
        {
            node.dependencies.push(prev);
        }
        previous = Some(node.id);
    }
}

fn should_force_serial(plan_config: &PlanGenerationConfig, max_parallel: u8) -> bool {
    max_parallel <= 1 || plan_config.conflict_resolution == ConflictResolution::ForceSerial
}

fn compute_parallel_groups(tasks: &[TaskSpec]) -> Vec<Vec<Uuid>> {
    let mut remaining = tasks.to_vec();
    let mut completed = BTreeSet::new();
    let mut groups = Vec::new();

    while !remaining.is_empty() {
        let ready: Vec<Uuid> = remaining
            .iter()
            .filter(|node| node.dependencies.iter().all(|dep| completed.contains(dep)))
            .map(|node| node.id)
            .collect();
        if ready.is_empty() {
            break;
        }
        for id in &ready {
            completed.insert(*id);
        }
        remaining.retain(|node| !ready.contains(&node.id));
        groups.push(ready);
    }

    groups
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

fn change_type_label(change_type: &ChangeType) -> &'static str {
    match change_type {
        ChangeType::Bugfix => "bugfix",
        ChangeType::Feature => "feature",
        ChangeType::Refactor => "refactor",
        ChangeType::Performance => "performance",
        ChangeType::Security => "security",
        ChangeType::Docs => "docs",
        ChangeType::Chore => "chore",
        ChangeType::Unknown => "unknown",
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
        assert_eq!(plan.parallel_groups.len(), 6);
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
        assert_eq!(plan_spec.parallel_groups.len(), 6);
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
