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

/// Why a [`MergeCandidate::try_accept`] was rejected. Surfaced so Layer 1 / the
/// reviewer UI explains exactly why a candidate could not be accepted.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AcceptError {
    /// The candidate is not awaiting review — it was already accepted /
    /// rejected / marked conflicting, so accepting it now would be a
    /// double-transition.
    #[error("merge candidate is not awaiting review (current state: {current:?})")]
    NotPending {
        /// The candidate's current review state.
        current: ReviewState,
    },
    /// The candidate's decision payload carries unresolved conflicts; per
    /// S2-INV-07 a conflicted candidate must never be accepted / auto-applied.
    #[error("merge candidate has {conflict_count} unresolved conflict(s) and cannot be accepted")]
    HasConflicts {
        /// Number of conflicts blocking acceptance.
        conflict_count: usize,
    },
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

    /// Attempt to move the candidate to [`ReviewState::Accepted`] after a human
    /// (or, later, a flag-gated auto-merge) signs off.
    ///
    /// Enforces the S2-INV-07 safety invariant at the type boundary so no caller
    /// can apply a sub-agent patch set to the main worktree while it is
    /// conflicted or not yet under review:
    ///
    /// - the candidate must currently be in [`ReviewState::NeedsHumanReview`]
    ///   (otherwise [`AcceptError::NotPending`]);
    /// - the decision `payload.conflict_list` must be empty (otherwise
    ///   [`AcceptError::HasConflicts`]) — a conflicted candidate is never
    ///   auto-applied (agent.md Step 2.5 验收 (2)).
    ///
    /// On success the `review_state` becomes `Accepted`. On error the state is
    /// left unchanged. Pure state transition — no I/O, no patch application
    /// (applying the accepted patch to the worktree is a separate runtime step).
    pub fn try_accept(&mut self, payload: &MergeDecisionPayloadV0) -> Result<(), AcceptError> {
        if self.review_state != ReviewState::NeedsHumanReview {
            return Err(AcceptError::NotPending {
                current: self.review_state,
            });
        }
        if !payload.conflict_list.is_empty() {
            return Err(AcceptError::HasConflicts {
                conflict_count: payload.conflict_list.len(),
            });
        }
        self.review_state = ReviewState::Accepted;
        Ok(())
    }

    /// Reject a candidate awaiting review, moving it to
    /// [`ReviewState::Rejected`]. Unlike [`try_accept`](Self::try_accept) a
    /// reject is always safe regardless of conflicts — the patch is discarded,
    /// not applied — but the candidate must still be pending so a decided
    /// candidate is not silently re-decided ([`AcceptError::NotPending`]
    /// otherwise). Pure; leaves the state unchanged on error.
    pub fn reject(&mut self) -> Result<(), AcceptError> {
        self.transition_from_pending(ReviewState::Rejected)
    }

    /// Mark a candidate as [`ReviewState::Conflict`] when conflict detection
    /// found unresolved conflicts. A subsequent [`try_accept`](Self::try_accept)
    /// then fails with [`AcceptError::NotPending`], so a conflicted candidate
    /// can never be accepted without first being re-reviewed. Must be pending
    /// to mark; pure; leaves the state unchanged on error.
    pub fn mark_conflict(&mut self) -> Result<(), AcceptError> {
        self.transition_from_pending(ReviewState::Conflict)
    }

    /// Move from [`ReviewState::NeedsHumanReview`] to `next`, or return
    /// [`AcceptError::NotPending`] with the current state. Shared by
    /// [`reject`](Self::reject) and [`mark_conflict`](Self::mark_conflict).
    fn transition_from_pending(&mut self, next: ReviewState) -> Result<(), AcceptError> {
        if self.review_state != ReviewState::NeedsHumanReview {
            return Err(AcceptError::NotPending {
                current: self.review_state,
            });
        }
        self.review_state = next;
        Ok(())
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

impl MergeDecision {
    /// Build the `Decision[E]` event recording the verdict applied to a
    /// reviewed `candidate` (agent.md Step 2.5 验收 (4): "final decision 写入
    /// `Decision[E]`").
    ///
    /// Derives the aggregate id fields — `merge_candidate_id` and
    /// `agent_run_ids` — and `resulting_state` directly from `candidate` so the
    /// event can never desync from the candidate it decides (the exact failure
    /// the CEX-S2-10 (2) "`merge_candidate_id + agent_run_ids`" aggregate-id rule
    /// exists to prevent). A fresh [`DecisionId`] is allocated for the event.
    /// Pure — no I/O; persisting the event is the caller's job.
    pub fn for_candidate(candidate: &MergeCandidate, payload: MergeDecisionPayloadV0) -> Self {
        Self {
            id: DecisionId::new(),
            merge_candidate_id: candidate.id,
            agent_run_ids: candidate.agent_run_ids.clone(),
            resulting_state: candidate.review_state,
            payload,
        }
    }
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

    fn conflict() -> Conflict {
        Conflict {
            kind: "both_modified".to_string(),
            path: "src/a.rs".to_string(),
            detail: None,
        }
    }

    /// **S2-INV-07**: a clean candidate awaiting review can be accepted, and the
    /// state transitions to `Accepted`.
    #[test]
    fn try_accept_succeeds_for_clean_pending_candidate() {
        let mut candidate = MergeCandidate::new(MergeCandidateId::new(), vec![], vec![]);
        let payload = MergeDecisionPayloadV0::default();
        assert_eq!(candidate.try_accept(&payload), Ok(()));
        assert_eq!(candidate.review_state, ReviewState::Accepted);
    }

    /// **S2-INV-07 safety**: a candidate whose payload carries conflicts MUST
    /// NOT be acceptable; the state is left unchanged so a conflicted patch can
    /// never reach the main worktree.
    #[test]
    fn try_accept_rejects_conflicted_candidate() {
        let mut candidate = MergeCandidate::new(MergeCandidateId::new(), vec![], vec![]);
        let payload = MergeDecisionPayloadV0 {
            conflict_list: vec![conflict(), conflict()],
            ..MergeDecisionPayloadV0::default()
        };
        assert_eq!(
            candidate.try_accept(&payload),
            Err(AcceptError::HasConflicts { conflict_count: 2 }),
        );
        assert_eq!(
            candidate.review_state,
            ReviewState::NeedsHumanReview,
            "a rejected accept must leave the candidate awaiting review",
        );
    }

    /// Accepting a candidate that is not pending (already accepted) is a
    /// double-transition and is rejected with the current state.
    #[test]
    fn try_accept_rejects_non_pending_candidate() {
        let mut candidate = MergeCandidate::new(MergeCandidateId::new(), vec![], vec![]);
        let payload = MergeDecisionPayloadV0::default();
        candidate
            .try_accept(&payload)
            .expect("first accept succeeds");
        assert_eq!(
            candidate.try_accept(&payload),
            Err(AcceptError::NotPending {
                current: ReviewState::Accepted,
            }),
        );
    }

    /// `for_candidate` derives the aggregate id fields and resulting state from
    /// the candidate verbatim — the event can never desync from the candidate
    /// it decides (CEX-S2-10 (2) aggregate-id rule).
    #[test]
    fn for_candidate_derives_aggregate_ids_and_state() {
        let mut candidate = MergeCandidate::new(
            MergeCandidateId::new(),
            vec![AgentPatchSetId::new()],
            vec![AgentRunId::new(), AgentRunId::new()],
        );
        candidate
            .try_accept(&MergeDecisionPayloadV0::default())
            .expect("accept");

        let payload = MergeDecisionPayloadV0 {
            test_evidence: vec![EvidenceId::new()],
            ..MergeDecisionPayloadV0::default()
        };
        let decision = MergeDecision::for_candidate(&candidate, payload.clone());

        assert_eq!(decision.merge_candidate_id, candidate.id);
        assert_eq!(decision.agent_run_ids, candidate.agent_run_ids);
        assert_eq!(decision.resulting_state, ReviewState::Accepted);
        assert_eq!(
            decision.payload.test_evidence, payload.test_evidence,
            "the supplied payload is threaded through unchanged",
        );
    }

    /// Two decisions built for the same candidate get distinct `DecisionId`s —
    /// the event id is freshly allocated, not derived from the candidate.
    #[test]
    fn for_candidate_allocates_a_fresh_decision_id() {
        let candidate = MergeCandidate::new(MergeCandidateId::new(), vec![], vec![]);
        let a = MergeDecision::for_candidate(&candidate, MergeDecisionPayloadV0::default());
        let b = MergeDecision::for_candidate(&candidate, MergeDecisionPayloadV0::default());
        assert_ne!(a.id, b.id, "each decision event needs its own id");
    }

    /// `reject` moves a pending candidate to `Rejected` — even one carrying
    /// conflicts, since a reject discards the patch rather than applying it.
    #[test]
    fn reject_moves_pending_candidate_to_rejected() {
        let mut candidate = MergeCandidate::new(MergeCandidateId::new(), vec![], vec![]);
        assert_eq!(candidate.reject(), Ok(()));
        assert_eq!(candidate.review_state, ReviewState::Rejected);
    }

    /// `mark_conflict` moves a pending candidate to `Conflict`, after which
    /// `try_accept` fails with `NotPending` — a conflicted candidate can never
    /// be accepted without re-review. This closes the conflict→accept loop.
    #[test]
    fn mark_conflict_then_accept_is_blocked() {
        let mut candidate = MergeCandidate::new(MergeCandidateId::new(), vec![], vec![]);
        assert_eq!(candidate.mark_conflict(), Ok(()));
        assert_eq!(candidate.review_state, ReviewState::Conflict);
        assert_eq!(
            candidate.try_accept(&MergeDecisionPayloadV0::default()),
            Err(AcceptError::NotPending {
                current: ReviewState::Conflict,
            }),
            "a candidate marked Conflict must not be acceptable",
        );
    }

    /// `reject` / `mark_conflict` are double-transition-safe: a decided
    /// candidate cannot be re-decided.
    #[test]
    fn transitions_reject_non_pending_candidate() {
        let mut rejected = MergeCandidate::new(MergeCandidateId::new(), vec![], vec![]);
        rejected.reject().expect("first reject succeeds");
        assert_eq!(
            rejected.reject(),
            Err(AcceptError::NotPending {
                current: ReviewState::Rejected,
            }),
        );
        assert_eq!(
            rejected.mark_conflict(),
            Err(AcceptError::NotPending {
                current: ReviewState::Rejected,
            }),
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
