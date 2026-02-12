//! MCP `ServerHandler` implementation: resources (URI) and tool routing.
//!
//! - `LibraMcpServer` declares MCP capabilities (resources/tools) and implements resource reads.
//! - Tool implementations live in `crate::internal::ai::mcp::tools` and are registered via
//!   `rmcp`'s `#[tool_router]`.
//!
//! # Resource behavior (summary)
//!
//! - `libra://object/{object_id}`: resolve id -> hash in history, then read JSON blob from storage.
//! - `libra://objects/{object_type}`: list objects by type (one line: `{object_id} {object_hash}`).
//! - `libra://history/latest`: placeholder resource (future: expose history head commit hash).
//!
//! If `HistoryManager` or `Storage` is missing, related calls return `ErrorData`.
use std::sync::Arc;

use rmcp::{
    RoleServer, ServerHandler, handler::server::router::tool::ToolRouter, model::*,
    service::RequestContext, tool_handler, tool_router,
};

use crate::{internal::ai::history::HistoryManager, utils::storage::Storage};

#[derive(Clone)]
pub struct LibraMcpServer {
    pub history_manager: Option<Arc<HistoryManager>>,
    pub storage: Option<Arc<dyn Storage + Send + Sync>>,
    tool_router: ToolRouter<LibraMcpServer>,
}

#[tool_router]
impl LibraMcpServer {
    pub fn new(
        history_manager: Option<Arc<HistoryManager>>,
        storage: Option<Arc<dyn Storage + Send + Sync>>,
    ) -> Self {
        Self {
            history_manager,
            storage,
            tool_router: Self::tool_router(),
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
            // For now return a placeholder or HEAD hash if we can expose it
            return Ok(vec![ResourceContents::text("latest", "History Head")]);
        }

        if let Some(object_type) = uri.strip_prefix("libra://objects/") {
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
            if let Some(history) = &self.history_manager
                && let Some(storage) = &self.storage
                && let Ok(Some((hash, _type))) = history.find_object_hash(object_id_str).await
            {
                // Read object from storage
                // Storage::get returns (Vec<u8>, ObjectType).
                if let Ok((data, _)) = storage.get(&hash).await {
                    let json_str = String::from_utf8_lossy(&data).to_string();
                    return Ok(vec![ResourceContents::text(json_str, uri)]);
                }
            }
            return Err(ErrorData::resource_not_found(
                format!("Object not found: {}", object_id_str),
                None,
            ));
        }

        Err(ErrorData::resource_not_found("Resource not found", None))
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
