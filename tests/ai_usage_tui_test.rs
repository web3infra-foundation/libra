//! CEX-16 usage display formatting tests.

use libra::internal::ai::usage::{UsageDisplaySnapshot, format_usage_badge};

#[test]
fn usage_badge_shows_model_tokens_and_wall_clock() {
    let snapshot = UsageDisplaySnapshot {
        provider: "openai".to_string(),
        model: "gpt-test".to_string(),
        prompt_tokens: 1200,
        completion_tokens: 345,
        wall_clock_ms: 12_345,
        cost_usd: Some(0.42),
    };

    let badge = format_usage_badge(&snapshot);

    assert!(badge.contains("openai/gpt-test"));
    assert!(badge.contains("1.5k tok"));
    assert!(badge.contains("12.3s"));
    assert!(badge.contains("$0.4200"));
}
