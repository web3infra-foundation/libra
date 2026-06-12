//! CEX-S2-18 (Step 2.8): read-only Evidence → Memory Distillation query API.
//!
//! Three read-only accessors over sub-agent [`AgentEvidence`] and the
//! [`MergeDecision`] distillable-evidence list. Read-only **by design**
//! (CEX-S2-18 (5)): no distillation pipeline, no AI summarisation, no
//! cross-session auto-loading, and — critically — **no write path**.
//! The `distillable_evidence_ids` write is owned by CEX-S2-15 and the
//! evidence persistence by CEX-S2-14/15; this module only reads.
//!
//! The query functions take the evidence collection **explicitly** so
//! they stay pure and unit-testable ahead of the projection that will
//! feed them once evidence is persisted. Querying an empty collection
//! returns an empty result (never panics) — exactly the flag-off
//! behaviour CEX-S2-18 (3) requires (under `code.sub_agents.enabled =
//! false` no `AgentEvidence` is ever persisted, so every query is
//! empty).

use futures::Stream;

use super::{AgentEvidence, AgentType, AnchorScope, EvidenceId, MergeDecision};

/// AND-combined filter for [`evidence_stream`]. An unset field (`None`
/// / `false`) does not constrain the result; [`EvidenceFilter::default`]
/// matches every record.
#[derive(Clone, Debug, Default)]
pub struct EvidenceFilter {
    /// Keep only evidence whose `applies_to_scope` equals this scope.
    pub scope: Option<AnchorScope>,
    /// Keep only evidence produced by this sub-agent type.
    pub source_agent_type: Option<AgentType>,
    /// Keep only evidence the sub-agent flagged `distillable = true`.
    pub distillable_only: bool,
}

impl EvidenceFilter {
    fn matches(&self, evidence: &AgentEvidence) -> bool {
        self.scope
            .is_none_or(|scope| evidence.applies_to_scope == scope)
            && self
                .source_agent_type
                .is_none_or(|kind| evidence.source_agent_type == kind)
            && (!self.distillable_only || evidence.distillable)
    }
}

/// Every evidence record that applies to exactly `scope`, in input
/// order. Read-only.
pub fn evidence_query_by_scope(
    evidence: &[AgentEvidence],
    scope: AnchorScope,
) -> Vec<AgentEvidence> {
    evidence
        .iter()
        .filter(|item| item.applies_to_scope == scope)
        .cloned()
        .collect()
}

/// Evidence matching `filter`, surfaced as a [`Stream`] so a future
/// projection can back this with async IO without changing the public
/// signature. Read-only; preserves input order.
pub fn evidence_stream(
    evidence: &[AgentEvidence],
    filter: &EvidenceFilter,
) -> impl Stream<Item = AgentEvidence> {
    let matched: Vec<AgentEvidence> = evidence
        .iter()
        .filter(|item| filter.matches(item))
        .cloned()
        .collect();
    futures::stream::iter(matched)
}

/// The `distillable = true` evidence ids CEX-S2-15 recorded on a
/// [`MergeDecision`]. Read path only (CEX-S2-18 (2)); the write path is
/// owned by CEX-S2-15, which fills `MergeDecisionPayloadV0::
/// distillable_evidence_ids` before the decision is persisted.
pub fn merge_decision_distillable_evidence(decision: &MergeDecision) -> Vec<EvidenceId> {
    decision.payload.distillable_evidence_ids.clone()
}
