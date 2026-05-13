//! CEX-14 source-pool contract tests.
//!
//! These tests keep the new source abstraction compatible with the existing MCP
//! bridge while enforcing trust-tier and per-session isolation rules.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use libra::internal::ai::{
    mcp::server::LibraMcpServer,
    sources::{
        BUILTIN_MCP_SOURCE_SLUG, CapabilityManifest, ManifestValidationError, McpSource, Source,
        SourceCallContext, SourceEnablement, SourceKind, SourcePool, SourceToolCapability,
        SourceToolNaming, TrustTier, openapi_tool_capabilities_from_fixture,
        source_prefixed_tool_name,
    },
    tools::{
        context::{ToolInvocation, ToolKind, ToolOutput, ToolPayload},
        error::ToolResult,
        handlers::McpBridgeHandler,
        spec::ToolSpec,
    },
};

#[derive(Clone)]
struct FakeSource {
    manifest: CapabilityManifest,
    contexts: Arc<Mutex<Vec<SourceCallContext>>>,
}

impl FakeSource {
    fn new(manifest: CapabilityManifest) -> Self {
        Self {
            manifest,
            contexts: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn contexts(&self) -> Vec<SourceCallContext> {
        self.contexts.lock().expect("context lock").clone()
    }
}

#[async_trait]
impl Source for FakeSource {
    fn manifest(&self) -> &CapabilityManifest {
        &self.manifest
    }

    async fn call_tool(
        &self,
        context: SourceCallContext,
        _invocation: ToolInvocation,
    ) -> ToolResult<ToolOutput> {
        self.contexts
            .lock()
            .expect("context lock")
            .push(context.clone());
        Ok(ToolOutput::success(format!(
            "{}:{}",
            context.source_slug, context.tool_name
        )))
    }
}

fn read_tool(name: &str) -> SourceToolCapability {
    SourceToolCapability::new(name, ToolSpec::new(name, "Read test source data"))
}

fn mutating_tool_without_approval(name: &str) -> SourceToolCapability {
    let mut tool = SourceToolCapability::new(name, ToolSpec::new(name, "Mutate test source data"));
    tool.mutating = true;
    tool
}

fn invocation(tool_name: &str) -> ToolInvocation {
    ToolInvocation::new(
        "call-1",
        tool_name,
        ToolPayload::Function {
            arguments: "{}".to_string(),
        },
        PathBuf::from("/tmp"),
    )
}

#[test]
fn source_manifest_rejects_mutating_tools_without_approval_scope() {
    let manifest =
        CapabilityManifest::new("project_docs", SourceKind::LocalDocs, TrustTier::Project)
            .with_tool(mutating_tool_without_approval("rewrite_doc"));

    let err = manifest
        .validate()
        .expect_err("mutating source tool without approval scope must fail");

    assert!(matches!(
        err,
        ManifestValidationError::MissingApprovalScope { tool_name }
            if tool_name == "rewrite_doc"
    ));
}

#[tokio::test]
async fn third_party_sources_are_disabled_until_explicitly_enabled() {
    let manifest =
        CapabilityManifest::new("vendor_docs", SourceKind::RestApi, TrustTier::ThirdParty)
            .with_tool(read_tool("lookup"));
    let pool = SourcePool::new();

    pool.register_source(Arc::new(FakeSource::new(manifest)))
        .expect("register third-party source");

    let disabled_handlers = pool
        .tool_handlers_for_session("session-a", SourceToolNaming::Prefixed)
        .expect("build disabled source handlers");
    assert!(disabled_handlers.is_empty());

    pool.enable_source("vendor_docs", SourceEnablement::ProjectConfig)
        .expect("explicit project config should enable third-party source");

    let enabled_handlers = pool
        .tool_handlers_for_session("session-a", SourceToolNaming::Prefixed)
        .expect("build enabled source handlers");

    assert_eq!(enabled_handlers.len(), 1);
    assert_eq!(enabled_handlers[0].0, "vendor_docs__lookup");
    assert_eq!(
        enabled_handlers[0].1.schema().function.name,
        "vendor_docs__lookup"
    );
}

#[tokio::test]
async fn source_pool_records_calls_with_session_isolated_state_namespaces() {
    let manifest =
        CapabilityManifest::new("project_docs", SourceKind::LocalDocs, TrustTier::Project)
            .with_tool(read_tool("lookup"))
            .with_shared_state(false);
    let source = Arc::new(FakeSource::new(manifest));
    let pool = SourcePool::new();
    pool.register_source(source.clone())
        .expect("register project source");

    let session_a_handlers = pool
        .tool_handlers_for_session("session-a", SourceToolNaming::Prefixed)
        .expect("build session-a handlers");
    let session_b_handlers = pool
        .tool_handlers_for_session("session-b", SourceToolNaming::Prefixed)
        .expect("build session-b handlers");

    let tool_name = source_prefixed_tool_name("project_docs", "lookup");
    session_a_handlers[0]
        .1
        .handle(invocation(&tool_name))
        .await
        .expect("session-a source call");
    session_b_handlers[0]
        .1
        .handle(invocation(&tool_name))
        .await
        .expect("session-b source call");

    let contexts = source.contexts();
    assert_eq!(contexts.len(), 2);
    assert_ne!(contexts[0].state_namespace, contexts[1].state_namespace);
    assert_eq!(
        contexts[0].state_namespace,
        "session:session-a:project_docs"
    );
    assert_eq!(
        contexts[1].state_namespace,
        "session:session-b:project_docs"
    );

    let records = pool.recorded_calls().expect("read source call records");
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].source_slug, "project_docs");
    assert_eq!(records[0].tool_name, "lookup");
    assert_eq!(records[0].registered_tool_name, "project_docs__lookup");
    assert!(records[0].latency_ms.is_some());
    assert!(records[0].output_bytes > 0);
}

#[tokio::test]
async fn source_pool_lists_enablement_and_reload_affects_next_handler_build() {
    let initial =
        CapabilityManifest::new("project_docs", SourceKind::LocalDocs, TrustTier::Project)
            .with_tool(read_tool("lookup"));
    let pool = SourcePool::new();
    pool.register_source(Arc::new(FakeSource::new(initial)))
        .expect("register project source");

    let statuses = pool.source_statuses().expect("list source statuses");
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].slug, "project_docs");
    assert_eq!(statuses[0].enablement, SourceEnablement::ProjectConfig);
    assert!(statuses[0].enablement.is_enabled());
    assert_eq!(statuses[0].enablement.label(), "project_config");
    assert_eq!(statuses[0].tool_count, 1);

    pool.disable_source("project_docs")
        .expect("disable project source");
    assert!(
        pool.tool_handlers_for_session("session-a", SourceToolNaming::Prefixed)
            .expect("build disabled handlers")
            .is_empty()
    );
    pool.enable_source("project_docs", SourceEnablement::SessionExplicit)
        .expect("enable project source for this session");

    let reloaded =
        CapabilityManifest::new("project_docs", SourceKind::LocalDocs, TrustTier::Project)
            .with_tool(read_tool("lookup"))
            .with_tool(read_tool("search"));
    let status = pool
        .reload_source(Arc::new(FakeSource::new(reloaded)))
        .expect("reload project source");

    assert_eq!(status.enablement, SourceEnablement::SessionExplicit);
    assert_eq!(status.tool_count, 2);
    let handler_names = pool
        .tool_handlers_for_session("session-a", SourceToolNaming::Prefixed)
        .expect("build reloaded handlers")
        .into_iter()
        .map(|(name, _)| name)
        .collect::<Vec<_>>();
    assert_eq!(
        handler_names,
        vec!["project_docs__lookup", "project_docs__search"]
    );
}

#[tokio::test]
async fn mcp_source_keeps_legacy_bridge_names_and_schema_compatible() {
    let server = Arc::new(LibraMcpServer::new(None, None));
    let legacy_handlers = McpBridgeHandler::all_handlers(server.clone());
    let legacy_run = legacy_handlers
        .iter()
        .find_map(|(name, handler)| (name == "run_libra_vcs").then_some(handler))
        .expect("legacy run_libra_vcs handler");

    assert_eq!(legacy_run.kind(), ToolKind::Function);
    assert_eq!(legacy_run.schema().function.name, "run_libra_vcs");
    assert!(
        !legacy_run
            .is_mutating(&ToolInvocation::new(
                "call-1",
                "run_libra_vcs",
                ToolPayload::Function {
                    arguments: r#"{"command":"status","args":["--json"]}"#.to_string(),
                },
                PathBuf::from("/tmp"),
            ))
            .await
    );

    let pool = SourcePool::new();
    pool.register_source(Arc::new(McpSource::builtin(server)))
        .expect("register builtin MCP source");
    let source_handlers = pool
        .tool_handlers_for_session("session-a", SourceToolNaming::Prefixed)
        .expect("build MCP source handlers");
    let prefixed_name = source_prefixed_tool_name(BUILTIN_MCP_SOURCE_SLUG, "run_libra_vcs");
    let prefixed_run = source_handlers
        .iter()
        .find_map(|(name, handler)| (name == &prefixed_name).then_some(handler))
        .expect("prefixed run_libra_vcs handler");

    assert_eq!(prefixed_run.schema().function.name, prefixed_name);
    assert_eq!(
        serde_json::to_value(prefixed_run.schema().function.parameters).expect("prefixed schema"),
        serde_json::to_value(legacy_run.schema().function.parameters).expect("legacy schema")
    );
}

#[test]
fn openapi_fixture_generates_rest_tool_specs() {
    let fixture = r#"
    {
      "openapi": "3.1.0",
      "info": { "title": "Demo", "version": "1.0.0" },
      "paths": {
        "/repos/{owner}/{repo}": {
          "parameters": [
            {
              "name": "owner",
              "in": "path",
              "required": true,
              "schema": { "type": "string" }
            }
          ],
          "get": {
            "operationId": "getRepo",
            "summary": "Fetch a repository",
            "parameters": [
              {
                "name": "repo",
                "in": "path",
                "required": true,
                "schema": { "type": "string" }
              },
              {
                "name": "include_stats",
                "in": "query",
                "required": false,
                "schema": { "type": "boolean" }
              }
            ]
          }
        },
        "/issues": {
          "post": {
            "operationId": "create_issue",
            "requestBody": {
              "required": true,
              "content": {
                "application/json": {
                  "schema": {
                    "type": "object",
                    "required": ["title"],
                    "properties": {
                      "title": { "type": "string" }
                    }
                  }
                }
              }
            }
          }
        }
      }
    }
    "#;

    let capabilities =
        openapi_tool_capabilities_from_fixture(fixture).expect("OpenAPI fixture must parse");

    assert_eq!(
        capabilities
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        vec!["create_issue", "get_repo"]
    );
    assert!(capabilities.iter().all(|tool| tool.requires_network));

    let get_repo = capabilities
        .iter()
        .find(|tool| tool.name == "get_repo")
        .expect("getRepo operation should become get_repo");
    assert_eq!(get_repo.spec.function.description, "Fetch a repository");
    let serde_json::Value::Object(params) =
        serde_json::to_value(&get_repo.spec.function.parameters).expect("params json")
    else {
        panic!("tool parameters must serialize as an object");
    };
    assert_eq!(params["required"], serde_json::json!(["owner", "repo"]));
    assert_eq!(params["properties"]["include_stats"]["type"], "boolean");

    let create_issue = capabilities
        .iter()
        .find(|tool| tool.name == "create_issue")
        .expect("create issue operation should exist");
    let serde_json::Value::Object(params) =
        serde_json::to_value(&create_issue.spec.function.parameters).expect("params json")
    else {
        panic!("tool parameters must serialize as an object");
    };
    assert_eq!(params["required"], serde_json::json!(["body"]));
    assert_eq!(
        params["properties"]["body"]["required"],
        serde_json::json!(["title"])
    );
}
