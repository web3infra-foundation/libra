//! `AgentBudget[S]` snapshot.
//!
//! Five enforcement dimensions per CEX-S2-12 (3) and the `RunUsage[E]` row of
//! the Step 2 core-objects table. Concrete field names mirror the
//! `agent_usage_stats` schema owned by Step 1.11 / CEX-16.

#![cfg(feature = "subagent-scaffold")]

use serde::{Deserialize, Serialize};

/// One of the five enforcement dimensions. Used as the `dimension` field on
/// `AgentRunEvent::BudgetExceeded`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
