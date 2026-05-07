//! Sub-agent dispatcher contract — types and trait definitions only.
//!
//! This module is the OC-Phase 3 P3.2 deliverable from
//! `docs/improvement/opencode.md`. It defines the **vocabulary** that
//! P3.3 → P3.7 will fill in: a [`SubAgentDispatcher`] trait that the tool
//! loop forwards `task` calls into, the [`DispatchContext`] that carries
//! the parent-session state the dispatcher needs, and the
//! [`TaskInvocation`] / [`TaskResult`] / [`TaskFailure`] types that bound
//! the call shape on either side.
//!
//! What this module is:
//! - Pure data and trait definitions. No runtime implementation, no
//!   registration into the tool loop, no `code.multi_agent.enabled` gate.
//!   The dispatcher landing in P3.3+ will live next to or replace this
//!   file.
//! - Forward-stable shapes the doc commits to. Any field rename here
//!   has to update the contract section of `opencode.md` first.
//!
//! What this module is **not**:
//! - It does not register the `task` tool — that is OC-Phase 3 P3.1's
//!   `ToolSpec::task()` constructor, gated separately.
//! - It does not implement the 14-step dispatcher main flow from the
//!   doc — that lands in P3.3 / P3.4 / P3.7.
//! - It does not own the runtime services it references
//!   ([`PermissionService`], [`ContextFrameLoader`]). Those are
//!   placeholder shells that the real wiring PRs replace; their **names
//!   and method signatures** are the future contract, not the bodies.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use futures::future::BoxFuture;

use crate::internal::ai::{
    agent::profile::{AgentExecutionSpec, ModelBinding},
    completion::{CompletionError, CompletionUsageSummary},
    permission::PermissionRuleset,
    providers::ProviderFactory,
    session::{SessionId, jsonl::SessionJsonlStore},
    usage::UsageRecorder,
};

/// Opaque alias for a runtime message identifier. Today's runtime stores
/// these as strings (the JSONL row id of the assistant message that
/// triggered the dispatch); a later PR may promote it to a newtype with
/// stricter parsing.
pub type MessageId = String;

// ─── Cancellation primitive ─────────────────────────────────────────────

/// Cooperative cancellation token threaded through a sub-agent dispatch.
///
/// OC-Phase 3 P3.7 (per `opencode.md` "Cancel / Abort 传播合同") replaces
/// this with a `tokio_util::sync::CancellationToken` (or equivalent) that
/// supports the doc's "parent abort must await child cancel completion"
/// cleanup invariant. P3.2 ships the minimal placeholder so the trait
/// signature compiles; the dispatcher in P3.3 still does not block on it.
#[derive(Clone, Debug, Default)]
pub struct AbortToken {
    /// Boolean cancellation flag. The placeholder shape is intentionally
    /// the simplest one that makes `child()` and `is_cancelled()` work
    /// without taking on a new crate dependency before P3.7 needs it.
    inner: Arc<AtomicBool>,
}

impl AbortToken {
    /// Create a fresh, un-cancelled root token.
    pub fn new() -> Self {
        Self::default()
    }

    /// Trigger cancellation. After this returns, `is_cancelled()` is true.
    pub fn cancel(&self) {
        self.inner.store(true, Ordering::Release);
    }

    /// Returns `true` once cancellation has been requested on this token.
    pub fn is_cancelled(&self) -> bool {
        self.inner.load(Ordering::Acquire)
    }

    /// Spawn a child token. P3.7 will make parent cancellation propagate
    /// to children automatically; P3.2's placeholder just hands back a
    /// fresh token so the dispatcher's call-site code compiles.
    pub fn child(&self) -> Self {
        Self::default()
    }
}

// ─── Runtime service placeholders ───────────────────────────────────────

/// Placeholder for the permission service that mediates the three-state
/// `Once` / `Always` / `Reject` reply flow. Real shape arrives in P3.4
/// when the dispatcher wires `permission.ask()` for `LlmInitiated`
/// dispatches.
#[derive(Debug, Default)]
pub struct PermissionService {
    /// Intentionally empty — future fields land in P3.4. Marked `_marker`
    /// so the placeholder cannot accidentally be constructed and used as a
    /// real service in production code today.
    _marker: (),
}

/// Placeholder for the context-frame loader the dispatcher uses to
/// materialise a [`ContextHandoff`]-style summary from the parent session
/// JSONL. Real shape arrives in P4.3 alongside the handoff builder.
#[derive(Debug, Default)]
pub struct ContextFrameLoader {
    _marker: (),
}

// ─── Invocation / Result / Entry Kind ───────────────────────────────────

/// The shape the model emits when it calls the `task` tool.
///
/// Mirrors `TaskInvocation` from `docs/improvement/opencode.md` and the
/// JSON schema returned by `ToolSpec::task()` (OC-Phase 3 P3.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskInvocation {
    /// Short human-readable summary surfaced in transcripts and budget logs.
    pub description: String,
    /// The user-message body sent to the sub-agent.
    pub prompt: String,
    /// Name of the agent profile to dispatch.
    pub subagent_type: String,
    /// Optional resume token from a prior dispatch under the same parent
    /// thread.
    pub task_id: Option<String>,
}

/// Successful return shape from the dispatcher.
///
/// `usage` is a [`CompletionUsageSummary`] aggregating every model call
/// inside the child run (assistant turns + any nested tool retries).
/// `agent_name` / `provider_id` / `model_id` come from the resolved
/// [`AgentExecutionSpec`]; they are echoed back so the parent transcript
/// records exactly what ran.
#[derive(Clone, Debug, PartialEq)]
pub struct TaskResult {
    pub task_id: String,
    pub agent_name: String,
    pub provider_id: String,
    pub model_id: String,
    pub final_text: String,
    pub steps_used: u32,
    pub usage: CompletionUsageSummary,
}

/// How a `task` dispatch entered the dispatcher.
///
/// `LlmInitiated` is the regular flow: the parent agent's model emitted a
/// `task(...)` tool call. `UserInitiated` is the slash-command / Code
/// Control / SubtaskPart flow where the user explicitly typed the
/// dispatch. Per the doc "Two Entry Points" table:
///
/// - `LlmInitiated` ⇒ runs `permission.ask({permission:"task", ...})`.
/// - `UserInitiated { bypass_permission_ask: true }` ⇒ skips the
///   approval dialog (the user already chose) but still runs every other
///   gate (depth, concurrency, SafetyDecision, escalation).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskEntryKind {
    LlmInitiated,
    UserInitiated { bypass_permission_ask: bool },
}

// ─── Failure taxonomy ──────────────────────────────────────────────────

/// Reason an `Always`-reply approval would have been required but the
/// dispatcher refused to dispatch. Wraps a human-readable explanation
/// produced by the SafetyDecision module (Step 1.1) — the placeholder is
/// a string until that module's denial type stabilises.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SafetyDecisionDenial {
    pub reason: String,
}

/// Why a budget gate fired during dispatch. Each variant carries a short
/// human-readable label so the TUI surface can render the limit; the
/// numeric thresholds live on the budget config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BudgetExceededReason {
    /// The dispatcher refused to enter because a hard cost cap is
    /// already at or past its limit.
    CostHardCap,
    /// Wall-clock budget for the parent session has expired before the
    /// child started.
    WallClock,
    /// A sub-agent step budget would be violated.
    Steps,
}

/// Why the dispatcher could not assemble a [`ContextHandoff`] for the
/// child run. The `SchemaMismatch` variant matches the literal-template
/// validation rule from `opencode.md` (OC-Phase 4 P4.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContextHandoffError {
    /// The compaction agent's summary did not contain every required
    /// `## ...` heading from the literal SUMMARY_TEMPLATE.
    SchemaMismatch { missing_sections: Vec<String> },
    /// The compaction agent itself failed (provider error, timeout, etc.).
    CompactionFailed { reason: String },
    /// The parent session had no `ContextFrameEvent` available to seed
    /// the handoff from.
    NoFrameAvailable,
}

/// Why the child tool loop returned a non-completion outcome. Wraps the
/// generic completion error today; OC-Phase 4 P4.1 will extend this to
/// the structured `ProviderError` taxonomy.
#[derive(Debug)]
pub enum ToolLoopError {
    Completion(CompletionError),
    /// Step budget hit before a final answer arrived.
    StepBudgetExhausted {
        steps: u32,
    },
}

/// Who triggered a cancellation. Surfaced inside
/// [`TaskFailure::Cancelled`] so the parent can distinguish "user
/// pressed Ctrl-C" from "child hit its own budget hard cap".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancellationSource {
    /// The parent session's abort token fired (Ctrl-C, `/cancel`,
    /// Code Control `cancel` RPC).
    ParentAbort,
    /// The child's own budget tripped its hard cap.
    BudgetHardCap,
    /// The child's wall-clock timeout ran out.
    Timeout,
}

/// Structured failure modes returned by the dispatcher.
///
/// Each variant mirrors the doc's `TaskFailure` enum literally so a
/// future opencode-shaped JSON event for these cases is a structural
/// copy. New variants must be added BOTH here AND to `opencode.md`.
#[derive(Debug)]
pub enum TaskFailure {
    /// `code.multi_agent.enabled` is `false` and the dispatcher refused
    /// the spawn at step 1 of the gate flow. Distinct from
    /// [`Self::SafetyDenied`] (which is the step-5 SafetyDecision
    /// rejection) so log analysis and error matchers can disambiguate
    /// "feature off" from "sandbox said no". This variant is **not**
    /// in opencode's original taxonomy — it is a Libra-specific addition
    /// because the feature flag lives in `[code.multi_agent]` config
    /// (OC-Phase 5) instead of being a deploy-time toggle.
    FeatureDisabled,
    /// `subagent_type` did not resolve to a profile, or the profile's
    /// mode was `Primary` (not eligible for sub-agent dispatch).
    UnknownSubagent {
        name: String,
        suggestions: Vec<String>,
    },
    /// `ctx.depth + 1 > max_subagent_depth`. P3.3 enforces the gate.
    DepthExceeded { current: u8, limit: u8 },
    /// More child dispatches in flight than `max_concurrent_subagents`.
    ConcurrencyExceeded { current: u32, limit: u32 },
    /// The escalation gate (Permission Escalation Gate) refused — a
    /// child rule would have flipped a parent `Deny` to `Allow`.
    PermissionEscalationDenied { permission: String, pattern: String },
    /// SafetyDecision module refused the spawn before any other gate.
    SafetyDenied(SafetyDecisionDenial),
    /// The user replied `Reject` to the `LlmInitiated` permission ask;
    /// the optional `feedback` carries the user's typed reason.
    ApprovalRejected { feedback: Option<String> },
    /// Budget gate fired (cost / wall-clock / steps).
    BudgetExceeded(BudgetExceededReason),
    /// Context handoff failed; the dispatcher refused to fall back to
    /// raw transcript so the model never sees an unbounded history.
    ContextHandoffFailed(ContextHandoffError),
    /// The child's own provider call failed (network, auth, malformed
    /// response).
    ProviderError(CompletionError),
    /// The child tool loop returned an error — distinct from a clean
    /// `final_text` outcome.
    ChildToolLoopFailed(ToolLoopError),
    /// Cancellation propagated from `source`.
    Cancelled { source: CancellationSource },
    /// Wall-clock deadline hit before the child finished.
    Timeout { wall_clock_ms: u64 },
}

// ─── DispatchContext + SubAgentDispatcher trait ─────────────────────────

/// Context the parent session passes to the dispatcher.
///
/// Every reference is borrowed from the live runtime; the dispatcher
/// returns a future whose lifetime ties back to `'a` so the borrow
/// graph is checked at compile time. Once P3.3 lands, the dispatcher
/// implementation will hold each of these references long enough to
/// drive the child run and then drop them.
pub struct DispatchContext<'a> {
    /// Stable thread id under which both parent and child events live.
    pub parent_thread_id: &'a str,
    /// Parent session id (the [`SessionId`] alias is `String` today).
    pub parent_session_id: &'a SessionId,
    /// The parent agent's resolved spec — drives permission inheritance
    /// and is forwarded into the child handoff.
    pub parent_agent: &'a AgentExecutionSpec,
    /// Parent ruleset; the child inherits the doc's
    /// `child_ruleset(parent, sub_spec)` projection.
    pub parent_ruleset: &'a PermissionRuleset,
    /// Parent's resolved model binding. Unused by the gates but
    /// surfaced into the child run's handoff so the model knows what
    /// mode the parent was running in.
    pub parent_model_binding: &'a ModelBinding,
    /// The assistant message id that emitted the `task` tool call.
    /// Forwarded to `AgentRunEvent::Spawned` for replay.
    pub parent_message_id: MessageId,
    /// Permission service the dispatcher delegates `Ask` replies to.
    /// **The child's permission service must be the parent's instance**
    /// (not a fresh one) per the doc — children cannot self-approve.
    pub permission_service: &'a PermissionService,
    /// Session JSONL store for the parent. The dispatcher writes the
    /// `Spawned` / `Completed` / `Failed` events on the parent side and
    /// creates the child's JSONL on the same backing store.
    pub session_store: &'a SessionJsonlStore,
    /// Provider factory used to build the child's `AnyCompletionModel`.
    /// Stateless today (OC-Phase 1 P1.2) — the dispatcher just holds a
    /// reference for parity with the doc's contract.
    pub provider_factory: &'a ProviderFactory,
    /// Usage recorder the child run pipes its rows into. The recorder is
    /// the parent's instance with `agent_run_id` bound to the child's
    /// id; OC-Phase 5 adds the `agent_name` column.
    pub usage_recorder: &'a UsageRecorder,
    /// Loader that materialises the parent's most recent
    /// `ContextFrameEvent` into a handoff structure.
    pub context_frame_loader: &'a ContextFrameLoader,
    /// Child's abort token. Created via `ctx.abort_token.child()` so a
    /// parent-side cancel propagates downstream (the placeholder in
    /// P3.2 hands back a fresh token; P3.7 makes the propagation real).
    pub abort_token: AbortToken,
    /// Stack depth of this dispatch. The session driver passes `0` for
    /// the first level; the dispatcher rejects `depth + 1 >
    /// max_subagent_depth`.
    pub depth: u8,
}

/// Object-safe trait the tool loop forwards `task` calls into when
/// `code.multi_agent.enabled = true` (OC-Phase 5).
///
/// The signature uses [`BoxFuture`] (not `async fn`) so the trait can be
/// stored as `Arc<dyn SubAgentDispatcher>` inside a generic
/// `ToolLoopConfig`. This is the explicit reason the doc forbids relying
/// on `async fn in trait` for this specific trait — the runtime needs
/// trait objects.
pub trait SubAgentDispatcher: Send + Sync {
    fn dispatch<'a>(
        &'a self,
        ctx: DispatchContext<'a>,
        invocation: TaskInvocation,
        entry_kind: TaskEntryKind,
    ) -> BoxFuture<'a, Result<TaskResult, TaskFailure>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scenario: `AbortToken` round-trips a cancellation flag through
    /// `cancel()` / `is_cancelled()`. Child tokens are independent in
    /// the placeholder implementation (P3.7 will make them propagate);
    /// the test pins the placeholder semantics so a future regression
    /// is loud.
    #[test]
    fn abort_token_placeholder_round_trips_cancellation() {
        let root = AbortToken::new();
        assert!(!root.is_cancelled());
        root.cancel();
        assert!(root.is_cancelled());

        // Child is independent today (placeholder behavior).
        let other_root = AbortToken::new();
        let child = other_root.child();
        assert!(!child.is_cancelled());
        other_root.cancel();
        assert!(
            !child.is_cancelled(),
            "P3.2 placeholder does not propagate; P3.7 will fix this"
        );
    }

    /// Scenario: every `TaskEntryKind` variant constructs without
    /// panicking and the `bypass_permission_ask` payload survives.
    #[test]
    fn task_entry_kind_variants_construct() {
        let _llm = TaskEntryKind::LlmInitiated;
        let user = TaskEntryKind::UserInitiated {
            bypass_permission_ask: true,
        };
        match user {
            TaskEntryKind::UserInitiated {
                bypass_permission_ask,
            } => assert!(bypass_permission_ask),
            other => panic!("expected UserInitiated, got {other:?}"),
        }
    }

    /// Scenario: a `TaskInvocation` round-trips through clone +
    /// equality, pinning the field set the doc commits to. Adding a
    /// new required field (or renaming one) breaks this test loudly.
    #[test]
    fn task_invocation_field_set_matches_doc_contract() {
        let invocation = TaskInvocation {
            description: "find TODOs".to_string(),
            prompt: "grep TODO src/".to_string(),
            subagent_type: "explore".to_string(),
            task_id: None,
        };
        let cloned = invocation.clone();
        assert_eq!(cloned, invocation);
        assert_eq!(invocation.description, "find TODOs");
        assert_eq!(invocation.prompt, "grep TODO src/");
        assert_eq!(invocation.subagent_type, "explore");
        assert!(invocation.task_id.is_none());

        let resumed = TaskInvocation {
            task_id: Some("task-42".to_string()),
            ..invocation.clone()
        };
        assert_eq!(resumed.task_id.as_deref(), Some("task-42"));
    }

    /// Scenario: every `TaskFailure` variant the doc enumerates has a
    /// constructible match arm. A regression that drops or renames a
    /// variant fails this test even before any consumer code breaks,
    /// which keeps the in-tree taxonomy in lock-step with the doc.
    #[test]
    fn task_failure_variants_match_doc_taxonomy() {
        let cases: Vec<TaskFailure> = vec![
            TaskFailure::FeatureDisabled,
            TaskFailure::UnknownSubagent {
                name: "?".to_string(),
                suggestions: vec!["explore".to_string()],
            },
            TaskFailure::DepthExceeded {
                current: 1,
                limit: 1,
            },
            TaskFailure::ConcurrencyExceeded {
                current: 1,
                limit: 1,
            },
            TaskFailure::PermissionEscalationDenied {
                permission: "edit".to_string(),
                pattern: "*".to_string(),
            },
            TaskFailure::SafetyDenied(SafetyDecisionDenial {
                reason: "x".to_string(),
            }),
            TaskFailure::ApprovalRejected { feedback: None },
            TaskFailure::BudgetExceeded(BudgetExceededReason::CostHardCap),
            TaskFailure::ContextHandoffFailed(ContextHandoffError::NoFrameAvailable),
            TaskFailure::ProviderError(CompletionError::ProviderError("x".into())),
            TaskFailure::ChildToolLoopFailed(ToolLoopError::StepBudgetExhausted { steps: 1 }),
            TaskFailure::Cancelled {
                source: CancellationSource::ParentAbort,
            },
            TaskFailure::Timeout { wall_clock_ms: 0 },
        ];
        // Every variant constructs cleanly. Surface a debug print to
        // catch a stray Display/Debug regression too.
        for failure in cases {
            let _ = format!("{failure:?}");
        }
    }

    /// Scenario: `SubAgentDispatcher` is object-safe (the trait must be
    /// usable as `Arc<dyn SubAgentDispatcher>` so the tool loop can
    /// store it generically). This test compiles the trait object
    /// construction explicitly so a future regression that adds a
    /// generic method to the trait fails compilation here, not only at
    /// the eventual store site.
    #[test]
    fn sub_agent_dispatcher_is_object_safe() {
        // We only need the type to exist and be coercible.
        struct NoopDispatcher;
        impl SubAgentDispatcher for NoopDispatcher {
            fn dispatch<'a>(
                &'a self,
                _ctx: DispatchContext<'a>,
                _invocation: TaskInvocation,
                _entry_kind: TaskEntryKind,
            ) -> BoxFuture<'a, Result<TaskResult, TaskFailure>> {
                Box::pin(async move {
                    Err(TaskFailure::UnknownSubagent {
                        name: "noop".to_string(),
                        suggestions: vec![],
                    })
                })
            }
        }
        let dispatcher: Arc<dyn SubAgentDispatcher> = Arc::new(NoopDispatcher);
        // Sanity: drop-it. The compile-time check is the meaningful
        // assertion; this drop just keeps clippy from flagging an
        // unused binding.
        drop(dispatcher);
    }
}
