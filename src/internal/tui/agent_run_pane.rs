//! `/agents` pane projection (CEX-S2-16, Step 2.6).
//!
//! Pure, side-effect-free rendering of the sub-agent run pane. The pane is a
//! projection of the persisted [`AgentRun`] snapshots plus their per-run
//! [`RunUsage`] (and, optionally, Source-Pool call counts) — it holds no hidden
//! in-memory state, so a pane rebuilt after a cache wipe or process restart
//! renders byte-for-byte identically (CEX-S2-16 验收 (5)). The caller supplies
//! the persisted records via closures; this module only orders and formats them.

use crate::internal::ai::agent_run::{AgentRun, AgentRunStatus, RunUsage};

/// Placeholder shown when no sub-agent run has been persisted yet.
const EMPTY_PLACEHOLDER: &str = "No sub-agent runs recorded yet.";

/// Render the agent-run pane with each run's persisted token/cost usage.
///
/// `usage_for` returns the persisted [`RunUsage`] for a run, or `None` when the
/// run never recorded usage (e.g. an in-flight run that has not closed a
/// provider call). See [`format_agent_run_pane_with_usage_and_sources`] for the
/// variant that also renders the per-run Source-Pool call count.
pub fn format_agent_run_pane_with_usage(
    runs: &[AgentRun],
    usage_for: impl Fn(&AgentRun) -> Option<RunUsage>,
) -> String {
    format_pane(runs, usage_for, |_| None, false)
}

/// Render the agent-run pane with both per-run usage and the Source-Pool call
/// count (`src` column) joined from `source_call_log` by `agent_run_id`.
///
/// `sources_for` returns the number of source calls attributed to a run, or
/// `None` when none were recorded.
pub fn format_agent_run_pane_with_usage_and_sources(
    runs: &[AgentRun],
    usage_for: impl Fn(&AgentRun) -> Option<RunUsage>,
    sources_for: impl Fn(&AgentRun) -> Option<i64>,
) -> String {
    format_pane(runs, usage_for, sources_for, true)
}

/// Shared renderer. `include_sources` toggles the `src` column; when `false`
/// the `sources_for` closure is never consulted.
fn format_pane(
    runs: &[AgentRun],
    usage_for: impl Fn(&AgentRun) -> Option<RunUsage>,
    sources_for: impl Fn(&AgentRun) -> Option<i64>,
    include_sources: bool,
) -> String {
    if runs.is_empty() {
        return EMPTY_PLACEHOLDER.to_string();
    }

    // Deterministic order independent of the on-disk read order so the pane
    // rebuilds byte-identically: in-flight runs first (so live work is at the
    // top), then terminal runs, each group ordered by run id.
    let mut ordered: Vec<&AgentRun> = runs.iter().collect();
    ordered.sort_by(|a, b| {
        a.status
            .is_terminal()
            .cmp(&b.status.is_terminal())
            .then_with(|| a.id.0.cmp(&b.id.0))
    });

    let mut out = String::from("Agent runs:\n");
    if include_sources {
        out.push_str(&format!(
            "  {:<36}  {:<10}  {:>8}  {:>10}  {:>5}  {}\n",
            "run", "status", "tokens", "cost", "src", "activity",
        ));
    } else {
        out.push_str(&format!(
            "  {:<36}  {:<10}  {:>8}  {:>10}  {}\n",
            "run", "status", "tokens", "cost", "activity",
        ));
    }

    for run in ordered {
        let usage = usage_for(run);
        let tokens = usage
            .map(|u| u.total_tokens().to_string())
            .unwrap_or_else(|| "-".to_string());
        let cost = usage
            .map(|u| format_cost(u.cost_estimate_micro_dollars))
            .unwrap_or_else(|| "-".to_string());
        // A terminal run shows no live activity; an in-flight run is "active".
        let activity = if run.status.is_terminal() {
            "-"
        } else {
            "active"
        };

        if include_sources {
            let src = sources_for(run)
                .map(|count| count.to_string())
                .unwrap_or_else(|| "-".to_string());
            out.push_str(&format!(
                "  {:<36}  {:<10}  {:>8}  {:>10}  {:>5}  {}\n",
                run.id.0,
                status_label(run.status),
                tokens,
                cost,
                src,
                activity,
            ));
        } else {
            out.push_str(&format!(
                "  {:<36}  {:<10}  {:>8}  {:>10}  {}\n",
                run.id.0,
                status_label(run.status),
                tokens,
                cost,
                activity,
            ));
        }
    }

    out
}

/// Stable lowercase label for a run's lifecycle status (matches the persisted
/// snake_case wire tag).
fn status_label(status: AgentRunStatus) -> &'static str {
    match status {
        AgentRunStatus::Queued => "queued",
        AgentRunStatus::Running => "running",
        AgentRunStatus::Blocked => "blocked",
        AgentRunStatus::Completed => "completed",
        AgentRunStatus::Failed => "failed",
    }
}

/// Format a micro-dollar cost estimate as a fixed 4-decimal dollar string
/// (e.g. `1_500` micro-dollars → `"$0.0015"`). Integer arithmetic keeps the
/// rendering exact and deterministic — no float rounding.
fn format_cost(micro_dollars: u64) -> String {
    format!(
        "${}.{:04}",
        micro_dollars / 1_000_000,
        (micro_dollars % 1_000_000) / 100,
    )
}
