//! CEX-S2-15 (1) merge risk-score computation.
//!
//! Pure scoring function that turns the aggregate signals of a
//! [`MergeCandidate`](super::decision::MergeCandidate) review into the
//! [`RiskScore`](super::decision::RiskScore) a human reviewer (or, later, the
//! flag-gated auto-merge path) reads before accepting sub-agent patches.
//!
//! # Scope
//!
//! This module owns **only** the risk-score arithmetic specified by CEX-S2-15
//! 完成判定 (1): given the per-merge signal counts it returns a [`RiskScore`].
//! It does **not** schedule the validator test DAG, collect `test_evidence`, or
//! write the `MergeDecision` — those are the rest of CEX-S2-15 (ValidatorEngine)
//! and are wired separately. Per CEX-S2-13 schema-ownership the [`RiskScore`] /
//! `RiskLevel` shapes are frozen in [`super::decision`]; this module only
//! *constructs* values and never mutates the schema, so its output drops into
//! the CEX-S2-13-declared `MergeDecision.risk_score` field (`None` → `Some`).
//!
//! # Inputs and weighting (CEX-S2-15 完成判定 (1) / Step 2.5 Phase 4)
//!
//! The score is a saturating weighted sum of:
//! - per-[`BudgetDimension`](super::BudgetDimension) `budget_exceeded` hit
//!   counts, weighted **token / cost highest, wall_clock next, tool_call /
//!   source_call lowest** (the documented dimension priority);
//! - merge-quality signals: conflict count, failed-run count, and
//!   unverified-patch-scope count;
//! - parallelism breadth: each sub-agent beyond the first.
//!
//! `factors` enumerates every signal that contributed a non-zero amount (each
//! non-zero budget dimension is always listed, per the completion criterion),
//! keyed with the stable vocabulary already pinned by the `decision` module's
//! `risk_score_round_trips_with_factors` wire test (e.g.
//! `("budget_token_exceeded", "3")`, `("conflict_count", "2")`), so a reviewer
//! can see exactly why a candidate scored the way it did.

use super::{
    AgentRun, AgentRunStatus, BudgetDimension,
    decision::{RiskLevel, RiskScore},
    event::AgentRunEvent,
};

/// Per-[`BudgetDimension`](super::BudgetDimension) tally of `budget_exceeded`
/// occurrences across the sub-agent runs aggregated into a merge candidate.
///
/// The caller (CEX-S2-15 ValidatorEngine) walks the run events and calls
/// [`record`](Self::record) once per `AgentRunEvent::budget_exceeded` event;
/// the resulting counts feed [`compute_merge_risk_score`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BudgetExceededCounts {
    /// `token` dimension breaches (highest weight).
    pub token: u32,
    /// `tool_call` dimension breaches (lowest weight).
    pub tool_call: u32,
    /// `wall_clock` dimension breaches (middle weight).
    pub wall_clock: u32,
    /// `source_call` dimension breaches (lowest weight).
    pub source_call: u32,
    /// `cost` dimension breaches (highest weight).
    pub cost: u32,
}

impl BudgetExceededCounts {
    /// Tally one `AgentRunEvent::budget_exceeded` occurrence for `dimension`.
    ///
    /// Saturating so a pathological run cannot overflow the counter.
    pub fn record(&mut self, dimension: BudgetDimension) {
        // INVARIANT: exhaustive (no wildcard) so a future `BudgetDimension`
        // variant is a compile error here rather than a silently dropped
        // breach — `BudgetDimension` is `#[non_exhaustive]` only for
        // downstream crates; in-crate matches must stay total.
        let slot = match dimension {
            BudgetDimension::Token => &mut self.token,
            BudgetDimension::ToolCall => &mut self.tool_call,
            BudgetDimension::WallClock => &mut self.wall_clock,
            BudgetDimension::SourceCall => &mut self.source_call,
            BudgetDimension::Cost => &mut self.cost,
        };
        *slot = slot.saturating_add(1);
    }
}

/// Aggregate signal counts for a single `MergeCandidate` review, fed to
/// [`compute_merge_risk_score`].
///
/// All fields are plain counts so the scorer stays pure and trivially
/// testable; the ValidatorEngine tallies them from the candidate's
/// `AgentPatchSet`s, conflict detection, run outcomes and `budget_exceeded`
/// events.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MergeRiskInputs {
    /// Number of sub-agents whose patches the candidate aggregates. The first
    /// agent is the baseline; each additional agent adds a little merge risk.
    pub sub_agent_count: u32,
    /// Detected merge conflicts (overlapping hunk / same symbol /
    /// test+lockfile cross-edit).
    pub conflict_count: u32,
    /// Sub-agent runs that ended in a non-success terminal state.
    pub failed_run_count: u32,
    /// Patches whose scope the validator could not verify.
    pub unverified_patch_scope: u32,
    /// Per-dimension `budget_exceeded` hit counts.
    pub budget_exceeded: BudgetExceededCounts,
}

// Budget-dimension weights: token / cost highest, wall_clock next, tool_call /
// source_call lowest (CEX-S2-15 完成判定 (1) dimension priority).
const WEIGHT_TOKEN: u32 = 4;
const WEIGHT_COST: u32 = 4;
const WEIGHT_WALL_CLOCK: u32 = 2;
const WEIGHT_TOOL_CALL: u32 = 1;
const WEIGHT_SOURCE_CALL: u32 = 1;
// Merge-quality signal weights.
const WEIGHT_CONFLICT: u32 = 3;
const WEIGHT_FAILED_RUN: u32 = 3;
const WEIGHT_UNVERIFIED_SCOPE: u32 = 2;

// Score thresholds (inclusive lower bound) for each escalating level.
const MEDIUM_THRESHOLD: u32 = 2;
const HIGH_THRESHOLD: u32 = 4;
const CRITICAL_THRESHOLD: u32 = 8;

/// Compute the merge [`RiskScore`] from the aggregate review signals.
///
/// Returns the escalated `RiskLevel` plus the `factors` breakdown listing every
/// non-zero contributing signal (each non-zero budget dimension is always
/// listed, per CEX-S2-15 完成判定 (1)). The result is written into the
/// CEX-S2-13-declared `MergeDecision.risk_score` field (`None` → `Some`) by the
/// ValidatorEngine; this function performs no I/O and mutates no schema.
pub fn compute_merge_risk_score(inputs: &MergeRiskInputs) -> RiskScore {
    RiskScore {
        level: level_for_score(weighted_score(inputs)),
        factors: collect_factors(inputs),
    }
}

/// Tally the [`MergeRiskInputs`] for a merge candidate from its aggregated
/// sub-agent runs, their lifecycle events, and the validator-supplied conflict
/// / unverified-scope counts — the **pure** half of the CEX-S2-15
/// ValidatorEngine's risk-input step (完成判定 (1)).
///
/// - `sub_agent_count` = the runs aggregated into the candidate.
/// - `failed_run_count` = runs that ended in [`AgentRunStatus::Failed`] (the
///   only non-success terminal state — `Completed` is success and
///   `Queued`/`Running`/`Blocked` are still in flight, so neither counts).
/// - `budget_exceeded` = one [`BudgetExceededCounts::record`] per
///   [`AgentRunEvent::BudgetExceeded`], bucketed by `dimension`.
/// - `conflict_count` / `unverified_patch_scope` are supplied by the caller
///   (conflict detection and the validator's scope check live outside this
///   pure tally).
///
/// Pure — a fold over the supplied slices with no I/O — so the orchestrator
/// ValidatorEngine owns only the loading of `runs` / `events`, and the result
/// feeds [`compute_merge_risk_score`] unchanged.
pub fn gather_merge_risk_inputs(
    runs: &[AgentRun],
    events: &[AgentRunEvent],
    conflict_count: u32,
    unverified_patch_scope: u32,
) -> MergeRiskInputs {
    let mut budget_exceeded = BudgetExceededCounts::default();
    for event in events {
        if let AgentRunEvent::BudgetExceeded { dimension, .. } = event {
            budget_exceeded.record(*dimension);
        }
    }
    let failed_run_count = runs
        .iter()
        .filter(|run| run.status == AgentRunStatus::Failed)
        .count() as u32;
    MergeRiskInputs {
        sub_agent_count: runs.len() as u32,
        conflict_count,
        failed_run_count,
        unverified_patch_scope,
        budget_exceeded,
    }
}

/// Saturating weighted sum of every risk signal.
fn weighted_score(i: &MergeRiskInputs) -> u32 {
    let b = &i.budget_exceeded;
    [
        b.token.saturating_mul(WEIGHT_TOKEN),
        b.cost.saturating_mul(WEIGHT_COST),
        b.wall_clock.saturating_mul(WEIGHT_WALL_CLOCK),
        b.tool_call.saturating_mul(WEIGHT_TOOL_CALL),
        b.source_call.saturating_mul(WEIGHT_SOURCE_CALL),
        i.conflict_count.saturating_mul(WEIGHT_CONFLICT),
        i.failed_run_count.saturating_mul(WEIGHT_FAILED_RUN),
        i.unverified_patch_scope
            .saturating_mul(WEIGHT_UNVERIFIED_SCOPE),
        // Each sub-agent beyond the first adds one breadth point.
        i.sub_agent_count.saturating_sub(1),
    ]
    .into_iter()
    .fold(0u32, u32::saturating_add)
}

fn level_for_score(score: u32) -> RiskLevel {
    if score >= CRITICAL_THRESHOLD {
        RiskLevel::Critical
    } else if score >= HIGH_THRESHOLD {
        RiskLevel::High
    } else if score >= MEDIUM_THRESHOLD {
        RiskLevel::Medium
    } else {
        RiskLevel::Low
    }
}

/// Enumerate the non-zero contributing factors in a deterministic order: budget
/// dimensions first (in [`BudgetDimension`](super::BudgetDimension) declaration
/// order), then the merge-quality signals, then parallelism breadth. Factor
/// keys use the stable vocabulary pinned by the `decision` round-trip test.
fn collect_factors(i: &MergeRiskInputs) -> Vec<(String, String)> {
    let b = &i.budget_exceeded;
    let mut factors = Vec::new();
    push_factor(&mut factors, "budget_token_exceeded", b.token);
    push_factor(&mut factors, "budget_tool_call_exceeded", b.tool_call);
    push_factor(&mut factors, "budget_wall_clock_exceeded", b.wall_clock);
    push_factor(&mut factors, "budget_source_call_exceeded", b.source_call);
    push_factor(&mut factors, "budget_cost_exceeded", b.cost);
    push_factor(&mut factors, "conflict_count", i.conflict_count);
    push_factor(&mut factors, "failed_run_count", i.failed_run_count);
    push_factor(
        &mut factors,
        "unverified_patch_scope",
        i.unverified_patch_scope,
    );
    // Only agents beyond the first add risk; report the raw count once it
    // actually contributed (i.e. there is more than one sub-agent).
    if i.sub_agent_count > 1 {
        factors.push(("sub_agent_count".to_string(), i.sub_agent_count.to_string()));
    }
    factors
}

/// Push a `(key, count)` factor only when `count` is non-zero.
fn push_factor(factors: &mut Vec<(String, String)>, key: &str, count: u32) {
    if count > 0 {
        factors.push((key.to_string(), count.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::{
        super::{AgentRunId, AgentTaskId},
        *,
    };

    fn run(status: AgentRunStatus) -> AgentRun {
        let id = AgentRunId::new();
        AgentRun {
            id,
            task_id: AgentTaskId::new(),
            thread_id: uuid::Uuid::from_u128(0x7777),
            provider: "deepseek".to_string(),
            model: "deepseek-chat".to_string(),
            transcript_path: format!("agents/{}.jsonl", id.0),
            workspace_path: None,
            status,
        }
    }

    /// CEX-S2-15 完成判定 (1): the pure tally counts sub-agents, failed runs and
    /// per-dimension budget breaches from the raw run/event slices, passes the
    /// validator-supplied conflict / unverified counts through, and feeds
    /// `compute_merge_risk_score` unchanged. Non-budget events and non-failed
    /// runs are ignored.
    #[test]
    fn gather_merge_risk_inputs_tallies_runs_and_budget_events() {
        let failed = run(AgentRunStatus::Failed);
        let completed = run(AgentRunStatus::Completed);
        let running = run(AgentRunStatus::Running);
        let runs = vec![failed.clone(), completed.clone(), running.clone()];

        let events = vec![
            AgentRunEvent::BudgetExceeded {
                agent_run_id: failed.id,
                dimension: BudgetDimension::Token,
            },
            AgentRunEvent::BudgetExceeded {
                agent_run_id: failed.id,
                dimension: BudgetDimension::Token,
            },
            AgentRunEvent::BudgetExceeded {
                agent_run_id: completed.id,
                dimension: BudgetDimension::Cost,
            },
            // A non-budget lifecycle event must not be tallied.
            AgentRunEvent::Started {
                agent_run_id: running.id,
            },
        ];

        let inputs = gather_merge_risk_inputs(&runs, &events, 1, 2);
        assert_eq!(inputs.sub_agent_count, 3, "all aggregated runs are counted");
        assert_eq!(
            inputs.failed_run_count, 1,
            "only the Failed run counts (Completed = success, Running = in flight)"
        );
        assert_eq!(
            inputs.conflict_count, 1,
            "caller-supplied conflict count passes through"
        );
        assert_eq!(
            inputs.unverified_patch_scope, 2,
            "caller-supplied scope count passes through"
        );
        assert_eq!(inputs.budget_exceeded.token, 2);
        assert_eq!(inputs.budget_exceeded.cost, 1);
        assert_eq!(inputs.budget_exceeded.wall_clock, 0);
        assert_eq!(inputs.budget_exceeded.tool_call, 0);
        assert_eq!(inputs.budget_exceeded.source_call, 0);

        // The gathered inputs feed the scorer unchanged.
        let score = compute_merge_risk_score(&inputs);
        // 2*token(4) + 1*cost(4) + 1*conflict(3) + 1*failed(3) + 2*unverified(2)
        // + (3-1) breadth = 8 + 4 + 3 + 3 + 4 + 2 = 24 ≥ critical threshold.
        assert_eq!(score.level, RiskLevel::Critical);
    }

    /// Empty inputs tally to the all-zero default (no panic, no underflow on the
    /// `sub_agent_count - 1` breadth term inside the scorer).
    #[test]
    fn gather_merge_risk_inputs_empty_is_zeroed() {
        let inputs = gather_merge_risk_inputs(&[], &[], 0, 0);
        assert_eq!(inputs, MergeRiskInputs::default());
        assert_eq!(compute_merge_risk_score(&inputs).level, RiskLevel::Low);
    }

    /// `record` routes each `BudgetDimension` to its own counter and is
    /// saturating; the all-zero default is distinguishable from any breach.
    #[test]
    fn record_maps_each_dimension_to_its_slot() {
        let mut counts = BudgetExceededCounts::default();
        counts.record(BudgetDimension::Token);
        counts.record(BudgetDimension::Token);
        counts.record(BudgetDimension::ToolCall);
        counts.record(BudgetDimension::WallClock);
        counts.record(BudgetDimension::SourceCall);
        counts.record(BudgetDimension::Cost);
        assert_eq!(
            counts,
            BudgetExceededCounts {
                token: 2,
                tool_call: 1,
                wall_clock: 1,
                source_call: 1,
                cost: 1,
            },
        );
        assert_ne!(counts, BudgetExceededCounts::default());
    }

    /// `record` is saturating: a counter already at `u32::MAX` stays there
    /// rather than wrapping back to zero.
    #[test]
    fn record_saturates_at_u32_max() {
        let mut counts = BudgetExceededCounts {
            token: u32::MAX,
            ..Default::default()
        };
        counts.record(BudgetDimension::Token);
        assert_eq!(counts.token, u32::MAX);
    }

    // ---- 5-case risk-score corpus (CEX-S2-15 完成判定 (1) fixture) ----

    /// Corpus 1: a benign single-agent candidate with no breaches scores Low
    /// and reports no factors.
    #[test]
    fn corpus_benign_single_agent_is_low_with_no_factors() {
        let score = compute_merge_risk_score(&MergeRiskInputs {
            sub_agent_count: 1,
            ..Default::default()
        });
        assert_eq!(score.level, RiskLevel::Low);
        assert!(score.factors.is_empty());
    }

    /// Corpus 2: a single lowest-weight (tool_call) budget breach stays Low but
    /// is still surfaced as a factor.
    #[test]
    fn corpus_single_tool_call_breach_is_low_with_factor() {
        let score = compute_merge_risk_score(&MergeRiskInputs {
            sub_agent_count: 1,
            budget_exceeded: BudgetExceededCounts {
                tool_call: 1,
                ..Default::default()
            },
            ..Default::default()
        });
        assert_eq!(score.level, RiskLevel::Low);
        assert_eq!(
            score.factors,
            vec![("budget_tool_call_exceeded".to_string(), "1".to_string())],
        );
    }

    /// Corpus 3: a single middle-weight (wall_clock) breach escalates to
    /// Medium.
    #[test]
    fn corpus_single_wall_clock_breach_is_medium() {
        let score = compute_merge_risk_score(&MergeRiskInputs {
            sub_agent_count: 1,
            budget_exceeded: BudgetExceededCounts {
                wall_clock: 1,
                ..Default::default()
            },
            ..Default::default()
        });
        assert_eq!(score.level, RiskLevel::Medium);
        assert_eq!(
            score.factors,
            vec![("budget_wall_clock_exceeded".to_string(), "1".to_string())],
        );
    }

    /// Corpus 4: a single highest-weight (token) breach escalates to High.
    #[test]
    fn corpus_single_token_breach_is_high() {
        let score = compute_merge_risk_score(&MergeRiskInputs {
            sub_agent_count: 1,
            budget_exceeded: BudgetExceededCounts {
                token: 1,
                ..Default::default()
            },
            ..Default::default()
        });
        assert_eq!(score.level, RiskLevel::High);
        assert_eq!(
            score.factors,
            vec![("budget_token_exceeded".to_string(), "1".to_string())],
        );
    }

    /// Corpus 5: combined token + cost breaches plus a conflict and a second
    /// sub-agent push the candidate to Critical, and every non-zero signal is
    /// listed in the deterministic factor order (budget dimensions first, then
    /// quality signals, then breadth).
    #[test]
    fn corpus_token_cost_conflict_is_critical_with_ordered_factors() {
        let score = compute_merge_risk_score(&MergeRiskInputs {
            sub_agent_count: 2,
            conflict_count: 1,
            budget_exceeded: BudgetExceededCounts {
                token: 1,
                cost: 1,
                ..Default::default()
            },
            ..Default::default()
        });
        // 4 (token) + 4 (cost) + 3 (conflict) + 1 (extra sub-agent) = 12 ≥ 8.
        assert_eq!(score.level, RiskLevel::Critical);
        assert_eq!(
            score.factors,
            vec![
                ("budget_token_exceeded".to_string(), "1".to_string()),
                ("budget_cost_exceeded".to_string(), "1".to_string()),
                ("conflict_count".to_string(), "1".to_string()),
                ("sub_agent_count".to_string(), "2".to_string()),
            ],
        );
    }

    /// The documented dimension priority — token / cost highest, wall_clock
    /// next, tool_call / source_call lowest — must hold: the same single breach
    /// count maps to a strictly higher level the higher the dimension's weight.
    #[test]
    fn dimension_weighting_orders_levels() {
        let level = |budget_exceeded: BudgetExceededCounts| {
            compute_merge_risk_score(&MergeRiskInputs {
                sub_agent_count: 1,
                budget_exceeded,
                ..Default::default()
            })
            .level
        };
        assert_eq!(
            level(BudgetExceededCounts {
                token: 1,
                ..Default::default()
            }),
            RiskLevel::High,
        );
        assert_eq!(
            level(BudgetExceededCounts {
                cost: 1,
                ..Default::default()
            }),
            RiskLevel::High,
        );
        assert_eq!(
            level(BudgetExceededCounts {
                wall_clock: 1,
                ..Default::default()
            }),
            RiskLevel::Medium,
        );
        assert_eq!(
            level(BudgetExceededCounts {
                tool_call: 1,
                ..Default::default()
            }),
            RiskLevel::Low,
        );
        assert_eq!(
            level(BudgetExceededCounts {
                source_call: 1,
                ..Default::default()
            }),
            RiskLevel::Low,
        );
    }

    /// `factors` lists only the non-zero budget dimensions, in
    /// `BudgetDimension` declaration order, and omits zero dimensions.
    #[test]
    fn factors_list_only_nonzero_dimensions_in_order() {
        let score = compute_merge_risk_score(&MergeRiskInputs {
            sub_agent_count: 1,
            budget_exceeded: BudgetExceededCounts {
                token: 2,
                wall_clock: 3,
                cost: 1,
                ..Default::default()
            },
            ..Default::default()
        });
        assert_eq!(
            score.factors,
            vec![
                ("budget_token_exceeded".to_string(), "2".to_string()),
                ("budget_wall_clock_exceeded".to_string(), "3".to_string()),
                ("budget_cost_exceeded".to_string(), "1".to_string()),
            ],
        );
    }

    /// Every budget dimension has a pinned stable factor key, emitted in
    /// `BudgetDimension` declaration order (token, tool_call, wall_clock,
    /// source_call, cost). Guards the full key vocabulary, including the
    /// otherwise lightly-exercised `source_call` and `tool_call` keys.
    #[test]
    fn all_budget_dimension_factor_keys_are_pinned() {
        let score = compute_merge_risk_score(&MergeRiskInputs {
            sub_agent_count: 1,
            budget_exceeded: BudgetExceededCounts {
                token: 1,
                tool_call: 2,
                wall_clock: 3,
                source_call: 4,
                cost: 5,
            },
            ..Default::default()
        });
        assert_eq!(
            score.factors,
            vec![
                ("budget_token_exceeded".to_string(), "1".to_string()),
                ("budget_tool_call_exceeded".to_string(), "2".to_string()),
                ("budget_wall_clock_exceeded".to_string(), "3".to_string()),
                ("budget_source_call_exceeded".to_string(), "4".to_string()),
                ("budget_cost_exceeded".to_string(), "5".to_string()),
            ],
        );
    }

    /// Quality signals (conflict / failed-run / unverified scope) contribute
    /// and are surfaced: a single conflict alone is Medium; a failed run plus
    /// an unverified-scope patch reaches High.
    #[test]
    fn quality_signals_contribute_and_surface() {
        let one_conflict = compute_merge_risk_score(&MergeRiskInputs {
            sub_agent_count: 1,
            conflict_count: 1,
            ..Default::default()
        });
        assert_eq!(one_conflict.level, RiskLevel::Medium);
        assert_eq!(
            one_conflict.factors,
            vec![("conflict_count".to_string(), "1".to_string())],
        );

        let heavier = compute_merge_risk_score(&MergeRiskInputs {
            sub_agent_count: 1,
            failed_run_count: 1,
            unverified_patch_scope: 1,
            ..Default::default()
        });
        // 3 (failed run) + 2 (unverified scope) = 5 ≥ 4.
        assert_eq!(heavier.level, RiskLevel::High);
        assert_eq!(
            heavier.factors,
            vec![
                ("failed_run_count".to_string(), "1".to_string()),
                ("unverified_patch_scope".to_string(), "1".to_string()),
            ],
        );
    }

    /// A lone single sub-agent never contributes breadth risk; the second
    /// agent is the first to add a point and surface the factor, and the third
    /// crosses into Medium.
    #[test]
    fn sub_agent_breadth_starts_at_second_agent() {
        let one = compute_merge_risk_score(&MergeRiskInputs {
            sub_agent_count: 1,
            ..Default::default()
        });
        assert_eq!(one.level, RiskLevel::Low);
        assert!(one.factors.is_empty());

        let two = compute_merge_risk_score(&MergeRiskInputs {
            sub_agent_count: 2,
            ..Default::default()
        });
        // breadth 1 < Medium threshold, but the count is surfaced.
        assert_eq!(two.level, RiskLevel::Low);
        assert_eq!(
            two.factors,
            vec![("sub_agent_count".to_string(), "2".to_string())],
        );

        let three = compute_merge_risk_score(&MergeRiskInputs {
            sub_agent_count: 3,
            ..Default::default()
        });
        // breadth 2 ≥ Medium threshold.
        assert_eq!(three.level, RiskLevel::Medium);
    }

    /// The score is saturating: pathological `u32::MAX` counts produce Critical
    /// without panicking on overflow.
    #[test]
    fn saturating_does_not_panic_on_extreme_counts() {
        let score = compute_merge_risk_score(&MergeRiskInputs {
            sub_agent_count: u32::MAX,
            conflict_count: u32::MAX,
            failed_run_count: u32::MAX,
            unverified_patch_scope: u32::MAX,
            budget_exceeded: BudgetExceededCounts {
                token: u32::MAX,
                tool_call: u32::MAX,
                wall_clock: u32::MAX,
                source_call: u32::MAX,
                cost: u32::MAX,
            },
        });
        assert_eq!(score.level, RiskLevel::Critical);
    }

    /// The produced `RiskScore` serialises through the CEX-S2-13 frozen wire
    /// shape, confirming this module only constructs the existing schema.
    #[test]
    fn produced_score_serialises_through_frozen_schema() {
        let score = compute_merge_risk_score(&MergeRiskInputs {
            sub_agent_count: 1,
            budget_exceeded: BudgetExceededCounts {
                token: 1,
                ..Default::default()
            },
            ..Default::default()
        });
        let wire = serde_json::to_string(&score).expect("serialize RiskScore");
        assert_eq!(
            wire,
            r#"{"level":"high","factors":[["budget_token_exceeded","1"]]}"#,
        );
    }
}
