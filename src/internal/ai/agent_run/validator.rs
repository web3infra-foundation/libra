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

use super::{AgentEvidence, AgentPatchSet, AgentRun, AgentTaskId, EvidenceId};

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

/// Resolve the [`AgentTaskId`] that a failing `patchset` should route back to
/// (CEX-S2-15 验收 (3): "validation fail 可路由回具体 `AgentTask`").
///
/// When validation rejects a sub-agent patch set, Layer 1 must send the failure
/// back to the *specific* task that produced it — not the whole candidate — so
/// the right sub-agent can revise. The link is two hops:
/// `AgentPatchSet.agent_run_id` → the matching [`AgentRun`] → its `task_id`.
///
/// Returns `None` when no run in `runs` owns the patch set (a dangling patch set
/// whose run was never recorded); the caller surfaces that as an internal
/// inconsistency rather than silently dropping the failure. Pure — a lookup over
/// the supplied runs with no I/O.
pub fn resolve_task_for_patchset(
    patchset: &AgentPatchSet,
    runs: &[AgentRun],
) -> Option<AgentTaskId> {
    runs.iter()
        .find(|run| run.id == patchset.agent_run_id)
        .map(|run| run.task_id)
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

    use super::super::{AgentPatchSetId, AgentRunStatus, AgentTaskId};

    fn run(run_id: AgentRunId, task_id: AgentTaskId) -> AgentRun {
        AgentRun {
            id: run_id,
            task_id,
            thread_id: uuid::Uuid::new_v4(),
            provider: "deepseek".to_string(),
            model: "deepseek-chat".to_string(),
            transcript_path: "t.jsonl".to_string(),
            workspace_path: None,
            status: AgentRunStatus::Failed,
        }
    }

    fn patchset(agent_run_id: AgentRunId) -> AgentPatchSet {
        AgentPatchSet {
            id: AgentPatchSetId::new(),
            agent_run_id,
            patchset_id: uuid::Uuid::new_v4(),
            workspace_scope_constrained: false,
        }
    }

    /// CEX-S2-15 验收 (3): a failing patch set routes back to the task of the
    /// run that produced it — the specific task, not the whole candidate.
    #[test]
    fn resolve_task_routes_failure_to_originating_task() {
        let run_a = AgentRunId::new();
        let task_a = AgentTaskId::new();
        let run_b = AgentRunId::new();
        let task_b = AgentTaskId::new();
        let runs = vec![run(run_a, task_a), run(run_b, task_b)];

        // A patch set owned by run_b resolves to task_b, not task_a.
        assert_eq!(
            resolve_task_for_patchset(&patchset(run_b), &runs),
            Some(task_b),
        );
        assert_eq!(
            resolve_task_for_patchset(&patchset(run_a), &runs),
            Some(task_a),
        );
    }

    /// A patch set whose owning run is absent resolves to `None` (dangling) — the
    /// caller surfaces the inconsistency rather than mis-routing.
    #[test]
    fn resolve_task_returns_none_for_unknown_run() {
        let runs = vec![run(AgentRunId::new(), AgentTaskId::new())];
        assert_eq!(
            resolve_task_for_patchset(&patchset(AgentRunId::new()), &runs),
            None,
        );
        // Empty run set is also None, never a panic.
        assert_eq!(
            resolve_task_for_patchset(&patchset(AgentRunId::new()), &[]),
            None,
        );
    }
}
