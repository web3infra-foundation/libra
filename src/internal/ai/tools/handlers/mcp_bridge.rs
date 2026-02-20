//! MCP bridge handler: exposes [`LibraMcpServer`] tools as [`ToolHandler`]
//! implementations so the TUI agent can call MCP workflow tools
//! (create_task, list_runs, …) without going through the HTTP transport.
//!
//! Each tool is wrapped in its own [`McpBridgeHandler`] instance which
//! deserialises JSON arguments, resolves the actor identity, and delegates
//! to the corresponding `_impl` method on [`LibraMcpServer`].

use std::sync::Arc;

use async_trait::async_trait;
use rmcp::model::CallToolResult;

use crate::internal::ai::{
    mcp::{resource::*, server::LibraMcpServer},
    tools::{
        context::{ToolInvocation, ToolKind, ToolOutput, ToolPayload},
        error::{ToolError, ToolResult},
        registry::ToolHandler,
        spec::{FunctionParameters, ToolSpec},
    },
};

/// Convert rmcp `CallToolResult` → TUI `ToolOutput`.
fn call_tool_result_to_output(result: CallToolResult) -> ToolOutput {
    let text: String = result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.as_str()))
        .collect::<Vec<_>>()
        .join("\n");

    if result.is_error.unwrap_or(false) {
        ToolOutput::failure(text)
    } else {
        ToolOutput::success(text)
    }
}

/// Convert rmcp `ErrorData` → TUI `ToolError`.
fn mcp_error_to_tool_error(e: rmcp::model::ErrorData) -> ToolError {
    ToolError::ExecutionFailed(format!("MCP error ({:?}): {}", e.code, e.message))
}

/// Parse JSON arguments string into a typed params struct.
fn parse_args<T: serde::de::DeserializeOwned>(arguments: &str) -> ToolResult<T> {
    serde_json::from_str(arguments)
        .map_err(|e| ToolError::ParseError(format!("Failed to parse MCP tool arguments: {e}")))
}

/// Helper to inline `definitions` into `properties` by resolving `$ref`.
/// This is needed because `FunctionParameters` does not support top-level definitions,
/// and `schemars` generates `$ref` for nested structs.
fn inline_definitions(mut root: serde_json::Value) -> serde_json::Value {
    let definitions = root
        .get("definitions")
        .cloned()
        .unwrap_or(serde_json::json!({}));

    fn recurse(val: &mut serde_json::Value, definitions: &serde_json::Value) {
        if let Some(obj) = val.as_object_mut() {
            // Check if this object is a $ref and resolve it
            if let Some(ref_str) = obj.get("$ref").and_then(|v| v.as_str())
                && let Some(def_name) = ref_str.strip_prefix("#/definitions/")
                && let Some(def) = definitions.get(def_name)
            {
                *val = def.clone();
                recurse(val, definitions);
                return;
            }
            for (_, v) in obj.iter_mut() {
                recurse(v, definitions);
            }
        } else if let Some(arr) = val.as_array_mut() {
            for v in arr.iter_mut() {
                recurse(v, definitions);
            }
        }
    }

    recurse(&mut root, &definitions);
    root
}

/// Derive `FunctionParameters` from a `schemars::JsonSchema` type so the LLM
/// receives the full parameter schema for the tool.
fn schema_to_params<T: rmcp::schemars::JsonSchema>() -> FunctionParameters {
    let root = rmcp::schemars::schema_for!(T);
    let mut schema_value = serde_json::to_value(root).unwrap_or_default();

    // Inline definitions to resolve $ref
    schema_value = inline_definitions(schema_value);

    let properties = schema_value
        .get("properties")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    let required = schema_value
        .get("required")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    FunctionParameters::Object {
        param_type: "object".to_string(),
        properties,
        required,
    }
}

// ---------------------------------------------------------------------------
// McpBridgeHandler
// ---------------------------------------------------------------------------

/// A [`ToolHandler`] that delegates to a single MCP tool on
/// [`LibraMcpServer`]. One instance is created per tool and registered in the
/// TUI's [`ToolRegistry`](crate::internal::ai::tools::ToolRegistry).
pub struct McpBridgeHandler {
    server: Arc<LibraMcpServer>,
    tool_name: String,
    description: String,
    params_schema: FunctionParameters,
}

impl McpBridgeHandler {
    fn new(
        server: Arc<LibraMcpServer>,
        tool_name: impl Into<String>,
        description: impl Into<String>,
        params_schema: FunctionParameters,
    ) -> Self {
        Self {
            server,
            tool_name: tool_name.into(),
            description: description.into(),
            params_schema,
        }
    }

    /// Build all MCP bridge handlers ready for bulk-registration in a
    /// [`ToolRegistryBuilder`](crate::internal::ai::tools::ToolRegistryBuilder).
    ///
    /// Returns `(tool_name, handler)` pairs.
    pub fn all_handlers(server: Arc<LibraMcpServer>) -> Vec<(String, Arc<dyn ToolHandler>)> {
        let defs: Vec<(&str, &str, FunctionParameters)> = vec![
            // ---- create tools ----
            (
                "create_intent",
                "Create a new Intent (Prompt/Goal)",
                schema_to_params::<CreateIntentParams>(),
            ),
            (
                "create_task",
                "Create a new Task for tracking an AI coding goal",
                schema_to_params::<CreateTaskParams>(),
            ),
            (
                "create_run",
                "Create a new Run (an execution attempt for a Task)",
                schema_to_params::<CreateRunParams>(),
            ),
            (
                "create_context_snapshot",
                "Create a new ContextSnapshot (files/state at a point in time)",
                schema_to_params::<CreateContextSnapshotParams>(),
            ),
            (
                "create_plan",
                "Create a new Plan (ordered steps for a Run)",
                schema_to_params::<CreatePlanParams>(),
            ),
            (
                "create_patchset",
                "Create a new PatchSet (code changes produced by a Run)",
                schema_to_params::<CreatePatchSetParams>(),
            ),
            (
                "create_evidence",
                "Create a new Evidence (test/lint/build results)",
                schema_to_params::<CreateEvidenceParams>(),
            ),
            (
                "create_tool_invocation",
                "Record a ToolInvocation (external tool call during a Run)",
                schema_to_params::<CreateToolInvocationParams>(),
            ),
            (
                "create_provenance",
                "Record Provenance (LLM provider/model metadata for a Run)",
                schema_to_params::<CreateProvenanceParams>(),
            ),
            (
                "create_decision",
                "Record a Decision (commit / checkpoint / abandon / retry)",
                schema_to_params::<CreateDecisionParams>(),
            ),
            // ---- update tools ----
            (
                "update_intent",
                "Update an existing Intent (set commit_sha or status)",
                schema_to_params::<UpdateIntentParams>(),
            ),
            // ---- list tools ----
            (
                "list_intents",
                "List recent intents",
                schema_to_params::<ListIntentsParams>(),
            ),
            (
                "list_tasks",
                "List recent tasks",
                schema_to_params::<ListTasksParams>(),
            ),
            (
                "list_runs",
                "List recent runs",
                schema_to_params::<ListRunsParams>(),
            ),
            (
                "list_context_snapshots",
                "List recent context snapshots",
                schema_to_params::<ListContextSnapshotsParams>(),
            ),
            (
                "list_plans",
                "List recent plans",
                schema_to_params::<ListPlansParams>(),
            ),
            (
                "list_patchsets",
                "List recent patchsets",
                schema_to_params::<ListPatchSetsParams>(),
            ),
            (
                "list_evidences",
                "List recent evidences",
                schema_to_params::<ListEvidencesParams>(),
            ),
            (
                "list_tool_invocations",
                "List recent tool invocations",
                schema_to_params::<ListToolInvocationsParams>(),
            ),
            (
                "list_provenances",
                "List recent provenances",
                schema_to_params::<ListProvenancesParams>(),
            ),
            (
                "list_decisions",
                "List recent decisions",
                schema_to_params::<ListDecisionsParams>(),
            ),
        ];

        defs.into_iter()
            .map(|(name, desc, params)| {
                let handler: Arc<dyn ToolHandler> =
                    Arc::new(Self::new(server.clone(), name, desc, params));
                (name.to_string(), handler)
            })
            .collect()
    }
}

#[async_trait]
impl ToolHandler for McpBridgeHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        self.tool_name.starts_with("create_") || self.tool_name.starts_with("update_")
    }

    async fn handle(&self, invocation: ToolInvocation) -> ToolResult<ToolOutput> {
        let arguments = match invocation.payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(ToolError::IncompatiblePayload(
                    "MCP bridge handler only accepts Function payloads".into(),
                ));
            }
        };

        let result: Result<CallToolResult, rmcp::model::ErrorData> = match self.tool_name.as_str() {
            // ---- create tools ----
            "create_intent" => {
                let params: CreateIntentParams = parse_args(&arguments)?;
                let actor = self
                    .server
                    .resolve_actor_from_params(
                        params.actor_kind.as_deref(),
                        params.actor_id.as_deref(),
                    )
                    .map_err(mcp_error_to_tool_error)?;
                self.server.create_intent_impl(params, actor).await
            }
            "create_task" => {
                let params: CreateTaskParams = parse_args(&arguments)?;
                let actor = self
                    .server
                    .resolve_actor_from_params(
                        params.actor_kind.as_deref(),
                        params.actor_id.as_deref(),
                    )
                    .map_err(mcp_error_to_tool_error)?;
                self.server.create_task_impl(params, actor).await
            }
            "create_run" => {
                let params: CreateRunParams = parse_args(&arguments)?;
                let actor = self
                    .server
                    .resolve_actor_from_params(
                        params.actor_kind.as_deref(),
                        params.actor_id.as_deref(),
                    )
                    .map_err(mcp_error_to_tool_error)?;
                self.server.create_run_impl(params, actor).await
            }
            "create_context_snapshot" => {
                let params: CreateContextSnapshotParams = parse_args(&arguments)?;
                let actor = self
                    .server
                    .resolve_actor_from_params(
                        params.actor_kind.as_deref(),
                        params.actor_id.as_deref(),
                    )
                    .map_err(mcp_error_to_tool_error)?;
                self.server
                    .create_context_snapshot_impl(params, actor)
                    .await
            }
            "create_plan" => {
                let params: CreatePlanParams = parse_args(&arguments)?;
                let actor = self
                    .server
                    .resolve_actor_from_params(
                        params.actor_kind.as_deref(),
                        params.actor_id.as_deref(),
                    )
                    .map_err(mcp_error_to_tool_error)?;
                self.server.create_plan_impl(params, actor).await
            }
            "create_patchset" => {
                let params: CreatePatchSetParams = parse_args(&arguments)?;
                let actor = self
                    .server
                    .resolve_actor_from_params(
                        params.actor_kind.as_deref(),
                        params.actor_id.as_deref(),
                    )
                    .map_err(mcp_error_to_tool_error)?;
                self.server.create_patchset_impl(params, actor).await
            }
            "create_evidence" => {
                let params: CreateEvidenceParams = parse_args(&arguments)?;
                let actor = self
                    .server
                    .resolve_actor_from_params(
                        params.actor_kind.as_deref(),
                        params.actor_id.as_deref(),
                    )
                    .map_err(mcp_error_to_tool_error)?;
                self.server.create_evidence_impl(params, actor).await
            }
            "create_tool_invocation" => {
                let params: CreateToolInvocationParams = parse_args(&arguments)?;
                let actor = self
                    .server
                    .resolve_actor_from_params(
                        params.actor_kind.as_deref(),
                        params.actor_id.as_deref(),
                    )
                    .map_err(mcp_error_to_tool_error)?;
                self.server.create_tool_invocation_impl(params, actor).await
            }
            "create_provenance" => {
                let params: CreateProvenanceParams = parse_args(&arguments)?;
                let actor = self
                    .server
                    .resolve_actor_from_params(
                        params.actor_kind.as_deref(),
                        params.actor_id.as_deref(),
                    )
                    .map_err(mcp_error_to_tool_error)?;
                self.server.create_provenance_impl(params, actor).await
            }
            "create_decision" => {
                let params: CreateDecisionParams = parse_args(&arguments)?;
                let actor = self
                    .server
                    .resolve_actor_from_params(
                        params.actor_kind.as_deref(),
                        params.actor_id.as_deref(),
                    )
                    .map_err(mcp_error_to_tool_error)?;
                self.server.create_decision_impl(params, actor).await
            }
            // ---- update tools ----
            "update_intent" => {
                let params: UpdateIntentParams = parse_args(&arguments)?;
                self.server.update_intent_impl(params).await
            }
            // ---- list tools ----
            "list_intents" => {
                let params: ListIntentsParams = parse_args(&arguments)?;
                self.server.list_intents_impl(params).await
            }
            "list_tasks" => {
                let params: ListTasksParams = parse_args(&arguments)?;
                self.server.list_tasks_impl(params).await
            }
            "list_runs" => {
                let params: ListRunsParams = parse_args(&arguments)?;
                self.server.list_runs_impl(params).await
            }
            "list_context_snapshots" => {
                let params: ListContextSnapshotsParams = parse_args(&arguments)?;
                self.server.list_context_snapshots_impl(params).await
            }
            "list_plans" => {
                let params: ListPlansParams = parse_args(&arguments)?;
                self.server.list_plans_impl(params).await
            }
            "list_patchsets" => {
                let params: ListPatchSetsParams = parse_args(&arguments)?;
                self.server.list_patchsets_impl(params).await
            }
            "list_evidences" => {
                let params: ListEvidencesParams = parse_args(&arguments)?;
                self.server.list_evidences_impl(params).await
            }
            "list_tool_invocations" => {
                let params: ListToolInvocationsParams = parse_args(&arguments)?;
                self.server.list_tool_invocations_impl(params).await
            }
            "list_provenances" => {
                let params: ListProvenancesParams = parse_args(&arguments)?;
                self.server.list_provenances_impl(params).await
            }
            "list_decisions" => {
                let params: ListDecisionsParams = parse_args(&arguments)?;
                self.server.list_decisions_impl(params).await
            }
            other => return Err(ToolError::ToolNotFound(other.to_string())),
        };

        result
            .map(call_tool_result_to_output)
            .map_err(mcp_error_to_tool_error)
    }

    fn schema(&self) -> ToolSpec {
        ToolSpec::new(&self.tool_name, &self.description)
            .with_parameters(self.params_schema.clone())
    }
}
