//! Append-only event types for sub-agent runs.
//!
//! # Unknown-event-safe envelope (S2-INV-10 / R-A3)
//!
//! Two-layer pattern:
//!
//! 1. `AgentRunEvent` uses `tag = "kind", content = "payload"` for the known
//!    variants. New variants append cleanly; existing variants do not break
//!    when extra payload fields appear.
//! 2. `AgentRunEventEnvelope` is the **wire-level** type readers should use
//!    when parsing JSONL. It is an `untagged` enum that tries `Known` first
//!    and falls back to `Unknown(serde_json::Value)` for any tag the current
//!    reader does not recognize. This satisfies S2-INV-10 / R-A3: an old
//!    reader will skip-and-warn instead of erroring out on a future event
//!    type.
//!
//! `#[serde(other)]` on the inner enum cannot work here because future
//! variants will carry payloads (maps), and `#[serde(other)]` requires a
//! unit catch-all that ignores the content. The two-layer pattern delegates
//! that responsibility to the outer envelope.
//!
//! CEX-00.5 is expected to lift this exact pattern into a generic `Event`
//! trait; until then the pattern lives here directly.
//!
//! # Hook dispatch schema freeze (CEX-S2-10 (5) / S2-INV-13)
//!
//! Per the audit closure, `HookInvocationPayload` and the five outcome
//! variants (`HookPassed` / `BlockedByHook` / `HookRequestedHuman` /
//! `BlockedByHookFailure` / `PostToolReviewRequired`) are frozen here.
//! CEX-S2-12 hook dispatch implementation may NOT add fields to these types;
//! field additions require a new CEX-S2-* card.

#![cfg(feature = "subagent-scaffold")]

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::{AgentRunId, ApprovalRequestId, BudgetDimension, PackageId, Sha256, ToolCallId};

// ----------------------------------------------------------------------------
// Hook dispatch schema (S2-INV-13 / CEX-S2-10 (5))
// ----------------------------------------------------------------------------

/// Phase of the hook dispatch lifecycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookPhase {
    PreToolUse,
    PostToolUse,
}

/// Where the hook executable lives. `CapabilityPackage` is forward-declared
/// for CEX-S2-17 capability packages.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum HookKind {
    Builtin,
    ProjectLocal,
    UserLocal,
    CapabilityPackage { package_id: PackageId },
}

/// Per-invocation context passed to every hook outcome variant. Frozen by
/// CEX-S2-10 (5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookInvocationPayload {
    pub phase: HookPhase,
    pub tool_name: String,
    pub tool_call_id: ToolCallId,
    pub agent_run_id: AgentRunId,
    pub hook_path: PathBuf,
    pub hook_checksum: Sha256,
    pub hook_kind: HookKind,
    /// JSON event the hook received on stdin. Stored verbatim for replay /
    /// audit; size is bounded by the truncation rules referenced below.
    pub stdin_event_json: String,
    pub timeout_ms: u32,
}

/// Reason a hook failed in a way that we mapped to `deny` (fail-closed). The
/// values match the table at "Step 2.2 Hook exit-code 权威映射表" verbatim.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum HookFailureReason {
    /// Exit code was not 0/2/3; treated as deny per fail-closed default.
    UnknownExitCode { exit_code: i32 },
    /// Hook process panicked / aborted with no exit code.
    Panic,
    /// Hook exceeded `[hooks].timeout_ms` (default 30s).
    Timeout,
    /// Hook killed by an OS signal; signal number recorded.
    KilledBySignal { signo: i32 },
    /// `execve(2)` returned ENOENT (hook binary not found).
    SpawnEnoent,
    /// `execve(2)` returned EACCES (binary not executable).
    SpawnEacces,
    /// `needs-human` (exit 3) waited longer than `[hooks].needs_human_timeout_ms`
    /// (default 10 min).
    NeedsHumanTimeout,
    /// Catch-all fallback when no specific reason applies. **Never** used by
    /// `BlockedByHook` / `HookRequestedHuman` (those use `hook_reason: None`).
    Unspecified,
}

/// Reason payload for `AgentRunEvent::PostToolReviewRequired`.
///
/// Per the audit-closure schema in `docs/improvement/agent.md` Step 2.2 hook
/// table: same variant set as `HookFailureReason` PLUS the two PostToolUse-
/// only literals `hook_deny` / `hook_needs_human`. The variants are listed
/// flat (not wrapped in a `Failure(HookFailureReason)` newtype) so the wire
/// schema matches the doc literal shape.
///
/// `SpawnEnoent` / `SpawnEacces` are technically unreachable in PostToolUse
/// (the dispatch already happened), but they are listed here for schema-mirror
/// completeness with `HookFailureReason`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum PostToolReason {
    /// PostToolUse-only: hook returned exit 2 after dispatch.
    HookDeny,
    /// PostToolUse-only: hook returned exit 3 after dispatch.
    HookNeedsHuman,
    /// Mirrors `HookFailureReason::UnknownExitCode`.
    UnknownExitCode { exit_code: i32 },
    /// Mirrors `HookFailureReason::Panic`.
    Panic,
    /// Mirrors `HookFailureReason::Timeout`.
    Timeout,
    /// Mirrors `HookFailureReason::KilledBySignal`.
    KilledBySignal { signo: i32 },
    /// Mirrors `HookFailureReason::SpawnEnoent`. Unreachable in PostToolUse
    /// because spawn failure happens pre-dispatch; listed for schema mirror.
    SpawnEnoent,
    /// Mirrors `HookFailureReason::SpawnEacces`. Same unreachability note.
    SpawnEacces,
    /// Mirrors `HookFailureReason::NeedsHumanTimeout`.
    NeedsHumanTimeout,
    /// Mirrors `HookFailureReason::Unspecified`.
    Unspecified,
}

// ----------------------------------------------------------------------------
// Workspace materialization event (CEX-S2-11 forward-stable schema)
// ----------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceStrategy {
    /// `.git` < 1GB and worktree files < 100K (default).
    Worktree,
    /// `.git` ≥ 1GB or worktree files ≥ 100K.
    Sparse,
    /// User explicitly enabled `agent.allow_full_copy = true`.
    FullCopy,
    /// Write scope outside materialized paths; task blocked.
    Blocked,
}

/// Payload for `AgentRunEvent::WorkspaceMaterialized`. Per CEX-S2-11 (3),
/// every workspace creation writes one of these.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceMaterialized {
    pub strategy: WorkspaceStrategy,
    pub elapsed_ms: u64,
    pub materialized_file_count: u64,
    pub source_repo_size: u64,
    /// Reason for fallback to a less-preferred strategy (e.g. "worktree
    /// reservation failed: <error>"). Empty string when no fallback occurred.
    #[serde(default)]
    pub fallback_reason: String,
}

// ----------------------------------------------------------------------------
// RunUsage event (per-`agent_run_id` token / latency / cost aggregation)
// ----------------------------------------------------------------------------

/// `RunUsage[E]` shares its dimension fields with the Step 1.11
/// `agent_usage_stats` SQLite schema so the row insert can be a direct copy.
/// Owned by CEX-S2-10 (this file) per the core-objects table; values written
/// by CEX-S2-12 after each provider call ends.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cached_tokens: u64,
    pub reasoning_tokens: u64,
    pub wall_clock_ms: u64,
    pub provider_latency_ms: u64,
    pub cost_estimate_micro_dollars: u64,
    pub tool_call_count: u32,
}

// ----------------------------------------------------------------------------
// AgentRunEvent — append-only stream
// ----------------------------------------------------------------------------

/// Reason payload for `AgentRunEvent::Failed`. Free-form `String` keeps the
/// schema stable across Step 2 development; CEX-S2-12 may refine.
pub type FailureReason = String;

/// Reason payload for `AgentRunEvent::Cancelled`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancellationReason {
    UserRequested,
    LayerOneTimeout,
    Other(String),
}

/// All sub-agent lifecycle events. The envelope is `tag = "kind"` /
/// `content = "payload"` with `Unknown` as the catch-all for forward
/// compatibility (S2-INV-10). Field-level invariants:
///
/// - **Single-run events** carry `agent_run_id`. Every variant in this enum is
///   currently single-run.
/// - **Aggregate events** (none here — `MergeDecision` lives in `decision.rs`
///   and uses `merge_candidate_id + agent_run_ids`) are written by separate
///   producers.
///
/// Hook variants embed the hook payload **inline** rather than carrying a
/// generic `HookOutcome` enum. This makes `AgentRunEvent::HookPassed` exclude
/// `BlockedByHook`-shaped payloads at the type level (no `kind=hook_passed`
/// row whose body contradicts the variant name) and lets serde produce a
/// flat JSON shape per outcome.
///
/// CEX-S2-12 + CEX-S2-15 may emit any of these. The wire-level
/// `AgentRunEventEnvelope` (below) wraps this enum with a catch-all
/// `Unknown(Value)` for forward-compatible deserialization; producers always
/// emit a recognized variant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload", rename_all = "snake_case")]
pub enum AgentRunEvent {
    Started {
        agent_run_id: AgentRunId,
    },
    ToolCall {
        agent_run_id: AgentRunId,
        tool_call_id: ToolCallId,
        tool_name: String,
    },
    Blocked {
        agent_run_id: AgentRunId,
        reason: String,
    },
    Completed {
        agent_run_id: AgentRunId,
    },
    Failed {
        agent_run_id: AgentRunId,
        reason: FailureReason,
    },
    Cancelled {
        agent_run_id: AgentRunId,
        reason: CancellationReason,
    },
    TimedOut {
        agent_run_id: AgentRunId,
    },
    BudgetExceeded {
        agent_run_id: AgentRunId,
        dimension: BudgetDimension,
    },
    /// Hook returned exit 0 (or empty stdout); dispatch continues.
    HookPassed {
        agent_run_id: AgentRunId,
        invocation: HookInvocationPayload,
        empty_stdout: bool,
    },
    /// PreToolUse hook returned exit 2 (or fail-closed deny). Dispatch
    /// blocked. Used ONLY in PreToolUse; PostToolUse exit 2 maps to
    /// `PostToolReviewRequired` instead.
    BlockedByHook {
        agent_run_id: AgentRunId,
        invocation: HookInvocationPayload,
        exit_code: i32,
        stdout_truncated: String,
        stderr_truncated: String,
        /// `None` when stdout was empty (per Step 2.2 table "exit 2/3 + 空
        /// stdout" row). Never `Some("unspecified")`.
        hook_reason: Option<String>,
    },
    /// PreToolUse hook returned exit 3; dispatch paused waiting for Layer 1
    /// approval. Approval request id supplied so the response can be matched.
    HookRequestedHuman {
        agent_run_id: AgentRunId,
        invocation: HookInvocationPayload,
        hook_reason: Option<String>,
        approval_request_id: ApprovalRequestId,
    },
    /// Hook process failed in a way that maps to deny fail-closed (panic,
    /// timeout, signal, spawn error, etc.). Used ONLY in PreToolUse phase;
    /// PostToolUse failures use `PostToolReviewRequired`.
    BlockedByHookFailure {
        agent_run_id: AgentRunId,
        invocation: HookInvocationPayload,
        reason: HookFailureReason,
        stdout_truncated: String,
        stderr_truncated: String,
    },
    /// PostToolUse-stage decision when the tool result was already produced
    /// but the hook signals a problem (deny, needs-human, panic, timeout,
    /// signal). Routes to Layer 1 review without retroactively cancelling
    /// the dispatched tool call. Reason is the flat `PostToolReason` union.
    PostToolReviewRequired {
        agent_run_id: AgentRunId,
        invocation: HookInvocationPayload,
        reason: PostToolReason,
        stdout_truncated: String,
        stderr_truncated: String,
    },
    WorkspaceMaterialized {
        agent_run_id: AgentRunId,
        materialization: WorkspaceMaterialized,
    },
    RunUsage {
        agent_run_id: AgentRunId,
        usage: RunUsage,
    },
}

/// Wire-level wrapper for `AgentRunEvent` that lets old readers parse newer
/// streams without errors.
///
/// Always deserialize JSONL lines through this type. Match on `Known` for the
/// recognized event types and on `Unknown` to skip-and-warn for events
/// emitted by a future Step 2 / Step 3 implementation. Producers always emit
/// `Known(...)`; `Unknown` is a parse-time concept only.
///
/// `Known` is boxed because `AgentRunEvent` is a relatively large sum type
/// (its biggest variant carries a `HookInvocationPayload` plus stdout/stderr
/// strings inline); the boxing keeps the envelope cheap to pass around and
/// silences the `large_enum_variant` lint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AgentRunEventEnvelope {
    Known(Box<AgentRunEvent>),
    /// A JSONL row whose `kind` field is not recognized by this reader.
    /// Carries the raw JSON so audit / replay code can preserve it verbatim.
    Unknown(serde_json::Value),
}

impl AgentRunEventEnvelope {
    /// Returns the recognized event, or `None` for `Unknown`.
    pub fn known(&self) -> Option<&AgentRunEvent> {
        match self {
            Self::Known(event) => Some(event.as_ref()),
            Self::Unknown(_) => None,
        }
    }

    /// Returns `true` for an unknown / future variant.
    pub fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown(_))
    }
}

impl From<AgentRunEvent> for AgentRunEventEnvelope {
    fn from(event: AgentRunEvent) -> Self {
        Self::Known(Box::new(event))
    }
}
