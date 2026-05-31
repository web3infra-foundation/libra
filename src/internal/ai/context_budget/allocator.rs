//! Deterministic allocation of concrete context candidates against a budget.
//!
//! 针对预算的具体上下文候选的确定性分配。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::budget::{ContextBudget, ContextSegmentKind};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextBudgetCandidate {
    pub id: String,
    pub segment: ContextSegmentKind,
    pub token_estimate: u64,
    #[serde(default)]
    pub non_compressible: bool,
}

impl ContextBudgetCandidate {
    pub fn new(id: impl Into<String>, segment: ContextSegmentKind, token_estimate: u64) -> Self {
        Self {
            id: id.into(),
            segment,
            token_estimate,
            non_compressible: false,
        }
    }

    pub fn non_compressible(mut self, value: bool) -> Self {
        self.non_compressible = value;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AllocationOmissionReason {
    UnknownSegment,
    SegmentBudgetExceeded,
    TotalBudgetExceeded,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextAllocationOmission {
    pub id: String,
    pub segment: ContextSegmentKind,
    pub token_estimate: u64,
    pub reason: AllocationOmissionReason,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextAllocation {
    selected: Vec<ContextBudgetCandidate>,
    omitted: Vec<ContextAllocationOmission>,
    total_selected_tokens: u64,
    budget_exceeded_by: u64,
}

impl ContextAllocation {
    pub fn selected(&self) -> &[ContextBudgetCandidate] {
        &self.selected
    }

    pub fn omitted(&self) -> &[ContextAllocationOmission] {
        &self.omitted
    }

    pub fn selected_ids(&self) -> Vec<&str> {
        self.selected
            .iter()
            .map(|candidate| candidate.id.as_str())
            .collect()
    }

    pub fn omission_for(&self, id: &str) -> Option<&ContextAllocationOmission> {
        self.omitted.iter().find(|omission| omission.id == id)
    }

    pub fn total_selected_tokens(&self) -> u64 {
        self.total_selected_tokens
    }

    pub fn budget_exceeded_by(&self) -> u64 {
        self.budget_exceeded_by
    }
}

#[derive(Clone, Debug)]
pub struct ContextBudgetAllocator {
    budget: ContextBudget,
}

impl ContextBudgetAllocator {
    pub fn new(budget: ContextBudget) -> Self {
        Self { budget }
    }

    pub fn allocate(&self, candidates: Vec<ContextBudgetCandidate>) -> ContextAllocation {
        let mut selected = Vec::new();
        let mut omitted = Vec::new();
        let mut compressible = Vec::new();
        let mut segment_used = HashMap::new();
        let mut total_selected_tokens = 0_u64;

        for (index, candidate) in candidates.into_iter().enumerate() {
            let Some(segment_budget) = self.budget.segment(candidate.segment) else {
                omitted.push(omission(
                    candidate,
                    AllocationOmissionReason::UnknownSegment,
                ));
                continue;
            };

            if candidate.non_compressible || segment_budget.non_compressible {
                total_selected_tokens =
                    total_selected_tokens.saturating_add(candidate.token_estimate);
                add_segment_tokens(
                    &mut segment_used,
                    candidate.segment,
                    candidate.token_estimate,
                );
                selected.push(IndexedCandidate { index, candidate });
            } else {
                compressible.push(IndexedCandidate { index, candidate });
            }
        }

        compressible.sort_by_key(|indexed| {
            let priority_rank = self
                .budget
                .segment(indexed.candidate.segment)
                .map(|segment| segment.priority.retention_rank())
                .unwrap_or(u8::MAX);
            (priority_rank, indexed.index)
        });

        for indexed in compressible {
            let candidate = indexed.candidate;
            let Some(segment_budget) = self.budget.segment(candidate.segment) else {
                omitted.push(omission(
                    candidate,
                    AllocationOmissionReason::UnknownSegment,
                ));
                continue;
            };

            if total_selected_tokens.saturating_add(candidate.token_estimate)
                > self.budget.max_prompt_tokens()
            {
                omitted.push(omission(
                    candidate,
                    AllocationOmissionReason::TotalBudgetExceeded,
                ));
                continue;
            }

            let used_for_segment = segment_used
                .get(&candidate.segment)
                .copied()
                .unwrap_or_default();
            if used_for_segment.saturating_add(candidate.token_estimate) > segment_budget.max_tokens
            {
                omitted.push(omission(
                    candidate,
                    AllocationOmissionReason::SegmentBudgetExceeded,
                ));
                continue;
            }

            total_selected_tokens = total_selected_tokens.saturating_add(candidate.token_estimate);
            add_segment_tokens(
                &mut segment_used,
                candidate.segment,
                candidate.token_estimate,
            );
            selected.push(IndexedCandidate {
                index: indexed.index,
                candidate,
            });
        }

        selected.sort_by_key(|indexed| indexed.index);
        ContextAllocation {
            selected: selected
                .into_iter()
                .map(|indexed| indexed.candidate)
                .collect(),
            omitted,
            total_selected_tokens,
            budget_exceeded_by: total_selected_tokens
                .saturating_sub(self.budget.max_prompt_tokens()),
        }
    }
}

#[derive(Clone, Debug)]
struct IndexedCandidate {
    index: usize,
    candidate: ContextBudgetCandidate,
}

fn add_segment_tokens(
    segment_used: &mut HashMap<ContextSegmentKind, u64>,
    segment: ContextSegmentKind,
    tokens: u64,
) {
    let entry = segment_used.entry(segment).or_insert(0);
    *entry = entry.saturating_add(tokens);
}

fn omission(
    candidate: ContextBudgetCandidate,
    reason: AllocationOmissionReason,
) -> ContextAllocationOmission {
    ContextAllocationOmission {
        id: candidate.id,
        segment: candidate.segment,
        token_estimate: candidate.token_estimate,
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::context_budget::budget::{
        ContextBudget, ContextSegmentBudget, TruncationPolicy,
    };

    fn budget_with(segments: Vec<ContextSegmentBudget>, total: u64) -> ContextBudget {
        ContextBudget::from_segments(total, segments).expect("valid segments")
    }

    fn seg(kind: ContextSegmentKind, max_tokens: u64) -> ContextSegmentBudget {
        ContextSegmentBudget::new(kind, max_tokens, TruncationPolicy::OldestFirst)
    }

    fn never_seg(kind: ContextSegmentKind, max_tokens: u64) -> ContextSegmentBudget {
        ContextSegmentBudget::new(kind, max_tokens, TruncationPolicy::Never)
    }

    #[test]
    fn candidate_new_defaults_non_compressible_to_false() {
        // INVARIANT: `new` builds a compressible candidate unless the
        // caller chains `.non_compressible(true)`. A flip in the
        // default would invert how the allocator treats every
        // candidate created through the convenience constructor.
        let candidate = ContextBudgetCandidate::new("id-1", ContextSegmentKind::SystemRules, 100);
        assert_eq!(candidate.id, "id-1");
        assert_eq!(candidate.segment, ContextSegmentKind::SystemRules);
        assert_eq!(candidate.token_estimate, 100);
        assert!(!candidate.non_compressible);
    }

    #[test]
    fn candidate_builder_toggles_non_compressible_flag() {
        let candidate = ContextBudgetCandidate::new("id", ContextSegmentKind::RecentMessages, 10)
            .non_compressible(true);
        assert!(candidate.non_compressible);
        let off = candidate.clone().non_compressible(false);
        assert!(!off.non_compressible);
    }

    #[test]
    fn allocate_with_no_candidates_yields_empty_allocation() {
        let budget = budget_with(vec![seg(ContextSegmentKind::ToolResults, 100)], 100);
        let allocator = ContextBudgetAllocator::new(budget);
        let allocation = allocator.allocate(Vec::new());
        assert!(allocation.selected().is_empty());
        assert!(allocation.omitted().is_empty());
        assert_eq!(allocation.total_selected_tokens(), 0);
        assert_eq!(allocation.budget_exceeded_by(), 0);
        assert!(allocation.selected_ids().is_empty());
    }

    #[test]
    fn allocate_preserves_input_order_for_selected_candidates() {
        // INVARIANT: even though compressible candidates are sorted
        // internally by retention rank, the final `selected` list is
        // re-sorted by original input index so callers can rely on
        // the order they passed in.
        let budget = budget_with(
            vec![
                seg(ContextSegmentKind::SystemRules, 100),
                seg(ContextSegmentKind::ToolResults, 100),
            ],
            500,
        );
        let allocator = ContextBudgetAllocator::new(budget);
        let allocation = allocator.allocate(vec![
            ContextBudgetCandidate::new("low", ContextSegmentKind::ToolResults, 10),
            ContextBudgetCandidate::new("high", ContextSegmentKind::SystemRules, 10),
        ]);
        assert_eq!(allocation.selected_ids(), vec!["low", "high"]);
        assert_eq!(allocation.total_selected_tokens(), 20);
    }

    #[test]
    fn allocate_omits_candidate_whose_segment_is_not_in_budget() {
        // INVARIANT: a candidate referencing a segment not defined in
        // the budget is recorded as `UnknownSegment` — never silently
        // dropped or coerced into another segment. The omission row
        // surfaces a config drift to the operator.
        let budget = budget_with(vec![seg(ContextSegmentKind::SystemRules, 100)], 100);
        let allocator = ContextBudgetAllocator::new(budget);
        let allocation = allocator.allocate(vec![ContextBudgetCandidate::new(
            "stray",
            ContextSegmentKind::ToolResults,
            10,
        )]);
        assert!(allocation.selected().is_empty());
        let omission = allocation
            .omission_for("stray")
            .expect("stray must appear in omissions");
        assert_eq!(omission.id, "stray");
        assert_eq!(omission.segment, ContextSegmentKind::ToolResults);
        assert_eq!(omission.token_estimate, 10);
        assert_eq!(omission.reason, AllocationOmissionReason::UnknownSegment);
    }

    #[test]
    fn allocate_omits_compressible_candidate_exceeding_segment_budget() {
        let budget = budget_with(vec![seg(ContextSegmentKind::ToolResults, 50)], 500);
        let allocator = ContextBudgetAllocator::new(budget);
        let allocation = allocator.allocate(vec![
            ContextBudgetCandidate::new("a", ContextSegmentKind::ToolResults, 30),
            ContextBudgetCandidate::new("b", ContextSegmentKind::ToolResults, 30),
        ]);
        assert_eq!(allocation.selected_ids(), vec!["a"]);
        let omission = allocation.omission_for("b").expect("b must be omitted");
        assert_eq!(
            omission.reason,
            AllocationOmissionReason::SegmentBudgetExceeded
        );
        assert_eq!(allocation.total_selected_tokens(), 30);
        assert_eq!(allocation.budget_exceeded_by(), 0);
    }

    #[test]
    fn allocate_omits_compressible_candidate_exceeding_total_budget() {
        let budget = budget_with(
            vec![
                seg(ContextSegmentKind::SystemRules, 1_000),
                seg(ContextSegmentKind::ToolResults, 1_000),
            ],
            100,
        );
        let allocator = ContextBudgetAllocator::new(budget);
        let allocation = allocator.allocate(vec![
            ContextBudgetCandidate::new("a", ContextSegmentKind::SystemRules, 60),
            ContextBudgetCandidate::new("b", ContextSegmentKind::ToolResults, 60),
        ]);
        // Both segment budgets allow the candidates individually, but
        // the prompt-wide cap is 100; the second candidate must be
        // omitted with `TotalBudgetExceeded`.
        assert_eq!(allocation.selected_ids(), vec!["a"]);
        let omission = allocation.omission_for("b").expect("b must be omitted");
        assert_eq!(
            omission.reason,
            AllocationOmissionReason::TotalBudgetExceeded
        );
    }

    #[test]
    fn allocate_includes_non_compressible_segment_even_when_total_overflows() {
        // INVARIANT: `non_compressible` segments (TruncationPolicy::Never
        // or per-candidate flag) are added unconditionally, even past
        // the prompt cap. `budget_exceeded_by` then reports the
        // overflow so callers can decide to refuse the prompt. A
        // regression that started skipping non-compressible content
        // would silently drop the SystemRules segment.
        let budget = budget_with(
            vec![
                never_seg(ContextSegmentKind::SystemRules, 1_000),
                seg(ContextSegmentKind::ToolResults, 1_000),
            ],
            10,
        );
        let allocator = ContextBudgetAllocator::new(budget);
        let allocation = allocator.allocate(vec![
            ContextBudgetCandidate::new("rules", ContextSegmentKind::SystemRules, 50),
            ContextBudgetCandidate::new("tools", ContextSegmentKind::ToolResults, 5),
        ]);
        assert!(
            allocation.selected_ids().contains(&"rules"),
            "non_compressible candidate must always land in selected"
        );
        assert_eq!(allocation.total_selected_tokens(), 50);
        assert_eq!(
            allocation.budget_exceeded_by(),
            40,
            "overflow must be surfaced verbatim so the caller can decide"
        );
        // Compressible candidate must be omitted because the prompt
        // budget is already exhausted by the non-compressible one.
        let omission = allocation
            .omission_for("tools")
            .expect("tools must be omitted under overflow");
        assert_eq!(
            omission.reason,
            AllocationOmissionReason::TotalBudgetExceeded
        );
    }

    #[test]
    fn allocate_prefers_higher_priority_segments_when_compressing() {
        // INVARIANT: when both candidates fit individually but only
        // one can be selected, the allocator sorts by retention rank
        // first. SystemRules (Critical, rank 0) must win over
        // SourceContext (Low, rank 3).
        let budget = budget_with(
            vec![
                seg(ContextSegmentKind::SystemRules, 1_000),
                seg(ContextSegmentKind::SourceContext, 1_000),
            ],
            10,
        );
        let allocator = ContextBudgetAllocator::new(budget);
        let allocation = allocator.allocate(vec![
            ContextBudgetCandidate::new("low_first", ContextSegmentKind::SourceContext, 10),
            ContextBudgetCandidate::new("high_second", ContextSegmentKind::SystemRules, 10),
        ]);
        assert_eq!(
            allocation.selected_ids(),
            vec!["high_second"],
            "Critical-priority segment must win even when ordered second in input"
        );
        let dropped = allocation
            .omission_for("low_first")
            .expect("low priority must be omitted");
        assert_eq!(
            dropped.reason,
            AllocationOmissionReason::TotalBudgetExceeded
        );
    }

    #[test]
    fn allocate_ties_are_broken_by_input_index() {
        // INVARIANT: equal-priority compressible candidates are
        // sorted stably — earlier input wins on ties. Without this
        // determinism, prompt assembly would be order-dependent on
        // HashMap iteration and audit reproduction would break.
        let budget = budget_with(vec![seg(ContextSegmentKind::ToolResults, 1_000)], 10);
        let allocator = ContextBudgetAllocator::new(budget);
        let allocation = allocator.allocate(vec![
            ContextBudgetCandidate::new("first", ContextSegmentKind::ToolResults, 10),
            ContextBudgetCandidate::new("second", ContextSegmentKind::ToolResults, 10),
        ]);
        assert_eq!(allocation.selected_ids(), vec!["first"]);
        assert_eq!(
            allocation
                .omission_for("second")
                .expect("second must be omitted")
                .reason,
            AllocationOmissionReason::TotalBudgetExceeded
        );
    }

    #[test]
    fn allocate_records_zero_overflow_when_under_budget() {
        let budget = budget_with(vec![seg(ContextSegmentKind::ToolResults, 1_000)], 1_000);
        let allocator = ContextBudgetAllocator::new(budget);
        let allocation = allocator.allocate(vec![ContextBudgetCandidate::new(
            "small",
            ContextSegmentKind::ToolResults,
            10,
        )]);
        assert_eq!(allocation.total_selected_tokens(), 10);
        assert_eq!(
            allocation.budget_exceeded_by(),
            0,
            "saturating_sub must clamp non-overflow allocations to zero"
        );
    }

    #[test]
    fn omission_for_returns_none_for_unknown_id() {
        let budget = budget_with(vec![seg(ContextSegmentKind::ToolResults, 100)], 100);
        let allocator = ContextBudgetAllocator::new(budget);
        let allocation = allocator.allocate(vec![ContextBudgetCandidate::new(
            "x",
            ContextSegmentKind::ToolResults,
            10,
        )]);
        assert!(allocation.omission_for("does-not-exist").is_none());
    }
}
