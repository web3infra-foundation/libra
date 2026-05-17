//! `MergeCandidate[S]` and `MergeDecision[E]` — Layer 1 aggregate of one or
//! more `AgentPatchSet`s plus the human-gated decision applied to them.
//!
//! # Schema-ownership boundaries
//!
//! Per CEX-S2-13 ownership rule (and audit-closure note in `mod.rs`), this
//! file freezes only the **field shape** of the merge decision payload:
//! `risk_score: Option<RiskScore>` / `conflict_list` / `test_evidence` /
//! `distillable_evidence_ids`, plus the `MergeCandidate.review_evidence`
//! field. CEX-S2-15 fills the values. CEX-S2-10 only declares the field
//! shape via `MergeDecisionPayloadV0`.

#![cfg(feature = "subagent-scaffold")]

use serde::{Deserialize, Serialize};

use super::{AgentPatchSetId, AgentRunId, DecisionId, EvidenceId, MergeCandidateId};

/// Review state of a `MergeCandidate`. Per S2-INV-07 the default for **every**
/// candidate is `NeedsHumanReview`; auto-merge is a separate feature flag
/// owned by CEX-S2-15 and is off by default in Step 2.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewState {
    NeedsHumanReview,
    Accepted,
    Rejected,
    RequestChanges,
    Conflict,
}

/// Risk level summary attached to a `MergeDecision`. CEX-S2-15 computes the
/// concrete value; CEX-S2-13 only declares the shape.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RiskScore {
    pub level: RiskLevel,
    /// Free-form factors enumerated by CEX-S2-15; key/value pairs like
    /// `("budget_token_exceeded", "3")` to keep the schema stable across
    /// future risk-input additions.
    #[serde(default)]
    pub factors: Vec<(String, String)>,
}

/// One conflict entry detected during merge candidate aggregation.
/// CEX-S2-15 fills out the conflict semantics; the schema is frozen here
/// so CEX-S2-10 callers can write `Vec<Conflict>` shaped fields.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Conflict {
    pub kind: String,
    pub path: String,
    /// Optional human-readable detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Aggregate of one or more `AgentPatchSet`s under review.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MergeCandidate {
    pub id: MergeCandidateId,

    /// Patches included in this candidate.
    pub patchset_ids: Vec<AgentPatchSetId>,

    /// Sub-agent runs that produced those patches. Aggregated event payloads
    /// (e.g. `MergeDecision`) reference all of these via `agent_run_ids`.
    pub agent_run_ids: Vec<AgentRunId>,

    /// Default state per S2-INV-07.
    pub review_state: ReviewState,

    /// Reviewer-sub-agent evidence ids that informed this candidate. Empty
    /// `Vec` is the CEX-S2-13 placeholder; CEX-S2-15 reviewer path fills it.
    #[serde(default)]
    pub review_evidence: Vec<EvidenceId>,
}

impl MergeCandidate {
    /// Convenience constructor that respects S2-INV-07 default.
    pub fn new(
        id: MergeCandidateId,
        patchset_ids: Vec<AgentPatchSetId>,
        agent_run_ids: Vec<AgentRunId>,
    ) -> Self {
        Self {
            id,
            patchset_ids,
            agent_run_ids,
            // INVARIANT: must default to NeedsHumanReview per S2-INV-07.
            // Auto-merge is a separate feature flag owned by CEX-S2-15.
            review_state: ReviewState::NeedsHumanReview,
            review_evidence: Vec::new(),
        }
    }
}

/// CEX-S2-13 stub payload. Fields exist with `None` / empty defaults; CEX-S2-15
/// fills them. Schema fields **must not** be renamed or retyped by any other
/// CEX (per audit closure).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MergeDecisionPayloadV0 {
    /// Risk score; `None` until CEX-S2-15 ValidatorEngine runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk_score: Option<RiskScore>,

    /// Conflicts detected; empty until CEX-S2-15 fills.
    #[serde(default)]
    pub conflict_list: Vec<Conflict>,

    /// Validator evidence ids; empty until CEX-S2-15 ValidatorEngine runs.
    #[serde(default)]
    pub test_evidence: Vec<EvidenceId>,

    /// Evidence ids the sub-agent flagged `distillable=true`; collected by
    /// CEX-S2-15 before writing the `MergeDecision`. Read API in CEX-S2-18.
    #[serde(default)]
    pub distillable_evidence_ids: Vec<EvidenceId>,
}

/// `MergeDecision[E]` — aggregate event over all `agent_run_ids` of a
/// `MergeCandidate`. Carries the decision verdict (encoded by `review_state`
/// in the candidate plus this event's payload).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MergeDecision {
    pub id: DecisionId,

    /// Aggregate id field per CEX-S2-10 (2): `merge_candidate_id +
    /// agent_run_ids` instead of single `agent_run_id`.
    pub merge_candidate_id: MergeCandidateId,
    pub agent_run_ids: Vec<AgentRunId>,

    /// Resulting state of the candidate after this decision (mirrors
    /// `MergeCandidate.review_state` post-decision).
    pub resulting_state: ReviewState,

    /// CEX-S2-15-filled payload. CEX-S2-10 always writes V0 default values.
    pub payload: MergeDecisionPayloadV0,
}
