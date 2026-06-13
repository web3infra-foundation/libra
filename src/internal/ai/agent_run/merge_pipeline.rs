//! Merge-decision payload assembly for sub-agent patch sets (CEX-S2-15).
//!
//! [`detect_conflicts`](super::conflict::detect_conflicts) (CEX-S2-13) and
//! [`compute_merge_risk_score`](super::risk_score::compute_merge_risk_score)
//! (CEX-S2-15) are independent pure building blocks; [`decision`](super::decision)
//! freezes the [`MergeDecisionPayloadV0`] *shape* (CEX-S2-13 ownership). This
//! module composes them into a single deterministic step that the CEX-S2-15
//! ValidatorEngine uses to **fill** the payload (`None` / empty → populated)
//! before Layer 1 writes the `MergeDecision`.
//!
//! The key invariant this composition enforces is that
//! [`MergeRiskInputs::conflict_count`](super::risk_score::MergeRiskInputs) is
//! **derived from the detected conflict list**, never supplied independently —
//! so the risk score can never disagree with `conflict_list` (e.g. claim zero
//! conflicts while `conflict_list` is non-empty, which would understate merge
//! risk to a human reviewer).
//!
//! Like its constituents this performs no I/O and is a pure function of its
//! inputs. See `docs/development/commands/agent.md` Step 2.5 (CEX-S2-15).

use super::{
    EvidenceId,
    conflict::{PatchTouch, detect_conflicts},
    decision::{Conflict, MergeDecisionPayloadV0},
    risk_score::{BudgetExceededCounts, MergeRiskInputs, compute_merge_risk_score},
};

/// The signals required to assemble a [`MergeDecisionPayloadV0`] that are **not**
/// derivable from the conflict list. `conflict_count` is intentionally absent:
/// it is always derived from the detected conflicts inside
/// [`build_merge_decision_payload`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MergeDecisionSignals {
    /// Number of sub-agents whose patches the candidate aggregates.
    pub sub_agent_count: u32,
    /// Sub-agent runs that ended in a non-success terminal state.
    pub failed_run_count: u32,
    /// Patches whose scope the validator could not verify.
    pub unverified_patch_scope: u32,
    /// Per-dimension `budget_exceeded` hit counts across the runs.
    pub budget_exceeded: BudgetExceededCounts,
    /// Validator test-evidence ids gathered for this candidate.
    pub test_evidence: Vec<EvidenceId>,
    /// Evidence ids the sub-agent flagged `distillable = true` (collected via
    /// [`collect_distillable_evidence_ids`](super::validator::collect_distillable_evidence_ids)).
    pub distillable_evidence_ids: Vec<EvidenceId>,
}

/// Assemble a [`MergeDecisionPayloadV0`] from an already-detected conflict list
/// and the remaining merge signals.
///
/// Derives `conflict_count` from `conflicts.len()` (saturating into `u32`),
/// computes the [`RiskScore`](super::decision::RiskScore) via
/// [`compute_merge_risk_score`], and threads the conflict list and evidence ids
/// into the payload. The candidate's review state remains the caller's concern
/// (it defaults to `NeedsHumanReview` per S2-INV-07); this function only fills
/// the decision payload and performs no I/O.
pub fn build_merge_decision_payload(
    conflicts: Vec<Conflict>,
    signals: MergeDecisionSignals,
) -> MergeDecisionPayloadV0 {
    let risk_inputs = MergeRiskInputs {
        sub_agent_count: signals.sub_agent_count,
        // Derived, never caller-supplied: keeps the risk score consistent with
        // the conflict list. Saturate rather than truncate on the (pathological)
        // overflow of usize → u32.
        conflict_count: u32::try_from(conflicts.len()).unwrap_or(u32::MAX),
        failed_run_count: signals.failed_run_count,
        unverified_patch_scope: signals.unverified_patch_scope,
        budget_exceeded: signals.budget_exceeded,
    };
    let risk = compute_merge_risk_score(&risk_inputs);

    MergeDecisionPayloadV0 {
        risk_score: Some(risk),
        conflict_list: conflicts,
        test_evidence: signals.test_evidence,
        distillable_evidence_ids: signals.distillable_evidence_ids,
    }
}

/// Detect conflicts among `patches` and assemble the merge-decision payload in
/// one step.
///
/// Thin composition over [`detect_conflicts`] + [`build_merge_decision_payload`]
/// so the ValidatorEngine never has to thread the conflict list by hand (which
/// is exactly how the derived-`conflict_count` invariant gets bypassed).
pub fn build_payload_from_patches(
    patches: &[PatchTouch],
    signals: MergeDecisionSignals,
) -> MergeDecisionPayloadV0 {
    build_merge_decision_payload(detect_conflicts(patches), signals)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::agent_run::decision::RiskLevel;

    fn conflict(path: &str) -> Conflict {
        Conflict {
            kind: "both_modified".to_string(),
            path: path.to_string(),
            detail: None,
        }
    }

    #[test]
    fn no_conflicts_benign_signals_is_low_risk() {
        let payload = build_merge_decision_payload(
            Vec::new(),
            MergeDecisionSignals {
                sub_agent_count: 1,
                ..Default::default()
            },
        );
        assert!(payload.conflict_list.is_empty());
        let risk = payload.risk_score.expect("risk score is always filled");
        assert_eq!(risk.level, RiskLevel::Low);
        assert!(risk.factors.is_empty());
    }

    #[test]
    fn conflict_count_is_derived_from_the_list_not_signals() {
        // Two conflicts, and `MergeDecisionSignals` carries no conflict field:
        // the risk score must still reflect both, proving derivation.
        let payload = build_merge_decision_payload(
            vec![conflict("a.rs"), conflict("b.rs")],
            MergeDecisionSignals {
                sub_agent_count: 1,
                ..Default::default()
            },
        );
        assert_eq!(payload.conflict_list.len(), 2);
        let risk = payload.risk_score.expect("risk score is always filled");
        // 2 conflicts * weight 3 = 6 >= HIGH threshold (4).
        assert_eq!(risk.level, RiskLevel::High);
        assert!(
            risk.factors
                .iter()
                .any(|(k, v)| k == "conflict_count" && v == "2"),
            "derived conflict_count must surface as a risk factor: {:?}",
            risk.factors,
        );
    }

    #[test]
    fn single_conflict_escalates_to_medium() {
        let payload = build_merge_decision_payload(
            vec![conflict("a.rs")],
            MergeDecisionSignals {
                sub_agent_count: 1,
                ..Default::default()
            },
        );
        let risk = payload.risk_score.expect("risk score is always filled");
        // 1 conflict * weight 3 = 3 >= MEDIUM threshold (2), < HIGH (4).
        assert_eq!(risk.level, RiskLevel::Medium);
    }

    #[test]
    fn evidence_ids_are_threaded_through() {
        let test_ev = vec![EvidenceId::new(), EvidenceId::new()];
        let distill = vec![EvidenceId::new()];
        let payload = build_merge_decision_payload(
            Vec::new(),
            MergeDecisionSignals {
                sub_agent_count: 1,
                test_evidence: test_ev.clone(),
                distillable_evidence_ids: distill.clone(),
                ..Default::default()
            },
        );
        assert_eq!(payload.test_evidence, test_ev);
        assert_eq!(payload.distillable_evidence_ids, distill);
    }

    #[test]
    fn from_patches_with_no_patches_yields_no_conflicts() {
        // Composition over detect_conflicts: an empty patch set has no
        // conflicts, so the payload is clean and low-risk.
        let payload = build_payload_from_patches(
            &[],
            MergeDecisionSignals {
                sub_agent_count: 1,
                ..Default::default()
            },
        );
        assert!(payload.conflict_list.is_empty());
        assert_eq!(
            payload
                .risk_score
                .expect("risk score is always filled")
                .level,
            RiskLevel::Low,
        );
    }

    #[test]
    fn payload_round_trips_through_json() {
        let payload = build_merge_decision_payload(
            vec![conflict("a.rs")],
            MergeDecisionSignals {
                sub_agent_count: 2,
                failed_run_count: 1,
                ..Default::default()
            },
        );
        let json = serde_json::to_string(&payload).expect("serialize");
        let back: MergeDecisionPayloadV0 = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.conflict_list.len(), 1);
        assert!(back.risk_score.is_some());
    }
}
