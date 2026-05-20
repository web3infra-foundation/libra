//! Step 2 sub-agent contracts (CEX-S2-10 schema-only scaffold).
//!
//! # Status
//!
//! This module is **schema-only**: it defines the data types Step 2 will use,
//! but no runtime, no dispatcher, no flag, no behaviour change. It is gated
//! behind the `subagent-scaffold` Cargo feature and **never** linked in the
//! default build. Production callers must not depend on it.
//!
//! # CP-4 gate violation
//!
//! Per `docs/improvement/agent.md` "Step 2 audit closure (CEX-S2-00 / 01 / 02)",
//! all CEX-S2-10..18 Runtime task cards are gated on **CP-4** (Step 1 single-
//! agent gate). Step 1 is currently incomplete (multiple `未开始` cards in the
//! milestone index). This file ships the schema scaffold ahead of CP-4 by
//! explicit user request to unblock parallel design work; production wiring
//! must wait for Step 1 to finish. The feature gate keeps the violation
//! invisible to default builds.
//!
//! # Schema-ownership notes
//!
//! - `AgentEvidence` (in `evidence.rs`) **wraps** the persistent
//!   `git_internal::internal::object::evidence::Evidence` Snapshot rather than
//!   forking a parallel schema. The wrapper adds the raw-fact-chain fields
//!   required by S2-INV-12. See R-A4 in the audit closure for why we don't
//!   touch the runtime `EvidenceKind` enum here.
//! - `AgentTask` / `AgentRun` / `AgentPatchSet` reference (not copy) the
//!   existing `IntentSpec` / `Plan` / `Task` / `Run` / `PatchSet` Snapshots
//!   from `git-internal`.
//! - `MergeDecision` event payload starts as `MergeDecisionPayloadV0` stub;
//!   CEX-S2-13 fills the real payload. We only freeze the **field shape** of
//!   `risk_score` / `conflict_list` / `test_evidence` /
//!   `distillable_evidence_ids` here per CEX-S2-13 ownership rule.
//! - `Event` / `Snapshot` traits owned by CEX-00.5 are **not** introduced in
//!   this scaffold; types here implement only `Serialize` + `Deserialize` and
//!   will pick up the trait bound when CEX-00.5 lands.
//!
//! # Unknown-event-safe pattern
//!
//! Two layers, satisfying S2-INV-10:
//! - `AgentRunEvent` uses `#[serde(tag = "kind", content = "payload")]` for
//!   the recognized variants.
//! - `AgentRunEventEnvelope` is the wire-level wrapper readers should parse;
//!   it is `#[serde(untagged)]` over `Known(AgentRunEvent)` and
//!   `Unknown(Value)` so unknown future tags fall through cleanly without
//!   losing the raw payload.
//!
//! This is the canonical pattern CEX-00.5 will lift to the `Event` trait.
//! `#[serde(other)]` on the inner enum cannot do this on its own because
//! future variants will carry payloads (maps), and `#[serde(other)]` requires
//! a unit catch-all.

#![cfg(feature = "subagent-scaffold")]
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub mod budget;
pub mod context_pack;
pub mod decision;
pub mod event;
pub mod evidence;
pub mod patchset;
pub mod permission;
pub mod run;
pub mod task;

// ----------------------------------------------------------------------------
// Newtype IDs
// ----------------------------------------------------------------------------

macro_rules! uuid_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl From<Uuid> for $name {
            fn from(uuid: Uuid) -> Self {
                Self(uuid)
            }
        }

        impl From<$name> for Uuid {
            fn from(id: $name) -> Self {
                id.0
            }
        }
    };
}

uuid_newtype!(
    /// Identifier for an `AgentTask` (Phase 2 dispatch unit derived from a
    /// confirmed `Task`).
    AgentTaskId
);
uuid_newtype!(
    /// Identifier for an `AgentRun` (one sub-agent execution attempt).
    AgentRunId
);
uuid_newtype!(
    /// Identifier for an `AgentPatchSet` (sub-agent output staged in isolated
    /// workspace).
    AgentPatchSetId
);
uuid_newtype!(
    /// Identifier for a `MergeCandidate` (Layer 1 aggregate of one or more
    /// `AgentPatchSet`s).
    MergeCandidateId
);
uuid_newtype!(
    /// Identifier for an `ApprovalRequest` raised by a sub-agent. Approver
    /// `agent_run_id` MUST differ from request originator (S2-INV-06).
    ApprovalRequestId
);
uuid_newtype!(
    /// Identifier for an `AgentEvidence` event.
    EvidenceId
);
uuid_newtype!(
    /// Identifier for any append-only event in the JSONL stream. Backreferenced
    /// by `AgentEvidence::source_event_id`.
    EventId
);
uuid_newtype!(
    /// Identifier for one tool call dispatch. Component of the trace id chain
    /// `thread_id → agent_run_id → tool_call_id → source_call_id`.
    ToolCallId
);
uuid_newtype!(
    /// Identifier for one Source Pool call. Trailing component of the trace id
    /// chain.
    SourceCallId
);
uuid_newtype!(
    /// Identifier for a `Decision[E]` event (final merge / phase-4 decision).
    DecisionId
);

// ----------------------------------------------------------------------------
// Forward-declared cross-CEX types
// ----------------------------------------------------------------------------

/// Capability package identifier.
///
/// Forward-declared per CEX-S2-10 (5): CEX-S2-17 will replace the inner shape
/// with the real manifest-derived id but must keep the public type signature
/// compatible. Today this is a wrapper over a `String` slug.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PackageId(pub String);

/// SHA-256 digest carried in `HookInvocationPayload::hook_checksum` and other
/// integrity fields. Stored as the 64-character lowercase hex string to keep
/// JSON serialization stable and human-readable.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Sha256(pub String);

/// Anchor scope for evidence, mirroring Step 1.9 `MemoryAnchor` scope so
/// distillation downstream (Step 3.D) can consume `AgentEvidence` directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnchorScope {
    Session,
    AgentRun,
    Project,
}

/// Confidence score attached to evidence (sub-agent self-assessment +
/// verification result, range `0.0..=1.0`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Confidence(pub f32);

impl Confidence {
    pub fn new(value: f32) -> Self {
        Self(value.clamp(0.0, 1.0))
    }
}

// ----------------------------------------------------------------------------
// Re-exports for downstream consumers
// ----------------------------------------------------------------------------

pub use budget::{AgentBudget, BudgetDimension};
pub use context_pack::AgentContextPack;
pub use decision::{
    Conflict, MergeCandidate, MergeDecision, MergeDecisionPayloadV0, ReviewState, RiskScore,
};
pub use event::{
    AgentRunEvent, AgentRunEventEnvelope, CancellationReason, FailureReason, HookFailureReason,
    HookInvocationPayload, HookKind, HookPhase, PostToolReason, RunUsage, WorkspaceMaterialized,
    WorkspaceStrategy,
};
pub use evidence::{AgentEvidence, AgentType};
pub use patchset::AgentPatchSet;
pub use permission::AgentPermissionProfile;
pub use run::{AgentRun, AgentRunStatus};
pub use task::AgentTask;
