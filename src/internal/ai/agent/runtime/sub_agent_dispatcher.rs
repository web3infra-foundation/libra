//! Default `SubAgentDispatcher` implementation — gates only (steps 1–7).
//!
//! This module is the OC-Phase 3 P3.3 deliverable from
//! `docs/improvement/opencode.md`. It implements the **gate half** of
//! the 14-step dispatcher main flow:
//!
//! 1. validate feature flag (`code.multi_agent.enabled`) — implemented
//! 2. validate `ctx.depth + 1 <= max_subagent_depth` — implemented
//! 3. validate `concurrent_count + 1 <= max_concurrent_subagents`
//!    via atomic `fetch_add` claim — implemented
//! 4. resolve `subagent_type` via the spec registry; reject `Primary`
//!    profiles — implemented
//! 5. `SafetyDecision::evaluate(SubAgentSpawn { name, prompt_digest })`
//!    — **DEFERRED** to P3.4. Today the call is a marked TODO no-op
//!    that always accepts. The P3.4 wire-up needs a `SubAgentSpawn`
//!    [`crate::internal::ai::runtime::ToolOperation`] variant before
//!    `ToolBoundaryPolicy::decide` can take it. Documenting the
//!    deferral here so the surrounding gate ordering is correct;
//!    callers see no semantic gap because no real safety policy is
//!    configured against `SubAgentSpawn` today.
//! 6. compute `effective_ruleset` via `child_ruleset(parent, sub_spec)`
//!    — implemented
//! 7. assert no permission escalation (Permission Escalation Gate)
//!    — implemented
//!
//! Steps 8–13 (permission ask, handoff, model build, child loop) are
//! P3.4+. In this PR the dispatcher returns a placeholder
//! [`TaskResult`] **only when every gate passes** — the call cannot
//! reach a real sub-agent run yet. Callers who hit the success path
//! will see an empty `final_text` and `steps_used = 0`; that is by
//! design and tests pin it.
//!
//! The gates are tested in isolation. The placeholder tail makes it
//! possible for the steps-1-through-7 checks to land with their full
//! cleanup story (concurrency counter decrement, SafetyDecision call)
//! without coupling P3.3 to the still-unwritten P3.4 services.

use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};

use futures::future::BoxFuture;

use super::sub_agent::{
    DispatchContext, SubAgentDispatcher, TaskEntryKind, TaskFailure, TaskInvocation, TaskResult,
};
use crate::internal::ai::{
    agent::profile::AgentExecutionSpec,
    completion::CompletionUsageSummary,
    permission::{
        EDIT_TOOLS, PermissionRuleset, agent_permission_spec_to_ruleset, assert_no_escalation,
        child_ruleset,
    },
};

/// Runtime configuration for the multi-agent feature gate.
///
/// `enabled` mirrors `code.multi_agent.enabled` from the doc's
/// configuration section (OC-Phase 5 will wire the TOML loader; today
/// the default is `false` so the gate is loud whenever the dispatcher
/// is invoked under flag-off).
///
/// Limits default to the doc's `max_subagent_depth = 1` and
/// `max_concurrent_subagents = 1` — even when the feature flag flips,
/// the runtime starts conservative.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MultiAgentConfig {
    pub enabled: bool,
    pub max_subagent_depth: u8,
    pub max_concurrent_subagents: u32,
}

impl Default for MultiAgentConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_subagent_depth: 1,
            max_concurrent_subagents: 1,
        }
    }
}

/// Registry the dispatcher consults to resolve a `subagent_type` string
/// into the agent's [`AgentExecutionSpec`].
///
/// The trait stays minimal so callers can plug in either an
/// `AgentProfileRouter` adapter or a synthetic test registry without
/// pulling the entire profile loader through. The two methods are
/// `lookup` (the resolve path the dispatcher uses) and
/// `registered_names` (so error suggestions match the live registry).
pub trait AgentSpecRegistry: Send + Sync {
    fn lookup(&self, name: &str) -> Option<AgentExecutionSpec>;
    fn registered_names(&self) -> Vec<String>;
}

/// Default dispatcher implementation. Holds a registry, a config, and
/// a shared concurrency counter that subsequent dispatches increment +
/// decrement around the gate.
pub struct DefaultSubAgentDispatcher {
    registry: Arc<dyn AgentSpecRegistry>,
    config: MultiAgentConfig,
    in_flight: Arc<AtomicU32>,
}

impl DefaultSubAgentDispatcher {
    pub fn new(registry: Arc<dyn AgentSpecRegistry>, config: MultiAgentConfig) -> Self {
        Self {
            registry,
            config,
            in_flight: Arc::new(AtomicU32::new(0)),
        }
    }

    /// Number of dispatches currently running (test introspection only).
    #[cfg(test)]
    pub fn in_flight(&self) -> u32 {
        self.in_flight.load(Ordering::Acquire)
    }

    /// Run the seven gates in order, returning either a placeholder
    /// [`TaskResult`] (steps 8-13 land in P3.4) or the first
    /// [`TaskFailure`] that fires.
    fn run_capability_gates(
        &self,
        ctx: &DispatchContext<'_>,
        invocation: &TaskInvocation,
        _entry_kind: TaskEntryKind,
    ) -> Result<(AgentExecutionSpec, PermissionRuleset), TaskFailure> {
        // Step 1: feature flag. A dedicated `FeatureDisabled` variant
        // keeps log analysis distinct from the step-5 SafetyDenied path
        // that lands in P3.4.
        if !self.config.enabled {
            return Err(TaskFailure::FeatureDisabled);
        }

        // Step 2: depth gate.
        let next_depth = ctx.depth.saturating_add(1);
        if next_depth > self.config.max_subagent_depth {
            return Err(TaskFailure::DepthExceeded {
                current: ctx.depth,
                limit: self.config.max_subagent_depth,
            });
        }

        // Step 3 lives in dispatch() so the slot increment happens
        // atomically with the check (avoiding a TOCTOU race where two
        // concurrent dispatches both pass step 3 with `current = 0`).

        // Step 4: resolve subagent_type. `Primary`-only profiles cannot
        // be dispatched as sub-agents — they must be either `Subagent`
        // or `All`.
        let sub_spec = match self.registry.lookup(&invocation.subagent_type) {
            Some(spec) if spec.mode.is_subagent_eligible() => spec,
            Some(_unsuitable) => {
                return Err(TaskFailure::UnknownSubagent {
                    name: invocation.subagent_type.clone(),
                    suggestions: self.subagent_eligible_suggestions(),
                });
            }
            None => {
                return Err(TaskFailure::UnknownSubagent {
                    name: invocation.subagent_type.clone(),
                    suggestions: self.subagent_eligible_suggestions(),
                });
            }
        };

        // Step 5: SafetyDecision evaluate.
        //
        // TODO(OC-Phase 3 P3.4): wire `ToolBoundaryPolicy::decide`
        // against a freshly-introduced `ToolOperation::SubAgentSpawn`
        // variant carrying `{ name, prompt_digest }`. Today this is a
        // documented no-op — P3.3 ships the gate ordering and the
        // P3.4 PR will swap this stub for the real call without
        // touching the surrounding capability gates. The dispatcher
        // accepts any sub-spec that survived the prior gates; no
        // semantic gap exists today because Libra has no
        // `SubAgentSpawn` policy configured.
        let _safety_decision_stub_marker = ();

        // Step 6: compute effective ruleset for the child.
        let effective = child_ruleset(ctx.parent_ruleset, &sub_spec.permission);

        // Step 7: escalation gate. The doc spec calls for (builtin tool
        // names ∪ sub-spec permission keys) × ("*" ∪ sub-spec patterns).
        // Both sample sets are computed dynamically so a future
        // `AgentPermissionSpec` schema that grows non-`"*"` patterns
        // does not silently lose coverage.
        let permission_keys = collect_permission_keys(&sub_spec.permission);
        let permission_key_refs: Vec<&str> = permission_keys.iter().map(String::as_str).collect();
        let pattern_samples = collect_pattern_samples(&sub_spec.permission);
        let pattern_sample_refs: Vec<&str> = pattern_samples.iter().map(String::as_str).collect();
        if let Err((permission, pattern)) = assert_no_escalation(
            ctx.parent_ruleset,
            &effective,
            &permission_key_refs,
            &pattern_sample_refs,
        ) {
            return Err(TaskFailure::PermissionEscalationDenied {
                permission,
                pattern,
            });
        }

        Ok((sub_spec, effective))
    }

    fn subagent_eligible_suggestions(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .registry
            .registered_names()
            .into_iter()
            .filter(|name| {
                self.registry
                    .lookup(name)
                    .is_some_and(|spec| spec.mode.is_subagent_eligible())
            })
            .collect();
        names.sort();
        names
    }
}

impl SubAgentDispatcher for DefaultSubAgentDispatcher {
    fn dispatch<'a>(
        &'a self,
        ctx: DispatchContext<'a>,
        invocation: TaskInvocation,
        entry_kind: TaskEntryKind,
    ) -> BoxFuture<'a, Result<TaskResult, TaskFailure>> {
        Box::pin(async move {
            // Steps 1, 2: feature flag + depth. These cannot mutate
            // shared state, so they run before any concurrency slot
            // is claimed. (Step 3 follows with an atomic claim.)
            if !self.config.enabled {
                return Err(TaskFailure::FeatureDisabled);
            }
            if ctx.depth.saturating_add(1) > self.config.max_subagent_depth {
                return Err(TaskFailure::DepthExceeded {
                    current: ctx.depth,
                    limit: self.config.max_subagent_depth,
                });
            }

            // Step 3: claim a concurrency slot ATOMICALLY. `fetch_add`
            // unconditionally increments and returns the previous
            // value; if that was already at the limit we roll back
            // and surface `ConcurrencyExceeded`. This avoids the
            // load-then-add TOCTOU race where two concurrent
            // dispatches could both pass a `load == 0, limit == 1`
            // check and end up with `in_flight == 2`.
            let prev = self.in_flight.fetch_add(1, Ordering::AcqRel);
            if prev >= self.config.max_concurrent_subagents {
                self.in_flight.fetch_sub(1, Ordering::AcqRel);
                return Err(TaskFailure::ConcurrencyExceeded {
                    current: prev,
                    limit: self.config.max_concurrent_subagents,
                });
            }

            // RAII guard: from here on every exit path (early-return on
            // a TaskFailure from steps 4-7, panic, or normal success at
            // the end) decrements the counter exactly once. P3.4 will
            // put real I/O between this guard's creation and the
            // placeholder result; the guard is what prevents a panic
            // in that I/O from orphaning the slot.
            let _slot = ConcurrencyGuard {
                counter: Arc::clone(&self.in_flight),
            };

            // Steps 4-7: capability + permission gates that don't
            // touch the concurrency counter.
            let (sub_spec, _effective) =
                self.run_capability_gates(&ctx, &invocation, entry_kind)?;

            // Steps 8-13 land in P3.4. Today the placeholder tail
            // produces a TaskResult shaped from the resolved spec so
            // gate-success is observable end-to-end.
            let result = TaskResult {
                task_id: invocation.task_id.clone().unwrap_or_else(|| {
                    // P3.5 will mint a real id from the AgentRunEvent
                    // chain; for now the synthetic id keeps the test
                    // surface deterministic.
                    format!(
                        "task-placeholder-{}-depth-{}",
                        invocation.subagent_type, ctx.depth
                    )
                }),
                agent_name: sub_spec.name.clone(),
                provider_id: sub_spec
                    .model
                    .as_ref()
                    .map(|m| m.provider_id.clone())
                    .unwrap_or_default(),
                model_id: sub_spec
                    .model
                    .as_ref()
                    .map(|m| m.model_id.clone())
                    .unwrap_or_default(),
                final_text: String::new(),
                steps_used: 0,
                usage: CompletionUsageSummary::default(),
            };
            // `_slot` drops here, releasing the concurrency slot.
            Ok(result)
        })
    }
}

/// RAII handle for a concurrency slot claimed via [`AtomicU32::fetch_add`].
///
/// Dropping the guard decrements the counter once, regardless of
/// whether the dispatch returned `Ok`, returned `Err`, or panicked.
/// This is the doc-promised "decrement happens in dispatch's
/// drop-guarded scope" invariant. The guard holds an `Arc` to the
/// counter so it can outlive the dispatcher's borrow if a future
/// refactor moves the dispatcher behind a different ownership model.
struct ConcurrencyGuard {
    counter: Arc<AtomicU32>,
}

impl Drop for ConcurrencyGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Collect every permission key referenced by a sub-spec, plus the
/// canonical defense-in-depth set the doc requires (`task`,
/// `todowrite`, `edit`, every member of `EDIT_TOOLS`). The result
/// feeds into the escalation gate's cartesian product.
fn collect_permission_keys(
    spec: &crate::internal::ai::agent::profile::AgentPermissionSpec,
) -> Vec<String> {
    use std::collections::BTreeSet;
    let mut set: BTreeSet<String> = BTreeSet::new();
    for tool in &spec.allowed_tools {
        set.insert(tool.clone());
    }
    for tool in &spec.denied_tools {
        set.insert(tool.clone());
    }
    set.insert("task".to_string());
    set.insert("todowrite".to_string());
    set.insert("edit".to_string());
    for tool in EDIT_TOOLS {
        set.insert((*tool).to_string());
    }
    set.into_iter().collect()
}

/// Collect every pattern referenced by a sub-spec's converted ruleset,
/// always including `"*"` as a defense-in-depth sample. The doc
/// requires `("*" ∪ child_spec.permission.iter().map(|r| &r.pattern))`
/// for the escalation gate's cartesian product; computing this from
/// the converted ruleset future-proofs the dispatcher against a
/// schema evolution that grows non-`"*"` patterns on
/// `AgentPermissionSpec`.
fn collect_pattern_samples(
    spec: &crate::internal::ai::agent::profile::AgentPermissionSpec,
) -> Vec<String> {
    use std::collections::BTreeSet;
    let mut set: BTreeSet<String> = BTreeSet::new();
    set.insert("*".to_string());
    for rule in agent_permission_spec_to_ruleset(spec) {
        set.insert(rule.pattern);
    }
    set.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeSet, HashMap},
        sync::Mutex,
    };

    use sea_orm::Database;

    use super::*;
    use crate::internal::ai::{
        agent::{
            profile::{
                AgentExecutionSpec, AgentMode, AgentPermissionSpec, ApprovalRoutingSpec,
                ModelBinding, ToolSelection,
            },
            runtime::sub_agent::{
                AbortToken, ContextFrameLoader, DispatchContext, MessageId, PermissionService,
                SubAgentDispatcher, TaskEntryKind, TaskFailure, TaskInvocation,
            },
        },
        permission::{PermissionAction, PermissionRule, PermissionRuleset},
        providers::ProviderFactory,
        session::SessionId,
        usage::UsageRecorder,
    };

    /// Test-only registry storing specs in a HashMap; the doc says any
    /// registry works as long as it implements the trait.
    #[derive(Default)]
    struct TestRegistry {
        specs: Mutex<HashMap<String, AgentExecutionSpec>>,
    }

    impl TestRegistry {
        fn insert(&self, spec: AgentExecutionSpec) {
            self.specs.lock().unwrap().insert(spec.name.clone(), spec);
        }
    }

    impl AgentSpecRegistry for TestRegistry {
        fn lookup(&self, name: &str) -> Option<AgentExecutionSpec> {
            self.specs.lock().unwrap().get(name).cloned()
        }
        fn registered_names(&self) -> Vec<String> {
            self.specs.lock().unwrap().keys().cloned().collect()
        }
    }

    fn explore_subagent() -> AgentExecutionSpec {
        let mut spec = AgentExecutionSpec {
            name: "explore".to_string(),
            description: "Read-only explorer".to_string(),
            mode: AgentMode::Subagent,
            ..AgentExecutionSpec::default()
        };
        let mut allowed = BTreeSet::new();
        allowed.insert("read_file".to_string());
        spec.permission = AgentPermissionSpec {
            allowed_tools: allowed,
            ..AgentPermissionSpec::default()
        };
        spec
    }

    fn primary_only_agent() -> AgentExecutionSpec {
        AgentExecutionSpec {
            name: "planner".to_string(),
            description: "Primary planner".to_string(),
            mode: AgentMode::Primary,
            ..AgentExecutionSpec::default()
        }
    }

    fn parent_spec() -> AgentExecutionSpec {
        AgentExecutionSpec {
            name: "parent".to_string(),
            description: "Parent driver".to_string(),
            mode: AgentMode::Primary,
            tools: ToolSelection::Inherit,
            permission: AgentPermissionSpec {
                approval_routing: ApprovalRoutingSpec::Layer1Human,
                ..AgentPermissionSpec::default()
            },
            ..AgentExecutionSpec::default()
        }
    }

    fn parent_binding() -> ModelBinding {
        ModelBinding::parse("anthropic/claude-3-5-sonnet-latest").unwrap()
    }

    /// Build a `DispatchContext` for tests. The placeholder service
    /// shells are intentionally `Default::default()`; the gates we
    /// exercise here do not touch them.
    #[allow(clippy::too_many_arguments)]
    fn ctx<'a>(
        parent_thread_id: &'a str,
        parent_session_id: &'a SessionId,
        parent_agent: &'a AgentExecutionSpec,
        parent_ruleset: &'a PermissionRuleset,
        parent_binding: &'a ModelBinding,
        permission_service: &'a PermissionService,
        session_store: &'a crate::internal::ai::session::jsonl::SessionJsonlStore,
        provider_factory: &'a ProviderFactory,
        usage_recorder: &'a UsageRecorder,
        context_frame_loader: &'a ContextFrameLoader,
        depth: u8,
    ) -> DispatchContext<'a> {
        DispatchContext {
            parent_thread_id,
            parent_session_id,
            parent_agent,
            parent_ruleset,
            parent_model_binding: parent_binding,
            parent_message_id: MessageId::from("msg-1"),
            permission_service,
            session_store,
            provider_factory,
            usage_recorder,
            context_frame_loader,
            abort_token: AbortToken::new(),
            depth,
        }
    }

    /// Helper to async-build the runtime services tests need.
    async fn dispatcher_test_harness(
        config: MultiAgentConfig,
    ) -> (
        DefaultSubAgentDispatcher,
        Arc<TestRegistry>,
        UsageRecorder,
        crate::internal::ai::session::jsonl::SessionJsonlStore,
    ) {
        let registry = Arc::new(TestRegistry::default());
        let dispatcher = DefaultSubAgentDispatcher::new(registry.clone(), config);
        let conn = Database::connect("sqlite::memory:").await.unwrap();
        let usage_recorder = UsageRecorder::new(conn);
        let temp = tempfile::tempdir().unwrap();
        let store =
            crate::internal::ai::session::jsonl::SessionJsonlStore::new(temp.path().to_path_buf());
        // Leak the temp dir so the SessionJsonlStore reference remains
        // valid for the test duration.
        std::mem::forget(temp);
        (dispatcher, registry, usage_recorder, store)
    }

    fn invocation(subagent_type: &str) -> TaskInvocation {
        TaskInvocation {
            description: "test invocation".to_string(),
            prompt: "do a thing".to_string(),
            subagent_type: subagent_type.to_string(),
            task_id: None,
        }
    }

    /// Scenario: with `multi_agent.enabled = false`, the dispatcher
    /// rejects every dispatch with `FeatureDisabled`. This is the
    /// flag-off invariant — even if the tool slipped past the
    /// registry-level filter, the dispatcher still refuses with a
    /// dedicated variant (not `SafetyDenied`, which is reserved for
    /// step-5 sandbox rejections in P3.4).
    #[tokio::test]
    async fn dispatch_rejects_when_feature_flag_disabled() {
        let (dispatcher, registry, usage, store) =
            dispatcher_test_harness(MultiAgentConfig::default()).await;
        registry.insert(explore_subagent());

        let parent = parent_spec();
        let parent_ruleset: PermissionRuleset = Vec::new();
        let parent_binding = parent_binding();
        let permission_service = PermissionService::default();
        let provider_factory = ProviderFactory;
        let context_frame_loader = ContextFrameLoader::default();
        let parent_thread = "thread-1".to_string();
        let parent_session: SessionId = "session-1".to_string();

        let context = ctx(
            &parent_thread,
            &parent_session,
            &parent,
            &parent_ruleset,
            &parent_binding,
            &permission_service,
            &store,
            &provider_factory,
            &usage,
            &context_frame_loader,
            0,
        );

        let result = dispatcher
            .dispatch(context, invocation("explore"), TaskEntryKind::LlmInitiated)
            .await;
        assert!(
            matches!(result, Err(TaskFailure::FeatureDisabled)),
            "expected FeatureDisabled when multi_agent.enabled = false, got {:?}",
            result.as_ref().err()
        );
    }

    /// Scenario: depth gate fires when `ctx.depth + 1 > limit`. The
    /// default config sets depth=1 so a depth-1 ctx must be rejected.
    #[tokio::test]
    async fn dispatch_rejects_when_depth_exceeded() {
        let config = MultiAgentConfig {
            enabled: true,
            max_subagent_depth: 1,
            max_concurrent_subagents: 4,
        };
        let (dispatcher, registry, usage, store) = dispatcher_test_harness(config).await;
        registry.insert(explore_subagent());

        let parent = parent_spec();
        let parent_ruleset: PermissionRuleset = Vec::new();
        let parent_binding = parent_binding();
        let permission_service = PermissionService::default();
        let provider_factory = ProviderFactory;
        let context_frame_loader = ContextFrameLoader::default();
        let parent_thread = "thread-1".to_string();
        let parent_session: SessionId = "session-1".to_string();

        let context = ctx(
            &parent_thread,
            &parent_session,
            &parent,
            &parent_ruleset,
            &parent_binding,
            &permission_service,
            &store,
            &provider_factory,
            &usage,
            &context_frame_loader,
            1, // depth + 1 = 2 > limit 1
        );

        let result = dispatcher
            .dispatch(context, invocation("explore"), TaskEntryKind::LlmInitiated)
            .await;
        assert!(matches!(
            result,
            Err(TaskFailure::DepthExceeded {
                current: 1,
                limit: 1
            })
        ));
    }

    /// Scenario: concurrency gate fires when the in-flight counter is
    /// already at the limit. We seed the counter directly to emulate a
    /// parallel dispatch.
    #[tokio::test]
    async fn dispatch_rejects_when_concurrency_exceeded() {
        let config = MultiAgentConfig {
            enabled: true,
            max_subagent_depth: 4,
            max_concurrent_subagents: 1,
        };
        let (dispatcher, registry, usage, store) = dispatcher_test_harness(config).await;
        registry.insert(explore_subagent());
        // Pre-occupy the only slot.
        dispatcher.in_flight.fetch_add(1, Ordering::AcqRel);

        let parent = parent_spec();
        let parent_ruleset: PermissionRuleset = Vec::new();
        let parent_binding = parent_binding();
        let permission_service = PermissionService::default();
        let provider_factory = ProviderFactory;
        let context_frame_loader = ContextFrameLoader::default();
        let parent_thread = "thread-1".to_string();
        let parent_session: SessionId = "session-1".to_string();

        let context = ctx(
            &parent_thread,
            &parent_session,
            &parent,
            &parent_ruleset,
            &parent_binding,
            &permission_service,
            &store,
            &provider_factory,
            &usage,
            &context_frame_loader,
            0,
        );

        let result = dispatcher
            .dispatch(context, invocation("explore"), TaskEntryKind::LlmInitiated)
            .await;
        assert!(matches!(
            result,
            Err(TaskFailure::ConcurrencyExceeded {
                current: 1,
                limit: 1
            })
        ));
    }

    /// Scenario: an unknown subagent_type errors with suggestions
    /// drawn from the subagent-eligible registry entries. A
    /// `Primary`-only profile in the registry must NOT appear in
    /// suggestions — the doc explicitly forbids dispatching primary
    /// agents through `task`.
    #[tokio::test]
    async fn dispatch_rejects_unknown_subagent_with_eligible_suggestions_only() {
        let config = MultiAgentConfig {
            enabled: true,
            max_subagent_depth: 4,
            max_concurrent_subagents: 4,
        };
        let (dispatcher, registry, usage, store) = dispatcher_test_harness(config).await;
        registry.insert(explore_subagent()); // mode = Subagent
        registry.insert(primary_only_agent()); // mode = Primary

        let parent = parent_spec();
        let parent_ruleset: PermissionRuleset = Vec::new();
        let parent_binding = parent_binding();
        let permission_service = PermissionService::default();
        let provider_factory = ProviderFactory;
        let context_frame_loader = ContextFrameLoader::default();
        let parent_thread = "thread-1".to_string();
        let parent_session: SessionId = "session-1".to_string();

        let context = ctx(
            &parent_thread,
            &parent_session,
            &parent,
            &parent_ruleset,
            &parent_binding,
            &permission_service,
            &store,
            &provider_factory,
            &usage,
            &context_frame_loader,
            0,
        );

        let result = dispatcher
            .dispatch(
                context,
                invocation("does-not-exist"),
                TaskEntryKind::LlmInitiated,
            )
            .await;
        match result {
            Err(TaskFailure::UnknownSubagent { name, suggestions }) => {
                assert_eq!(name, "does-not-exist");
                assert!(suggestions.contains(&"explore".to_string()));
                assert!(
                    !suggestions.contains(&"planner".to_string()),
                    "primary-only agents must NOT appear in subagent suggestions"
                );
            }
            other => panic!("expected UnknownSubagent, got {other:?}"),
        }
    }

    /// Scenario: a sub-spec that opts into `edit` while the parent
    /// denies `edit: *` is refused by the escalation gate. The
    /// returned `(permission, pattern)` pair must name `edit`.
    #[tokio::test]
    async fn dispatch_rejects_permission_escalation() {
        let config = MultiAgentConfig {
            enabled: true,
            max_subagent_depth: 4,
            max_concurrent_subagents: 4,
        };
        let (dispatcher, registry, usage, store) = dispatcher_test_harness(config).await;

        // Sub-spec opts into edit.
        let mut sub = explore_subagent();
        sub.name = "edit-explorer".to_string();
        let mut allowed = BTreeSet::new();
        allowed.insert("edit".to_string());
        sub.permission = AgentPermissionSpec {
            allowed_tools: allowed,
            ..AgentPermissionSpec::default()
        };
        registry.insert(sub);

        // Parent denies edit globally.
        let parent_ruleset: PermissionRuleset =
            vec![PermissionRule::new("edit", "*", PermissionAction::Deny)];
        let parent = parent_spec();
        let parent_binding = parent_binding();
        let permission_service = PermissionService::default();
        let provider_factory = ProviderFactory;
        let context_frame_loader = ContextFrameLoader::default();
        let parent_thread = "thread-1".to_string();
        let parent_session: SessionId = "session-1".to_string();

        let context = ctx(
            &parent_thread,
            &parent_session,
            &parent,
            &parent_ruleset,
            &parent_binding,
            &permission_service,
            &store,
            &provider_factory,
            &usage,
            &context_frame_loader,
            0,
        );

        let result = dispatcher
            .dispatch(
                context,
                invocation("edit-explorer"),
                TaskEntryKind::LlmInitiated,
            )
            .await;
        match result {
            Err(TaskFailure::PermissionEscalationDenied {
                permission,
                pattern: _,
            }) => {
                assert_eq!(permission, "edit");
            }
            other => panic!("expected PermissionEscalationDenied, got {other:?}"),
        }
    }

    /// Scenario (TOCTOU regression guard): with the only slot already
    /// held, two concurrent dispatches must BOTH receive
    /// `ConcurrencyExceeded` and the counter must remain at the
    /// pre-test value (the rejected `fetch_add` calls rolled back).
    /// A naive load-then-add gate would let both pass step 3 and end
    /// up with `in_flight == 3`; the atomic `fetch_add` + rollback
    /// pattern keeps the invariant tight under contention.
    #[tokio::test]
    async fn dispatch_concurrent_calls_against_held_slot_both_reject() {
        let config = MultiAgentConfig {
            enabled: true,
            max_subagent_depth: 4,
            max_concurrent_subagents: 1,
        };
        let (dispatcher, registry, usage, store) = dispatcher_test_harness(config).await;
        registry.insert(explore_subagent());
        // Hold the only slot for the entire test — both concurrent
        // dispatches will see `prev = 1, limit = 1` and roll back.
        // (Acquiring the slot via fetch_add reproduces what a real
        // in-flight dispatch would do.)
        dispatcher.in_flight.fetch_add(1, Ordering::AcqRel);

        let parent = parent_spec();
        let parent_ruleset: PermissionRuleset = Vec::new();
        let parent_binding = parent_binding();
        let permission_service = PermissionService::default();
        let provider_factory = ProviderFactory;
        let context_frame_loader = ContextFrameLoader::default();
        let parent_thread = "thread-1".to_string();
        let parent_session: SessionId = "session-1".to_string();
        let context_a = ctx(
            &parent_thread,
            &parent_session,
            &parent,
            &parent_ruleset,
            &parent_binding,
            &permission_service,
            &store,
            &provider_factory,
            &usage,
            &context_frame_loader,
            0,
        );
        let context_b = ctx(
            &parent_thread,
            &parent_session,
            &parent,
            &parent_ruleset,
            &parent_binding,
            &permission_service,
            &store,
            &provider_factory,
            &usage,
            &context_frame_loader,
            0,
        );

        let (result_a, result_b) = tokio::join!(
            dispatcher.dispatch(
                context_a,
                invocation("explore"),
                TaskEntryKind::LlmInitiated,
            ),
            dispatcher.dispatch(
                context_b,
                invocation("explore"),
                TaskEntryKind::LlmInitiated,
            ),
        );
        // Both calls observed the held slot → both must reject.
        assert!(matches!(
            result_a,
            Err(TaskFailure::ConcurrencyExceeded {
                current: 1,
                limit: 1
            })
        ));
        assert!(matches!(
            result_b,
            Err(TaskFailure::ConcurrencyExceeded {
                current: 1,
                limit: 1
            })
        ));
        // Counter still at the held value (1); rejected calls rolled
        // their fetch_add back.
        assert_eq!(dispatcher.in_flight(), 1);
    }

    /// Scenario: every gate passes → the placeholder TaskResult flows
    /// through with the resolved provider/model bound to the agent's
    /// spec. The concurrency counter returns to 0 after the call.
    #[tokio::test]
    async fn dispatch_returns_placeholder_result_when_every_gate_passes() {
        let config = MultiAgentConfig {
            enabled: true,
            max_subagent_depth: 4,
            max_concurrent_subagents: 4,
        };
        let (dispatcher, registry, usage, store) = dispatcher_test_harness(config).await;

        let mut sub = explore_subagent();
        sub.model = ModelBinding::parse("anthropic/claude-3-5-haiku-latest");
        registry.insert(sub);

        let parent = parent_spec();
        let parent_ruleset: PermissionRuleset = Vec::new();
        let parent_binding = parent_binding();
        let permission_service = PermissionService::default();
        let provider_factory = ProviderFactory;
        let context_frame_loader = ContextFrameLoader::default();
        let parent_thread = "thread-1".to_string();
        let parent_session: SessionId = "session-1".to_string();

        let context = ctx(
            &parent_thread,
            &parent_session,
            &parent,
            &parent_ruleset,
            &parent_binding,
            &permission_service,
            &store,
            &provider_factory,
            &usage,
            &context_frame_loader,
            0,
        );

        let result = dispatcher
            .dispatch(context, invocation("explore"), TaskEntryKind::LlmInitiated)
            .await
            .expect("every gate should pass");

        assert_eq!(result.agent_name, "explore");
        assert_eq!(result.provider_id, "anthropic");
        assert_eq!(result.model_id, "claude-3-5-haiku-latest");
        // P3.3 placeholder tail leaves these empty/zero — P3.4 fills.
        assert_eq!(result.final_text, "");
        assert_eq!(result.steps_used, 0);

        // Concurrency counter must return to 0 after the call.
        assert_eq!(dispatcher.in_flight(), 0);
    }
}
