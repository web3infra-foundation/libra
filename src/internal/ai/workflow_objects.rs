use anyhow::{Context, Result};
use git_internal::internal::object::{
    intent::{Intent as GitIntent, IntentSpec as GitIntentSpec},
    plan::{Plan as GitPlan, PlanStep},
    task::{GoalType, Task as GitTask},
    types::ActorRef,
};
use serde_json::json;
use uuid::Uuid;

use crate::internal::ai::{
    intentspec::{IntentSpec, canonical::to_canonical_json},
    orchestrator::types::{ExecutionPlanSpec, TaskKind, TaskSpec},
};

const LIBRA_PLAN_ACTOR: &str = "libra-plan";
const LIBRA_EXECUTOR_ACTOR: &str = "libra-executor";

pub fn planner_actor() -> Result<ActorRef> {
    ActorRef::system(LIBRA_PLAN_ACTOR).map_err(anyhow::Error::msg)
}

pub fn executor_actor() -> Result<ActorRef> {
    ActorRef::agent(LIBRA_EXECUTOR_ACTOR).map_err(anyhow::Error::msg)
}

pub fn build_git_intent(spec: &IntentSpec) -> Result<GitIntent> {
    let actor = planner_actor()?;
    let canonical =
        to_canonical_json(spec).context("Failed to serialize IntentSpec to canonical JSON")?;
    let mut intent = GitIntent::new(actor, spec.intent.summary.clone())
        .map_err(anyhow::Error::msg)
        .context("Failed to construct git-internal Intent")?;
    let parsed_spec: serde_json::Value =
        serde_json::from_str(&canonical).context("Failed to parse canonical IntentSpec JSON")?;
    intent.set_spec(Some(GitIntentSpec(parsed_spec)));
    Ok(intent)
}

pub fn build_git_plan(intent_id: Uuid, plan_spec: &ExecutionPlanSpec) -> Result<GitPlan> {
    let actor = planner_actor()?;
    let mut plan = GitPlan::new(actor, intent_id)
        .map_err(anyhow::Error::msg)
        .context("Failed to construct git-internal Plan")?;

    for task in &plan_spec.tasks {
        plan.add_step(task_to_plan_step(task));
    }

    Ok(plan)
}

pub fn build_git_task(intent_id: Option<Uuid>, task: &TaskSpec) -> Result<GitTask> {
    let actor = executor_actor()?;
    let goal = Some(match task.kind {
        TaskKind::Gate => GoalType::Test,
        TaskKind::Implementation => GoalType::Other("implementation".to_string()),
    });

    let mut git_task = GitTask::new(actor, task.title.clone(), goal)
        .map_err(anyhow::Error::msg)
        .context("Failed to construct git-internal Task")?;
    git_task.set_description(task.description.clone().or(Some(task.objective.clone())));
    for constraint in &task.constraints {
        git_task.add_constraint(constraint.clone());
    }
    for criterion in &task.acceptance_criteria {
        git_task.add_acceptance_criterion(criterion.clone());
    }
    git_task.set_intent(intent_id);
    for dependency in &task.dependencies {
        git_task.add_dependency(*dependency);
    }

    Ok(git_task)
}

pub fn parse_object_id(value: &str) -> Result<Uuid> {
    let trimmed = value.strip_prefix("uuid:").unwrap_or(value);
    Uuid::parse_str(trimmed).with_context(|| format!("Invalid object UUID: {value}"))
}

fn task_to_plan_step(task: &TaskSpec) -> PlanStep {
    let mut step = PlanStep::new(task.title.clone());
    step.set_inputs(Some(json!({
        "taskId": task.id,
        "objective": task.objective,
        "kind": format!("{:?}", task.kind),
        "gateStage": task.gate_stage.as_ref().map(|stage| format!("{:?}", stage)),
        "scopeIn": task.scope_in,
        "scopeOut": task.scope_out,
        "touchFiles": task.contract.touch_files,
        "touchSymbols": task.contract.touch_symbols,
        "touchApis": task.contract.touch_apis,
        "constraints": task.constraints,
        "expectedOutputs": task.contract.expected_outputs,
        "acceptanceCriteria": task.acceptance_criteria,
        "ownerRole": task.owner_role,
    })));
    let checks = if task.checks.is_empty() {
        None
    } else {
        Some(serde_json::to_value(&task.checks).unwrap_or_else(|_| json!([])))
    };
    step.set_checks(checks);
    step
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::{
        intentspec::types::Check,
        orchestrator::types::{ExecutionCheckpoint, GateStage, TaskContract},
    };

    fn task() -> TaskSpec {
        TaskSpec {
            id: Uuid::new_v4(),
            title: "Implement auth".into(),
            objective: "Update auth flow".into(),
            description: Some("Adjust login".into()),
            kind: TaskKind::Implementation,
            gate_stage: Some(GateStage::Fast),
            owner_role: Some("coder".into()),
            dependencies: vec![Uuid::new_v4()],
            constraints: vec!["network:deny".into()],
            acceptance_criteria: vec!["tests pass".into()],
            scope_in: vec!["src".into()],
            scope_out: vec!["vendor".into()],
            checks: vec![Check {
                id: "unit".into(),
                kind: crate::internal::ai::intentspec::types::CheckKind::TestSuite,
                command: Some("cargo test".into()),
                timeout_seconds: Some(30),
                expected_exit_code: Some(0),
                required: true,
                artifacts_produced: vec!["test-report".into()],
            }],
            contract: TaskContract {
                write_scope: vec!["src".into()],
                forbidden_scope: vec!["vendor".into()],
                touch_files: vec!["src/auth.rs".into()],
                touch_symbols: vec!["login".into()],
                touch_apis: vec!["AuthService".into()],
                expected_outputs: vec!["updated flow".into()],
            },
        }
    }

    #[test]
    fn builds_git_task() {
        let built = build_git_task(Some(Uuid::new_v4()), &task()).expect("git task");
        assert_eq!(built.title(), "Implement auth");
        assert!(built.description().is_some());
        assert_eq!(built.constraints(), &["network:deny".to_string()]);
        assert_eq!(built.acceptance_criteria(), &["tests pass".to_string()]);
        assert_eq!(built.dependencies().len(), 1);
    }

    #[test]
    fn builds_git_plan_steps() {
        let t = task();
        let plan = build_git_plan(
            Uuid::new_v4(),
            &ExecutionPlanSpec {
                intent_spec_id: "intent-1".into(),
                summary: "summary".into(),
                revision: 1,
                parent_revision: None,
                replan_reason: None,
                tasks: vec![t.clone()],
                max_parallel: 1,
                parallel_groups: vec![vec![t.id]],
                checkpoints: vec![ExecutionCheckpoint {
                    label: "after-fast".into(),
                    after_tasks: vec![t.id],
                    reason: "checkpoint".into(),
                }],
            },
        )
        .expect("git plan");

        assert_eq!(plan.steps().len(), 1);
        assert_eq!(plan.steps()[0].description(), "Implement auth");
        assert!(plan.steps()[0].inputs().is_some());
        assert!(plan.steps()[0].checks().is_some());
    }
}
