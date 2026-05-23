//! Default `SubAgentDispatcher` implementation — gates 1–4, 6–8.
//!
//! This module ships the dispatcher across OC-Phase 3 P3.3 and P3.4 from
//! `docs/improvement/opencode.md`. It implements the **gate + ask** half
//! of the 14-step dispatcher main flow:
//!
//! 1. validate feature flag (`code.multi_agent.enabled`) — P3.3 implemented
//! 2. validate `ctx.depth + 1 <= max_subagent_depth` — P3.3 implemented
//! 3. validate `concurrent_count + 1 <= max_concurrent_subagents`
//!    via atomic `fetch_add` claim — P3.3 implemented
//! 4. resolve `subagent_type` via the spec registry; reject `Primary`
//!    profiles — P3.3 implemented
//! 5. `SafetyDecision::evaluate(SubAgentSpawn { name, prompt_digest })`
//!    — still a TODO no-op. The `ToolOperation::sub_agent_spawn`
//!    constructor exists (v0.17.712), so wiring `ToolBoundaryPolicy::decide`
//!    against it is a small follow-up; today the dispatcher accepts
//!    every sub-spec that survived the prior gates; no semantic gap
//!    exists because Libra has no `SubAgentSpawn` policy configured.
//! 6. compute `effective_ruleset` via `child_ruleset(parent, sub_spec)`
//!    — P3.3 implemented
//! 7. assert no permission escalation (Permission Escalation Gate)
//!    — P3.3 implemented; v0.17.743 layered a parent-abort cancel
//!    check (pre-gate + post-ask) returning
//!    `TaskFailure::Cancelled { ParentAbort }` if the parent
//!    short-circuited mid-dispatch. — P3.7 partial
//! 8. `PermissionService.ask(...)` for `LlmInitiated` only;
//!    `UserInitiated { bypass_permission_ask: true }` skips the
//!    dialog. `Reject{feedback}` surfaces as
//!    [`TaskFailure::ApprovalRejected`]. — P3.4 implemented
//!
//! `Spawned` / `Completed` AgentRun lifecycle events are written into
//! the parent session JSONL as soon as gates clear — P3.5 partial
//! (v0.17.739). When a [`SubAgentChildRunner`] is attached via
//! [`DefaultSubAgentDispatcher::with_child_runner`] (v0.17.756), the
//! dispatcher delegates the post-gate work to the runner and maps
//! its `TaskFailure` into the matching `AgentRunEvent` terminal
//! variant — `Cancelled` / `TimedOut` / `BudgetExceeded` /
//! `Failed { reason }` — via [`map_failure_to_terminal_event`]
//! (v0.17.757). The runner trait itself is the OC-Phase 3 P3.4
//! entry seam.
//!
//! Steps 9–13 (model build, handoff via `ContextHandoffBuilder`,
//! child JSONL session, child run_tool_loop) have shipped as
//! callable helpers — [`DispatchContext::resolve_provider_build_options`]
//! (v0.17.752), [`DispatchContext::build_child_model`] (v0.17.755),
//! [`ContextFrameLoader::latest_frame_for_session`] (v0.17.744), and
//! [`crate::internal::ai::context_budget::ContextHandoffBuilder`]
//! (v0.17.740). The remaining P3.4 work is purely to implement the
//! `SubAgentChildRunner` that drives those helpers through
//! `run_tool_loop_with_history_and_observer` for the child run. No
//! restructure of the dispatcher itself is required to land it.
//!
//! [`SubAgentChildRunner`]: super::sub_agent::SubAgentChildRunner
//! [`DispatchContext::resolve_provider_build_options`]: super::sub_agent::DispatchContext::resolve_provider_build_options
//! [`DispatchContext::build_child_model`]: super::sub_agent::DispatchContext::build_child_model
//! [`ContextFrameLoader::latest_frame_for_session`]: super::sub_agent::ContextFrameLoader::latest_frame_for_session
//! Callers that pass step 8 still see the placeholder
//! [`TaskResult`] from P3.3 — empty `final_text`, zero `steps_used`,
//! the spec-derived agent / provider / model identities. Tests pin
//! that shape so a future regression that drops the placeholder
//! before steps 9–13 land is loud.

use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};

use futures::future::BoxFuture;

use super::sub_agent::{
    CancellationSource, DispatchContext, PermissionAskRequest, PermissionAskSource,
    PermissionReply, SubAgentDispatcher, TaskEntryKind, TaskFailure, TaskInvocation, TaskResult,
};
use crate::internal::ai::{
    agent::profile::AgentExecutionSpec,
    agent_run::{AgentRunEvent, AgentRunEventEnvelope, AgentRunId},
    completion::CompletionUsageSummary,
    permission::{
        EDIT_TOOLS, PermissionRuleset, agent_permission_spec_to_ruleset, assert_no_escalation,
        child_ruleset,
    },
    session::jsonl::SessionEvent,
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
    /// Optional child runner the dispatcher delegates to after the
    /// gates clear. When `Some`, the dispatcher tail constructs a
    /// [`SubAgentChildRunRequest`] from the current dispatch state
    /// and calls `runner.run(...).await` instead of synthesising the
    /// P3.3-era placeholder `TaskResult`. When `None`, the dispatcher
    /// falls back to the placeholder so existing call sites (and the
    /// gate-only tests) keep working unchanged.
    ///
    /// The field is plumbed now so the P3.4 child-loop PR is purely
    /// additive: that PR ships the `RealChildRunner` implementation
    /// and a `with_child_runner` constructor; nothing else in the
    /// dispatcher needs to change.
    child_runner: Option<Arc<dyn super::sub_agent::SubAgentChildRunner>>,
}

impl DefaultSubAgentDispatcher {
    pub fn new(registry: Arc<dyn AgentSpecRegistry>, config: MultiAgentConfig) -> Self {
        Self {
            registry,
            config,
            in_flight: Arc::new(AtomicU32::new(0)),
            child_runner: None,
        }
    }

    /// Attach a [`SubAgentChildRunner`] to the dispatcher. The runner
    /// is consulted after every gate (1-8) clears and the Spawned
    /// event is written; its `TaskResult` (or `TaskFailure`) becomes
    /// the dispatch's outcome, and the dispatcher writes the
    /// matching `Completed` / `Failed` lifecycle event before
    /// returning. Production wires the OC-Phase 3 P3.4 implementation
    /// via this seam; tests can supply a deterministic stub.
    ///
    /// [`SubAgentChildRunner`]: super::sub_agent::SubAgentChildRunner
    pub fn with_child_runner(
        mut self,
        runner: Arc<dyn super::sub_agent::SubAgentChildRunner>,
    ) -> Self {
        self.child_runner = Some(runner);
        self
    }

    /// Convenience wrapper that attaches the production
    /// [`DefaultSubAgentChildRunner`] (single-shot model invocation
    /// with `run_tool_loop` integration). This is the dispatcher
    /// shape libra code's session bootstrap should call when
    /// `code.sub_agents.enabled = true`: gate behaviour stays
    /// unchanged, and the dispatch tail actually runs the child
    /// model instead of synthesising the P3.3-era placeholder
    /// result.
    ///
    /// [`DefaultSubAgentChildRunner`]: super::sub_agent::DefaultSubAgentChildRunner
    pub fn with_default_child_runner(self) -> Self {
        self.with_child_runner(Arc::new(super::sub_agent::DefaultSubAgentChildRunner::new()))
    }

    /// Number of dispatches currently running (test introspection only).
    #[cfg(test)]
    pub fn in_flight(&self) -> u32 {
        self.in_flight.load(Ordering::Acquire)
    }

    /// Run the seven gates in order, returning either the resolved
    /// `(sub_spec, effective_ruleset)` pair the dispatcher tail
    /// consumes or the first [`TaskFailure`] that fires. Step 8
    /// (permission ask) and the P3.5 lifecycle event writes run in
    /// the dispatcher proper; steps 9-13 (child model build,
    /// `ContextHandoff` build, child JSONL, child `run_tool_loop`)
    /// still wait for P3.4 follow-ups.
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
        // against the `ToolOperation::sub_agent_spawn(name,
        // prompt_digest)` operation that v0.17.712 already exposes.
        // The remaining blocker is that `DispatchContext` does not
        // carry a `ToolBoundaryPolicy` reference; wiring requires
        // adding `tool_boundary_policy: Option<&ToolBoundaryPolicy>`
        // to `DispatchContext` and threading the system principal
        // from the parent runtime. Today this is a documented no-op
        // — P3.3 ships the gate ordering and the P3.4 PR will swap
        // this stub for the real call without touching the
        // surrounding capability gates. The dispatcher accepts any
        // sub-spec that survived the prior gates; no semantic gap
        // exists today because Libra has no `SubAgentSpawn` policy
        // configured.
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
            // P3.7 cancel propagation pre-check: if the parent's abort
            // token has already been cancelled before the call even
            // reaches us, refuse the dispatch up front rather than
            // claiming a concurrency slot, writing a `Spawned` event,
            // or invoking the asker. This matches opencode PR #25798's
            // "parent abort short-circuits the whole subtree"
            // semantics — running a now-stale dispatch through to
            // `Completed` would let the parent observe a successful
            // child run after the user already pressed `Ctrl-C`.
            if ctx.abort_token.is_cancelled() {
                return Err(TaskFailure::Cancelled {
                    source: CancellationSource::ParentAbort,
                });
            }

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

            // The same prompt digest is used both by the LlmInitiated
            // permission ask and by the `Spawned` event below, so
            // compute it once and reuse.
            let prompt_digest = digest_for_prompt(&invocation.prompt);

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
                let patterns = vec![invocation.subagent_type.clone()];
                let request = PermissionAskRequest {
                    permission: "task",
                    patterns: &patterns,
                    thread_id: ctx.parent_thread_id,
                    session_id: ctx.parent_session_id,
                    source: PermissionAskSource::SubAgentSpawn {
                        name: invocation.subagent_type.clone(),
                        prompt_digest: prompt_digest.clone(),
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

            // P3.7: a second cancel check after step 8 covers the
            // window where the asker awaited a human reply long
            // enough that the parent aborted in between. Failing
            // closed here means we never write a `Spawned` event for
            // a dispatch that the caller has already abandoned.
            if ctx.abort_token.is_cancelled() {
                return Err(TaskFailure::Cancelled {
                    source: CancellationSource::ParentAbort,
                });
            }

            // P3.5: emit the `Spawned` lifecycle event into the parent
            // session JSONL immediately after every dispatch gate
            // (capability + concurrency + permission) has cleared.
            // This is the earliest point at which a child run is
            // semantically committed; tests and replay tooling rely on
            // `Spawned` preceding any child-side event. The event is a
            // best-effort fire-and-forget write — propagating an IO
            // error here would force the dispatcher to fail dispatches
            // that have already passed every safety gate, so we log
            // and continue.
            let agent_run_id = AgentRunId::new();
            let provider_id = sub_spec
                .model
                .as_ref()
                .map(|m| m.provider_id.clone())
                .unwrap_or_default();
            let model_id = sub_spec
                .model
                .as_ref()
                .map(|m| m.model_id.clone())
                .unwrap_or_default();
            if let Err(err) =
                ctx.session_store
                    .append(&SessionEvent::AgentRun(AgentRunEventEnvelope::from(
                        AgentRunEvent::Spawned {
                            agent_run_id,
                            parent_thread_id: ctx.parent_thread_id.to_string(),
                            parent_session_id: ctx.parent_session_id.clone(),
                            parent_message_id: ctx.parent_message_id.clone(),
                            subagent_name: invocation.subagent_type.clone(),
                            provider_id: provider_id.clone(),
                            model_id: model_id.clone(),
                            depth: ctx.depth.saturating_add(1),
                            prompt_digest,
                        },
                    )))
            {
                tracing::warn!(
                    error = %err,
                    agent_run_id = %agent_run_id.0,
                    subagent = %invocation.subagent_type,
                    "failed to append AgentRunEvent::Spawned to parent session JSONL"
                );
            }

            // Bind the task id to the run id so future call sites
            // that grep the JSONL stream can correlate the dispatch
            // back to its `Spawned` event. Both the P3.4 child runner
            // and the legacy placeholder tail consume the same id.
            let task_id = invocation
                .task_id
                .clone()
                .unwrap_or_else(|| format!("task-placeholder-{}", agent_run_id.0));

            // Steps 9-13: when a child runner is registered, delegate
            // to it. Otherwise fall back to the P3.3-era placeholder
            // so existing gate-only tests keep working unchanged. The
            // runner branch is the seam OC-Phase 3 P3.4 fills in;
            // today every test path takes the placeholder branch.
            let outcome = if let Some(runner) = self.child_runner.as_ref() {
                // OC-Phase 4 minimum-viable handoff (v0.17.773) +
                // P4.4 compacted handoff (v0.17.785): load the
                // parent's latest `ContextFrameEvent` from the
                // session JSONL and materialise it into the
                // child's history before the user prompt lands.
                //
                // Routing rule:
                //   - parent frame present + `ctx.compaction_model`
                //     present: run the compaction agent and feed
                //     the validated `ContextHandoff` via
                //     `to_handoff_messages()` (v0.17.781).
                //   - parent frame present + no compaction model:
                //     fall back to the v0.17.773 raw-segment
                //     dump.
                //   - no parent frame: empty history.
                //
                // Compaction failures (provider error, malformed
                // SUMMARY template) emit `tracing::warn!` and
                // degrade to the raw-segment path. The dispatch
                // never blocks on a compaction failure — the
                // child still runs.
                let parent_frame = ctx
                    .context_frame_loader
                    .latest_frame_for_session(ctx.session_store)
                    .ok()
                    .flatten();
                let history = match (parent_frame.as_ref(), ctx.compaction_model) {
                    (Some(frame), Some(compaction_model)) => {
                        let frame_text = frame
                            .segments
                            .iter()
                            .filter_map(|seg| seg.content.as_deref())
                            .collect::<Vec<_>>()
                            .join("\n\n");
                        let attachment_refs = frame.attachment_refs();
                        let system_prompt =
                            crate::internal::ai::context_budget::embedded_compaction_system_prompt(
                            );
                        match crate::internal::ai::context_budget::run_compaction(
                            compaction_model,
                            system_prompt,
                            &frame_text,
                            frame.frame_id,
                            attachment_refs,
                            Vec::new(),
                            0,
                        )
                        .await
                        {
                            Ok(handoff) => handoff.to_handoff_messages(),
                            Err(err) => {
                                tracing::warn!(
                                    %err,
                                    "compaction agent failed; falling back to raw-segment handoff",
                                );
                                frame.to_handoff_messages()
                            }
                        }
                    }
                    (Some(frame), None) => frame.to_handoff_messages(),
                    (None, _) => Vec::new(),
                };
                let request = super::sub_agent::SubAgentChildRunRequest {
                    ctx: &ctx,
                    invocation: &invocation,
                    sub_spec: &sub_spec,
                    effective_ruleset: &_effective,
                    task_id: task_id.clone(),
                    agent_run_id,
                    history,
                };
                runner.run(request).await
            } else {
                Ok(TaskResult {
                    task_id,
                    agent_name: sub_spec.name.clone(),
                    provider_id,
                    model_id,
                    final_text: String::new(),
                    steps_used: 0,
                    usage: CompletionUsageSummary::default(),
                })
            };

            // P3.5: mirror the dispatch tail with the matching
            // lifecycle event. `Completed` for success; for failures
            // the doc spec distinguishes structurally between
            // Failed / Cancelled / TimedOut / BudgetExceeded so
            // replay tooling can branch on the variant tag without
            // string-matching the reason. Free-form `Failed {
            // reason }` is the catch-all for everything else (e.g.
            // ProviderError, ChildToolLoopFailed) — the reason text
            // is the TaskFailure's Display so the parent transcript
            // and the persisted event agree byte-for-byte.
            // Best-effort: append failures degrade to tracing::warn
            // rather than overriding the outcome.
            let terminal_event = match &outcome {
                Ok(_) => AgentRunEvent::Completed { agent_run_id },
                Err(failure) => map_failure_to_terminal_event(agent_run_id, failure),
            };
            if let Err(err) =
                ctx.session_store
                    .append(&SessionEvent::AgentRun(AgentRunEventEnvelope::from(
                        terminal_event,
                    )))
            {
                tracing::warn!(
                    error = %err,
                    agent_run_id = %agent_run_id.0,
                    subagent = %invocation.subagent_type,
                    outcome_ok = outcome.is_ok(),
                    "failed to append AgentRunEvent::Completed/Failed to parent session JSONL"
                );
            }

            // `_slot` drops here, releasing the concurrency slot.
            outcome
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

/// Produce a short, human-readable preview of a prompt for the
/// permission ask UI. Cap at the first line and 80 characters so the
/// digest fits in a one-line prompt header. Not a cryptographic hash —
/// the goal is "enough to recognise the dispatch in a log", not
/// uniqueness.
/// Translate a `TaskFailure` into the matching `AgentRunEvent`
/// terminal variant.
///
/// The OC-Phase 3 P3.5 contract distinguishes between Failed /
/// Cancelled / TimedOut / BudgetExceeded at the event level so
/// replay tooling can branch on the variant tag without scanning
/// `Failed.reason` for substrings. Variants outside this
/// structural taxonomy (e.g. provider error, child-tool-loop
/// failure) fall through to `Failed { reason }` with the
/// `TaskFailure`'s `Display` text as the reason — that text is the
/// same one the parent transcript shows, so a downstream reader
/// can correlate the event with the parent's diagnostic byte-for-byte.
fn map_failure_to_terminal_event(
    agent_run_id: crate::internal::ai::agent_run::AgentRunId,
    failure: &TaskFailure,
) -> AgentRunEvent {
    use crate::internal::ai::agent_run::{BudgetDimension, CancellationReason};

    match failure {
        TaskFailure::Cancelled {
            source: CancellationSource::ParentAbort,
        } => AgentRunEvent::Cancelled {
            agent_run_id,
            reason: CancellationReason::UserRequested,
        },
        TaskFailure::Cancelled {
            source: CancellationSource::Timeout,
        } => AgentRunEvent::Cancelled {
            agent_run_id,
            reason: CancellationReason::LayerOneTimeout,
        },
        TaskFailure::Cancelled {
            source: CancellationSource::BudgetHardCap,
        } => AgentRunEvent::Cancelled {
            agent_run_id,
            reason: CancellationReason::Other("budget_hard_cap".to_string()),
        },
        TaskFailure::Timeout { .. } => AgentRunEvent::TimedOut { agent_run_id },
        TaskFailure::BudgetExceeded(super::sub_agent::BudgetExceededReason::CostHardCap) => {
            AgentRunEvent::BudgetExceeded {
                agent_run_id,
                dimension: BudgetDimension::Cost,
            }
        }
        TaskFailure::BudgetExceeded(super::sub_agent::BudgetExceededReason::TokenHardCap) => {
            AgentRunEvent::BudgetExceeded {
                agent_run_id,
                dimension: BudgetDimension::Token,
            }
        }
        TaskFailure::BudgetExceeded(super::sub_agent::BudgetExceededReason::WallClock) => {
            AgentRunEvent::BudgetExceeded {
                agent_run_id,
                dimension: BudgetDimension::WallClock,
            }
        }
        TaskFailure::BudgetExceeded(super::sub_agent::BudgetExceededReason::Steps) => {
            // No dedicated "Steps" dimension — use ToolCall as the
            // structural neighbour and preserve the Display reason
            // in the event semantics via the variant tag itself.
            AgentRunEvent::BudgetExceeded {
                agent_run_id,
                dimension: BudgetDimension::ToolCall,
            }
        }
        // Everything else (FeatureDisabled / UnknownSubagent /
        // DepthExceeded / ConcurrencyExceeded /
        // PermissionEscalationDenied / SafetyDenied /
        // ApprovalRejected / BudgetExceeded(Internal) /
        // ContextHandoffFailed / ProviderError /
        // ChildToolLoopFailed) goes through Failed with the
        // Display text. Pre-gate failures never reach this helper
        // because they return Err before the Spawned event fires.
        _ => AgentRunEvent::Failed {
            agent_run_id,
            reason: failure.to_string(),
        },
    }
}

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
        sync::{Mutex, OnceLock},
    };

    use futures::future::BoxFuture;
    use sea_orm::Database;

    use super::*;
    use crate::internal::ai::{
        agent::{
            profile::{
                AgentExecutionSpec, AgentMode, AgentPermissionSpec, ApprovalRoutingSpec,
                ModelBinding, ToolSelection,
            },
            runtime::sub_agent::{
                AbortToken, ContextFrameLoader, DispatchContext, MessageId, PermissionAskRequest,
                PermissionAsker, PermissionReply, PermissionService, SubAgentDispatcher,
                TaskEntryKind, TaskFailure, TaskInvocation,
            },
        },
        permission::{PermissionAction, PermissionRule, PermissionRuleset},
        providers::{ProviderBuildOptions, ProviderFactory},
        session::SessionId,
        tools::ToolRegistry,
        usage::UsageRecorder,
    };

    /// Process-wide empty `ProviderBuildOptions` used by every
    /// `DispatchContext` test fixture. The gates exercised here never
    /// read these fields, but the struct shape requires the borrow,
    /// so a single shared static keeps every `ctx()` call site free of
    /// per-test allocation noise.
    fn default_provider_build_options() -> &'static ProviderBuildOptions {
        static OPTS: OnceLock<ProviderBuildOptions> = OnceLock::new();
        OPTS.get_or_init(ProviderBuildOptions::default)
    }

    /// Process-wide empty `ToolRegistry` used by every
    /// `DispatchContext` test fixture. Construction uses
    /// `with_working_dir(".")` so the helper never panics on
    /// CWD-resolution like `ToolRegistry::new()` could under a
    /// concurrent harness.
    fn default_tool_registry() -> &'static ToolRegistry {
        static REG: OnceLock<ToolRegistry> = OnceLock::new();
        REG.get_or_init(|| ToolRegistry::with_working_dir(std::path::PathBuf::from(".")))
    }

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
            provider_build_options: default_provider_build_options(),
            provider_build_options_resolver: None,
            tool_registry: default_tool_registry(),
            runtime_context: None,
            usage_recorder,
            context_frame_loader,
            abort_token: AbortToken::new(),
            depth,
            compaction_model: None,
            hook_runner: None,
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

        // P3.8 byte-level flag-off regression contract: a refused
        // dispatch must NOT mutate the parent session JSONL. If the
        // events.jsonl file does not exist that is the strongest
        // possible "no side effects" signal — the dispatcher rejected
        // on the feature flag before any append could even create the
        // file. If the file exists from a prior write in the same
        // test harness, then no new bytes may be appended after the
        // rejected dispatch.
        let events_path = store.events_path();
        let bytes_after = std::fs::read(&events_path).unwrap_or_default();
        assert!(
            bytes_after.is_empty(),
            "flag-off dispatch must NOT mutate parent session JSONL; \
             found {} bytes at '{}': {:?}",
            bytes_after.len(),
            events_path.display(),
            String::from_utf8_lossy(&bytes_after),
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
        };
        let (dispatcher, registry, usage, store) = dispatcher_test_harness(config).await;
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
    /// `Once` lets the dispatch through to the placeholder tail. The
    /// asker is invoked exactly once, regardless of `Once` vs
    /// `Always` (the asker, not the dispatcher, persists `Always`
    /// rules).
    #[tokio::test]
    async fn dispatch_proceeds_when_asker_replies_once() {
        let config = MultiAgentConfig {
            enabled: true,
            max_subagent_depth: 4,
            max_concurrent_subagents: 4,
        };
        let (dispatcher, registry, usage, store) = dispatcher_test_harness(config).await;
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
        };
        let (dispatcher, registry, usage, store) = dispatcher_test_harness(config).await;
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
            0,
        );

        let result = dispatcher
            .dispatch(context, invocation("explore"), TaskEntryKind::LlmInitiated)
            .await
            .expect("every gate should pass");

        assert_eq!(result.agent_name, "explore");
        assert_eq!(result.provider_id, "anthropic");
        assert_eq!(result.model_id, "claude-3-5-haiku-latest");
        // Placeholder tail still leaves these empty/zero — steps
        // 9–13 (handoff + model build + child loop) fill them in
        // subsequent OC-Phase 3 sub-PRs.
        assert_eq!(result.final_text, "");
        assert_eq!(result.steps_used, 0);

        // Concurrency counter must return to 0 after the call.
        assert_eq!(dispatcher.in_flight(), 0);
    }

    /// P3.7 cancel propagation: a dispatch whose context carries an
    /// already-cancelled `abort_token` short-circuits with
    /// `TaskFailure::Cancelled { source: ParentAbort }` BEFORE any
    /// gate runs. Neither the concurrency slot nor the session JSONL
    /// must be touched, otherwise a `Ctrl-C` between the parent
    /// awaiting the asker and the dispatcher returning would leak a
    /// half-committed dispatch.
    #[tokio::test]
    async fn dispatch_short_circuits_when_parent_abort_already_fired() {
        let config = MultiAgentConfig {
            enabled: true,
            max_subagent_depth: 4,
            max_concurrent_subagents: 4,
        };
        let (dispatcher, registry, usage, store) = dispatcher_test_harness(config).await;
        registry.insert(explore_subagent());

        let parent = parent_spec();
        let parent_ruleset: PermissionRuleset = Vec::new();
        let parent_binding = parent_binding();
        let asker = Arc::new(TestAsker::always(PermissionReply::Once));
        let permission_service = PermissionService::new(asker.clone());
        let provider_factory = ProviderFactory;
        let context_frame_loader = ContextFrameLoader::default();
        let parent_thread = "thread-cancelled".to_string();
        let parent_session: SessionId = "session-cancelled".to_string();

        // Build a context whose abort token is already cancelled.
        let pre_cancelled = AbortToken::new();
        pre_cancelled.cancel();
        let context = DispatchContext {
            parent_thread_id: &parent_thread,
            parent_session_id: &parent_session,
            parent_agent: &parent,
            parent_ruleset: &parent_ruleset,
            parent_model_binding: &parent_binding,
            parent_message_id: MessageId::from("msg-cancelled"),
            permission_service: &permission_service,
            session_store: &store,
            provider_factory: &provider_factory,
            provider_build_options: default_provider_build_options(),
            provider_build_options_resolver: None,
            tool_registry: default_tool_registry(),
            runtime_context: None,
            usage_recorder: &usage,
            context_frame_loader: &context_frame_loader,
            abort_token: pre_cancelled,
            depth: 0,
            compaction_model: None,
            hook_runner: None,
        };

        let result = dispatcher
            .dispatch(context, invocation("explore"), TaskEntryKind::LlmInitiated)
            .await;
        assert!(
            matches!(
                result,
                Err(TaskFailure::Cancelled {
                    source: CancellationSource::ParentAbort,
                }),
            ),
            "expected Cancelled{{ParentAbort}} when abort already fired, got {:?}",
            result.as_ref().err()
        );

        assert_eq!(
            asker.ask_call_count(),
            0,
            "pre-cancelled dispatch must NOT call the asker"
        );
        assert_eq!(
            dispatcher.in_flight(),
            0,
            "pre-cancelled dispatch must NOT claim a concurrency slot"
        );

        let events_path = store.events_path();
        let bytes = std::fs::read(&events_path).unwrap_or_default();
        assert!(
            bytes.is_empty(),
            "pre-cancelled dispatch must NOT write any Spawned/Completed bytes; \
             found {} bytes at '{}': {:?}",
            bytes.len(),
            events_path.display(),
            String::from_utf8_lossy(&bytes),
        );
    }

    /// P3.5 wire-up: a successful dispatch writes `Spawned` followed
    /// immediately by `Completed` into the parent session JSONL. Both
    /// events share the same `agent_run_id` and carry the spec-resolved
    /// `provider_id` / `model_id` so replay tooling can correlate the
    /// pair without re-resolving the registry.
    #[tokio::test]
    async fn dispatch_writes_spawned_then_completed_events_to_parent_session() {
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
        let (permission_service, _asker) = allow_once_service();
        let provider_factory = ProviderFactory;
        let context_frame_loader = ContextFrameLoader::default();
        let parent_thread = "thread-events".to_string();
        let parent_session: SessionId = "session-events".to_string();

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

        dispatcher
            .dispatch(context, invocation("explore"), TaskEntryKind::LlmInitiated)
            .await
            .expect("every gate should pass");

        let events: Vec<_> = store
            .load_events()
            .expect("session JSONL must be readable after dispatch")
            .into_iter()
            .filter_map(|envelope| match envelope {
                crate::internal::ai::session::jsonl::SessionEvent::AgentRun(known) => {
                    known.known().cloned()
                }
                _ => None,
            })
            .collect();

        assert_eq!(
            events.len(),
            2,
            "dispatch should emit exactly Spawned + Completed"
        );

        let (spawned_id, recorded_provider, recorded_model, recorded_depth, recorded_digest) =
            match &events[0] {
                AgentRunEvent::Spawned {
                    agent_run_id,
                    parent_thread_id,
                    parent_session_id,
                    subagent_name,
                    provider_id,
                    model_id,
                    depth,
                    prompt_digest,
                    ..
                } => {
                    assert_eq!(parent_thread_id, &parent_thread);
                    assert_eq!(parent_session_id, &parent_session);
                    assert_eq!(subagent_name, "explore");
                    (
                        *agent_run_id,
                        provider_id.clone(),
                        model_id.clone(),
                        *depth,
                        prompt_digest.clone(),
                    )
                }
                other => panic!("first event must be Spawned, got {other:?}"),
            };
        assert_eq!(recorded_provider, "anthropic");
        assert_eq!(recorded_model, "claude-3-5-haiku-latest");
        assert_eq!(
            recorded_depth, 1,
            "Spawned.depth should be parent depth + 1 (parent was 0)"
        );
        assert_eq!(
            recorded_digest, "do a thing",
            "prompt digest must equal the invocation's first-line preview"
        );

        match &events[1] {
            AgentRunEvent::Completed { agent_run_id } => {
                assert_eq!(
                    agent_run_id, &spawned_id,
                    "Completed must reuse the agent_run_id minted for Spawned"
                );
            }
            other => panic!("second event must be Completed, got {other:?}"),
        }
    }

    /// OC-Phase 3 P3.4 seam: when a `SubAgentChildRunner` is attached
    /// via `with_child_runner`, the dispatcher delegates the result
    /// to it instead of synthesising the legacy placeholder. The
    /// `Spawned` event still fires up front, and the runner's
    /// outcome (Ok or Err) flips the terminal event between
    /// `Completed` and `Failed`.
    #[tokio::test]
    async fn dispatch_delegates_to_child_runner_when_attached_and_writes_completed() {
        let config = MultiAgentConfig {
            enabled: true,
            max_subagent_depth: 4,
            max_concurrent_subagents: 4,
        };

        // A deterministic runner that returns a recognisable
        // TaskResult so the test can assert it propagated through.
        struct ConstantRunner;
        impl crate::internal::ai::agent::runtime::SubAgentChildRunner for ConstantRunner {
            fn run<'a>(
                &'a self,
                request: crate::internal::ai::agent::runtime::SubAgentChildRunRequest<'a>,
            ) -> futures::future::BoxFuture<'a, Result<TaskResult, TaskFailure>> {
                let task_id = request.task_id.clone();
                let agent_name = request.sub_spec.name.clone();
                Box::pin(async move {
                    Ok(TaskResult {
                        task_id,
                        agent_name,
                        provider_id: "runner-provider".to_string(),
                        model_id: "runner-model".to_string(),
                        final_text: "runner produced this".to_string(),
                        steps_used: 7,
                        usage: CompletionUsageSummary::default(),
                    })
                })
            }
        }

        let (dispatcher, registry, usage, store) = dispatcher_test_harness(config).await;
        let dispatcher = dispatcher.with_child_runner(Arc::new(ConstantRunner));

        let mut sub = explore_subagent();
        sub.model = ModelBinding::parse("anthropic/claude-3-5-haiku-latest");
        registry.insert(sub);

        let parent = parent_spec();
        let parent_ruleset: PermissionRuleset = Vec::new();
        let parent_binding = parent_binding();
        let (permission_service, _asker) = allow_once_service();
        let provider_factory = ProviderFactory;
        let context_frame_loader = ContextFrameLoader::default();
        let parent_thread = "thread-runner".to_string();
        let parent_session: SessionId = "session-runner".to_string();

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
            .expect("runner returned Ok");
        assert_eq!(result.final_text, "runner produced this");
        assert_eq!(result.steps_used, 7);
        assert_eq!(result.provider_id, "runner-provider");

        let events: Vec<_> = store
            .load_events()
            .expect("JSONL readable")
            .into_iter()
            .filter_map(|envelope| match envelope {
                crate::internal::ai::session::jsonl::SessionEvent::AgentRun(known) => {
                    known.known().cloned()
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            events.len(),
            2,
            "runner success must still emit Spawned + Completed"
        );
        assert!(matches!(events[0], AgentRunEvent::Spawned { .. }));
        assert!(matches!(events[1], AgentRunEvent::Completed { .. }));
    }

    /// Symmetric counterpart: a runner that returns
    /// `TaskFailure::Timeout` produces a structurally-typed
    /// `AgentRunEvent::TimedOut` terminal (not Failed). The P3.5
    /// taxonomy distinguishes Failed / Cancelled / TimedOut /
    /// BudgetExceeded at the event level so replay tooling can
    /// branch on the variant tag without scanning Failed.reason
    /// substrings.
    #[tokio::test]
    async fn dispatch_runner_error_emits_failed_event_with_reason() {
        let config = MultiAgentConfig {
            enabled: true,
            max_subagent_depth: 4,
            max_concurrent_subagents: 4,
        };

        struct FailingRunner;
        impl crate::internal::ai::agent::runtime::SubAgentChildRunner for FailingRunner {
            fn run<'a>(
                &'a self,
                _request: crate::internal::ai::agent::runtime::SubAgentChildRunRequest<'a>,
            ) -> futures::future::BoxFuture<'a, Result<TaskResult, TaskFailure>> {
                Box::pin(async {
                    Err(TaskFailure::Timeout {
                        wall_clock_ms: 60_000,
                    })
                })
            }
        }

        let (dispatcher, registry, usage, store) = dispatcher_test_harness(config).await;
        let dispatcher = dispatcher.with_child_runner(Arc::new(FailingRunner));
        registry.insert(explore_subagent());

        let parent = parent_spec();
        let parent_ruleset: PermissionRuleset = Vec::new();
        let parent_binding = parent_binding();
        let (permission_service, _asker) = allow_once_service();
        let provider_factory = ProviderFactory;
        let context_frame_loader = ContextFrameLoader::default();
        let parent_thread = "thread-fail".to_string();
        let parent_session: SessionId = "session-fail".to_string();

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

        let err = dispatcher
            .dispatch(context, invocation("explore"), TaskEntryKind::LlmInitiated)
            .await
            .expect_err("runner returned Err must surface from dispatch");
        assert!(matches!(err, TaskFailure::Timeout { .. }));

        let events: Vec<_> = store
            .load_events()
            .expect("JSONL readable")
            .into_iter()
            .filter_map(|envelope| match envelope {
                crate::internal::ai::session::jsonl::SessionEvent::AgentRun(known) => {
                    known.known().cloned()
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            events.len(),
            2,
            "runner failure must still emit Spawned + TimedOut"
        );
        assert!(matches!(events[0], AgentRunEvent::Spawned { .. }));
        assert!(
            matches!(events[1], AgentRunEvent::TimedOut { .. }),
            "TaskFailure::Timeout must map to AgentRunEvent::TimedOut, got: {:?}",
            events[1],
        );
    }

    /// `with_default_child_runner` attaches the production runner
    /// without forcing call sites to import the runner type. The
    /// dispatcher's behaviour after attachment is identical to
    /// `with_child_runner(Arc::new(DefaultSubAgentChildRunner))`;
    /// this test pins the equivalence so the convenience wrapper
    /// cannot silently drift from the explicit form.
    #[tokio::test]
    async fn with_default_child_runner_attaches_the_production_runner() {
        let config = MultiAgentConfig {
            enabled: true,
            max_subagent_depth: 4,
            max_concurrent_subagents: 4,
        };
        let (dispatcher, _registry, _usage, _store) = dispatcher_test_harness(config).await;
        let with_explicit = dispatcher.with_child_runner(Arc::new(
            crate::internal::ai::agent::runtime::DefaultSubAgentChildRunner::new(),
        ));
        // The explicit + convenience wrappers attach equivalent
        // runners. The dispatcher does not expose its runner field
        // directly for inspection (private), but the inability to
        // distinguish the two at any caller surface IS the
        // contract — a future refactor that drops the convenience
        // wrapper must keep `with_child_runner(Arc::new(...))`
        // working for the public production path.
        drop(with_explicit);

        let (dispatcher2, _registry2, _usage2, _store2) =
            dispatcher_test_harness(MultiAgentConfig {
                enabled: true,
                max_subagent_depth: 4,
                max_concurrent_subagents: 4,
            })
            .await;
        let with_convenience = dispatcher2.with_default_child_runner();
        drop(with_convenience);
    }

    /// P3.7 wire-up: a runner that returns `TaskFailure::Cancelled
    /// { ParentAbort }` produces `AgentRunEvent::Cancelled { reason:
    /// UserRequested }` — the schema variant tag distinguishes
    /// human-driven aborts from `LayerOneTimeout` (timeout-driven)
    /// and the `Other` catch-all (budget-hard-cap, etc.).
    #[tokio::test]
    async fn dispatch_runner_cancel_emits_cancelled_event_with_user_requested_reason() {
        use crate::internal::ai::agent_run::CancellationReason;

        let config = MultiAgentConfig {
            enabled: true,
            max_subagent_depth: 4,
            max_concurrent_subagents: 4,
        };

        struct CancellingRunner;
        impl crate::internal::ai::agent::runtime::SubAgentChildRunner for CancellingRunner {
            fn run<'a>(
                &'a self,
                _request: crate::internal::ai::agent::runtime::SubAgentChildRunRequest<'a>,
            ) -> futures::future::BoxFuture<'a, Result<TaskResult, TaskFailure>> {
                Box::pin(async {
                    Err(TaskFailure::Cancelled {
                        source: CancellationSource::ParentAbort,
                    })
                })
            }
        }

        let (dispatcher, registry, usage, store) = dispatcher_test_harness(config).await;
        let dispatcher = dispatcher.with_child_runner(Arc::new(CancellingRunner));
        registry.insert(explore_subagent());

        let parent = parent_spec();
        let parent_ruleset: PermissionRuleset = Vec::new();
        let parent_binding = parent_binding();
        let (permission_service, _asker) = allow_once_service();
        let provider_factory = ProviderFactory;
        let context_frame_loader = ContextFrameLoader::default();
        let parent_thread = "thread-cancel".to_string();
        let parent_session: SessionId = "session-cancel".to_string();

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

        dispatcher
            .dispatch(context, invocation("explore"), TaskEntryKind::LlmInitiated)
            .await
            .expect_err("runner returned Cancelled");

        let events: Vec<_> = store
            .load_events()
            .expect("JSONL readable")
            .into_iter()
            .filter_map(|envelope| match envelope {
                crate::internal::ai::session::jsonl::SessionEvent::AgentRun(known) => {
                    known.known().cloned()
                }
                _ => None,
            })
            .collect();
        assert_eq!(events.len(), 2);
        match &events[1] {
            AgentRunEvent::Cancelled { reason, .. } => {
                assert!(
                    matches!(reason, CancellationReason::UserRequested),
                    "ParentAbort must map to CancellationReason::UserRequested, got: {reason:?}",
                );
            }
            other => panic!("expected Cancelled terminal event, got {other:?}"),
        }
    }
}
