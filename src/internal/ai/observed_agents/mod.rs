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
pub mod rpc;

pub use adapter::{
    AgentKind, AgentSessionCtx, AgentStability, ObservedAgent, ObservedAgentHooks,
    TranscriptChunker, TranscriptTruncator,
};
pub use builtin::{
    ClaudeCodeObservedAgent, GeminiObservedAgent, STABLE_PROMOTED_SPECS, StablePromotedAgent,
    rfc3339_boundary_for_unix_seconds, stable_promoted_spec_for, write_truncated_transcript,
};
pub use derived::derive_tool_call_records;
pub use preview::{PREVIEW_SPECS, PreviewAgent, PreviewSpec, is_preview, preview_spec_for};
pub use redaction::{
    RedactedBytes, RedactedSink, RedactionMatch, RedactionReport, RedactionRule, Redactor,
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
    // Wrappers ensure each `StablePromotedAgent` lives in `'static`
    // storage so we can return a borrowed reference. We can't put a
    // `StablePromotedAgent` directly in a `static` because it holds a
    // `&'static StablePromotedSpec` (which is fine) but Rust 2024 still
    // requires the wrapping value's address to be stable for `&'static`
    // lifetime extension — `LazyLock<StablePromotedAgent>` provides
    // that without an allocation.
    use std::sync::LazyLock;

    static CURSOR: LazyLock<StablePromotedAgent> = LazyLock::new(|| {
        StablePromotedAgent(
            stable_promoted_spec_for(AgentKind::Cursor).expect("Cursor must have a promoted spec"),
        )
    });
    static CODEX: LazyLock<StablePromotedAgent> = LazyLock::new(|| {
        StablePromotedAgent(
            stable_promoted_spec_for(AgentKind::Codex).expect("Codex must have a promoted spec"),
        )
    });
    static OPENCODE: LazyLock<StablePromotedAgent> = LazyLock::new(|| {
        StablePromotedAgent(
            stable_promoted_spec_for(AgentKind::OpenCode)
                .expect("OpenCode must have a promoted spec"),
        )
    });
    static COPILOT: LazyLock<StablePromotedAgent> = LazyLock::new(|| {
        StablePromotedAgent(
            stable_promoted_spec_for(AgentKind::Copilot)
                .expect("Copilot must have a promoted spec"),
        )
    });
    static FACTORY_AI: LazyLock<StablePromotedAgent> = LazyLock::new(|| {
        StablePromotedAgent(
            stable_promoted_spec_for(AgentKind::FactoryAi)
                .expect("FactoryAi must have a promoted spec"),
        )
    });
    static CLAUDE_CODE: ClaudeCodeObservedAgent = ClaudeCodeObservedAgent::new();
    static GEMINI: GeminiObservedAgent = GeminiObservedAgent::new();

    match kind {
        AgentKind::ClaudeCode => &CLAUDE_CODE,
        AgentKind::Gemini => &GEMINI,
        AgentKind::Cursor => &*CURSOR,
        AgentKind::Codex => &*CODEX,
        AgentKind::OpenCode => &*OPENCODE,
        AgentKind::Copilot => &*COPILOT,
        AgentKind::FactoryAi => &*FACTORY_AI,
    }
}

#[cfg(test)]
mod registry_tests {
    use super::*;

    /// `agent_for` must return an adapter for every [`AgentKind`], and
    /// the adapter's `provider_kind()` must match the requested kind.
    /// The exhaustive `match` in `agent_for` already forces a future
    /// variant to add a registration; this test pins the
    /// kind-round-trip invariant so a refactor that wires a new
    /// variant to the wrong adapter fails here.
    #[test]
    fn agent_for_returns_matching_kind_for_every_variant() {
        for kind in AgentKind::all() {
            let agent = agent_for(*kind);
            assert_eq!(
                agent.provider_kind(),
                *kind,
                "agent_for({kind:?}) returned wrong kind",
            );
            assert_eq!(
                agent.stability(),
                AgentStability::Stable,
                "agent_for({kind:?}) must report Stable tier — \
                 preview specs are not registered here",
            );
        }
    }

    /// Multiple calls to `agent_for` for the same kind must return the
    /// same `'static` reference so callers can cheaply cache an
    /// adapter handle without indirection.
    #[test]
    fn agent_for_returns_stable_static_references_across_calls() {
        for kind in AgentKind::all() {
            let a = agent_for(*kind);
            let b = agent_for(*kind);
            assert!(
                std::ptr::eq(
                    a as *const dyn ObservedAgent as *const (),
                    b as *const dyn ObservedAgent as *const (),
                ),
                "agent_for({kind:?}) must return the same &'static reference on every call",
            );
        }
    }
}
