//! External-Agent capture (CEX-EntireIO).
//!
//! This module owns the runtime that observes lifecycle events from
//! externally-hosted AI agents (Claude Code, Gemini CLI, Cursor, …) and
//! materialises them into Libra's catalog (`agent_session`, `agent_checkpoint`)
//! plus the `refs/libra/agent-traces` orphan ref. See
//! `docs/improvement/entire.md` (sections 5–8) for the design.
//!
//! Sub-modules:
//!
//! - [`adapter`]: the small core trait [`adapter::ObservedAgent`] every captured
//!   agent must implement, plus the optional capability traits
//!   ([`adapter::ObservedAgentHooks`], [`adapter::TranscriptTruncator`],
//!   [`adapter::TranscriptChunker`]).
//! - [`redaction`]: the [`redaction::Redactor`] engine and the
//!   [`redaction::RedactedBytes`] compile-time contract — only redacted bytes
//!   may flow into checkpoint storage.
//!
//! Phase 1 (this module's first cut) only ships traits, the redaction engine,
//! and the migration that backs the catalog. Phase 2 wires checkpoint
//! generation; Phase 3 wires the cloud-sync hooks.

pub mod adapter;
pub mod builtin;
pub mod derived;
pub mod preview;
pub mod redaction;
#[cfg(test)]
mod registry_tests;
pub mod rpc;

pub use adapter::{
    AgentKind, AgentSessionCtx, AgentStability, ObservedAgent, ObservedAgentHooks,
    TranscriptChunker, TranscriptTruncator,
};
use builtin::stable_promoted::{
    CODEX_STABLE_PROMOTED_SPEC, COPILOT_STABLE_PROMOTED_SPEC, CURSOR_STABLE_PROMOTED_SPEC,
    FACTORY_AI_STABLE_PROMOTED_SPEC, OPENCODE_STABLE_PROMOTED_SPEC,
};
pub use builtin::{
    ClaudeCodeObservedAgent, GeminiObservedAgent, STABLE_PROMOTED_SPECS, StablePromotedAgent,
    rfc3339_boundary_for_unix_seconds, stable_promoted_spec_for, write_truncated_transcript,
};
pub use derived::derive_tool_call_records;
pub use preview::{PREVIEW_SPECS, PreviewAgent, PreviewSpec, is_preview, preview_spec_for};
pub use redaction::{
    PiiConfig, RedactedBytes, RedactedSink, RedactionMatch, RedactionMode, RedactionReport,
    RedactionRule, Redactor,
};
pub use rpc::{
    RPC_BINARY_PREFIX, RPC_DEFAULT_TIMEOUT, RpcAgent, RpcAgentBinary, RpcError, RpcRequest,
    RpcResponse, discover_rpc_agents,
};

/// Borrow the static [`ObservedAgent`] for the supplied [`AgentKind`].
///
/// This is the single dispatch entry point downstream callers (the
/// hook runtime, `libra agent` subcommands, the checkpoint writer)
/// use to find the adapter for a kind without hard-coding the
/// dedicated-vs-promoted split. The two original stable kinds
/// (`ClaudeCode`, `Gemini`) resolve to their hand-written struct;
/// the five Phase 4.4-promoted kinds (`Cursor`, `Codex`, `OpenCode`,
/// `Copilot`, `FactoryAi`) resolve to a `&'static StablePromotedAgent`
/// borrowed from a per-kind static cell so the function can return a
/// `&'static dyn ObservedAgent` for every kind without per-call
/// allocation.
///
/// The function is total over [`AgentKind`]: the exhaustive `match`
/// arms force a future variant to add its own registration in the
/// same patch, which is the same compile-time guard the v0.17.660+
/// `*::all()` enumerators established.
pub fn agent_for(kind: AgentKind) -> &'static dyn ObservedAgent {
    static CURSOR: StablePromotedAgent = StablePromotedAgent(&CURSOR_STABLE_PROMOTED_SPEC);
    static CODEX: StablePromotedAgent = StablePromotedAgent(&CODEX_STABLE_PROMOTED_SPEC);
    static OPENCODE: StablePromotedAgent = StablePromotedAgent(&OPENCODE_STABLE_PROMOTED_SPEC);
    static COPILOT: StablePromotedAgent = StablePromotedAgent(&COPILOT_STABLE_PROMOTED_SPEC);
    static FACTORY_AI: StablePromotedAgent = StablePromotedAgent(&FACTORY_AI_STABLE_PROMOTED_SPEC);
    static CLAUDE_CODE: ClaudeCodeObservedAgent = ClaudeCodeObservedAgent::new();
    static GEMINI: GeminiObservedAgent = GeminiObservedAgent::new();

    match kind {
        AgentKind::ClaudeCode => &CLAUDE_CODE,
        AgentKind::Gemini => &GEMINI,
        AgentKind::Cursor => &CURSOR,
        AgentKind::Codex => &CODEX,
        AgentKind::OpenCode => &OPENCODE,
        AgentKind::Copilot => &COPILOT,
        AgentKind::FactoryAi => &FACTORY_AI,
    }
}

/// Return the static [`TranscriptTruncator`] adapter for the supplied
/// kind, or `None` when the adapter does not implement that optional
/// capability.
///
/// Companion to [`agent_for`] for the
/// `libra agent checkpoint rewind --apply` dispatch path. As of the
/// entire.md §14.4 phase-4 work, [`ClaudeCodeObservedAgent`] (JSONL,
/// per-line `timestamp`), [`GeminiObservedAgent`] (single JSON doc,
/// `messages[].timestamp`), and the stable-promoted Cursor / Codex /
/// OpenCode / Copilot adapters implement the truncator trait. Factory
/// AI Droid returns `None` because its parsed transcript envelope does
/// not carry a stable timestamp boundary.
///
/// Adding a further truncator capability is a two-step process:
/// 1. Implement `TranscriptTruncator` on the adapter struct.
/// 2. Add a `match` arm here returning `Some(&STATIC_INSTANCE)`.
///
/// The exhaustive match below makes step 2 a compile-time obligation
/// — a new variant added to `AgentKind` without a corresponding arm
/// here fails to build, which prevents the silent
/// "adapter exists but its truncator isn't wired" bug class.
pub fn truncator_for(kind: AgentKind) -> Option<&'static dyn TranscriptTruncator> {
    static CLAUDE_CODE_TRUNCATOR: ClaudeCodeObservedAgent = ClaudeCodeObservedAgent::new();
    static GEMINI_TRUNCATOR: GeminiObservedAgent = GeminiObservedAgent::new();
    static CURSOR_TRUNCATOR: StablePromotedAgent =
        StablePromotedAgent(&CURSOR_STABLE_PROMOTED_SPEC);
    static CODEX_TRUNCATOR: StablePromotedAgent = StablePromotedAgent(&CODEX_STABLE_PROMOTED_SPEC);
    static OPENCODE_TRUNCATOR: StablePromotedAgent =
        StablePromotedAgent(&OPENCODE_STABLE_PROMOTED_SPEC);
    static COPILOT_TRUNCATOR: StablePromotedAgent =
        StablePromotedAgent(&COPILOT_STABLE_PROMOTED_SPEC);

    match kind {
        AgentKind::ClaudeCode => Some(&CLAUDE_CODE_TRUNCATOR),
        AgentKind::Gemini => Some(&GEMINI_TRUNCATOR),
        AgentKind::Cursor => Some(&CURSOR_TRUNCATOR),
        AgentKind::Codex => Some(&CODEX_TRUNCATOR),
        AgentKind::OpenCode => Some(&OPENCODE_TRUNCATOR),
        AgentKind::Copilot => Some(&COPILOT_TRUNCATOR),
        AgentKind::FactoryAi => None,
    }
}
