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
//! This module owns the **pure, side-effect-free** stages: the distillable
//! evidence scan, failure → originating-task routing, and
//! [`validate_merge_candidate`] (the output stage that composes the risk score,
//! conflicts, test evidence and distillable ids into the filled
//! `MergeDecision`). The remaining orchestrator-wired piece — actually
//! *executing* the verification test DAG to produce `test_evidence` — touches
//! `orchestrator::verifier` and lands separately; this module takes that
//! evidence as an input so the field-filling stays deterministic and testable.

use super::{
    AgentEvidence, AgentPatchSet, AgentRun, AgentRunEvent, AgentTaskId, Conflict, EvidenceId,
    MergeCandidate, MergeDecision, MergeDecisionPayloadV0, compute_merge_risk_score,
    gather_merge_risk_inputs,
};

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

/// Count the patch sets whose write scope is **unverified** — the
/// `unverified_patch_scope` risk-score input [`validate_merge_candidate`]
/// otherwise takes as a caller-supplied number (CEX-S2-15 risk inputs).
///
/// A patch set is unverified exactly when [`AgentPatchSet::workspace_scope_constrained`]
/// is `true`: that flag marks a run that used a sparse / blocked workspace
/// materialization (CEX-S2-11), so its writes were **not** validated against the
/// real tree during execution and "merge review must double-check write scope".
/// A full-copy run (`false`) ran against the real tree, so its scope is already
/// verified and contributes nothing. Pure — a count over the supplied patch sets.
pub fn count_unverified_patch_scope(patchsets: &[AgentPatchSet]) -> u32 {
    patchsets
        .iter()
        .filter(|patchset| patchset.workspace_scope_constrained)
        .count() as u32
}

/// Assemble the validated [`MergeDecision`] for a reviewed candidate — the
/// **pure output stage** of the CEX-S2-15 ValidatorEngine (完成判定: "填充
/// CEX-S2-13 已声明的字段"). Composes the merge risk score (gathered from the
/// candidate's `runs` + `events`), the detected `conflicts`, the verification
/// `test_evidence` the engine produced, and the distillable-evidence scan over
/// `all_evidence` into [`MergeDecisionPayloadV0`], then builds the event via
/// [`MergeDecision::for_candidate`] — which derives the aggregate ids and
/// `resulting_state` from the candidate, so the engine **never decides the
/// verdict** (that stays with Layer 1 human review per S2-INV-07; the engine
/// only vets and fills fields).
///
/// Running the verification test DAG to produce `test_evidence`, and the
/// patch-scope check behind `unverified_patch_scope`, are the engine's I/O and
/// are the caller's job. This function is pure so the field-filling contract —
/// every CEX-S2-13 `None`/empty default becoming its computed value — is
/// deterministically unit-testable without the orchestrator.
#[allow(clippy::too_many_arguments)]
pub fn validate_merge_candidate(
    candidate: &MergeCandidate,
    runs: &[AgentRun],
    events: &[AgentRunEvent],
    conflicts: Vec<Conflict>,
    test_evidence: Vec<EvidenceId>,
    all_evidence: &[AgentEvidence],
    unverified_patch_scope: u32,
) -> MergeDecision {
    let risk_inputs =
        gather_merge_risk_inputs(runs, events, conflicts.len() as u32, unverified_patch_scope);
    let payload = MergeDecisionPayloadV0 {
        risk_score: Some(compute_merge_risk_score(&risk_inputs)),
        conflict_list: conflicts,
        test_evidence,
        distillable_evidence_ids: collect_distillable_evidence_ids(all_evidence),
    };
    MergeDecision::for_candidate(candidate, payload)
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

    /// CEX-S2-15 risk input: `count_unverified_patch_scope` counts exactly the
    /// patch sets that used a sparse/blocked workspace
    /// (`workspace_scope_constrained = true`, "merge review must double-check");
    /// full-copy runs (`false`) and an empty set contribute nothing.
    #[test]
    fn count_unverified_patch_scope_counts_only_constrained_patchsets() {
        // `patchset(..)` builds `workspace_scope_constrained = false` (verified);
        // override the flag for the constrained (unverified) ones.
        let constrained = |run: AgentRunId| AgentPatchSet {
            workspace_scope_constrained: true,
            ..patchset(run)
        };

        let sets = vec![
            constrained(AgentRunId::new()),
            patchset(AgentRunId::new()),
            constrained(AgentRunId::new()),
        ];
        assert_eq!(
            count_unverified_patch_scope(&sets),
            2,
            "only the two sparse/blocked patch sets are unverified",
        );

        // All full-copy → nothing unverified; empty → 0, never a panic.
        assert_eq!(
            count_unverified_patch_scope(&[patchset(AgentRunId::new())]),
            0,
        );
        assert_eq!(count_unverified_patch_scope(&[]), 0);
    }

    /// CEX-S2-15 完成判定: the ValidatorEngine's pure output stage fills every
    /// CEX-S2-13 `MergeDecision` field that defaults to `None`/empty — risk
    /// score (gathered from the candidate's runs + events), conflict list, test
    /// evidence, and distillable ids — while leaving the verdict
    /// (`resulting_state`) as the candidate's (the engine vets, never decides).
    #[test]
    fn validate_merge_candidate_fills_the_s2_13_declared_fields() {
        use super::super::BudgetDimension;

        let run_id = AgentRunId::new();
        let r = run(run_id, AgentTaskId::new()); // status Failed
        let candidate = MergeCandidate::from_patchsets(
            MergeCandidateId::new(),
            std::slice::from_ref(&patchset(run_id)),
        );

        let events = vec![AgentRunEvent::BudgetExceeded {
            agent_run_id: run_id,
            dimension: BudgetDimension::Token,
        }];
        let conflicts = vec![Conflict {
            kind: "overlapping_hunk".to_string(),
            path: "src/a.rs".to_string(),
            detail: None,
        }];
        let test_evidence = vec![EvidenceId::new(), EvidenceId::new()];
        let distillable = evidence(true);
        let distillable_id = distillable.id;
        let all_evidence = vec![distillable, evidence(false)];

        let decision = validate_merge_candidate(
            &candidate,
            std::slice::from_ref(&r),
            &events,
            conflicts,
            test_evidence.clone(),
            &all_evidence,
            1, // unverified_patch_scope (validator-determined)
        );

        // Aggregate ids + verdict mirror the candidate (engine does not decide).
        assert_eq!(decision.merge_candidate_id, candidate.id);
        assert_eq!(decision.resulting_state, candidate.review_state);
        // Every S2-13 None/empty default is now its computed value.
        assert!(
            decision.payload.risk_score.is_some(),
            "risk_score filled from a Failed run + Token breach + conflict + unverified scope",
        );
        assert_eq!(decision.payload.conflict_list.len(), 1);
        assert_eq!(decision.payload.test_evidence, test_evidence);
        assert_eq!(
            decision.payload.distillable_evidence_ids,
            vec![distillable_id]
        );
    }
}
