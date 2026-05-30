//! OC-Phase 3 UserInitiated sub-agent dispatch E2E (fake-provider).
//!
//! `docs/improvement/opencode.md` line 932 names this file explicitly:
//! the second of the two fake-provider end-to-end acceptance gates.
//! The other (`ai_subagent_llm_initiated_test`) covers
//! `TaskEntryKind::LlmInitiated`; this one pins
//! `UserInitiated { bypass_permission_ask: true }` — the slash
//! command / Code Control `task.dispatch` / SubtaskPart entry that
//! the user *explicitly* invokes, where the dialog ask is redundant
//! because the user already chose.
//!
//! What this test pins
//!
//! 1. Same dispatcher + runner shape as the LlmInitiated E2E, but
//!    the permission asker is a **rejecting** one. Even so, the
//!    dispatch must succeed because UserInitiated bypasses the ask.
//! 2. The `ask_call_count` (tracked by the fixture asker) stays at
//!    `0`: a regression that drops the bypass check would surface
//!    here as a count of `1` plus an `ApprovalRejected` failure.
//! 3. The same final-text / events contract as the LlmInitiated
//!    sibling — UserInitiated does not change anything downstream
//!    of step 8; the sole difference is in the ask gate.

#![cfg(feature = "test-provider")]

use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicU32},
};

use libra::internal::ai::{
    agent::{
        profile::{
            AgentExecutionSpec, AgentMode, AgentPermissionSpec, ModelBinding, ToolSelection,
        },
        runtime::{
            AbortToken, ContextFrameLoader, DefaultSubAgentDispatcher, DispatchContext, MessageId,
            MultiAgentConfig, PermissionAskRequest, PermissionAsker, PermissionReply,
            PermissionService, SubAgentDispatcher, TaskEntryKind, TaskInvocation,
        },
    },
    agent_run::AgentRunEvent,
    providers::{ProviderBuildOptions, ProviderFactory},
    session::{
        SessionId,
        jsonl::{SessionEvent, SessionJsonlStore},
    },
    tools::ToolRegistry,
    usage::UsageRecorder,
};

fn fixture_path() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests/fixtures/sub_agent/explore_simple.json");
    path
}

fn explore_sub_agent_spec() -> AgentExecutionSpec {
    AgentExecutionSpec {
        name: "explore".to_string(),
        description: "Read-only explorer (E2E fixture)".to_string(),
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
        description: "E2E fixture parent".to_string(),
        mode: AgentMode::Primary,
        model: ModelBinding::parse("fake/parent-model"),
        ..AgentExecutionSpec::default()
    }
}

/// Asker that always rejects AND counts every call. Used to prove
/// the UserInitiated bypass really does skip step 8 — if the
/// bypass regresses, this asker will fire (count > 0) and reject
/// the dispatch (failing the success assertion below).
struct CountingRejectAsker {
    calls: Arc<AtomicU32>,
}

impl CountingRejectAsker {
    fn new() -> (Arc<Self>, Arc<AtomicU32>) {
        let calls = Arc::new(AtomicU32::new(0));
        let asker = Arc::new(Self {
            calls: calls.clone(),
        });
        (asker, calls)
    }
}

impl PermissionAsker for CountingRejectAsker {
    fn ask<'a>(
        &'a self,
        _request: PermissionAskRequest<'a>,
    ) -> futures::future::BoxFuture<'a, PermissionReply> {
        let calls = self.calls.clone();
        Box::pin(async move {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            PermissionReply::Reject {
                feedback: Some("UserInitiated bypass must skip me".to_string()),
            }
        })
    }
}

struct StaticRegistry {
    spec: AgentExecutionSpec,
}

impl libra::internal::ai::agent::runtime::AgentSpecRegistry for StaticRegistry {
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

#[tokio::test]
async fn user_initiated_bypass_skips_ask_and_runs_child_against_fake_provider() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = SessionJsonlStore::new(temp.path().to_path_buf());
    let conn = sea_orm::Database::connect("sqlite::memory:")
        .await
        .expect("sqlite memory db");
    let usage_recorder = UsageRecorder::new(conn);
    let context_frame_loader = ContextFrameLoader::default();
    let (asker, ask_call_count) = CountingRejectAsker::new();
    let permission_service = PermissionService::new(asker as Arc<dyn PermissionAsker>);
    let provider_factory = ProviderFactory;
    let provider_options = ProviderBuildOptions {
        fake_fixture_path: Some(fixture_path()),
        ..ProviderBuildOptions::default()
    };
    let tool_registry = ToolRegistry::with_working_dir(std::path::PathBuf::from(temp.path()));

    let parent = parent_spec();
    let parent_ruleset = Vec::new();
    let parent_binding = parent.model.clone().expect("parent binding");
    let session_id: SessionId = "session-user-init".to_string();

    let registry = Arc::new(StaticRegistry {
        spec: explore_sub_agent_spec(),
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
        parent_thread_id: "thread-user-init",
        parent_session_id: &session_id,
        parent_agent: &parent,
        parent_ruleset: &parent_ruleset,
        parent_model_binding: &parent_binding,
        parent_message_id: MessageId::from("msg-user-init"),
        permission_service: &permission_service,
        session_store: &store,
        provider_factory: &provider_factory,
        provider_build_options: &provider_options,
        provider_build_options_resolver: None,
        tool_registry: &tool_registry,
        runtime_context: None,
        usage_recorder: &usage_recorder,
        context_frame_loader: &context_frame_loader,
        abort_token,
        depth: 0,
        compaction_model: None,
        hook_runner: None,
    };

    let invocation = TaskInvocation {
        description: "user-driven summarise dispatch".to_string(),
        prompt: "summarise the repo".to_string(),
        subagent_type: "explore".to_string(),
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
        .expect(
            "UserInitiated bypass must succeed even though the asker rejects — \
             a regression that calls the asker would surface as ApprovalRejected here",
        );

    assert_eq!(
        ask_call_count.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "UserInitiated{{bypass_permission_ask:true}} must NOT invoke the asker",
    );

    assert_eq!(result.agent_name, "explore");
    assert!(
        result.final_text.contains("explorer sub-agent"),
        "final_text must surface the fixture's matched-rule text, got: {}",
        result.final_text,
    );
    assert!(
        result.steps_used >= 1,
        "steps_used must be at least 1 after a successful child run, got: {}",
        result.steps_used,
    );

    let agent_run_events: Vec<AgentRunEvent> = store
        .load_events()
        .expect("session JSONL must be readable")
        .into_iter()
        .filter_map(|envelope| match envelope {
            SessionEvent::AgentRun(known) => known.known().cloned(),
            _ => None,
        })
        .collect();
    assert_eq!(
        agent_run_events.len(),
        2,
        "expected exactly Spawned + Completed events; got {agent_run_events:?}",
    );
    assert!(matches!(agent_run_events[0], AgentRunEvent::Spawned { .. }));
    assert!(matches!(
        agent_run_events[1],
        AgentRunEvent::Completed { .. }
    ));
}
