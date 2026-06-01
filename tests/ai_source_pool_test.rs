//! CEX-14 source-pool contract tests.
//!
//! These tests keep the new source abstraction compatible with the existing MCP
//! bridge while enforcing trust-tier and per-session isolation rules.

use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use libra::internal::ai::{
    mcp::server::LibraMcpServer,
    sources::{
        BUILTIN_MCP_SOURCE_SLUG, CapabilityManifest, ManifestValidationError, McpSource, Source,
        SourceCallContext, SourceConfigOrigin, SourceEnablement, SourceKind, SourcePool,
        SourceToolCapability, SourceToolNaming, TrustTier, openapi_tool_capabilities_from_fixture,
        register_builtin_mcp_source_from_project_config, source_config_view_from_project_config,
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

/// A source whose `call_tool` records how many calls are in flight at once, so
/// a test can observe the effect of the CEX-S2-14 per-slug throttle on the real
/// handler dispatch path. Each call increments the in-flight gauge, holds it
/// briefly (so concurrent calls overlap), then decrements.
#[derive(Clone)]
struct ConcurrencyProbeSource {
    manifest: CapabilityManifest,
    current: Arc<AtomicUsize>,
    max_seen: Arc<AtomicUsize>,
}

impl ConcurrencyProbeSource {
    fn new(manifest: CapabilityManifest) -> Self {
        Self {
            manifest,
            current: Arc::new(AtomicUsize::new(0)),
            max_seen: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[async_trait]
impl Source for ConcurrencyProbeSource {
    fn manifest(&self) -> &CapabilityManifest {
        &self.manifest
    }

    async fn call_tool(
        &self,
        context: SourceCallContext,
        _invocation: ToolInvocation,
    ) -> ToolResult<ToolOutput> {
        let now = self.current.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_seen.fetch_max(now, Ordering::SeqCst);
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        self.current.fetch_sub(1, Ordering::SeqCst);
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

#[tokio::test]
async fn legacy_mcp_project_config_maps_to_enabled_builtin_source_view() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let libra_dir = temp_dir.path().join(".libra");
    fs::create_dir_all(&libra_dir).expect("create .libra");
    fs::write(
        libra_dir.join("config.toml"),
        r#"[mcp]
enabled = true
transport = "stdio"
"#,
    )
    .expect("write legacy mcp config");

    let view = source_config_view_from_project_config(temp_dir.path());
    let entry = view
        .source(BUILTIN_MCP_SOURCE_SLUG)
        .expect("legacy mcp config should map to builtin MCP source");
    assert_eq!(entry.slug, BUILTIN_MCP_SOURCE_SLUG);
    assert_eq!(entry.kind, SourceKind::Mcp);
    assert_eq!(entry.enablement, SourceEnablement::ProjectConfig);
    assert_eq!(entry.origin, SourceConfigOrigin::LegacyMcp);

    let server = Arc::new(LibraMcpServer::new(None, None));
    let expected_tool_count = McpSource::builtin(server.clone()).manifest().tools.len();
    let pool = SourcePool::new();
    let report = register_builtin_mcp_source_from_project_config(&pool, server, temp_dir.path())
        .expect("register builtin MCP source from legacy config");

    assert!(report.legacy_mcp_config_mapped);
    let statuses = pool.source_statuses().expect("list source statuses");
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].slug, BUILTIN_MCP_SOURCE_SLUG);
    assert_eq!(statuses[0].kind, SourceKind::Mcp);
    assert_eq!(statuses[0].enablement, SourceEnablement::ProjectConfig);
    assert_eq!(statuses[0].tool_count, expected_tool_count);

    let handlers = pool
        .tool_handlers_for_session("session-a", SourceToolNaming::Prefixed)
        .expect("legacy-config MCP source should remain enabled");
    let prefixed_name = source_prefixed_tool_name(BUILTIN_MCP_SOURCE_SLUG, "run_libra_vcs");
    assert!(
        handlers.iter().any(|(name, handler)| {
            name == &prefixed_name && handler.schema().function.name == prefixed_name
        }),
        "source-prefixed run_libra_vcs handler should be visible"
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

/// Drive `n_tasks` concurrent `handle()` calls at one source slug through a pool
/// built with the given per-slug `limit`, returning the peak observed in-flight
/// count. `limit == 0` disables throttling.
async fn observe_peak_concurrency(limit: usize, n_tasks: usize) -> usize {
    let manifest =
        CapabilityManifest::new("project_docs", SourceKind::LocalDocs, TrustTier::Project)
            .with_tool(read_tool("lookup"));
    let probe = Arc::new(ConcurrencyProbeSource::new(manifest));
    let pool = SourcePool::new().with_source_concurrency_limit(limit);
    pool.register_source(probe.clone())
        .expect("register probe source");

    let tool_name = source_prefixed_tool_name("project_docs", "lookup");
    let handler = pool
        .tool_handlers_for_session("session-a", SourceToolNaming::Prefixed)
        .expect("build handlers")
        .into_iter()
        .find(|(name, _)| name == &tool_name)
        .map(|(_, handler)| handler)
        .expect("probe handler must be built");

    let mut tasks = Vec::new();
    for _ in 0..n_tasks {
        let handler = handler.clone();
        let tool_name = tool_name.clone();
        tasks.push(tokio::spawn(async move {
            handler
                .handle(invocation(&tool_name))
                .await
                .expect("source call must succeed");
        }));
    }
    for task in tasks {
        task.await.expect("task must not panic");
    }

    assert_eq!(
        probe.current.load(Ordering::SeqCst),
        0,
        "every in-flight permit must be released after the calls complete",
    );
    probe.max_seen.load(Ordering::SeqCst)
}

/// CEX-S2-14 end-to-end: the per-slug limit configured on the pool is threaded
/// all the way into the handlers it builds and enforced on the real
/// `SourceToolHandler::handle` dispatch path — not just on the standalone
/// `SourceThrottle`. A disabled pool (`limit == 0`) is the control: it must
/// exceed the cap, proving the bound is the throttle and not test scheduling.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn source_pool_throttle_caps_concurrent_handler_dispatch_per_slug() {
    let throttled = observe_peak_concurrency(2, 12).await;
    assert!(
        throttled <= 2,
        "a pool built with limit 2 must cap per-slug handler concurrency at 2, saw {throttled}",
    );

    let unthrottled = observe_peak_concurrency(0, 12).await;
    assert!(
        unthrottled > 2,
        "a disabled throttle (limit 0) must allow more than 2 concurrent calls \
         (saw {unthrottled}); this confirms the cap above is the throttle, not scheduling",
    );
}
