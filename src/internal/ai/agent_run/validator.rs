//! CEX-S2-15 ValidatorEngine — merge-candidate validation helpers (Step 2.5).
//!
//! The ValidatorEngine validates an aggregated `MergeCandidate` before the
//! human merge decision and fills the CEX-S2-13-frozen `MergeDecisionPayloadV0`
//! fields: it computes the `risk_score` (see [`super::compute_merge_risk_score`]),
//! triggers the verification test DAG to produce `test_evidence`, and — per
//! CEX-S2-15 完成判定 (2) — scans every `AgentEvidence` involved in the merge
//! for `distillable = true` entries and records their ids in
//! `distillable_evidence_ids` before the `MergeDecision` is persisted.
//!
//! This module currently owns the pure, side-effect-free distillable-scan step.
//! The orchestrator-wired pieces — triggering the test DAG and routing a
//! validation failure back to the originating `AgentTask` — touch
//! `orchestrator::verifier` and land separately.

use super::{AgentEvidence, EvidenceId};

/// Collect the ids of every `distillable = true` evidence record, in input
/// order, ready to fill `MergeDecisionPayloadV0::distillable_evidence_ids`
/// before a `MergeDecision` is persisted (CEX-S2-15 完成判定 (2)).
///
/// This is the **write-side** scan that produces the ids;
/// [`super::merge_decision_distillable_evidence`] is the complementary
/// CEX-S2-18 **read-side** accessor that reads them back off a persisted
/// `MergeDecision`. The function is pure — it derives ids only and performs no
/// persistence (the `MergeDecision` write itself stays with the caller), so it
/// never mutates the CEX-S2-13-frozen schema.
pub fn collect_distillable_evidence_ids(evidence: &[AgentEvidence]) -> Vec<EvidenceId> {
    evidence
        .iter()
        .filter(|item| item.distillable)
        .map(|item| item.id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        super::{
            AgentRunId, AgentType, AnchorScope, Confidence, DecisionId, EventId, MergeCandidateId,
            MergeDecision, MergeDecisionPayloadV0, ReviewState,
            merge_decision_distillable_evidence,
        },
        *,
    };

    /// Build an `AgentEvidence` with the distillable flag under test and the
    /// rest filled with fresh defaults (mirrors the CEX-S2-18 read-side test
    /// helper so both sides exercise the same shape).
    fn evidence(distillable: bool) -> AgentEvidence {
        AgentEvidence {
            id: EvidenceId::new(),
            agent_run_id: AgentRunId::new(),
            source_agent_type: AgentType::Worker,
            source_event_id: EventId::new(),
            tool_call_id: None,
            source_call_id: None,
            confidence: Confidence::new(0.9),
            applies_to_scope: AnchorScope::AgentRun,
            distillable,
            evidence_snapshot_id: uuid::Uuid::new_v4(),
        }
    }

    /// An empty corpus yields no ids and never panics — the flag-off shape
    /// (under `code.sub_agents.enabled = false` no `AgentEvidence` is
    /// persisted, so the scan always sees an empty slice).
    #[test]
    fn empty_corpus_collects_nothing() {
        assert!(collect_distillable_evidence_ids(&[]).is_empty());
    }

    /// Only `distillable = true` records are collected; non-distillable
    /// evidence is skipped and input order is preserved.
    #[test]
    fn collects_only_distillable_ids_in_order() {
        let corpus = vec![
            evidence(true),
            evidence(false),
            evidence(true),
            evidence(false),
        ];
        let ids = collect_distillable_evidence_ids(&corpus);
        assert_eq!(ids, vec![corpus[0].id, corpus[2].id]);
    }

    /// A non-empty corpus with nothing distillable collects nothing.
    #[test]
    fn no_distillable_yields_empty() {
        let corpus = vec![evidence(false), evidence(false)];
        assert!(collect_distillable_evidence_ids(&corpus).is_empty());
    }

    /// An all-distillable corpus collects every id, order preserved.
    #[test]
    fn all_distillable_collects_every_id() {
        let corpus = vec![evidence(true), evidence(true), evidence(true)];
        let ids = collect_distillable_evidence_ids(&corpus);
        assert_eq!(ids, corpus.iter().map(|item| item.id).collect::<Vec<_>>());
    }

    /// Round-trip: the write-side ids this function collects read back
    /// identically through the CEX-S2-18 read accessor once stored on a
    /// `MergeDecision`, pinning the write ↔ read symmetry across the two CEXes.
    #[test]
    fn collected_ids_round_trip_through_read_accessor() {
        let corpus = vec![evidence(true), evidence(false), evidence(true)];
        let collected = collect_distillable_evidence_ids(&corpus);

        let decision = MergeDecision {
            id: DecisionId::new(),
            merge_candidate_id: MergeCandidateId::new(),
            agent_run_ids: vec![AgentRunId::new()],
            resulting_state: ReviewState::NeedsHumanReview,
            payload: MergeDecisionPayloadV0 {
                distillable_evidence_ids: collected.clone(),
                ..MergeDecisionPayloadV0::default()
            },
        };

        assert_eq!(merge_decision_distillable_evidence(&decision), collected);
    }
}
