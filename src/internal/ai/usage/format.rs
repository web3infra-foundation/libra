#[derive(Clone, Debug, PartialEq)]
pub struct UsageDisplaySnapshot {
    pub provider: String,
    pub model: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub wall_clock_ms: u64,
    pub cost_usd: Option<f64>,
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

fn compact_count(value: u64) -> String {
    if value >= 1_000_000 {
        format!("{:.1}m", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1}k", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}
