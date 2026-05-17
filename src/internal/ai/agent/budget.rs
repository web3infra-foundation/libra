//! Per-session and per-agent budget enforcement (OC-Phase 5 P5.3).
//!
//! Reads thresholds from
//! [`AgentsConfig.budget`](super::profile::AgentsConfig) and tracks
//! running totals across one `libra code` session. Each
//! `[BudgetTracker::accumulate]` call updates the in-memory counters;
//! `check_session()` and `check_agent(name)` return
//! `Err(BudgetExceededError)` once a configured cap is crossed.
//!
//! ## Why a separate module
//!
//! `src/internal/ai/agent_run/budget.rs` already defines `AgentBudget`
//! / `BudgetDimension` for the OC-Phase 3 sub-agent dispatcher, but
//! that module is gated behind the `subagent-scaffold` feature. P5.3
//! needs enforcement in the **default** build so a single-agent
//! `libra code` run respects `[code.budget]` even when
//! `code.multi_agent.enabled = false`. The two modules eventually
//! converge: the dispatcher will call into `BudgetTracker` once the
//! gate lifts in P3 GA. Until then, this module owns the cross-cutting
//! enforcement contract.
//!
//! ## Stable error code
//!
//! Every threshold breach surfaces as
//! [`StableErrorCode::AgentBudgetExceeded`]
//! (`LBR-AGENT-001`) so CI scripts and the TUI can branch on the
//! identifier without parsing the human-readable message.
//!
//! ## Concurrency
//!
//! `BudgetTracker` is `!Sync` by design: the runtime serialises
//! tool-loop turn boundaries so concurrent mutation would itself be a
//! correctness bug. Callers needing shared access wrap the tracker in
//! a `tokio::sync::Mutex` at the session boundary.

use std::collections::BTreeMap;

use thiserror::Error;

use super::profile::config::{AgentsConfig, GoalBudgetConfig, PerAgentBudgetConfig};
use crate::{internal::ai::completion::CompletionUsageSummary, utils::error::StableErrorCode};

/// Which budget dimension fired. Mirrors the surface
/// [`super::super::agent_run::budget::BudgetDimension`] uses for
/// schema continuity — the doc's "concrete units owned by Step 1.11 /
/// CEX-16" rule applies here too.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BudgetAxis {
    /// Cost in USD (matches `[code.budget].max_session_cost_usd` and
    /// `[code.budget.per_agent.<name>].max_cost_usd`).
    Cost,
    /// Total tokens (prompt + completion + cached + reasoning).
    /// Matches `[code.budget].max_session_tokens`.
    Tokens,
    /// Tool / step count (matches
    /// `[code.budget.per_agent.<name>].max_steps`).
    Steps,
    /// Wall-clock minutes (matches the goal-loop dimension).
    WallClockMinutes,
}

impl BudgetAxis {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cost => "cost",
            Self::Tokens => "tokens",
            Self::Steps => "steps",
            Self::WallClockMinutes => "wall_clock_minutes",
        }
    }
}

/// Where the threshold lives. Distinguishes session-wide caps from
/// per-agent caps so the operator-facing error message can name the
/// right config key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BudgetScope {
    Session,
    Agent { name: String },
    Goal,
}

/// Reported as the actual measured value at the moment of breach.
/// Numeric type preserves the source unit so operators can correlate
/// with `[code.budget]` thresholds without a conversion table.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BudgetMeasurement {
    UsdMicros(u64),
    Tokens(u64),
    Steps(u64),
    WallClockMinutes(f64),
}

impl BudgetMeasurement {
    /// Best-effort projection to a USD-denominated threshold for
    /// the cost axis. Other axes return `None` so the caller can
    /// branch without inspecting the variant tag.
    pub fn as_threshold_usd(&self) -> Option<f64> {
        if let Self::UsdMicros(micros) = self {
            Some(*micros as f64 / 1_000_000.0)
        } else {
            None
        }
    }
}

/// Threshold breach surfaced to the dispatcher / tool_loop. Designed
/// to map onto a `CompletionError::ProviderError` or a CLI error with
/// `StableErrorCode::AgentBudgetExceeded` at the boundary.
#[derive(Clone, Debug, PartialEq, Error)]
pub struct BudgetExceededError {
    pub axis: BudgetAxis,
    pub scope: BudgetScope,
    /// Human-readable threshold (rendered with the source unit).
    pub threshold: String,
    /// Human-readable measured value at the moment of breach.
    pub actual: String,
}

impl BudgetExceededError {
    /// Stable error code mirroring the `LBR-AGENT-001` identifier.
    pub fn stable_code(&self) -> StableErrorCode {
        StableErrorCode::AgentBudgetExceeded
    }
}

impl std::fmt::Display for BudgetExceededError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let scope_label = match &self.scope {
            BudgetScope::Session => "session".to_string(),
            BudgetScope::Agent { name } => format!("agent '{name}'"),
            BudgetScope::Goal => "goal".to_string(),
        };
        write!(
            f,
            "{scope} budget exceeded on {axis}: actual {actual} >= configured cap {threshold} \
             (`{code}`)",
            scope = scope_label,
            axis = self.axis.as_str(),
            actual = self.actual,
            threshold = self.threshold,
            code = self.stable_code().as_str(),
        )
    }
}

/// Per-axis warning notification. Emitted exactly once per axis when
/// the running total first crosses the `warn_*` threshold; the caller
/// surfaces it through the TUI badge. Subsequent `accumulate` calls
/// do NOT re-emit because the operator has already been told.
#[derive(Clone, Debug, PartialEq)]
pub struct BudgetWarning {
    pub axis: BudgetAxis,
    pub scope: BudgetScope,
    pub threshold: String,
    pub actual: String,
}

/// Accumulator of one session's running totals across all agents.
/// Constructed fresh per `libra code` session; dropped at session
/// teardown.
#[derive(Clone, Debug, Default)]
pub struct BudgetTracker {
    session_total: RunningTotals,
    per_agent_totals: BTreeMap<String, RunningTotals>,
    warnings_emitted: WarningEmitTracker,
}

#[derive(Clone, Debug, Default)]
struct RunningTotals {
    cost_micro_usd: u64,
    total_tokens: u64,
    steps: u64,
    wall_clock_ms: u64,
}

/// Per-axis flags so a warning fires exactly once per scope.
/// Cleaner than a `HashSet<(scope, axis)>` because the scope key
/// would need `Hash + Eq` (not stable for `Agent { name }` derived
/// values vs `&str` queries).
///
/// Only the axes the current `drain_warnings` implementation emits
/// have flag fields; per-agent warnings (and the token / step
/// session warnings) are intentionally absent — when those surfaces
/// land, add the fields here in the same edit so the tracker stays
/// the single source of truth for "have we already told the
/// operator about this?".
#[derive(Clone, Debug, Default)]
struct WarningEmitTracker {
    session: AxisFlags,
    goal: AxisFlags,
}

#[derive(Clone, Debug, Default)]
struct AxisFlags {
    cost: bool,
    wall_clock: bool,
}

impl BudgetTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot of the session-wide running totals.
    pub fn session_cost_usd(&self) -> f64 {
        self.session_total.cost_micro_usd as f64 / 1_000_000.0
    }
    pub fn session_total_tokens(&self) -> u64 {
        self.session_total.total_tokens
    }
    pub fn session_wall_clock_ms(&self) -> u64 {
        self.session_total.wall_clock_ms
    }

    /// Snapshot of one agent's running totals.
    pub fn agent_steps(&self, agent: &str) -> u64 {
        self.per_agent_totals
            .get(agent)
            .map(|t| t.steps)
            .unwrap_or(0)
    }
    pub fn agent_cost_usd(&self, agent: &str) -> f64 {
        self.per_agent_totals
            .get(agent)
            .map(|t| t.cost_micro_usd as f64 / 1_000_000.0)
            .unwrap_or(0.0)
    }

    /// Fold one usage summary into the running totals. Optional
    /// `wall_clock_ms` is the per-turn elapsed time; pass `None` if
    /// the caller does not measure (e.g. fake provider tests).
    /// `agent_name` attribution mirrors `UsageContext.agent_name`:
    /// `None` → session totals only; `Some(name)` → both session and
    /// per-agent.
    pub fn accumulate(
        &mut self,
        usage: &CompletionUsageSummary,
        wall_clock_ms: Option<u64>,
        agent_name: Option<&str>,
    ) {
        let cost_micros = usage_cost_micro_usd(usage);
        let tokens = usage_total_tokens(usage);
        let elapsed = wall_clock_ms.unwrap_or(0);

        self.session_total.cost_micro_usd = self
            .session_total
            .cost_micro_usd
            .saturating_add(cost_micros);
        self.session_total.total_tokens = self.session_total.total_tokens.saturating_add(tokens);
        self.session_total.wall_clock_ms = self.session_total.wall_clock_ms.saturating_add(elapsed);

        if let Some(name) = agent_name {
            let entry = self.per_agent_totals.entry(name.to_string()).or_default();
            entry.cost_micro_usd = entry.cost_micro_usd.saturating_add(cost_micros);
            entry.total_tokens = entry.total_tokens.saturating_add(tokens);
            entry.wall_clock_ms = entry.wall_clock_ms.saturating_add(elapsed);
        }
    }

    /// Increment the per-agent step count by one. Step counters are
    /// turn-driven (one tool dispatch = one step), not usage-driven,
    /// so they live in their own accumulator.
    pub fn record_step(&mut self, agent_name: Option<&str>) {
        self.session_total.steps = self.session_total.steps.saturating_add(1);
        if let Some(name) = agent_name {
            let entry = self.per_agent_totals.entry(name.to_string()).or_default();
            entry.steps = entry.steps.saturating_add(1);
        }
    }

    /// Test session-level caps from `[code.budget]`. Returns
    /// `Ok(())` when under cap; `Err(BudgetExceededError)` once a cap
    /// fires. Idempotent: callers can poll on every turn boundary.
    pub fn check_session(&self, config: &AgentsConfig) -> Result<(), BudgetExceededError> {
        if let Some(max) = config.budget.max_session_cost_usd
            && self.session_cost_usd() >= max
        {
            return Err(BudgetExceededError {
                axis: BudgetAxis::Cost,
                scope: BudgetScope::Session,
                threshold: format!("${max:.4}"),
                actual: format!("${:.4}", self.session_cost_usd()),
            });
        }
        if let Some(max) = config.budget.max_session_tokens
            && self.session_total_tokens() >= max
        {
            return Err(BudgetExceededError {
                axis: BudgetAxis::Tokens,
                scope: BudgetScope::Session,
                threshold: max.to_string(),
                actual: self.session_total_tokens().to_string(),
            });
        }
        Ok(())
    }

    /// Test per-agent caps from `[code.budget.per_agent.<name>]`.
    /// Unknown agent name returns Ok (no cap configured).
    pub fn check_agent(
        &self,
        agent_name: &str,
        config: &AgentsConfig,
    ) -> Result<(), BudgetExceededError> {
        let Some(cap) = config.budget.per_agent.get(agent_name) else {
            return Ok(());
        };
        let totals = self
            .per_agent_totals
            .get(agent_name)
            .cloned()
            .unwrap_or_default();
        check_per_agent(agent_name, &totals, cap)
    }

    /// Test the goal-loop caps from `[code.budget.goal]`. Wall-clock
    /// minutes is computed from the session-wide elapsed time (Goal
    /// mode runs as a single contiguous loop, not a separate
    /// accumulator).
    pub fn check_goal(&self, config: &AgentsConfig) -> Result<(), BudgetExceededError> {
        check_goal_caps(&self.session_total, &config.budget.goal)
    }

    /// Compute warnings to surface to the TUI for axes that just
    /// crossed their `warn_*` threshold but haven't yet hit the
    /// `max_*` cap. Idempotent across calls; each axis fires once per
    /// scope. The in-memory `warnings_emitted` table is updated as a
    /// side effect, so back-to-back calls do not re-emit.
    pub fn drain_warnings(&mut self, config: &AgentsConfig) -> Vec<BudgetWarning> {
        let mut out = Vec::new();
        // Session-wide cost.
        if let Some(warn) = config.budget.warn_session_cost_usd
            && !self.warnings_emitted.session.cost
            && self.session_cost_usd() >= warn
        {
            self.warnings_emitted.session.cost = true;
            out.push(BudgetWarning {
                axis: BudgetAxis::Cost,
                scope: BudgetScope::Session,
                threshold: format!("${warn:.4}"),
                actual: format!("${:.4}", self.session_cost_usd()),
            });
        }
        // Goal-loop cost + wall clock.
        let goal = &config.budget.goal;
        if let Some(warn) = goal.warn_cost_usd
            && !self.warnings_emitted.goal.cost
            && self.session_cost_usd() >= warn
        {
            self.warnings_emitted.goal.cost = true;
            out.push(BudgetWarning {
                axis: BudgetAxis::Cost,
                scope: BudgetScope::Goal,
                threshold: format!("${warn:.4}"),
                actual: format!("${:.4}", self.session_cost_usd()),
            });
        }
        if let Some(warn_minutes) = goal.warn_wall_clock_minutes {
            let actual_minutes = self.session_total.wall_clock_ms as f64 / 60_000.0;
            if !self.warnings_emitted.goal.wall_clock && actual_minutes >= warn_minutes as f64 {
                self.warnings_emitted.goal.wall_clock = true;
                out.push(BudgetWarning {
                    axis: BudgetAxis::WallClockMinutes,
                    scope: BudgetScope::Goal,
                    threshold: format!("{warn_minutes}m"),
                    actual: format!("{actual_minutes:.2}m"),
                });
            }
        }
        out
    }
}

fn check_per_agent(
    name: &str,
    totals: &RunningTotals,
    cap: &PerAgentBudgetConfig,
) -> Result<(), BudgetExceededError> {
    if let Some(max_usd) = cap.max_cost_usd {
        let actual = totals.cost_micro_usd as f64 / 1_000_000.0;
        if actual >= max_usd {
            return Err(BudgetExceededError {
                axis: BudgetAxis::Cost,
                scope: BudgetScope::Agent {
                    name: name.to_string(),
                },
                threshold: format!("${max_usd:.4}"),
                actual: format!("${actual:.4}"),
            });
        }
    }
    if let Some(max_steps) = cap.max_steps
        && u64::from(max_steps) <= totals.steps
    {
        return Err(BudgetExceededError {
            axis: BudgetAxis::Steps,
            scope: BudgetScope::Agent {
                name: name.to_string(),
            },
            threshold: max_steps.to_string(),
            actual: totals.steps.to_string(),
        });
    }
    Ok(())
}

fn check_goal_caps(
    totals: &RunningTotals,
    cap: &GoalBudgetConfig,
) -> Result<(), BudgetExceededError> {
    if let Some(max_usd) = cap.max_cost_usd {
        let actual = totals.cost_micro_usd as f64 / 1_000_000.0;
        if actual >= max_usd {
            return Err(BudgetExceededError {
                axis: BudgetAxis::Cost,
                scope: BudgetScope::Goal,
                threshold: format!("${max_usd:.4}"),
                actual: format!("${actual:.4}"),
            });
        }
    }
    if let Some(max_minutes) = cap.max_wall_clock_minutes {
        let actual_minutes = totals.wall_clock_ms as f64 / 60_000.0;
        if actual_minutes >= max_minutes as f64 {
            return Err(BudgetExceededError {
                axis: BudgetAxis::WallClockMinutes,
                scope: BudgetScope::Goal,
                threshold: format!("{max_minutes}m"),
                actual: format!("{actual_minutes:.2}m"),
            });
        }
    }
    Ok(())
}

fn usage_cost_micro_usd(usage: &CompletionUsageSummary) -> u64 {
    let Some(cost) = usage.cost_usd else {
        return 0;
    };
    if !cost.is_finite() || cost <= 0.0 {
        return 0;
    }
    let micros = cost * 1_000_000.0;
    if micros > u64::MAX as f64 {
        u64::MAX
    } else {
        micros.round() as u64
    }
}

fn usage_total_tokens(usage: &CompletionUsageSummary) -> u64 {
    usage.total_tokens.unwrap_or_else(|| {
        usage
            .input_tokens
            .saturating_add(usage.output_tokens)
            .saturating_add(usage.reasoning_tokens.unwrap_or(0))
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::internal::ai::agent::profile::config::{
        AgentConfigEntry, BudgetConfig, GoalBudgetConfig, PerAgentBudgetConfig,
    };

    fn usage(input: u64, output: u64, cost_usd: Option<f64>) -> CompletionUsageSummary {
        CompletionUsageSummary {
            input_tokens: input,
            output_tokens: output,
            cached_tokens: None,
            reasoning_tokens: None,
            total_tokens: Some(input + output),
            cost_usd,
        }
    }

    fn config_with_session_caps(
        max_cost: Option<f64>,
        warn_cost: Option<f64>,
        max_tokens: Option<u64>,
    ) -> AgentsConfig {
        AgentsConfig {
            budget: BudgetConfig {
                max_session_cost_usd: max_cost,
                warn_session_cost_usd: warn_cost,
                max_session_tokens: max_tokens,
                ..BudgetConfig::default()
            },
            ..AgentsConfig::default()
        }
    }

    fn config_with_per_agent_cap(name: &str, cap: PerAgentBudgetConfig) -> AgentsConfig {
        // Per-agent budget references must point at a declared agent;
        // the validator enforces that in P5.1, so build a config whose
        // agents map at least lists the name.
        let mut agents = BTreeMap::new();
        agents.insert(
            name.to_string(),
            AgentConfigEntry {
                model: "openai/gpt-4o".to_string(),
                mode: "primary".to_string(),
                tools: Vec::new(),
                permission: BTreeMap::new(),
                steps: None,
            },
        );
        let mut per_agent = BTreeMap::new();
        per_agent.insert(name.to_string(), cap);
        AgentsConfig {
            agents,
            budget: BudgetConfig {
                per_agent,
                ..BudgetConfig::default()
            },
            ..AgentsConfig::default()
        }
    }

    #[test]
    fn accumulate_updates_session_and_per_agent_totals() {
        let mut t = BudgetTracker::new();
        t.accumulate(&usage(10, 5, Some(0.001)), Some(120), Some("planner"));
        assert!((t.session_cost_usd() - 0.001).abs() < 1e-9);
        assert_eq!(t.session_total_tokens(), 15);
        assert_eq!(t.session_wall_clock_ms(), 120);
        assert!((t.agent_cost_usd("planner") - 0.001).abs() < 1e-9);
    }

    #[test]
    fn accumulate_without_agent_only_updates_session() {
        let mut t = BudgetTracker::new();
        t.accumulate(&usage(7, 3, Some(0.0005)), None, None);
        assert!((t.session_cost_usd() - 0.0005).abs() < 1e-9);
        assert_eq!(t.agent_steps("planner"), 0);
        assert_eq!(t.agent_cost_usd("planner"), 0.0);
    }

    #[test]
    fn check_session_reports_cost_breach_with_actual_and_threshold() {
        let mut t = BudgetTracker::new();
        t.accumulate(&usage(0, 0, Some(0.5)), None, None);
        let cfg = config_with_session_caps(Some(0.4), None, None);
        let err = t
            .check_session(&cfg)
            .expect_err("over-cap session must fail");
        assert_eq!(err.axis, BudgetAxis::Cost);
        assert_eq!(err.scope, BudgetScope::Session);
        assert_eq!(err.stable_code(), StableErrorCode::AgentBudgetExceeded);
        let msg = err.to_string();
        assert!(msg.contains("$0.4000") && msg.contains("$0.5000"));
        assert!(msg.contains("LBR-AGENT-001"));
    }

    #[test]
    fn check_session_reports_token_breach() {
        let mut t = BudgetTracker::new();
        t.accumulate(&usage(800, 300, Some(0.0)), None, None);
        let cfg = config_with_session_caps(None, None, Some(1_000));
        let err = t.check_session(&cfg).expect_err("over-token cap must fail");
        assert_eq!(err.axis, BudgetAxis::Tokens);
    }

    #[test]
    fn check_session_passes_when_under_cap() {
        let mut t = BudgetTracker::new();
        t.accumulate(&usage(10, 10, Some(0.001)), None, None);
        let cfg = config_with_session_caps(Some(1.0), None, Some(1_000_000));
        t.check_session(&cfg).expect("under cap must pass");
    }

    #[test]
    fn check_agent_reports_per_agent_cost_breach() {
        let mut t = BudgetTracker::new();
        t.accumulate(&usage(0, 0, Some(2.0)), None, Some("explorer"));
        let cfg = config_with_per_agent_cap(
            "explorer",
            PerAgentBudgetConfig {
                max_cost_usd: Some(1.0),
                max_steps: None,
            },
        );
        let err = t
            .check_agent("explorer", &cfg)
            .expect_err("over-agent cap must fail");
        assert_eq!(err.axis, BudgetAxis::Cost);
        assert_eq!(
            err.scope,
            BudgetScope::Agent {
                name: "explorer".to_string(),
            }
        );
    }

    #[test]
    fn check_agent_reports_step_breach() {
        let mut t = BudgetTracker::new();
        for _ in 0..5 {
            t.record_step(Some("explorer"));
        }
        let cfg = config_with_per_agent_cap(
            "explorer",
            PerAgentBudgetConfig {
                max_cost_usd: None,
                max_steps: Some(3),
            },
        );
        let err = t
            .check_agent("explorer", &cfg)
            .expect_err("over-step cap must fail");
        assert_eq!(err.axis, BudgetAxis::Steps);
    }

    #[test]
    fn check_agent_unknown_name_passes() {
        let t = BudgetTracker::new();
        let cfg = config_with_per_agent_cap(
            "planner",
            PerAgentBudgetConfig {
                max_cost_usd: Some(0.5),
                max_steps: None,
            },
        );
        // No cap for "explorer" → no failure.
        t.check_agent("explorer", &cfg)
            .expect("unknown agent must pass");
    }

    #[test]
    fn check_goal_reports_wall_clock_breach() {
        let mut t = BudgetTracker::new();
        // 11 minutes elapsed.
        t.accumulate(&usage(0, 0, Some(0.0)), Some(11 * 60_000), None);
        let cfg = AgentsConfig {
            budget: BudgetConfig {
                goal: GoalBudgetConfig {
                    max_wall_clock_minutes: Some(10),
                    ..GoalBudgetConfig::default()
                },
                ..BudgetConfig::default()
            },
            ..AgentsConfig::default()
        };
        let err = t.check_goal(&cfg).expect_err("over wall-clock must fail");
        assert_eq!(err.axis, BudgetAxis::WallClockMinutes);
        assert_eq!(err.scope, BudgetScope::Goal);
    }

    #[test]
    fn drain_warnings_fires_once_per_axis_per_scope() {
        let mut t = BudgetTracker::new();
        t.accumulate(&usage(0, 0, Some(2.5)), None, None);
        let cfg = config_with_session_caps(Some(5.0), Some(2.0), None);

        let first = t.drain_warnings(&cfg);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].axis, BudgetAxis::Cost);
        assert_eq!(first[0].scope, BudgetScope::Session);

        // Add more usage that pushes higher; the warning must not
        // re-fire because we already told the operator.
        t.accumulate(&usage(0, 0, Some(0.5)), None, None);
        let second = t.drain_warnings(&cfg);
        assert!(
            second.is_empty(),
            "warning must fire once per axis-per-scope, got {second:?}"
        );
    }

    #[test]
    fn check_session_disabled_caps_always_pass() {
        let mut t = BudgetTracker::new();
        // Over $1B cost; no cap configured.
        t.accumulate(&usage(0, 0, Some(1_000_000_000.0)), None, None);
        let cfg = AgentsConfig::default();
        t.check_session(&cfg)
            .expect("no cap configured must pass regardless of running totals");
    }

    #[test]
    fn invalid_or_zero_cost_does_not_advance_session_total() {
        let mut t = BudgetTracker::new();
        t.accumulate(&usage(10, 5, Some(f64::NAN)), None, None);
        t.accumulate(&usage(10, 5, Some(-1.0)), None, None);
        t.accumulate(&usage(10, 5, None), None, None);
        // Tokens still accrue (they're valid).
        assert_eq!(t.session_total_tokens(), 45);
        assert_eq!(t.session_cost_usd(), 0.0);
    }

    #[test]
    fn budget_measurement_threshold_usd_helper() {
        assert_eq!(
            BudgetMeasurement::UsdMicros(2_500_000).as_threshold_usd(),
            Some(2.5)
        );
        assert!(BudgetMeasurement::Tokens(100).as_threshold_usd().is_none());
    }

    #[test]
    fn budget_exceeded_error_display_pins_each_scope_and_axis() {
        assert_eq!(
            BudgetExceededError {
                axis: BudgetAxis::Cost,
                scope: BudgetScope::Session,
                threshold: "$5.00".to_string(),
                actual: "$5.12".to_string(),
            }
            .to_string(),
            "session budget exceeded on cost: actual $5.12 >= configured cap $5.00 \
             (`LBR-AGENT-001`)",
        );
        assert_eq!(
            BudgetExceededError {
                axis: BudgetAxis::Tokens,
                scope: BudgetScope::Agent {
                    name: "reviewer".to_string(),
                },
                threshold: "100000".to_string(),
                actual: "100050".to_string(),
            }
            .to_string(),
            "agent 'reviewer' budget exceeded on tokens: actual 100050 >= configured cap \
             100000 (`LBR-AGENT-001`)",
        );
        assert_eq!(
            BudgetExceededError {
                axis: BudgetAxis::Steps,
                scope: BudgetScope::Agent {
                    name: "planner".to_string(),
                },
                threshold: "20".to_string(),
                actual: "21".to_string(),
            }
            .to_string(),
            "agent 'planner' budget exceeded on steps: actual 21 >= configured cap 20 \
             (`LBR-AGENT-001`)",
        );
        assert_eq!(
            BudgetExceededError {
                axis: BudgetAxis::WallClockMinutes,
                scope: BudgetScope::Goal,
                threshold: "30".to_string(),
                actual: "31".to_string(),
            }
            .to_string(),
            "goal budget exceeded on wall_clock_minutes: actual 31 >= configured cap 30 \
             (`LBR-AGENT-001`)",
        );
    }
}
