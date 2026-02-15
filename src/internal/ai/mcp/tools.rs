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
//!   All `create_*` tools accept optional `actor_kind` (`"human"`, `"agent"`, `"system"`,
//!   `"mcp_client"`) and `actor_id` parameters to identify the creator. When omitted, the
//!   actor is derived from the MCP client handshake or defaults to `mcp_client("mcp-user")`.
//! - `list_*` returns summaries with key fields (ID, status, title, etc.) for quick browsing.
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
    run::{AgentInstance, Run, RunStatus},
    task::{GoalType, Task, TaskStatus},
    tool::{IoFootprint, ToolInvocation, ToolStatus},
    types::{ActorKind, ActorRef},
};
use rmcp::{
    RoleServer,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars,
    service::RequestContext,
    tool, tool_router,
};
use uuid::Uuid;

use crate::{internal::ai::mcp::server::LibraMcpServer, utils::storage_ext::StorageExt};

impl LibraMcpServer {
    /// Default actor for MCP tool calls. Extracted for testability.
    pub fn default_actor(&self) -> Result<ActorRef, ErrorData> {
        ActorRef::mcp_client("mcp-user").map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    /// Resolve actor identity from explicit tool parameters only, without requiring
    /// a `RequestContext`. Falls back to `default_actor()` when no explicit params
    /// are provided.
    ///
    /// This is used by the TUI bridge handler where no MCP session exists.
    pub fn resolve_actor_from_params(
        &self,
        actor_kind: Option<&str>,
        actor_id: Option<&str>,
    ) -> Result<ActorRef, ErrorData> {
        if let Some(kind_str) = actor_kind {
            let id = actor_id.unwrap_or("unknown");
            let kind: ActorKind = kind_str.into();
            return ActorRef::new(kind, id).map_err(|e| ErrorData::invalid_params(e, None));
        }
        self.default_actor()
    }

    /// Resolve actor identity for a tool call.
    ///
    /// Priority:
    /// 1. Explicit `actor_kind` + `actor_id` from tool parameters (lets callers specify
    ///    human / agent / system / mcp_client).
    /// 2. MCP peer info from the initialization handshake (`McpClient` kind).
    /// 3. Fallback default `McpClient("mcp-user")`.
    fn resolve_actor(
        &self,
        ctx: &RequestContext<RoleServer>,
        actor_kind: Option<&str>,
        actor_id: Option<&str>,
    ) -> Result<ActorRef, ErrorData> {
        if let Some(kind_str) = actor_kind {
            let id = actor_id.unwrap_or("unknown");
            let kind: ActorKind = kind_str.into();
            return ActorRef::new(kind, id).map_err(|e| ErrorData::invalid_params(e, None));
        }
        // No explicit actor — derive from MCP peer info.
        if let Some(client_info) = ctx.peer.peer_info() {
            let client_name = &client_info.client_info.name;
            return ActorRef::mcp_client(client_name)
                .map_err(|e| ErrorData::internal_error(e.to_string(), None));
        }
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
    /// Actor who requested this task (kind: "human", "agent", etc.).
    pub requested_by_kind: Option<String>,
    /// Actor ID for the requester.
    pub requested_by_id: Option<String>,
    /// UUIDs of tasks this task depends on.
    pub dependencies: Option<Vec<String>>,
    /// Task status: "draft", "running", "done", "failed", "cancelled". Defaults to "draft".
    pub status: Option<String>,
    /// Actor kind: "human", "agent", "system", "mcp_client". Omit to auto-detect.
    pub actor_kind: Option<String>,
    /// Actor identifier (e.g. username, agent name). Required when `actor_kind` is set.
    pub actor_id: Option<String>,
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
    /// Agent instances participating in this run.
    pub agent_instances: Option<Vec<AgentInstanceParams>>,
    /// Arbitrary metrics JSON (e.g. token counts, timings).
    pub metrics_json: Option<String>,
    /// Actor kind: "human", "agent", "system", "mcp_client". Omit to auto-detect.
    pub actor_kind: Option<String>,
    /// Actor identifier (e.g. username, agent name). Required when `actor_kind` is set.
    pub actor_id: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AgentInstanceParams {
    pub role: String,
    pub provider_route: Option<String>,
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
    /// Actor kind: "human", "agent", "system", "mcp_client". Omit to auto-detect.
    pub actor_kind: Option<String>,
    /// Actor identifier (e.g. username, agent name). Required when `actor_kind` is set.
    pub actor_id: Option<String>,
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
    /// Actor kind: "human", "agent", "system", "mcp_client". Omit to auto-detect.
    pub actor_kind: Option<String>,
    /// Actor identifier (e.g. username, agent name). Required when `actor_kind` is set.
    pub actor_id: Option<String>,
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
    /// Actor kind: "human", "agent", "system", "mcp_client". Omit to auto-detect.
    pub actor_kind: Option<String>,
    /// Actor identifier (e.g. username, agent name). Required when `actor_kind` is set.
    pub actor_id: Option<String>,
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
    /// Actor kind: "human", "agent", "system", "mcp_client". Omit to auto-detect.
    pub actor_kind: Option<String>,
    /// Actor identifier (e.g. username, agent name). Required when `actor_kind` is set.
    pub actor_id: Option<String>,
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
    /// Actor kind: "human", "agent", "system", "mcp_client". Omit to auto-detect.
    pub actor_kind: Option<String>,
    /// Actor identifier (e.g. username, agent name). Required when `actor_kind` is set.
    pub actor_id: Option<String>,
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
    /// Actor kind: "human", "agent", "system", "mcp_client". Omit to auto-detect.
    pub actor_kind: Option<String>,
    /// Actor identifier (e.g. username, agent name). Required when `actor_kind` is set.
    pub actor_id: Option<String>,
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
    /// The commit SHA produced by this decision (64-hex or 40-hex SHA-1).
    pub result_commit_sha: Option<String>,
    pub checkpoint_id: Option<String>,
    pub rationale: Option<String>,
    /// Actor kind: "human", "agent", "system", "mcp_client". Omit to auto-detect.
    pub actor_kind: Option<String>,
    /// Actor identifier (e.g. username, agent name). Required when `actor_kind` is set.
    pub actor_id: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListDecisionsParams {
    pub limit: Option<usize>,
}

#[tool_router]
impl LibraMcpServer {
    #[tool(description = "Create a new Task")]
    pub async fn create_task(
        &self,
        ctx: RequestContext<RoleServer>,
        Parameters(params): Parameters<CreateTaskParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = self.resolve_actor(
            &ctx,
            params.actor_kind.as_deref(),
            params.actor_id.as_deref(),
        )?;
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

        // Set optional requested_by actor
        if let Some(rb_kind) = params.requested_by_kind {
            let rb_id = params.requested_by_id.as_deref().unwrap_or("unknown");
            let rb_actor_kind: ActorKind = rb_kind.as_str().into();
            let rb_actor = ActorRef::new(rb_actor_kind, rb_id)
                .map_err(|e| ErrorData::invalid_params(e, None))?;
            task.set_requested_by(Some(rb_actor));
        }

        // Add task dependencies
        if let Some(deps) = params.dependencies {
            for dep in deps {
                let dep_id = dep
                    .parse::<Uuid>()
                    .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
                task.add_dependency(dep_id);
            }
        }

        // Set task status if explicitly provided
        if let Some(s) = params.status {
            task.set_status(match s.as_str() {
                "draft" => TaskStatus::Draft,
                "running" => TaskStatus::Running,
                "done" => TaskStatus::Done,
                "failed" => TaskStatus::Failed,
                "cancelled" => TaskStatus::Cancelled,
                _ => return Err(ErrorData::invalid_params("invalid task status", None)),
            });
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
        self.list_tasks_impl(params).await
    }

    /// Core implementation of list_tasks, callable without rmcp Parameters wrapper.
    pub async fn list_tasks_impl(
        &self,
        params: ListTasksParams,
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
        let actor = self.resolve_actor(
            &ctx,
            params.actor_kind.as_deref(),
            params.actor_id.as_deref(),
        )?;
        self.create_run_impl(params, actor).await
    }

    /// Core implementation of create_run, callable without RequestContext.
    pub async fn create_run_impl(
        &self,
        params: CreateRunParams,
        actor: ActorRef,
    ) -> Result<CallToolResult, ErrorData> {
        let storage = self
            .storage
            .as_ref()
            .ok_or_else(|| ErrorData::internal_error("Storage not available", None))?;

        let repo_id = self.repo_id;
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

        // Add agent instances
        if let Some(instances) = params.agent_instances {
            for ai in instances {
                run.add_agent_instance(AgentInstance {
                    role: ai.role,
                    provider_route: ai.provider_route,
                });
            }
        }

        // Set metrics
        if let Some(metrics_json) = params.metrics_json {
            let v = serde_json::from_str(&metrics_json)
                .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
            run.set_metrics(Some(v));
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
        self.list_runs_impl(params).await
    }

    /// Core implementation of list_runs, callable without rmcp Parameters wrapper.
    pub async fn list_runs_impl(
        &self,
        params: ListRunsParams,
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
        let actor = self.resolve_actor(
            &ctx,
            params.actor_kind.as_deref(),
            params.actor_id.as_deref(),
        )?;
        self.create_context_snapshot_impl(params, actor).await
    }

    /// Core implementation of create_context_snapshot, callable without RequestContext.
    pub async fn create_context_snapshot_impl(
        &self,
        params: CreateContextSnapshotParams,
        actor: ActorRef,
    ) -> Result<CallToolResult, ErrorData> {
        let storage = self
            .storage
            .as_ref()
            .ok_or_else(|| ErrorData::internal_error("Storage not available", None))?;

        let repo_id = self.repo_id;

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
        self.list_context_snapshots_impl(params).await
    }

    /// Core implementation of list_context_snapshots, callable without rmcp Parameters wrapper.
    pub async fn list_context_snapshots_impl(
        &self,
        params: ListContextSnapshotsParams,
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
            .list_objects("snapshot")
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let limit = params.limit.unwrap_or(10);
        let mut out = Vec::new();
        for (_id, hash) in objects.into_iter() {
            if out.len() >= limit {
                break;
            }
            if let Ok(snap) = storage.get_json::<ContextSnapshot>(&hash).await {
                out.push(format!(
                    "ID: {} | Strategy: {:?} | Items: {} | Summary: {}",
                    snap.header().object_id(),
                    snap.selection_strategy(),
                    snap.items().len(),
                    snap.summary().unwrap_or("-"),
                ));
            }
        }

        if out.is_empty() {
            Ok(CallToolResult::success(vec![Content::text(
                "No context snapshots found.",
            )]))
        } else {
            Ok(CallToolResult::success(vec![Content::text(out.join("\n"))]))
        }
    }

    #[tool(description = "Create a new Plan")]
    pub async fn create_plan(
        &self,
        ctx: RequestContext<RoleServer>,
        Parameters(params): Parameters<CreatePlanParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = self.resolve_actor(
            &ctx,
            params.actor_kind.as_deref(),
            params.actor_id.as_deref(),
        )?;
        self.create_plan_impl(params, actor).await
    }

    /// Core implementation of create_plan, callable without RequestContext.
    pub async fn create_plan_impl(
        &self,
        params: CreatePlanParams,
        actor: ActorRef,
    ) -> Result<CallToolResult, ErrorData> {
        let storage = self
            .storage
            .as_ref()
            .ok_or_else(|| ErrorData::internal_error("Storage not available", None))?;

        let repo_id = self.repo_id;
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
        self.list_plans_impl(params).await
    }

    /// Core implementation of list_plans, callable without rmcp Parameters wrapper.
    pub async fn list_plans_impl(
        &self,
        params: ListPlansParams,
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
            .list_objects("plan")
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let limit = params.limit.unwrap_or(10);
        let mut out = Vec::new();
        for (_id, hash) in objects.into_iter() {
            if out.len() >= limit {
                break;
            }
            if let Ok(plan) = storage.get_json::<Plan>(&hash).await {
                out.push(format!(
                    "ID: {} | Run: {} | Version: {} | Steps: {}",
                    plan.header().object_id(),
                    plan.run_id(),
                    plan.plan_version(),
                    plan.steps().len(),
                ));
            }
        }

        if out.is_empty() {
            Ok(CallToolResult::success(vec![Content::text(
                "No plans found.",
            )]))
        } else {
            Ok(CallToolResult::success(vec![Content::text(out.join("\n"))]))
        }
    }

    #[tool(description = "Create a new PatchSet")]
    pub async fn create_patchset(
        &self,
        ctx: RequestContext<RoleServer>,
        Parameters(params): Parameters<CreatePatchSetParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = self.resolve_actor(
            &ctx,
            params.actor_kind.as_deref(),
            params.actor_id.as_deref(),
        )?;
        self.create_patchset_impl(params, actor).await
    }

    /// Core implementation of create_patchset, callable without RequestContext.
    pub async fn create_patchset_impl(
        &self,
        params: CreatePatchSetParams,
        actor: ActorRef,
    ) -> Result<CallToolResult, ErrorData> {
        let storage = self
            .storage
            .as_ref()
            .ok_or_else(|| ErrorData::internal_error("Storage not available", None))?;

        let repo_id = self.repo_id;
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
        self.list_patchsets_impl(params).await
    }

    /// Core implementation of list_patchsets, callable without rmcp Parameters wrapper.
    pub async fn list_patchsets_impl(
        &self,
        params: ListPatchSetsParams,
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
            .list_objects("patchset")
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let limit = params.limit.unwrap_or(10);
        let mut out = Vec::new();
        for (_id, hash) in objects.into_iter() {
            if out.len() >= limit {
                break;
            }
            if let Ok(ps) = storage.get_json::<PatchSet>(&hash).await {
                out.push(format!(
                    "ID: {} | Run: {} | Gen: {} | Files: {} | Status: {:?}",
                    ps.header().object_id(),
                    ps.run_id(),
                    ps.generation(),
                    ps.touched_files().len(),
                    ps.apply_status(),
                ));
            }
        }

        if out.is_empty() {
            Ok(CallToolResult::success(vec![Content::text(
                "No patchsets found.",
            )]))
        } else {
            Ok(CallToolResult::success(vec![Content::text(out.join("\n"))]))
        }
    }

    #[tool(description = "Create a new Evidence")]
    pub async fn create_evidence(
        &self,
        ctx: RequestContext<RoleServer>,
        Parameters(params): Parameters<CreateEvidenceParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = self.resolve_actor(
            &ctx,
            params.actor_kind.as_deref(),
            params.actor_id.as_deref(),
        )?;
        self.create_evidence_impl(params, actor).await
    }

    /// Core implementation of create_evidence, callable without RequestContext.
    pub async fn create_evidence_impl(
        &self,
        params: CreateEvidenceParams,
        actor: ActorRef,
    ) -> Result<CallToolResult, ErrorData> {
        let storage = self
            .storage
            .as_ref()
            .ok_or_else(|| ErrorData::internal_error("Storage not available", None))?;

        let repo_id = self.repo_id;
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
        self.list_evidences_impl(params).await
    }

    /// Core implementation of list_evidences, callable without rmcp Parameters wrapper.
    pub async fn list_evidences_impl(
        &self,
        params: ListEvidencesParams,
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
            .list_objects("evidence")
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let limit = params.limit.unwrap_or(10);
        let mut out = Vec::new();
        for (_id, hash) in objects.into_iter() {
            if out.len() >= limit {
                break;
            }
            if let Ok(ev) = storage.get_json::<Evidence>(&hash).await {
                out.push(format!(
                    "ID: {} | Kind: {:?} | Tool: {} | Exit: {} | Summary: {}",
                    ev.header().object_id(),
                    ev.kind(),
                    ev.tool(),
                    ev.exit_code().map_or("-".to_string(), |c| c.to_string()),
                    ev.summary().unwrap_or("-"),
                ));
            }
        }

        if out.is_empty() {
            Ok(CallToolResult::success(vec![Content::text(
                "No evidences found.",
            )]))
        } else {
            Ok(CallToolResult::success(vec![Content::text(out.join("\n"))]))
        }
    }

    #[tool(description = "Create a new ToolInvocation")]
    pub async fn create_tool_invocation(
        &self,
        ctx: RequestContext<RoleServer>,
        Parameters(params): Parameters<CreateToolInvocationParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = self.resolve_actor(
            &ctx,
            params.actor_kind.as_deref(),
            params.actor_id.as_deref(),
        )?;
        self.create_tool_invocation_impl(params, actor).await
    }

    /// Core implementation of create_tool_invocation, callable without RequestContext.
    pub async fn create_tool_invocation_impl(
        &self,
        params: CreateToolInvocationParams,
        actor: ActorRef,
    ) -> Result<CallToolResult, ErrorData> {
        let storage = self
            .storage
            .as_ref()
            .ok_or_else(|| ErrorData::internal_error("Storage not available", None))?;

        let repo_id = self.repo_id;
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
        self.list_tool_invocations_impl(params).await
    }

    /// Core implementation of list_tool_invocations, callable without rmcp Parameters wrapper.
    pub async fn list_tool_invocations_impl(
        &self,
        params: ListToolInvocationsParams,
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
            .list_objects("invocation")
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let limit = params.limit.unwrap_or(10);
        let mut out = Vec::new();
        for (_id, hash) in objects.into_iter() {
            if out.len() >= limit {
                break;
            }
            if let Ok(inv) = storage.get_json::<ToolInvocation>(&hash).await {
                out.push(format!(
                    "ID: {} | Tool: {} | Status: {:?} | Summary: {}",
                    inv.header().object_id(),
                    inv.tool_name(),
                    inv.status(),
                    inv.result_summary().unwrap_or("-"),
                ));
            }
        }

        if out.is_empty() {
            Ok(CallToolResult::success(vec![Content::text(
                "No tool invocations found.",
            )]))
        } else {
            Ok(CallToolResult::success(vec![Content::text(out.join("\n"))]))
        }
    }

    #[tool(description = "Create a new Provenance")]
    pub async fn create_provenance(
        &self,
        ctx: RequestContext<RoleServer>,
        Parameters(params): Parameters<CreateProvenanceParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = self.resolve_actor(
            &ctx,
            params.actor_kind.as_deref(),
            params.actor_id.as_deref(),
        )?;
        self.create_provenance_impl(params, actor).await
    }

    /// Core implementation of create_provenance, callable without RequestContext.
    pub async fn create_provenance_impl(
        &self,
        params: CreateProvenanceParams,
        actor: ActorRef,
    ) -> Result<CallToolResult, ErrorData> {
        let storage = self
            .storage
            .as_ref()
            .ok_or_else(|| ErrorData::internal_error("Storage not available", None))?;

        let repo_id = self.repo_id;
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
        self.list_provenances_impl(params).await
    }

    /// Core implementation of list_provenances, callable without rmcp Parameters wrapper.
    pub async fn list_provenances_impl(
        &self,
        params: ListProvenancesParams,
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
            .list_objects("provenance")
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let limit = params.limit.unwrap_or(10);
        let mut out = Vec::new();
        for (_id, hash) in objects.into_iter() {
            if out.len() >= limit {
                break;
            }
            if let Ok(prov) = storage.get_json::<Provenance>(&hash).await {
                out.push(format!(
                    "ID: {} | Provider: {} | Model: {}",
                    prov.header().object_id(),
                    prov.provider(),
                    prov.model(),
                ));
            }
        }

        if out.is_empty() {
            Ok(CallToolResult::success(vec![Content::text(
                "No provenances found.",
            )]))
        } else {
            Ok(CallToolResult::success(vec![Content::text(out.join("\n"))]))
        }
    }

    #[tool(description = "Create a new Decision")]
    pub async fn create_decision(
        &self,
        ctx: RequestContext<RoleServer>,
        Parameters(params): Parameters<CreateDecisionParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = self.resolve_actor(
            &ctx,
            params.actor_kind.as_deref(),
            params.actor_id.as_deref(),
        )?;
        self.create_decision_impl(params, actor).await
    }

    /// Core implementation of create_decision, callable without RequestContext.
    pub async fn create_decision_impl(
        &self,
        params: CreateDecisionParams,
        actor: ActorRef,
    ) -> Result<CallToolResult, ErrorData> {
        let storage = self
            .storage
            .as_ref()
            .ok_or_else(|| ErrorData::internal_error("Storage not available", None))?;

        let repo_id = self.repo_id;
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

        // Set result commit SHA if provided
        if let Some(sha) = params.result_commit_sha {
            let normalized = crate::internal::ai::util::normalize_commit_anchor(&sha)
                .map_err(|e| ErrorData::invalid_params(e, None))?;
            let hash_val = normalized
                .parse()
                .map_err(|e: String| ErrorData::invalid_params(e, None))?;
            decision.set_result_commit_sha(Some(hash_val));
        }

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
        self.list_decisions_impl(params).await
    }

    /// Core implementation of list_decisions, callable without rmcp Parameters wrapper.
    pub async fn list_decisions_impl(
        &self,
        params: ListDecisionsParams,
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
            .list_objects("decision")
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let limit = params.limit.unwrap_or(10);
        let mut out = Vec::new();
        for (_id, hash) in objects.into_iter() {
            if out.len() >= limit {
                break;
            }
            if let Ok(dec) = storage.get_json::<Decision>(&hash).await {
                out.push(format!(
                    "ID: {} | Type: {:?} | Rationale: {}",
                    dec.header().object_id(),
                    dec.decision_type(),
                    dec.rationale().unwrap_or("-"),
                ));
            }
        }

        if out.is_empty() {
            Ok(CallToolResult::success(vec![Content::text(
                "No decisions found.",
            )]))
        } else {
            Ok(CallToolResult::success(vec![Content::text(out.join("\n"))]))
        }
    }
}

impl LibraMcpServer {
    /// Public accessor for the tool router generated by `#[tool_router]`.
    ///
    /// The `#[tool_router]` macro generates a private `tool_router()` method on
    /// the impl block where the `#[tool]` methods live (this file). This wrapper
    /// re-exports it so `server.rs` (`new()`) can call it.
    pub(crate) fn build_tool_router() -> ToolRouter<Self> {
        Self::tool_router()
    }
}
