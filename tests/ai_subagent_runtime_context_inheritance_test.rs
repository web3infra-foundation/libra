//! CEX-S2-12 / S2-INV-06 — parent → child `runtime_context` inheritance.
//!
//! `DispatchContext::runtime_context` is documented as the runtime
//! sandbox / approval / file-history context "inherited by child tool
//! invocations ... they must not get a fresh approval authority".
//! Before the fix this test guards, `DefaultSubAgentChildRunner::run`
//! built its child `ToolLoopConfig` with `runtime_context` left at the
//! `None` default, silently dropping the parent's context — so every
//! child tool call ran with no sandbox and approval defaulting to
//! `Skip`, strictly *more* permissive than the parent (a violation of
//! S2-INV-06).
//!
//! This is an end-to-end proof through the real dispatcher + child
//! runner + fake provider: a child sub-agent invokes a recording tool,
//! and we assert the `ToolInvocation` the tool received carries the
//! exact `runtime_context` the parent put on the `DispatchContext`
//! (identified by a distinctive `max_output_bytes` marker). A
//! regression that drops the forwarding makes the recorder observe
//! `None` and trips the assertion.
//!
//! Scope note: this pins *inheritance* (S2-INV-06), not workspace
//! *isolation* (S2-INV-03). Rebasing the inherited sandbox
//! `writable_roots` onto a materialized per-run workspace is a separate
//! follow-on; here the child simply inherits whatever the parent
//! already enforces.

#![cfg(feature = "test-provider")]

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use libra::internal::ai::{
    agent::{
        profile::{
            AgentExecutionSpec, AgentMode, AgentPermissionSpec, ModelBinding, ToolSelection,
        },
        runtime::{
            AbortToken, AgentSpecRegistry, ContextFrameLoader, DefaultSubAgentDispatcher,
            DispatchContext, MessageId, MultiAgentConfig, PermissionAskRequest, PermissionAsker,
            PermissionReply, PermissionService, SubAgentDispatcher, TaskEntryKind, TaskInvocation,
        },
    },
    providers::{ProviderBuildOptions, ProviderFactory},
    sandbox::{
        ApprovalCachePolicy, ApprovalStore, AskForApproval, ExecApprovalRequest,
        FileHistoryRuntimeContext, SandboxPermissions, SandboxPolicy, ToolApprovalContext,
        ToolRuntimeContext, ToolSandboxContext,
    },
    session::{SessionId, file_history::FileHistoryStore, jsonl::SessionJsonlStore},
    tools::{
        ToolHandler, ToolInvocation, ToolKind, ToolOutput, ToolRegistry, ToolResult, ToolSpec,
        handlers::ApplyPatchHandler,
    },
    usage::UsageRecorder,
};

/// Distinctive marker values that a freshly-defaulted
/// `ToolRuntimeContext` would never carry, so observing them on the
/// child's tool invocation proves the parent context was inherited
/// verbatim rather than reconstructed. One marker per authority
/// component S2-INV-06 protects (sandbox / approval / file-history)
/// plus the scalar budget.
const MARKER_MAX_OUTPUT_BYTES: usize = 0x00C0_FFEE;
const MARKER_SCOPE_PREFIX: &str = "s2-inv-06-marker-scope-prefix";
const MARKER_BATCH_ID: &str = "s2-inv-06-marker-batch-id";

fn fixture_path() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests/fixtures/sub_agent/tool_call_then_done.json");
    path
}

/// Sub-agent whose fixture drives exactly one `record_runtime_context`
/// tool call. `ToolSelection::Inherit` keeps the recorder visible in
/// the child's tool surface; the default permission spec only
/// default-denies `task` / `todowrite`, so the uniquely-named recorder
/// survives `available_for`.
fn recorder_sub_agent_spec() -> AgentExecutionSpec {
    AgentExecutionSpec {
        name: "recorder".to_string(),
        description: "Records the runtime_context it is dispatched with".to_string(),
        mode: AgentMode::Subagent,
        model: ModelBinding::parse("fake/some-model"),
        tools: ToolSelection::Inherit,
        permission: AgentPermissionSpec::default(),
        ..AgentExecutionSpec::default()
    }
}

fn parent_spec() -> AgentExecutionSpec {
    AgentExecutionSpec {
        name: "parent".to_string(),
        description: "runtime_context inheritance fixture parent".to_string(),
        mode: AgentMode::Primary,
        model: ModelBinding::parse("fake/parent-model"),
        ..AgentExecutionSpec::default()
    }
}

/// Tool that records the `runtime_context` attached to the invocation
/// it receives. The captured value is shared back out so the test can
/// assert the child inherited the parent's context.
struct RuntimeContextRecorder {
    captured: Arc<Mutex<Option<Option<ToolRuntimeContext>>>>,
}

#[async_trait]
impl ToolHandler for RuntimeContextRecorder {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> ToolResult<ToolOutput> {
        // Record the FULL `Option` (outer = "did the tool fire",
        // inner = "did it carry a runtime_context") so a dropped
        // forwarding surfaces as `Some(None)`, distinct from the tool
        // never being invoked (`None`).
        *self.captured.lock().expect("recorder mutex") = Some(invocation.runtime_context.clone());
        Ok(ToolOutput::success("recorded"))
    }

    fn schema(&self) -> ToolSpec {
        ToolSpec::new(
            "record_runtime_context",
            "Record the runtime_context attached to this invocation",
        )
    }
}

struct StaticRegistry {
    spec: AgentExecutionSpec,
}

impl AgentSpecRegistry for StaticRegistry {
    fn lookup(&self, name: &str) -> Option<AgentExecutionSpec> {
        if name == self.spec.name {
            Some(self.spec.clone())
        } else {
            None
        }
    }

    fn registered_names(&self) -> Vec<String> {
        vec![self.spec.name.clone()]
    }
}

/// Asker that must never be reached: the dispatch is `UserInitiated`
/// with `bypass_permission_ask: true`, so step 8 is skipped entirely.
struct UnreachableAsker;

impl PermissionAsker for UnreachableAsker {
    fn ask<'a>(
        &'a self,
        _request: PermissionAskRequest<'a>,
    ) -> futures::future::BoxFuture<'a, PermissionReply> {
        Box::pin(async { unreachable!("UserInitiated bypass must not reach the asker") })
    }
}

#[tokio::test]
async fn child_tool_invocation_inherits_parent_runtime_context() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = SessionJsonlStore::new(temp.path().to_path_buf());
    let conn = sea_orm::Database::connect("sqlite::memory:")
        .await
        .expect("sqlite memory db");
    let usage_recorder = UsageRecorder::new(conn);
    let context_frame_loader = ContextFrameLoader::default();
    let permission_service = PermissionService::new(Arc::new(UnreachableAsker));
    let provider_factory = ProviderFactory;
    let provider_options = ProviderBuildOptions {
        fake_fixture_path: Some(fixture_path()),
        ..ProviderBuildOptions::default()
    };

    // Register the recording tool on the parent registry; the child
    // runner intersects this registry with the sub-spec to build the
    // child's tool surface.
    let captured: Arc<Mutex<Option<Option<ToolRuntimeContext>>>> = Arc::new(Mutex::new(None));
    let mut tool_registry = ToolRegistry::with_working_dir(temp.path().to_path_buf());
    tool_registry.register(
        "record_runtime_context",
        Arc::new(RuntimeContextRecorder {
            captured: Arc::clone(&captured),
        }),
    );

    // The parent context the child must inherit. Every one of the
    // authority components S2-INV-06 protects is populated with a
    // distinctive marker (a defaulted `ToolRuntimeContext` carries
    // `None` for sandbox/approval/file_history and `None` for
    // `max_output_bytes`), so observing all four on the child's
    // invocation proves the WHOLE context is inherited verbatim — not
    // just one scalar field.
    //
    // `_approval_rx` is held in scope for the whole run so the
    // inherited `request_tx` stays valid; the recorder tool never
    // requests approval, so nothing is ever sent on it.
    let (approval_tx, _approval_rx) = tokio::sync::mpsc::unbounded_channel::<ExecApprovalRequest>();
    let parent_runtime_context = ToolRuntimeContext {
        sandbox: Some(ToolSandboxContext {
            policy: SandboxPolicy::ReadOnly,
            permissions: SandboxPermissions::RequireEscalated,
        }),
        approval: Some(ToolApprovalContext {
            policy: AskForApproval::OnRequest,
            request_tx: approval_tx,
            store: Arc::new(tokio::sync::Mutex::new(ApprovalStore::default())),
            scope_key_prefix: Some(MARKER_SCOPE_PREFIX.to_string()),
            approval_ttl: Duration::from_secs(4242),
            cache_policy: ApprovalCachePolicy::default(),
        }),
        file_history: Some(FileHistoryRuntimeContext {
            session_root: temp.path().join("parent-session-root"),
            batch_id: MARKER_BATCH_ID.to_string(),
        }),
        max_output_bytes: Some(MARKER_MAX_OUTPUT_BYTES),
        ..ToolRuntimeContext::default()
    };

    let parent = parent_spec();
    let parent_ruleset = Vec::new();
    let parent_binding = parent.model.clone().expect("parent binding");
    let session_id: SessionId = "session-runtime-ctx".to_string();

    let registry = Arc::new(StaticRegistry {
        spec: recorder_sub_agent_spec(),
    });
    let dispatcher = DefaultSubAgentDispatcher::new(
        registry,
        MultiAgentConfig {
            enabled: true,
            max_subagent_depth: 4,
            max_concurrent_subagents: 4,
        },
    )
    .with_default_child_runner();

    let abort_token = AbortToken::new();
    let context = DispatchContext {
        parent_thread_id: "thread-runtime-ctx",
        parent_session_id: &session_id,
        parent_agent: &parent,
        parent_ruleset: &parent_ruleset,
        parent_model_binding: &parent_binding,
        parent_message_id: MessageId::from("msg-runtime-ctx"),
        permission_service: &permission_service,
        session_store: &store,
        provider_factory: &provider_factory,
        provider_build_options: &provider_options,
        provider_build_options_resolver: None,
        tool_registry: &tool_registry,
        runtime_context: Some(parent_runtime_context),
        usage_recorder: &usage_recorder,
        context_frame_loader: &context_frame_loader,
        abort_token,
        depth: 0,
        compaction_model: None,
        hook_runner: None,
    };

    let invocation = TaskInvocation {
        description: "ask the recorder to capture its runtime context".to_string(),
        // Must contain the fixture's `contains` matcher so the fake
        // provider emits the `record_runtime_context` tool call.
        prompt: "please invoke the recorder tool now".to_string(),
        subagent_type: "recorder".to_string(),
        task_id: None,
    };

    let result = dispatcher
        .dispatch(
            context,
            invocation,
            TaskEntryKind::UserInitiated {
                bypass_permission_ask: true,
            },
        )
        .await
        .expect("child run must complete against the fake provider");

    // The fixture falls through to the text fallback on turn 2 (the
    // tool-result message carries no user text), so a clean run ends
    // with the fallback text — proving the tool call happened and the
    // loop terminated rather than looping on the matcher.
    assert_eq!(result.agent_name, "recorder");
    assert!(
        result.final_text.contains("recorder sub-agent done"),
        "expected the fixture fallback text after the tool call, got: {}",
        result.final_text,
    );

    let captured = captured.lock().expect("recorder mutex").clone();
    let invocation_runtime_context = captured.expect(
        "the recorder tool must have been invoked by the child — \
         if this is None the tool call was blocked or never emitted",
    );
    let runtime_context = invocation_runtime_context.expect(
        "the child tool invocation must carry the inherited runtime_context — \
         `None` here means DefaultSubAgentChildRunner dropped \
         DispatchContext::runtime_context (S2-INV-06 regression)",
    );

    // Every authority component must arrive on the child verbatim, not
    // just the scalar budget. Each assertion would still hold under a
    // hypothetical partial forward, so checking all four is what proves
    // the full struct (sandbox + approval + file-history authority) is
    // inherited.
    let sandbox = runtime_context
        .sandbox
        .as_ref()
        .expect("child must inherit the parent's sandbox authority, not a fresh `None`");
    assert!(
        matches!(sandbox.policy, SandboxPolicy::ReadOnly),
        "child must inherit the parent's sandbox policy verbatim, got: {:?}",
        sandbox.policy,
    );
    assert_eq!(
        sandbox.permissions,
        SandboxPermissions::RequireEscalated,
        "child must inherit the parent's sandbox permissions verbatim",
    );

    let approval = runtime_context
        .approval
        .as_ref()
        .expect("child must inherit the parent's approval authority, not a fresh `None`");
    assert_eq!(
        approval.scope_key_prefix.as_deref(),
        Some(MARKER_SCOPE_PREFIX),
        "child must inherit the parent's approval scope, not a fresh authority \
         (S2-INV-06: sub-agents must not get a fresh approval authority)",
    );

    let file_history = runtime_context
        .file_history
        .as_ref()
        .expect("child must inherit the parent's file-history authority, not a fresh `None`");
    assert_eq!(
        file_history.batch_id, MARKER_BATCH_ID,
        "child must inherit the parent's file-history batch verbatim",
    );

    assert_eq!(
        runtime_context.max_output_bytes,
        Some(MARKER_MAX_OUTPUT_BYTES),
        "child must inherit the parent's output-budget cap verbatim",
    );
}

// ───────────────────────────────────────────────────────────────────
// End-to-end: a child `apply_patch` records undo preimages under the
// inherited file-history batch (S2-INV-06 / undo path).
// ───────────────────────────────────────────────────────────────────

fn apply_patch_fixture_path() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests/fixtures/sub_agent/apply_patch_then_done.json");
    path
}

/// Sub-agent whose fixture drives exactly one `apply_patch` tool call
/// against `target.txt`.
fn patcher_sub_agent_spec() -> AgentExecutionSpec {
    AgentExecutionSpec {
        name: "patcher".to_string(),
        description: "Applies a patch to target.txt".to_string(),
        mode: AgentMode::Subagent,
        model: ModelBinding::parse("fake/some-model"),
        tools: ToolSelection::Inherit,
        permission: AgentPermissionSpec::default(),
        ..AgentExecutionSpec::default()
    }
}

const PREIMAGE_BATCH_ID: &str = "child-preimage-batch";
const TARGET_BEFORE: &str = "line 1\nline 2\nline 3\n";

/// Proves the full undo path: a dispatched child that calls the real
/// `ApplyPatchHandler` records an undo preimage under the file-history
/// batch it inherited from the parent's `runtime_context`. Without the
/// inheritance wiring the child invocation would carry `file_history:
/// None`, `ApplyPatchHandler` would silently skip `record_preimages`,
/// and `undo_latest_batch` below would find nothing to restore.
#[tokio::test]
async fn child_apply_patch_records_undo_preimage_under_inherited_batch() {
    let temp = tempfile::tempdir().expect("tempdir");
    let working_dir = temp.path().to_path_buf();
    let target = working_dir.join("target.txt");
    std::fs::write(&target, TARGET_BEFORE).expect("seed target.txt");
    let session_root = working_dir.join(".libra/sessions/preimage-session");

    let store = SessionJsonlStore::new(temp.path().to_path_buf());
    let conn = sea_orm::Database::connect("sqlite::memory:")
        .await
        .expect("sqlite memory db");
    let usage_recorder = UsageRecorder::new(conn);
    let context_frame_loader = ContextFrameLoader::default();
    let permission_service = PermissionService::new(Arc::new(UnreachableAsker));
    let provider_factory = ProviderFactory;
    let provider_options = ProviderBuildOptions {
        fake_fixture_path: Some(apply_patch_fixture_path()),
        ..ProviderBuildOptions::default()
    };

    // Register the REAL apply_patch handler so the child exercises the
    // production preimage-recording path.
    let mut tool_registry = ToolRegistry::with_working_dir(working_dir.clone());
    tool_registry.register("apply_patch", Arc::new(ApplyPatchHandler));

    // Inherit a file-history batch (and no approval context, so the
    // in-workspace patch applies without an approval round-trip).
    let parent_runtime_context = ToolRuntimeContext {
        file_history: Some(FileHistoryRuntimeContext {
            session_root: session_root.clone(),
            batch_id: PREIMAGE_BATCH_ID.to_string(),
        }),
        ..ToolRuntimeContext::default()
    };

    let parent = parent_spec();
    let parent_ruleset = Vec::new();
    let parent_binding = parent.model.clone().expect("parent binding");
    let session_id: SessionId = "session-preimage".to_string();

    let registry = Arc::new(StaticRegistry {
        spec: patcher_sub_agent_spec(),
    });
    let dispatcher = DefaultSubAgentDispatcher::new(
        registry,
        MultiAgentConfig {
            enabled: true,
            max_subagent_depth: 4,
            max_concurrent_subagents: 4,
        },
    )
    .with_default_child_runner();

    let abort_token = AbortToken::new();
    let context = DispatchContext {
        parent_thread_id: "thread-preimage",
        parent_session_id: &session_id,
        parent_agent: &parent,
        parent_ruleset: &parent_ruleset,
        parent_model_binding: &parent_binding,
        parent_message_id: MessageId::from("msg-preimage"),
        permission_service: &permission_service,
        session_store: &store,
        provider_factory: &provider_factory,
        provider_build_options: &provider_options,
        provider_build_options_resolver: None,
        tool_registry: &tool_registry,
        runtime_context: Some(parent_runtime_context),
        usage_recorder: &usage_recorder,
        context_frame_loader: &context_frame_loader,
        abort_token,
        depth: 0,
        compaction_model: None,
        hook_runner: None,
    };

    let invocation = TaskInvocation {
        description: "patch target.txt".to_string(),
        prompt: "please apply the patch to target.txt".to_string(),
        subagent_type: "patcher".to_string(),
        task_id: None,
    };

    let result = dispatcher
        .dispatch(
            context,
            invocation,
            TaskEntryKind::UserInitiated {
                bypass_permission_ask: true,
            },
        )
        .await
        .expect("child run must complete against the fake provider");
    assert_eq!(result.agent_name, "patcher");

    // The child's apply_patch must have actually modified the file.
    let after = std::fs::read_to_string(&target).expect("read target.txt after patch");
    assert!(
        after.contains("line 2 modified"),
        "child apply_patch should have modified target.txt, got: {after:?}",
    );

    // The preimage must have been recorded under the INHERITED batch:
    // undoing the latest batch restores the original content and the
    // report names the inherited batch id.
    let report = FileHistoryStore::new(session_root)
        .undo_latest_batch(&working_dir)
        .expect(
            "the child apply_patch must have recorded an undo preimage under the \
             inherited file-history batch — nothing to undo means file_history \
             was not inherited (S2-INV-06 regression)",
        );
    assert_eq!(
        report.batch_id, PREIMAGE_BATCH_ID,
        "the preimage must be recorded under the parent-inherited batch id",
    );
    assert_eq!(
        std::fs::read_to_string(&target).expect("read target.txt after undo"),
        TARGET_BEFORE,
        "undoing the inherited batch must restore the child's pre-patch content",
    );
}
