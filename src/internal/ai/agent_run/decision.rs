//! `MergeCandidate[S]` and `MergeDecision[E]` — Layer 1 aggregate of one or
//! more `AgentPatchSet`s plus the human-gated decision applied to them.
//!
//! `MergeCandidate[S]` 和 `MergeDecision[E]` — 一个或多个 `AgentPatchSet` 加上应用于它们的人工门控决策的第 1 层聚合。
//!
//! # Schema-ownership boundaries
//!
//! Per CEX-S2-13 ownership rule (and audit-closure note in `mod.rs`), this
//! file freezes only the **field shape** of the merge decision payload:
//! `risk_score: Option<RiskScore>` / `conflict_list` / `test_evidence` /
//! `distillable_evidence_ids`, plus the `MergeCandidate.review_evidence`
//! field. CEX-S2-15 fills the values. CEX-S2-10 only declares the field
//! shape via `MergeDecisionPayloadV0`.

use serde::{Deserialize, Serialize};

use super::{AgentPatchSetId, AgentRunId, DecisionId, EvidenceId, MergeCandidateId};

/// Review state of a `MergeCandidate`. Per S2-INV-07 the default for **every**
/// candidate is `NeedsHumanReview`; auto-merge is a separate feature flag
/// owned by CEX-S2-15 and is off by default in Step 2.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
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
#[non_exhaustive]
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

#[cfg(test)]
mod tests {
    use super::*;

    /// **Safety invariant S2-INV-07**: every `MergeCandidate` built
    /// through the public constructor MUST start in
    /// `ReviewState::NeedsHumanReview`. Sub-agent patches can never
    /// reach the main worktree without a human (or, later, an
    /// explicitly-flag-gated CEX-S2-15 auto-merge path) signing off, so
    /// a refactor of `MergeCandidate::new` that accidentally defaulted
    /// to `Accepted` would be a silent security regression. Pin it.
    #[test]
    fn merge_candidate_new_defaults_to_needs_human_review() {
        let candidate = MergeCandidate::new(
            MergeCandidateId::new(),
            vec![AgentPatchSetId::new()],
            vec![AgentRunId::new(), AgentRunId::new()],
        );
        assert_eq!(
            candidate.review_state,
            ReviewState::NeedsHumanReview,
            "S2-INV-07: a freshly constructed MergeCandidate must require human review",
        );
        assert!(
            candidate.review_evidence.is_empty(),
            "review_evidence is the CEX-S2-13 empty placeholder; CEX-S2-15 fills it",
        );
    }

    /// The constructor threads the supplied ids through verbatim — the
    /// aggregate `MergeDecision` later references the candidate's
    /// `agent_run_ids`, so they must survive construction unchanged.
    #[test]
    fn merge_candidate_new_preserves_supplied_ids() {
        let id = MergeCandidateId::new();
        let patchsets = vec![AgentPatchSetId::new(), AgentPatchSetId::new()];
        let runs = vec![AgentRunId::new()];
        let candidate = MergeCandidate::new(id, patchsets.clone(), runs.clone());

        assert_eq!(candidate.id, id);
        assert_eq!(candidate.patchset_ids, patchsets);
        assert_eq!(candidate.agent_run_ids, runs);
    }

    /// `MergeDecisionPayloadV0::default()` (what CEX-S2-10 always
    /// writes) must be entirely empty / `None` — CEX-S2-15's
    /// ValidatorEngine is the only thing that may populate
    /// `risk_score` / `conflict_list` / `test_evidence` /
    /// `distillable_evidence_ids`. Pin the empty V0 shape so a stray
    /// default value can't leak a fabricated risk score into the
    /// decision record before the validator runs.
    #[test]
    fn merge_decision_payload_v0_default_is_empty() {
        let payload = MergeDecisionPayloadV0::default();
        assert!(payload.risk_score.is_none());
        assert!(payload.conflict_list.is_empty());
        assert!(payload.test_evidence.is_empty());
        assert!(payload.distillable_evidence_ids.is_empty());
    }

    /// `ReviewState` serializes to the stable snake_case wire tags that
    /// JSONL audit consumers and projection readers depend on. Pin the
    /// exact strings so a rename trips here rather than silently
    /// desyncing persisted decision records.
    #[test]
    fn review_state_serializes_to_stable_snake_case_tags() {
        for (state, tag) in [
            (ReviewState::NeedsHumanReview, "\"needs_human_review\""),
            (ReviewState::Accepted, "\"accepted\""),
            (ReviewState::Rejected, "\"rejected\""),
            (ReviewState::RequestChanges, "\"request_changes\""),
            (ReviewState::Conflict, "\"conflict\""),
        ] {
            let wire = serde_json::to_string(&state).expect("serialize ReviewState");
            assert_eq!(wire, tag, "unexpected wire tag for {state:?}");
            let back: ReviewState = serde_json::from_str(&wire).expect("deserialize ReviewState");
            assert_eq!(back, state, "ReviewState wire tag must round-trip");
        }
    }

    /// `RiskLevel` wire tags are likewise stable snake_case. Pin them
    /// so the CEX-S2-15 risk-score payload (which CEX-S2-13 only shapes)
    /// keeps a consistent serialized vocabulary. Asserts the round-trip
    /// (deserialize too) so a rename that desyncs the reader side is
    /// caught here, not just the serialize side (matches the
    /// `ReviewState` pin).
    #[test]
    fn risk_level_serializes_to_stable_snake_case_tags() {
        for (level, tag) in [
            (RiskLevel::Low, "\"low\""),
            (RiskLevel::Medium, "\"medium\""),
            (RiskLevel::High, "\"high\""),
            (RiskLevel::Critical, "\"critical\""),
        ] {
            let wire = serde_json::to_string(&level).expect("serialize RiskLevel");
            assert_eq!(wire, tag, "unexpected wire tag for {level:?}");
            let back: RiskLevel = serde_json::from_str(&wire).expect("deserialize RiskLevel");
            assert_eq!(back, level, "RiskLevel wire tag must round-trip");
        }
    }

    /// A populated `RiskScore` (non-empty `factors`) pins to an exact
    /// JSON wire shape. CEX-S2-15 fills the factors — so freeze the
    /// serialized vocabulary now against the literal payload, not just a
    /// serialize→deserialize echo (which a synchronized rename would
    /// slip through). `factors` is a `Vec<(String, String)>`, so each
    /// entry serializes as a two-element JSON array; `deny_unknown_fields`
    /// on the struct means a future field addition that desyncs readers
    /// of persisted decision records trips the deserialize from-literal.
    #[test]
    fn risk_score_round_trips_with_factors() {
        let score = RiskScore {
            level: RiskLevel::High,
            factors: vec![
                ("budget_token_exceeded".to_string(), "3".to_string()),
                ("conflict_count".to_string(), "2".to_string()),
            ],
        };
        // The frozen wire shape: snake_case level tag + factors as an
        // array of [key, value] pairs.
        const WIRE: &str =
            r#"{"level":"high","factors":[["budget_token_exceeded","3"],["conflict_count","2"]]}"#;

        let serialized = serde_json::to_string(&score).expect("serialize RiskScore");
        assert_eq!(serialized, WIRE, "RiskScore wire shape drifted");

        let back: RiskScore =
            serde_json::from_str(WIRE).expect("deserialize RiskScore from literal");
        assert_eq!(
            back, score,
            "RiskScore must round-trip from the frozen wire shape"
        );
    }
}
