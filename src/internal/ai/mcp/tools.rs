use git_internal::internal::object::{
    task::{GoalType, Task},
    types::ActorRef,
};
use rmcp::{handler::server::wrapper::Parameters, model::*, schemars, tool};
use uuid::Uuid;

use crate::{internal::ai::mcp::server::LibraMcpServer, utils::storage_ext::StorageExt};

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

impl LibraMcpServer {
    #[tool(description = "Create a new Task")]
    pub async fn create_task(
        &self,
        Parameters(params): Parameters<CreateTaskParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let storage = self
            .storage
            .as_ref()
            .ok_or_else(|| ErrorData::internal_error("Storage not available", None))?;

        let repo_id = Uuid::new_v4(); // TODO: Use actual repo ID
        let actor = ActorRef::mcp_client("mcp-user")
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

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

        // List "tasks" type objects
        // The object_type string used in HistoryManager.append("tasks", ...) must match.
        // Task::object_type() returns "task" (singular) from git_internal.
        // But HistoryManager typically uses plural for directories?
        // Wait, StorageExt::put_tracked uses `object.object_type()`.
        // git_internal::ObjectType::Task.to_string() -> "task".
        // So it should be "task".
        let objects = history
            .list_objects("task")
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let mut tasks_info = Vec::new();
        let limit = params.limit.unwrap_or(10);

        for (_id, hash) in objects.into_iter().take(limit) {
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
}
