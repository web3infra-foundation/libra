//! Default `SubAgentDispatcher` implementation — gates, ask, and child loop.
//!
//! This module ships the dispatcher across OC-Phase 3 P3.3 and P3.4 from
//! `docs/improvement/opencode.md`. It implements the dispatcher main flow
//! through child model construction and normal tool-loop re-entry:
//!
//! 1. validate feature flag (`code.multi_agent.enabled`) — P3.3 implemented
//! 2. validate `ctx.depth + 1 <= max_subagent_depth` — P3.3 implemented
//! 3. validate `concurrent_count + 1 <= max_concurrent_subagents`
//!    via atomic `fetch_add` claim — P3.3 implemented
//! 4. resolve `subagent_type` via the spec registry; reject `Primary`
//!    profiles — P3.3 implemented
//! 5. `SafetyDecision::evaluate(SubAgentSpawn { name, prompt_digest })`
//!    through the registry's runtime `ToolBoundaryRuntime` when present,
//!    or a conservative system-principal default policy otherwise.
//! 6. compute `effective_ruleset` via `child_ruleset(parent, sub_spec)`
//!    — P3.3 implemented
//! 7. assert no permission escalation (Permission Escalation Gate)
//!    — P3.3 implemented
//! 8. `PermissionService.ask(...)` for `LlmInitiated` only;
//!    `UserInitiated { bypass_permission_ask: true }` skips the
//!    dialog. `Reject{feedback}` surfaces as
//!    [`TaskFailure::ApprovalRejected`]. — P3.4 implemented
//!
//! 9. build the child model from the child binding (or parent fallback)
//! 10. pre-filter child tools through `ToolRegistry::available_for`
//! 11. write a child JSONL session snapshot under `subagents/`
//! 12. run the child through the normal tool loop with nested `task`
//!     disabled
//! 13. append the child completion snapshot and return the child's
//!     final text / usage summary
//!
//! Timeout enforcement, token hard caps, and parent-cancel propagation are
//! implemented, including a pre-permission gate and an in-flight child-loop
//! cancellation branch. User-initiated `/task` and Code-Control
//! `task.dispatch` both route through `TaskEntryKind::UserInitiated`.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};

use futures::future::BoxFuture;

use super::{
    sub_agent::{
        BudgetExceededReason, CancellationSource, DispatchContext, PermissionAskRequest,
        PermissionAskSource, PermissionReply, SafetyDecisionDenial, SubAgentChildRunRequest,
        SubAgentChildRunner, SubAgentDispatcher, TaskEntryKind, TaskFailure, TaskInvocation,
        TaskResult, ToolLoopError,
    },
    tool_loop::{ToolLoopConfig, ToolLoopObserver, run_tool_loop_with_history_and_observer},
};
use crate::internal::ai::{
    agent::{
        BudgetAxis, BudgetExceededError, BudgetTracker,
        profile::{AgentExecutionSpec, AgentProfileRouter, ToolSelection, config::AgentsConfig},
    },
    agent_run::{AgentRunEvent, AgentRunId},
    completion::{CompletionError, CompletionUsageSummary, Message},
    permission::{
        EDIT_TOOLS, PermissionRuleset, agent_permission_spec_to_ruleset, assert_no_escalation,
        child_ruleset,
    },
    runtime::{PrincipalContext, ToolBoundaryPolicy, ToolOperation},
    session::{SessionState, jsonl::SessionEvent},
    usage::UsageContext,
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
    pub subagent_timeout_ms: Option<u64>,
}

impl Default for MultiAgentConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_subagent_depth: 1,
            max_concurrent_subagents: 1,
            subagent_timeout_ms: Some(600_000),
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

impl AgentSpecRegistry for AgentProfileRouter {
    fn lookup(&self, name: &str) -> Option<AgentExecutionSpec> {
        self.execution_spec(name)
    }

    fn registered_names(&self) -> Vec<String> {
        self.profiles()
            .iter()
            .map(|profile| profile.name.clone())
            .collect()
    }
}

/// Default dispatcher implementation. Holds a registry, a config, and
/// a shared concurrency counter that subsequent dispatches increment +
/// decrement around the gate.
pub struct DefaultSubAgentDispatcher {
    registry: Arc<dyn AgentSpecRegistry>,
    config: MultiAgentConfig,
    in_flight: Arc<AtomicU32>,
    child_runner: Arc<dyn SubAgentChildRunner>,
    budget: Option<SubAgentBudgetRuntime>,
}

#[derive(Clone)]
struct SubAgentBudgetRuntime {
    config: AgentsConfig,
    tracker: Arc<Mutex<BudgetTracker>>,
}

impl DefaultSubAgentDispatcher {
    pub fn new(registry: Arc<dyn AgentSpecRegistry>, config: MultiAgentConfig) -> Self {
        Self::new_with_child_runner(registry, config, Arc::new(ToolLoopSubAgentChildRunner))
    }

    pub fn new_with_child_runner(
        registry: Arc<dyn AgentSpecRegistry>,
        config: MultiAgentConfig,
        child_runner: Arc<dyn SubAgentChildRunner>,
    ) -> Self {
        Self::new_with_child_runner_and_budget(registry, config, child_runner, None)
    }

    pub fn new_with_budget_tracker(
        registry: Arc<dyn AgentSpecRegistry>,
        config: MultiAgentConfig,
        child_runner: Arc<dyn SubAgentChildRunner>,
        agents_config: AgentsConfig,
        tracker: Arc<Mutex<BudgetTracker>>,
    ) -> Self {
        Self::new_with_child_runner_and_budget(
            registry,
            config,
            child_runner,
            Some(SubAgentBudgetRuntime {
                config: agents_config,
                tracker,
            }),
        )
    }

    fn new_with_child_runner_and_budget(
        registry: Arc<dyn AgentSpecRegistry>,
        config: MultiAgentConfig,
        child_runner: Arc<dyn SubAgentChildRunner>,
        budget: Option<SubAgentBudgetRuntime>,
    ) -> Self {
        Self {
            registry,
            config,
            in_flight: Arc::new(AtomicU32::new(0)),
            child_runner,
            budget,
        }
    }

    /// Number of dispatches currently running (test introspection only).
    #[cfg(test)]
    pub fn in_flight(&self) -> u32 {
        self.in_flight.load(Ordering::Acquire)
    }

    /// Run the capability gates in order, returning the resolved child
    /// spec and effective ruleset or the first [`TaskFailure`] that
    /// fires.
    async fn run_capability_gates(
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

        // Step 5: SafetyDecision evaluate. A sub-agent spawn is a
        // mutating boundary operation because it may lead to child
        // tool calls and persistent session events even when the child
        // is read-only.
        evaluate_subagent_spawn_safety(ctx, invocation).await?;

        // Step 5b: budget hard caps. Budget checks live after
        // sub-agent resolution so per-agent caps can name the
        // resolved profile, but before child construction so an
        // already-over-cap session cannot start more provider work.
        self.enforce_budget_before_dispatch(&sub_spec.name)?;

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

    fn enforce_budget_before_dispatch(&self, agent_name: &str) -> Result<(), TaskFailure> {
        let Some(budget) = &self.budget else {
            return Ok(());
        };
        let tracker = budget.tracker.lock().map_err(|error| {
            TaskFailure::BudgetExceeded(BudgetExceededReason::Internal {
                reason: format!("budget tracker lock poisoned: {error}"),
            })
        })?;
        tracker
            .check_session(&budget.config)
            .and_then(|()| tracker.check_agent(agent_name, &budget.config))
            .map_err(budget_error_to_task_failure)
    }

    fn record_budget_after_success(&self, result: &TaskResult) {
        let Some(budget) = &self.budget else {
            return;
        };
        let Ok(mut tracker) = budget.tracker.lock() else {
            tracing::warn!("failed to lock sub-agent budget tracker after successful run");
            return;
        };
        tracker.accumulate(&result.usage, None, Some(&result.agent_name));
        for _ in 0..result.steps_used {
            tracker.record_step(Some(&result.agent_name));
        }
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
            // the end) decrements the counter exactly once. The child
            // loop does real provider/tool I/O after this point, so the
            // guard prevents an early failure from orphaning the slot.
            let _slot = ConcurrencyGuard {
                counter: Arc::clone(&self.in_flight),
            };

            // Steps 4-7: capability + permission gates that don't
            // touch the concurrency counter.
            let (sub_spec, effective) = self
                .run_capability_gates(&ctx, &invocation, entry_kind)
                .await?;

            if ctx.abort_token.is_cancelled() {
                return Err(TaskFailure::Cancelled {
                    source: CancellationSource::ParentAbort,
                });
            }

            // Step 8: permission ask. Per the doc's "Two Entry Points"
            // table, only `LlmInitiated` triggers the ask. **All**
            // `UserInitiated` variants — both `bypass_permission_ask:
            // true` and `false` — currently skip the dialog because
            // today's only `UserInitiated` call sites (slash command,
            // Code Control RPC, SubtaskPart payload arriving in P3.6)
            // set `bypass: true` by construction. P3.6 reviews
            // whether a `bypass: false` slash-command path is
            // actually meaningful; if so, this match arm widens to
            // include it.
            if let TaskEntryKind::LlmInitiated = entry_kind {
                let prompt_digest = digest_for_prompt(&invocation.prompt);
                let patterns = vec![invocation.subagent_type.clone()];
                let request = PermissionAskRequest {
                    permission: "task",
                    patterns: &patterns,
                    thread_id: ctx.parent_thread_id,
                    session_id: ctx.parent_session_id,
                    source: PermissionAskSource::SubAgentSpawn {
                        name: invocation.subagent_type.clone(),
                        prompt_digest,
                    },
                };
                match ctx.permission_service.ask(request).await {
                    PermissionReply::Once | PermissionReply::Always { .. } => {
                        // The dispatcher itself does not persist
                        // `Always` patterns — that is the responsibility
                        // of the asker implementation, which has access
                        // to the project's `ApprovedRulesetStore`.
                    }
                    PermissionReply::Reject { feedback } => {
                        return Err(TaskFailure::ApprovalRejected { feedback });
                    }
                }
            }

            let task_id = invocation.task_id.clone().unwrap_or_else(|| {
                format!(
                    "task-{}-{}-depth-{}",
                    invocation.subagent_type,
                    uuid::Uuid::new_v4(),
                    ctx.depth
                )
            });
            let agent_run_id = AgentRunId::new();
            append_agent_run_event(
                ctx.session_store,
                AgentRunEvent::Spawned {
                    agent_run_id,
                    parent_thread_id: ctx.parent_thread_id.to_string(),
                    parent_session_id: ctx.parent_session_id.clone(),
                    parent_message_id: ctx.parent_message_id.clone(),
                    subagent_name: sub_spec.name.clone(),
                    provider_id: sub_spec
                        .model
                        .as_ref()
                        .unwrap_or(ctx.parent_model_binding)
                        .provider_id
                        .clone(),
                    model_id: sub_spec
                        .model
                        .as_ref()
                        .unwrap_or(ctx.parent_model_binding)
                        .model_id
                        .clone(),
                    depth: ctx.depth.saturating_add(1),
                    prompt_digest: digest_for_prompt(&invocation.prompt),
                },
            )?;
            let result = run_child_runner_with_timeout(
                self.child_runner.as_ref(),
                SubAgentChildRunRequest {
                    ctx: &ctx,
                    invocation: &invocation,
                    sub_spec: &sub_spec,
                    effective_ruleset: &effective,
                    task_id: task_id.clone(),
                    agent_run_id,
                },
                self.config.subagent_timeout_ms,
            )
            .await;
            match &result {
                Ok(result) => {
                    self.record_budget_after_success(result);
                    append_agent_run_event(
                        ctx.session_store,
                        AgentRunEvent::Completed { agent_run_id },
                    )?;
                }
                Err(error) => {
                    if matches!(error, TaskFailure::Timeout { .. }) {
                        append_child_agent_run_failed(
                            ctx.session_store,
                            &task_id,
                            agent_run_id,
                            task_failure_event_reason(error),
                        )?;
                    }
                    append_agent_run_event(
                        ctx.session_store,
                        AgentRunEvent::Failed {
                            agent_run_id,
                            reason: task_failure_event_reason(error),
                        },
                    )?;
                }
            }
            let result = result?;
            // `_slot` drops here, releasing the concurrency slot.
            Ok(result)
        })
    }
}

/// Default tail runner for OC-Phase 3 P3.4: build the child model,
/// pre-filter the child tool schema, write a minimal child session JSONL
/// snapshot, and re-enter the normal tool loop with `task` disabled for
/// the child.
#[derive(Debug, Default)]
pub struct ToolLoopSubAgentChildRunner;

impl SubAgentChildRunner for ToolLoopSubAgentChildRunner {
    fn run<'a>(
        &'a self,
        request: SubAgentChildRunRequest<'a>,
    ) -> BoxFuture<'a, Result<TaskResult, TaskFailure>> {
        Box::pin(async move {
            let child_store = request.ctx.session_store.child(&request.task_id);
            let agent_run_id = request.agent_run_id;
            let result = run_child_tool_loop(request).await;
            if let Err(error) = &result {
                let _ = child_store.append(&SessionEvent::agent_run(AgentRunEvent::Failed {
                    agent_run_id,
                    reason: task_failure_event_reason(error),
                }));
            }
            result
        })
    }
}

async fn run_child_tool_loop(
    request: SubAgentChildRunRequest<'_>,
) -> Result<TaskResult, TaskFailure> {
    if request.ctx.abort_token.is_cancelled() {
        return Err(TaskFailure::Cancelled {
            source: CancellationSource::ParentAbort,
        });
    }

    let binding = request
        .sub_spec
        .model
        .as_ref()
        .unwrap_or(request.ctx.parent_model_binding)
        .clone();
    let provider_options = match request.ctx.provider_build_options_resolver {
        Some(resolver) => resolver.resolve(&binding).map_err(|error| {
            TaskFailure::ProviderError(CompletionError::ProviderError(format!(
                "failed to resolve provider options for {}: {error}",
                binding.to_canonical_string()
            )))
        })?,
        None => request.ctx.provider_build_options.clone(),
    };
    let model = request
        .ctx
        .provider_factory
        .build(&binding, provider_options)
        .map_err(|error| {
            TaskFailure::ProviderError(CompletionError::ProviderError(error.to_string()))
        })?;

    let allowed_tools = available_tool_names(
        request.ctx.tool_registry,
        request.sub_spec,
        request.effective_ruleset,
    );
    let child_store = request.ctx.session_store.child(&request.task_id);
    child_store
        .append(&SessionEvent::agent_run(AgentRunEvent::Started {
            agent_run_id: request.agent_run_id,
        }))
        .map_err(|error| {
            TaskFailure::ContextHandoffFailed(
                super::sub_agent::ContextHandoffError::CompactionFailed {
                    reason: format!("failed to append child agent-run start event: {error}"),
                },
            )
        })?;
    let child_working_dir = request
        .ctx
        .tool_registry
        .working_dir()
        .to_string_lossy()
        .to_string();
    let mut child_session = SessionState::new(&child_working_dir);
    child_session.id = request.task_id.clone();
    child_session.metadata.insert(
        "parent_thread_id".to_string(),
        serde_json::json!(request.ctx.parent_thread_id),
    );
    child_session.metadata.insert(
        "parent_session_id".to_string(),
        serde_json::json!(request.ctx.parent_session_id),
    );
    child_session.metadata.insert(
        "parent_message_id".to_string(),
        serde_json::json!(request.ctx.parent_message_id),
    );
    child_session.metadata.insert(
        "agent_name".to_string(),
        serde_json::json!(request.sub_spec.name),
    );
    child_session.add_user_message(&request.invocation.prompt);
    child_store
        .append(&SessionEvent::snapshot(child_session.clone()))
        .map_err(|error| {
            TaskFailure::ContextHandoffFailed(
                super::sub_agent::ContextHandoffError::CompactionFailed {
                    reason: format!("failed to append child session snapshot: {error}"),
                },
            )
        })?;

    let mut observer = ChildLoopObserver::default();
    let child_loop = run_tool_loop_with_history_and_observer(
        &model,
        Vec::new(),
        request.invocation.prompt.clone(),
        request.ctx.tool_registry,
        ToolLoopConfig {
            preamble: non_empty_string(request.sub_spec.system_prompt.clone()),
            temperature: request.sub_spec.temperature.map(f64::from),
            allowed_tools: Some(allowed_tools),
            runtime_context: request.ctx.runtime_context.clone(),
            max_turns: request
                .sub_spec
                .max_steps
                .and_then(|steps| usize::try_from(steps).ok()),
            subagent_runtime: None,
            context_frame_session_root: Some(child_store.session_root().to_path_buf()),
            context_frame_prompt_id: Some(format!("subagent:{}", request.task_id)),
            usage_recorder: Some(request.ctx.usage_recorder.clone()),
            usage_context: Some(UsageContext {
                session_id: Some(request.task_id.clone()),
                thread_id: Some(request.ctx.parent_thread_id.to_string()),
                agent_run_id: Some(request.task_id.clone()),
                run_id: Some(request.task_id.clone()),
                provider: binding.provider_id.clone(),
                model: binding.model_id.clone(),
                request_kind: "subagent".to_string(),
                intent: None,
                agent_name: Some(request.sub_spec.name.clone()),
            }),
            ..Default::default()
        },
        &mut observer,
    );
    let turn = tokio::select! {
        result = child_loop => {
            result.map_err(|error| TaskFailure::ChildToolLoopFailed(ToolLoopError::Completion(error)))?
        }
        _ = request.ctx.abort_token.cancelled() => {
            return Err(TaskFailure::Cancelled {
                source: CancellationSource::ParentAbort,
            });
        }
    };

    child_session.add_assistant_message(&turn.final_text);
    child_store
        .append(&SessionEvent::agent_run(AgentRunEvent::Completed {
            agent_run_id: request.agent_run_id,
        }))
        .map_err(|error| {
            TaskFailure::ContextHandoffFailed(
                super::sub_agent::ContextHandoffError::CompactionFailed {
                    reason: format!("failed to append child agent-run completion event: {error}"),
                },
            )
        })?;
    child_store
        .append(&SessionEvent::snapshot(child_session))
        .map_err(|error| {
            TaskFailure::ContextHandoffFailed(
                super::sub_agent::ContextHandoffError::CompactionFailed {
                    reason: format!("failed to append child session completion snapshot: {error}"),
                },
            )
        })?;

    Ok(TaskResult {
        task_id: request.task_id,
        agent_name: request.sub_spec.name.clone(),
        provider_id: binding.provider_id,
        model_id: binding.model_id,
        final_text: turn.final_text,
        steps_used: count_assistant_steps(&turn.history),
        usage: observer.usage,
    })
}

async fn run_child_runner_with_timeout<'a>(
    runner: &'a dyn SubAgentChildRunner,
    request: SubAgentChildRunRequest<'a>,
    timeout_ms: Option<u64>,
) -> Result<TaskResult, TaskFailure> {
    let run = runner.run(request);
    match timeout_ms {
        Some(timeout_ms) => {
            match tokio::time::timeout(Duration::from_millis(timeout_ms), run).await {
                Ok(result) => result,
                Err(_) => Err(TaskFailure::Timeout {
                    wall_clock_ms: timeout_ms,
                }),
            }
        }
        None => run.await,
    }
}

fn append_agent_run_event(
    store: &crate::internal::ai::session::jsonl::SessionJsonlStore,
    event: AgentRunEvent,
) -> Result<(), TaskFailure> {
    store
        .append(&SessionEvent::agent_run(event))
        .map_err(|error| {
            TaskFailure::ContextHandoffFailed(
                super::sub_agent::ContextHandoffError::CompactionFailed {
                    reason: format!("failed to append parent agent-run event: {error}"),
                },
            )
        })
}

fn append_child_agent_run_failed(
    store: &crate::internal::ai::session::jsonl::SessionJsonlStore,
    task_id: &str,
    agent_run_id: AgentRunId,
    reason: String,
) -> Result<(), TaskFailure> {
    store
        .child(task_id)
        .append(&SessionEvent::agent_run(AgentRunEvent::Failed {
            agent_run_id,
            reason,
        }))
        .map_err(|error| {
            TaskFailure::ContextHandoffFailed(
                super::sub_agent::ContextHandoffError::CompactionFailed {
                    reason: format!("failed to append child agent-run timeout event: {error}"),
                },
            )
        })
}

fn task_failure_event_reason(error: &TaskFailure) -> String {
    match error {
        TaskFailure::Cancelled { source } => format!("Cancelled({source:?})"),
        _ => error.to_string(),
    }
}

fn budget_error_to_task_failure(error: BudgetExceededError) -> TaskFailure {
    let reason = match error.axis {
        BudgetAxis::Cost => BudgetExceededReason::CostHardCap,
        BudgetAxis::Tokens => BudgetExceededReason::TokenHardCap,
        BudgetAxis::Steps => BudgetExceededReason::Steps,
        BudgetAxis::WallClockMinutes => BudgetExceededReason::WallClock,
    };
    tracing::warn!(
        stable_code = %error.stable_code().as_str(),
        scope = ?error.scope,
        axis = ?error.axis,
        "sub-agent dispatch rejected by budget hard cap"
    );
    TaskFailure::BudgetExceeded(reason)
}

#[derive(Default)]
struct ChildLoopObserver {
    usage: CompletionUsageSummary,
}

impl ToolLoopObserver for ChildLoopObserver {
    fn on_model_usage_recorded(&mut self, usage: &CompletionUsageSummary, _wall_clock_ms: u64) {
        self.usage.merge(usage);
    }
}

fn available_tool_names(
    registry: &crate::internal::ai::tools::ToolRegistry,
    sub_spec: &AgentExecutionSpec,
    effective_ruleset: &PermissionRuleset,
) -> Vec<String> {
    let mut effective_spec;
    let spec = match &sub_spec.tools {
        ToolSelection::Inherit => {
            effective_spec = sub_spec.clone();
            effective_spec.tools = ToolSelection::Allow(Vec::new());
            &effective_spec
        }
        _ => sub_spec,
    };
    registry
        .available_for(spec, effective_ruleset)
        .into_iter()
        .map(|spec| spec.function.name)
        .collect()
}

fn non_empty_string(value: String) -> Option<String> {
    let value = value.trim().to_string();
    if value.is_empty() { None } else { Some(value) }
}

fn count_assistant_steps(history: &[Message]) -> u32 {
    let count = history
        .iter()
        .filter(|message| matches!(message, Message::Assistant { .. }))
        .count();
    u32::try_from(count).unwrap_or(u32::MAX)
}

async fn evaluate_subagent_spawn_safety(
    ctx: &DispatchContext<'_>,
    invocation: &TaskInvocation,
) -> Result<(), TaskFailure> {
    let prompt_digest = digest_for_prompt(&invocation.prompt);
    let operation = ToolOperation::sub_agent_spawn(&invocation.subagent_type, &prompt_digest);
    let decision = if let Some(hardening) = ctx.tool_registry.hardening() {
        let decision = hardening.decide(&operation);
        hardening
            .append_audit(
                "tool_boundary.task",
                format!(
                    "operation=sub_agent_spawn name={} prompt_digest={} decision={} approval_required={} reason={}",
                    invocation.subagent_type,
                    prompt_digest,
                    if decision.allowed { "allow" } else { "deny" },
                    decision.approval_required,
                    decision.reason
                ),
            )
            .await
            .map_err(|error| {
                TaskFailure::SafetyDenied(SafetyDecisionDenial {
                    reason: format!("failed to persist sub-agent boundary audit event: {error}"),
                })
            })?;
        hardening.flush_audit().await.map_err(|error| {
            TaskFailure::SafetyDenied(SafetyDecisionDenial {
                reason: format!("failed to flush sub-agent boundary audit event: {error}"),
            })
        })?;
        decision
    } else {
        ToolBoundaryPolicy::default_runtime().decide(&PrincipalContext::system(), &operation)
    };

    if !decision.allowed {
        return Err(TaskFailure::SafetyDenied(SafetyDecisionDenial {
            reason: decision.reason,
        }));
    }

    Ok(())
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

/// Produce a short, human-readable preview of a prompt for the
/// permission ask UI. Cap at the first line and 80 characters so the
/// digest fits in a one-line prompt header. Not a cryptographic hash —
/// the goal is "enough to recognise the dispatch in a log", not
/// uniqueness.
fn digest_for_prompt(prompt: &str) -> String {
    let first_line = prompt.lines().next().unwrap_or("").trim();
    if first_line.chars().count() <= 80 {
        first_line.to_string()
    } else {
        let truncated: String = first_line.chars().take(77).collect();
        format!("{truncated}…")
    }
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

    use futures::future::BoxFuture;
    use sea_orm::Database;

    use super::*;
    #[cfg(feature = "test-provider")]
    use crate::internal::ai::agent::runtime::sub_agent::ProviderBuildOptionsResolver;
    use crate::internal::ai::{
        agent::{
            profile::{
                AgentExecutionSpec, AgentMode, AgentPermissionSpec, ApprovalRoutingSpec,
                BudgetConfig, ModelBinding, ToolSelection,
            },
            runtime::sub_agent::{
                AbortToken, ContextFrameLoader, DispatchContext, MessageId, PermissionAskRequest,
                PermissionAsker, PermissionReply, PermissionService, SubAgentChildRunRequest,
                SubAgentChildRunner, SubAgentDispatcher, TaskEntryKind, TaskFailure,
                TaskInvocation, TaskResult,
            },
        },
        permission::{PermissionAction, PermissionRule, PermissionRuleset},
        providers::{ProviderBuildOptions, ProviderFactory},
        runtime::{
            InMemoryAuditSink, PrincipalContext, PrincipalRole, SecretRedactor, ToolBoundaryPolicy,
            ToolBoundaryRuntime,
        },
        session::SessionId,
        tools::{ToolRegistry, ToolRegistryBuilder, handlers::ReadFileHandler},
        usage::UsageRecorder,
    };

    /// Test asker that replies with a pre-canned [`PermissionReply`]
    /// and counts how many times `ask()` was invoked. The counter
    /// pins the doc's "ask only on `LlmInitiated`" rule.
    struct TestAsker {
        reply: PermissionReply,
        ask_calls: Mutex<u32>,
    }

    impl TestAsker {
        fn always(reply: PermissionReply) -> Self {
            Self {
                reply,
                ask_calls: Mutex::new(0),
            }
        }
        fn ask_call_count(&self) -> u32 {
            *self.ask_calls.lock().unwrap()
        }
    }

    impl PermissionAsker for TestAsker {
        fn ask<'a>(&'a self, _request: PermissionAskRequest<'a>) -> BoxFuture<'a, PermissionReply> {
            *self.ask_calls.lock().unwrap() += 1;
            let reply = self.reply.clone();
            Box::pin(async move { reply })
        }
    }

    #[derive(Default)]
    struct TestChildRunner {
        calls: Mutex<Vec<String>>,
    }

    impl SubAgentChildRunner for TestChildRunner {
        fn run<'a>(
            &'a self,
            request: SubAgentChildRunRequest<'a>,
        ) -> BoxFuture<'a, Result<TaskResult, TaskFailure>> {
            Box::pin(async move {
                self.calls.lock().unwrap().push(request.task_id.clone());
                let binding = request
                    .sub_spec
                    .model
                    .as_ref()
                    .unwrap_or(request.ctx.parent_model_binding);
                Ok(TaskResult {
                    task_id: request.task_id,
                    agent_name: request.sub_spec.name.clone(),
                    provider_id: binding.provider_id.clone(),
                    model_id: binding.model_id.clone(),
                    final_text: format!("child result for {}", request.invocation.description),
                    steps_used: 1,
                    usage: CompletionUsageSummary::default(),
                })
            })
        }
    }

    struct FailingChildRunner;

    impl SubAgentChildRunner for FailingChildRunner {
        fn run<'a>(
            &'a self,
            _request: SubAgentChildRunRequest<'a>,
        ) -> BoxFuture<'a, Result<TaskResult, TaskFailure>> {
            Box::pin(async {
                Err(TaskFailure::ProviderError(CompletionError::ProviderError(
                    "fixture child failure".to_string(),
                )))
            })
        }
    }

    struct SlowChildRunner;

    impl SubAgentChildRunner for SlowChildRunner {
        fn run<'a>(
            &'a self,
            _request: SubAgentChildRunRequest<'a>,
        ) -> BoxFuture<'a, Result<TaskResult, TaskFailure>> {
            Box::pin(async move {
                tokio::time::sleep(Duration::from_secs(60)).await;
                Err(TaskFailure::ProviderError(CompletionError::ProviderError(
                    "slow fixture should be timed out".to_string(),
                )))
            })
        }
    }

    struct CostingChildRunner;

    impl SubAgentChildRunner for CostingChildRunner {
        fn run<'a>(
            &'a self,
            request: SubAgentChildRunRequest<'a>,
        ) -> BoxFuture<'a, Result<TaskResult, TaskFailure>> {
            Box::pin(async move {
                Ok(TaskResult {
                    task_id: request.task_id,
                    agent_name: request.sub_spec.name.clone(),
                    provider_id: "fake".to_string(),
                    model_id: "default".to_string(),
                    final_text: "budgeted child result".to_string(),
                    steps_used: 1,
                    usage: CompletionUsageSummary {
                        input_tokens: 1,
                        output_tokens: 1,
                        cached_tokens: None,
                        reasoning_tokens: None,
                        total_tokens: Some(2),
                        cost_usd: Some(0.02),
                    },
                })
            })
        }
    }

    #[cfg(feature = "test-provider")]
    #[derive(Clone)]
    struct TestProviderOptionsResolver {
        options: ProviderBuildOptions,
    }

    #[cfg(feature = "test-provider")]
    impl ProviderBuildOptionsResolver for TestProviderOptionsResolver {
        fn resolve(&self, _binding: &ModelBinding) -> Result<ProviderBuildOptions, String> {
            Ok(self.options.clone())
        }
    }

    /// Build a [`PermissionService`] backed by a fresh `TestAsker` and
    /// hand back a clone of the asker so the test can assert the call
    /// count after dispatch.
    fn allow_once_service() -> (PermissionService, Arc<TestAsker>) {
        let asker = Arc::new(TestAsker::always(PermissionReply::Once));
        let service = PermissionService::new(asker.clone());
        (service, asker)
    }

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
        provider_build_options: &'a ProviderBuildOptions,
        tool_registry: &'a ToolRegistry,
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
            provider_build_options,
            provider_build_options_resolver: None,
            tool_registry,
            runtime_context: None,
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
        ProviderBuildOptions,
        ToolRegistry,
    ) {
        dispatcher_test_harness_with_runner(config, Arc::new(TestChildRunner::default())).await
    }

    async fn dispatcher_test_harness_with_runner(
        config: MultiAgentConfig,
        child_runner: Arc<dyn SubAgentChildRunner>,
    ) -> (
        DefaultSubAgentDispatcher,
        Arc<TestRegistry>,
        UsageRecorder,
        crate::internal::ai::session::jsonl::SessionJsonlStore,
        ProviderBuildOptions,
        ToolRegistry,
    ) {
        let registry = Arc::new(TestRegistry::default());
        let dispatcher = DefaultSubAgentDispatcher::new_with_child_runner(
            registry.clone(),
            config,
            child_runner,
        );
        let conn = Database::connect("sqlite::memory:").await.unwrap();
        let usage_recorder = UsageRecorder::new(conn);
        let temp = tempfile::tempdir().unwrap();
        let store =
            crate::internal::ai::session::jsonl::SessionJsonlStore::new(temp.path().to_path_buf());
        let tool_registry = ToolRegistry::with_working_dir(temp.path().to_path_buf());
        // Leak the temp dir so the SessionJsonlStore reference remains
        // valid for the test duration.
        std::mem::forget(temp);
        (
            dispatcher,
            registry,
            usage_recorder,
            store,
            ProviderBuildOptions::default(),
            tool_registry,
        )
    }

    fn invocation(subagent_type: &str) -> TaskInvocation {
        TaskInvocation {
            description: "test invocation".to_string(),
            prompt: "do a thing".to_string(),
            subagent_type: subagent_type.to_string(),
            task_id: None,
        }
    }

    #[test]
    fn available_tool_names_treats_subagent_inherit_as_empty_allow_list() {
        let temp = tempfile::tempdir().unwrap();
        let registry = ToolRegistryBuilder::with_working_dir(temp.path().to_path_buf())
            .register("read_file", Arc::new(ReadFileHandler))
            .build();
        let mut spec = explore_subagent();
        spec.tools = ToolSelection::Inherit;

        let allowed = available_tool_names(&registry, &spec, &PermissionRuleset::default());

        assert!(
            allowed.is_empty(),
            "sub-agent ToolSelection::Inherit must default to no tools"
        );
    }

    /// Scenario: with `multi_agent.enabled = false`, the dispatcher
    /// rejects every dispatch with `FeatureDisabled`. This is the
    /// flag-off invariant — even if the tool slipped past the
    /// registry-level filter, the dispatcher still refuses with a
    /// dedicated variant (not `SafetyDenied`, which is reserved for
    /// step-5 sandbox rejections in P3.4).
    #[tokio::test]
    async fn dispatch_rejects_when_feature_flag_disabled() {
        let (dispatcher, registry, usage, store, provider_options, tool_registry) =
            dispatcher_test_harness(MultiAgentConfig::default()).await;
        registry.insert(explore_subagent());

        let parent = parent_spec();
        let parent_ruleset: PermissionRuleset = Vec::new();
        let parent_binding = parent_binding();
        let (permission_service, _asker) = allow_once_service();
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
            &provider_options,
            &tool_registry,
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
            subagent_timeout_ms: None,
        };
        let (dispatcher, registry, usage, store, provider_options, tool_registry) =
            dispatcher_test_harness(config).await;
        registry.insert(explore_subagent());

        let parent = parent_spec();
        let parent_ruleset: PermissionRuleset = Vec::new();
        let parent_binding = parent_binding();
        let (permission_service, _asker) = allow_once_service();
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
            &provider_options,
            &tool_registry,
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
            subagent_timeout_ms: None,
        };
        let (dispatcher, registry, usage, store, provider_options, tool_registry) =
            dispatcher_test_harness(config).await;
        registry.insert(explore_subagent());
        // Pre-occupy the only slot.
        dispatcher.in_flight.fetch_add(1, Ordering::AcqRel);

        let parent = parent_spec();
        let parent_ruleset: PermissionRuleset = Vec::new();
        let parent_binding = parent_binding();
        let (permission_service, _asker) = allow_once_service();
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
            &provider_options,
            &tool_registry,
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
            subagent_timeout_ms: None,
        };
        let (dispatcher, registry, usage, store, provider_options, tool_registry) =
            dispatcher_test_harness(config).await;
        registry.insert(explore_subagent()); // mode = Subagent
        registry.insert(primary_only_agent()); // mode = Primary

        let parent = parent_spec();
        let parent_ruleset: PermissionRuleset = Vec::new();
        let parent_binding = parent_binding();
        let (permission_service, _asker) = allow_once_service();
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
            &provider_options,
            &tool_registry,
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
            subagent_timeout_ms: None,
        };
        let (dispatcher, registry, usage, store, provider_options, tool_registry) =
            dispatcher_test_harness(config).await;

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
        let (permission_service, _asker) = allow_once_service();
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
            &provider_options,
            &tool_registry,
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

    /// Scenario (OC-Phase 3 P3.4 step 5): the dispatcher evaluates a
    /// `SubAgentSpawn` safety operation before the permission ask. An
    /// Observer-bound runtime is read-only, so a `task` spawn is denied,
    /// audited with redaction, and the asker is not invoked.
    #[tokio::test]
    async fn dispatch_rejects_subagent_spawn_denied_by_safety_policy() {
        let config = MultiAgentConfig {
            enabled: true,
            max_subagent_depth: 4,
            max_concurrent_subagents: 4,
            subagent_timeout_ms: None,
        };
        let (dispatcher, registry, usage, store, provider_options, tool_registry) =
            dispatcher_test_harness(config).await;
        registry.insert(explore_subagent());

        let audit_sink = Arc::new(InMemoryAuditSink::default());
        let hardened_tool_registry =
            tool_registry
                .clone()
                .with_hardening(ToolBoundaryRuntime::new(
                    uuid::Uuid::new_v4(),
                    PrincipalContext {
                        principal_id: "readonly-user".to_string(),
                        role: PrincipalRole::Observer,
                    },
                    ToolBoundaryPolicy::default_runtime(),
                    SecretRedactor::default_runtime(),
                    audit_sink.clone(),
                ));
        let parent = parent_spec();
        let parent_ruleset: PermissionRuleset = Vec::new();
        let parent_binding = parent_binding();
        let (permission_service, asker) = allow_once_service();
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
            &provider_options,
            &hardened_tool_registry,
            0,
        );

        let mut request = invocation("explore");
        request.prompt = "inspect token=super-secret".to_string();
        let err = dispatcher
            .dispatch(context, request, TaskEntryKind::LlmInitiated)
            .await
            .expect_err("observer-bound safety policy must deny sub-agent spawn");

        match err {
            TaskFailure::SafetyDenied(denial) => {
                assert!(denial.reason.contains("observer principals cannot run"));
            }
            other => panic!("expected SafetyDenied, got {other:?}"),
        }
        assert_eq!(
            asker.ask_call_count(),
            0,
            "safety denial must happen before permission.ask"
        );
        assert_eq!(dispatcher.in_flight(), 0);

        let events = audit_sink.events().await;
        assert_eq!(events.len(), 1, "safety decision must be audited");
        assert_eq!(events[0].action, "tool_boundary.task");
        assert!(
            events[0]
                .redacted_summary
                .contains("operation=sub_agent_spawn")
        );
        assert!(!events[0].redacted_summary.contains("super-secret"));
        assert!(
            store
                .load_events()
                .expect("parent events load after safety denial")
                .is_empty(),
            "safety denial happens before AgentRunEvent::Spawned"
        );
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
            subagent_timeout_ms: None,
        };
        let (dispatcher, registry, usage, store, provider_options, tool_registry) =
            dispatcher_test_harness(config).await;
        registry.insert(explore_subagent());
        // Hold the only slot for the entire test — both concurrent
        // dispatches will see `prev = 1, limit = 1` and roll back.
        // (Acquiring the slot via fetch_add reproduces what a real
        // in-flight dispatch would do.)
        dispatcher.in_flight.fetch_add(1, Ordering::AcqRel);

        let parent = parent_spec();
        let parent_ruleset: PermissionRuleset = Vec::new();
        let parent_binding = parent_binding();
        let (permission_service, _asker) = allow_once_service();
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
            &provider_options,
            &tool_registry,
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
            &provider_options,
            &tool_registry,
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

    /// Scenario (OC-Phase 3 P3.4 step 8 — Reject path): a
    /// `LlmInitiated` dispatch whose permission ask returns `Reject`
    /// surfaces `TaskFailure::ApprovalRejected`, with the user's
    /// optional feedback forwarded so the caller can show it to the
    /// model. The concurrency counter releases via the RAII guard.
    #[tokio::test]
    async fn dispatch_returns_approval_rejected_when_asker_rejects() {
        let config = MultiAgentConfig {
            enabled: true,
            max_subagent_depth: 4,
            max_concurrent_subagents: 4,
            subagent_timeout_ms: None,
        };
        let (dispatcher, registry, usage, store, provider_options, tool_registry) =
            dispatcher_test_harness(config).await;
        registry.insert(explore_subagent());

        let asker = Arc::new(TestAsker::always(PermissionReply::Reject {
            feedback: Some("budget concerns".to_string()),
        }));
        let permission_service = PermissionService::new(asker.clone());

        let parent = parent_spec();
        let parent_ruleset: PermissionRuleset = Vec::new();
        let parent_binding = parent_binding();
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
            &provider_options,
            &tool_registry,
            0,
        );

        let result = dispatcher
            .dispatch(context, invocation("explore"), TaskEntryKind::LlmInitiated)
            .await;
        match result {
            Err(TaskFailure::ApprovalRejected { feedback }) => {
                assert_eq!(feedback.as_deref(), Some("budget concerns"));
            }
            other => panic!("expected ApprovalRejected, got {other:?}"),
        }
        assert_eq!(asker.ask_call_count(), 1);
        // RAII guard must have released the slot.
        assert_eq!(dispatcher.in_flight(), 0);
    }

    /// Scenario (P3.4 step 8 — Once allow path): an asker that replies
    /// `Once` lets the dispatch through to the child-runner tail. The
    /// asker is invoked exactly once, regardless of `Once` vs
    /// `Always` (the asker, not the dispatcher, persists `Always`
    /// rules).
    #[tokio::test]
    async fn dispatch_proceeds_when_asker_replies_once() {
        let config = MultiAgentConfig {
            enabled: true,
            max_subagent_depth: 4,
            max_concurrent_subagents: 4,
            subagent_timeout_ms: None,
        };
        let (dispatcher, registry, usage, store, provider_options, tool_registry) =
            dispatcher_test_harness(config).await;
        registry.insert(explore_subagent());

        let (permission_service, asker) = allow_once_service();
        let parent = parent_spec();
        let parent_ruleset: PermissionRuleset = Vec::new();
        let parent_binding = parent_binding();
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
            &provider_options,
            &tool_registry,
            0,
        );

        let result = dispatcher
            .dispatch(context, invocation("explore"), TaskEntryKind::LlmInitiated)
            .await
            .expect("Once should let the dispatch through");
        assert_eq!(result.agent_name, "explore");
        assert_eq!(asker.ask_call_count(), 1, "ask must fire on LlmInitiated");
    }

    /// Scenario (P3.4 step 8 — UserInitiated bypass path): a
    /// `UserInitiated { bypass_permission_ask: true }` dispatch
    /// MUST NOT call the asker. The user already chose the dispatch
    /// (slash command, Code Control RPC, SubtaskPart payload), so the
    /// dialog would be redundant. Even an asker that always rejects
    /// would not fire.
    #[tokio::test]
    async fn dispatch_user_initiated_bypass_skips_permission_ask() {
        let config = MultiAgentConfig {
            enabled: true,
            max_subagent_depth: 4,
            max_concurrent_subagents: 4,
            subagent_timeout_ms: None,
        };
        let (dispatcher, registry, usage, store, provider_options, tool_registry) =
            dispatcher_test_harness(config).await;
        registry.insert(explore_subagent());

        let asker = Arc::new(TestAsker::always(PermissionReply::Reject {
            feedback: Some("would have rejected".to_string()),
        }));
        let permission_service = PermissionService::new(asker.clone());

        let parent = parent_spec();
        let parent_ruleset: PermissionRuleset = Vec::new();
        let parent_binding = parent_binding();
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
            &provider_options,
            &tool_registry,
            0,
        );

        let result = dispatcher
            .dispatch(
                context,
                invocation("explore"),
                TaskEntryKind::UserInitiated {
                    bypass_permission_ask: true,
                },
            )
            .await
            .expect("UserInitiated bypass must not fail at step 8");
        assert_eq!(result.agent_name, "explore");
        assert_eq!(
            asker.ask_call_count(),
            0,
            "UserInitiated bypass must NOT call the asker"
        );
    }

    /// Scenario: every gate passes → the child runner result flows
    /// through with the resolved provider/model bound to the agent's
    /// spec. The concurrency counter returns to 0 after the call.
    #[tokio::test]
    async fn dispatch_returns_child_runner_result_when_every_gate_passes() {
        let config = MultiAgentConfig {
            enabled: true,
            max_subagent_depth: 4,
            max_concurrent_subagents: 4,
            subagent_timeout_ms: None,
        };
        let (dispatcher, registry, usage, store, provider_options, tool_registry) =
            dispatcher_test_harness(config).await;

        let mut sub = explore_subagent();
        sub.model = ModelBinding::parse("anthropic/claude-3-5-haiku-latest");
        registry.insert(sub);

        let parent = parent_spec();
        let parent_ruleset: PermissionRuleset = Vec::new();
        let parent_binding = parent_binding();
        let (permission_service, _asker) = allow_once_service();
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
            &provider_options,
            &tool_registry,
            0,
        );

        let result = dispatcher
            .dispatch(context, invocation("explore"), TaskEntryKind::LlmInitiated)
            .await
            .expect("every gate should pass");

        assert_eq!(result.agent_name, "explore");
        assert_eq!(result.provider_id, "anthropic");
        assert_eq!(result.model_id, "claude-3-5-haiku-latest");
        assert_eq!(result.final_text, "child result for test invocation");
        assert_eq!(result.steps_used, 1);

        let events = store.load_events().expect("parent events load");
        let lifecycle: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::AgentRun(envelope) => envelope.known(),
                _ => None,
            })
            .collect();
        assert_eq!(lifecycle.len(), 2, "parent lifecycle events: {lifecycle:?}");
        let spawned_run_id = match lifecycle[0] {
            AgentRunEvent::Spawned {
                agent_run_id,
                parent_thread_id: event_thread,
                parent_session_id: event_session,
                subagent_name,
                provider_id,
                model_id,
                depth,
                ..
            } => {
                assert_eq!(event_thread, "thread-1");
                assert_eq!(event_session, "session-1");
                assert_eq!(subagent_name, "explore");
                assert_eq!(provider_id, "anthropic");
                assert_eq!(model_id, "claude-3-5-haiku-latest");
                assert_eq!(*depth, 1);
                *agent_run_id
            }
            other => panic!("expected Spawned event, got {other:?}"),
        };
        match lifecycle[1] {
            AgentRunEvent::Completed { agent_run_id } => {
                assert_eq!(*agent_run_id, spawned_run_id);
            }
            other => panic!("expected Completed event, got {other:?}"),
        }

        // Concurrency counter must return to 0 after the call.
        assert_eq!(dispatcher.in_flight(), 0);
    }

    /// Scenario (OC-Phase 3 P3.5 failure event): once the dispatcher has
    /// spawned a child run, a child-runner error must be reflected in
    /// the parent JSONL as `AgentRunEvent::Failed` with the same
    /// `agent_run_id` as the preceding `Spawned` event.
    #[tokio::test]
    async fn dispatch_writes_failed_agent_run_event_when_child_runner_fails() {
        let config = MultiAgentConfig {
            enabled: true,
            max_subagent_depth: 4,
            max_concurrent_subagents: 4,
            subagent_timeout_ms: None,
        };
        let (dispatcher, registry, usage, store, provider_options, tool_registry) =
            dispatcher_test_harness_with_runner(config, Arc::new(FailingChildRunner)).await;
        registry.insert(explore_subagent());

        let parent = parent_spec();
        let parent_ruleset: PermissionRuleset = Vec::new();
        let parent_binding = parent_binding();
        let (permission_service, _asker) = allow_once_service();
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
            &provider_options,
            &tool_registry,
            0,
        );

        let err = dispatcher
            .dispatch(context, invocation("explore"), TaskEntryKind::LlmInitiated)
            .await
            .expect_err("fixture child runner must fail");
        assert!(matches!(err, TaskFailure::ProviderError(_)));

        let events = store.load_events().expect("parent events load");
        let lifecycle: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::AgentRun(envelope) => envelope.known(),
                _ => None,
            })
            .collect();
        assert_eq!(lifecycle.len(), 2, "parent lifecycle events: {lifecycle:?}");
        let spawned_run_id = match lifecycle[0] {
            AgentRunEvent::Spawned { agent_run_id, .. } => *agent_run_id,
            other => panic!("expected Spawned event, got {other:?}"),
        };
        match lifecycle[1] {
            AgentRunEvent::Failed {
                agent_run_id,
                reason,
            } => {
                assert_eq!(*agent_run_id, spawned_run_id);
                assert!(reason.contains("fixture child failure"));
            }
            other => panic!("expected Failed event, got {other:?}"),
        }
        assert_eq!(dispatcher.in_flight(), 0);
    }

    /// Scenario (OC-Phase 3 P3.7 timeout): a child runner that does
    /// not finish inside `subagent_timeout_ms` returns
    /// `TaskFailure::Timeout`, writes parent `Spawned` + `Failed`, and
    /// also appends a child-side `Failed` event so resume/replay sees
    /// the child run as cleaned up.
    #[tokio::test]
    async fn dispatch_times_out_child_runner_and_writes_failed_events() {
        let config = MultiAgentConfig {
            enabled: true,
            max_subagent_depth: 4,
            max_concurrent_subagents: 4,
            subagent_timeout_ms: Some(10),
        };
        let (dispatcher, registry, usage, store, provider_options, tool_registry) =
            dispatcher_test_harness_with_runner(config, Arc::new(SlowChildRunner)).await;
        registry.insert(explore_subagent());

        let parent = parent_spec();
        let parent_ruleset: PermissionRuleset = Vec::new();
        let parent_binding = parent_binding();
        let (permission_service, _asker) = allow_once_service();
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
            &provider_options,
            &tool_registry,
            0,
        );

        let err = dispatcher
            .dispatch(
                context,
                TaskInvocation {
                    task_id: Some("task-timeout".to_string()),
                    ..invocation("explore")
                },
                TaskEntryKind::LlmInitiated,
            )
            .await
            .expect_err("slow child runner must time out");
        assert!(matches!(err, TaskFailure::Timeout { wall_clock_ms: 10 }));

        let parent_events = store.load_events().expect("parent events load");
        let parent_lifecycle: Vec<_> = parent_events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::AgentRun(envelope) => envelope.known(),
                _ => None,
            })
            .collect();
        assert_eq!(parent_lifecycle.len(), 2);
        let run_id = match parent_lifecycle[0] {
            AgentRunEvent::Spawned { agent_run_id, .. } => *agent_run_id,
            other => panic!("expected parent Spawned, got {other:?}"),
        };
        match parent_lifecycle[1] {
            AgentRunEvent::Failed {
                agent_run_id,
                reason,
            } => {
                assert_eq!(*agent_run_id, run_id);
                assert!(reason.contains("timed out"));
            }
            other => panic!("expected parent Failed, got {other:?}"),
        }

        let child_events = store
            .child("task-timeout")
            .load_events()
            .expect("child events load");
        let child_lifecycle: Vec<_> = child_events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::AgentRun(envelope) => envelope.known(),
                _ => None,
            })
            .collect();
        assert_eq!(child_lifecycle.len(), 1);
        match child_lifecycle[0] {
            AgentRunEvent::Failed {
                agent_run_id,
                reason,
            } => {
                assert_eq!(*agent_run_id, run_id);
                assert!(reason.contains("timed out"));
            }
            other => panic!("expected child Failed, got {other:?}"),
        }
        assert_eq!(dispatcher.in_flight(), 0);
    }

    /// Scenario (OC-Phase 3 P3.7 budget hard cap): successful child
    /// usage is folded into the dispatcher's budget tracker. Once the
    /// configured session cap is exceeded, the next dispatch is
    /// rejected before `permission.ask` and before any child runner
    /// starts.
    #[tokio::test]
    async fn dispatch_rejects_next_child_when_budget_hard_cap_is_already_exceeded() {
        let config = MultiAgentConfig {
            enabled: true,
            max_subagent_depth: 4,
            max_concurrent_subagents: 4,
            subagent_timeout_ms: None,
        };
        let registry = Arc::new(TestRegistry::default());
        let tracker = Arc::new(Mutex::new(BudgetTracker::new()));
        let agents_config = AgentsConfig {
            budget: BudgetConfig {
                max_session_cost_usd: Some(0.01),
                ..BudgetConfig::default()
            },
            ..AgentsConfig::default()
        };
        let dispatcher = DefaultSubAgentDispatcher::new_with_budget_tracker(
            registry.clone(),
            config,
            Arc::new(CostingChildRunner),
            agents_config,
            tracker,
        );
        let conn = Database::connect("sqlite::memory:").await.unwrap();
        let usage = UsageRecorder::new(conn);
        let temp = tempfile::tempdir().unwrap();
        let store =
            crate::internal::ai::session::jsonl::SessionJsonlStore::new(temp.path().to_path_buf());
        let tool_registry = ToolRegistry::with_working_dir(temp.path().to_path_buf());
        registry.insert(explore_subagent());

        let parent = parent_spec();
        let parent_ruleset: PermissionRuleset = Vec::new();
        let parent_binding = parent_binding();
        let (permission_service, asker) = allow_once_service();
        let provider_factory = ProviderFactory;
        let context_frame_loader = ContextFrameLoader::default();
        let provider_options = ProviderBuildOptions::default();
        let parent_thread = "thread-1".to_string();
        let parent_session: SessionId = "session-1".to_string();

        let first_context = ctx(
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
            &provider_options,
            &tool_registry,
            0,
        );
        dispatcher
            .dispatch(
                first_context,
                invocation("explore"),
                TaskEntryKind::LlmInitiated,
            )
            .await
            .expect("first dispatch starts under cap and records usage");

        let second_context = ctx(
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
            &provider_options,
            &tool_registry,
            0,
        );
        let err = dispatcher
            .dispatch(
                second_context,
                invocation("explore"),
                TaskEntryKind::LlmInitiated,
            )
            .await
            .expect_err("second dispatch must be rejected by pre-dispatch budget cap");

        assert!(matches!(
            err,
            TaskFailure::BudgetExceeded(BudgetExceededReason::CostHardCap)
        ));
        assert_eq!(
            asker.ask_call_count(),
            1,
            "budget denial must happen before the second permission.ask"
        );
        assert_eq!(dispatcher.in_flight(), 0);
    }

    /// Scenario (P3.7 parent-cancel gate): if the parent session has
    /// already cancelled before the dispatcher reaches the permission
    /// prompt, dispatch returns a structured cancellation and does not
    /// ask the user for approval.
    #[tokio::test]
    async fn dispatch_rejects_pre_cancelled_parent_without_permission_ask() {
        let config = MultiAgentConfig {
            enabled: true,
            max_subagent_depth: 4,
            max_concurrent_subagents: 4,
            subagent_timeout_ms: None,
        };
        let (dispatcher, registry, usage, store, provider_options, tool_registry) =
            dispatcher_test_harness(config).await;
        registry.insert(explore_subagent());

        let parent = parent_spec();
        let parent_ruleset: PermissionRuleset = Vec::new();
        let parent_binding = parent_binding();
        let provider_factory = ProviderFactory;
        let context_frame_loader = ContextFrameLoader::default();
        let (permission_service, asker) = allow_once_service();
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
            &provider_options,
            &tool_registry,
            0,
        );
        context.abort_token.cancel();

        let err = dispatcher
            .dispatch(context, invocation("explore"), TaskEntryKind::LlmInitiated)
            .await
            .expect_err("pre-cancelled parent must stop dispatch");

        assert!(matches!(
            err,
            TaskFailure::Cancelled {
                source: CancellationSource::ParentAbort
            }
        ));
        assert_eq!(asker.ask_call_count(), 0);
        assert_eq!(dispatcher.in_flight(), 0);
    }

    /// Scenario (P3.4 steps 9-13): the default child runner builds the
    /// child model through `ProviderFactory`, re-enters the normal tool
    /// loop, returns the child's final text, and writes the child
    /// session under the parent JSONL store's subagent namespace.
    #[cfg(feature = "test-provider")]
    #[tokio::test]
    async fn default_child_runner_executes_fake_provider_child_loop_and_writes_child_session() {
        use crate::internal::ai::providers::{fake::FAKE_DEFAULT_MODEL, runtime::provider_id};

        let config = MultiAgentConfig {
            enabled: true,
            max_subagent_depth: 4,
            max_concurrent_subagents: 4,
            subagent_timeout_ms: None,
        };
        let registry = Arc::new(TestRegistry::default());
        let dispatcher = DefaultSubAgentDispatcher::new(registry.clone(), config);

        let mut sub = explore_subagent();
        sub.model = Some(ModelBinding {
            provider_id: provider_id::FAKE.to_string(),
            model_id: FAKE_DEFAULT_MODEL.to_string(),
            variant: None,
        });
        registry.insert(sub);

        let temp = tempfile::tempdir().unwrap();
        let fixture_path = temp.path().join("fake-provider.json");
        std::fs::write(
            &fixture_path,
            serde_json::json!({
                "responses": [{
                    "match": { "contains": "grep TODO src/" },
                    "type": "text",
                    "text": "Found 3 TODOs in 2 files."
                }]
            })
            .to_string(),
        )
        .unwrap();

        let store = crate::internal::ai::session::jsonl::SessionJsonlStore::new(
            temp.path().join("session"),
        );
        let resolved_provider_options = ProviderBuildOptions {
            fake_fixture_path: Some(fixture_path),
            accept_unknown_models: true,
            ..ProviderBuildOptions::default()
        };
        let provider_options = ProviderBuildOptions::default();
        let resolver = TestProviderOptionsResolver {
            options: resolved_provider_options,
        };
        let tool_registry = ToolRegistry::with_working_dir(temp.path().to_path_buf());
        let usage = UsageRecorder::new(Database::connect("sqlite::memory:").await.unwrap());
        let provider_factory = ProviderFactory;
        let context_frame_loader = ContextFrameLoader::default();
        let parent = parent_spec();
        let parent_ruleset: PermissionRuleset = Vec::new();
        let parent_binding = parent_binding();
        let (permission_service, asker) = allow_once_service();
        let parent_thread = "thread-1".to_string();
        let parent_session: SessionId = "session-1".to_string();

        let mut context = ctx(
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
            &provider_options,
            &tool_registry,
            0,
        );
        context.provider_build_options_resolver = Some(&resolver);

        let result = dispatcher
            .dispatch(
                context,
                TaskInvocation {
                    description: "find TODOs".to_string(),
                    prompt: "grep TODO src/".to_string(),
                    subagent_type: "explore".to_string(),
                    task_id: Some("task-explicit".to_string()),
                },
                TaskEntryKind::LlmInitiated,
            )
            .await
            .expect("fake child loop should complete");

        assert_eq!(asker.ask_call_count(), 1);
        assert_eq!(result.task_id, "task-explicit");
        assert_eq!(result.agent_name, "explore");
        assert_eq!(result.provider_id, provider_id::FAKE);
        assert_eq!(result.model_id, FAKE_DEFAULT_MODEL);
        assert_eq!(result.final_text, "Found 3 TODOs in 2 files.");
        assert_eq!(result.steps_used, 1);

        let child_state = store
            .child(&result.task_id)
            .load_state()
            .expect("child JSONL should load")
            .expect("child JSONL should contain snapshots");
        assert_eq!(child_state.id, "task-explicit");
        assert_eq!(child_state.messages.len(), 2);
        assert_eq!(child_state.messages[0].content, "grep TODO src/");
        assert_eq!(child_state.messages[1].content, "Found 3 TODOs in 2 files.");

        let child_events = store
            .child(&result.task_id)
            .load_events()
            .expect("child JSONL events should load");
        let child_lifecycle: Vec<_> = child_events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::AgentRun(envelope) => envelope.known(),
                _ => None,
            })
            .collect();
        assert_eq!(
            child_lifecycle.len(),
            2,
            "child lifecycle events: {child_lifecycle:?}"
        );
        let child_run_id = match child_lifecycle[0] {
            AgentRunEvent::Started { agent_run_id } => *agent_run_id,
            other => panic!("expected child Started event, got {other:?}"),
        };
        match child_lifecycle[1] {
            AgentRunEvent::Completed { agent_run_id } => {
                assert_eq!(*agent_run_id, child_run_id);
            }
            other => panic!("expected child Completed event, got {other:?}"),
        }
    }
}
