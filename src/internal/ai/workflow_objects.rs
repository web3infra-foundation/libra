//! Bridge between Libra's high-level orchestrator types and the persisted
//! `git-internal` workflow objects (Intent / Plan / Task).
//!
//! The orchestrator deals with rich, in-memory specs (`IntentSpec`,
//! `ExecutionPlanSpec`, `TaskSpec`) that carry agent-level metadata such as
//! gate stages, scope rules, and check definitions. Those types are not
//! suitable for storage on the AI history branch — the canonical workflow
//! objects live in `git-internal::object` and use a more compact, signed
//! representation. This module is the conversion seam: it produces
//! storable `GitIntent`, `GitPlan`, and `GitTask` instances stamped with the
//! correct system actor and packs orchestrator metadata into each plan step's
//! `inputs`/`checks` payload so that round-trips remain lossless.
//!
//! See the [`history`](crate::internal::ai::history) module for how those
//! objects are appended to the orphan branch once built here.

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

/// Stable system actor name used for planning-side workflow objects.
const LIBRA_PLAN_ACTOR: &str = "libra-plan";
/// Stable agent actor name used when writing executor-side task objects.
const LIBRA_EXECUTOR_ACTOR: &str = "libra-executor";

/// Build the canonical [`ActorRef`] used to attribute planner-emitted
/// workflow objects (Intents and Plans).
///
/// Functional scope:
/// - Wraps `ActorRef::system` so that planner authorship is recorded as the
///   `libra-plan` system identity.
///
/// Boundary conditions:
/// - Returns an error if `git-internal` rejects the actor name (e.g. due to
///   future validation rules); callers should treat this as a programmer
///   error since the constant is fixed.
pub fn planner_actor() -> Result<ActorRef> {
    ActorRef::system(LIBRA_PLAN_ACTOR).map_err(anyhow::Error::msg)
}

/// Build the canonical [`ActorRef`] used to attribute executor-emitted
/// task objects.
///
/// Functional scope:
/// - Uses the `agent` actor flavour so executor-side records are
///   distinguishable from planner-side `system` records when auditing
///   provenance.
///
/// Boundary conditions:
/// - Mirrors [`planner_actor`]: validation failures are treated as fatal
///   programmer errors.
pub fn executor_actor() -> Result<ActorRef> {
    ActorRef::agent(LIBRA_EXECUTOR_ACTOR).map_err(anyhow::Error::msg)
}

/// Convert an in-memory [`IntentSpec`] into a storable [`GitIntent`].
///
/// Functional scope:
/// - Stamps the new intent with the planner actor and the summary from
///   `spec.intent.summary`.
/// - Serialises `spec` to canonical JSON, parses it back into a
///   [`serde_json::Value`], and attaches it as the intent's `IntentSpec`
///   payload so the full structured spec is preserved alongside the
///   short summary.
///
/// Boundary conditions:
/// - Returns an error if canonical serialisation fails, if the JSON cannot
///   be re-parsed (should be impossible for well-formed canonical JSON), or
///   if `git-internal::Intent::new` rejects the inputs.
pub fn build_git_intent(spec: &IntentSpec) -> Result<GitIntent> {
    let actor = planner_actor()?;
    // Canonical JSON guarantees byte-for-byte stability across runs, which is
    // important because the intent's hash is content-derived: any field
    // reordering would otherwise produce a different object id.
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

/// Convert an [`ExecutionPlanSpec`] into a storable [`GitPlan`] linked to
/// `intent_id`.
///
/// Functional scope:
/// - Constructs an empty [`GitPlan`] under the planner actor and appends one
///   [`PlanStep`] per task in declaration order.
///
/// Boundary conditions:
/// - Order of tasks matters: callers should sort/topologise before calling
///   this function. The plan stores steps as a linear sequence — DAG edges are
///   conveyed via the per-task `dependencies()` list inside each
///   [`TaskSpec`], not via plan ordering.
/// - Returns an error if any task cannot be converted (see
///   [`task_to_plan_step`]).
pub fn build_git_plan(intent_id: Uuid, plan_spec: &ExecutionPlanSpec) -> Result<GitPlan> {
    let actor = planner_actor()?;
    let mut plan = GitPlan::new(actor, intent_id)
        .map_err(anyhow::Error::msg)
        .context("Failed to construct git-internal Plan")?;

    for task in &plan_spec.tasks {
        plan.add_step(task_to_plan_step(task)?);
    }

    Ok(plan)
}

/// Convert a [`TaskSpec`] into a storable [`GitTask`].
///
/// Functional scope:
/// - Reuses the spec's own `goal()` if the planner attached one; otherwise
///   maps the high-level [`TaskKind`] onto a sensible [`GoalType`]
///   (`Gate` -> `GoalType::Test`, `Implementation`/`Analysis` -> `Other`).
/// - Falls back to the spec's `objective` when no description is supplied,
///   ensuring the persisted task always carries a human-readable narrative.
/// - Mirrors the spec's constraints, acceptance criteria, and dependency
///   UUIDs onto the git task and links it back to its origin plan step via
///   `origin_step_id`.
///
/// Boundary conditions:
/// - `intent_id` is optional because tasks may exist standalone (e.g. when
///   replanning) — callers should pass `Some(id)` whenever the task belongs
///   to a known intent.
/// - Returns an error from `git-internal` if the title or goal is rejected.
pub fn build_git_task(intent_id: Option<Uuid>, task: &TaskSpec) -> Result<GitTask> {
    let actor = executor_actor()?;
    // Prefer an explicit goal already on the spec; fall back to a kind-derived
    // default so every persisted task carries some goal classification.
    let goal = task.task.goal().cloned().or(Some(match task.kind {
        TaskKind::Gate => GoalType::Test,
        TaskKind::Implementation => GoalType::Other("implementation".to_string()),
        TaskKind::Analysis => GoalType::Other("analysis".to_string()),
    }));

    let mut git_task = GitTask::new(actor, task.title().to_string(), goal)
        .map_err(anyhow::Error::msg)
        .context("Failed to construct git-internal Task")?;
    git_task.set_description(
        task.description()
            .map(ToString::to_string)
            .or(Some(task.objective.clone())),
    );
    for constraint in task.constraints() {
        git_task.add_constraint(constraint.clone());
    }
    for criterion in task.acceptance_criteria() {
        git_task.add_acceptance_criterion(criterion.clone());
    }
    git_task.set_intent(intent_id);
    git_task.set_origin_step_id(Some(task.step_id()));
    for dependency in task.dependencies() {
        git_task.add_dependency(*dependency);
    }

    Ok(git_task)
}

/// Parse a workflow object identifier, tolerating an optional `uuid:` prefix.
///
/// Functional scope:
/// - Accepts either a bare UUID string (`"550e8400-e29b-41d4-a716-446655440000"`)
///   or its prefixed form (`"uuid:550e8400-..."`) so callers can pass scheme
///   tags from links or tool arguments without manual stripping.
///
/// Boundary conditions:
/// - The original `value` (with prefix, if any) is included verbatim in the
///   error message so users can see what they actually typed.
/// - Any other prefix is treated as part of the UUID and causes a parse
///   failure.
pub fn parse_object_id(value: &str) -> Result<Uuid> {
    let trimmed = value.strip_prefix("uuid:").unwrap_or(value);
    Uuid::parse_str(trimmed).with_context(|| format!("Invalid object UUID: {value}"))
}

/// Translate a [`TaskSpec`] into the [`PlanStep`] form stored inside a
/// [`GitPlan`].
///
/// Functional scope:
/// - Clones the spec's pre-built `step` shell and decorates it with two
///   structured payloads:
///   * `inputs`: a JSON snapshot of the orchestrator-level metadata (task
///     id, objective, kind, scope rules, contract, owner role, ...). This
///     keeps the persisted plan step rich enough to reconstruct context
///     without consulting the orchestrator state.
///   * `checks`: serialised verification checks if any are defined; an
///     empty check list is stored as `None` rather than an empty array to
///     keep the on-disk shape minimal.
///
/// Boundary conditions:
/// - Returns an error if `serde_json::to_value` fails on the task's checks
///   (each check carries arbitrary user-supplied strings, but serialisation
///   should not fail in practice).
fn task_to_plan_step(task: &TaskSpec) -> Result<PlanStep> {
    let mut step = task.step.clone();
    // Snapshot orchestrator metadata as JSON inputs so a reader of the plan
    // alone can reconstruct task context without round-tripping through the
    // in-memory orchestrator types.
    step.set_inputs(Some(json!({
        "taskId": task.id(),
        "objective": task.objective,
        "kind": format!("{:?}", task.kind),
        "gateStage": task.gate_stage.as_ref().map(|stage| format!("{:?}", stage)),
        "scopeIn": task.scope_in,
        "scopeOut": task.scope_out,
        "touchFiles": task.contract.touch_files,
        "touchSymbols": task.contract.touch_symbols,
        "touchApis": task.contract.touch_apis,
        "constraints": task.constraints(),
        "expectedOutputs": task.contract.expected_outputs,
        "acceptanceCriteria": task.acceptance_criteria(),
        "ownerRole": task.owner_role,
    })));
    let checks = if task.checks.is_empty() {
        None
    } else {
        Some(serde_json::to_value(&task.checks).with_context(|| {
            format!(
                "Failed to serialize checks for task '{}' ({})",
                task.title(),
                task.id()
            )
        })?)
    };
    step.set_checks(checks);
    Ok(step)
}

#[cfg(test)]
mod tests {
    use git_internal::internal::object::{task::Task as GitTask, types::ActorRef};

    use super::*;
    use crate::internal::ai::{
        intentspec::types::Check,
        orchestrator::types::{ExecutionCheckpoint, GateStage, TaskContract},
    };

    fn task() -> TaskSpec {
        let actor = ActorRef::agent("test-workflow").unwrap();
        let dependency = Uuid::new_v4();
        let mut git_task = GitTask::new(actor, "Implement auth", Some(GoalType::Feature)).unwrap();
        git_task.set_description(Some("Adjust login".into()));
        git_task.add_dependency(dependency);
        git_task.add_constraint("network:deny");
        git_task.add_acceptance_criterion("tests pass");
        TaskSpec {
            step: PlanStep::new("Implement auth"),
            task: git_task,
            objective: "Update auth flow".into(),
            kind: TaskKind::Implementation,
            gate_stage: Some(GateStage::Fast),
            owner_role: Some("coder".into()),
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

    /// Scenario: a fully populated `TaskSpec` should round-trip into a
    /// `GitTask` that preserves the title, description, constraints,
    /// acceptance criteria, dependency edges, and origin step id.
    #[test]
    fn builds_git_task() {
        let task = task();
        let built = build_git_task(Some(Uuid::new_v4()), &task).expect("git task");
        assert_eq!(built.title(), "Implement auth");
        assert!(built.description().is_some());
        assert_eq!(built.constraints(), &["network:deny".to_string()]);
        assert_eq!(built.acceptance_criteria(), &["tests pass".to_string()]);
        assert_eq!(built.dependencies().len(), 1);
        assert_eq!(built.origin_step_id(), Some(task.step_id()));
    }

    /// Scenario: building a plan from a single-task spec should produce
    /// exactly one persisted `PlanStep` carrying both the structured
    /// `inputs` payload and the serialised `checks` payload.
    #[test]
    fn builds_git_plan_steps() {
        let t = task();
        let plan = build_git_plan(
            Uuid::new_v4(),
            &ExecutionPlanSpec {
                intent_spec_id: "intent-1".into(),
                revision: 1,
                parent_revision: None,
                replan_reason: None,
                tasks: vec![t.clone()],
                max_parallel: 1,
                checkpoints: vec![ExecutionCheckpoint {
                    label: "after-fast".into(),
                    after_tasks: vec![t.id()],
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
