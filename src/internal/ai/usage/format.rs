//! Usage statistics formatting for human-readable display and reporting.
//!
//! 用于人类可读显示和报告的使用统计格式化。

use super::query::UsageAggregate;

#[derive(Clone, Debug, PartialEq)]
pub struct UsageDisplaySnapshot {
    pub provider: String,
    pub model: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub wall_clock_ms: u64,
    pub cost_usd: Option<f64>,
}

pub fn format_usage_detail_panel(
    snapshot: &UsageDisplaySnapshot,
    grouping_label: &str,
    aggregates: &[UsageAggregate],
) -> String {
    let mut lines = vec![
        "Usage Details".to_string(),
        format!("Current: {}", format_usage_badge(snapshot)),
        format!("Grouped by: {grouping_label}"),
    ];

    if aggregates.is_empty() {
        lines.push("No SQLite usage rows found for this session yet.".to_string());
        return lines.join("\n");
    }

    lines.push("Rows:".to_string());
    for aggregate in aggregates {
        lines.push(format!(
            "{} | req {} | tok {} | tools {} | wall {:.1}s | cost {} | failed {}",
            usage_aggregate_label(aggregate),
            aggregate.request_count,
            compact_count(aggregate.total_tokens),
            aggregate.tool_call_count,
            aggregate.wall_clock_ms as f64 / 1000.0,
            format_aggregate_cost(aggregate),
            aggregate.failed_count,
        ));
    }
    lines.join("\n")
}

pub fn format_usage_badge(snapshot: &UsageDisplaySnapshot) -> String {
    let total_tokens = snapshot
        .prompt_tokens
        .saturating_add(snapshot.completion_tokens);
    let mut parts = vec![
        format!("{}/{}", snapshot.provider, snapshot.model),
        format!("{} tok", compact_count(total_tokens)),
        format!("{:.1}s", snapshot.wall_clock_ms as f64 / 1000.0),
    ];
    if let Some(cost) = snapshot.cost_usd {
        parts.push(format!("${cost:.4}"));
    }
    parts.join(" · ")
}

fn usage_aggregate_label(aggregate: &UsageAggregate) -> String {
    match (
        aggregate.agent_name.as_deref(),
        aggregate.provider.as_str(),
        aggregate.model.as_str(),
    ) {
        (Some(agent), "", "") => agent.to_string(),
        (Some(agent), provider, model) => format!("{agent} {provider}/{model}"),
        (None, provider, model) => format!("{provider}/{model}"),
    }
}

fn format_aggregate_cost(aggregate: &UsageAggregate) -> String {
    if let Some(cost) = aggregate.cost_usd {
        return format!("${cost:.4}");
    }
    if let Some(micros) = aggregate.cost_estimate_micro_dollars {
        return format!("~${:.4}", micros as f64 / 1_000_000.0);
    }
    "-".to_string()
}

fn compact_count(value: u64) -> String {
    if value >= 1_000_000 {
        format!("{:.1}m", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1}k", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_aggregate() -> UsageAggregate {
        UsageAggregate {
            agent_name: None,
            provider: "openai".to_string(),
            model: "gpt-4".to_string(),
            request_count: 0,
            prompt_tokens: 0,
            completion_tokens: 0,
            cached_tokens: 0,
            reasoning_tokens: 0,
            total_tokens: 0,
            tool_call_count: 0,
            wall_clock_ms: 0,
            cost_usd: None,
            cost_estimate_micro_dollars: None,
            failed_count: 0,
        }
    }

    /// `compact_count` boundary table:
    /// - `0` / `1` / `999` → plain integer string
    /// - `1_000..1_000_000` → `<X>.<Y>k`
    /// - `>= 1_000_000` → `<X>.<Y>m`
    ///
    /// Pin so a future "comma-separated" or SI-prefix refactor breaks
    /// this test instead of silently changing the TUI rendering.
    #[test]
    fn compact_count_boundary_table() {
        assert_eq!(compact_count(0), "0");
        assert_eq!(compact_count(1), "1");
        assert_eq!(compact_count(999), "999");
        assert_eq!(compact_count(1_000), "1.0k");
        assert_eq!(compact_count(1_500), "1.5k");
        assert_eq!(compact_count(999_999), "1000.0k");
        assert_eq!(compact_count(1_000_000), "1.0m");
        assert_eq!(compact_count(1_500_000), "1.5m");
    }

    /// `format_usage_badge` always emits `provider/model`, total tokens,
    /// and wall-clock seconds. Cost is appended only when `Some`.
    #[test]
    fn format_usage_badge_omits_cost_when_none() {
        let snapshot = UsageDisplaySnapshot {
            provider: "openai".to_string(),
            model: "gpt-4".to_string(),
            prompt_tokens: 500,
            completion_tokens: 500,
            wall_clock_ms: 1_500,
            cost_usd: None,
        };
        let badge = format_usage_badge(&snapshot);
        assert!(badge.contains("openai/gpt-4"));
        assert!(badge.contains("1.0k tok"));
        assert!(badge.contains("1.5s"));
        assert!(
            !badge.contains('$'),
            "no cost should appear when cost_usd is None; got {badge}",
        );
    }

    /// Cost branch: `Some(cost)` must render as `${cost:.4}` (4
    /// decimal places).
    #[test]
    fn format_usage_badge_renders_cost_with_four_decimals() {
        let snapshot = UsageDisplaySnapshot {
            provider: "openai".to_string(),
            model: "gpt-4".to_string(),
            prompt_tokens: 0,
            completion_tokens: 0,
            wall_clock_ms: 0,
            cost_usd: Some(0.012345),
        };
        let badge = format_usage_badge(&snapshot);
        assert!(badge.contains("$0.0123"), "got {badge}");
    }

    /// `format_usage_badge` uses dotted-middle (` · `) joiner so the
    /// monospace TUI cell parses cleanly. Pin the joiner so a future
    /// table-style refactor breaks this test loudly.
    #[test]
    fn format_usage_badge_uses_dotted_joiner() {
        let snapshot = UsageDisplaySnapshot {
            provider: "p".to_string(),
            model: "m".to_string(),
            prompt_tokens: 0,
            completion_tokens: 0,
            wall_clock_ms: 0,
            cost_usd: None,
        };
        assert_eq!(format_usage_badge(&snapshot), "p/m · 0 tok · 0.0s");
    }

    /// `format_aggregate_cost`:
    /// - `cost_usd = Some(v)` → `$v.4`
    /// - `cost_usd = None` + `estimate = Some(micros)` → `~${v.4}`
    /// - both None → `"-"`
    #[test]
    fn format_aggregate_cost_real_estimate_and_missing_branches() {
        let mut agg = empty_aggregate();

        agg.cost_usd = Some(1.2345);
        agg.cost_estimate_micro_dollars = None;
        assert_eq!(format_aggregate_cost(&agg), "$1.2345");

        agg.cost_usd = None;
        agg.cost_estimate_micro_dollars = Some(1_234_500);
        assert_eq!(format_aggregate_cost(&agg), "~$1.2345");

        agg.cost_usd = None;
        agg.cost_estimate_micro_dollars = None;
        assert_eq!(format_aggregate_cost(&agg), "-");

        // Real cost takes priority over estimate even if both are Some
        // (matches the documented "real overrides estimate" rule).
        agg.cost_usd = Some(2.0);
        agg.cost_estimate_micro_dollars = Some(5_000_000);
        assert_eq!(format_aggregate_cost(&agg), "$2.0000");
    }

    /// `usage_aggregate_label` cases:
    /// - `Some(agent)` + provider/model both empty → bare agent name
    /// - `Some(agent)` + provider/model populated → `agent provider/model`
    /// - `None` → `provider/model`
    #[test]
    fn usage_aggregate_label_three_branches() {
        let mut agg = empty_aggregate();

        // None branch: provider/model only.
        agg.agent_name = None;
        assert_eq!(usage_aggregate_label(&agg), "openai/gpt-4");

        // Some + provider/model populated → combined.
        agg.agent_name = Some("coder".to_string());
        assert_eq!(usage_aggregate_label(&agg), "coder openai/gpt-4");

        // Some + provider/model empty → bare agent name.
        agg.agent_name = Some("coder".to_string());
        agg.provider = String::new();
        agg.model = String::new();
        assert_eq!(usage_aggregate_label(&agg), "coder");
    }

    /// `format_usage_detail_panel` empty path: surfaces the
    /// "No SQLite usage rows…" placeholder so operators understand
    /// the pane is empty by-design (not broken).
    #[test]
    fn format_usage_detail_panel_renders_empty_placeholder() {
        let snapshot = UsageDisplaySnapshot {
            provider: "p".to_string(),
            model: "m".to_string(),
            prompt_tokens: 0,
            completion_tokens: 0,
            wall_clock_ms: 0,
            cost_usd: None,
        };
        let out = format_usage_detail_panel(&snapshot, "provider/model", &[]);
        assert!(out.starts_with("Usage Details"));
        assert!(out.contains("No SQLite usage rows found"));
        assert!(
            !out.contains("Rows:"),
            "empty panel must NOT render the Rows: header; got\n{out}",
        );
    }

    /// `format_usage_detail_panel` non-empty path: renders the row
    /// section with one line per aggregate, including the canonical
    /// `req` / `tok` / `tools` / `wall` / `cost` / `failed` columns.
    #[test]
    fn format_usage_detail_panel_renders_aggregate_rows() {
        let snapshot = UsageDisplaySnapshot {
            provider: "p".to_string(),
            model: "m".to_string(),
            prompt_tokens: 100,
            completion_tokens: 50,
            wall_clock_ms: 200,
            cost_usd: Some(0.01),
        };
        let mut agg = empty_aggregate();
        agg.request_count = 3;
        agg.total_tokens = 1_500;
        agg.tool_call_count = 7;
        agg.wall_clock_ms = 2_000;
        agg.cost_usd = Some(0.05);
        agg.failed_count = 1;

        let out = format_usage_detail_panel(&snapshot, "provider/model", &[agg]);
        assert!(out.contains("Usage Details"));
        assert!(out.contains("Current: "));
        assert!(out.contains("Grouped by: provider/model"));
        assert!(out.contains("Rows:"));
        // Per-row canonical columns.
        assert!(out.contains("req 3"));
        assert!(out.contains("tok 1.5k"));
        assert!(out.contains("tools 7"));
        assert!(out.contains("wall 2.0s"));
        assert!(out.contains("$0.0500"));
        assert!(out.contains("failed 1"));
    }
}
