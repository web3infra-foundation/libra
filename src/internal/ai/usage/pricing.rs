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
}
