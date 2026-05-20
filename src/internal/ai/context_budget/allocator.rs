//! Deterministic allocation of concrete context candidates against a budget.

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
