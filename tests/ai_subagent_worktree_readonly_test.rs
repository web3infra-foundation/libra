//! OC-Phase 3 sub-agent worktree readonly contract.
//!
//! `docs/improvement/opencode.md` line 1019..1030 commits Libra to a
//! temporary sub-agent worktree posture: until CEX-S2-11 lands a real
//! isolated workspace, an OC-Phase 3 sub-agent **cannot** write to the
//! parent's main worktree. The doc names the regression target by
//! file (`tests/ai_subagent_worktree_readonly_test.rs`) so future PRs
//! that ship Phase 3 hardening have an obvious place to extend.
//!
//! This integration test pins the registry-level contract that the
//! pre-filter (`ToolRegistry::available_for`) strips every member of
//! `permission::EDIT_TOOLS` from the schema once the sub-agent's
//! resolved ruleset carries `[{edit:*: deny}]`. Schema-level filtering
//! is the contract that keeps `apply_patch` out of the model's tool
//! menu — the model literally cannot see the tool to call it.
//!
//! Two scenarios:
//! 1. The doc's named example (`apply_patch`) is filtered when the
//!    sub-agent's effective ruleset denies `edit`.
//! 2. The full `EDIT_TOOLS` set (`apply_patch`, `write_file`,
//!    `patch`) is filtered together, so a regression that drops the
//!    `EDIT_TOOLS` alias list from the pre-filter is loud rather
//!    than partial.

use std::sync::Arc;

use async_trait::async_trait;
use libra::internal::ai::{
    agent::profile::{AgentExecutionSpec, AgentMode},
    permission::{EDIT_TOOLS, PermissionAction, PermissionRule, PermissionRuleset},
    tools::{
        ToolHandler, ToolInvocation, ToolKind, ToolOutput, ToolRegistry, ToolResult, ToolSpec,
    },
};

/// Inert tool handler used purely to occupy slots in the registry.
/// The pre-filter contract is schema-level — these handlers are never
/// dispatched, so the body of `handle` is unreachable.
struct InertHandler {
    name: &'static str,
    description: &'static str,
}

#[async_trait]
impl ToolHandler for InertHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, _invocation: ToolInvocation) -> ToolResult<ToolOutput> {
        unreachable!(
            "worktree_readonly fixture only exercises the registry pre-filter; \
             handle() is never invoked because the tool is removed from the schema"
        );
    }

    fn schema(&self) -> ToolSpec {
        ToolSpec::new(self.name, self.description)
    }
}

/// Build a registry containing every member of `permission::EDIT_TOOLS`
/// plus one read-only tool (`read_file`) so the pre-filter has both a
/// "must be stripped" and a "must survive" axis. Handlers are inert
/// stubs — the test exercises the schema, not the execution path.
fn registry_with_edit_tools_and_read_file() -> ToolRegistry {
    let mut registry = ToolRegistry::with_working_dir(std::path::PathBuf::from("/tmp"));
    for name in EDIT_TOOLS {
        registry.register(
            *name,
            Arc::new(InertHandler {
                name,
                description: "worktree-readonly inert edit-tool stub",
            }),
        );
    }
    registry.register(
        "read_file",
        Arc::new(InertHandler {
            name: "read_file",
            description: "worktree-readonly inert read-only stub",
        }),
    );
    registry
}

/// Construct the minimal `AgentExecutionSpec` shape the sub-agent
/// dispatcher hands to `available_for`: `mode = Subagent`, tools
/// inherit (so the deny comes from the ruleset, not a spec-level
/// allow-list).
fn sub_agent_spec() -> AgentExecutionSpec {
    AgentExecutionSpec {
        name: "sub-agent-worktree-readonly-fixture".to_string(),
        mode: AgentMode::Subagent,
        ..AgentExecutionSpec::default()
    }
}

/// Per opencode.md line 1030: "fake sub-agent 试图调用 `apply_patch`,
/// 应在 `available_for()` 阶段被过滤". A sub-agent whose effective
/// ruleset includes `[{edit:*: deny}]` must NOT see `apply_patch` in
/// the schema returned by `ToolRegistry::available_for`.
#[test]
fn sub_agent_with_edit_deny_ruleset_cannot_see_apply_patch_in_schema() {
    let registry = registry_with_edit_tools_and_read_file();
    let spec = sub_agent_spec();
    let ruleset: PermissionRuleset = vec![PermissionRule::new("edit", "*", PermissionAction::Deny)];

    let surviving: Vec<String> = registry
        .available_for(&spec, &ruleset)
        .into_iter()
        .map(|spec| spec.function.name)
        .collect();

    assert!(
        !surviving.contains(&"apply_patch".to_string()),
        "OC-Phase 3 sub-agent worktree readonly contract requires \
         apply_patch be stripped under [{{edit:*: deny}}]; got \
         surviving = {surviving:?}"
    );
    assert!(
        surviving.contains(&"read_file".to_string()),
        "read-only tools must survive the edit-deny filter; got \
         surviving = {surviving:?}"
    );
}

/// Stronger form: the entire `EDIT_TOOLS` alias group
/// (`apply_patch`, `write_file`, `patch`) must be stripped together.
/// A regression that filters only `apply_patch` and forgets the
/// other aliases would let a model call `write_file` to mutate the
/// parent worktree.
#[test]
fn sub_agent_with_edit_deny_ruleset_strips_full_edit_tools_alias_set() {
    let registry = registry_with_edit_tools_and_read_file();
    let spec = sub_agent_spec();
    let ruleset: PermissionRuleset = vec![PermissionRule::new("edit", "*", PermissionAction::Deny)];

    let surviving: Vec<String> = registry
        .available_for(&spec, &ruleset)
        .into_iter()
        .map(|spec| spec.function.name)
        .collect();

    for tool in EDIT_TOOLS {
        assert!(
            !surviving.contains(&(*tool).to_string()),
            "edit tool `{tool}` must be stripped from sub-agent schema \
             under [{{edit:*: deny}}]; got surviving = {surviving:?}"
        );
    }
}
