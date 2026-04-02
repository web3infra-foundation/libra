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
        NetworkPolicy, ObjectiveKind, PlanGenerationConfig, RiskLevel, TouchHints,
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
    let plan_config = effective_plan_generation(spec);
    let max_parallel = effective_max_parallel(spec);
    let common_constraints = build_common_constraints(spec);
    let common_contract = build_common_contract(spec);
    let objectives = classify_objectives(spec);

    let implementation_tasks = build_work_tasks(
        spec,
        &objectives,
        &plan_config,
        max_parallel,
        &common_constraints,
        &common_contract,
    )?;
    let mut tasks = apply_conflict_resolution(implementation_tasks, &plan_config)?;

    if should_force_serial(
        &plan_config,
        max_parallel,
        contains_implementation_tasks(&tasks),
    ) {
        make_sequential(&mut tasks);
    }
    let has_implementation_work = contains_implementation_tasks(&tasks);
    let work_task_ids: Vec<Uuid> = tasks
        .iter()
        .filter(|task| task.kind != TaskKind::Gate)
        .map(TaskSpec::id)
        .collect();
    let mut checkpoints = Vec::new();

    if plan_config.gate_task_per_stage {
        let mut upstream_gate_ids = work_task_ids.clone();

        if has_implementation_work && !spec.acceptance.verification_plan.fast_checks.is_empty() {
            let fast_gate_checks = spec.acceptance.verification_plan.fast_checks.clone();
            let mut fast_gate_ids = Vec::new();
            let work_tasks = tasks
                .iter()
                .filter(|task| task.kind == TaskKind::Implementation)
                .cloned()
                .collect::<Vec<_>>();

            for task in &work_tasks {
                let task_title = task.title().to_string();
                let fast_gate = task_spec(
                    git_task(
                        format!("Fast gate: {task_title}"),
                        Some(format!(
                            "Run fast verification checks for {} before downstream stages continue.",
                            task.title()
                        )),
                        Some(GoalType::Test),
                        common_constraints.clone(),
                        spec.acceptance.success_criteria.clone(),
                        vec![task.id()],
                    )?,
                    TaskSpecMeta {
                        objective: format!("Run fast verification checks for {}", task.objective),
                        kind: TaskKind::Gate,
                        gate_stage: Some(GateStage::Fast),
                        owner_role: Some("verifier".to_string()),
                        scope_in: task.scope_in.clone(),
                        scope_out: task.scope_out.clone(),
                        checks: fast_gate_checks.clone(),
                        contract: task.contract.clone(),
                    },
                );
                let gate_id = fast_gate.id();
                fast_gate_ids.push(gate_id);
                checkpoints.push(ExecutionCheckpoint {
                    label: format!("after-fast-{}", gate_id.as_simple()),
                    after_tasks: vec![gate_id],
                    reason: format!("fast gate boundary for {}", task.title()),
                });
                add_fast_gate_to_direct_dependents(&mut tasks, task.id(), gate_id);
                tasks.push(fast_gate);
            }

            upstream_gate_ids = fast_gate_ids;
        }

        let gate_chain = vec![
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
            if !has_implementation_work && checks.is_empty() {
                continue;
            }
            let dependencies = previous_gate
                .map(|id| vec![id])
                .unwrap_or_else(|| upstream_gate_ids.clone());
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

fn effective_plan_generation(spec: &IntentSpec) -> PlanGenerationConfig {
    let mut plan_generation = spec
        .libra
        .as_ref()
        .and_then(|binding| binding.plan_generation.clone())
        .unwrap_or_default();

    let plan_generation_is_explicit = spec
        .libra
        .as_ref()
        .and_then(|binding| binding.plan_generation.as_ref())
        .is_some();
    if !plan_generation_is_explicit && should_auto_use_per_file_cluster(spec) {
        plan_generation.decomposition_mode = DecompositionMode::PerFileCluster;
    }

    plan_generation
}

fn should_auto_use_per_file_cluster(spec: &IntentSpec) -> bool {
    spec.intent
        .touch_hints
        .as_ref()
        .is_some_and(|hints| !hints.files.is_empty())
        && spec
            .intent
            .objectives
            .iter()
            .all(|objective| objective.kind == ObjectiveKind::Implementation)
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

fn build_work_tasks(
    spec: &IntentSpec,
    objectives: &[(String, ObjectiveKind)],
    plan_config: &PlanGenerationConfig,
    max_parallel: u8,
    common_constraints: &[String],
    common_contract: &TaskContract,
) -> Result<Vec<TaskSpec>, OrchestratorError> {
    if objectives.is_empty() {
        return Err(OrchestratorError::PlanningFailed(
            "intent.objectives must contain at least one planned task".to_string(),
        ));
    }

    match plan_config.decomposition_mode {
        DecompositionMode::SingleTask => {
            let merged_kind = merged_objective_kind(objectives);
            let title = match merged_kind {
                TaskKind::Implementation => "Implement requested change".to_string(),
                TaskKind::Analysis => "Analyze requested scope".to_string(),
                TaskKind::Gate => unreachable!("work items cannot be gate tasks"),
            };
            Ok(vec![work_task(
                title,
                objectives
                    .iter()
                    .map(|(objective, _)| objective.clone())
                    .collect::<Vec<_>>()
                    .join("\n"),
                Some(spec.intent.problem_statement.clone()),
                goal_for_work_item(&merged_kind, &spec.intent.change_type),
                merged_kind,
                common_constraints.to_vec(),
                common_contract.clone(),
            )?])
        }
        DecompositionMode::PerObjective => {
            let sequential = should_force_serial(
                plan_config,
                max_parallel,
                objectives
                    .iter()
                    .any(|(_, kind)| *kind == ObjectiveKind::Implementation),
            );
            let mut tasks = Vec::with_capacity(objectives.len());
            let mut previous: Option<Uuid> = None;

            for (objective, objective_kind) in objectives {
                let dependencies = if sequential {
                    previous.into_iter().collect()
                } else {
                    Vec::new()
                };
                let work_kind = task_kind_for_objective(*objective_kind);
                let task = task_spec(
                    git_task(
                        objective.clone(),
                        Some(spec.intent.problem_statement.clone()),
                        goal_for_work_item(&work_kind, &spec.intent.change_type),
                        common_constraints.to_vec(),
                        spec.acceptance.success_criteria.clone(),
                        dependencies,
                    )?,
                    TaskSpecMeta {
                        objective: objective.clone(),
                        kind: work_kind.clone(),
                        gate_stage: None,
                        owner_role: Some(owner_role_for_kind(&work_kind).to_string()),
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
            if objectives
                .iter()
                .any(|(_, kind)| *kind != ObjectiveKind::Implementation)
            {
                return Err(OrchestratorError::PlanningFailed(
                    "perFileCluster decomposition only supports implementation objectives"
                        .to_string(),
                ));
            }
            let hints = spec.intent.touch_hints.clone().unwrap_or(TouchHints {
                files: Vec::new(),
                symbols: Vec::new(),
                apis: Vec::new(),
            });

            if hints.files.is_empty() {
                return build_work_tasks(
                    spec,
                    objectives,
                    &PlanGenerationConfig {
                        decomposition_mode: DecompositionMode::PerObjective,
                        ..plan_config.clone()
                    },
                    max_parallel,
                    common_constraints,
                    common_contract,
                );
            }

            let sequential = should_force_serial(plan_config, max_parallel, true);
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
                        owner_role: Some(
                            owner_role_for_kind(&TaskKind::Implementation).to_string(),
                        ),
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

fn classify_objectives(spec: &IntentSpec) -> Vec<(String, ObjectiveKind)> {
    spec.intent
        .objectives
        .iter()
        .map(|objective| (objective.title.clone(), objective.kind))
        .collect()
}

fn task_kind_for_objective(kind: ObjectiveKind) -> TaskKind {
    match kind {
        ObjectiveKind::Implementation => TaskKind::Implementation,
        ObjectiveKind::Analysis => TaskKind::Analysis,
    }
}

fn merged_objective_kind(items: &[(String, ObjectiveKind)]) -> TaskKind {
    if items
        .iter()
        .any(|(_, kind)| *kind == ObjectiveKind::Implementation)
    {
        TaskKind::Implementation
    } else {
        TaskKind::Analysis
    }
}

fn goal_for_work_item(kind: &TaskKind, change_type: &ChangeType) -> Option<GoalType> {
    Some(match kind {
        TaskKind::Implementation => goal_type(change_type),
        TaskKind::Analysis => GoalType::Other("analysis".to_string()),
        TaskKind::Gate => GoalType::Test,
    })
}

fn work_task(
    title: String,
    objective: String,
    description: Option<String>,
    goal: Option<GoalType>,
    kind: TaskKind,
    constraints: Vec<String>,
    contract: TaskContract,
) -> Result<TaskSpec, OrchestratorError> {
    Ok(task_spec(
        git_task(
            title,
            description,
            goal,
            constraints,
            contract.expected_outputs.clone(),
            Vec::new(),
        )?,
        TaskSpecMeta {
            objective,
            kind: kind.clone(),
            gate_stage: None,
            owner_role: Some(owner_role_for_kind(&kind).to_string()),
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
            let merged_kind = tasks
                .first()
                .map(|task| task.kind.clone())
                .unwrap_or(TaskKind::Implementation);
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
                    Some(match &merged_kind {
                        TaskKind::Implementation => GoalType::Other("implementation".to_string()),
                        TaskKind::Analysis => GoalType::Other("analysis".to_string()),
                        TaskKind::Gate => GoalType::Test,
                    }),
                    constraints.into_iter().collect(),
                    acceptance.into_iter().collect(),
                    Vec::new(),
                )?,
                TaskSpecMeta {
                    objective: merged_objective,
                    kind: merged_kind.clone(),
                    gate_stage: None,
                    owner_role: Some(owner_role_for_kind(&merged_kind).into()),
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
    let mut overlaps = BTreeSet::new();
    let implementation_nodes = nodes
        .iter()
        .filter(|node| node.kind == TaskKind::Implementation)
        .collect::<Vec<_>>();

    for node in &implementation_nodes {
        for path in &node.contract.touch_files {
            let count = seen.entry(path.as_str()).or_default();
            *count += 1;
            if *count == 2 {
                overlaps.insert(path.clone());
            }
        }
    }

    for (idx, left) in implementation_nodes.iter().enumerate() {
        for right in implementation_nodes.iter().skip(idx + 1) {
            if !left.contract.touch_files.is_empty() && !right.contract.touch_files.is_empty() {
                continue;
            }

            for left_scope in &left.scope_in {
                for right_scope in &right.scope_in {
                    if scope_patterns_overlap(left_scope, right_scope) {
                        overlaps.insert(format!("scope:{}<->{}", left_scope, right_scope));
                    }
                }
            }
        }
    }

    overlaps.into_iter().collect()
}

fn scope_patterns_overlap(left: &str, right: &str) -> bool {
    if matches!(left.trim(), "*" | "**") || matches!(right.trim(), "*" | "**") {
        return true;
    }

    let left_root = scope_pattern_root(left);
    let right_root = scope_pattern_root(right);

    if left_root.is_empty() || right_root.is_empty() {
        return true;
    }

    left_root == right_root
        || left_root
            .strip_prefix(&right_root)
            .is_some_and(|rest| rest.starts_with('/'))
        || right_root
            .strip_prefix(&left_root)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn scope_pattern_root(pattern: &str) -> String {
    let normalized = pattern.trim().replace('\\', "/");
    let trimmed = normalized.trim_start_matches("./");
    let prefix = trimmed
        .find('*')
        .map(|index| &trimmed[..index])
        .unwrap_or(trimmed)
        .trim_end_matches('/');
    prefix.to_string()
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

fn add_fast_gate_to_direct_dependents(tasks: &mut [TaskSpec], task_id: Uuid, gate_id: Uuid) {
    for task in tasks
        .iter_mut()
        .filter(|task| task.kind != TaskKind::Gate)
        .filter(|task| task.id() != task_id)
        .filter(|task| task.dependencies().contains(&task_id))
    {
        if !task.dependencies().contains(&gate_id) {
            task.task.add_dependency(gate_id);
        }
    }
}

fn contains_implementation_tasks(tasks: &[TaskSpec]) -> bool {
    tasks
        .iter()
        .any(|task| task.kind == TaskKind::Implementation)
}

fn should_force_serial(
    _plan_config: &PlanGenerationConfig,
    max_parallel: u8,
    _has_implementation_tasks: bool,
) -> bool {
    max_parallel <= 1
}

fn owner_role_for_kind(kind: &TaskKind) -> &'static str {
    match kind {
        TaskKind::Implementation => "coder",
        TaskKind::Analysis => "analyst",
        TaskKind::Gate => "verifier",
    }
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
        ChangeType::Test => GoalType::Test,
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
                objectives: vec![
                    Objective {
                        title: "Add login flow".into(),
                        kind: ObjectiveKind::Implementation,
                    },
                    Objective {
                        title: "Add logout flow".into(),
                        kind: ObjectiveKind::Implementation,
                    },
                ],
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
        assert_eq!(plan.tasks.len(), 7);
        let groups = plan.parallel_groups();
        assert_eq!(groups.len(), 7);
        assert!(groups.iter().all(|group| group.len() == 1), "{groups:?}");
        assert!(
            plan.tasks
                .iter()
                .any(|task| task.gate_stage == Some(GateStage::Fast))
        );
    }

    #[test]
    fn test_compile_execution_plan_spec_tracks_dependencies_without_runtime_plan() {
        let plan_spec = compile_execution_plan_spec(&minimal_spec()).unwrap();
        assert_eq!(plan_spec.tasks.len(), 7);
        assert_eq!(plan_spec.max_parallel, 2);
        assert!(
            plan_spec
                .tasks
                .iter()
                .any(|task| task.gate_stage == Some(GateStage::Fast))
        );
        let groups = plan_spec.parallel_groups();
        assert_eq!(groups.len(), 7);
        assert!(groups.iter().all(|group| group.len() == 1), "{groups:?}");
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
    fn test_compile_execution_plan_auto_uses_file_clusters_for_default_specs() {
        let mut spec = minimal_spec();
        spec.libra = None;

        let plan = compile_execution_plan_spec(&spec).unwrap();
        let implementation_tasks = plan
            .tasks
            .iter()
            .filter(|task| task.kind == TaskKind::Implementation)
            .collect::<Vec<_>>();

        assert_eq!(implementation_tasks.len(), 2);
        assert!(
            implementation_tasks
                .iter()
                .all(|task| task.contract.touch_files.len() == 1)
        );
        assert!(
            implementation_tasks
                .iter()
                .any(|task| task.contract.touch_files == vec!["src/auth/login.rs".to_string()])
        );
        assert!(
            implementation_tasks
                .iter()
                .any(|task| task.contract.touch_files == vec!["src/auth/logout.rs".to_string()])
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

    #[test]
    fn test_compile_execution_plan_analysis_only_uses_analysis_tasks() {
        let mut spec = minimal_spec();
        spec.intent.change_type = ChangeType::Unknown;
        spec.intent.objectives = vec![
            Objective {
                title: "Analyze repository structure".into(),
                kind: ObjectiveKind::Analysis,
            },
            Objective {
                title: "Inventory technical debt hotspots".into(),
                kind: ObjectiveKind::Analysis,
            },
        ];
        spec.acceptance.verification_plan.fast_checks.clear();
        spec.libra
            .as_mut()
            .unwrap()
            .plan_generation
            .as_mut()
            .unwrap()
            .conflict_resolution = ConflictResolution::MergeTasks;

        let plan = compile_execution_plan_spec(&spec).unwrap();
        assert!(
            plan.tasks
                .iter()
                .any(|task| task.kind == TaskKind::Analysis),
            "{:?}",
            plan.tasks
                .iter()
                .map(|task| format!("{:?}", task.kind))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_compile_execution_plan_serializes_overlapping_scopes_without_touch_hints() {
        let mut spec = minimal_spec();
        spec.intent.touch_hints = None;

        let plan = compile_execution_plan_spec(&spec).unwrap();
        let implementation_tasks = plan
            .tasks
            .iter()
            .filter(|task| task.kind == TaskKind::Implementation)
            .collect::<Vec<_>>();

        assert_eq!(implementation_tasks.len(), 2);
        assert!(
            implementation_tasks[1]
                .dependencies()
                .contains(&implementation_tasks[0].id()),
            "{:?}",
            implementation_tasks
                .iter()
                .map(|task| (task.title().to_string(), task.dependencies().to_vec()))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_fast_gate_blocks_downstream_tasks_after_overlap_serialization() {
        let mut spec = minimal_spec();
        spec.intent.touch_hints = None;
        spec.execution.concurrency.max_parallel_tasks = 2;

        let plan = compile_execution_plan_spec(&spec).unwrap();
        let implementation_tasks = plan
            .tasks
            .iter()
            .filter(|task| task.kind == TaskKind::Implementation)
            .collect::<Vec<_>>();
        assert_eq!(implementation_tasks.len(), 2);

        let first_task = implementation_tasks[0];
        let second_task = implementation_tasks[1];
        let first_fast_gate = plan
            .tasks
            .iter()
            .find(|task| {
                task.gate_stage == Some(GateStage::Fast)
                    && task.dependencies() == [first_task.id()]
            })
            .expect("fast gate for first implementation task");

        assert!(second_task.dependencies().contains(&first_task.id()));
        assert!(
            second_task.dependencies().contains(&first_fast_gate.id()),
            "{:?}",
            second_task.dependencies()
        );
    }

    #[test]
    fn test_analysis_only_plan_omits_empty_global_gates_and_keeps_parallel_lanes() {
        let mut spec = minimal_spec();
        spec.intent.change_type = ChangeType::Unknown;
        spec.execution.concurrency.max_parallel_tasks = 4;
        spec.intent.touch_hints = Some(TouchHints {
            files: vec!["src/shared.rs".into()],
            symbols: vec![],
            apis: vec![],
        });
        spec.intent.objectives = vec![
            Objective {
                title: "Analyze repository structure".into(),
                kind: ObjectiveKind::Analysis,
            },
            Objective {
                title: "Inventory technical debt hotspots".into(),
                kind: ObjectiveKind::Analysis,
            },
        ];
        spec.acceptance.verification_plan.fast_checks.clear();
        spec.acceptance.verification_plan.integration_checks.clear();
        spec.acceptance.verification_plan.security_checks.clear();
        spec.acceptance.verification_plan.release_checks.clear();

        let plan = compile_execution_plan_spec(&spec).unwrap();
        assert!(
            plan.tasks
                .iter()
                .all(|task| task.kind == TaskKind::Analysis)
        );

        let groups = plan.parallel_groups();
        assert_eq!(groups.len(), 1, "{groups:?}");
        assert_eq!(groups[0].len(), 2, "{groups:?}");
    }

    #[test]
    fn test_analysis_only_plan_ignores_fast_checks_and_emits_no_gates() {
        let mut spec = minimal_spec();
        spec.intent.change_type = ChangeType::Unknown;
        spec.intent.objectives = vec![Objective {
            title: "Analyze repository structure".into(),
            kind: ObjectiveKind::Analysis,
        }];

        let plan = compile_execution_plan_spec(&spec).unwrap();
        assert!(
            plan.tasks
                .iter()
                .all(|task| task.kind == TaskKind::Analysis)
        );
        assert!(plan.tasks.iter().all(|task| task.gate_stage.is_none()));
    }
}
