use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};

use git_internal::internal::object::types::ActorRef;
use rmcp::model::CallToolResult;
use serde_json::json;
use uuid::Uuid;

use super::{
    run_state::RunStateSnapshot,
    types::{
        DecisionOutcome, ExecutionPlanSpec, GateStage, OrchestratorError, PersistedCheckpoint,
        PersistedExecution, PersistedTaskArtifacts, SystemReport, TaskKind, TaskResult,
        ToolCallRecord,
    },
};
use crate::internal::ai::{
    intentspec::persistence::persist_intentspec,
    intentspec::types::IntentSpec,
    mcp::{
        resource::{
            AgentInstanceParams, ContextItemParams, CreateContextSnapshotParams,
            CreateDecisionParams, CreateEvidenceParams, CreatePatchSetParams, CreatePlanParams,
            CreateProvenanceParams, CreateRunParams, CreateTaskParams,
            CreateToolInvocationParams, IoFootprintParams, PlanStepParams, TouchedFileParams,
        },
        server::LibraMcpServer,
    },
    workflow_objects::{build_git_plan, parse_object_id},
};

const ZERO_COMMIT_SHA: &str = "0000000000000000000000000000000000000000";

pub struct ExecutionPersistenceRequest<'a> {
    pub mcp_server: &'a Arc<LibraMcpServer>,
    pub spec: &'a IntentSpec,
    pub execution_plan_spec: &'a ExecutionPlanSpec,
    pub plan_revision_specs: &'a [ExecutionPlanSpec],
    pub run_state: &'a RunStateSnapshot,
    pub system_report: &'a SystemReport,
    pub decision: &'a DecisionOutcome,
    pub working_dir: &'a Path,
    pub base_commit: Option<&'a str>,
    pub model_name: &'a str,
}

struct PatchSetRequest<'a> {
    mcp_server: &'a Arc<LibraMcpServer>,
    run_id: &'a str,
    base_commit_sha: &'a str,
    generation: u32,
    task_title: &'a str,
    task_objective: &'a str,
    tool_calls: &'a [ToolCallRecord],
}

struct EvidenceRequest<'a> {
    mcp_server: &'a Arc<LibraMcpServer>,
    run_id: &'a str,
    patchset_id: Option<&'a str>,
    kind: &'a str,
    tool: &'a str,
    command: Option<String>,
    exit_code: Option<i32>,
    summary: Option<String>,
}

struct FinalDecisionRequest<'a> {
    mcp_server: &'a Arc<LibraMcpServer>,
    run_id: &'a str,
    chosen_patchset_id: Option<&'a str>,
    checkpoint_id: Option<&'a str>,
    execution_plan: &'a ExecutionPlanSpec,
    task_results: &'a [TaskResult],
    system_report: &'a SystemReport,
    decision: &'a DecisionOutcome,
}

struct RunRequest<'a> {
    mcp_server: &'a Arc<LibraMcpServer>,
    task_id: &'a str,
    base_commit_sha: &'a str,
    plan_id: Option<&'a str>,
    context_snapshot_id: Option<&'a str>,
    task_results: &'a [TaskResult],
    decision: &'a DecisionOutcome,
    model_name: &'a str,
}

pub async fn persist_execution(
    request: ExecutionPersistenceRequest<'_>,
) -> Result<PersistedExecution, OrchestratorError> {
    let task_results = request.run_state.ordered_task_results();
    let base_commit_sha = resolve_base_commit(request.base_commit, request.working_dir);
    let intent_id = persist_intentspec(request.spec, request.mcp_server)
        .await
        .map_err(|e| OrchestratorError::ConfigError(format!("MCP create_intent failed: {e}")))?;
    let initial_snapshot_id = if snapshot_on_run_start(request.spec) {
        Some(
            create_context_snapshot(
                request.mcp_server,
                build_snapshot_summary(
                    request.spec,
                    request.plan_revision_specs.first(),
                    "Run start context snapshot",
                ),
                collect_snapshot_items(
                    request.spec,
                    request.plan_revision_specs.first(),
                    request.working_dir,
                    task_results,
                ),
            )
            .await?,
        )
    } else {
        None
    };
    let mut plan_ids = Vec::with_capacity(request.plan_revision_specs.len());
    let mut parent_plan_id = None;
    for plan_spec in request.plan_revision_specs {
        let plan_id = create_plan_revision(
            request.mcp_server,
            &intent_id,
            parent_plan_id.as_deref(),
            plan_spec,
        )
        .await?;
        parent_plan_id = Some(plan_id.clone());
        plan_ids.push(plan_id);
    }
    let root_task_id = create_execution_task(
        request.mcp_server,
        &intent_id,
        request.execution_plan_spec.summary.as_str(),
        request.spec.intent.problem_statement.as_str(),
    )
    .await?;
    let run_id = create_run(RunRequest {
        mcp_server: request.mcp_server,
        task_id: &root_task_id,
        base_commit_sha: &base_commit_sha,
        plan_id: plan_ids.last().map(String::as_str),
        context_snapshot_id: initial_snapshot_id.as_deref(),
        task_results,
        decision: request.decision,
        model_name: request.model_name,
    })
    .await?;

    let provenance_id = Some(
        create_provenance(
            request.mcp_server,
            &run_id,
            request.execution_plan_spec,
            task_results,
            request.system_report,
            request.decision,
            request.model_name,
        )
        .await?,
    );
    let mut checkpoints = create_replan_checkpoints(
        request.mcp_server,
        request.spec,
        &run_id,
        request.plan_revision_specs,
        request.working_dir,
        task_results,
    )
    .await?;

    let task_index: HashMap<Uuid, _> = request
        .execution_plan_spec
        .tasks
        .iter()
        .map(|task| (task.id(), task))
        .collect();

    let mut persisted_tasks = Vec::with_capacity(task_results.len());
    let mut generation: u32 = 1;

    for result in task_results {
        let task = task_index.get(&result.task_id).ok_or_else(|| {
            OrchestratorError::PlanningFailed(format!(
                "missing compiled task for result {} during persistence",
                result.task_id
            ))
        })?;

        let mut persisted = PersistedTaskArtifacts {
            task_id: result.task_id,
            ..PersistedTaskArtifacts::default()
        };

        for call in &result.tool_calls {
            let tool_invocation_id =
                create_tool_invocation(request.mcp_server, &run_id, task.title(), call)
                    .await?;
            persisted.tool_invocation_ids.push(tool_invocation_id);
        }

        if task.kind == TaskKind::Implementation
            && let Some(patchset_id) = create_patchset(PatchSetRequest {
                mcp_server: request.mcp_server,
                run_id: &run_id,
                base_commit_sha: &base_commit_sha,
                generation,
                task_title: task.title(),
                task_objective: task.objective.as_str(),
                tool_calls: &result.tool_calls,
            })
            .await?
        {
            generation += 1;
            persisted.patchset_id = Some(patchset_id);
        }

        if let Some(report) = &result.gate_report {
            for gate in &report.results {
                let summary = format!(
                    "{} [{}] {}",
                    gate.check_id,
                    gate.kind,
                    if gate.passed { "passed" } else { "failed" }
                );
                let evidence_id = create_evidence(EvidenceRequest {
                    mcp_server: request.mcp_server,
                    run_id: &run_id,
                    patchset_id: persisted.patchset_id.as_deref(),
                    kind: normalize_evidence_kind(&gate.kind),
                    tool: task_gate_tool_name(task.gate_stage.as_ref()),
                    command: Some(gate.check_id.clone()),
                    exit_code: Some(gate.exit_code),
                    summary: Some(summary),
                })
                .await?;
                persisted.evidence_ids.push(evidence_id);
            }
        }

        if !result.policy_violations.is_empty() {
            let summary = result
                .policy_violations
                .iter()
                .map(|violation| format!("{}: {}", violation.code, violation.message))
                .collect::<Vec<_>>()
                .join("; ");
            let evidence_id = create_evidence(EvidenceRequest {
                mcp_server: request.mcp_server,
                run_id: &run_id,
                patchset_id: persisted.patchset_id.as_deref(),
                kind: "policy",
                tool: "policy-engine",
                command: None,
                exit_code: None,
                summary: Some(summary),
            })
            .await?;
            persisted.evidence_ids.push(evidence_id);
        }

        if let Some(review) = &result.review {
            let summary = if review.issues.is_empty() {
                review.summary.clone()
            } else {
                format!("{} [{}]", review.summary, review.issues.join("; "))
            };
            let evidence_id = create_evidence(EvidenceRequest {
                mcp_server: request.mcp_server,
                run_id: &run_id,
                patchset_id: persisted.patchset_id.as_deref(),
                kind: "review",
                tool: "reviewer",
                command: None,
                exit_code: None,
                summary: Some(summary),
            })
            .await?;
            persisted.evidence_ids.push(evidence_id);
        }

        persisted_tasks.push(persisted);
    }

    let chosen_patchset_id = if *request.decision == DecisionOutcome::Commit {
        persisted_tasks
            .iter()
            .rev()
            .find_map(|task| task.patchset_id.clone())
    } else {
        None
    };
    let final_checkpoint_id = if *request.decision == DecisionOutcome::HumanReviewRequired {
        Some(
            create_context_snapshot(
                request.mcp_server,
                build_snapshot_summary(
                    request.spec,
                    Some(request.execution_plan_spec),
                    "Human review checkpoint",
                ),
                collect_snapshot_items(
                    request.spec,
                    Some(request.execution_plan_spec),
                    request.working_dir,
                    task_results,
                ),
            )
            .await?,
        )
    } else {
        None
    };

    let decision_id = Some(
        create_decision(FinalDecisionRequest {
            mcp_server: request.mcp_server,
            run_id: &run_id,
            chosen_patchset_id: chosen_patchset_id.as_deref(),
            checkpoint_id: final_checkpoint_id.as_deref(),
            execution_plan: request.execution_plan_spec,
            task_results,
            system_report: request.system_report,
            decision: request.decision,
        })
        .await?,
    );
    if let Some(snapshot_id) = final_checkpoint_id {
        checkpoints.push(PersistedCheckpoint {
            revision: request.execution_plan_spec.revision,
            reason: "human review required".to_string(),
            snapshot_id: Some(snapshot_id),
            decision_id: decision_id.clone(),
            dagrs_checkpoint_id: request
                .run_state
                .dagrs_runtime
                .checkpoints
                .last()
                .map(|checkpoint| checkpoint.checkpoint_id.clone()),
        });
    }

    checkpoints.extend(
        request
            .run_state
            .dagrs_runtime
            .checkpoints
            .iter()
            .map(|checkpoint| PersistedCheckpoint {
                revision: request.execution_plan_spec.revision,
                reason: format!(
                    "dagrs runtime checkpoint at pc {} after {} completed nodes",
                    checkpoint.pc, checkpoint.completed_nodes
                ),
                snapshot_id: None,
                decision_id: None,
                dagrs_checkpoint_id: Some(checkpoint.checkpoint_id.clone()),
            }),
    );

    Ok(PersistedExecution {
        run_id,
        initial_snapshot_id,
        provenance_id,
        decision_id,
        plan_ids,
        checkpoints,
        tasks: persisted_tasks,
    })
}

async fn create_run(request: RunRequest<'_>) -> Result<String, OrchestratorError> {
    let status = match request.decision {
        DecisionOutcome::Abandon => "failed",
        DecisionOutcome::Commit | DecisionOutcome::HumanReviewRequired => "completed",
    };
    let metrics_json = json!({
        "taskCount": request.task_results.len(),
        "completedTasks": request.task_results.iter().filter(|result| result.status == super::types::TaskNodeStatus::Completed).count(),
        "failedTasks": request.task_results.iter().filter(|result| result.status == super::types::TaskNodeStatus::Failed).count(),
        "toolCalls": request.task_results.iter().map(|result| result.tool_calls.len()).sum::<usize>(),
        "policyViolations": request.task_results.iter().map(|result| result.policy_violations.len()).sum::<usize>(),
        "model": request.model_name,
    })
    .to_string();

    let params = CreateRunParams {
        task_id: request.task_id.to_string(),
        base_commit_sha: request.base_commit_sha.to_string(),
        plan_id: request.plan_id.map(ToString::to_string),
        status: Some(status.to_string()),
        context_snapshot_id: request.context_snapshot_id.map(ToString::to_string),
        error: request.task_results.iter().find_map(|result| {
            (result.status == super::types::TaskNodeStatus::Failed).then(|| {
                result
                    .agent_output
                    .clone()
                    .unwrap_or_else(|| "task execution failed".to_string())
            })
        }),
        agent_instances: Some(vec![AgentInstanceParams {
            role: "orchestrator".to_string(),
            provider_route: Some(request.model_name.to_string()),
        }]),
        metrics_json: Some(metrics_json),
        reason: Some(format!(
            "orchestrator finished with decision {:?}",
            request.decision
        )),
        orchestrator_version: Some("libra-intentspec".to_string()),
        tags: None,
        external_ids: None,
        actor_kind: Some("system".to_string()),
        actor_id: Some("libra-orchestrator".to_string()),
    };

    let actor = resolve_actor(
        request.mcp_server,
        params.actor_kind.as_deref(),
        params.actor_id.as_deref(),
    )?;
    let result = request
        .mcp_server
        .create_run_impl(params, actor)
        .await
        .map_err(|e| OrchestratorError::ConfigError(format!("MCP create_run failed: {e:?}")))?;
    parse_created_id("run", &result)
}

async fn create_execution_task(
    mcp_server: &Arc<LibraMcpServer>,
    intent_id: &str,
    title: &str,
    description: &str,
) -> Result<String, OrchestratorError> {
    let params = CreateTaskParams {
        title: title.to_string(),
        description: Some(description.to_string()),
        goal_type: None,
        constraints: None,
        acceptance_criteria: None,
        requested_by_kind: None,
        requested_by_id: None,
        dependencies: None,
        intent_id: Some(intent_id.to_string()),
        parent_task_id: None,
        origin_step_id: None,
        status: Some("running".to_string()),
        reason: Some("orchestrator execution root task".to_string()),
        tags: None,
        external_ids: None,
        actor_kind: Some("system".to_string()),
        actor_id: Some("libra-executor".to_string()),
    };

    let actor = resolve_actor(
        mcp_server,
        params.actor_kind.as_deref(),
        params.actor_id.as_deref(),
    )?;
    let result = mcp_server
        .create_task_impl(params, actor)
        .await
        .map_err(|e| {
            OrchestratorError::ConfigError(format!("MCP create_task failed: {e:?}"))
        })?;
    parse_created_id("task", &result)
}

async fn create_plan_revision(
    mcp_server: &Arc<LibraMcpServer>,
    intent_id: &str,
    parent_plan_id: Option<&str>,
    plan: &ExecutionPlanSpec,
) -> Result<String, OrchestratorError> {
    let git_plan = build_git_plan(
        parse_object_id(intent_id)
            .map_err(|e| OrchestratorError::ConfigError(format!("invalid intent id: {e}")))?,
        plan,
    )
    .map_err(|e| OrchestratorError::ConfigError(format!("failed to build git plan: {e}")))?;
    let steps = git_plan
        .steps()
        .iter()
        .map(|step| PlanStepParams {
            description: step.description().to_string(),
            inputs: step.inputs().cloned(),
            checks: step.checks().cloned(),
        })
        .collect::<Vec<_>>();

    let params = CreatePlanParams {
        intent_id: intent_id.to_string(),
        parent_plan_ids: parent_plan_id.map(|id| vec![id.to_string()]),
        context_frame_ids: None,
        steps: Some(steps),
        tags: None,
        external_ids: None,
        actor_kind: Some("system".to_string()),
        actor_id: Some("libra-plan".to_string()),
    };

    let actor = resolve_actor(
        mcp_server,
        params.actor_kind.as_deref(),
        params.actor_id.as_deref(),
    )?;
    let result = mcp_server
        .create_plan_impl(params, actor)
        .await
        .map_err(|e| OrchestratorError::ConfigError(format!("MCP create_plan failed: {e:?}")))?;
    parse_created_id("plan", &result)
}

async fn create_provenance(
    mcp_server: &Arc<LibraMcpServer>,
    run_id: &str,
    execution_plan: &ExecutionPlanSpec,
    task_results: &[TaskResult],
    system_report: &SystemReport,
    decision: &DecisionOutcome,
    model_name: &str,
) -> Result<String, OrchestratorError> {
    let parameters_json = json!({
        "intentSpecId": execution_plan.intent_spec_id,
        "planSummary": execution_plan.summary,
        "parallelGroups": execution_plan.parallel_groups.len(),
        "checkpoints": execution_plan.checkpoints.iter().map(|checkpoint| checkpoint.label.clone()).collect::<Vec<_>>(),
        "decision": format!("{decision:?}"),
        "systemReport": {
            "overallPassed": system_report.overall_passed,
            "integrationPassed": system_report.integration.all_required_passed,
            "securityPassed": system_report.security.all_required_passed,
            "releasePassed": system_report.release.all_required_passed,
        },
        "taskRetries": task_results.iter().map(|result| json!({
            "taskId": result.task_id,
            "retryCount": result.retry_count,
        })).collect::<Vec<_>>(),
    })
    .to_string();

    let params = CreateProvenanceParams {
        run_id: run_id.to_string(),
        provider: "internal".to_string(),
        model: model_name.to_string(),
        parameters_json: Some(parameters_json),
        temperature: None,
        max_tokens: None,
        tags: None,
        external_ids: None,
        actor_kind: Some("agent".to_string()),
        actor_id: Some("libra-coder".to_string()),
    };

    let actor = resolve_actor(
        mcp_server,
        params.actor_kind.as_deref(),
        params.actor_id.as_deref(),
    )?;
    let result = mcp_server
        .create_provenance_impl(params, actor)
        .await
        .map_err(|e| {
            OrchestratorError::ConfigError(format!("MCP create_provenance failed: {e:?}"))
        })?;
    parse_created_id("provenance", &result)
}

async fn create_tool_invocation(
    mcp_server: &Arc<LibraMcpServer>,
    run_id: &str,
    task_title: &str,
    call: &ToolCallRecord,
) -> Result<String, OrchestratorError> {
    let result_summary = call
        .summary
        .as_ref()
        .map(|summary| format!("{task_title}: {summary}"));
    let params = CreateToolInvocationParams {
        run_id: run_id.to_string(),
        tool_name: call.tool_name.clone(),
        status: Some(if call.success { "ok" } else { "error" }.to_string()),
        args_json: call
            .arguments_json
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| {
                OrchestratorError::ConfigError(format!("failed to encode tool args for MCP: {e}"))
            })?,
        io_footprint: Some(IoFootprintParams {
            paths_read: (!call.paths_read.is_empty()).then(|| call.paths_read.clone()),
            paths_written: (!call.paths_written.is_empty()).then(|| call.paths_written.clone()),
        }),
        result_summary,
        artifacts: None,
        tags: None,
        external_ids: None,
        actor_kind: Some("agent".to_string()),
        actor_id: Some("libra-coder".to_string()),
    };

    let actor = resolve_actor(
        mcp_server,
        params.actor_kind.as_deref(),
        params.actor_id.as_deref(),
    )?;
    let result = mcp_server
        .create_tool_invocation_impl(params, actor)
        .await
        .map_err(|e| {
            OrchestratorError::ConfigError(format!("MCP create_tool_invocation failed: {e:?}"))
        })?;
    parse_created_id("tool invocation", &result)
}

async fn create_patchset(
    request: PatchSetRequest<'_>,
) -> Result<Option<String>, OrchestratorError> {
    let (touched_files, diff_text) = build_patchset_payload(request.tool_calls);
    if touched_files.is_empty() {
        return Ok(None);
    }

    let params = CreatePatchSetParams {
        run_id: request.run_id.to_string(),
        generation: request.generation,
        sequence: Some(request.generation),
        base_commit_sha: request.base_commit_sha.to_string(),
        touched_files: Some(touched_files),
        rationale: Some(format!(
            "{}: {}",
            request.task_title, request.task_objective
        )),
        diff_format: diff_text.as_ref().map(|_| "unified_diff".to_string()),
        diff_artifact: None,
        tags: None,
        external_ids: None,
        actor_kind: Some("agent".to_string()),
        actor_id: Some("libra-coder".to_string()),
    };

    let actor = resolve_actor(
        request.mcp_server,
        params.actor_kind.as_deref(),
        params.actor_id.as_deref(),
    )?;
    let result = request
        .mcp_server
        .create_patchset_impl(params, actor)
        .await
        .map_err(|e| {
            OrchestratorError::ConfigError(format!("MCP create_patchset failed: {e:?}"))
        })?;
    parse_created_id("patchset", &result).map(Some)
}

async fn create_evidence(request: EvidenceRequest<'_>) -> Result<String, OrchestratorError> {
    let params = CreateEvidenceParams {
        run_id: request.run_id.to_string(),
        patchset_id: request.patchset_id.map(ToString::to_string),
        kind: request.kind.to_string(),
        tool: request.tool.to_string(),
        command: request.command,
        exit_code: request.exit_code,
        summary: request.summary,
        report_artifacts: None,
        tags: None,
        external_ids: None,
        actor_kind: Some("system".to_string()),
        actor_id: Some("libra-verifier".to_string()),
    };

    let actor = resolve_actor(
        request.mcp_server,
        params.actor_kind.as_deref(),
        params.actor_id.as_deref(),
    )?;
    let result = request
        .mcp_server
        .create_evidence_impl(params, actor)
        .await
        .map_err(|e| {
            OrchestratorError::ConfigError(format!("MCP create_evidence failed: {e:?}"))
        })?;
    parse_created_id("evidence", &result)
}

async fn create_decision(request: FinalDecisionRequest<'_>) -> Result<String, OrchestratorError> {
    let decision_type = match request.decision {
        DecisionOutcome::Commit => "commit",
        DecisionOutcome::HumanReviewRequired => "checkpoint",
        DecisionOutcome::Abandon => "abandon",
    };
    let rationale = Some(format!(
        "{}; overall_passed={}; failed_tasks={}; checkpoints={}",
        request.execution_plan.summary,
        request.system_report.overall_passed,
        request
            .task_results
            .iter()
            .filter(|result| result.status == super::types::TaskNodeStatus::Failed)
            .count(),
        request.execution_plan.checkpoints.len()
    ));

    let params = CreateDecisionParams {
        run_id: request.run_id.to_string(),
        decision_type: decision_type.to_string(),
        chosen_patchset_id: request.chosen_patchset_id.map(ToString::to_string),
        result_commit_sha: None,
        checkpoint_id: request.checkpoint_id.map(ToString::to_string),
        rationale,
        tags: None,
        external_ids: None,
        actor_kind: Some("system".to_string()),
        actor_id: Some("libra-orchestrator".to_string()),
    };

    let actor = resolve_actor(
        request.mcp_server,
        params.actor_kind.as_deref(),
        params.actor_id.as_deref(),
    )?;
    let result = request
        .mcp_server
        .create_decision_impl(params, actor)
        .await
        .map_err(|e| {
            OrchestratorError::ConfigError(format!("MCP create_decision failed: {e:?}"))
        })?;
    parse_created_id("decision", &result)
}

fn build_patchset_payload(
    tool_calls: &[ToolCallRecord],
) -> (Vec<TouchedFileParams>, Option<String>) {
    let mut touched: BTreeMap<String, TouchedFileParams> = BTreeMap::new();
    let mut diffs = Vec::new();

    for call in tool_calls {
        if !call.diffs.is_empty() {
            for diff in &call.diffs {
                let (lines_added, lines_deleted) = count_diff_lines(&diff.diff);
                touched.insert(
                    diff.path.clone(),
                    TouchedFileParams {
                        path: diff.path.clone(),
                        change_type: normalize_change_type(&diff.change_type).to_string(),
                        lines_added,
                        lines_deleted,
                    },
                );
                diffs.push(diff.diff.clone());
            }
            continue;
        }

        for path in &call.paths_written {
            touched.entry(path.clone()).or_insert(TouchedFileParams {
                path: path.clone(),
                change_type: "modify".to_string(),
                lines_added: 0,
                lines_deleted: 0,
            });
        }
    }

    let diff_text = (!diffs.is_empty()).then(|| diffs.join("\n"));
    (touched.into_values().collect(), diff_text)
}

fn count_diff_lines(diff: &str) -> (u32, u32) {
    let mut added = 0_u32;
    let mut deleted = 0_u32;
    for line in diff.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if line.starts_with('+') {
            added += 1;
        } else if line.starts_with('-') {
            deleted += 1;
        }
    }
    (added, deleted)
}

fn normalize_change_type(change_type: &str) -> &str {
    match change_type {
        "add" | "modify" | "delete" | "rename" | "copy" => change_type,
        "update" => "modify",
        _ => "modify",
    }
}

fn normalize_evidence_kind(kind: &str) -> &str {
    match kind {
        "test" | "lint" | "build" => kind,
        _ => "other",
    }
}

fn task_gate_tool_name(stage: Option<&GateStage>) -> &'static str {
    match stage {
        Some(GateStage::Fast) => "gate-fast",
        Some(GateStage::Integration) => "gate-integration",
        Some(GateStage::Security) => "gate-security",
        Some(GateStage::Release) => "gate-release",
        None => "gate",
    }
}

fn resolve_actor(
    mcp_server: &Arc<LibraMcpServer>,
    actor_kind: Option<&str>,
    actor_id: Option<&str>,
) -> Result<ActorRef, OrchestratorError> {
    mcp_server
        .resolve_actor_from_params(actor_kind, actor_id)
        .map_err(|e| OrchestratorError::ConfigError(format!("failed to resolve MCP actor: {e:?}")))
}

fn parse_created_id(kind: &str, result: &CallToolResult) -> Result<String, OrchestratorError> {
    if result.is_error.unwrap_or(false) {
        return Err(OrchestratorError::ConfigError(format!(
            "MCP create_{kind} returned an error result"
        )));
    }

    for content in &result.content {
        if let Some(text) = content.as_text().map(|value| value.text.as_str())
            && let Some(id) = text.split("ID:").nth(1)
        {
            let id = id.trim();
            if !id.is_empty() {
                return Ok(id.to_string());
            }
        }
    }

    Err(OrchestratorError::ConfigError(format!(
        "failed to parse {kind} id from MCP response"
    )))
}

fn resolve_base_commit(base_commit: Option<&str>, working_dir: &Path) -> String {
    if let Some(commit) = base_commit {
        let trimmed = commit.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    let output = Command::new("git")
        .arg("rev-parse")
        .arg("HEAD")
        .current_dir(working_dir)
        .output();

    match output {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if text.is_empty() {
                ZERO_COMMIT_SHA.to_string()
            } else {
                text
            }
        }
        _ => ZERO_COMMIT_SHA.to_string(),
    }
}

async fn create_replan_checkpoints(
    mcp_server: &Arc<LibraMcpServer>,
    spec: &IntentSpec,
    run_id: &str,
    plan_revisions: &[ExecutionPlanSpec],
    working_dir: &Path,
    task_results: &[TaskResult],
) -> Result<Vec<PersistedCheckpoint>, OrchestratorError> {
    if !checkpoint_on_replan(spec) && !checkpoint_before_replan(spec) {
        return Ok(Vec::new());
    }

    let mut persisted = Vec::new();
    for (index, entry) in spec.lifecycle.change_log.iter().enumerate() {
        let Some(plan) = plan_revisions.get(index) else {
            break;
        };

        let snapshot_id = if checkpoint_on_replan(spec) || checkpoint_before_replan(spec) {
            Some(
                create_context_snapshot(
                    mcp_server,
                    build_checkpoint_summary(plan, entry.reason.as_str()),
                    collect_snapshot_items(spec, Some(plan), working_dir, task_results),
                )
                .await?,
            )
        } else {
            None
        };
        let decision_id = if checkpoint_before_replan(spec) {
            Some(
                create_checkpoint_decision(
                    mcp_server,
                    run_id,
                    snapshot_id.as_deref(),
                    plan,
                    entry.reason.as_str(),
                )
                .await?,
            )
        } else {
            None
        };
        persisted.push(PersistedCheckpoint {
            revision: plan.revision,
            reason: entry.reason.clone(),
            snapshot_id,
            decision_id,
            dagrs_checkpoint_id: None,
        });
    }

    Ok(persisted)
}

async fn create_context_snapshot(
    mcp_server: &Arc<LibraMcpServer>,
    summary: String,
    items: Vec<ContextItemParams>,
) -> Result<String, OrchestratorError> {
    let params = CreateContextSnapshotParams {
        selection_strategy: if items.is_empty() {
            "heuristic".to_string()
        } else {
            "explicit".to_string()
        },
        items: (!items.is_empty()).then_some(items),
        summary: Some(summary),
        tags: None,
        external_ids: None,
        actor_kind: Some("system".to_string()),
        actor_id: Some("libra-orchestrator".to_string()),
    };

    let actor = resolve_actor(
        mcp_server,
        params.actor_kind.as_deref(),
        params.actor_id.as_deref(),
    )?;
    let result = mcp_server
        .create_context_snapshot_impl(params, actor)
        .await
        .map_err(|e| {
            OrchestratorError::ConfigError(format!("MCP create_context_snapshot failed: {e:?}"))
        })?;
    parse_created_id("context snapshot", &result)
}

async fn create_checkpoint_decision(
    mcp_server: &Arc<LibraMcpServer>,
    run_id: &str,
    checkpoint_id: Option<&str>,
    plan: &ExecutionPlanSpec,
    reason: &str,
) -> Result<String, OrchestratorError> {
    let params = CreateDecisionParams {
        run_id: run_id.to_string(),
        decision_type: "checkpoint".to_string(),
        chosen_patchset_id: None,
        result_commit_sha: None,
        checkpoint_id: checkpoint_id.map(ToString::to_string),
        rationale: Some(format!(
            "checkpoint before replanning plan revision {}: {}",
            plan.revision, reason
        )),
        tags: None,
        external_ids: None,
        actor_kind: Some("system".to_string()),
        actor_id: Some("libra-orchestrator".to_string()),
    };

    let actor = resolve_actor(
        mcp_server,
        params.actor_kind.as_deref(),
        params.actor_id.as_deref(),
    )?;
    let result = mcp_server
        .create_decision_impl(params, actor)
        .await
        .map_err(|e| {
            OrchestratorError::ConfigError(format!("MCP create_checkpoint_decision failed: {e:?}"))
        })?;
    parse_created_id("decision", &result)
}

fn collect_snapshot_items(
    spec: &IntentSpec,
    plan: Option<&ExecutionPlanSpec>,
    working_dir: &Path,
    task_results: &[TaskResult],
) -> Vec<ContextItemParams> {
    let mut candidates = BTreeSet::new();
    if let Some(touch_hints) = &spec.intent.touch_hints {
        candidates.extend(touch_hints.files.iter().cloned());
    }
    if let Some(plan) = plan {
        for task in &plan.tasks {
            candidates.extend(task.contract.touch_files.iter().cloned());
        }
    }
    for result in task_results {
        for call in &result.tool_calls {
            candidates.extend(call.paths_written.iter().cloned());
            candidates.extend(call.paths_read.iter().cloned());
        }
    }

    candidates
        .into_iter()
        .filter_map(|path| build_context_item(working_dir, path))
        .collect()
}

fn build_context_item(working_dir: &Path, path: String) -> Option<ContextItemParams> {
    if !is_literal_file_path(&path) {
        return None;
    }

    let resolved = resolve_workspace_file(working_dir, &path)?;
    let content_hash = hash_file_blob(working_dir, &resolved)?;
    Some(ContextItemParams {
        kind: Some("file".to_string()),
        path,
        preview: None,
        content_hash: Some(content_hash.clone()),
        blob_hash: Some(content_hash),
    })
}

fn resolve_workspace_file(working_dir: &Path, path: &str) -> Option<PathBuf> {
    let workspace_root = fs::canonicalize(working_dir).ok()?;
    let candidate = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        workspace_root.join(path)
    };
    let canonical = fs::canonicalize(candidate).ok()?;
    (canonical.is_file() && canonical.starts_with(&workspace_root)).then_some(canonical)
}

fn is_literal_file_path(path: &str) -> bool {
    !path.is_empty()
        && !path.ends_with('/')
        && !path.contains('*')
        && !path.contains('?')
        && !path.contains('[')
        && !path.contains('{')
}

fn hash_file_blob(working_dir: &Path, path: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("hash-object")
        .arg(path)
        .current_dir(working_dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let hash = String::from_utf8(output.stdout).ok()?;
    let trimmed = hash.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn build_snapshot_summary(
    spec: &IntentSpec,
    plan: Option<&ExecutionPlanSpec>,
    prefix: &str,
) -> String {
    match plan {
        Some(plan) => format!(
            "{prefix}: {} (intent {}, plan revision {})",
            spec.intent.summary, spec.metadata.id, plan.revision
        ),
        None => format!(
            "{prefix}: {} (intent {})",
            spec.intent.summary, spec.metadata.id
        ),
    }
}

fn build_checkpoint_summary(plan: &ExecutionPlanSpec, reason: &str) -> String {
    format!(
        "Checkpoint before replan after revision {}: {}",
        plan.revision, reason
    )
}

fn snapshot_on_run_start(spec: &IntentSpec) -> bool {
    spec.libra
        .as_ref()
        .and_then(|libra| libra.run_policy.as_ref())
        .is_none_or(|policy| policy.snapshot_on_run_start)
}

fn checkpoint_on_replan(spec: &IntentSpec) -> bool {
    spec.libra
        .as_ref()
        .and_then(|libra| libra.context_pipeline.as_ref())
        .is_none_or(|pipeline| pipeline.checkpoint_on_replan)
}

fn checkpoint_before_replan(spec: &IntentSpec) -> bool {
    spec.libra
        .as_ref()
        .and_then(|libra| libra.decision_policy.as_ref())
        .is_none_or(|policy| policy.checkpoint_before_replan)
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, path::Path, sync::Arc};

    use git_internal::internal::object::{task::Task as GitTask, types::ActorRef};
    use sea_orm::{ConnectionTrait, Database, Schema};
    use tempfile::tempdir;
    use super::*;
    use crate::{
        internal::{
            ai::{
                history::HistoryManager,
                intentspec::types::*,
                orchestrator::{
                    run_state::{RunStateSnapshot, TaskStatusSnapshot},
                    types::{
                        ExecutionCheckpoint, ExecutionPlanSpec, GateReport, GateResult,
                        TaskContract, TaskKind, TaskNodeStatus, TaskSpec, ToolDiffRecord,
                    },
                },
            },
            model::reference,
        },
        utils::storage::local::LocalStorage,
    };

    async fn setup_server() -> Arc<LibraMcpServer> {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        let builder = db.get_database_backend();
        let schema = Schema::new(builder);
        let stmt = schema.create_table_from_entity(reference::Entity);
        db.execute(builder.build(&stmt)).await.unwrap();

        let temp_dir = tempdir().unwrap();
        let storage = Arc::new(LocalStorage::new(temp_dir.path().join("objects")));
        let history_manager = Arc::new(HistoryManager::new(
            storage.clone(),
            temp_dir.path().to_path_buf(),
            Arc::new(db),
        ));
        Arc::new(LibraMcpServer::new(Some(history_manager), Some(storage)))
    }

    fn test_spec(change_log: Vec<ChangeLogEntry>) -> IntentSpec {
        IntentSpec {
            api_version: "intentspec.io/v1alpha1".into(),
            kind: "IntentSpec".into(),
            metadata: Metadata {
                id: "intent-1".into(),
                created_at: "2025-01-01T00:00:00Z".into(),
                created_by: CreatedBy {
                    creator_type: CreatorType::User,
                    id: "tester".into(),
                    display_name: None,
                },
                target: Target {
                    repo: RepoTarget {
                        repo_type: RepoType::Local,
                        locator: ".".into(),
                    },
                    base_ref: "HEAD".into(),
                    workspace_id: None,
                    labels: BTreeMap::new(),
                },
            },
            intent: Intent {
                summary: "Implement feature and verify it".into(),
                problem_statement: "problem".into(),
                change_type: ChangeType::Feature,
                objectives: vec!["Update src/lib.rs".into()],
                in_scope: vec!["src/".into()],
                out_of_scope: vec![],
                touch_hints: Some(TouchHints {
                    files: vec!["src/lib.rs".into()],
                    symbols: vec![],
                    apis: vec![],
                }),
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
                    network_policy: NetworkPolicy::Deny,
                    dependency_policy: DependencyPolicy::NoNew,
                    crypto_policy: String::new(),
                },
                privacy: ConstraintPrivacy {
                    data_classes_allowed: vec![DataClass::Public],
                    redaction_required: false,
                    retention_days: 1,
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
                    max_wall_clock_seconds: 30,
                    max_cost_units: 0,
                },
            },
            risk: Risk {
                level: RiskLevel::Low,
                rationale: String::new(),
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
                    encoding_policy: EncodingPolicy::StrictJson,
                    no_direct_eval: true,
                },
            },
            execution: ExecutionPolicy {
                concurrency: ConcurrencyPolicy {
                    max_parallel_tasks: 1,
                },
                retry: RetryPolicy {
                    max_retries: 1,
                    backoff_seconds: 0,
                },
                replan: ReplanPolicy {
                    triggers: vec![ReplanTrigger::SecurityGateFail],
                },
            },
            provenance: ProvenancePolicy {
                require_slsa_provenance: true,
                require_sbom: false,
                transparency_log: TransparencyLogPolicy {
                    mode: TransparencyMode::None,
                },
                bindings: ProvenanceBindings {
                    embed_intent_spec_digest: true,
                    embed_evidence_digests: true,
                },
            },
            lifecycle: Lifecycle {
                schema_version: "1".into(),
                status: LifecycleStatus::Active,
                change_log,
            },
            libra: Some(LibraBinding {
                object_store: None,
                context_pipeline: None,
                plan_generation: None,
                run_policy: None,
                actor_mapping: None,
                decision_policy: None,
            }),
            artifacts: Artifacts {
                required: vec![],
                retention: ArtifactRetention::default(),
            },
            extensions: BTreeMap::new(),
        }
    }

    #[tokio::test]
    async fn test_persist_execution_creates_object_chain() {
        let server = setup_server().await;
        let spec = test_spec(vec![ChangeLogEntry {
            at: "2025-01-01T00:01:00Z".into(),
            by: "libra-orchestrator".into(),
            reason: "security gate failed".into(),
            diff_summary: "revision 2: replan in serial mode".into(),
        }]);
        let impl_task = {
            let actor = ActorRef::agent("test-persistence").unwrap();
            GitTask::new(actor, "Edit source", None).unwrap()
        };
        let impl_task_id = impl_task.header().object_id();
        let gate_task = {
            let actor = ActorRef::agent("test-persistence").unwrap();
            let mut task = GitTask::new(actor, "Run fast checks", None).unwrap();
            task.add_dependency(impl_task_id);
            task
        };
        let gate_task_id = gate_task.header().object_id();
        let plan_spec = ExecutionPlanSpec {
            intent_spec_id: "intent-1".to_string(),
            summary: "Implement feature and verify it".to_string(),
            revision: 1,
            parent_revision: None,
            replan_reason: None,
            tasks: vec![
                TaskSpec {
                    task: impl_task,
                    objective: "Update src/lib.rs".to_string(),
                    kind: TaskKind::Implementation,
                    gate_stage: None,
                    owner_role: Some("coder".to_string()),
                    scope_in: vec!["src/".to_string()],
                    scope_out: vec![],
                    checks: vec![],
                    contract: TaskContract::default(),
                },
                TaskSpec {
                    task: gate_task,
                    objective: "Verify".to_string(),
                    kind: TaskKind::Gate,
                    gate_stage: Some(GateStage::Fast),
                    owner_role: Some("verifier".to_string()),
                    scope_in: vec![],
                    scope_out: vec![],
                    checks: vec![],
                    contract: TaskContract::default(),
                },
            ],
            max_parallel: 1,
            parallel_groups: vec![vec![impl_task_id], vec![gate_task_id]],
            checkpoints: vec![ExecutionCheckpoint {
                label: "after-fast".to_string(),
                after_tasks: vec![gate_task_id],
                reason: "verify".to_string(),
            }],
        };
        let results = vec![
            TaskResult {
                task_id: impl_task_id,
                status: TaskNodeStatus::Completed,
                gate_report: None,
                agent_output: Some("done".to_string()),
                retry_count: 0,
                tool_calls: vec![ToolCallRecord {
                    tool_name: "apply_patch".to_string(),
                    action: "write".to_string(),
                    arguments_json: Some(json!({"input": "*** Begin Patch"})),
                    paths_read: vec![],
                    paths_written: vec!["src/lib.rs".to_string()],
                    success: true,
                    summary: Some("updated src/lib.rs".to_string()),
                    diffs: vec![ToolDiffRecord {
                        path: "src/lib.rs".to_string(),
                        change_type: "modify".to_string(),
                        diff: "--- a/src/lib.rs\n+++ b/src/lib.rs\n+fn added() {}\n".to_string(),
                    }],
                }],
                policy_violations: vec![],
                review: None,
            },
            TaskResult {
                task_id: gate_task_id,
                status: TaskNodeStatus::Completed,
                gate_report: Some(GateReport {
                    results: vec![GateResult {
                        check_id: "cargo-test".to_string(),
                        kind: "test".to_string(),
                        passed: true,
                        exit_code: 0,
                        stdout: String::new(),
                        stderr: String::new(),
                        duration_ms: 10,
                        timed_out: false,
                    }],
                    all_required_passed: true,
                }),
                agent_output: None,
                retry_count: 0,
                tool_calls: vec![],
                policy_violations: vec![],
                review: None,
            },
        ];
        let system_report = SystemReport {
            integration: GateReport::empty(),
            security: GateReport::empty(),
            release: GateReport::empty(),
            review_passed: true,
            review_findings: vec![],
            artifacts_complete: true,
            missing_artifacts: vec![],
            overall_passed: true,
        };
        let run_state = RunStateSnapshot {
            intent_spec_id: plan_spec.intent_spec_id.clone(),
            revision: plan_spec.revision,
            task_statuses: results
                .iter()
                .map(|result| TaskStatusSnapshot {
                    task_id: result.task_id,
                    status: result.status.clone(),
                })
                .collect(),
            task_results: results.clone(),
            dagrs_runtime: Default::default(),
        };

        let persisted = persist_execution(ExecutionPersistenceRequest {
            mcp_server: &server,
            spec: &spec,
            execution_plan_spec: &plan_spec,
            plan_revision_specs: std::slice::from_ref(&plan_spec),
            run_state: &run_state,
            system_report: &system_report,
            decision: &DecisionOutcome::Commit,
            working_dir: Path::new("."),
            base_commit: Some(ZERO_COMMIT_SHA),
            model_name: "test-model",
        })
        .await
        .unwrap();

        assert!(!persisted.run_id.is_empty());
        assert!(persisted.initial_snapshot_id.is_some());
        assert!(persisted.provenance_id.is_some());
        assert!(persisted.decision_id.is_some());
        assert_eq!(persisted.plan_ids.len(), 1);
        assert_eq!(persisted.checkpoints.len(), 1);
        assert_eq!(persisted.tasks.len(), 2);
        assert_eq!(persisted.tasks[0].tool_invocation_ids.len(), 1);
        assert!(persisted.tasks[0].patchset_id.is_some());
        assert_eq!(persisted.tasks[1].evidence_ids.len(), 1);

        let history = server.intent_history_manager.as_ref().unwrap();
        assert_eq!(history.list_objects("run").await.unwrap().len(), 1);
        assert_eq!(history.list_objects("patchset").await.unwrap().len(), 1);
        assert_eq!(history.list_objects("evidence").await.unwrap().len(), 1);
        assert_eq!(history.list_objects("decision").await.unwrap().len(), 2);
        assert_eq!(history.list_objects("provenance").await.unwrap().len(), 1);
        assert_eq!(history.list_objects("invocation").await.unwrap().len(), 1);
        assert_eq!(history.list_objects("snapshot").await.unwrap().len(), 2);
    }
}
