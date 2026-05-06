//! Top-level `Event` trait — CEX-00.5 deliverable.
//!
//! All append-only event types in the agent runtime should implement this
//! trait so that `AuditSink` and future projection rebuild code can treat
//! them uniformly. The trait is dyn-compatible (no generics on methods, no
//! `Self: Sized` bounds beyond what `&dyn Event` requires) so callers can
//! pass `&dyn Event` to record-event APIs.
//!
//! # Envelope-with-typed-payload requirement (R-A3)
//!
//! Per `docs/improvement/agent.md` "Step 2 audit closure" R-A3, every concrete
//! `Event` implementation **MUST** serialize as an envelope-with-typed-payload
//! shape so that an old reader can skip-and-warn instead of failing on a
//! variant it has never seen before:
//!
//! 1. The wire form is `{"kind": "<snake_case>", "payload": <object>}` — i.e.
//!    serde `tag = "kind", content = "payload"` for adjacent enums, or an
//!    explicit struct with the same shape.
//! 2. Readers consume the event through a wire-level wrapper (typically an
//!    `untagged` enum with `Known(Box<E>)` and `Unknown(Value)` variants —
//!    see `crate::internal::ai::agent_run::AgentRunEventEnvelope` for the
//!    canonical pattern).
//!
//! `#[serde(other)]` on a unit variant inside the inner enum is **not
//! sufficient**, because future variants will carry payloads (maps), and a
//! unit catch-all rejects map content. The wrapper-based approach is the only
//! pattern that satisfies S2-INV-10 unconditionally.
//!
//! This trait does not enforce the wire format at compile time (Rust traits
//! cannot constrain serde derive output), but every implementor in the agent
//! runtime is expected to follow it. CEX-00.5 freezes the contract; future
//! CEX cards adding events MUST follow the same envelope pattern or document
//! the deviation in the audit closure.
//!
//! # ADR — doc-only enforcement of the envelope (CEX-00.5 Codex review P1-b)
//!
//! Codex's CEX-00.5 review flagged that R-A3 is documented but not enforced.
//! The accepted trade-off, recorded here as a mini-ADR:
//!
//! - **Decision**: enforce the envelope through (a) module documentation,
//!   (b) the canonical implementation in
//!   `crate::internal::ai::agent_run::AgentRunEventEnvelope`, and (c) PR
//!   review of any new `Event` impl. Compile-time enforcement (e.g., a
//!   generic `EventEnvelope<T>` requiring all events to round-trip through
//!   it) is intentionally not done in this CEX.
//! - **Why doc-only**: a generic envelope wrapper would force every
//!   `Event`-implementing struct to expose its serde-derive-friendly payload
//!   shape through an intermediate type, complicating both struct-level
//!   `#[serde(deny_unknown_fields)]` and existing impls (`LifecycleEvent`
//!   currently has no `Serialize`/`Deserialize` derives at all). The cost
//!   outweighs the benefit when the agent runtime has only two `Event`
//!   implementors today.
//! - **Follow-up**: when a third `Event` implementor lands (Step 1.8 JSONL
//!   session events, Step 1.9 `ContextFrame` / `CompactionEvent`, or Step
//!   1.10 automation events), a separate CEX card should re-evaluate the
//!   envelope-wrapper approach. Until then, the doc + PR-review path is the
//!   stop-gap.

use uuid::Uuid;

/// Marker + metadata trait for append-only domain events.
///
/// Implementors include:
/// - `crate::internal::ai::hooks::lifecycle::LifecycleEvent` (provider hook
///   lifecycle stream).
/// - `crate::internal::ai::agent_run::AgentRunEvent` (Step 2 sub-agent event
///   stream — gated behind `subagent-scaffold`).
///
/// Future event types (compaction events, automation events, sub-agent
/// merge decisions) plug into the same trait.
pub trait Event: Send + Sync {
    /// Stable kind discriminator in `snake_case`. **Must not change** once an
    /// event has shipped — readers may dispatch on this string verbatim.
    fn event_kind(&self) -> &'static str;

    /// Stable id for this event occurrence. For events that have a natural id
    /// (e.g. an `EventId` newtype) return it as a `Uuid`; for events without
    /// one return `Uuid::nil()` and document why.
    fn event_id(&self) -> Uuid;

    /// One-line summary used as the human-readable description of the event
    /// in audit channels.
    ///
    /// # Redaction contract
    ///
    /// **Implementors do NOT need to redact.** The default
    /// `AuditSink::record_event` impl runs every summary through the
    /// supplied `SecretRedactor` before populating
    /// `AuditEvent.redacted_summary`. The summary may therefore contain
    /// raw user-controlled strings (tool names, free-form reasons,
    /// session ids), and the sink path is the choke point that enforces
    /// redaction.
    ///
    /// **Do not** call `event_summary()` and emit the result directly into
    /// a log file or external system without piping it through a redactor —
    /// the trait makes no in-string secret guarantee. Use
    /// `AuditSink::record_event` (or pass the result through
    /// `SecretRedactor::redact` yourself) when persisting.
    fn event_summary(&self) -> String;
}

/// Default-impl helper used by `AuditSink::record_event` to produce a stable
/// `action` string (`event/<kind>`) when forwarding an event to the audit
/// channel.
pub fn audit_action_for(event: &dyn Event) -> String {
    format!("event/{}", event.event_kind())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummyEvent {
        id: Uuid,
    }

    impl Event for DummyEvent {
        fn event_kind(&self) -> &'static str {
            "dummy"
        }

        fn event_id(&self) -> Uuid {
            self.id
        }

        fn event_summary(&self) -> String {
            "dummy event".to_string()
        }
    }

    #[test]
    fn event_trait_is_dyn_compatible() {
        let e = DummyEvent { id: Uuid::nil() };
        let dyn_ref: &dyn Event = &e;
        assert_eq!(dyn_ref.event_kind(), "dummy");
        assert_eq!(audit_action_for(dyn_ref), "event/dummy");
    }
}
