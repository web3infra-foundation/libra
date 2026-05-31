//! `AgentBudget[S]` snapshot.
//!
//! `AgentBudget[S]` 快照。
//!
//! Five enforcement dimensions per CEX-S2-12 (3) and the `RunUsage[E]` row of
//! the Step 2 core-objects table. Concrete field names mirror the
//! `agent_usage_stats` schema owned by Step 1.11 / CEX-16.

use serde::{Deserialize, Serialize};

use super::event::RunUsage;

/// One of the five enforcement dimensions. Used as the `dimension` field on
/// `AgentRunEvent::BudgetExceeded`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum BudgetDimension {
    Token,
    ToolCall,
    WallClock,
    SourceCall,
    Cost,
}

/// Per-`AgentRun` budget. All fields optional; absent = no enforcement on
/// that dimension. Concrete units owned by Step 1.11 / CEX-16.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentBudget {
    /// Total token budget (prompt + completion + cached + reasoning).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,

    /// Total tool-call count budget across the run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tool_calls: Option<u32>,

    /// Wall-clock budget in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_wall_clock_ms: Option<u64>,

    /// Source Pool call count budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_source_calls: Option<u32>,

    /// Cost budget in micro-dollars (unit shared with
    /// `agent_usage_stats.cost_estimate_micro_dollars`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cost_micro_dollars: Option<u64>,
}

impl AgentBudget {
    /// Which budget dimensions a run has **exceeded**, given its current
    /// [`RunUsage`] plus the Source-Pool call count (which `RunUsage`
    /// does not track — Source Pool accounting is owned by Step 1.10, so
    /// the caller supplies it explicitly).
    ///
    /// Returned in [`BudgetDimension`] declaration order (`Token`,
    /// `ToolCall`, `WallClock`, `SourceCall`, `Cost`) so the runtime can
    /// emit one `AgentRunEvent::BudgetExceeded` per breached dimension
    /// deterministically.
    ///
    /// Semantics:
    /// - A dimension whose limit is `None` is **unenforced** and never
    ///   reported (absent = no budget on that dimension).
    /// - "Exceeded" means usage is **strictly greater** than the limit
    ///   — landing exactly on the cap is within budget, surpassing it by
    ///   one is over. (`max_tokens = 1000` permits a 1000-token run and
    ///   flags a 1001-token run.)
    /// - The token dimension compares against
    ///   [`RunUsage::total_tokens`] (prompt + completion + cached +
    ///   reasoning), matching the `max_tokens` doc.
    pub fn exceeded_dimensions(
        &self,
        usage: &RunUsage,
        source_call_count: u32,
    ) -> Vec<BudgetDimension> {
        let mut exceeded = Vec::new();
        if let Some(limit) = self.max_tokens
            && usage.total_tokens() > limit
        {
            exceeded.push(BudgetDimension::Token);
        }
        if let Some(limit) = self.max_tool_calls
            && usage.tool_call_count > limit
        {
            exceeded.push(BudgetDimension::ToolCall);
        }
        if let Some(limit) = self.max_wall_clock_ms
            && usage.wall_clock_ms > limit
        {
            exceeded.push(BudgetDimension::WallClock);
        }
        if let Some(limit) = self.max_source_calls
            && source_call_count > limit
        {
            exceeded.push(BudgetDimension::SourceCall);
        }
        if let Some(limit) = self.max_cost_micro_dollars
            && usage.cost_estimate_micro_dollars > limit
        {
            exceeded.push(BudgetDimension::Cost);
        }
        exceeded
    }

    /// `true` when any enforced dimension has been exceeded. Convenience
    /// wrapper over [`exceeded_dimensions`](Self::exceeded_dimensions)
    /// for callers that only need a go / no-go gate.
    pub fn is_exceeded(&self, usage: &RunUsage, source_call_count: u32) -> bool {
        !self
            .exceeded_dimensions(usage, source_call_count)
            .is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The all-`None` default budget enforces nothing — no dimension is
    /// ever reported exceeded regardless of usage. Pins the
    /// "absent = unenforced" contract.
    #[test]
    fn default_budget_enforces_nothing() {
        let budget = AgentBudget::default();
        let heavy = RunUsage {
            prompt_tokens: u64::MAX,
            completion_tokens: 0,
            cached_tokens: 0,
            reasoning_tokens: 0,
            wall_clock_ms: u64::MAX,
            provider_latency_ms: 0,
            cost_estimate_micro_dollars: u64::MAX,
            tool_call_count: u32::MAX,
        };
        assert!(budget.exceeded_dimensions(&heavy, u32::MAX).is_empty());
        assert!(!budget.is_exceeded(&heavy, u32::MAX));
    }

    /// Exceeding is strictly-greater-than: usage exactly at the limit is
    /// within budget; limit + 1 is over. Checked on the token dimension.
    #[test]
    fn token_budget_boundary_is_strictly_greater() {
        let budget = AgentBudget {
            max_tokens: Some(1_000),
            ..AgentBudget::default()
        };
        let at_limit = RunUsage {
            prompt_tokens: 600,
            completion_tokens: 400, // total 1000 == limit
            ..RunUsage::default()
        };
        assert!(
            budget.exceeded_dimensions(&at_limit, 0).is_empty(),
            "exactly at the token limit must be within budget",
        );

        let over = RunUsage {
            prompt_tokens: 600,
            completion_tokens: 401, // total 1001 > limit
            ..RunUsage::default()
        };
        assert_eq!(
            budget.exceeded_dimensions(&over, 0),
            vec![BudgetDimension::Token],
        );
    }

    /// Each dimension is checked independently against its own limit,
    /// and the Source-Pool count comes from the explicit parameter (not
    /// `RunUsage`, which has no source-call field).
    #[test]
    fn each_dimension_checked_independently() {
        let budget = AgentBudget {
            max_tokens: Some(100),
            max_tool_calls: Some(5),
            max_wall_clock_ms: Some(1_000),
            max_source_calls: Some(2),
            max_cost_micro_dollars: Some(500),
        };

        let only_tool_calls_over = RunUsage {
            prompt_tokens: 50,
            tool_call_count: 6, // over the 5 limit
            wall_clock_ms: 900,
            cost_estimate_micro_dollars: 100,
            ..RunUsage::default()
        };
        assert_eq!(
            budget.exceeded_dimensions(&only_tool_calls_over, 1),
            vec![BudgetDimension::ToolCall],
        );

        // Source-call count comes from the param, not RunUsage.
        let usage_within = RunUsage {
            prompt_tokens: 10,
            tool_call_count: 1,
            wall_clock_ms: 10,
            cost_estimate_micro_dollars: 10,
            ..RunUsage::default()
        };
        assert_eq!(
            budget.exceeded_dimensions(&usage_within, 3), // 3 > 2 source limit
            vec![BudgetDimension::SourceCall],
        );
    }

    /// Multiple simultaneous breaches are reported in `BudgetDimension`
    /// declaration order so the runtime emits a deterministic event
    /// sequence.
    #[test]
    fn multiple_breaches_reported_in_declaration_order() {
        let budget = AgentBudget {
            max_tokens: Some(10),
            max_tool_calls: Some(1),
            max_wall_clock_ms: Some(10),
            max_source_calls: Some(1),
            max_cost_micro_dollars: Some(10),
        };
        let everything_over = RunUsage {
            prompt_tokens: 100,
            completion_tokens: 0,
            cached_tokens: 0,
            reasoning_tokens: 0,
            wall_clock_ms: 100,
            provider_latency_ms: 0,
            cost_estimate_micro_dollars: 100,
            tool_call_count: 100,
        };
        assert_eq!(
            budget.exceeded_dimensions(&everything_over, 100),
            vec![
                BudgetDimension::Token,
                BudgetDimension::ToolCall,
                BudgetDimension::WallClock,
                BudgetDimension::SourceCall,
                BudgetDimension::Cost,
            ],
        );
        assert!(budget.is_exceeded(&everything_over, 100));
    }
}
