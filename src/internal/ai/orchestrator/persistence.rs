use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
    process::Command,
    sync::Arc,
};

use git_internal::internal::object::types::ActorRef;
use rmcp::model::CallToolResult;
use serde_json::json;
use uuid::Uuid;

use super::types::{
    DecisionOutcome, ExecutionPlan, GateStage, OrchestratorError, PersistedExecution,
    PersistedTaskArtifacts, SystemReport, TaskKind, TaskResult, ToolCallRecord,
};
use crate::internal::ai::mcp::{
    resource::{
        AgentInstanceParams, CreateDecisionParams, CreateEvidenceParams, CreatePatchSetParams,
        CreateProvenanceParams, CreateRunParams, CreateToolInvocationParams, IoFootprintParams,
        TouchedFileParams,
    },
    server::LibraMcpServer,
};

const ZERO_COMMIT_SHA: &str = "0000000000000000000000000000000000000000";

pub struct ExecutionPersistenceRequest<'a> {
    pub mcp_server: &'a Arc<LibraMcpServer>,
    pub execution_plan: &'a ExecutionPlan,
    pub task_results: &'a [TaskResult],
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
    task_status: &'a super::types::TaskNodeStatus,
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

pub async fn persist_execution(
    request: ExecutionPersistenceRequest<'_>,
) -> Result<PersistedExecution, OrchestratorError> {
    let base_commit_sha = resolve_base_commit(request.base_commit, request.working_dir);
    let run_id = create_run(
        request.mcp_server,
        &base_commit_sha,
        request.task_results,
        request.decision,
        request.model_name,
    )
    .await?;

    let provenance_id = Some(
        create_provenance(
            request.mcp_server,
            &run_id,
            request.execution_plan,
            request.task_results,
            request.system_report,
            request.decision,
            request.model_name,
        )
        .await?,
    );

    let task_index: HashMap<Uuid, _> = request
        .execution_plan
        .dag
        .nodes
        .iter()
        .map(|node| (node.id, node))
        .collect();

    let mut persisted_tasks = Vec::with_capacity(request.task_results.len());
    let mut generation: u32 = 1;

    for result in request.task_results {
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
                create_tool_invocation(request.mcp_server, &run_id, task.title.as_str(), call)
                    .await?;
            persisted.tool_invocation_ids.push(tool_invocation_id);
        }

        if task.kind == TaskKind::Implementation
            && let Some(patchset_id) = create_patchset(PatchSetRequest {
                mcp_server: request.mcp_server,
                run_id: &run_id,
                base_commit_sha: &base_commit_sha,
                generation,
                task_title: task.title.as_str(),
                task_objective: task.objective.as_str(),
                tool_calls: &result.tool_calls,
                task_status: &result.status,
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

    let decision_id = Some(
        create_decision(
            request.mcp_server,
            &run_id,
            chosen_patchset_id.as_deref(),
            request.execution_plan,
            request.task_results,
            request.system_report,
            request.decision,
        )
        .await?,
    );

    Ok(PersistedExecution {
        run_id,
        provenance_id,
        decision_id,
        tasks: persisted_tasks,
    })
}

async fn create_run(
    mcp_server: &Arc<LibraMcpServer>,
    base_commit_sha: &str,
    task_results: &[TaskResult],
    decision: &DecisionOutcome,
    model_name: &str,
) -> Result<String, OrchestratorError> {
    let status = match decision {
        DecisionOutcome::Abandon => "failed",
        DecisionOutcome::Commit | DecisionOutcome::HumanReviewRequired => "completed",
    };
    let metrics_json = json!({
        "taskCount": task_results.len(),
        "completedTasks": task_results.iter().filter(|result| result.status == super::types::TaskNodeStatus::Completed).count(),
        "failedTasks": task_results.iter().filter(|result| result.status == super::types::TaskNodeStatus::Failed).count(),
        "toolCalls": task_results.iter().map(|result| result.tool_calls.len()).sum::<usize>(),
        "policyViolations": task_results.iter().map(|result| result.policy_violations.len()).sum::<usize>(),
    })
    .to_string();

    let params = CreateRunParams {
        task_id: Uuid::new_v4().to_string(),
        base_commit_sha: base_commit_sha.to_string(),
        status: Some(status.to_string()),
        context_snapshot_id: None,
        error: task_results.iter().find_map(|result| {
            (result.status == super::types::TaskNodeStatus::Failed).then(|| {
                result
                    .agent_output
                    .clone()
                    .unwrap_or_else(|| "task execution failed".to_string())
            })
        }),
        agent_instances: Some(vec![AgentInstanceParams {
            role: "orchestrator".to_string(),
            provider_route: Some(model_name.to_string()),
        }]),
        metrics_json: Some(metrics_json),
        orchestrator_version: Some("libra-intentspec".to_string()),
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
        .create_run_impl(params, actor)
        .await
        .map_err(|e| OrchestratorError::ConfigError(format!("MCP create_run failed: {e:?}")))?;
    parse_created_id("run", &result)
}

async fn create_provenance(
    mcp_server: &Arc<LibraMcpServer>,
    run_id: &str,
    execution_plan: &ExecutionPlan,
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
        token_usage_json: None,
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
        base_commit_sha: request.base_commit_sha.to_string(),
        touched_files: Some(touched_files),
        rationale: Some(format!(
            "{}: {}",
            request.task_title, request.task_objective
        )),
        apply_status: Some(
            match request.task_status {
                super::types::TaskNodeStatus::Completed => "applied",
                super::types::TaskNodeStatus::Failed => "rejected",
                _ => "proposed",
            }
            .to_string(),
        ),
        diff_format: diff_text.as_ref().map(|_| "unified".to_string()),
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

async fn create_decision(
    mcp_server: &Arc<LibraMcpServer>,
    run_id: &str,
    chosen_patchset_id: Option<&str>,
    execution_plan: &ExecutionPlan,
    task_results: &[TaskResult],
    system_report: &SystemReport,
    decision: &DecisionOutcome,
) -> Result<String, OrchestratorError> {
    let decision_type = match decision {
        DecisionOutcome::Commit => "commit",
        DecisionOutcome::HumanReviewRequired => "checkpoint",
        DecisionOutcome::Abandon => "abandon",
    };
    let rationale = Some(format!(
        "{}; overall_passed={}; failed_tasks={}; checkpoints={}",
        execution_plan.summary,
        system_report.overall_passed,
        task_results
            .iter()
            .filter(|result| result.status == super::types::TaskNodeStatus::Failed)
            .count(),
        execution_plan.checkpoints.len()
    ));
    let checkpoint_id = (*decision == DecisionOutcome::HumanReviewRequired)
        .then(|| {
            execution_plan
                .checkpoints
                .last()
                .map(|checkpoint| checkpoint.label.clone())
        })
        .flatten();

    let params = CreateDecisionParams {
        run_id: run_id.to_string(),
        decision_type: decision_type.to_string(),
        chosen_patchset_id: chosen_patchset_id.map(ToString::to_string),
        result_commit_sha: None,
        checkpoint_id,
        rationale,
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

#[cfg(test)]
mod tests {
    use std::{path::Path, sync::Arc};

    use sea_orm::{ConnectionTrait, Database, Schema};
    use tempfile::tempdir;
    use uuid::Uuid;

    use super::*;
    use crate::{
        internal::{
            ai::{
                history::HistoryManager,
                orchestrator::types::{
                    ExecutionCheckpoint, GateReport, GateResult, TaskContract, TaskDAG, TaskNode,
                    TaskNodeStatus, ToolDiffRecord,
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

    #[tokio::test]
    async fn test_persist_execution_creates_object_chain() {
        let server = setup_server().await;
        let impl_task_id = Uuid::new_v4();
        let gate_task_id = Uuid::new_v4();
        let plan = ExecutionPlan {
            intent_spec_id: "intent-1".to_string(),
            summary: "Implement feature and verify it".to_string(),
            dag: TaskDAG {
                nodes: vec![
                    TaskNode {
                        id: impl_task_id,
                        title: "Edit source".to_string(),
                        objective: "Update src/lib.rs".to_string(),
                        description: None,
                        kind: TaskKind::Implementation,
                        gate_stage: None,
                        owner_role: Some("coder".to_string()),
                        dependencies: vec![],
                        constraints: vec![],
                        acceptance_criteria: vec![],
                        scope_in: vec!["src/".to_string()],
                        scope_out: vec![],
                        checks: vec![],
                        contract: TaskContract::default(),
                        status: TaskNodeStatus::Completed,
                    },
                    TaskNode {
                        id: gate_task_id,
                        title: "Run fast checks".to_string(),
                        objective: "Verify".to_string(),
                        description: None,
                        kind: TaskKind::Gate,
                        gate_stage: Some(GateStage::Fast),
                        owner_role: Some("verifier".to_string()),
                        dependencies: vec![impl_task_id],
                        constraints: vec![],
                        acceptance_criteria: vec![],
                        scope_in: vec![],
                        scope_out: vec![],
                        checks: vec![],
                        contract: TaskContract::default(),
                        status: TaskNodeStatus::Completed,
                    },
                ],
                intent_spec_id: "intent-1".to_string(),
                max_parallel: 1,
            },
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
            },
        ];
        let system_report = SystemReport {
            integration: GateReport::empty(),
            security: GateReport::empty(),
            release: GateReport::empty(),
            overall_passed: true,
        };

        let persisted = persist_execution(ExecutionPersistenceRequest {
            mcp_server: &server,
            execution_plan: &plan,
            task_results: &results,
            system_report: &system_report,
            decision: &DecisionOutcome::Commit,
            working_dir: Path::new("."),
            base_commit: Some(ZERO_COMMIT_SHA),
            model_name: "test-model",
        })
        .await
        .unwrap();

        assert!(!persisted.run_id.is_empty());
        assert!(persisted.provenance_id.is_some());
        assert!(persisted.decision_id.is_some());
        assert_eq!(persisted.tasks.len(), 2);
        assert_eq!(persisted.tasks[0].tool_invocation_ids.len(), 1);
        assert!(persisted.tasks[0].patchset_id.is_some());
        assert_eq!(persisted.tasks[1].evidence_ids.len(), 1);

        let history = server.intent_history_manager.as_ref().unwrap();
        assert_eq!(history.list_objects("run").await.unwrap().len(), 1);
        assert_eq!(history.list_objects("patchset").await.unwrap().len(), 1);
        assert_eq!(history.list_objects("evidence").await.unwrap().len(), 1);
        assert_eq!(history.list_objects("decision").await.unwrap().len(), 1);
        assert_eq!(history.list_objects("provenance").await.unwrap().len(), 1);
        assert_eq!(history.list_objects("invocation").await.unwrap().len(), 1);
    }
}
