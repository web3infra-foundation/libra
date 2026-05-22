//! OC-Phase 3 UserInitiated cancel-propagation E2E (fake-provider).
//!
//! `docs/improvement/opencode.md` line 979 names this file
//! explicitly: opencode PR #25798 lifted cancel semantics to
//! `Effect.Effect<void>` so a parent abort must await child cleanup
//! before releasing. Libra's equivalent contract is the
//! `AbortToken` tree the dispatcher threads through every dispatch:
//! a parent cancel surfaces as `TaskFailure::Cancelled { source:
//! ParentAbort }` from the dispatch future.
//!
//! Scope of THIS test
//!
//! - **Pre-flight cancel**: the parent's abort token is cancelled
//!   BEFORE the dispatch even reaches the gates. The dispatcher's
//!   first cancel check (v0.17.743) short-circuits with
//!   `Cancelled { ParentAbort }`, so neither the asker nor the
//!   provider is touched, and the parent JSONL stays byte-empty.
//! - This is the "before Spawned" half of the contract; the
//!   "during child run" half (await child cleanup; emit
//!   `AgentRunEvent::Failed { reason: Cancelled }` after the child
//!   loop unwinds) is P3.7's child-handle-await work and rides in a
//!   follow-up PR that swaps the synchronous runner call for a
//!   `tokio::spawn` + `tokio::select!`.

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
            AbortToken, CancellationSource, ContextFrameLoader, DefaultSubAgentDispatcher,
            DispatchContext, MessageId, MultiAgentConfig, PermissionAskRequest, PermissionAsker,
            PermissionReply, PermissionService, SubAgentDispatcher, TaskEntryKind, TaskFailure,
            TaskInvocation,
        },
    },
    providers::{ProviderBuildOptions, ProviderFactory},
    session::{SessionId, jsonl::SessionJsonlStore},
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
        description: "Read-only explorer (cancel-E2E fixture)".to_string(),
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
        description: "cancel-E2E fixture parent".to_string(),
        mode: AgentMode::Primary,
        model: ModelBinding::parse("fake/parent-model"),
        ..AgentExecutionSpec::default()
    }
}

/// Asker that records every invocation but never resolves the ask.
/// The pre-flight cancel path must short-circuit before the
/// dispatcher reaches the asker, so this counter MUST stay at 0;
/// any non-zero value proves the cancel guard regressed.
struct CountingNeverFireAsker {
    calls: Arc<AtomicU32>,
}

impl CountingNeverFireAsker {
    fn new() -> (Arc<Self>, Arc<AtomicU32>) {
        let calls = Arc::new(AtomicU32::new(0));
        let asker = Arc::new(Self {
            calls: calls.clone(),
        });
        (asker, calls)
    }
}

impl PermissionAsker for CountingNeverFireAsker {
    fn ask<'a>(
        &'a self,
        _request: PermissionAskRequest<'a>,
    ) -> futures::future::BoxFuture<'a, PermissionReply> {
        let calls = self.calls.clone();
        Box::pin(async move {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            // Reply does not matter — the test expects the
            // dispatcher to never reach this point.
            PermissionReply::Once
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
async fn pre_cancelled_user_initiated_dispatch_short_circuits_with_parent_abort() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = SessionJsonlStore::new(temp.path().to_path_buf());
    let conn = sea_orm::Database::connect("sqlite::memory:")
        .await
        .expect("sqlite memory db");
    let usage_recorder = UsageRecorder::new(conn);
    let context_frame_loader = ContextFrameLoader::default();
    let (asker, ask_call_count) = CountingNeverFireAsker::new();
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
    let session_id: SessionId = "session-cancel".to_string();

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

    // Cancel the abort token BEFORE we hand the context to the
    // dispatcher. The pre-flight check at the top of `dispatch`
    // must observe this and return Cancelled immediately.
    let abort_token = AbortToken::new();
    abort_token.cancel();

    let context = DispatchContext {
        parent_thread_id: "thread-cancel",
        parent_session_id: &session_id,
        parent_agent: &parent,
        parent_ruleset: &parent_ruleset,
        parent_model_binding: &parent_binding,
        parent_message_id: MessageId::from("msg-cancel"),
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
    };

    let invocation = TaskInvocation {
        description: "cancel before gates".to_string(),
        prompt: "summarise the repo".to_string(),
        subagent_type: "explore".to_string(),
        task_id: None,
    };

    let err = dispatcher
        .dispatch(
            context,
            invocation,
            TaskEntryKind::UserInitiated {
                bypass_permission_ask: true,
            },
        )
        .await
        .expect_err("pre-cancelled abort token must short-circuit dispatch");

    assert!(
        matches!(
            err,
            TaskFailure::Cancelled {
                source: CancellationSource::ParentAbort,
            }
        ),
        "expected Cancelled{{ParentAbort}}, got: {err:?}",
    );

    assert_eq!(
        ask_call_count.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "pre-flight cancel must short-circuit BEFORE the asker is invoked",
    );

    // The dispatcher's first cancel check fires before any JSONL
    // write — Spawned never lands, so the parent session JSONL
    // must be byte-empty. The matching post-Spawned cancel test
    // (P3.7 child-handle-await follow-up) will assert a Cancelled
    // event instead.
    let events_path = store.events_path();
    let bytes = std::fs::read(&events_path).unwrap_or_default();
    assert!(
        bytes.is_empty(),
        "pre-cancelled dispatch must NOT write any JSONL events; found {} bytes at '{}'",
        bytes.len(),
        events_path.display(),
    );
}
