//! OC-Phase 3 LlmInitiated sub-agent dispatch E2E (fake-provider).
//!
//! `docs/improvement/opencode.md` line 932 names this file explicitly:
//! the OC-Phase 3 acceptance gate requires a fake-provider end-to-end
//! that drives `TaskEntryKind::LlmInitiated` through the full
//! dispatcher → child runner → tool loop → parent JSONL chain. The
//! v0.17.737..v0.17.763 sequence shipped each piece in isolation;
//! this file confirms they compose.
//!
//! Sequence under test:
//! 1. Build a `fake/` provider client from
//!    `tests/fixtures/sub_agent/explore_simple.json`.
//! 2. Resolve a sub-agent spec whose `model = "fake/some-model"` so
//!    `DispatchContext::build_child_model` builds the fake model
//!    via `ProviderFactory::build` (the same code path libra code's
//!    production session bootstrap uses).
//! 3. Attach `DefaultSubAgentChildRunner` to the dispatcher via
//!    `with_default_child_runner`.
//! 4. Dispatch `task(subagent_type = "explore", prompt =
//!    "summarise the repo")` through `LlmInitiated` (which triggers
//!    the permission ask; the fixture asker returns `Once`).
//! 5. Assert: (a) the dispatch returns `Ok(TaskResult)` whose
//!    `final_text` matches the fixture's matched response
//!    (`contains: "summarise"`); (b) `steps_used >= 1` because the
//!    fixture rule has no tool calls, so the child loop ends after
//!    one model turn; (c) the parent session JSONL carries exactly
//!    two AgentRun events: `Spawned` then `Completed`.
//!
//! This is the test the doc requires, gated `#[cfg(feature =
//! "test-provider")]`. The fixture is intentionally minimal —
//! tool-call edge cases ride in separate E2Es that share this
//! harness pattern.

#![cfg(feature = "test-provider")]

use std::{path::PathBuf, sync::Arc};

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
        // No tools needed for this single-turn fixture; the
        // fixture's matched rule emits text directly. Default
        // `AgentPermissionSpec` denies everything tool-wise (empty
        // `allowed_tools`), which is what we want here.
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

struct AllowOnceAsker;
impl PermissionAsker for AllowOnceAsker {
    fn ask<'a>(
        &'a self,
        _request: PermissionAskRequest<'a>,
    ) -> futures::future::BoxFuture<'a, PermissionReply> {
        Box::pin(async { PermissionReply::Once })
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
async fn llm_initiated_dispatch_with_fake_provider_returns_fixture_response_and_writes_paired_events()
 {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = SessionJsonlStore::new(temp.path().to_path_buf());
    let conn = sea_orm::Database::connect("sqlite::memory:")
        .await
        .expect("sqlite memory db");
    let usage_recorder = UsageRecorder::new(conn);
    let context_frame_loader = ContextFrameLoader::default();
    let permission_service =
        PermissionService::new(Arc::new(AllowOnceAsker) as Arc<dyn PermissionAsker>);
    let provider_factory = ProviderFactory;
    let provider_options = ProviderBuildOptions {
        fake_fixture_path: Some(fixture_path()),
        ..ProviderBuildOptions::default()
    };
    let tool_registry = ToolRegistry::with_working_dir(std::path::PathBuf::from(temp.path()));

    let parent = parent_spec();
    let parent_ruleset = Vec::new();
    let parent_binding = parent.model.clone().expect("parent binding");
    let session_id: SessionId = "session-e2e".to_string();

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
        parent_thread_id: "thread-e2e",
        parent_session_id: &session_id,
        parent_agent: &parent,
        parent_ruleset: &parent_ruleset,
        parent_model_binding: &parent_binding,
        parent_message_id: MessageId::from("msg-e2e"),
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
    };

    let invocation = TaskInvocation {
        description: "ask explorer to summarise".to_string(),
        prompt: "summarise the repo".to_string(),
        subagent_type: "explore".to_string(),
        task_id: None,
    };

    let result = dispatcher
        .dispatch(context, invocation, TaskEntryKind::LlmInitiated)
        .await
        .expect("E2E dispatch must succeed against the fake provider");

    assert_eq!(result.agent_name, "explore");
    assert_eq!(result.provider_id, "fake");
    assert_eq!(result.model_id, "some-model");
    assert!(
        result.final_text.contains("explorer sub-agent"),
        "final_text must surface the fixture's matched-rule text, got: {}",
        result.final_text,
    );
    assert!(
        result.final_text.contains("summary"),
        "final_text must come from the `summarise`-matched rule, got: {}",
        result.final_text,
    );
    assert!(
        result.steps_used >= 1,
        "steps_used must be at least 1 after a successful provider call, got: {}",
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
    assert!(
        matches!(agent_run_events[0], AgentRunEvent::Spawned { .. }),
        "first event must be Spawned, got: {:?}",
        agent_run_events[0],
    );
    assert!(
        matches!(agent_run_events[1], AgentRunEvent::Completed { .. }),
        "second event must be Completed, got: {:?}",
        agent_run_events[1],
    );
}
