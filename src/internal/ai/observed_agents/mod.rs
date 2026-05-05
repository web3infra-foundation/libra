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
pub mod redaction;

pub use adapter::{
    AgentKind, AgentSessionCtx, ObservedAgent, ObservedAgentHooks, TranscriptChunker,
    TranscriptTruncator,
};
pub use redaction::{
    RedactedBytes, RedactedSink, RedactionMatch, RedactionReport, RedactionRule, Redactor,
};
