//! OC-Phase 3 UserInitiated cancel-propagation E2E (fake-provider).
//!
//! `docs/development/commands/_general.md` line 979 names this file
//! explicitly: opencode PR #25798 lifted cancel semantics to
//! `Effect.Effect<void>` so a parent abort must await child cleanup
//! before releasing. Libra's equivalent contract is the
//! `AbortToken` tree the dispatcher threads through every dispatch:
//! a parent cancel surfaces as `TaskFailure::Cancelled { source:
//! ParentAbort }` from the dispatch future.
//!
//! Scope of THIS file (two cancel paths, both verified)
//!
//! - **Pre-flight cancel**: the parent's abort token is cancelled
//!   BEFORE the dispatch even reaches the gates. The dispatcher's
//!   first cancel check (v0.17.743) short-circuits with
//!   `Cancelled { ParentAbort }`, so neither the asker nor the
//!   provider is touched, and the parent JSONL stays byte-empty.
//!   `pre_cancelled_user_initiated_dispatch_short_circuits_with_parent_abort`
//!   below.
//! - **Mid-flight cancel** (v0.17.767): the parent aborts WHILE
//!   the child is in a long provider await. The runner's
//!   `tokio::select!` short-circuits the in-flight tool loop and
//!   returns `Cancelled { ParentAbort }` before the fake fixture's
//!   delay elapses; the dispatcher then writes `Spawned +
//!   Cancelled { reason: UserRequested }` to the parent JSONL, and
//!   the child JSONL reaches a terminal cancelled snapshot.
//!   `mid_flight_cancel_during_child_run_writes_cancelled_event`
//!   below.

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
        compaction_model: None,
        hook_runner: None,
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
    // below pins the mid-flight cancel path that DOES write a
    // Cancelled event.
    let events_path = store.events_path();
    let bytes = std::fs::read(&events_path).unwrap_or_default();
    assert!(
        bytes.is_empty(),
        "pre-cancelled dispatch must NOT write any JSONL events; found {} bytes at '{}'",
        bytes.len(),
        events_path.display(),
    );
}

/// Mid-flight cancel: dispatch a sub-agent whose fake fixture
/// stalls for 5 seconds, then cancel the parent's abort token
/// after the dispatch is in-flight. The `tokio::select!` in
/// `DefaultSubAgentChildRunner::run` must short-circuit with
/// `Cancelled { ParentAbort }` instead of waiting for the stalled
/// provider future. Unlike the pre-flight cancel above, this
/// dispatch DOES write `AgentRunEvent::Spawned` first (the cancel
/// fires after gates clear and the Spawned event lands), so the
/// JSONL ends with `Spawned + Cancelled`.
#[tokio::test]
async fn mid_flight_cancel_during_child_run_writes_cancelled_event() {
    use libra::internal::ai::{
        agent_run::{AgentRunEvent, CancellationReason},
        session::jsonl::SessionEvent,
    };

    let temp = tempfile::tempdir().expect("tempdir");
    let store = SessionJsonlStore::new(temp.path().to_path_buf());
    let conn = sea_orm::Database::connect("sqlite::memory:")
        .await
        .expect("sqlite memory db");
    let usage_recorder = UsageRecorder::new(conn);
    let context_frame_loader = ContextFrameLoader::default();
    let (asker, _) = CountingNeverFireAsker::new();
    let permission_service = PermissionService::new(asker as Arc<dyn PermissionAsker>);
    let provider_factory = ProviderFactory;
    let mut slow_fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    slow_fixture.push("tests/fixtures/sub_agent/explore_slow.json");
    let provider_options = ProviderBuildOptions {
        fake_fixture_path: Some(slow_fixture),
        ..ProviderBuildOptions::default()
    };
    let tool_registry = ToolRegistry::with_working_dir(std::path::PathBuf::from(temp.path()));

    let parent = parent_spec();
    let parent_ruleset = Vec::new();
    let parent_binding = parent.model.clone().expect("parent binding");
    let session_id: SessionId = "session-midflight".to_string();

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

    // Construct the abort token; clone the inner so a parallel
    // task can cancel it after the dispatch is in-flight.
    let abort_token = AbortToken::new();
    let abort_canceller = abort_token.clone();
    let context = DispatchContext {
        parent_thread_id: "thread-midflight",
        parent_session_id: &session_id,
        parent_agent: &parent,
        parent_ruleset: &parent_ruleset,
        parent_model_binding: &parent_binding,
        parent_message_id: MessageId::from("msg-midflight"),
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
        description: "kick off slow child run".to_string(),
        prompt: "slow request".to_string(),
        subagent_type: "explore".to_string(),
        task_id: None,
    };

    // Race the dispatch (which awaits a 5-second fake provider
    // response) against a 100ms cancel. The cancel MUST win and
    // the dispatch MUST return Cancelled in well under the
    // fixture's delay.
    let canceller_task = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        abort_canceller.cancel();
    });

    let dispatch_start = std::time::Instant::now();
    let err = dispatcher
        .dispatch(
            context,
            invocation,
            TaskEntryKind::UserInitiated {
                bypass_permission_ask: true,
            },
        )
        .await
        .expect_err("mid-flight cancel must short-circuit the in-flight dispatch");
    let elapsed = dispatch_start.elapsed();
    canceller_task.await.expect("canceller task should finish");

    assert!(
        matches!(
            err,
            TaskFailure::Cancelled {
                source: CancellationSource::ParentAbort,
            }
        ),
        "expected Cancelled{{ParentAbort}}, got: {err:?}",
    );
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "mid-flight cancel must return well before the fake's 5s delay; elapsed = {elapsed:?}",
    );

    // The dispatch DID pass the gates + write Spawned before the
    // cancel fired (cancel happens during the child loop's
    // provider await). The dispatcher's terminal event mapping
    // converts Cancelled{ParentAbort} -> Cancelled{UserRequested}.
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
        "mid-flight cancel must still emit Spawned + Cancelled; got {agent_run_events:?}",
    );
    let spawned_run_id = match &agent_run_events[0] {
        AgentRunEvent::Spawned { agent_run_id, .. } => agent_run_id,
        other => panic!("first event must be Spawned, got: {other:?}"),
    };
    match &agent_run_events[1] {
        AgentRunEvent::Cancelled {
            agent_run_id,
            reason,
        } => {
            assert_eq!(
                agent_run_id, spawned_run_id,
                "Cancelled terminal event must keep the Spawned agent_run_id",
            );
            assert!(
                matches!(reason, CancellationReason::UserRequested),
                "ParentAbort must map to UserRequested, got: {reason:?}",
            );
        }
        other => panic!("expected Cancelled terminal event, got: {other:?}"),
    }

    let spawned_run_id_string = spawned_run_id.0.to_string();
    let child_store = store.child(&spawned_run_id_string);
    let child_events = child_store
        .load_events()
        .expect("child session JSONL must be readable after cancel");
    let child_event_kinds: Vec<_> = child_events
        .iter()
        .map(libra::internal::ai::runtime::Event::event_kind)
        .collect();
    assert_eq!(
        child_event_kinds,
        vec!["session_snapshot", "session_snapshot"],
        "mid-flight cancel must persist a child start snapshot and a terminal cancel snapshot",
    );

    let child_state = child_store
        .load_state()
        .expect("child session JSONL must replay after cancel")
        .expect("mid-flight cancel must persist a child session snapshot");
    assert_eq!(
        child_state
            .metadata
            .get("agent_run_id")
            .and_then(serde_json::Value::as_str),
        Some(spawned_run_id_string.as_str()),
        "child session metadata must link back to the parent Spawned event",
    );
    assert_eq!(
        child_state
            .metadata
            .get("status")
            .and_then(serde_json::Value::as_str),
        Some("cancelled"),
        "child session must leave replay in a terminal cancelled state",
    );
    assert_eq!(
        child_state.summary, "Sub-agent cancelled by parent abort",
        "child replay summary must explain the cancellation source",
    );
    assert_eq!(
        child_state.messages.len(),
        1,
        "cancelled child run should retain the prompt without inventing an assistant reply",
    );
    assert_eq!(child_state.messages[0].role, "user");
    assert_eq!(child_state.messages[0].content, "slow request");
}
