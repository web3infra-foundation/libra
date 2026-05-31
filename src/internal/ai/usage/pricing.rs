//! Provider pricing tables for computing token cost estimates.
//!
//! 用于计算令牌成本估算的提供商定价表。

use std::{collections::HashMap, fmt};

use serde::Deserialize;

use crate::internal::ai::{completion::CompletionUsageSummary, providers::capability};

const TOKENS_PER_MILLION: u128 = 1_000_000;
const MICRO_DOLLARS_PER_USD: f64 = 1_000_000.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UsagePrice {
    pub input_micro_dollars_per_mtok: u64,
    pub output_micro_dollars_per_mtok: u64,
    pub cached_micro_dollars_per_mtok: Option<u64>,
    pub reasoning_micro_dollars_per_mtok: Option<u64>,
}

impl UsagePrice {
    pub fn new(input_micro_dollars_per_mtok: u64, output_micro_dollars_per_mtok: u64) -> Self {
        Self {
            input_micro_dollars_per_mtok,
            output_micro_dollars_per_mtok,
            cached_micro_dollars_per_mtok: None,
            reasoning_micro_dollars_per_mtok: None,
        }
    }

    pub fn with_cached_micro_dollars_per_mtok(mut self, price: u64) -> Self {
        self.cached_micro_dollars_per_mtok = Some(price);
        self
    }

    pub fn with_reasoning_micro_dollars_per_mtok(mut self, price: u64) -> Self {
        self.reasoning_micro_dollars_per_mtok = Some(price);
        self
    }

    fn estimate_micro_dollars(self, summary: &CompletionUsageSummary) -> Option<i64> {
        let cached_tokens = summary.cached_tokens.unwrap_or(0).min(summary.input_tokens);
        let uncached_input_tokens = summary.input_tokens.saturating_sub(cached_tokens);
        let reasoning_tokens = summary.reasoning_tokens.unwrap_or(0);

        let mut micro_dollars = 0_u128;
        micro_dollars = micro_dollars.saturating_add(price_tokens(
            uncached_input_tokens,
            self.input_micro_dollars_per_mtok,
        ));
        let cached_price = self
            .cached_micro_dollars_per_mtok
            .unwrap_or(self.input_micro_dollars_per_mtok);
        micro_dollars = micro_dollars.saturating_add(price_tokens(cached_tokens, cached_price));
        micro_dollars = micro_dollars.saturating_add(price_tokens(
            summary.output_tokens,
            self.output_micro_dollars_per_mtok,
        ));
        let reasoning_price = self
            .reasoning_micro_dollars_per_mtok
            .unwrap_or(self.output_micro_dollars_per_mtok);
        micro_dollars =
            micro_dollars.saturating_add(price_tokens(reasoning_tokens, reasoning_price));

        if micro_dollars > i64::MAX as u128 {
            None
        } else {
            Some(micro_dollars as i64)
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct UsagePriceTable {
    overrides: HashMap<(String, String), UsagePrice>,
}

impl UsagePriceTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_override(
        mut self,
        provider: impl Into<String>,
        model: impl Into<String>,
        price: UsagePrice,
    ) -> Self {
        self.overrides
            .insert((provider.into(), model.into()), price);
        self
    }

    pub fn from_project_config_toml(contents: &str) -> Result<Self, UsagePricingConfigError> {
        let config: UsageProjectConfig =
            toml::from_str(contents).map_err(UsagePricingConfigError::ParseToml)?;
        let mut table = Self::new();
        for (provider, models) in config.usage.pricing {
            for (model, price) in models {
                table = table.with_override(provider.clone(), model, price.try_into()?);
            }
        }
        Ok(table)
    }

    pub fn estimate_micro_dollars(
        &self,
        provider: &str,
        model: &str,
        summary: &CompletionUsageSummary,
    ) -> Option<i64> {
        self.overrides
            .get(&(provider.to_string(), model.to_string()))
            .copied()
            .or_else(|| capability_price(provider, model))
            .and_then(|price| price.estimate_micro_dollars(summary))
    }

    pub fn is_empty(&self) -> bool {
        self.overrides.is_empty()
    }
}

#[derive(Debug)]
pub enum UsagePricingConfigError {
    ParseToml(toml::de::Error),
    MissingField(&'static str),
}

impl fmt::Display for UsagePricingConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ParseToml(error) => write!(f, "failed to parse usage pricing config: {error}"),
            Self::MissingField(field) => write!(f, "usage pricing config is missing `{field}`"),
        }
    }
}

impl std::error::Error for UsagePricingConfigError {}

#[derive(Debug, Default, Deserialize)]
struct UsageProjectConfig {
    #[serde(default)]
    usage: UsageConfig,
}

#[derive(Debug, Default, Deserialize)]
struct UsageConfig {
    #[serde(default)]
    pricing: HashMap<String, HashMap<String, UsagePriceConfig>>,
}

#[derive(Debug, Deserialize)]
struct UsagePriceConfig {
    input_micro_dollars_per_mtok: Option<u64>,
    output_micro_dollars_per_mtok: Option<u64>,
    cached_micro_dollars_per_mtok: Option<u64>,
    reasoning_micro_dollars_per_mtok: Option<u64>,
}

impl TryFrom<UsagePriceConfig> for UsagePrice {
    type Error = UsagePricingConfigError;

    fn try_from(value: UsagePriceConfig) -> Result<Self, Self::Error> {
        let input =
            value
                .input_micro_dollars_per_mtok
                .ok_or(UsagePricingConfigError::MissingField(
                    "input_micro_dollars_per_mtok",
                ))?;
        let output =
            value
                .output_micro_dollars_per_mtok
                .ok_or(UsagePricingConfigError::MissingField(
                    "output_micro_dollars_per_mtok",
                ))?;
        let mut price = UsagePrice::new(input, output);
        if let Some(cached) = value.cached_micro_dollars_per_mtok {
            price = price.with_cached_micro_dollars_per_mtok(cached);
        }
        if let Some(reasoning) = value.reasoning_micro_dollars_per_mtok {
            price = price.with_reasoning_micro_dollars_per_mtok(reasoning);
        }
        Ok(price)
    }
}

fn capability_price(provider: &str, model: &str) -> Option<UsagePrice> {
    let cost = capability::lookup(provider, model)?.cost?;
    Some(UsagePrice::new(
        usd_per_mtok_to_micro_dollars(cost.input_per_million_tokens_usd)?,
        usd_per_mtok_to_micro_dollars(cost.output_per_million_tokens_usd)?,
    ))
}

fn usd_per_mtok_to_micro_dollars(value: f64) -> Option<u64> {
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    let micro_dollars = value * MICRO_DOLLARS_PER_USD;
    if micro_dollars > u64::MAX as f64 {
        None
    } else {
        Some(micro_dollars.round() as u64)
    }
}

fn price_tokens(tokens: u64, micro_dollars_per_mtok: u64) -> u128 {
    (u128::from(tokens) * u128::from(micro_dollars_per_mtok) + (TOKENS_PER_MILLION / 2))
        / TOKENS_PER_MILLION
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_config_toml_builds_price_overrides() {
        let table = UsagePriceTable::from_project_config_toml(
            r#"
            [usage.pricing.openai."gpt-test"]
            input_micro_dollars_per_mtok = 10
            output_micro_dollars_per_mtok = 20
            cached_micro_dollars_per_mtok = 2
            reasoning_micro_dollars_per_mtok = 30
            "#,
        )
        .expect("pricing config should parse");

        let estimate = table
            .estimate_micro_dollars(
                "openai",
                "gpt-test",
                &CompletionUsageSummary {
                    input_tokens: 2_000_000,
                    output_tokens: 1_000_000,
                    cached_tokens: Some(500_000),
                    reasoning_tokens: Some(1_000_000),
                    total_tokens: None,
                    cost_usd: None,
                },
            )
            .expect("override should estimate cost");

        assert_eq!(estimate, 66);
    }

    #[test]
    fn usage_pricing_config_error_display_pins_owned_variant_and_parse_template() {
        assert_eq!(
            UsagePricingConfigError::MissingField("input_micro_dollars_per_mtok").to_string(),
            "usage pricing config is missing `input_micro_dollars_per_mtok`",
        );

        let parse_err = UsagePricingConfigError::ParseToml(
            toml::from_str::<toml::Value>("invalid = ").unwrap_err(),
        );
        let rendered = parse_err.to_string();
        assert!(
            rendered.starts_with("failed to parse usage pricing config: "),
            "got: {rendered}",
        );
    }

    /// `UsagePrice::new` initialises both required fields and leaves
    /// cached/reasoning overrides at `None` (so they default to the
    /// input/output rates respectively at estimation time).
    #[test]
    fn usage_price_new_constructor_leaves_overrides_none() {
        let price = UsagePrice::new(10, 20);
        assert_eq!(price.input_micro_dollars_per_mtok, 10);
        assert_eq!(price.output_micro_dollars_per_mtok, 20);
        assert!(price.cached_micro_dollars_per_mtok.is_none());
        assert!(price.reasoning_micro_dollars_per_mtok.is_none());
    }

    /// Builder methods set the respective override fields.
    #[test]
    fn usage_price_builders_set_override_fields() {
        let price = UsagePrice::new(10, 20)
            .with_cached_micro_dollars_per_mtok(2)
            .with_reasoning_micro_dollars_per_mtok(30);
        assert_eq!(price.cached_micro_dollars_per_mtok, Some(2));
        assert_eq!(price.reasoning_micro_dollars_per_mtok, Some(30));
    }

    /// All-zero summary must estimate as 0 micro-dollars. No overflow,
    /// no division-by-zero, no panic.
    #[test]
    fn estimate_micro_dollars_zero_summary_yields_zero() {
        let price = UsagePrice::new(10, 20);
        let summary = CompletionUsageSummary::default();
        assert_eq!(price.estimate_micro_dollars(&summary), Some(0));
    }

    /// When `cached_micro_dollars_per_mtok` is `None`, cached tokens
    /// must be billed at the *input* rate, NOT the output rate. Pin
    /// this default so a future refactor that flips the fallback
    /// (e.g. to output rate) breaks here.
    #[test]
    fn estimate_micro_dollars_cached_defaults_to_input_rate() {
        // No cached override: cached tokens should be billed at the
        // input rate (10). With 1M input tokens of which 0.5M are
        // cached: uncached_input = 0.5M (billed at 10) + cached =
        // 0.5M (billed at 10) = 10 micro-dollars total.
        let price = UsagePrice::new(10, 20);
        let summary = CompletionUsageSummary {
            input_tokens: 1_000_000,
            output_tokens: 0,
            cached_tokens: Some(500_000),
            reasoning_tokens: None,
            total_tokens: None,
            cost_usd: None,
        };
        assert_eq!(price.estimate_micro_dollars(&summary), Some(10));
    }

    /// When `reasoning_micro_dollars_per_mtok` is `None`, reasoning
    /// tokens must be billed at the *output* rate. Pin the default
    /// fallback.
    #[test]
    fn estimate_micro_dollars_reasoning_defaults_to_output_rate() {
        // No reasoning override: 1M reasoning tokens billed at the
        // output rate (20).
        let price = UsagePrice::new(10, 20);
        let summary = CompletionUsageSummary {
            input_tokens: 0,
            output_tokens: 0,
            cached_tokens: None,
            reasoning_tokens: Some(1_000_000),
            total_tokens: None,
            cost_usd: None,
        };
        assert_eq!(price.estimate_micro_dollars(&summary), Some(20));
    }

    /// `cached_tokens` must be clamped to `input_tokens` to prevent
    /// double-counting. If a provider returns cached > input (a bug),
    /// the billing math must not credit phantom cached tokens.
    #[test]
    fn estimate_micro_dollars_cached_clamped_to_input_tokens() {
        let price = UsagePrice::new(10, 20).with_cached_micro_dollars_per_mtok(2);
        // input=100k, but provider claims 500k cached → clamp to 100k.
        // uncached_input = 0; cached = 100k at rate 2 = 0.2 micro.
        // Rounded to nearest = 0.
        let summary = CompletionUsageSummary {
            input_tokens: 100_000,
            output_tokens: 0,
            cached_tokens: Some(500_000),
            reasoning_tokens: None,
            total_tokens: None,
            cost_usd: None,
        };
        // 100k * 2 / 1M with banker's rounding = 0.
        assert_eq!(price.estimate_micro_dollars(&summary), Some(0));
    }

    /// `UsagePriceTable::with_override` takes precedence over the
    /// built-in capability-price lookup. Pin so a future "merge"
    /// refactor doesn't accidentally fall back when an override exists.
    #[test]
    fn price_table_override_takes_precedence_over_capability_default() {
        let summary = CompletionUsageSummary {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            cached_tokens: None,
            reasoning_tokens: None,
            total_tokens: None,
            cost_usd: None,
        };
        let table = UsagePriceTable::new().with_override(
            "openai",
            "fake-model-not-in-capability",
            UsagePrice::new(7, 11),
        );
        // 1M * 7 + 1M * 11 = 18 micro-dollars.
        assert_eq!(
            table.estimate_micro_dollars("openai", "fake-model-not-in-capability", &summary),
            Some(18),
        );
    }

    /// `UsagePriceTable::is_empty` returns true on a fresh table and
    /// false after at least one override is added.
    #[test]
    fn price_table_is_empty_tracks_overrides() {
        let mut table = UsagePriceTable::new();
        assert!(table.is_empty());
        table = table.with_override("p", "m", UsagePrice::new(1, 2));
        assert!(!table.is_empty());
    }

    /// `usd_per_mtok_to_micro_dollars` rejects invalid inputs and
    /// converts valid USD-per-million-tokens to micro-dollars via
    /// `* 1e6` rounded half-away-from-zero.
    #[test]
    fn usd_per_mtok_to_micro_dollars_validity_and_rounding() {
        // Happy path: 0.001 USD/M = 1_000 micro/M.
        assert_eq!(usd_per_mtok_to_micro_dollars(0.001), Some(1_000));
        // Zero is valid.
        assert_eq!(usd_per_mtok_to_micro_dollars(0.0), Some(0));
        // Rounding half-away-from-zero.
        assert_eq!(usd_per_mtok_to_micro_dollars(0.0000015), Some(2));

        // Rejection cases.
        assert_eq!(usd_per_mtok_to_micro_dollars(-1.0), None);
        assert_eq!(usd_per_mtok_to_micro_dollars(f64::NAN), None);
        assert_eq!(usd_per_mtok_to_micro_dollars(f64::INFINITY), None);
        assert_eq!(usd_per_mtok_to_micro_dollars(f64::NEG_INFINITY), None);
        // u64 overflow.
        let huge = (u64::MAX as f64) / 1_000_000.0 * 10.0;
        assert_eq!(usd_per_mtok_to_micro_dollars(huge), None);
    }

    /// `price_tokens` applies banker's-style half-add rounding via
    /// the `+ TOKENS_PER_MILLION/2` adjustment before division. Pin
    /// the rounding direction with a deliberate half-boundary value.
    #[test]
    fn price_tokens_rounds_via_half_adjustment() {
        // 500_000 tokens at 1 micro/M = 0.5 micro → rounds to 1.
        assert_eq!(price_tokens(500_000, 1), 1);
        // 499_999 tokens at 1 micro/M = 0.499... → rounds to 0.
        assert_eq!(price_tokens(499_999, 1), 0);
        // Exact integer: 1M tokens at 7 micro = 7.
        assert_eq!(price_tokens(1_000_000, 7), 7);
        // Zero tokens or zero rate → 0.
        assert_eq!(price_tokens(0, 100), 0);
        assert_eq!(price_tokens(1_000_000, 0), 0);
    }

    /// A `UsagePriceConfig` missing the required `input` field must
    /// surface `MissingField("input_micro_dollars_per_mtok")`.
    #[test]
    fn usage_price_config_missing_input_field_fails_with_named_error() {
        let err = UsagePriceTable::from_project_config_toml(
            r#"
            [usage.pricing.openai."gpt-test"]
            output_micro_dollars_per_mtok = 20
            "#,
        )
        .expect_err("missing input field must fail");
        match err {
            UsagePricingConfigError::MissingField(name) => {
                assert_eq!(name, "input_micro_dollars_per_mtok");
            }
            other => panic!("expected MissingField, got {other:?}"),
        }
    }

    /// Same as above for the `output` field.
    #[test]
    fn usage_price_config_missing_output_field_fails_with_named_error() {
        let err = UsagePriceTable::from_project_config_toml(
            r#"
            [usage.pricing.openai."gpt-test"]
            input_micro_dollars_per_mtok = 10
            "#,
        )
        .expect_err("missing output field must fail");
        match err {
            UsagePricingConfigError::MissingField(name) => {
                assert_eq!(name, "output_micro_dollars_per_mtok");
            }
            other => panic!("expected MissingField, got {other:?}"),
        }
    }
}
