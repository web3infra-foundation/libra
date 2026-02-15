//! MCP `ServerHandler` implementation: resources (URI) and tool routing.
//!
//! - `LibraMcpServer` declares MCP capabilities (resources/tools) and implements resource reads.
//! - Tool implementations live in `crate::internal::ai::mcp::tools` and are registered via
//!   `rmcp`'s `#[tool_router]`.
//!
//! # Resource behavior (summary)
//!
//! - `libra://object/{object_id}`: resolve id -> hash in history (both main and intent history), then read JSON blob from storage.
//! - `libra://objects/{object_type}`: list objects by type (one line: `{object_id} {object_hash}`).
//!   - For `intent` type, it queries the dedicated intent history (`refs/libra/intent`).
//!   - For other types, it queries the main code history (`refs/libra/history`).
//! - `libra://history/latest`: returns the current history orphan-branch HEAD commit hash.
//! - `libra://context/active`: returns the latest active Run/Task/ContextSnapshot as JSON.
//!
//! If `HistoryManager` or `Storage` is missing, related calls return `ErrorData`.
use std::sync::Arc;

use rmcp::{
    RoleServer, ServerHandler, handler::server::router::tool::ToolRouter, model::*,
    service::RequestContext, tool_handler,
};
use uuid::Uuid;

use crate::{
    internal::ai::history::HistoryManager,
    utils::{storage::Storage, storage_ext::StorageExt},
};

#[derive(Clone)]
pub struct LibraMcpServer {
    pub history_manager: Option<Arc<HistoryManager>>,
    pub intent_history_manager: Option<Arc<HistoryManager>>,
    pub storage: Option<Arc<dyn Storage + Send + Sync>>,
    pub repo_id: Uuid,
    tool_router: ToolRouter<LibraMcpServer>,
}

impl LibraMcpServer {
    pub fn new(
        history_manager: Option<Arc<HistoryManager>>,
        intent_history_manager: Option<Arc<HistoryManager>>,
        storage: Option<Arc<dyn Storage + Send + Sync>>,
        repo_id: Uuid,
    ) -> Self {
        Self {
            history_manager,
            intent_history_manager,
            storage,
            repo_id,
            tool_router: Self::build_tool_router(),
        }
    }
}

impl LibraMcpServer {
    pub async fn list_resources_impl(&self) -> Result<Vec<Annotated<RawResource>>, ErrorData> {
        Ok(vec![
            RawResource::new("libra://history/latest", "Latest History Head").no_annotation(),
            RawResource::new("libra://context/active", "Active Context").no_annotation(),
        ])
    }

    pub async fn read_resource_impl(&self, uri: &str) -> Result<Vec<ResourceContents>, ErrorData> {
        if uri == "libra://history/latest" {
            let history = self
                .history_manager
                .as_ref()
                .ok_or_else(|| ErrorData::internal_error("History not available", None))?;
            let head = history
                .resolve_history_head()
                .await
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
            let text = match head {
                Some(hash) => hash.to_string(),
                None => "no history".to_string(),
            };
            return Ok(vec![ResourceContents::text(text, uri)]);
        }

        if uri == "libra://context/active" {
            return self.read_active_context().await;
        }

        if let Some(object_type) = uri.strip_prefix("libra://objects/") {
            if object_type == "intent" {
                let history = self
                    .intent_history_manager
                    .as_ref()
                    .ok_or_else(|| ErrorData::internal_error("Intent history not available", None))?;
                let objects = history
                    .list_objects(object_type)
                    .await
                    .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
                let body = objects
                    .into_iter()
                    .map(|(id, hash)| format!("{} {}", id, hash))
                    .collect::<Vec<_>>()
                    .join("\n");
                return Ok(vec![ResourceContents::text(body, uri)]);
            }

            let history = self
                .history_manager
                .as_ref()
                .ok_or_else(|| ErrorData::internal_error("History not available", None))?;
            let objects = history
                .list_objects(object_type)
                .await
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
            let body = objects
                .into_iter()
                .map(|(id, hash)| format!("{} {}", id, hash))
                .collect::<Vec<_>>()
                .join("\n");
            return Ok(vec![ResourceContents::text(body, uri)]);
        }

        if let Some(object_id_str) = uri.strip_prefix("libra://object/") {
            let history = self
                .history_manager
                .as_ref()
                .ok_or_else(|| ErrorData::internal_error("History not available", None))?;
            let storage = self
                .storage
                .as_ref()
                .ok_or_else(|| ErrorData::internal_error("Storage not available", None))?;

            let result = history
                .find_object_hash(object_id_str)
                .await
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

            // If not found in main history, try intent history
            let result = match result {
                Some(res) => Some(res),
                None => {
                    if let Some(intent_history) = &self.intent_history_manager {
                         intent_history
                            .find_object_hash(object_id_str)
                            .await
                            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?
                    } else {
                        None
                    }
                }
            };

            match result {
                Some((hash, _type)) => {
                    let (data, _) = storage
                        .get(&hash)
                        .await
                        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
                    let json_str = String::from_utf8_lossy(&data).to_string();
                    return Ok(vec![ResourceContents::text(json_str, uri)]);
                }
                None => {
                    return Err(ErrorData::resource_not_found(
                        format!("Object not found: {}", object_id_str),
                        None,
                    ));
                }
            }
        }

        Err(ErrorData::resource_not_found("Resource not found", None))
    }

    /// Build the `libra://context/active` resource by finding the latest
    /// non-terminal Run, then loading its parent Task and linked ContextSnapshot.
    ///
    /// Returns a JSON object with `task`, `run`, and optionally `context_snapshot` fields.
    /// If no active run is found, falls back to the latest non-terminal Task.
    /// If nothing is active, returns `{"active": false}`.
    async fn read_active_context(&self) -> Result<Vec<ResourceContents>, ErrorData> {
        use git_internal::internal::object::{
            context::ContextSnapshot,
            run::{Run, RunStatus},
            task::Task,
        };

        let history = self
            .history_manager
            .as_ref()
            .ok_or_else(|| ErrorData::internal_error("History not available", None))?;
        let storage = self
            .storage
            .as_ref()
            .ok_or_else(|| ErrorData::internal_error("Storage not available", None))?;

        let uri = "libra://context/active";

        // 1. Find the latest active Run (UUID v7 is lexicographically time-ordered)
        let runs = history
            .list_objects("run")
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let mut active_run: Option<Run> = None;
        // Iterate in reverse so the latest (by UUID sort) is checked first
        for (_id, hash) in runs.into_iter().rev() {
            if let Ok(run) = storage.get_json::<Run>(&hash).await {
                match run.status() {
                    RunStatus::Completed | RunStatus::Failed => continue,
                    _ => {
                        active_run = Some(run);
                        break;
                    }
                }
            }
        }

        let mut result = serde_json::Map::new();

        if let Some(run) = &active_run {
            // Serialize run info
            let run_obj = serde_json::json!({
                "id": run.header().object_id().to_string(),
                "status": run.status().as_str(),
                "task_id": run.task_id().to_string(),
                "base_commit_sha": run.base_commit_sha().to_string(),
            });
            result.insert("run".to_string(), run_obj);

            // Load parent Task
            let task_hash = history
                .get_object_hash("task", &run.task_id().to_string())
                .await
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
            if let Some(hash) = task_hash
                && let Ok(task) = storage.get_json::<Task>(&hash).await
            {
                let task_obj = serde_json::json!({
                    "id": task.header().object_id().to_string(),
                    "title": task.title(),
                    "status": task.status().as_str(),
                    "goal_type": task.goal_type().map(|g| g.to_string()),
                    "constraints": task.constraints(),
                    "acceptance_criteria": task.acceptance_criteria(),
                });
                result.insert("task".to_string(), task_obj);
            }

            // Load linked ContextSnapshot if present
            if let Some(snapshot_id) = run.context_snapshot_id() {
                let snap_hash = history
                    .get_object_hash("snapshot", &snapshot_id.to_string())
                    .await
                    .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
                if let Some(hash) = snap_hash
                    && let Ok(snapshot) = storage.get_json::<ContextSnapshot>(&hash).await
                {
                    let items: Vec<serde_json::Value> = snapshot
                        .items()
                        .iter()
                        .map(|item| {
                            serde_json::json!({
                                "kind": format!("{:?}", item.kind),
                                "path": item.path,
                                "content_id": item.content_id.to_string(),
                            })
                        })
                        .collect();
                    let snap_obj = serde_json::json!({
                        "id": snapshot.header().object_id().to_string(),
                        "base_commit_sha": snapshot.base_commit_sha().to_string(),
                        "selection_strategy": format!("{:?}", snapshot.selection_strategy()),
                        "items": items,
                        "summary": snapshot.summary(),
                    });
                    result.insert("context_snapshot".to_string(), snap_obj);
                }
            }

            result.insert("active".to_string(), serde_json::Value::Bool(true));
        } else {
            // No active run — try to find the latest non-terminal Task as fallback
            let tasks = history
                .list_objects("task")
                .await
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

            let mut found_task = false;
            for (_id, hash) in tasks.into_iter().rev() {
                if let Ok(task) = storage.get_json::<Task>(&hash).await {
                    use git_internal::internal::object::task::TaskStatus;
                    match task.status() {
                        TaskStatus::Done | TaskStatus::Failed | TaskStatus::Cancelled => continue,
                        _ => {}
                    }
                    let task_obj = serde_json::json!({
                        "id": task.header().object_id().to_string(),
                        "title": task.title(),
                        "status": task.status().as_str(),
                        "goal_type": task.goal_type().map(|g| g.to_string()),
                        "constraints": task.constraints(),
                        "acceptance_criteria": task.acceptance_criteria(),
                    });
                    result.insert("task".to_string(), task_obj);
                    result.insert("active".to_string(), serde_json::Value::Bool(true));
                    found_task = true;
                    break;
                }
            }

            if !found_task {
                result.insert("active".to_string(), serde_json::Value::Bool(false));
            }
        }

        let json = serde_json::to_string(&result)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(vec![ResourceContents::text(json, uri)])
    }
}

#[tool_handler]
impl ServerHandler for LibraMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder()
                .enable_resources()
                .enable_tools()
                .build(),
            server_info: Implementation {
                name: "libra".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                ..Default::default()
            },
            instructions: Some("Libra MCP Server provides access to AI workflow objects (Task, Run, Plan) and version control history.".to_string()),
        }
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParam>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        let resources = self.list_resources_impl().await?;
        Ok(ListResourcesResult {
            resources,
            next_cursor: None,
            meta: None,
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParam,
        _: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        let contents = self.read_resource_impl(&request.uri).await?;
        Ok(ReadResourceResult { contents })
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParam>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        Ok(ListResourceTemplatesResult {
            resource_templates: vec![
                ResourceTemplate::new(
                    RawResourceTemplate {
                        uri_template: "libra://object/{object_id}".to_string(),
                        name: "Get AI Object by ID".to_string(),
                        description: None,
                        mime_type: None,
                        title: None,
                        icons: None,
                    },
                    None,
                ),
                ResourceTemplate::new(
                    RawResourceTemplate {
                        uri_template: "libra://objects/{object_type}".to_string(),
                        name: "List AI Objects by Type".to_string(),
                        description: None,
                        mime_type: None,
                        title: None,
                        icons: None,
                    },
                    None,
                ),
            ],
            next_cursor: None,
            meta: None,
        })
    }
}
