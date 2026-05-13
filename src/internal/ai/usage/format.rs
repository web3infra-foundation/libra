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
