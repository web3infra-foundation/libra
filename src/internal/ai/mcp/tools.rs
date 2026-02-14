//! MCP tools: create and list AI workflow process objects (Task/Run/Plan/...).
//!
//! This file uses `rmcp`'s `#[tool]` macro to expose `LibraMcpServer` methods as MCP tools.
//! Each tool's input schema is derived via `schemars::JsonSchema` for client discovery and validation.
//!
//! # Tool naming
//!
//! Tool names match the Rust method names (e.g. `create_task`, `list_runs`). Results are returned
//! as text via `CallToolResult`.
//!
//! # How tools and resources work together
//!
//! - `create_*` returns an object UUID (e.g. `"Task created with ID: ..."`).
//! - `list_*` is for quick browsing (some include title/status summaries).
//! - To fetch the full JSON payload, read the resource: `libra://object/{object_id}`.
//!
//! # object_type (history directory name)
//!
//! List tools call `HistoryManager::list_objects(object_type)` using the following types:
//! `task`, `run`, `snapshot`, `plan`, `patchset`, `evidence`, `invocation`, `provenance`, `decision`.
use git_internal::internal::object::{
    context::{ContextItem, ContextItemKind, ContextSnapshot, SelectionStrategy},
    decision::{Decision, DecisionType},
    evidence::{Evidence, EvidenceKind},
    patchset::{ApplyStatus, ChangeType, PatchSet, TouchedFile},
    plan::{Plan, PlanStatus, PlanStep},
    provenance::Provenance,
    run::{Run, RunStatus},
    task::{GoalType, Task},
    tool::{IoFootprint, ToolInvocation, ToolStatus},
    types::ActorRef,
};
use rmcp::{
    RoleServer, handler::server::wrapper::Parameters, model::*, schemars, service::RequestContext,
    tool,
};
use uuid::Uuid;

use crate::{internal::ai::mcp::server::LibraMcpServer, utils::storage_ext::StorageExt};

impl LibraMcpServer {
    /// Default actor for MCP tool calls. Extracted for testability.
    pub fn default_actor(&self) -> Result<ActorRef, ErrorData> {
        ActorRef::mcp_client("mcp-user").map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    fn get_actor(&self, _ctx: &RequestContext<RoleServer>) -> Result<ActorRef, ErrorData> {
        // TODO: Extract real user from context headers or init params
        self.default_actor()
    }
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CreateTaskParams {
    pub title: String,
    pub description: Option<String>,
    pub goal_type: Option<String>,
    pub constraints: Option<Vec<String>>,
    pub acceptance_criteria: Option<Vec<String>>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListTasksParams {
    pub limit: Option<usize>,
    pub status: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CreateRunParams {
    pub task_id: String,
    pub base_commit_sha: String,
    pub status: Option<String>,
    pub context_snapshot_id: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListRunsParams {
    pub limit: Option<usize>,
    pub status: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CreateContextSnapshotParams {
    pub base_commit_sha: String,
    pub selection_strategy: String,
    pub items: Option<Vec<ContextItemParams>>,
    pub summary: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ContextItemParams {
    pub path: String,
    pub content_hash: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListContextSnapshotsParams {
    pub limit: Option<usize>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CreatePlanParams {
    pub run_id: String,
    pub plan_version: Option<u32>,
    pub steps: Option<Vec<PlanStepParams>>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct PlanStepParams {
    pub intent: String,
    pub status: Option<String>,
    pub owner_role: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListPlansParams {
    pub limit: Option<usize>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CreatePatchSetParams {
    pub run_id: String,
    pub generation: u32,
    pub base_commit_sha: String,
    pub touched_files: Option<Vec<TouchedFileParams>>,
    pub rationale: Option<String>,
    pub apply_status: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct TouchedFileParams {
    pub path: String,
    pub change_type: String,
    pub lines_added: u32,
    pub lines_deleted: u32,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListPatchSetsParams {
    pub limit: Option<usize>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CreateEvidenceParams {
    pub run_id: String,
    pub patchset_id: Option<String>,
    pub kind: String,
    pub tool: String,
    pub command: Option<String>,
    pub exit_code: Option<i32>,
    pub summary: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListEvidencesParams {
    pub limit: Option<usize>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CreateToolInvocationParams {
    pub run_id: String,
    pub tool_name: String,
    pub status: Option<String>,
    pub args_json: Option<String>,
    pub io_footprint: Option<IoFootprintParams>,
    pub result_summary: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListToolInvocationsParams {
    pub limit: Option<usize>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct IoFootprintParams {
    pub paths_read: Option<Vec<String>>,
    pub paths_written: Option<Vec<String>>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CreateProvenanceParams {
    pub run_id: String,
    pub provider: String,
    pub model: String,
    pub parameters_json: Option<String>,
    pub token_usage_json: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListProvenancesParams {
    pub limit: Option<usize>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CreateDecisionParams {
    pub run_id: String,
    pub decision_type: String,
    pub chosen_patchset_id: Option<String>,
    pub checkpoint_id: Option<String>,
    pub rationale: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListDecisionsParams {
    pub limit: Option<usize>,
}

impl LibraMcpServer {
    #[tool(description = "Create a new Task")]
    pub async fn create_task(
        &self,
        ctx: RequestContext<RoleServer>,
        Parameters(params): Parameters<CreateTaskParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = self.get_actor(&ctx)?;
        self.create_task_impl(params, actor).await
    }

    /// Core implementation of create_task, callable without RequestContext for testing.
    pub async fn create_task_impl(
        &self,
        params: CreateTaskParams,
        actor: ActorRef,
    ) -> Result<CallToolResult, ErrorData> {
        let storage = self
            .storage
            .as_ref()
            .ok_or_else(|| ErrorData::internal_error("Storage not available", None))?;

        let repo_id = self.repo_id;

        let goal_type = if let Some(gt) = params.goal_type {
            use std::str::FromStr;
            Some(GoalType::from_str(&gt).map_err(|e| ErrorData::invalid_params(e, None))?)
        } else {
            None
        };

        let mut task = Task::new(repo_id, actor, params.title, goal_type)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        if let Some(desc) = params.description {
            task.set_description(Some(desc));
        }

        if let Some(constraints) = params.constraints {
            for c in constraints {
                task.add_constraint(c);
            }
        }

        if let Some(criteria) = params.acceptance_criteria {
            for c in criteria {
                task.add_acceptance_criterion(c);
            }
        }

        let hash = storage
            .put_json(&task)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        if let Some(history) = &self.history_manager {
            history
                .append(
                    &task.header().object_type().to_string(),
                    &task.header().object_id().to_string(),
                    hash,
                )
                .await
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        }

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Task created with ID: {}",
            task.header().object_id()
        ))]))
    }

    #[tool(description = "List recent tasks")]
    pub async fn list_tasks(
        &self,
        Parameters(params): Parameters<ListTasksParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let history = self
            .history_manager
            .as_ref()
            .ok_or_else(|| ErrorData::internal_error("History not available", None))?;
        let storage = self
            .storage
            .as_ref()
            .ok_or_else(|| ErrorData::internal_error("Storage not available", None))?;

        let objects = history
            .list_objects("task")
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let mut tasks_info = Vec::new();
        let limit = params.limit.unwrap_or(10);

        for (_id, hash) in objects.into_iter() {
            if tasks_info.len() >= limit {
                break;
            }
            // Read task from storage to get title/status
            if let Ok(task) = storage.get_json::<Task>(&hash).await {
                // Filter by status if requested
                if let Some(status_filter) = &params.status
                    && task.status().as_str() != status_filter
                {
                    continue;
                }

                tasks_info.push(format!(
                    "ID: {} | Title: {} | Status: {}",
                    task.header().object_id(),
                    task.title(),
                    task.status()
                ));
            }
        }

        if tasks_info.is_empty() {
            Ok(CallToolResult::success(vec![Content::text(
                "No tasks found.",
            )]))
        } else {
            Ok(CallToolResult::success(vec![Content::text(
                tasks_info.join("\n"),
            )]))
        }
    }

    #[tool(description = "Create a new Run")]
    pub async fn create_run(
        &self,
        ctx: RequestContext<RoleServer>,
        Parameters(params): Parameters<CreateRunParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let storage = self
            .storage
            .as_ref()
            .ok_or_else(|| ErrorData::internal_error("Storage not available", None))?;

        let repo_id = self.repo_id;
        let actor = self.get_actor(&ctx)?;
        let task_id = params
            .task_id
            .parse::<Uuid>()
            .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;

        let base_commit_sha =
            crate::internal::ai::util::normalize_commit_anchor(&params.base_commit_sha)
                .map_err(|e| ErrorData::invalid_params(e, None))?;

        let mut run = Run::new(repo_id, actor, task_id, &base_commit_sha)
            .map_err(|e| ErrorData::invalid_params(e, None))?;

        if let Some(s) = params.status {
            run.set_status(match s.as_str() {
                "created" => RunStatus::Created,
                "patching" => RunStatus::Patching,
                "validating" => RunStatus::Validating,
                "completed" => RunStatus::Completed,
                "failed" => RunStatus::Failed,
                _ => return Err(ErrorData::invalid_params("invalid run status", None)),
            });
        }
        if let Some(id) = params.context_snapshot_id {
            let parsed = id
                .parse::<Uuid>()
                .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
            run.set_context_snapshot_id(Some(parsed));
        }
        if let Some(err) = params.error {
            run.set_error(Some(err));
        }

        let hash = storage
            .put_json(&run)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        if let Some(history) = &self.history_manager {
            history
                .append(
                    &run.header().object_type().to_string(),
                    &run.header().object_id().to_string(),
                    hash,
                )
                .await
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        }

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Run created with ID: {}",
            run.header().object_id()
        ))]))
    }

    #[tool(description = "List recent runs")]
    pub async fn list_runs(
        &self,
        Parameters(params): Parameters<ListRunsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let history = self
            .history_manager
            .as_ref()
            .ok_or_else(|| ErrorData::internal_error("History not available", None))?;
        let storage = self
            .storage
            .as_ref()
            .ok_or_else(|| ErrorData::internal_error("Storage not available", None))?;

        let objects = history
            .list_objects("run")
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let mut out = Vec::new();
        let limit = params.limit.unwrap_or(10);
        for (_id, hash) in objects.into_iter() {
            if out.len() >= limit {
                break;
            }
            if let Ok(run) = storage.get_json::<Run>(&hash).await {
                if let Some(status_filter) = &params.status
                    && run.status().as_str() != status_filter
                {
                    continue;
                }
                out.push(format!(
                    "ID: {} | Task: {} | Status: {}",
                    run.header().object_id(),
                    run.task_id(),
                    run.status()
                ));
            }
        }

        if out.is_empty() {
            Ok(CallToolResult::success(vec![Content::text(
                "No runs found.",
            )]))
        } else {
            Ok(CallToolResult::success(vec![Content::text(out.join("\n"))]))
        }
    }

    #[tool(description = "Create a new ContextSnapshot")]
    pub async fn create_context_snapshot(
        &self,
        ctx: RequestContext<RoleServer>,
        Parameters(params): Parameters<CreateContextSnapshotParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let storage = self
            .storage
            .as_ref()
            .ok_or_else(|| ErrorData::internal_error("Storage not available", None))?;

        let repo_id = self.repo_id;
        let actor = self.get_actor(&ctx)?;

        let strategy = match params.selection_strategy.as_str() {
            "explicit" => SelectionStrategy::Explicit,
            "heuristic" => SelectionStrategy::Heuristic,
            _ => {
                return Err(ErrorData::invalid_params(
                    "invalid selection_strategy",
                    None,
                ));
            }
        };

        let base_commit_sha =
            crate::internal::ai::util::normalize_commit_anchor(&params.base_commit_sha)
                .map_err(|e| ErrorData::invalid_params(e, None))?;

        let mut snapshot = ContextSnapshot::new(repo_id, actor, &base_commit_sha, strategy)
            .map_err(|e| ErrorData::invalid_params(e, None))?;

        if let Some(items) = params.items {
            for item in items {
                let content_id = item
                    .content_hash
                    .parse()
                    .map_err(|e| ErrorData::invalid_params(e, None))?;
                let ctx_item = ContextItem::new(ContextItemKind::File, item.path, content_id)
                    .map_err(|e| ErrorData::invalid_params(e, None))?;
                snapshot.add_item(ctx_item);
            }
        }
        if let Some(summary) = params.summary {
            snapshot.set_summary(Some(summary));
        }

        let hash = storage
            .put_json(&snapshot)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        if let Some(history) = &self.history_manager {
            history
                .append(
                    &snapshot.header().object_type().to_string(),
                    &snapshot.header().object_id().to_string(),
                    hash,
                )
                .await
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        }

        Ok(CallToolResult::success(vec![Content::text(format!(
            "ContextSnapshot created with ID: {}",
            snapshot.header().object_id()
        ))]))
    }

    #[tool(description = "List recent context snapshots")]
    pub async fn list_context_snapshots(
        &self,
        Parameters(params): Parameters<ListContextSnapshotsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let history = self
            .history_manager
            .as_ref()
            .ok_or_else(|| ErrorData::internal_error("History not available", None))?;

        let objects = history
            .list_objects("snapshot")
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let limit = params.limit.unwrap_or(10);
        let out = objects
            .into_iter()
            .take(limit)
            .map(|(id, _)| id)
            .collect::<Vec<_>>()
            .join("\n");

        if out.is_empty() {
            Ok(CallToolResult::success(vec![Content::text(
                "No context snapshots found.",
            )]))
        } else {
            Ok(CallToolResult::success(vec![Content::text(out)]))
        }
    }

    #[tool(description = "Create a new Plan")]
    pub async fn create_plan(
        &self,
        ctx: RequestContext<RoleServer>,
        Parameters(params): Parameters<CreatePlanParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let storage = self
            .storage
            .as_ref()
            .ok_or_else(|| ErrorData::internal_error("Storage not available", None))?;

        let repo_id = self.repo_id;
        let actor = self.get_actor(&ctx)?;
        let run_id = params
            .run_id
            .parse::<Uuid>()
            .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;

        let mut plan = if let Some(version) = params.plan_version {
            if version <= 1 {
                Plan::new(repo_id, actor, run_id)
            } else {
                Plan::new_next(repo_id, actor, run_id, version - 1)
            }
        } else {
            Plan::new(repo_id, actor, run_id)
        }
        .map_err(|e| ErrorData::internal_error(e, None))?;

        if let Some(steps) = params.steps {
            for step in steps {
                let status = match step.status.as_deref().unwrap_or("pending") {
                    "pending" => PlanStatus::Pending,
                    "in_progress" => PlanStatus::InProgress,
                    "completed" => PlanStatus::Completed,
                    "failed" => PlanStatus::Failed,
                    "skipped" => PlanStatus::Skipped,
                    _ => return Err(ErrorData::invalid_params("invalid plan step status", None)),
                };
                plan.add_step(PlanStep {
                    intent: step.intent,
                    inputs: None,
                    outputs: None,
                    checks: None,
                    owner_role: step.owner_role,
                    status,
                });
            }
        }

        let hash = storage
            .put_json(&plan)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        if let Some(history) = &self.history_manager {
            history
                .append(
                    &plan.header().object_type().to_string(),
                    &plan.header().object_id().to_string(),
                    hash,
                )
                .await
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        }

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Plan created with ID: {}",
            plan.header().object_id()
        ))]))
    }

    #[tool(description = "List recent plans")]
    pub async fn list_plans(
        &self,
        Parameters(params): Parameters<ListPlansParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let history = self
            .history_manager
            .as_ref()
            .ok_or_else(|| ErrorData::internal_error("History not available", None))?;

        let objects = history
            .list_objects("plan")
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let limit = params.limit.unwrap_or(10);
        let out = objects
            .into_iter()
            .take(limit)
            .map(|(id, _)| id)
            .collect::<Vec<_>>()
            .join("\n");

        if out.is_empty() {
            Ok(CallToolResult::success(vec![Content::text(
                "No plans found.",
            )]))
        } else {
            Ok(CallToolResult::success(vec![Content::text(out)]))
        }
    }

    #[tool(description = "Create a new PatchSet")]
    pub async fn create_patchset(
        &self,
        ctx: RequestContext<RoleServer>,
        Parameters(params): Parameters<CreatePatchSetParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let storage = self
            .storage
            .as_ref()
            .ok_or_else(|| ErrorData::internal_error("Storage not available", None))?;

        let repo_id = self.repo_id;
        let actor = self.get_actor(&ctx)?;
        let run_id = params
            .run_id
            .parse::<Uuid>()
            .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;

        let base_commit_sha =
            crate::internal::ai::util::normalize_commit_anchor(&params.base_commit_sha)
                .map_err(|e| ErrorData::invalid_params(e, None))?;

        let mut patchset =
            PatchSet::new(repo_id, actor, run_id, &base_commit_sha, params.generation)
                .map_err(|e| ErrorData::invalid_params(e, None))?;

        if let Some(files) = params.touched_files {
            for f in files {
                let ct = match f.change_type.as_str() {
                    "add" => ChangeType::Add,
                    "modify" => ChangeType::Modify,
                    "delete" => ChangeType::Delete,
                    "rename" => ChangeType::Rename,
                    "copy" => ChangeType::Copy,
                    _ => return Err(ErrorData::invalid_params("invalid change_type", None)),
                };
                let touched = TouchedFile::new(f.path, ct, f.lines_added, f.lines_deleted)
                    .map_err(|e| ErrorData::invalid_params(e, None))?;
                patchset.add_touched_file(touched);
            }
        }
        patchset.set_rationale(params.rationale);
        if let Some(s) = params.apply_status {
            patchset.set_apply_status(match s.as_str() {
                "proposed" => ApplyStatus::Proposed,
                "applied" => ApplyStatus::Applied,
                "rejected" => ApplyStatus::Rejected,
                "superseded" => ApplyStatus::Superseded,
                _ => return Err(ErrorData::invalid_params("invalid apply_status", None)),
            });
        }

        let hash = storage
            .put_json(&patchset)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        if let Some(history) = &self.history_manager {
            history
                .append(
                    &patchset.header().object_type().to_string(),
                    &patchset.header().object_id().to_string(),
                    hash,
                )
                .await
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        }

        Ok(CallToolResult::success(vec![Content::text(format!(
            "PatchSet created with ID: {}",
            patchset.header().object_id()
        ))]))
    }

    #[tool(description = "List recent patchsets")]
    pub async fn list_patchsets(
        &self,
        Parameters(params): Parameters<ListPatchSetsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let history = self
            .history_manager
            .as_ref()
            .ok_or_else(|| ErrorData::internal_error("History not available", None))?;

        let objects = history
            .list_objects("patchset")
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let limit = params.limit.unwrap_or(10);
        let out = objects
            .into_iter()
            .take(limit)
            .map(|(id, _)| id)
            .collect::<Vec<_>>()
            .join("\n");

        if out.is_empty() {
            Ok(CallToolResult::success(vec![Content::text(
                "No patchsets found.",
            )]))
        } else {
            Ok(CallToolResult::success(vec![Content::text(out)]))
        }
    }

    #[tool(description = "Create a new Evidence")]
    pub async fn create_evidence(
        &self,
        ctx: RequestContext<RoleServer>,
        Parameters(params): Parameters<CreateEvidenceParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let storage = self
            .storage
            .as_ref()
            .ok_or_else(|| ErrorData::internal_error("Storage not available", None))?;

        let repo_id = self.repo_id;
        let actor = self.get_actor(&ctx)?;
        let run_id = params
            .run_id
            .parse::<Uuid>()
            .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;

        let kind = match params.kind.as_str() {
            "test" => EvidenceKind::Test,
            "lint" => EvidenceKind::Lint,
            "build" => EvidenceKind::Build,
            other => EvidenceKind::Other(other.to_string()),
        };

        let mut evidence = Evidence::new(repo_id, actor, run_id, kind, params.tool)
            .map_err(|e| ErrorData::internal_error(e, None))?;

        if let Some(id) = params.patchset_id {
            let parsed = id
                .parse::<Uuid>()
                .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
            evidence.set_patchset_id(Some(parsed));
        }
        evidence.set_command(params.command);
        evidence.set_exit_code(params.exit_code);
        evidence.set_summary(params.summary);

        let hash = storage
            .put_json(&evidence)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        if let Some(history) = &self.history_manager {
            history
                .append(
                    &evidence.header().object_type().to_string(),
                    &evidence.header().object_id().to_string(),
                    hash,
                )
                .await
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        }

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Evidence created with ID: {}",
            evidence.header().object_id()
        ))]))
    }

    #[tool(description = "List recent evidences")]
    pub async fn list_evidences(
        &self,
        Parameters(params): Parameters<ListEvidencesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let history = self
            .history_manager
            .as_ref()
            .ok_or_else(|| ErrorData::internal_error("History not available", None))?;

        let objects = history
            .list_objects("evidence")
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let limit = params.limit.unwrap_or(10);
        let out = objects
            .into_iter()
            .take(limit)
            .map(|(id, _)| id)
            .collect::<Vec<_>>()
            .join("\n");

        if out.is_empty() {
            Ok(CallToolResult::success(vec![Content::text(
                "No evidences found.",
            )]))
        } else {
            Ok(CallToolResult::success(vec![Content::text(out)]))
        }
    }

    #[tool(description = "Create a new ToolInvocation")]
    pub async fn create_tool_invocation(
        &self,
        ctx: RequestContext<RoleServer>,
        Parameters(params): Parameters<CreateToolInvocationParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let storage = self
            .storage
            .as_ref()
            .ok_or_else(|| ErrorData::internal_error("Storage not available", None))?;

        let repo_id = self.repo_id;
        let actor = self.get_actor(&ctx)?;
        let run_id = params
            .run_id
            .parse::<Uuid>()
            .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;

        let mut inv = ToolInvocation::new(repo_id, actor, run_id, params.tool_name)
            .map_err(|e| ErrorData::internal_error(e, None))?;
        if let Some(status) = params.status {
            inv.set_status(match status.as_str() {
                "ok" => ToolStatus::Ok,
                "error" => ToolStatus::Error,
                _ => return Err(ErrorData::invalid_params("invalid tool status", None)),
            });
        }
        if let Some(args_json) = params.args_json {
            let args = serde_json::from_str(&args_json)
                .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
            inv.set_args(args);
        }
        inv.set_io_footprint(params.io_footprint.map(|p| IoFootprint {
            paths_read: p.paths_read.unwrap_or_default(),
            paths_written: p.paths_written.unwrap_or_default(),
        }));
        inv.set_result_summary(params.result_summary);

        let hash = storage
            .put_json(&inv)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        if let Some(history) = &self.history_manager {
            history
                .append(
                    &inv.header().object_type().to_string(),
                    &inv.header().object_id().to_string(),
                    hash,
                )
                .await
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        }

        Ok(CallToolResult::success(vec![Content::text(format!(
            "ToolInvocation created with ID: {}",
            inv.header().object_id()
        ))]))
    }

    #[tool(description = "List recent tool invocations")]
    pub async fn list_tool_invocations(
        &self,
        Parameters(params): Parameters<ListToolInvocationsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let history = self
            .history_manager
            .as_ref()
            .ok_or_else(|| ErrorData::internal_error("History not available", None))?;

        let objects = history
            .list_objects("invocation")
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let limit = params.limit.unwrap_or(10);
        let out = objects
            .into_iter()
            .take(limit)
            .map(|(id, _)| id)
            .collect::<Vec<_>>()
            .join("\n");

        if out.is_empty() {
            Ok(CallToolResult::success(vec![Content::text(
                "No tool invocations found.",
            )]))
        } else {
            Ok(CallToolResult::success(vec![Content::text(out)]))
        }
    }

    #[tool(description = "Create a new Provenance")]
    pub async fn create_provenance(
        &self,
        ctx: RequestContext<RoleServer>,
        Parameters(params): Parameters<CreateProvenanceParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let storage = self
            .storage
            .as_ref()
            .ok_or_else(|| ErrorData::internal_error("Storage not available", None))?;

        let repo_id = self.repo_id;
        let actor = self.get_actor(&ctx)?;
        let run_id = params
            .run_id
            .parse::<Uuid>()
            .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;

        let mut prov = Provenance::new(repo_id, actor, run_id, params.provider, params.model)
            .map_err(|e| ErrorData::internal_error(e, None))?;
        if let Some(parameters_json) = params.parameters_json {
            let v = serde_json::from_str(&parameters_json)
                .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
            prov.set_parameters(Some(v));
        }
        if let Some(token_usage_json) = params.token_usage_json {
            let v = serde_json::from_str(&token_usage_json)
                .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
            prov.set_token_usage(Some(v));
        }

        let hash = storage
            .put_json(&prov)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        if let Some(history) = &self.history_manager {
            history
                .append(
                    &prov.header().object_type().to_string(),
                    &prov.header().object_id().to_string(),
                    hash,
                )
                .await
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        }

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Provenance created with ID: {}",
            prov.header().object_id()
        ))]))
    }

    #[tool(description = "List recent provenances")]
    pub async fn list_provenances(
        &self,
        Parameters(params): Parameters<ListProvenancesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let history = self
            .history_manager
            .as_ref()
            .ok_or_else(|| ErrorData::internal_error("History not available", None))?;

        let objects = history
            .list_objects("provenance")
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let limit = params.limit.unwrap_or(10);
        let out = objects
            .into_iter()
            .take(limit)
            .map(|(id, _)| id)
            .collect::<Vec<_>>()
            .join("\n");

        if out.is_empty() {
            Ok(CallToolResult::success(vec![Content::text(
                "No provenances found.",
            )]))
        } else {
            Ok(CallToolResult::success(vec![Content::text(out)]))
        }
    }

    #[tool(description = "Create a new Decision")]
    pub async fn create_decision(
        &self,
        ctx: RequestContext<RoleServer>,
        Parameters(params): Parameters<CreateDecisionParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let storage = self
            .storage
            .as_ref()
            .ok_or_else(|| ErrorData::internal_error("Storage not available", None))?;

        let repo_id = self.repo_id;
        let actor = self.get_actor(&ctx)?;
        let run_id = params
            .run_id
            .parse::<Uuid>()
            .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;

        let decision_type = match params.decision_type.as_str() {
            "commit" => DecisionType::Commit,
            "checkpoint" => DecisionType::Checkpoint,
            "abandon" => DecisionType::Abandon,
            "retry" => DecisionType::Retry,
            "rollback" => DecisionType::Rollback,
            other => DecisionType::Other(other.to_string()),
        };

        let mut decision = Decision::new(repo_id, actor, run_id, decision_type)
            .map_err(|e| ErrorData::internal_error(e, None))?;

        if let Some(id) = params.chosen_patchset_id {
            let parsed = id
                .parse::<Uuid>()
                .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
            decision.set_chosen_patchset_id(Some(parsed));
        }
        decision.set_checkpoint_id(params.checkpoint_id);
        decision.set_rationale(params.rationale);

        let hash = storage
            .put_json(&decision)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        if let Some(history) = &self.history_manager {
            history
                .append(
                    &decision.header().object_type().to_string(),
                    &decision.header().object_id().to_string(),
                    hash,
                )
                .await
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        }

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Decision created with ID: {}",
            decision.header().object_id()
        ))]))
    }

    #[tool(description = "List recent decisions")]
    pub async fn list_decisions(
        &self,
        Parameters(params): Parameters<ListDecisionsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let history = self
            .history_manager
            .as_ref()
            .ok_or_else(|| ErrorData::internal_error("History not available", None))?;

        let objects = history
            .list_objects("decision")
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let limit = params.limit.unwrap_or(10);
        let out = objects
            .into_iter()
            .take(limit)
            .map(|(id, _)| id)
            .collect::<Vec<_>>()
            .join("\n");

        if out.is_empty() {
            Ok(CallToolResult::success(vec![Content::text(
                "No decisions found.",
            )]))
        } else {
            Ok(CallToolResult::success(vec![Content::text(out)]))
        }
    }
}
