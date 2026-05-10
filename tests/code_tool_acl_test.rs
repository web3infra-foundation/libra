//! Wave 8 / PR 8 — Tool registry ACL coverage for the
//! `libra code` `--context` modes (§5.9 first bullet).
//!
//! The TUI runtime registers ~12 first-party tools plus the
//! semantic / MCP bridge sets (see `src/command/code.rs`). The
//! `--context` flag maps to a `TaskIntent` (Dev → Feature,
//! Review → Review, Research → Question), and
//! `ToolRegistry::filter_by_intent` is the single point of truth
//! for which tools the agent is allowed to invoke. These tests
//! pin the TUI tool set against the filter so a future
//! intent-mapping or registry change can't silently expose
//! mutating tools to a Review/Research session.
//!
//! `tool ACL × --approval-policy` and `--network-access deny`
//! tracking listed in §5.9 are L2 follow-ups — the existing
//! approval matrix (Wave 6) already pins one approval-policy
//! path end-to-end, and the network-deny case touches the
//! sandbox runtime context which is out of scope for an L1 ACL
//! test.

use std::sync::Arc;

use libra::internal::ai::{
    agent::TaskIntent,
    mcp::server::LibraMcpServer,
    tools::{
        ToolRegistry, ToolRegistryBuilder,
        handlers::{
            ApplyPatchHandler, GrepFilesHandler, ListDirHandler, McpBridgeHandler, PlanHandler,
            ReadFileHandler, SearchFilesHandler, ShellHandler, SubmitIntentDraftHandler,
            SubmitPlanDraftHandler, SubmitTaskCompleteHandler, WebSearchHandler,
            register_semantic_handlers,
        },
    },
};

/// Build a `ToolRegistry` that mirrors what `src/command/code.rs`
/// registers for the TUI agent at startup. The set kept in sync
/// with the live registration block in `src/command/code.rs:1438-
/// 1462` (Wave 8 / PR 8 captured this with commit 70619aef);
/// when adding a new tool there, mirror the registration here so
/// the ACL contract is exercised against the full live surface.
///
/// Excluded from this helper:
///   * `RequestUserInputHandler` — needs a runtime `mpsc::Sender`
///     and is irrelevant to ACL filtering because its name is on
///     the read-only-or-semantic allowlist regardless.
///
/// MCP bridge handlers ARE included via
/// `McpBridgeHandler::all_handlers(...)`. Their names are
/// dynamic (driven off the `LibraMcpServer` manifest) so the
/// Review/Research assertions intentionally pin only the names
/// `is_read_only_or_semantic_tool` lists, not the whole MCP
/// surface — Codex pass-1 P2 follow-up calls this out so a
/// future MCP-bridge tool that names itself `delete_*` doesn't
/// silently slip into a Review session.
fn build_tui_like_registry() -> Arc<ToolRegistry> {
    let dir = tempfile::tempdir().expect("tempdir for ACL test");
    let mut builder = ToolRegistryBuilder::with_working_dir(dir.path().to_path_buf())
        .register("read_file", Arc::new(ReadFileHandler))
        .register("list_dir", Arc::new(ListDirHandler))
        .register("grep_files", Arc::new(GrepFilesHandler))
        .register("search_files", Arc::new(SearchFilesHandler))
        .register("web_search", Arc::new(WebSearchHandler))
        .register("apply_patch", Arc::new(ApplyPatchHandler))
        .register("shell", Arc::new(ShellHandler))
        .register("update_plan", Arc::new(PlanHandler))
        .register("submit_intent_draft", Arc::new(SubmitIntentDraftHandler))
        .register("submit_plan_draft", Arc::new(SubmitPlanDraftHandler))
        .register("submit_task_complete", Arc::new(SubmitTaskCompleteHandler));
    builder = register_semantic_handlers(builder);
    let mcp_server = Arc::new(LibraMcpServer::new(None, None));
    for (name, handler) in McpBridgeHandler::all_handlers(mcp_server) {
        builder = builder.register(name, handler);
    }
    Arc::new(builder.build())
}

/// `Dev` → `TaskIntent::Feature` lets every registered tool through
/// — including the mutating `apply_patch` / `shell` /
/// `submit_*_draft` set — so the agent can drive the full
/// implementation workflow. Pinning this contract guards against
/// a future intent-filter regression that would silently drop a
/// dev-mode tool.
#[test]
fn dev_context_filter_keeps_all_registered_tools() {
    let registry = build_tui_like_registry();
    let allowed = registry.filter_by_intent(TaskIntent::Feature);

    for required in [
        "read_file",
        "list_dir",
        "grep_files",
        "search_files",
        "web_search",
        "apply_patch",
        "shell",
        "update_plan",
        "submit_intent_draft",
        "submit_plan_draft",
        "submit_task_complete",
    ] {
        assert!(
            allowed.iter().any(|name| name == required),
            "Dev/Feature filter dropped '{required}'; allowed: {allowed:?}",
        );
    }
}

/// `Review` → `TaskIntent::Review` MUST exclude any tool that can
/// mutate the workspace or shell out, because a review-context
/// session is supposed to inspect, not change. This test pins the
/// exclusion list so a regression that flipped `apply_patch` or
/// `shell` into the read-only allowlist would fail loud.
#[test]
fn review_context_filter_drops_mutating_tools() {
    let registry = build_tui_like_registry();
    let allowed = registry.filter_by_intent(TaskIntent::Review);

    for forbidden in [
        "apply_patch",
        "shell",
        "submit_intent_draft",
        "submit_plan_draft",
        "submit_task_complete",
        "update_plan",
    ] {
        assert!(
            !allowed.iter().any(|name| name == forbidden),
            "Review filter must drop '{forbidden}', but allowed: {allowed:?}",
        );
    }

    for required in [
        "read_file",
        "list_dir",
        "grep_files",
        "search_files",
        "web_search",
    ] {
        assert!(
            allowed.iter().any(|name| name == required),
            "Review filter dropped read-only '{required}'; allowed: {allowed:?}",
        );
    }
}

/// `Research` → `TaskIntent::Question` shares the read-only-or-
/// semantic-tool predicate with Review, so the same exclusions
/// apply. Pin both contracts independently so Review and Research
/// can diverge in the future without a shared-test regression.
#[test]
fn research_context_filter_drops_mutating_tools() {
    let registry = build_tui_like_registry();
    let allowed = registry.filter_by_intent(TaskIntent::Question);

    for forbidden in ["apply_patch", "shell"] {
        assert!(
            !allowed.iter().any(|name| name == forbidden),
            "Research filter must drop '{forbidden}', but allowed: {allowed:?}",
        );
    }
    assert!(allowed.iter().any(|name| name == "read_file"));
    assert!(allowed.iter().any(|name| name == "list_dir"));
    assert!(allowed.iter().any(|name| name == "grep_files"));
    assert!(allowed.iter().any(|name| name == "web_search"));
}

/// Default (`None` context) → `TaskIntent::Unknown` keeps every
/// tool — the runtime relies on a downstream auto-classifier to
/// pick the actual intent on the first user message, and pre-
/// filtering would defeat that path.
#[test]
fn unknown_intent_keeps_all_registered_tools() {
    let registry = build_tui_like_registry();
    let allowed = registry.filter_by_intent(TaskIntent::Unknown);

    for required in ["apply_patch", "shell", "read_file", "submit_intent_draft"] {
        assert!(
            allowed.iter().any(|name| name == required),
            "Unknown filter dropped '{required}'; allowed: {allowed:?}",
        );
    }
}

/// `Command` → `TaskIntent::Command` lets `shell` through but no
/// other mutating tool — this is the special "run a single shell
/// command" intent surfaced by the auto-classifier and the
/// allowlist contract is documented in
/// `tool_allowed_for_intent` in `tools/registry.rs`.
#[test]
fn command_intent_allows_shell_only_among_mutating_tools() {
    let registry = build_tui_like_registry();
    let allowed = registry.filter_by_intent(TaskIntent::Command);

    assert!(
        allowed.iter().any(|name| name == "shell"),
        "Command filter must keep 'shell'; allowed: {allowed:?}"
    );
    for forbidden in [
        "apply_patch",
        "submit_intent_draft",
        "submit_plan_draft",
        "submit_task_complete",
        "update_plan",
    ] {
        assert!(
            !allowed.iter().any(|name| name == forbidden),
            "Command filter must drop '{forbidden}', but allowed: {allowed:?}",
        );
    }
}

/// MCP bridge tools (e.g. `run_libra_vcs`, `create_*`,
/// `update_intent`) are dynamically added to the live agent
/// registry. The `is_read_only_or_semantic_tool` allowlist in
/// `tools/registry.rs` is hardcoded with a small set of names —
/// MCP bridge mutating tools deliberately DON'T appear there, so
/// Review and Research filters drop them.
///
/// This test pins that contract: any MCP-bridge name starting
/// with `run_`, `create_`, or `update_` MUST be dropped under
/// `TaskIntent::Review`. A future MCP-bridge tool that ships
/// under one of those prefixes without an explicit Review allow
/// would silently slip through to a Review session — Codex
/// pass-1 P2 follow-up flagged this as a gap; this test closes
/// it.
#[test]
fn mcp_bridge_mutating_tools_are_dropped_in_review_intent() {
    let registry = build_tui_like_registry();
    let dev_allowed = registry.filter_by_intent(TaskIntent::Feature);
    let review_allowed = registry.filter_by_intent(TaskIntent::Review);

    let mutating_mcp_in_dev: Vec<&String> = dev_allowed
        .iter()
        .filter(|name| {
            name.starts_with("run_") || name.starts_with("create_") || name.starts_with("update_")
        })
        .collect();
    assert!(
        !mutating_mcp_in_dev.is_empty(),
        "expected at least one MCP-bridge mutating tool in the Dev allowlist; found: {dev_allowed:?}",
    );

    for name in &mutating_mcp_in_dev {
        assert!(
            !review_allowed.iter().any(|allowed| allowed == *name),
            "MCP-bridge mutating tool '{name}' must be dropped under Review, but allowed: {review_allowed:?}",
        );
    }
}
