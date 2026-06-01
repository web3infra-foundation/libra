//! Merge pipeline composition for sub-agent patch sets (CEX-S2-13 / CEX-S2-15).
//!
//! [`detect_conflicts`] (CEX-S2-13), [`RiskScoreSignals::score`] (CEX-S2-15) and
//! [`DecisionProposal`] (CEX-S2-13/15) are independent, pure building blocks.
//! This module composes them into a single deterministic entry point,
//! [`evaluate_merge`], so the human-gated merge step always follows one
//! consistent path regardless of caller.
//!
//! The key invariant this composition enforces is that the conflict-derived risk
//! signals (`has_conflicts` / `conflicted_files`) are **derived from the conflict
//! report**, never supplied independently. This eliminates the class of bugs
//! where a caller's hand-built [`RiskScoreSignals`] disagrees with the actual
//! [`ConflictReport`] (e.g. reporting `has_conflicts = false` while conflicts
//! exist, which would let a conflicting patch slip past the gate).
//!
//! Like its constituents, `evaluate_merge` performs no I/O and always yields the
//! same output for the same inputs. See `docs/improvement/agent.md` Step 2.4/2.5.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::decision::{DecisionProposal, MergeRecommendation};
use super::merge_conflict::{ChangeSet, ConflictReport, detect_conflicts};
use super::risk_score::{RiskScore, RiskScoreSignals};

/// The caller-supplied signals that are **not** derivable from conflict
/// detection. Conflict-related signals (`has_conflicts` / `conflicted_files`)
/// are computed inside [`evaluate_merge`] from the [`ConflictReport`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MergeEvaluationInput {
    /// Number of files touched by the sub-agent patch set.
    pub files_changed: usize,
    /// Total added + removed line count across the patch set.
    pub lines_changed: usize,
    /// Whether validation (tests / clippy / fmt) passed for the run.
    pub validation_passed: bool,
    /// Whether the run touched protected paths (e.g. CI config, secrets).
    pub touched_protected_paths: bool,
}

/// The composed outcome of the merge pipeline for a single sub-agent run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MergeEvaluation {
    /// The conflict-detection result against the main worktree.
    pub conflicts: ConflictReport,
    /// The computed risk score.
    pub risk: RiskScore,
    /// The pending decision proposal awaiting the human gate.
    pub proposal: DecisionProposal,
}

/// Run the full merge evaluation pipeline for one sub-agent run.
///
/// Composes conflict detection, risk scoring, recommendation derivation and
/// proposal construction into a single deterministic step. The returned
/// [`DecisionProposal`] is always in the pending human-gate state — no merge is
/// performed and no I/O occurs.
///
/// # Arguments
/// * `run_id` - identifier of the sub-agent run.
/// * `sub_agent` - the sub-agent's change set.
/// * `main` - the current main-worktree change set to merge against.
/// * `input` - caller-supplied non-conflict risk signals.
///
/// # Returns
/// A [`MergeEvaluation`] bundling the conflict report, risk score and a pending
/// [`DecisionProposal`], all consistent with one another.
pub fn evaluate_merge(
    run_id: impl Into<String>,
    sub_agent: &ChangeSet,
    main: &ChangeSet,
    input: &MergeEvaluationInput,
) -> MergeEvaluation {
    let conflicts = detect_conflicts(sub_agent, main);

    // Conflict-derived signals come from the report, never the caller. Count
    // distinct conflicting paths so the signal stays correct even if a future
    // detector reports more than one conflict kind per path.
    let conflicted_files = conflicts
        .conflicts
        .iter()
        .map(|c| c.path.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let has_conflicts = conflicted_files > 0;

    let signals = RiskScoreSignals {
        files_changed: input.files_changed,
        lines_changed: input.lines_changed,
        has_conflicts,
        conflicted_files,
        validation_passed: input.validation_passed,
        touched_protected_paths: input.touched_protected_paths,
    };
    let risk = signals.score();
    let recommendation = MergeRecommendation::from_risk(&risk, has_conflicts);
    let proposal = DecisionProposal::new(run_id, recommendation, risk.clone());

    MergeEvaluation {
        conflicts,
        risk,
        proposal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::agent_run::decision::HumanGateState;
    use crate::internal::ai::agent_run::merge_conflict::{ChangeKind, ConflictKind, FileChange};
    use crate::internal::ai::agent_run::risk_score::RiskTier;

    fn change(path: &str, kind: ChangeKind, hash: Option<&str>) -> FileChange {
        FileChange {
            path: path.to_string(),
            kind,
            content_hash: hash.map(|h| h.to_string()),
        }
    }

    fn clean_input() -> MergeEvaluationInput {
        MergeEvaluationInput {
            files_changed: 1,
            lines_changed: 10,
            validation_passed: true,
            touched_protected_paths: false,
        }
    }

    #[test]
    fn clean_low_risk_run_recommends_merge() {
        let sub = ChangeSet {
            changes: vec![change("src/a.rs", ChangeKind::Modified, Some("h1"))],
        };
        let main = ChangeSet::default();

        let eval = evaluate_merge("run-1", &sub, &main, &clean_input());

        assert!(eval.conflicts.is_clean());
        assert_eq!(eval.risk.tier, RiskTier::Low);
        assert_eq!(
            eval.proposal.recommendation,
            MergeRecommendation::Merge,
            "low-risk clean run should recommend merge"
        );
        assert_eq!(eval.proposal.gate, HumanGateState::Pending);
        assert_eq!(eval.proposal.run_id, "run-1");
        // The proposal's risk must equal the evaluation's risk (no drift).
        assert_eq!(eval.proposal.risk, eval.risk);
    }

    #[test]
    fn conflicts_force_hold_regardless_of_low_change_size() {
        // Both sides modify the same file -> BothModified conflict.
        let sub = ChangeSet {
            changes: vec![change("src/a.rs", ChangeKind::Modified, Some("sub"))],
        };
        let main = ChangeSet {
            changes: vec![change("src/a.rs", ChangeKind::Modified, Some("main"))],
        };

        let eval = evaluate_merge("run-2", &sub, &main, &clean_input());

        assert!(!eval.conflicts.is_clean());
        assert_eq!(eval.conflicts.conflicts.len(), 1);
        assert_eq!(eval.conflicts.conflicts[0].kind, ConflictKind::BothModified);
        // Conflict signals are derived from the report even though the input
        // carried no conflict fields.
        assert!(eval.risk.total >= 25, "conflict must add risk points");
        assert_eq!(
            eval.proposal.recommendation,
            MergeRecommendation::Hold,
            "any conflict must force Hold"
        );
    }

    #[test]
    fn conflict_signals_are_derived_not_caller_supplied() {
        // Two distinct conflicting paths -> conflicted_files == 2.
        let sub = ChangeSet {
            changes: vec![
                change("a.rs", ChangeKind::Modified, Some("s1")),
                change("b.rs", ChangeKind::Modified, Some("s2")),
                change("c.rs", ChangeKind::Modified, Some("s3")),
            ],
        };
        let main = ChangeSet {
            changes: vec![
                change("a.rs", ChangeKind::Modified, Some("m1")),
                change("b.rs", ChangeKind::Modified, Some("m2")),
                // c.rs only on the sub side -> no conflict.
            ],
        };

        let eval = evaluate_merge("run-3", &sub, &main, &clean_input());

        assert_eq!(eval.conflicts.conflicts.len(), 2);
        // 3 files changed (input), but 2 conflicting -> risk reflects both:
        // base 25 + 2*3 conflict points present.
        assert!(eval.risk.total >= 25 + 6);
        assert_eq!(eval.proposal.recommendation, MergeRecommendation::Hold);
    }

    #[test]
    fn validation_failure_without_conflict_requires_review() {
        let sub = ChangeSet {
            changes: vec![change("src/a.rs", ChangeKind::Modified, Some("h1"))],
        };
        let main = ChangeSet::default();
        let input = MergeEvaluationInput {
            files_changed: 2,
            lines_changed: 40,
            validation_passed: false,
            touched_protected_paths: false,
        };

        let eval = evaluate_merge("run-4", &sub, &main, &input);

        assert!(eval.conflicts.is_clean());
        // 2*2 (files) + 40/20 (lines) + 25 (validation fail) = 31 -> Medium.
        assert_eq!(eval.risk.tier, RiskTier::Medium);
        assert_eq!(
            eval.proposal.recommendation,
            MergeRecommendation::ReviewRequired
        );
    }

    #[test]
    fn add_add_same_content_is_not_a_conflict() {
        let sub = ChangeSet {
            changes: vec![change("new.rs", ChangeKind::Added, Some("same"))],
        };
        let main = ChangeSet {
            changes: vec![change("new.rs", ChangeKind::Added, Some("same"))],
        };

        let eval = evaluate_merge("run-5", &sub, &main, &clean_input());

        assert!(eval.conflicts.is_clean());
        assert_eq!(eval.proposal.recommendation, MergeRecommendation::Merge);
    }

    #[test]
    fn evaluation_round_trips_through_json() {
        let sub = ChangeSet {
            changes: vec![change("src/a.rs", ChangeKind::Modified, Some("h1"))],
        };
        let main = ChangeSet::default();
        let eval = evaluate_merge("run-6", &sub, &main, &clean_input());

        let json = serde_json::to_string(&eval).expect("serialize");
        let back: MergeEvaluation = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(eval, back);
    }

    #[test]
    fn input_rejects_unknown_fields() {
        let json = r#"{
            "files_changed": 1,
            "lines_changed": 10,
            "validation_passed": true,
            "touched_protected_paths": false,
            "surprise": 1
        }"#;
        let parsed: Result<MergeEvaluationInput, _> = serde_json::from_str(json);
        assert!(parsed.is_err(), "deny_unknown_fields must reject extras");
    }
}
