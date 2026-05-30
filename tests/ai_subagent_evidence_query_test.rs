//! CEX-S2-18 (Step 2.8) — read-only Evidence → Memory Distillation query API.
//!
//! Pins the three read-only accessors (`evidence_query_by_scope`,
//! `evidence_stream`, `merge_decision_distillable_evidence`) over the
//! frozen `AgentEvidence` / `MergeDecision` schema. These are pure,
//! side-effect-free reads (CEX-S2-18 (5): no distillation pipeline, no
//! write path). Querying an empty collection returns an empty result
//! and never panics — the function-level analogue of CEX-S2-18 (3)'s
//! flag-off guarantee (under `code.sub_agents.enabled = false` no
//! `AgentEvidence` is ever persisted, so every query is empty).

use futures::StreamExt;
use libra::internal::ai::agent_run::{
    AgentEvidence, AgentRunId, AgentType, AnchorScope, Confidence, DecisionId, EventId,
    EvidenceFilter, EvidenceId, MergeCandidateId, MergeDecision, MergeDecisionPayloadV0,
    ReviewState, evidence_query_by_scope, evidence_stream, merge_decision_distillable_evidence,
};

/// Build an `AgentEvidence` with the query-relevant fields set and the
/// rest filled with fresh defaults.
fn evidence(scope: AnchorScope, source_agent_type: AgentType, distillable: bool) -> AgentEvidence {
    AgentEvidence {
        id: EvidenceId::new(),
        agent_run_id: AgentRunId::new(),
        source_agent_type,
        source_event_id: EventId::new(),
        tool_call_id: None,
        source_call_id: None,
        confidence: Confidence::new(0.9),
        applies_to_scope: scope,
        distillable,
        evidence_snapshot_id: uuid::Uuid::new_v4(),
    }
}

fn merge_decision_with(distillable_evidence_ids: Vec<EvidenceId>) -> MergeDecision {
    MergeDecision {
        id: DecisionId::new(),
        merge_candidate_id: MergeCandidateId::new(),
        agent_run_ids: vec![AgentRunId::new()],
        resulting_state: ReviewState::NeedsHumanReview,
        payload: MergeDecisionPayloadV0 {
            distillable_evidence_ids,
            ..MergeDecisionPayloadV0::default()
        },
    }
}

#[test]
fn evidence_query_by_scope_returns_only_matching_scope_in_order() {
    let corpus = vec![
        evidence(AnchorScope::Project, AgentType::Explorer, false),
        evidence(AnchorScope::AgentRun, AgentType::Worker, true),
        evidence(AnchorScope::Project, AgentType::Reviewer, false),
        evidence(AnchorScope::Session, AgentType::Worker, false),
    ];

    let project = evidence_query_by_scope(&corpus, AnchorScope::Project);
    assert_eq!(project.len(), 2, "exactly the two Project-scoped records");
    assert_eq!(project[0].id, corpus[0].id, "input order is preserved");
    assert_eq!(project[1].id, corpus[2].id);
    assert!(
        project
            .iter()
            .all(|e| e.applies_to_scope == AnchorScope::Project)
    );

    let agent_run = evidence_query_by_scope(&corpus, AnchorScope::AgentRun);
    assert_eq!(agent_run.len(), 1);
    assert_eq!(agent_run[0].id, corpus[1].id);
}

#[test]
fn evidence_query_by_scope_on_empty_input_returns_empty_without_panicking() {
    // Flag-off analogue (CEX-S2-18 (3)): no persisted evidence → empty.
    let empty: Vec<AgentEvidence> = Vec::new();
    assert!(evidence_query_by_scope(&empty, AnchorScope::Session).is_empty());

    // Non-empty corpus but no record in the queried scope → empty, no panic.
    let corpus = vec![evidence(AnchorScope::Project, AgentType::Worker, true)];
    assert!(evidence_query_by_scope(&corpus, AnchorScope::Session).is_empty());
}

#[tokio::test]
async fn evidence_stream_applies_and_combined_filter() {
    let corpus = vec![
        evidence(AnchorScope::Project, AgentType::Explorer, true),
        evidence(AnchorScope::Project, AgentType::Worker, true),
        evidence(AnchorScope::Project, AgentType::Worker, false),
        evidence(AnchorScope::Session, AgentType::Worker, true),
    ];

    // Default filter matches everything.
    let all: Vec<AgentEvidence> = evidence_stream(&corpus, &EvidenceFilter::default())
        .collect()
        .await;
    assert_eq!(all.len(), 4);

    // scope AND source_agent_type AND distillable_only (all must hold).
    let filter = EvidenceFilter {
        scope: Some(AnchorScope::Project),
        source_agent_type: Some(AgentType::Worker),
        distillable_only: true,
    };
    let matched: Vec<AgentEvidence> = evidence_stream(&corpus, &filter).collect().await;
    assert_eq!(
        matched.len(),
        1,
        "only the Project + Worker + distillable record survives the AND",
    );
    assert_eq!(matched[0].id, corpus[1].id);

    // distillable_only alone keeps the three distillable records.
    let distillable = EvidenceFilter {
        distillable_only: true,
        ..EvidenceFilter::default()
    };
    let distillable: Vec<AgentEvidence> = evidence_stream(&corpus, &distillable).collect().await;
    assert_eq!(distillable.len(), 3);
    assert!(distillable.iter().all(|e| e.distillable));
}

#[tokio::test]
async fn evidence_stream_on_empty_input_yields_nothing() {
    let empty: Vec<AgentEvidence> = Vec::new();
    let collected: Vec<AgentEvidence> = evidence_stream(&empty, &EvidenceFilter::default())
        .collect()
        .await;
    assert!(collected.is_empty());
}

#[test]
fn merge_decision_distillable_evidence_reads_the_recorded_ids() {
    let ids = vec![EvidenceId::new(), EvidenceId::new()];
    let decision = merge_decision_with(ids.clone());
    assert_eq!(merge_decision_distillable_evidence(&decision), ids);

    // CEX-S2-13/CEX-S2-10 write V0 defaults (empty) until CEX-S2-15 fills
    // them; the read API must surface that as an empty list, not panic.
    let empty = merge_decision_with(Vec::new());
    assert!(merge_decision_distillable_evidence(&empty).is_empty());
}
