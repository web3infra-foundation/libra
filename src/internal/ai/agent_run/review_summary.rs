//! Human-review summary rendering for a merge candidate (CEX-S2-15, Step 2.5).
//!
//! Before a `MergeCandidate` reaches the human gate, Layer 1 presents the
//! reviewer with a "change summary、risk summary、test evidence、conflict
//! summary" (agent.md Step 2.5: "主 Agent 生成 change summary、risk summary、
//! test evidence、conflict summary"). This module is the **pure** renderer that
//! turns the already-frozen [`MergeDecisionPayloadV0`] plus the candidate's
//! patch/run counts into that reviewer-facing text block.
//!
//! It performs no I/O, computes no risk (CEX-S2-15's `compute_merge_risk_score`
//! owns that — this only *reads* the populated payload), and never mutates the
//! CEX-S2-13-frozen schema. Rendering the summary into a TUI / MCP view is a
//! separate concern; this function only produces the text.

use super::decision::{Conflict, MergeCandidate, MergeDecisionPayloadV0, RiskLevel, RiskScore};

/// The non-payload counts the reviewer needs alongside the decision payload.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MergeReviewCounts {
    /// Number of `AgentPatchSet`s aggregated into the candidate.
    pub patchset_count: usize,
    /// Number of sub-agent runs that produced them.
    pub agent_run_count: usize,
}

impl MergeReviewCounts {
    /// Derive the counts directly from a [`MergeCandidate`] so the rendered
    /// summary's "N patch set(s) from M sub-agent run(s)" line can never desync
    /// from the candidate it describes (the same hand-derive hazard
    /// [`MergeDecision::for_candidate`](super::decision::MergeDecision::for_candidate)
    /// removes for the decision event).
    pub fn from_candidate(candidate: &MergeCandidate) -> Self {
        Self {
            patchset_count: candidate.patchset_ids.len(),
            agent_run_count: candidate.agent_run_ids.len(),
        }
    }
}

/// Render the reviewer-facing summary for a merge candidate.
///
/// Produces a deterministic multi-line block with four labelled sections —
/// Changes, Risk, Conflicts, Test evidence — in that fixed order. A clean,
/// validated candidate renders an explicit "no conflicts" / risk-low summary so
/// the reviewer never has to infer absence from a missing line.
pub fn render_merge_review_summary(
    payload: &MergeDecisionPayloadV0,
    counts: &MergeReviewCounts,
) -> String {
    let mut lines = Vec::new();

    lines.push(format!(
        "Changes: {} patch set(s) from {} sub-agent run(s)",
        counts.patchset_count, counts.agent_run_count,
    ));

    lines.push(format!(
        "Risk: {}",
        render_risk(payload.risk_score.as_ref())
    ));

    lines.push(render_conflicts(&payload.conflict_list));

    lines.push(format!(
        "Test evidence: {} record(s)",
        payload.test_evidence.len(),
    ));

    lines.join("\n")
}

/// Render the risk line. An unscored candidate (validator not yet run) is
/// reported as `not scored` rather than silently omitted.
fn render_risk(risk: Option<&RiskScore>) -> String {
    let Some(risk) = risk else {
        return "not scored".to_string();
    };
    let level = risk_level_label(&risk.level);
    if risk.factors.is_empty() {
        level.to_string()
    } else {
        let factors = risk
            .factors
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!("{level} ({factors})")
    }
}

/// Stable lowercase label for a [`RiskLevel`]. Exhaustive (no wildcard) so a
/// future variant added to the `#[non_exhaustive]` enum is a compile error here
/// — forcing it to be given a deliberate label rather than silently rendering
/// as a catch-all.
fn risk_level_label(level: &RiskLevel) -> &'static str {
    match level {
        RiskLevel::Low => "low",
        RiskLevel::Medium => "medium",
        RiskLevel::High => "high",
        RiskLevel::Critical => "critical",
    }
}

/// Render the conflict section: a clean candidate says so explicitly; otherwise
/// each conflict is listed `kind path[: detail]` in input order.
fn render_conflicts(conflicts: &[Conflict]) -> String {
    if conflicts.is_empty() {
        return "Conflicts: none".to_string();
    }
    let mut out = format!("Conflicts: {}", conflicts.len());
    for conflict in conflicts {
        let detail = conflict
            .detail
            .as_ref()
            .map(|d| format!(": {d}"))
            .unwrap_or_default();
        out.push_str(&format!(
            "\n  - {} {}{}",
            conflict.kind, conflict.path, detail
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::agent_run::EvidenceId;

    fn risk(level: RiskLevel, factors: &[(&str, &str)]) -> RiskScore {
        RiskScore {
            level,
            factors: factors
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    fn conflict(kind: &str, path: &str, detail: Option<&str>) -> Conflict {
        Conflict {
            kind: kind.to_string(),
            path: path.to_string(),
            detail: detail.map(str::to_string),
        }
    }

    #[test]
    fn clean_low_risk_candidate_renders_explicit_negatives() {
        let payload = MergeDecisionPayloadV0 {
            risk_score: Some(risk(RiskLevel::Low, &[])),
            ..MergeDecisionPayloadV0::default()
        };
        let summary = render_merge_review_summary(
            &payload,
            &MergeReviewCounts {
                patchset_count: 1,
                agent_run_count: 1,
            },
        );
        assert_eq!(
            summary,
            "Changes: 1 patch set(s) from 1 sub-agent run(s)\n\
             Risk: low\n\
             Conflicts: none\n\
             Test evidence: 0 record(s)",
        );
    }

    #[test]
    fn unscored_candidate_reports_not_scored() {
        // Validator hasn't run yet -> risk_score None.
        let payload = MergeDecisionPayloadV0::default();
        let summary = render_merge_review_summary(&payload, &MergeReviewCounts::default());
        assert!(
            summary.contains("Risk: not scored"),
            "unscored risk must be explicit, got: {summary}",
        );
    }

    #[test]
    fn risk_factors_are_rendered_in_order() {
        let payload = MergeDecisionPayloadV0 {
            risk_score: Some(risk(
                RiskLevel::Critical,
                &[("budget_token_exceeded", "1"), ("conflict_count", "2")],
            )),
            ..MergeDecisionPayloadV0::default()
        };
        let summary = render_merge_review_summary(&payload, &MergeReviewCounts::default());
        assert!(
            summary.contains("Risk: critical (budget_token_exceeded=1, conflict_count=2)"),
            "got: {summary}",
        );
    }

    #[test]
    fn conflicts_are_listed_with_kind_path_and_detail() {
        let payload = MergeDecisionPayloadV0 {
            risk_score: Some(risk(RiskLevel::High, &[])),
            conflict_list: vec![
                conflict(
                    "overlapping_hunk",
                    "src/a.rs",
                    Some("lines 1-3 overlap 3-8"),
                ),
                conflict("non_mergeable_cross_edit", "Cargo.lock", Some("lockfile")),
                conflict("same_symbol", "src/b.rs", None),
            ],
            ..MergeDecisionPayloadV0::default()
        };
        let summary = render_merge_review_summary(&payload, &MergeReviewCounts::default());
        assert!(summary.contains("Conflicts: 3"));
        assert!(summary.contains("\n  - overlapping_hunk src/a.rs: lines 1-3 overlap 3-8"));
        assert!(summary.contains("\n  - non_mergeable_cross_edit Cargo.lock: lockfile"));
        // A conflict with no detail renders without a trailing colon.
        assert!(summary.contains("\n  - same_symbol src/b.rs"));
        assert!(!summary.contains("same_symbol src/b.rs:"));
    }

    #[test]
    fn test_evidence_count_is_reported() {
        let payload = MergeDecisionPayloadV0 {
            risk_score: Some(risk(RiskLevel::Low, &[])),
            test_evidence: vec![EvidenceId::new(), EvidenceId::new(), EvidenceId::new()],
            ..MergeDecisionPayloadV0::default()
        };
        let summary = render_merge_review_summary(&payload, &MergeReviewCounts::default());
        assert!(
            summary.contains("Test evidence: 3 record(s)"),
            "got: {summary}"
        );
    }

    #[test]
    fn counts_from_candidate_match_candidate_vectors() {
        use crate::internal::ai::agent_run::{AgentPatchSetId, AgentRunId, MergeCandidateId};

        let candidate = MergeCandidate::new(
            MergeCandidateId::new(),
            vec![AgentPatchSetId::new(), AgentPatchSetId::new()],
            vec![AgentRunId::new(), AgentRunId::new(), AgentRunId::new()],
        );
        let counts = MergeReviewCounts::from_candidate(&candidate);
        assert_eq!(counts.patchset_count, 2);
        assert_eq!(counts.agent_run_count, 3);

        // And the derived counts render the expected Changes line.
        let summary = render_merge_review_summary(&MergeDecisionPayloadV0::default(), &counts);
        assert!(
            summary.contains("Changes: 2 patch set(s) from 3 sub-agent run(s)"),
            "got: {summary}",
        );
    }

    #[test]
    fn section_order_is_fixed() {
        let payload = MergeDecisionPayloadV0 {
            risk_score: Some(risk(RiskLevel::Medium, &[])),
            conflict_list: vec![conflict("same_symbol", "x.rs", None)],
            ..MergeDecisionPayloadV0::default()
        };
        let summary = render_merge_review_summary(&payload, &MergeReviewCounts::default());
        let changes = summary.find("Changes:").expect("has Changes");
        let risk_at = summary.find("Risk:").expect("has Risk");
        let conflicts = summary.find("Conflicts:").expect("has Conflicts");
        let evidence = summary.find("Test evidence:").expect("has Test evidence");
        assert!(
            changes < risk_at && risk_at < conflicts && conflicts < evidence,
            "sections must be Changes < Risk < Conflicts < Test evidence",
        );
    }
}
